//! Ephemeral source-bound eligibility, derived only from currently owned,
//! enabled instances and a host-pinned fully verified calculation package.
use super::TradeAssemblyService;
use crate::{default_free_plugin_policy as policy, ports::*};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

struct EligiblePackage {
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
pub(super) fn grants(
    service: &TradeAssemblyService,
    requested_capability: Option<&str>,
) -> Vec<EntitlementGrant> {
    // The ordinary ownership helper deliberately permits internal ownerless
    // calls. New product eligibility requires an actual authenticated owner.
    if service.invocation_owner().is_none() {
        return Vec::new();
    }
    let Ok(host_policy) = policy::compiled_policy() else {
        return Vec::new();
    };
    if requested_capability.is_some_and(|cap| cap.is_empty() || cap.len() > 256) {
        return Vec::new();
    }
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
        if requested_capability.is_some_and(|cap| {
            !record["manifest"]["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|declared| declared["id"] == cap))
        }) {
            continue;
        }
        let loaded = packages
            .entry(rule.package_sha256.clone())
            .or_insert_with(|| {
                runtime
                    .plugin_packages
                    .verified_metadata_snapshot(&rule.package_sha256)
                    .ok()
                    .flatten()
                    .and_then(|metadata| verified(metadata, rule))
                    .map(Arc::new)
            });
        let Some(package) = loaded.as_ref() else {
            continue;
        };
        if record["manifest"] != package.metadata.package.manifest {
            continue;
        }
        for operation in &package.operations {
            if requested_capability.is_some_and(|cap| cap != operation.capability) {
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
        let cap = "indicator.ema.minute@1";
        let before = owner
            .runtime()
            .storage
            .list_json("entitlement_grants")
            .unwrap();
        let generated = grants(&owner, Some(cap));
        assert_eq!(generated.len(), 1);
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
