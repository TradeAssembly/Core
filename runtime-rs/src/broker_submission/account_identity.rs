//! Canonical account identity is derived by the host, never accepted as a caller hash.
use crate::ports::{CredentialPort, PluginPackagePort, PluginRegistryPort};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalAccountIdentity {
    pub schema_version: String,
    pub broker_namespace: String,
    pub environment: String,
    pub provider_account_id: String,
    pub identity_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountWitness {
    pub identity: CanonicalAccountIdentity,
    pub binding_sha256: String,
    pub account_ref: String,
}
fn hash(v: &Value) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(v)
        .map_err(|_| "canonical_account_encoding_invalid".into())
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .ok_or_else(|| "canonical_account_binding_invalid".into())
}
fn derive(
    record: &Value,
    manifest: &Value,
    selected: &Value,
    now: i64,
    mode: &str,
) -> Result<CanonicalAccountIdentity, String> {
    if !crate::live_execution_checks::account_observation_matches(record, selected, now, mode)
        || !crate::live_execution_checks::live_health_evidence_current(record)
    {
        return Err("canonical_account_observation_invalid".into());
    }
    let namespace = text(&manifest["metadata"]["provider"], "id")?;
    // Namespace comparison is exact; aliases are not inferred from URLs or names.
    let account = text(&record["health"]["account"], "id")?;
    let identity_sha256 = hash(
        &json!({"version":"tradeassembly.canonical_account.v1","brokerNamespace":namespace,"environment":mode,"providerAccountId":account}),
    )?;
    Ok(CanonicalAccountIdentity {
        schema_version: "tradeassembly.canonical_account.v1".into(),
        broker_namespace: namespace.into(),
        environment: mode.into(),
        provider_account_id: account.into(),
        identity_sha256,
    })
}
pub(crate) fn current(
    plugins: &dyn PluginRegistryPort,
    packages: &dyn PluginPackagePort,
    credentials: &dyn CredentialPort,
    selected: &Value,
    now: i64,
    mode: &str,
) -> Result<AccountWitness, String> {
    current_impl(plugins, packages, credentials, selected, now, mode, true)
}
pub(crate) fn observer(
    plugins: &dyn PluginRegistryPort,
    packages: &dyn PluginPackagePort,
    credentials: &dyn CredentialPort,
    selected: &Value,
    now: i64,
    mode: &str,
) -> Result<AccountWitness, String> {
    current_impl(plugins, packages, credentials, selected, now, mode, false)
}
fn current_impl(
    plugins: &dyn PluginRegistryPort,
    packages: &dyn PluginPackagePort,
    credentials: &dyn CredentialPort,
    selected: &Value,
    now: i64,
    mode: &str,
    require_trading: bool,
) -> Result<AccountWitness, String> {
    if !matches!(mode, "paper" | "live") {
        return Err("canonical_account_mode_invalid".into());
    }
    let instance_ref = text(selected, "pluginInstanceRef")?;
    let plugin_ref = text(selected, "pluginRef")?;
    let record = plugins
        .get_instance(instance_ref)?
        .ok_or("canonical_account_instance_missing")?;
    let manifest = plugins
        .get_manifest(plugin_ref)?
        .ok_or("canonical_account_manifest_missing")?;
    let package = packages
        .get(text(&record, "activePackageSha256")?)?
        .ok_or("canonical_account_package_missing")?;
    if package.plugin_ref != plugin_ref
        || record["pluginRef"] != plugin_ref
        || manifest["manifestDigest"] != package.manifest_sha256
        || manifest["package"]["packageSha256"] != package.package_sha256
        || manifest["manifest"] != package.manifest
        || selected["manifestFingerprint"] != package.manifest_sha256
    {
        return Err("canonical_account_source_changed".into());
    }
    let credential_ref = text(&record, "credentialRef")?;
    let record = crate::live_execution_checks::apply_current_credential_status(
        record.clone(),
        &credentials.status(credential_ref),
    );
    let identity = if require_trading {
        derive(&record, &package.manifest, selected, now, mode)?
    } else {
        let checked = record["health"]["checkedAtMs"]
            .as_i64()
            .ok_or("canonical_account_observation_invalid")?;
        if !crate::live_execution_checks::live_health_evidence_current(&record)
            || !(0..=crate::live_execution_checks::LIVE_ACCOUNT_OBSERVATION_MAX_AGE_MS)
                .contains(&(now - checked))
            || record["accountMode"] != mode
            || record["health"]["account"]["mode"] != mode
            || record["instanceRef"] != selected["pluginInstanceRef"]
            || record["pluginRef"] != selected["pluginRef"]
            || record["accountRef"] != selected["accountRef"]
        {
            return Err("canonical_account_observation_invalid".into());
        }
        let id = text(&record["health"]["account"], "id")?;
        if record["accountRef"] != format!("account://{instance_ref}/{id}") {
            return Err("canonical_account_binding_invalid".into());
        }
        let ns = text(&package.manifest["metadata"]["provider"], "id")?;
        CanonicalAccountIdentity {
            schema_version: "tradeassembly.canonical_account.v1".into(),
            broker_namespace: ns.into(),
            environment: mode.into(),
            provider_account_id: id.into(),
            identity_sha256: hash(
                &json!({"version":"tradeassembly.canonical_account.v1","brokerNamespace":ns,"environment":mode,"providerAccountId":id}),
            )?,
        }
    };
    let binding_sha256 = hash(
        &json!({"identity":identity.identity_sha256,"pluginInstanceRef":instance_ref,"pluginRef":plugin_ref,"packageSha256":package.package_sha256,"manifestSha256":package.manifest_sha256,"healthBinding":crate::live_execution_checks::live_health_evidence_binding(&record)}),
    )?;
    Ok(AccountWitness {
        identity,
        binding_sha256,
        account_ref: text(&record, "accountRef")?.into(),
    })
}
pub(crate) fn from_state(state: &super::BrokerCurrentState) -> Result<AccountWitness, String> {
    let identity = derive(
        &state.instance,
        &state.package.manifest,
        &state.selected,
        state.now_ms,
        &state.mode,
    )?;
    let binding_sha256 = hash(
        &json!({"identity":identity.identity_sha256,"pluginInstanceRef":state.selected["pluginInstanceRef"],"pluginRef":state.selected["pluginRef"],"packageSha256":state.package.package_sha256,"manifestSha256":state.package.manifest_sha256,"healthBinding":crate::live_execution_checks::live_health_evidence_binding(&state.instance)}),
    )?;
    Ok(AccountWitness {
        identity,
        binding_sha256,
        account_ref: text(&state.instance, "accountRef")?.into(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn observed(instance: &str) -> (Value, Value) {
        let mut r = json!({"instanceRef":instance,"pluginRef":format!("plugin.{instance}"),"activePackageSha256":"package","configuration":{},"credentialRevision":1,"credentialStatus":{"configured":true},"enabled":true,"accountRef":format!("account://{instance}/same-account"),"accountMode":"paper","health":{"state":"ready","checkedAtMs":1000,"connectivityChecked":true,"evidenceCurrent":true,"account":{"id":"same-account","mode":"paper","status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}}});
        r["health"]["binding"] = crate::live_execution_checks::live_health_evidence_binding(&r);
        let s = json!({"pluginInstanceRef":instance,"pluginRef":format!("plugin.{instance}"),"accountRef":r["accountRef"]});
        (r, s)
    }
    #[test]
    fn connector_aliases_share_identity_but_environment_namespace_and_account_do_not() {
        let (a, sa) = observed("connector-a");
        let (b, sb) = observed("connector-b");
        let m = json!({"metadata":{"provider":{"id":"fixture.broker"}}});
        let original = derive(&a, &m, &sa, 1001, "paper").unwrap();
        assert_eq!(original, derive(&b, &m, &sb, 1001, "paper").unwrap());
        let mut other = m.clone();
        other["metadata"]["provider"]["id"] = json!("other.broker");
        assert_ne!(
            original.identity_sha256,
            derive(&b, &other, &sb, 1001, "paper")
                .unwrap()
                .identity_sha256
        );
        assert!(derive(&a, &m, &sa, 1001, "live").is_err());
        assert!(derive(&a, &m, &sa, 121001, "paper").is_err());
        let mut changed = a.clone();
        changed["credentialRevision"] = json!(2);
        assert!(derive(&changed, &m, &sa, 1001, "paper").is_err());
        let mut blocked = a;
        blocked["health"]["account"]["tradingBlocked"] = json!(true);
        assert!(derive(&blocked, &m, &sa, 1001, "paper").is_err());
    }
}
