// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Immutable structure for one pure typed-config reconstruction, never an
//! authorization cache. Initial and final graph currentness are independent.
use super::{revision_for_context, TradeAssemblyService};
use crate::ports::CapabilityGraphResolution;
use crate::service::data_plans::PlanScope;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PROVIDERS: usize = 64;
const MAX_WITNESS_BYTES: usize = 32 * 1_048_576;
const STALE: &str = "typed_execution_graph_stale";
type Owner = Option<(String, String)>;
type Providers = BTreeMap<(String, String), Provider>;

#[derive(PartialEq, Serialize)]
struct Provider {
    // Health is deliberately not memoized. Final graph resolution evaluates
    // current health; provider/account/financial validators remain unchanged.
    identity: Value,
    manifest: Option<Value>,
    credential_status: Option<Value>,
    ownership: Option<Value>,
}
fn owner(s: &TradeAssemblyService) -> Owner {
    s.invocation_owner()
        .map(|v| (v.issuer.clone(), v.subject.clone()))
}
fn epoch(s: &TradeAssemblyService) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(s.runtime().clock.trusted_now_ms()?)
        .map(|v| v.to_rfc3339())
        .ok_or_else(|| STALE.into())
}
fn providers(
    s: &TradeAssemblyService,
    graph: &CapabilityGraphResolution,
) -> Result<Option<Providers>, String> {
    let ids = graph
        .nodes
        .iter()
        .filter_map(|n| n.selected.as_ref())
        .map(|v| (v.plugin_instance_ref.clone(), v.plugin_ref.clone()))
        .collect::<BTreeSet<_>>();
    if ids.len() > MAX_PROVIDERS {
        return Ok(None);
    }
    let runtime = s.runtime();
    let mut result = BTreeMap::new();
    let mut bytes = 0usize;
    for (instance_id, plugin_id) in ids {
        let record = runtime.plugins.get_instance(&instance_id)?;
        // Compare the complete stored record, including future fields, without
        // persisting or exposing the witness in any response or diagnostic.
        let identity = record.clone().unwrap_or(Value::Null);
        let credential_status = record
            .as_ref()
            .and_then(|v| v["credentialRef"].as_str())
            .map(|id| {
                let status = runtime.credentials.status(id);
                json!({"providerRef": status.provider_ref,
                    "configured": status.configured, "custody": status.custody,
                    "redactedDisplay": status.redacted_display})
            });
        let witness = Provider {
            identity,
            credential_status,
            manifest: runtime.plugins.get_manifest(&plugin_id)?,
            ownership: runtime.storage.get_json(
                "object_authorization_scopes_v1",
                &format!("plugin_instance:{instance_id}"),
            )?,
        };
        let Some(size) =
            crate::service::package_validation::bounded_size(&witness, MAX_WITNESS_BYTES - bytes)
        else {
            return Ok(None);
        };
        bytes += size;
        result.insert((instance_id, plugin_id), witness);
    }
    Ok(Some(result))
}

/// Not Clone/Serialize/Deserialize and never constructed from a caller DTO.
/// References cannot escape with_current; consumers obtain only graph shape.
pub(crate) struct StructuralContext {
    scope: PlanScope,
    revision_id: String,
    owner: Owner,
    graph: CapabilityGraphResolution,
    providers: Providers,
}
impl StructuralContext {
    pub(crate) fn graph_for<'a>(
        &'a self,
        s: &TradeAssemblyService,
        scope: &PlanScope,
        revision_id: &str,
    ) -> Result<&'a CapabilityGraphResolution, String> {
        if self.scope != *scope || self.revision_id != revision_id || self.owner != owner(s) {
            return Err(STALE.into());
        }
        Ok(&self.graph)
    }
}

#[cfg(test)]
thread_local! { static REFERENCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(crate) fn with_reference_resolution<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            REFERENCE.with(|v| v.set(self.0));
        }
    }
    let _restore = Restore(REFERENCE.with(|v| v.replace(true)));
    f()
}

