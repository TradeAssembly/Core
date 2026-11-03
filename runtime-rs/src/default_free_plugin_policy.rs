//! Host release eligibility for exact pinned pure calculations, never upstream
//! data availability, credential/feed entitlement or trading authority.
use crate::ports::{EntitlementGrant, PluginCatalogEntry, PluginOperationContract};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::OnceLock};

pub(crate) const SOURCE_TYPE: &str = "local_default_free_plugin_v1";
pub(crate) const POLICY_SHA256: &str =
    "be881e9e4838c0ded22a1a76086ecbcd1e815cec38ba88e9c09138a48d0e4683";
const POLICY_BYTES: &[u8] = include_bytes!("../policies/default-free-plugin-policy.v1.json");
const BINDING_SCHEMA: &str = "tradeassembly.local_default_free_plugin_binding.v1";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    Paper,
    Research,
    Backtest,
}
impl Mode {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Research => "research",
            Self::Backtest => "backtest",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Policy {
    pub schema_version: String,
    pub policy_id: String,
    pub source_type: String,
    pub profile: String,
    pub packages: Vec<PackageRule>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PackageRule {
    pub plugin_ref: String,
    pub version: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub modes: Vec<Mode>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceBinding {
    pub schema_version: String,
    pub policy_sha256: String,
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub version: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub operation_id: String,
    pub operation_sha256: String,
    pub operation_data_sha256: String,
    pub modes: Vec<Mode>,
}
fn label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|b| b.is_ascii_graphic())
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn modes(values: &[Mode]) -> bool {
    !values.is_empty()
        && values.len() <= 3
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
pub(crate) fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(value)
        .map_err(|_| "default_free_policy_encoding_invalid".into())
}
fn parse(bytes: &[u8]) -> Result<Policy, String> {
    if bytes.is_empty() || bytes.len() > 16384 {
        return Err("default_free_policy_bounds_invalid".into());
    }
    let policy: Policy =
        serde_json::from_slice(bytes).map_err(|_| "default_free_policy_invalid")?;
    if policy.schema_version != "tradeassembly.local_default_free_plugin_policy.v1"
        || policy.policy_id != "tradeassembly.default-calculations.local-free.v1"
        || policy.source_type != SOURCE_TYPE
        || policy.profile != "local_full"
        || policy.packages.is_empty()
        || policy.packages.len() > 8
        || policy.packages.iter().any(|p| {
            !label(&p.plugin_ref)
                || !label(&p.version)
                || !digest(&p.package_sha256)
                || !digest(&p.manifest_sha256)
                || !modes(&p.modes)
        })
        || policy
            .packages
            .iter()
            .map(|p| {
                (
                    &p.plugin_ref,
                    &p.version,
                    &p.package_sha256,
                    &p.manifest_sha256,
                )
            })
            .collect::<BTreeSet<_>>()
            .len()
            != policy.packages.len()
    {
        return Err("default_free_policy_invalid".into());
    }
    Ok(policy)
}
pub(crate) fn compiled_policy() -> Result<&'static Policy, String> {
    static POLICY: OnceLock<Result<Policy, String>> = OnceLock::new();
    POLICY
        .get_or_init(|| {
            if format!("{:x}", Sha256::digest(POLICY_BYTES)) != POLICY_SHA256 {
                return Err("default_free_policy_pin_invalid".into());
            }
            parse(POLICY_BYTES)
        })
        .as_ref()
        .map_err(Clone::clone)
}
pub(crate) fn is_host_source_type(source_type: &str) -> bool {
    source_type == SOURCE_TYPE
}

/// Candidate identity only. Normal resolver deny/state/revocation/expiry and
/// requested-mode handling remain authoritative, including stored overrides.
pub(crate) fn grant_matches_candidate(
    grant: &EntitlementGrant,
    plugin: &PluginCatalogEntry,
    operation: &PluginOperationContract,
) -> bool {
    let Ok(policy) = compiled_policy() else {
        return false;
    };
    if !is_host_source_type(&grant.source_type)
        || grant.profile != policy.profile
        || grant.scope != "plugin_instance"
        || grant.source_ref.as_deref() != Some(format!("sha256:{POLICY_SHA256}").as_str())
        || !plugin.enabled
        || plugin.account_ref.is_some()
        || !operation_is_pure(operation)
        || grant.capability != operation.capability
        || grant.limits.len() != 2
        || !plugin
            .capabilities
            .iter()
            .any(|capability| capability == &operation.capability)
        || !plugin
            .operations
            .iter()
            .any(|candidate| candidate == operation)
    {
        return false;
    }
    let Some(raw) = grant.limits.get("sourceBinding") else {
        return false;
    };
    if serde_json::to_vec(raw).map_or(true, |bytes| bytes.len() > 8192) {
        return false;
    }
    let Ok(binding) = serde_json::from_value::<SourceBinding>(raw.clone()) else {
        return false;
    };
    let Some(rule) = policy.packages.iter().find(|p| {
        p.plugin_ref == binding.plugin_ref
            && p.version == binding.version
            && p.package_sha256 == binding.package_sha256
            && p.manifest_sha256 == binding.manifest_sha256
    }) else {
        return false;
    };
    binding.schema_version == BINDING_SCHEMA
        && binding.policy_sha256 == POLICY_SHA256
        && label(&binding.plugin_instance_ref)
        && label(&binding.operation_id)
        && digest(&binding.operation_sha256)
        && digest(&binding.operation_data_sha256)
        && binding.plugin_instance_ref == plugin.instance_ref
        && binding.plugin_ref == plugin.plugin_ref
        && binding.manifest_sha256 == plugin.manifest_fingerprint
        && binding.operation_id == operation.id
        && hash(operation).is_ok_and(|sha| sha == binding.operation_sha256)
        && modes(&binding.modes)
        && binding.modes.iter().all(|mode| {
            rule.modes.contains(mode)
                && operation
                    .traits
                    .modes
                    .iter()
                    .any(|declared| declared == mode.name())
        })
        && grant.limits.get("modes") == Some(&json!(binding.modes))
        && grant_id(&binding).is_ok_and(|id| id == grant.grant_id)
}
pub(crate) fn operation_is_pure(operation: &PluginOperationContract) -> bool {
    operation.effect == "read"
        && operation.apf_action_id == "data.calculate"
        && operation.resource_type == "data"
        && operation.finance_resource_type == "data"
        && operation.risk == "low"
        && operation.no_advice
        && !operation.credential_grant_required
        && !operation.account_binding_required
        && !operation.mandate_required
        && operation.traits.deterministic
        && operation.traits.replayable
        && matches!(operation.protocol.as_str(), "local-rust" | "stdio")
}
pub(crate) fn grant_id(binding: &SourceBinding) -> Result<String, String> {
    Ok(format!("grant:{SOURCE_TYPE}:{}", hash(binding)?))
}
pub(crate) fn binding(
    rule: &PackageRule,
    instance: &str,
    operation: &PluginOperationContract,
    data_sha256: String,
    modes: Vec<Mode>,
) -> Result<SourceBinding, String> {
    if !label(instance)
        || !operation_is_pure(operation)
        || !digest(&data_sha256)
        || !crate::default_free_plugin_policy::modes(&modes)
    {
        return Err("default_free_binding_invalid".into());
    }
    Ok(SourceBinding {
        schema_version: BINDING_SCHEMA.into(),
        policy_sha256: POLICY_SHA256.into(),
        plugin_instance_ref: instance.into(),
        plugin_ref: rule.plugin_ref.clone(),
        version: rule.version.clone(),
        package_sha256: rule.package_sha256.clone(),
        manifest_sha256: rule.manifest_sha256.clone(),
        operation_id: operation.id.clone(),
        operation_sha256: hash(operation)?,
        operation_data_sha256: data_sha256,
        modes,
    })
}
pub(crate) fn grant(
    policy: &Policy,
    binding: SourceBinding,
    capability: &str,
) -> Result<EntitlementGrant, String> {
    Ok(EntitlementGrant { grant_id: grant_id(&binding)?, profile: policy.profile.clone(), source: SOURCE_TYPE.into(),
        capability: capability.into(), effect: "allow".into(), state: "active".into(), scope: "plugin_instance".into(),
        source_type: SOURCE_TYPE.into(), source_ref: Some(format!("sha256:{POLICY_SHA256}")), precedence: 0,
        limits: [("modes".into(), json!(binding.modes)), ("sourceBinding".into(), json!(binding))].into_iter().collect(),
        issued_at: None, expires_at: None, revoked_at: None,
        reason: Some("host release eligibility for the exact owned default calculation instance; no upstream data or trading entitlement".into()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    fn operation() -> PluginOperationContract {
        crate::plugin_catalog::operation_from_manifest(&json!({"id":"synthetic.calculate","capability":"synthetic.calculate@1",
            "protocol":"local-rust","effect":"read","riskClass":"low","resourceType":"data",
            "financeAction":{"actionId":"data.calculate","resourceType":"data"},"noAdvice":true,
            "credentialGrantRequired":false,"accountBindingRequired":false,"mandateRequired":false,
            "traits":{"deterministic":true,"replayable":true,"modes":["paper","research","backtest"]}})).unwrap()
    }
    fn candidate(operation: &PluginOperationContract) -> PluginCatalogEntry {
        let rule = &compiled_policy().unwrap().packages[0];
        PluginCatalogEntry {
            instance_ref: "synthetic-owned-instance".into(),
            plugin_ref: rule.plugin_ref.clone(),
            provider_ref: "synthetic".into(),
            account_ref: None,
            name: "synthetic policy matcher unit".into(),
            enabled: true,
            trust_level: "test".into(),
            manifest_fingerprint: rule.manifest_sha256.clone(),
            capabilities: vec![operation.capability.clone()],
            operations: vec![operation.clone()],
            credential_status: crate::ports::CredentialResolutionStatus {
                configured: false,
                custody: "none".into(),
                status: "missing".into(),
                revision_ref: None,
            },
            health: "ready".into(),
        }
    }
    #[test]
    fn compiled_release_policy_is_byte_pinned_and_closed() {
        let policy = compiled_policy().unwrap();
        assert_eq!(policy.packages.len(), 1);
        assert_eq!(policy.packages[0].version, "0.74.1");
        let mut wrong: Value = serde_json::from_slice(POLICY_BYTES).unwrap();
        wrong["callerAllowance"] = json!(true);
        assert!(parse(&serde_json::to_vec(&wrong).unwrap()).is_err());
        wrong.as_object_mut().unwrap().remove("callerAllowance");
        wrong["packages"][0]["modes"] = json!(["live"]);
        assert!(parse(&serde_json::to_vec(&wrong).unwrap()).is_err());
        assert!(is_host_source_type(SOURCE_TYPE));
        assert!(!is_host_source_type("local_default_free_plugin_v2"));
    }
    #[test]
    fn source_binding_cannot_transfer_to_another_candidate_or_operation() {
        let policy = compiled_policy().unwrap();
        let operation = operation();
        let plugin = candidate(&operation);
        let bound = binding(
            &policy.packages[0],
            &plugin.instance_ref,
            &operation,
            "c".repeat(64),
            vec![Mode::Paper, Mode::Research, Mode::Backtest],
        )
        .unwrap();
        let grant = grant(policy, bound, &operation.capability).unwrap();
        assert!(grant_matches_candidate(&grant, &plugin, &operation));
        for field in ["instance", "plugin", "manifest", "disabled", "account"] {
            let mut wrong = plugin.clone();
            match field {
                "instance" => wrong.instance_ref = "other".into(),
                "plugin" => wrong.plugin_ref = "other".into(),
                "manifest" => wrong.manifest_fingerprint = "e".repeat(64),
                "disabled" => wrong.enabled = false,
                _ => wrong.account_ref = Some("not-an-account-grant".into()),
            }
            assert!(!grant_matches_candidate(&grant, &wrong, &operation));
        }
        let mut other = operation.clone();
        other.id = "other".into();
        assert!(!grant_matches_candidate(&grant, &plugin, &other));
        let mut denied = grant.clone();
        denied.effect = "deny".into();
        denied.state = "denied".into();
        assert!(
            grant_matches_candidate(&denied, &plugin, &operation),
            "normal resolver must still see a bound explicit deny"
        );
        let mut revoked = grant.clone();
        revoked.revoked_at = Some("epoch-ms:1".into());
        revoked.state = "revoked".into();
        assert!(
            grant_matches_candidate(&revoked, &plugin, &operation),
            "identity matcher must not replace revocation/state evaluation"
        );
        let mut live = grant;
        live.limits.insert("modes".into(), json!(["live"]));
        assert!(!grant_matches_candidate(&live, &plugin, &operation));
    }
    #[test]
    fn resolver_applies_free_eligibility_per_candidate_and_preserves_denials() {
        use crate::ports::{CapabilityRequirement, CapabilityResolverCatalog};
        let policy = compiled_policy().unwrap();
        let operation = operation();
        let plugin = candidate(&operation);
        let bound = binding(
            &policy.packages[0],
            &plugin.instance_ref,
            &operation,
            "c".repeat(64),
            vec![Mode::Paper, Mode::Research, Mode::Backtest],
        )
        .unwrap();
        let allowed = grant(policy, bound, &operation.capability).unwrap();
        let mut foreign = plugin.clone();
        foreign.instance_ref = "foreign".into();
        let requirement = CapabilityRequirement {
            capability: operation.capability.clone(),
            mode: "paper".into(),
            ..Default::default()
        };
        let resolve = |requirement, grants| {
            crate::capability::resolve(
                requirement,
                CapabilityResolverCatalog {
                    plugins: vec![plugin.clone(), foreign.clone()],
                    entitlements: grants,
                },
            )
        };
        let result = resolve(requirement.clone(), vec![allowed.clone()]);
        assert!(result.ok);
        assert_eq!(
            result
                .candidates
                .iter()
                .filter(|c| c.entitlement_decision.decision == "allow")
                .count(),
            1
        );
        assert_eq!(
            result
                .candidates
                .iter()
                .find(|c| c.plugin_instance_ref == "foreign")
                .unwrap()
                .entitlement_decision
                .decision,
            "deny"
        );
        let mut live = requirement.clone();
        live.mode = "live".into();
        assert!(!resolve(live, vec![allowed.clone()]).ok);
        for state in ["revoked", "expired"] {
            let mut stopped = allowed.clone();
            stopped.state = state.into();
            assert!(!resolve(requirement.clone(), vec![stopped]).ok);
        }
        let mut expired = allowed.clone();
        expired.expires_at = Some("2000-01-01T00:00:00Z".into());
        assert!(!resolve(requirement.clone(), vec![expired]).ok);
        let mut deny = allowed.clone();
        deny.grant_id = "stored-explicit-deny".into();
        deny.source_type = "host_owned".into();
        deny.effect = "deny".into();
        assert!(!resolve(requirement, vec![allowed, deny]).ok);
    }

    #[test]
    fn calculation_identity_does_not_admit_provider_broker_or_privileged_operations() {
        let good = operation();
        assert!(operation_is_pure(&good));
        for change in 0..6 {
            let mut wrong = good.clone();
            match change {
                0 => wrong.apf_action_id = "market_data.read".into(),
                1 => wrong.effect = "write".into(),
                2 => wrong.credential_grant_required = true,
                3 => wrong.account_binding_required = true,
                4 => wrong.mandate_required = true,
                _ => wrong.traits.replayable = false,
            };
            assert!(!operation_is_pure(&wrong));
        }
    }
}
