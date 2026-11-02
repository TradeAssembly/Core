//! Ephemeral source-bound eligibility, derived only from currently owned,
//! enabled instances and a host-pinned fully verified calculation package.
use super::TradeAssemblyService;
use crate::{default_free_plugin_policy as policy, ports::*};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(serde::Serialize)]
pub(super) struct EligiblePackage {
    metadata: VerifiedPackageMetadataSnapshot,
    operations: Vec<PluginOperationContract>,
}
fn none_only(value: &Value) -> bool {
    value.as_array().is_some_and(|items| {
        items.len() <= 1
            && items.iter().all(|item| {
                item == "none"
                    || item.as_object().is_some_and(|object| {
                        object.get("id").and_then(Value::as_str) == Some("none")
                            && object
                                .keys()
                                .all(|key| matches!(key.as_str(), "id" | "description"))
                    })
            })
    })
}
fn verified(
    metadata: VerifiedPackageMetadataSnapshot,
    rule: &policy::PackageRule,
) -> Option<EligiblePackage> {
    let package = &metadata.package;
    let manifest = &package.manifest;
    if package.plugin_ref != rule.plugin_ref
        || package.version != rule.version
        || package.package_sha256 != rule.package_sha256
        || package.manifest_sha256 != rule.manifest_sha256
        || policy::hash(manifest).ok().as_deref() != Some(rule.manifest_sha256.as_str())
        || manifest["metadata"]["id"] != rule.plugin_ref
        || manifest["metadata"]["version"] != rule.version
        || !none_only(&manifest["permissions"])
        || !none_only(&manifest["hostApis"])
        || !manifest["health"]["requiredCredentials"]
            .as_array()
            .is_some_and(Vec::is_empty)
        || !manifest["configuration"]["fields"]
            .as_array()
            .is_some_and(|fields| {
                fields
                    .iter()
                    .all(|field| field["storageClass"] != "credential")
            })
    {
        return None;
    }
    let declared = manifest["operations"].as_array()?;
    if declared.len() > tradeassembly_plugin_sdk::data_package::MAX_MANIFEST_OPERATIONS {
        return None;
    }
    let mut operations = Vec::new();
    for raw in declared {
        // Do not let the catalog parser's defaults infer explicit purity.
        if raw["effect"] != "read"
            || raw["credentialGrantRequired"] != false
            || raw["accountBindingRequired"] != false
            || raw["mandateRequired"] != false
            || raw["traits"]["deterministic"] != true
            || raw["traits"]["replayable"] != true
        {
            continue;
        }
        let Some(operation) = crate::plugin_catalog::operation_from_manifest(raw) else {
            continue;
        };
        if !policy::operation_is_pure(&operation)
            || !manifest["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|cap| cap["id"] == operation.capability))
        {
            continue;
        }
        let data = metadata.contracts.get(&operation.id)?;
        if data.operation_id != operation.id {
            return None;
        }
        operations.push(operation);
    }
    Some(EligiblePackage {
        metadata,
        operations,
    })
}
#[cfg(test)]
fn grants(
    service: &TradeAssemblyService,
    requested_capability: Option<&str>,
) -> Vec<EntitlementGrant> {
    let capabilities =
        requested_capability.map(|capability| BTreeSet::from([capability.to_string()]));
    grants_matching(service, capabilities.as_ref())
}

pub(super) fn grants_matching(
    service: &TradeAssemblyService,
    requested_capabilities: Option<&BTreeSet<String>>,
) -> Vec<EntitlementGrant> {
    let _timing = super::order_plans::financial_timing("entitlement.free_total");
    // The ordinary ownership helper deliberately permits internal ownerless
    // calls. New product eligibility requires an actual authenticated owner.
    if service.invocation_owner().is_none() {
        return Vec::new();
    }
    let Ok(host_policy) = policy::compiled_policy() else {
        return Vec::new();
    };
    // Invalid/unresolved optional nodes still belong to resolver diagnostics.
    // They must not remove eligibility for unrelated valid required nodes.
    let valid_requested = requested_capabilities.map(|caps| {
        caps.iter()
            .filter(|cap| !cap.is_empty() && cap.len() <= 256)
            .cloned()
            .collect::<BTreeSet<_>>()
    });
    let requested_capabilities = valid_requested.as_ref();
    let runtime = service.runtime();
    let Ok(instances) = runtime.plugins.list_instances() else {
        return Vec::new();
    };
    if instances.len() > 4096 {
        return Vec::new();
    }
    let mut packages: BTreeMap<String, Option<Arc<EligiblePackage>>> = BTreeMap::new();
    let mut result = Vec::new();
    for instance in instances {
        let Some(instance_ref) = instance["instanceRef"].as_str() else {
            continue;
        };
        if instance["enabled"] != true
            || instance
                .get("removedAtMs")
                .is_some_and(|value| !value.is_null())
            || instance
                .get("accountRef")
                .is_some_and(|value| !value.is_null())
            || service
                .require_object("plugin_instance", instance_ref)
                .is_err()
        {
            continue;
        }
        let Some(rule) = host_policy.packages.iter().find(|rule| {
            instance["pluginRef"] == rule.plugin_ref
                && instance["activePackageSha256"] == rule.package_sha256
        }) else {
            continue;
        };
        let Ok(Some(record)) = runtime.plugins.get_manifest(&rule.plugin_ref) else {
            continue;
        };
        if record["pluginRef"] != rule.plugin_ref
            || record["manifestDigest"] != rule.manifest_sha256
            || record["package"]["packageSha256"] != rule.package_sha256
            || record["package"]["version"] != rule.version
        {
            continue;
        }
        // Cheap exact declared-capability prefilter BEFORE installed-byte work.
        if requested_capabilities.is_some_and(|requested| {
            !record["manifest"]["capabilities"]
                .as_array()
                .is_some_and(|caps| {
                    caps.iter().any(|declared| {
                        declared["id"]
                            .as_str()
                            .is_some_and(|cap| requested.contains(cap))
                    })
                })
        }) {
            continue;
        }
        let loaded = packages
            .entry(rule.package_sha256.clone())
            .or_insert_with(|| {
                let load = || {
                    runtime
                        .plugin_packages
                        .verified_metadata_snapshot(&rule.package_sha256)
                        .ok()
                        .flatten()
                        .and_then(|metadata| {
                            super::order_plans::measured("entitlement.free_semantic", || {
                                verified(metadata, rule)
                            })
                        })
                };
                if let Some(metadata) = &service.graph_metadata {
                    // Only package-derived purity facts are reused. Every call
                    // still checks owner, current instance/manifest and grants.
                    let witness = serde_json::to_value(rule).ok()?;
                    metadata.eligible.get(&rule.package_sha256, &witness, load)
                } else {
                    load().map(Arc::new)
                }
            });
        let Some(package) = loaded.as_ref() else {
            continue;
        };
        if record["manifest"] != package.metadata.package.manifest {
            continue;
        }
        for operation in &package.operations {
            if requested_capabilities.is_some_and(|caps| !caps.contains(&operation.capability)) {
                continue;
            }
            let Some(data) = package.metadata.contracts.get(&operation.id) else {
                continue;
            };
            let modes = rule
                .modes
                .iter()
                .filter(|mode| {
                    operation
                        .traits
                        .modes
                        .iter()
                        .any(|declared| declared == mode.name())
                        && data
                            .applicability
                            .modes
                            .iter()
                            .any(|declared| declared == mode.name())
                })
                .cloned()
                .collect::<Vec<_>>();
            if modes.is_empty() {
                continue;
            }
            let Ok(data_sha) = policy::hash(data) else {
                continue;
            };
            let Ok(binding) = policy::binding(rule, instance_ref, operation, data_sha, modes)
            else {
                continue;
            };
            let Ok(grant) = policy::grant(host_policy, binding, &operation.capability) else {
                continue;
            };
            result.push(grant);
            if result.len() > 65536 {
                return Vec::new();
            }
        }
    }
    result.sort_by(|left, right| left.grant_id.cmp(&right.grant_id));
    result.dedup_by(|left, right| left.grant_id == right.grant_id);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "requires exact MANAGED_MINUTE_TEST_PACKAGE; actual package lifecycle and synthetic owned store"]
    fn default_free_actual_package_is_owned_read_only_and_stored_overrides_win() {
        use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
        let (directory, owner) = installed_owned_calculation();
        let db = directory.path().join("eligibility.sqlite");
        let rule = &policy::compiled_policy().unwrap().packages[0];
        let cap = "indicator.ema.minute@1";
        let before = owner
            .runtime()
            .storage
            .list_json("entitlement_grants")
            .unwrap();
        let generated = grants(&owner, Some(cap));
        assert_eq!(generated.len(), 1);
        let selected_caps = BTreeSet::from([cap.to_string(), "indicator.tsi.minute@1".to_string()]);
        let selected = grants_matching(&owner, Some(&selected_caps));
        assert_eq!(selected.len(), 2);
        assert!(selected
            .iter()
            .all(|grant| selected_caps.contains(&grant.capability)));
        assert_eq!(
            selected.iter().find(|grant| grant.capability == cap),
            Some(&generated[0]),
            "request scope cannot change binding identity"
        );
        assert!(grants_matching(&owner, Some(&BTreeSet::new())).is_empty());
        assert_eq!(
            before,
            owner
                .runtime()
                .storage
                .list_json("entitlement_grants")
                .unwrap(),
            "eligibility lookup must not persist allows"
        );
        let foreign = owner.for_authenticated_invocation("fixture", "foreign-owner", None, None);
        assert!(grants(&foreign, Some(cap)).is_empty());
        assert!(grants(&owner, Some("broker.order_submit.live")).is_empty());
        let catalog = super::super::providers::plugin_catalog(&owner);
        let mut graph_request = CapabilityGraphRequest {
            requirements: selected_caps
                .iter()
                .enumerate()
                .map(|(index, capability)| CapabilityRequirement {
                    requirement_id: format!("root{index}"),
                    capability: capability.clone(),
                    mode: "paper".into(),
                    declaration: "required".into(),
                    fallback_policy: "none".into(),
                    plugin_instance_ref: Some("owned-calculation".into()),
                    ..Default::default()
                })
                .collect(),
            evaluation_epoch: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        };
        for (index, capability) in [String::new(), "unresolved".repeat(40)]
            .into_iter()
            .enumerate()
        {
            graph_request.requirements.push(CapabilityRequirement {
                requirement_id: format!("optional{index}"),
                capability,
                declaration: "optional".into(),
                mode: "paper".into(),
                fallback_policy: "none".into(),
                ..Default::default()
            });
        }
        let full = crate::capability::resolve_graph(
            graph_request.clone(),
            CapabilityResolverCatalog {
                plugins: catalog.clone(),
                entitlements: super::super::providers::entitlements(&owner),
            },
        );
        let scoped_grants =
            super::super::providers::entitlements_for_graph(&owner, &graph_request, &catalog);
        assert_eq!(
            scoped_grants
                .iter()
                .filter(|g| g.source_type == policy::SOURCE_TYPE)
                .count(),
            2
        );
        let scoped = crate::capability::resolve_graph(
            graph_request,
            CapabilityResolverCatalog {
                plugins: catalog.clone(),
                entitlements: scoped_grants,
            },
        );
        assert!(full.ok, "{full:?}");
        assert_eq!(
            scoped, full,
            "scoping must preserve selected bindings, diagnostics and immutable graph identity"
        );
        let plugin = catalog
            .iter()
            .find(|p| p.instance_ref == "owned-calculation")
            .unwrap();
        let operation = plugin
            .operations
            .iter()
            .find(|o| o.capability == cap)
            .unwrap();
        assert!(policy::grant_matches_candidate(
            &generated[0],
            plugin,
            operation
        ));
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("synthetic-stored-override").unwrap(),
        );
        let key = &generated[0].grant_id;
        owner
            .runtime()
            .storage
            .put_json(
                "entitlement_grants",
                key,
                serde_json::to_value(&generated[0]).unwrap(),
                &context,
            )
            .unwrap();
        assert!(!super::super::providers::entitlements(&foreign)
            .iter()
            .any(|g| g.source_type == policy::SOURCE_TYPE));
        let bare = TradeAssemblyService::test_local(db.to_str().unwrap());
        assert!(grants(&bare, Some(cap)).is_empty());
        for state in ["revoked", "expired"] {
            let mut stored = generated[0].clone();
            stored.state = state.into();
            owner
                .runtime()
                .storage
                .put_json(
                    "entitlement_grants",
                    key,
                    serde_json::to_value(stored).unwrap(),
                    &context,
                )
                .unwrap();
            let effective = super::super::providers::entitlements(&owner);
            assert_eq!(effective.iter().filter(|g| g.grant_id == *key).count(), 1);
            let resolved = crate::capability::resolve(
                CapabilityRequirement {
                    capability: cap.into(),
                    mode: "paper".into(),
                    plugin_instance_ref: Some(plugin.instance_ref.clone()),
                    ..Default::default()
                },
                CapabilityResolverCatalog {
                    plugins: catalog.clone(),
                    entitlements: effective,
                },
            );
            assert!(!resolved.ok, "stored {state} must not regenerate an allow");
        }
        owner
            .runtime()
            .storage
            .put_json(
                "entitlement_grants",
                key,
                json!({"malformed":true}),
                &context,
            )
            .unwrap();
        assert!(!super::super::providers::entitlements(&owner)
            .iter()
            .any(|g| g.source_type == policy::SOURCE_TYPE));
        let package = owner
            .runtime()
            .plugin_packages
            .get(&rule.package_sha256)
            .unwrap()
            .unwrap();
        let root = std::path::Path::new(&package.install_root);
        let contract = package.descriptor["dataContracts"][0]["document"]["path"]
            .as_str()
            .unwrap();
        let target = root.join(contract);
        std::fs::write(target, b"{}").unwrap();
        owner
            .runtime()
            .storage
            .put_json(
                "entitlement_grants",
                key,
                serde_json::to_value(&generated[0]).unwrap(),
                &context,
            )
            .unwrap();
        assert!(
            !super::super::providers::entitlements(&owner)
                .iter()
                .any(|g| g.source_type == policy::SOURCE_TYPE),
            "stored allow must not survive artifact tampering"
        );
        assert!(
            grants(&owner, Some(cap)).is_empty(),
            "all artifact bytes must still verify"
        );
    }
    fn installed_owned_calculation() -> (tempfile::TempDir, TradeAssemblyService) {
        let path =
            std::path::PathBuf::from(std::env::var_os("MANAGED_MINUTE_TEST_PACKAGE").unwrap());
        let rule = &policy::compiled_policy().unwrap().packages[0];
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("eligibility.sqlite");
        let owner = TradeAssemblyService::test_local(db.to_str().unwrap())
            .for_authenticated_invocation("fixture", "calculation-owner", None, None);
        let response = owner.handle_http("POST", "/plugins/packages", json!({
            "source":{"type":"package","locator":path,"package":{"type":"file","locator":path}},
            "integrity":{"packageSha256":rule.package_sha256,"manifestSha256":rule.manifest_sha256},
            "offline":true,"idempotencyKey":"free-actual-install"
        }));
        assert_eq!(response.status, 200, "{}", response.body);
        let response = owner.handle_http(
            "POST",
            "/plugins/instances",
            json!({
                "pluginRef":rule.plugin_ref,"instanceRef":"owned-calculation","enabled":false,
                "configuration":{},"idempotencyKey":"free-actual-create"
            }),
        );
        assert_eq!(response.status, 200, "{}", response.body);
        let response = owner.handle_http(
            "POST",
            "/plugins/instances/owned-calculation:enable",
            json!({"idempotencyKey":"free-actual-enable"}),
        );
        assert_eq!(response.status, 200, "{}", response.body);
        (directory, owner)
    }
    #[test]
    #[ignore = "requires exact MANAGED_MINUTE_TEST_PACKAGE; bounded43graph metadata profile, no financial actions"]
    fn actual_package_graph_metadata_profile_and_live_invalidations() {
        let (directory, owner) = installed_owned_calculation();
        let caps: BTreeSet<String> = BTreeSet::from([
            "indicator.ema.minute@1".into(),
            "indicator.tsi.minute@1".into(),
        ]);
        let request = CapabilityGraphRequest {
            requirements: caps
                .iter()
                .enumerate()
                .map(|(index, capability)| CapabilityRequirement {
                    requirement_id: format!("calculation{index}"),
                    capability: capability.clone(),
                    mode: "paper".into(),
                    declaration: "required".into(),
                    fallback_policy: "none".into(),
                    plugin_instance_ref: Some("owned-calculation".into()),
                    ..Default::default()
                })
                .collect(),
            evaluation_epoch: "2026-11-02T00:00:00Z".into(),
            ..Default::default()
        };
        let resolve = |s: &TradeAssemblyService| {
            let catalog = super::super::providers::plugin_catalog(s);
            let entitlements =
                super::super::providers::entitlements_for_graph(s, &request, &catalog);
            crate::capability::resolve_graph(
                request.clone(),
                CapabilityResolverCatalog {
                    plugins: catalog,
                    entitlements,
                },
            )
        };
        let measure = |s: &TradeAssemblyService| {
            let start = std::time::Instant::now();
            let mut graphs = Vec::new();
            for _ in 0..43 {
                let graph = resolve(s);
                assert!(graph.ok, "{graph:?}");
                graphs.push(graph);
            }
            (start.elapsed().as_micros(), graphs)
        };
        let (old_us, expected) = super::super::package_validation::with_round(&owner, |round| {
            let mut reference = round.clone();
            reference.graph_metadata = None;
            Ok(measure(&reference))
        })
        .unwrap();
        let (new_us, actual) =
            super::super::package_validation::with_round(&owner, |round| Ok(measure(round)))
                .unwrap();
        eprintln!("GRAPH_METADATA_MEASURE reference_us={old_us} memo_us={new_us}");
        assert_eq!(
            expected, actual,
            "pure reuse must preserve complete graph decisions and diagnostics"
        );
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("metadata-invalidation").unwrap(),
        );
        super::super::package_validation::with_round(&owner, |round| {
            assert!(resolve(round).ok);
            let foreign =
                round.for_authenticated_invocation("fixture", "foreign-owner", None, None);
            assert!(grants_matching(&foreign, Some(&caps)).is_empty());
            let instance = round
                .runtime()
                .plugins
                .get_instance("owned-calculation")?
                .unwrap();
            let mut disabled = instance.clone();
            disabled["enabled"] = serde_json::json!(false);
            round
                .runtime()
                .plugins
                .put_instance("owned-calculation", disabled, &ctx)?;
            assert!(!resolve(round).ok, "enablement cannot be memoized");
            let mut wrong_version = instance.clone();
            wrong_version["activePackageSha256"] = serde_json::json!("f".repeat(64));
            round
                .runtime()
                .plugins
                .put_instance("owned-calculation", wrong_version, &ctx)?;
            assert!(
                grants_matching(round, Some(&caps)).is_empty(),
                "changed package cannot reuse old purity"
            );
            round
                .runtime()
                .plugins
                .put_instance("owned-calculation", instance, &ctx)?;
            let rule = &policy::compiled_policy().unwrap().packages[0];
            let record = round
                .runtime()
                .plugins
                .get_manifest(&rule.plugin_ref)?
                .unwrap();
            let mut changed = record.clone();
            changed["manifest"]["operations"][0]["effect"] = serde_json::json!("write");
            round.runtime().storage.put_json(
                "plugin_manifests",
                &rule.plugin_ref,
                changed,
                &ctx,
            )?;
            assert!(
                grants_matching(round, Some(&caps)).is_empty(),
                "manifest tampering under old digest must refuse"
            );
            round
                .runtime()
                .storage
                .put_json("plugin_manifests", &rule.plugin_ref, record, &ctx)?;
            assert!(resolve(round).ok);
            let generated = grants_matching(round, Some(&caps));
            let mut revoked = generated[0].clone();
            revoked.state = "revoked".into();
            round.runtime().storage.put_json(
                "entitlement_grants",
                &revoked.grant_id,
                serde_json::to_value(&revoked).unwrap(),
                &ctx,
            )?;
            assert!(
                !resolve(round).ok,
                "stored revocation must still be read during cached round"
            );
            Ok(())
        })
        .unwrap();
        let recreated = TradeAssemblyService::test_local(
            directory
                .path()
                .join("eligibility.sqlite")
                .to_str()
                .unwrap(),
        )
        .for_authenticated_invocation("fixture", "calculation-owner", None, None);
        assert!(recreated.graph_metadata.is_none());
        assert!(
            !resolve(&recreated).ok,
            "revocation survives actual service recreation"
        );
        let report = serde_json::json!({"graphs":43,"referenceUs":old_us,"memoUs":new_us,"equalGraphs":true,"disabledPackageVersionTamperForeignAndRevocationRefuse":true,"serviceRecreationRevocationRefuses":true,"financialActions":0,"owningQualified":false});
        eprintln!("GRAPH_METADATA_PROFILE {report}");
        if let Some(path) = std::env::var_os("CORE_GRAPH_METADATA_BENCH_RESULT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
    }
    #[test]
    fn no_external_permission_or_host_api_is_inferred_from_missing_metadata() {
        assert!(none_only(&json!([])));
        assert!(none_only(&json!([{"id":"none","description":"pure"}])));
        for wrong in [
            Value::Null,
            json!([{"id":"network.outbound"}]),
            json!([{"id":"none","endpoint":"external"}]),
            json!(["none", "network.outbound"]),
        ] {
            assert!(!none_only(&wrong));
        }
    }
}
