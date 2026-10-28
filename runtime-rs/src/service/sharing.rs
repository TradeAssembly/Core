// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Authenticated, private-by-default local factsheets. No bearer links or delivery.
use super::{api_result, strategy, ServiceResponse, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::backtest_contracts::canonical_hash;
use crate::domain::BacktestRunState;
use crate::ports::{
    AuthorityContext, ComparePutOutcome, IdempotencyKey, ObjectOwner, SideEffectContext,
    StorageExpectation, StorageWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const RECORDS: &str = "local_share_snapshots_v1";
const STATES: &str = "local_share_access_v1";
const AUDIT: &str = "local_share_audit_v1";
const VERSION: &str = "local_snapshot_v1";
type Result<T> = std::result::Result<T, ServiceResponse>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    id: String,
    owner: ObjectOwner,
    strategy_id: String,
    request_hash: String,
    content_hash: String,
    content: Value,
    recipients: Vec<String>,
    created_at_ms: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Access {
    revision: u64,
    revoked: bool,
    revoked_subjects: BTreeSet<String>,
}
fn unavailable() -> ServiceResponse {
    ServiceResponse::error(404, "snapshot_unavailable", false)
}
fn failed() -> ServiceResponse {
    ServiceResponse::internal_error("local sharing storage unavailable")
}
fn hash(value: &impl Serialize) -> Result<String> {
    canonical_hash(value, "snapshot_integrity_failed").map_err(|_| failed())
}
fn field<'a>(body: &'a Value, key: &str) -> &'a str {
    body[key].as_str().unwrap_or("").trim()
}
fn owner(service: &TradeAssemblyService) -> Result<ObjectOwner> {
    service.invocation_owner().ok_or_else(unavailable)
}
fn human_owner(service: &TradeAssemblyService) -> Result<ObjectOwner> {
    let owner = owner(service)?;
    if service.invocation_actor_kind != "user" {
        return Err(ServiceResponse::forbidden("snapshot_owner_user_required"));
    }
    Ok(owner)
}
fn context(owner: &ObjectOwner, key: &str) -> Result<SideEffectContext> {
    Ok(SideEffectContext::new(
        AuthorityContext {
            actor: owner.subject.clone(),
            surface: "local_sharing".into(),
            account_mode: "paper".into(),
        },
        IdempotencyKey::new(key).map_err(|_| failed())?,
    ))
}
fn safe_text(value: &str, fallback: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 120
        || value.chars().filter(char::is_ascii_digit).count() >= 8
        || lower.contains("account")
        || [
            "secret",
            "token",
            "password",
            "credential",
            "bearer",
            "akia",
            "-----begin",
        ]
        .iter()
        .any(|word| lower.contains(word))
        || value
            .chars()
            .any(|c| !(c.is_alphanumeric() || " ._-/".contains(c)))
        || value
            .split(|c: char| !c.is_alphanumeric())
            .any(|part| part.len() > 24)
    {
        return fallback.into();
    }
    value.into()
}
fn content(service: &TradeAssemblyService, strategy_id: &str, run_id: &str) -> Result<Value> {
    service.require_object("strategy", strategy_id)?;
    let versions = strategy::strategy_versions(service, strategy_id);
    let version = versions
        .last()
        .filter(|v| v["immutable"] == true && v["version"].as_u64().is_some_and(|n| n > 0))
        .ok_or_else(|| ServiceResponse::bad_request("snapshot_published_version_required"))?;
    let spec = &version["spec"];
    let spec_hash = hash(spec)?;
    if version["specHash"].as_str() != Some(spec_hash.as_str()) {
        return Err(ServiceResponse::conflict(
            "snapshot_strategy_integrity_failed",
        ));
    }
    let symbols = spec["signal_market_requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|requirement| requirement["selectors"].as_array().into_iter().flatten())
        .filter_map(|selector| selector["normalized_id"].as_str())
        .map(|symbol| safe_text(symbol, "Instrument redacted"))
        .take(20)
        .collect::<Vec<_>>();
    let mut result = json!({"schemaVersion":"tradeassembly.local_factsheet.v1","scope":"research_simulation_paper","strategy":{"name":safe_text(spec["name"].as_str().unwrap_or(""),"Shared strategy"),"version":version["version"],"definitionHash":spec_hash,"instruments":symbols},"research":null,"notice":LEGAL_BOUNDARY,"limitations":"A local research factsheet; no executable strategy, credentials, broker accounts, orders or live-performance claim is included."});
    if !run_id.is_empty() {
        service.require_object("backtest_run", run_id)?;
        let run = service
            .runtime()
            .backtests
            .get_run(run_id)
            .map_err(|_| failed())?
            .ok_or_else(unavailable)?;
        if run.state != BacktestRunState::Completed {
            return Err(ServiceResponse::bad_request(
                "snapshot_completed_research_required",
            ));
        }
        let manifest = service
            .runtime()
            .backtests
            .get_manifest(&run.run_id)
            .map_err(|_| failed())?
            .ok_or_else(unavailable)?;
        manifest
            .verify()
            .map_err(|_| ServiceResponse::conflict("snapshot_research_integrity_failed"))?;
        let binding = &manifest.content.configuration.strategy;
        if binding.strategy_id != strategy_id
            || binding.strategy_version_id != field(version, "id")
            || binding.strategy_spec_hash != spec_hash
        {
            return Err(ServiceResponse::bad_request(
                "snapshot_research_version_mismatch",
            ));
        }
        let research = run.result.as_ref().ok_or_else(unavailable)?;
        research
            .verify()
            .map_err(|_| ServiceResponse::conflict("snapshot_research_integrity_failed"))?;
        if research.content.manifest_hash != run.manifest_hash
            || run.result_hash.as_ref() != Some(&research.result_hash)
        {
            return Err(ServiceResponse::conflict(
                "snapshot_research_integrity_failed",
            ));
        }
        let mut accounting = serde_json::Map::new();
        for key in [
            "startingCashMicros",
            "endingCashMicros",
            "endingEquityMicros",
            "realizedPnlMicros",
            "unrealizedPnlMicros",
            "totalPnlMicros",
        ] {
            if let Some(value) = research.content.accounting[key].as_i64() {
                accounting.insert(key.into(), json!(value));
            }
        }
        result["research"] = json!({"status":"completed","manifestHash":run.manifest_hash,"resultHash":research.result_hash,"datasetHash":manifest.content.configuration.dataset.content_hash,"reportingCurrency":safe_text(&manifest.content.configuration.capital.reporting_currency,"Unavailable"),"simulationOrderCount":research.content.orders.len(),"simulationFillCount":research.content.fills.len(),"accountingMicros":accounting});
    }
    if serde_json::to_vec(&result).map_err(|_| failed())?.len() > 32_768 {
        return Err(ServiceResponse::bad_request("snapshot_content_too_large"));
    }
    Ok(result)
}
fn audit(id: &str, record: &Snapshot, state: &Access, action: &str, at: i64) -> Result<Value> {
    Ok(
        json!({"schemaVersion":"tradeassembly.local_share_audit.v1","snapshotId":id,"revision":state.revision,"action":action,"recordHash":hash(record)?,"stateHash":hash(state)?,"observedAtMs":at}),
    )
}
fn load(service: &TradeAssemblyService, id: &str) -> Result<(Snapshot, Access)> {
    if id.is_empty() || id == "latest" {
        return Err(unavailable());
    }
    let record: Snapshot = serde_json::from_value(
        service
            .runtime()
            .storage
            .get_json(RECORDS, id)
            .map_err(|_| failed())?
            .ok_or_else(unavailable)?,
    )
    .map_err(|_| unavailable())?;
    let state: Access = serde_json::from_value(
        service
            .runtime()
            .storage
            .get_json(STATES, id)
            .map_err(|_| failed())?
            .ok_or_else(unavailable)?,
    )
    .map_err(|_| unavailable())?;
    let initial = service
        .runtime()
        .storage
        .get_json(AUDIT, &format!("{id}:0"))
        .map_err(|_| failed())?
        .ok_or_else(unavailable)?;
    let latest = service
        .runtime()
        .storage
        .get_json(AUDIT, &format!("{id}:{}", state.revision))
        .map_err(|_| failed())?
        .ok_or_else(unavailable)?;
    let scope = service
        .runtime()
        .object_authorization
        .scope("share_snapshot", id)
        .map_err(|_| failed())?
        .ok_or_else(unavailable)?;
    if record.id != id
        || scope.owner != record.owner
        || hash(&record.content)? != record.content_hash
        || initial["recordHash"] != hash(&record)?
        || latest["recordHash"] != initial["recordHash"]
        || latest["stateHash"] != hash(&state)?
        || latest["snapshotId"] != id
    {
        return Err(ServiceResponse::conflict("snapshot_integrity_failed"));
    }
    Ok((record, state))
}
fn summary(record: &Snapshot, state: &Access) -> Value {
    json!({"id":record.id,"status":if state.revoked{"revoked"}else{"active"},"createdAtMs":record.created_at_ms,"contentHash":record.content_hash,"name":record.content["strategy"]["name"],"version":record.content["strategy"]["version"],"recipientSubjects":record.recipients,"revokedSubjects":state.revoked_subjects,"private":record.recipients.is_empty(),"revision":state.revision})
}
pub(super) fn workspace(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let action = || -> Result<Value> {
        let owner = human_owner(service)?;
        let strategy_id = field(&body, "strategyId");
        service.require_object("strategy", strategy_id)?;
        let run_id = field(&body, "researchRunId");
        let preview = match content(service, strategy_id, run_id) {
            Ok(value) => json!({"contentHash":hash(&value)?,"content":value}),
            Err(response) => json!({"error":response.body["error"]["code"]}),
        };
        let mut research_options = Vec::new();
        let mut runs = service
            .runtime()
            .backtests
            .list_runs()
            .map_err(|_| failed())?;
        runs.sort_by_key(|run| std::cmp::Reverse(run.created_at_ms));
        for run in runs.into_iter().filter(|run| {
            run.state == BacktestRunState::Completed
                && service.object_is_visible("backtest_run", &run.run_id)
        }) {
            let Some(manifest) = service
                .runtime()
                .backtests
                .get_manifest(&run.run_id)
                .map_err(|_| failed())?
            else {
                continue;
            };
            if manifest.verify().is_err()
                || manifest.content.configuration.strategy.strategy_id != strategy_id
            {
                continue;
            }
            if content(service, strategy_id, &run.run_id).is_err() {
                continue;
            }
            research_options.push(json!({"runId":run.run_id,"createdAtMs":run.created_at_ms,"simulationFillCount":run.result.as_ref().map(|result|result.content.fills.len())}));
            if research_options.len() == 20 {
                break;
            }
        }
        let mut snapshots = Vec::new();
        for scope in service
            .runtime()
            .object_authorization
            .list_visible(&owner, "share_snapshot")
            .map_err(|_| failed())?
        {
            let (record, state) = match load(service, &scope.object_id) {
                Ok(value) => value,
                Err(error) if error.status == 404 => continue,
                Err(error) => return Err(error),
            };
            if record.strategy_id == strategy_id {
                snapshots.push(summary(&record, &state));
            }
        }
        snapshots.sort_by_key(|row| std::cmp::Reverse(row["createdAtMs"].as_i64().unwrap_or(0)));
        Ok(
            json!({"sharingVersion":VERSION,"capabilities":{"create":true,"authenticatedRecipients":true,"publicLinks":false,"hostedInvitations":false},"recipientIssuer":owner.issuer,"researchOptions":research_options,"preview":preview,"snapshots":snapshots,"exportChecklist":{"publishedVersion":preview["content"].is_object(),"redactedAllowlist":true,"immutableContent":true,"brokerAccountsIncluded":false}}),
        )
    };
    match action() {
        Ok(value) => ServiceResponse::ok(value),
        Err(error) => error,
    }
}
pub(super) fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let action = || -> Result<Value> {
        let owner = human_owner(service)?;
        let strategy_id = field(&body, "strategyId");
        service.require_object("strategy", strategy_id)?;
        let key = field(&body, "idempotencyKey");
        if key.is_empty() || key.len() > 200 {
            return Err(ServiceResponse::bad_request(
                "snapshot_idempotency_key_required",
            ));
        }
        let expected = field(&body, "expectedContentHash");
        if expected.is_empty() {
            return Err(ServiceResponse::bad_request("snapshot_preview_required"));
        }
        let values = body
            .get("recipientSubjects")
            .map(|v| {
                v.as_array()
                    .ok_or_else(|| ServiceResponse::bad_request("snapshot_recipients_invalid"))
            })
            .transpose()?
            .cloned()
            .unwrap_or_default();
        if values.len() > 10 {
            return Err(ServiceResponse::bad_request("snapshot_recipient_limit"));
        }
        let mut recipients = BTreeSet::new();
        for value in values {
            let recipient = value
                .as_str()
                .ok_or_else(|| ServiceResponse::bad_request("snapshot_recipients_invalid"))?;
            if recipient.is_empty()
                || recipient.trim() != recipient
                || recipient.len() > 256
                || recipient.chars().any(char::is_control)
                || recipient == owner.subject
            {
                return Err(ServiceResponse::bad_request("snapshot_recipients_invalid"));
            }
            recipients.insert(recipient.to_string());
        }
        if body["acknowledgeSnapshot"] != true
            || (!recipients.is_empty() && body["acknowledgeRecipients"] != true)
        {
            return Err(ServiceResponse::bad_request("snapshot_review_required"));
        }
        let recipients = recipients.into_iter().collect::<Vec<_>>();
        let request_hash = hash(
            &json!({"strategyId":strategy_id,"contentHash":expected,"researchRunId":field(&body,"researchRunId"),"recipientSubjects":recipients}),
        )?;
        let id = format!(
            "share_{:x}",
            Sha256::digest(format!("{}\0{}\0{key}", owner.issuer, owner.subject).as_bytes())
        );
        if service
            .runtime()
            .storage
            .get_json(RECORDS, &id)
            .map_err(|_| failed())?
            .is_some()
        {
            let (record, state) = load(service, &id)?;
            if record.owner != owner || record.request_hash != request_hash {
                return Err(ServiceResponse::conflict("snapshot_idempotency_conflict"));
            }
            return Ok(summary(&record, &state));
        }
        let content = content(service, strategy_id, field(&body, "researchRunId"))?;
        let content_hash = hash(&content)?;
        if expected != content_hash {
            return Err(ServiceResponse::conflict("snapshot_preview_changed"));
        }
        let record = Snapshot {
            id: id.clone(),
            owner: owner.clone(),
            strategy_id: strategy_id.into(),
            request_hash,
            content_hash,
            content,
            recipients,
            created_at_ms: service.runtime().clock.now_ms(),
        };
        let state = Access {
            revision: 0,
            revoked: false,
            revoked_subjects: BTreeSet::new(),
        };
        service
            .bind_inherited_object("share_snapshot", &id, "strategy", strategy_id)
            .map_err(|_| unavailable())?;
        let ctx = context(&owner, &format!("share:create:{id}"))?;
        let writes = [
            StorageWrite::new(
                RECORDS,
                &id,
                serde_json::to_value(&record).map_err(|_| failed())?,
                ctx.clone(),
            ),
            StorageWrite::new(
                STATES,
                &id,
                serde_json::to_value(&state).map_err(|_| failed())?,
                ctx.clone(),
            ),
            StorageWrite::new(
                AUDIT,
                format!("{id}:0"),
                audit(&id, &record, &state, "created", record.created_at_ms)?,
                ctx,
            ),
        ];
        let expectations = [
            StorageExpectation::new(RECORDS, &id, None),
            StorageExpectation::new(STATES, &id, None),
            StorageExpectation::new(AUDIT, format!("{id}:0"), None),
        ];
        if service
            .runtime()
            .storage
            .put_json_batch(&writes, &expectations)
            .map_err(|_| failed())?
            != ComparePutOutcome::Updated
        {
            return Err(ServiceResponse::conflict("snapshot_create_conflict"));
        }
        Ok(summary(&record, &state))
    };
    match action() {
        Ok(value) => ServiceResponse::created(api_result(value)),
        Err(error) => error,
    }
}
pub(super) fn view(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let action = || -> Result<Value> {
        let caller = human_owner(service)?;
        let id = field(&body, "snapshotId");
        // Authorization is decided before returning content or integrity diagnostics.
        let raw = service
            .runtime()
            .storage
            .get_json(RECORDS, id)
            .map_err(|_| failed())?
            .ok_or_else(unavailable)?;
        let owner_value = serde_json::to_value(&caller).map_err(|_| failed())?;
        let candidate_owner = raw["owner"] == owner_value;
        let candidate_recipient = raw["owner"]["issuer"] == caller.issuer
            && raw["recipients"]
                .as_array()
                .is_some_and(|list| list.iter().any(|value| value == &caller.subject));
        if !candidate_owner && !candidate_recipient {
            return Err(unavailable());
        }
        let (record, state) = load(service, id)?;
        if state.revoked {
            return Err(if candidate_owner {
                ServiceResponse::error(410, "snapshot_revoked", false)
            } else {
                unavailable()
            });
        }
        if !candidate_owner && state.revoked_subjects.contains(&caller.subject) {
            return Err(unavailable());
        }
        let manifest = json!({"schemaVersion":"tradeassembly.local_share_manifest.v1","snapshotId":id,"createdAtMs":record.created_at_ms,"contentHash":record.content_hash,"hashAlgorithm":"RFC8785+SHA-256"});
        Ok(
            json!({"sharingVersion":VERSION,"snapshot":{"id":id,"status":"active","private":record.recipients.is_empty(),"createdAtMs":record.created_at_ms},"access":if candidate_owner{"owner"}else{"authorized_recipient"},"canManage":candidate_owner,"manifest":manifest,"content":record.content,"factsheet":{"strategy":record.content["strategy"],"strategyVersionHash":record.content["strategy"]["definitionHash"]},"evidence":{"manifest":manifest,"content":record.content},"journal":null}),
        )
    };
    match action() {
        Ok(value) => ServiceResponse::ok(value),
        Err(error) => error,
    }
}
pub(super) fn revoke(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let action = || -> Result<Value> {
        let caller = human_owner(service)?;
        let id = field(&body, "snapshotId");
        service.require_object("share_snapshot", id)?;
        let (record, mut state) = load(service, id)?;
        if record.owner != caller {
            return Err(unavailable());
        }
        let old = serde_json::to_value(&state).map_err(|_| failed())?;
        let recipient = field(&body, "recipientSubject");
        if state.revoked || (!recipient.is_empty() && state.revoked_subjects.contains(recipient)) {
            return Ok(summary(&record, &state));
        }
        if recipient.is_empty() {
            state.revoked = true;
        } else if record.recipients.iter().any(|value| value == recipient) {
            state.revoked_subjects.insert(recipient.into());
        } else {
            return Err(ServiceResponse::bad_request(
                "snapshot_recipient_not_granted",
            ));
        }
        state.revision = state.revision.checked_add(1).ok_or_else(failed)?;
        let key = format!("{id}:{}", state.revision);
        let ctx = context(&caller, &format!("share:revoke:{key}"))?;
        let writes = [
            StorageWrite::new(
                STATES,
                id,
                serde_json::to_value(&state).map_err(|_| failed())?,
                ctx.clone(),
            ),
            StorageWrite::new(
                AUDIT,
                &key,
                audit(
                    id,
                    &record,
                    &state,
                    if recipient.is_empty() {
                        "revoked"
                    } else {
                        "recipient_revoked"
                    },
                    service.runtime().clock.now_ms(),
                )?,
                ctx,
            ),
        ];
        let expected = [
            StorageExpectation::new(STATES, id, Some(old)),
            StorageExpectation::new(AUDIT, &key, None),
        ];
        if service
            .runtime()
            .storage
            .put_json_batch(&writes, &expected)
            .map_err(|_| failed())?
            != ComparePutOutcome::Updated
        {
            return Err(ServiceResponse::conflict("snapshot_revoke_conflict"));
        }
        Ok(summary(&record, &state))
    };
    match action() {
        Ok(value) => ServiceResponse::ok(api_result(value)),
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        tempfile::TempDir,
        TradeAssemblyService,
        TradeAssemblyService,
        TradeAssemblyService,
        TradeAssemblyService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let base =
            TradeAssemblyService::test_local(dir.path().join("sharing.db").to_string_lossy());
        let alice = base.for_authenticated_invocation(
            "https://local-issuer.example",
            "alice",
            None,
            Some("Alice".into()),
        );
        let bob = base.for_authenticated_invocation(
            "https://local-issuer.example",
            "bob",
            None,
            Some("Bob".into()),
        );
        let foreign =
            base.for_authenticated_invocation("https://other-issuer.example", "bob", None, None);
        (dir, base, alice, bob, foreign)
    }
    fn request(service: &TradeAssemblyService, key: &str, recipients: Value) -> Value {
        let preview = workspace(service, json!({"strategyId":"strat_local_btc_demo"}));
        assert_eq!(preview.status, 200, "{:?}", preview.body);
        json!({"strategyId":"strat_local_btc_demo","expectedContentHash":preview.body["preview"]["contentHash"],"idempotencyKey":key,"recipientSubjects":recipients,"acknowledgeSnapshot":true,"acknowledgeRecipients":true})
    }
    fn created(service: &TradeAssemblyService, request: Value) -> String {
        let result = create(service, request);
        assert_eq!(result.status, 201, "{:?}", result.body);
        result.body["body"]["id"].as_str().unwrap().into()
    }
    #[test]
    fn sharing_private_defaults_and_exact_recipient_authorization() {
        let (_dir, base, alice, bob, foreign) = fixture();
        let id = created(&alice, request(&alice, "private", json!([])));
        assert_eq!(view(&alice, json!({"snapshotId":id})).status, 200);
        for caller in [&base, &bob, &foreign] {
            assert_eq!(
                view(caller, json!({"snapshotId":id,"owner":"alice"})).status,
                404
            );
        }
        let id = created(&alice, request(&alice, "recipient", json!(["bob"])));
        let exported = view(&bob, json!({"snapshotId":id}));
        assert_eq!(exported.status, 200, "{:?}", exported.body);
        assert_eq!(exported.body["access"], "authorized_recipient");
        assert_eq!(exported.body["canManage"], false);
        assert!(!exported.body.to_string().contains("alice"));
        assert!(!exported
            .body
            .to_string()
            .contains("https://local-issuer.example"));
        assert_eq!(view(&foreign, json!({"snapshotId":id})).status, 404);
        assert_eq!(revoke(&bob, json!({"snapshotId":id})).status, 404);
        assert_eq!(
            create(&bob, request(&alice, "theft", json!([]))).status,
            404
        );
        assert_eq!(
            workspace(&bob, json!({"strategyId":"strat_local_btc_demo"})).status,
            404
        );
    }
    #[test]
    fn sharing_revoke_is_durable_idempotent_and_create_retry_never_restores_access() {
        let (dir, _base, alice, bob, _foreign) = fixture();
        let request = request(&alice, "retry", json!(["bob"]));
        let id = created(&alice, request.clone());
        assert_eq!(created(&alice, request.clone()), id);
        assert_eq!(
            revoke(&alice, json!({"snapshotId":id,"recipientSubject":"bob"})).status,
            200
        );
        assert_eq!(view(&bob, json!({"snapshotId":id})).status, 404);
        assert_eq!(view(&alice, json!({"snapshotId":id})).status, 200);
        assert_eq!(created(&alice, request.clone()), id);
        assert_eq!(view(&bob, json!({"snapshotId":id})).status, 404);
        assert_eq!(revoke(&alice, json!({"snapshotId":id})).status, 200);
        assert_eq!(revoke(&alice, json!({"snapshotId":id})).status, 200);
        assert_eq!(create(&alice, request).body["body"]["status"], "revoked");
        let reopened =
            TradeAssemblyService::test_local(dir.path().join("sharing.db").to_string_lossy());
        let alice = reopened.for_authenticated_invocation(
            "https://local-issuer.example",
            "alice",
            None,
            None,
        );
        let bob = reopened.for_authenticated_invocation(
            "https://local-issuer.example",
            "bob",
            None,
            None,
        );
        assert_eq!(view(&alice, json!({"snapshotId":id})).status, 410);
        assert_eq!(view(&bob, json!({"snapshotId":id})).status, 404);
    }
    #[test]
    fn sharing_rejects_missing_review_stale_preview_and_changed_retry_payload() {
        let (_dir, _base, alice, _bob, _foreign) = fixture();
        let mut body = request(&alice, "review", json!(["bob"]));
        body["acknowledgeRecipients"] = json!(false);
        assert_eq!(create(&alice, body.clone()).status, 400);
        body["acknowledgeRecipients"] = json!(true);
        body["expectedContentHash"] = json!("wrong");
        assert_eq!(create(&alice, body).status, 409);
        let mut body = request(&alice, "same-key", json!([]));
        created(&alice, body.clone());
        body["recipientSubjects"] = json!(["bob"]);
        assert_eq!(create(&alice, body).status, 409);
    }
    #[test]
    fn sharing_content_and_access_tampering_fail_closed() {
        let (_dir, base, alice, bob, _foreign) = fixture();
        let id = created(&alice, request(&alice, "tamper", json!(["bob"])));
        let state_id = created(&alice, request(&alice, "tamper-state", json!([])));
        let mut record = base
            .runtime()
            .storage
            .get_json(RECORDS, &id)
            .unwrap()
            .unwrap();
        record["content"]["strategy"]["name"] = json!("Forged");
        record["contentHash"] = json!(hash(&record["content"]).unwrap());
        base.runtime()
            .storage
            .put_json(
                RECORDS,
                &id,
                record,
                &context(&alice.invocation_owner().unwrap(), "tamper-fixture").unwrap(),
            )
            .unwrap();
        assert_eq!(view(&alice, json!({"snapshotId":id})).status, 409);
        assert_eq!(view(&bob, json!({"snapshotId":id})).status, 409);
        let id = state_id;
        let mut state = base
            .runtime()
            .storage
            .get_json(STATES, &id)
            .unwrap()
            .unwrap();
        state["revision"] = json!(900);
        base.runtime()
            .storage
            .put_json(
                STATES,
                &id,
                state,
                &context(&alice.invocation_owner().unwrap(), "tamper-state-fixture").unwrap(),
            )
            .unwrap();
        assert_eq!(view(&alice, json!({"snapshotId":id})).status, 404);
    }
    #[test]
    fn sharing_redaction_is_allowlisted_and_bounded() {
        let (_dir, base, alice, _bob, _foreign) = fixture();
        // Seed an owned published fixture with deliberately sensitive unused
        // fields. The snapshot must never serialize the original strategy.
        let mut versions = strategy::strategy_versions(&alice, "strat_local_btc_demo");
        let mut version = versions.pop().unwrap();
        version["spec"]["name"] = json!("Secret account fixture");
        version["spec"]["metadata"] = json!({"apiKey":"AKIA_FIXTURE_MUST_NOT_ESCAPE","accountRef":"account://private/1234","accessToken":"opaque-fixture-token"});
        version["specHash"] = json!(hash(&version["spec"]).unwrap());
        base.runtime()
            .storage
            .put_json(
                "strategy_versions",
                "strat_local_btc_demo:1",
                version,
                &context(&alice.invocation_owner().unwrap(), "redaction-fixture").unwrap(),
            )
            .unwrap();
        let id = created(&alice, request(&alice, "redacted", json!([])));
        let result = view(&alice, json!({"snapshotId":id}));
        assert_eq!(result.status, 200);
        let text = result.body.to_string();
        for secret in [
            "AKIA_FIXTURE_MUST_NOT_ESCAPE",
            "account://private/1234",
            "opaque-fixture-token",
            "Secret account fixture",
            "metadata",
            "providerRef",
        ] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
        assert_eq!(
            result.body["content"]["strategy"]["name"],
            "Shared strategy"
        );
        assert_eq!(result.body["content"]["scope"], "research_simulation_paper");
    }
    #[test]
    fn sharing_missing_snapshot_and_research_boundaries_do_not_fabricate_content() {
        let (_dir, base, alice, _bob, _foreign) = fixture();
        for caller in [&base, &alice] {
            for id in ["missing", "latest", "snapshot_local"] {
                let result = view(caller, json!({"snapshotId":id}));
                assert_eq!(result.status, 404);
                assert!(result.body.get("content").is_none());
            }
        }
        let mut body = request(&alice, "research", json!([]));
        body["researchRunId"] = json!("unowned-run");
        assert_eq!(create(&alice, body).status, 404);
        let mut body = request(&alice, "too-many", json!([]));
        body["recipientSubjects"] = json!((0..11)
            .map(|index| format!("recipient-{index}"))
            .collect::<Vec<_>>());
        assert_eq!(create(&alice, body).status, 400);
    }
    #[test]
    fn sharing_rejects_unpublished_drafts_and_agent_management() {
        let (_dir, _base, alice, _bob, _foreign) = fixture();
        let body = request(&alice, "agent", json!([]));
        let id = created(&alice, request(&alice, "owner", json!([])));
        let mut agent = alice.clone();
        agent.invocation_actor_kind = "agent";
        assert_eq!(create(&agent, body).status, 403);
        assert_eq!(view(&agent, json!({"snapshotId":id})).status, 403);
        assert_eq!(revoke(&agent, json!({"snapshotId":id})).status, 403);
        strategy::create_strategy(
            &alice,
            json!({"id":"sharing-draft-only","name":"Owner draft","creationMode":"blank"}),
        );
        let preview = workspace(&alice, json!({"strategyId":"sharing-draft-only"}));
        assert_eq!(preview.status, 200);
        assert_eq!(
            preview.body["preview"]["error"],
            "snapshot_published_version_required"
        );
        assert_eq!(create(&alice,json!({"strategyId":"sharing-draft-only","idempotencyKey":"draft","expectedContentHash":"unpublished","acknowledgeSnapshot":true})).status,400);
    }
    #[test]
    fn sharing_text_scrubs_identity_like_numbers_addresses_and_tokens() {
        for value in [
            "9876543210",
            "123-45-6789",
            "owner@example.test",
            "Bearer credential",
            "0123456789abcdef0123456789abcdef",
        ] {
            assert_eq!(safe_text(value, "Redacted"), "Redacted");
        }
        assert_eq!(safe_text("Owner study V2", "Redacted"), "Owner study V2");
    }
    #[test]
    fn sharing_transport_authorizes_before_replaying_mutation_receipts() {
        let (_dir, base, alice, bob, _foreign) = fixture();
        let body = request(&alice, "cached-creation", json!([]));
        let created = alice.handle_http(
            "POST",
            "/product/strategies/share-snapshots/create",
            body.clone(),
        );
        assert_eq!(created.status, 201, "{created:?}");
        let mut agent = alice.clone();
        agent.invocation_actor_kind = "agent";
        assert_eq!(
            agent
                .handle_http(
                    "POST",
                    "/product/strategies/share-snapshots/create",
                    body.clone()
                )
                .status,
            403
        );
        assert_eq!(
            base.handle_http(
                "POST",
                "/product/strategies/share-snapshots/create",
                body.clone()
            )
            .status,
            404
        );
        assert_eq!(
            bob.handle_http("POST", "/product/strategies/share-snapshots/create", body)
                .status,
            404
        );
        let revoke_body =
            json!({"snapshotId":created.body["body"]["id"],"idempotencyKey":"cached-revocation"});
        assert_eq!(
            alice
                .handle_http(
                    "POST",
                    "/product/strategies/share-snapshots/revoke",
                    revoke_body.clone()
                )
                .status,
            200
        );
        assert_eq!(
            bob.handle_http(
                "POST",
                "/product/strategies/share-snapshots/revoke",
                revoke_body
            )
            .status,
            404
        );
    }
}