/// Pure reconstruction only: no provider invocation, persistence or C5 permit
/// may occur inside f. Live decisions are evaluated again before output escapes.
pub(crate) fn with_current<T>(
    s: &TradeAssemblyService,
    scope: &PlanScope,
    revision_id: &str,
    f: impl FnOnce(&Value, Option<&StructuralContext>) -> Result<T, String>,
) -> Result<T, String> {
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| STALE)?;
    let initial_owner = owner(s);
    let revision = revision_for_context(
        s,
        revision_id,
        &scope.strategy_id,
        &scope.strategy_version_id,
        "paper",
        &epoch(s)?,
    )
    .map_err(|_| STALE)?;
    if revision["strategySpecHash"] != scope.strategy_spec_hash {
        return Err(STALE.into());
    }
    // Exact old path for read-only comparison only, with each old helper's
    // independent current-resolution checks intact. No production switch.
    #[cfg(test)]
    if REFERENCE.with(|v| v.get()) {
        return f(&revision, None);
    }
    let graph: CapabilityGraphResolution =
        serde_json::from_value(revision["graph"].clone()).map_err(|_| STALE)?;
    let context = providers(s, &graph)?.map(|providers| StructuralContext {
        scope: scope.clone(),
        revision_id: revision_id.into(),
        owner: initial_owner.clone(),
        graph,
        providers,
    });
    // Oversized valid graphs keep the original helper resolution path.
    let result = f(&revision, context.as_ref())?;
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| STALE)?;
    let fresh = revision_for_context(
        s,
        revision_id,
        &scope.strategy_id,
        &scope.strategy_version_id,
        "paper",
        &epoch(s)?,
    )
    .map_err(|_| STALE)?;
    if owner(s) != initial_owner || fresh != revision {
        return Err(STALE.into());
    }
    if let Some(context) = context {
        if providers(s, &context.graph)?.as_ref() != Some(&context.providers) {
            return Err(STALE.into());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};

    fn effect() -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("graph-context-test").unwrap(),
        )
    }

    #[test]
    fn graph_context_is_scoped_and_equivalent_without_mutation() {
        let (_dir, service, scope, request) =
            crate::service::data_plans::tests::fixture_service(None);
        let result = with_current(
            &service,
            &scope,
            &request.capability_graph_revision,
            |revision, context| {
                let context = context.expect("bounded fixture");
                assert_eq!(
                    serde_json::to_value(context.graph_for(
                        &service,
                        &scope,
                        &request.capability_graph_revision
                    )?)
                    .unwrap(),
                    revision["graph"]
                );
                let mut wrong = scope.clone();
                wrong.strategy_id.push_str("-foreign");
                assert!(context
                    .graph_for(&service, &wrong, &request.capability_graph_revision)
                    .is_err());
                assert!(context
                    .graph_for(&service, &scope, "wrong-revision")
                    .is_err());
                Ok(17)
            },
        );
        assert_eq!(result.unwrap(), 17);
    }

    #[test]
    fn graph_context_refuses_mid_reconstruction_provider_changes() {
        for field in [
            "configuration",
            "enabled",
            "credentialRevision",
            "futureIdentityField",
        ] {
            let (_dir, service, scope, request) =
                crate::service::data_plans::tests::fixture_service(None);
            let result = with_current(
                &service,
                &scope,
                &request.capability_graph_revision,
                |_, context| {
                    let context = context.expect("bounded fixture");
                    let id = context
                        .providers
                        .keys()
                        .find(|(id, _)| id == "instance.provide")
                        .expect("source provider")
                        .0
                        .clone();
                    let runtime = service.runtime();
                    let mut instance = runtime.plugins.get_instance(&id)?.unwrap();
                    instance[field] = match field {
                        "configuration" => json!({"changed":true}),
                        "enabled" => json!(false),
                        _ => json!("changed"),
                    };
                    runtime.plugins.put_instance(&id, instance, &effect())?;
                    Ok(17)
                },
            );
            assert!(result.is_err(), "mutation escaped: {field}");
        }
    }

    #[test]
    fn graph_context_refuses_revocation_before_output_escapes() {
        let (_dir, service, scope, request) =
            crate::service::data_plans::tests::fixture_service(None);
        let result = with_current(
            &service,
            &scope,
            &request.capability_graph_revision,
            |_, _| {
                let runtime = service.runtime();
                let mut grant = runtime
                    .storage
                    .get_json("entitlement_grants", "test.provide")?
                    .unwrap();
                grant["state"] = json!("revoked");
                runtime
                    .storage
                    .put_json("entitlement_grants", "test.provide", grant, &effect())?;
                Ok(17)
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn reference_resolution_scope_restores_after_nested_unwind() {
        assert!(!REFERENCE.with(|v| v.get()));
        let result = std::panic::catch_unwind(|| {
            with_reference_resolution(|| {
                assert!(REFERENCE.with(|v| v.get()));
                with_reference_resolution(|| assert!(REFERENCE.with(|v| v.get())));
                panic!("fixture unwind");
            })
        });
        assert!(result.is_err());
        assert!(!REFERENCE.with(|v| v.get()));
    }
}
