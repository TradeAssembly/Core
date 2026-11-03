// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Host-owned continuation of an identical, already evaluated entry calculation.
//! This carries historical state, never a current signal or trading authority.
use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WarmupOrigin {
    pub schema_version: String,
    pub source_receipt_sha256: String,
    pub source_checkpoint_sha256: String,
    pub source_facts_sha256: String,
    pub source_plan_revision_id: String,
    pub event_start_ms: i64,
    pub event_end_ms: i64,
}
impl WarmupOrigin {
    pub(super) fn validate(&self) -> Result<(), String> {
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.schema_version != "tradeassembly.entry_warmup_origin.v1"
            || !digest(&self.source_receipt_sha256)
            || !digest(&self.source_checkpoint_sha256)
            || !digest(&self.source_facts_sha256)
            || self.source_plan_revision_id.is_empty()
            || self.source_plan_revision_id.len() > 256
            || self.source_plan_revision_id.chars().any(char::is_control)
            || self.event_start_ms <= 0
            || self.event_end_ms <= self.event_start_ms
            || self.event_end_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
        {
            return Err("data_evaluation_warmup_origin_invalid".into());
        }
        Ok(())
    }
}

fn source_keys(plan: &data_plans::StoredDataPlan) -> Result<BTreeSet<String>, String> {
    let mut keys = BTreeSet::new();
    for contract in plan
        .content
        .plan
        .contracts
        .iter()
        .filter(|c| c.contract.inputs.is_empty())
    {
        let node = plan
            .content
            .plan
            .request
            .nodes
            .iter()
            .find(|n| n.graph_node_id == contract.graph_node_id)
            .ok_or("data_evaluation_warmup_plan_invalid")?;
        for port in contract.contract.outputs.keys() {
            for instrument in &node.context.instruments {
                keys.insert(key(&[
                    &node.graph_node_id,
                    port,
                    &instrument.instrument_id,
                ])?);
            }
        }
    }
    Ok(keys)
}

pub(super) fn consumed_window(
    plan: &data_plans::StoredDataPlan,
    checkpoint: &EvaluationCheckpoint,
    window: &EvaluationWindow,
) -> Result<bool, String> {
    let keys = source_keys(plan)?;
    Ok(!keys.is_empty()
        && keys.iter().all(|k| {
            checkpoint
                .cursors
                .get(k)
                .is_some_and(|end| *end >= window.event_end_ms)
        }))
}

// Bound extra eligibility work independently of the ordinary evaluation budget.
// Larger/incompatible prefixes use normal ordered warm-up instead.
const MAX_SEED_FACTS: usize = 256;
fn verify_facts(s: &TradeAssemblyService, facts: &BTreeMap<String, Value>) -> Result<bool, String> {
    for (key, expected) in facts {
        match s.runtime().storage.get_json(SOURCES, key)? {
            None => return Ok(false),
            Some(actual) if actual != *expected => {
                return Err("data_evaluation_warmup_source_revised".into())
            }
            Some(_) => {}
        }
    }
    Ok(true)
}
fn prefix_facts(
    s: &TradeAssemblyService,
    original: &data_plans::StoredDataPlan,
    receipt: &EvaluationReceipt,
    window: &EvaluationWindow,
) -> Result<Option<BTreeMap<String, Value>>, String> {
    let run_key = key(&[&receipt.checkpoint.run_id, &original.revision_id])?;
    let mut expected = BTreeMap::new();
    for contract in original
        .content
        .plan
        .contracts
        .iter()
        .filter(|c| c.contract.inputs.is_empty())
    {
        let node = original
            .content
            .plan
            .request
            .nodes
            .iter()
            .find(|n| n.graph_node_id == contract.graph_node_id)
            .ok_or("data_evaluation_warmup_plan_invalid")?;
        let interval = node
            .context
            .interval_ms
            .filter(|v| *v > 0 && *v <= i64::MAX as u64)
            .ok_or("data_evaluation_warmup_plan_invalid")?;
        let duration = (window.event_end_ms - window.event_start_ms) as u64;
        if !duration.is_multiple_of(interval) {
            return Ok(None);
        }
        let count = (duration / interval)
            .checked_mul(node.context.instruments.len() as u64)
            .and_then(|n| n.checked_mul(contract.contract.outputs.len() as u64));
        let Some(count) = count.filter(|n| *n > 0 && *n <= MAX_SEED_FACTS as u64) else {
            return Ok(None);
        };
        if expected.len() + count as usize > MAX_SEED_FACTS {
            return Ok(None);
        }
        let Some(super::super::data_captures::SourceAuthentication::HostAcquisition {
            capture_id,
            ..
        }) = original
            .content
            .source_authentication
            .get(&node.graph_node_id)
        else {
            return Ok(None);
        };
        let capture = super::super::data_captures::current(s, capture_id)?;
        let mut response = capture.replay_response.clone();
        super::super::data_captures::envelope_source(s, original, node, &mut response)?;
        let before = expected.len();
        for (port, batch) in response.outputs {
            for point in batch.points.iter().filter(|p| {
                p.provenance.event_start_ms >= window.event_start_ms
                    && p.provenance.event_end_ms <= window.event_end_ms
                    && p.provenance.completed
            }) {
                if !node
                    .context
                    .instruments
                    .iter()
                    .any(|i| i.instrument_id == point.instrument_id)
                    || point.provenance.event_end_ms - point.provenance.event_start_ms
                        != interval as i64
                    || !((point.provenance.event_start_ms - window.event_start_ms) as u64)
                        .is_multiple_of(interval)
                {
                    return Ok(None);
                }
                let source_key = key(&[
                    &run_key,
                    &node.graph_node_id,
                    &port,
                    &point.instrument_id,
                    &point.provenance.event_start_ms.to_string(),
                ])?;
                let value = json!({"contentSha256":point.provenance.content_sha256,
                    "sourceEventId":point.source_event_id,"sourceRevision":point.provenance.source_revision});
                if expected.insert(source_key, value).is_some() {
                    return Err("data_evaluation_warmup_source_revised".into());
                }
            }
        }
        if expected.len() - before != count as usize {
            return Ok(None);
        }
    }
    if expected.is_empty() || !verify_facts(s, &expected)? {
        return Ok(None);
    }
    Ok(Some(expected))
}

/// No caller supplies checkpoint bytes. The position's actual immutable entry
/// selects the receipt; all current owned subject/source checks still run.
/// An ineligible layout falls back to normal ordered warm-up, never translation
/// of opaque plugin state or a guessed calculation identity.
pub(super) fn checkpoint(
    service: &TradeAssemblyService,
    plan: &data_plans::StoredDataPlan,
    request: &EvaluationStep,
) -> Result<Option<EvaluationCheckpoint>, String> {
    #[cfg(test)]
    if service
        .data_step_request
        .as_ref()
        .is_some_and(|context| context.cold_warmup)
    {
        return Ok(None);
    }
    let Some(source) = plan.content.selection_source.as_ref() else {
        return Ok(None);
    };
    let Some(position) = source.managed_position_id.as_ref() else {
        return Ok(None);
    };
    if source.source_streams.is_empty() || request.expected_checkpoint_sha256.is_some() {
        return Ok(None);
    }
    super::super::package_validation::with_round(service, |s| {
        let (original, receipt, _) = super::super::order_plans::management::entry_signal_source(
            s,
            position,
            &plan.content.scope,
        )?;
        let prior = &receipt.checkpoint;
        // Exact compiled identity is also the planSha256 supplied to plugins.
        // Instrument state can continue only with identical node/context/schema,
        // parameters, semantics, packages and nonsecret execution bindings.
        if original.content.plan != plan.content.plan
            || original.content.package_sha256 != plan.content.package_sha256
            || original.content.binding_sha256 != plan.content.binding_sha256
            || prior.warmup_origin.is_some()
            || prior.generation == 0
            || prior.last_frame.is_empty()
            || plan.content.plan.contracts.iter().any(|c| {
                c.contract
                    .state
                    .as_ref()
                    .is_some_and(|state| state.partition != StatePartition::Instrument)
            })
        {
            return Ok(None);
        }
        let Some(mut program) =
            super::super::data_strategy::program_for_content(s, &original.content)?
        else {
            return Ok(None);
        };
        program.entry = crate::data_strategy::Rule::IsTrue {
            value: crate::data_strategy::Operand::Boolean { value: false },
        };
        if super::super::data_strategy::program_for_content(s, &plan.content)?.as_ref()
            != Some(&program)
        {
            return Ok(None);
        }
        let keys = source_keys(plan)?;
        if keys.is_empty()
            || prior.cursors.keys().cloned().collect::<BTreeSet<_>>() != keys
            || prior
                .cursors
                .values()
                .any(|end| *end != request.window.event_end_ms)
        {
            return Ok(None);
        }
        for (node, id) in &source.source_streams {
            let binding = super::super::managed_data_streams::binding(s, id, &plan.content.scope)?;
            if binding.position_id != *position
                || binding.original_origin_ms != request.window.event_start_ms
                || !matches!(original.content.source_authentication.get(node), Some(super::super::data_captures::SourceAuthentication::HostAcquisition { capture_id, receipt_sha256, .. }) if capture_id == &binding.seed_capture_id && receipt_sha256 == &binding.seed_receipt_sha256)
            {
                return Ok(None);
            }
        }
        let Some(facts) = prefix_facts(s, &original, &receipt, &request.window)? else {
            return Ok(None);
        };
        let index = s
            .runtime()
            .storage
            .get_json(RECEIPT_INDEX, &receipt.receipt_sha256)?
            .ok_or("data_evaluation_receipt_index_invalid")?;
        let receipt_key = index["receiptKey"]
            .as_str()
            .ok_or("data_evaluation_receipt_index_invalid")?;
        let mut expectations = facts
            .iter()
            .map(|(key, value)| StorageExpectation::new(SOURCES, key, Some(value.clone())))
            .collect::<Vec<_>>();
        let original_record = s
            .runtime()
            .storage
            .get_json(RECEIPTS, receipt_key)?
            .ok_or("data_evaluation_receipt_invalid")?;
        if serde_json::from_value::<EvaluationReceipt>(original_record.clone())
            .map_err(|_| "data_evaluation_receipt_invalid")?
            != receipt
        {
            return Err("data_evaluation_warmup_source_revised".into());
        }
        // Keep the actual stored representation, including valid legacy empty
        // fields, as the CAS precondition. Never normalize the original record.
        expectations.push(StorageExpectation::new(
            RECEIPTS,
            receipt_key,
            Some(original_record),
        ));
        expectations.push(StorageExpectation::new(
            RECEIPT_INDEX,
            &receipt.receipt_sha256,
            Some(index),
        ));
        let facts_hash = hash(&facts)?;
        let origin = WarmupOrigin {
            schema_version: "tradeassembly.entry_warmup_origin.v1".into(),
            source_receipt_sha256: receipt.receipt_sha256,
            source_checkpoint_sha256: prior.checkpoint_sha256.clone(),
            source_facts_sha256: facts_hash,
            source_plan_revision_id: original.revision_id,
            event_start_ms: request.window.event_start_ms,
            event_end_ms: request.window.event_end_ms,
        };
        origin.validate()?;
        let mut checkpoint = prior.clone();
        checkpoint.run_id = request.run_id.clone();
        checkpoint.plan_revision_id = plan.revision_id.clone();
        checkpoint.warmup_origin = Some(origin);
        checkpoint.checkpoint_sha256 = checkpoint_hash(&checkpoint)?;
        *s.data_step_request
            .as_ref()
            .ok_or("data_evaluation_request_context_unavailable")?
            .warmup_expectations
            .lock()
            .map_err(|_| "data_evaluation_request_context_unavailable")? = expectations;
        Ok(Some(checkpoint))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_is_versioned_bounded_and_never_a_current_signal() {
        let origin = WarmupOrigin {
            schema_version: "tradeassembly.entry_warmup_origin.v1".into(),
            source_receipt_sha256: "a".repeat(64),
            source_checkpoint_sha256: "b".repeat(64),
            source_facts_sha256: "c".repeat(64),
            source_plan_revision_id: "dataplan_original".into(),
            event_start_ms: 1000,
            event_end_ms: 2000,
        };
        origin.validate().unwrap();
        for value in [
            json!({"schemaVersion":"wrong"}),
            json!({"eventStartMs":0}),
            json!({"eventEndMs":999}),
            json!({"sourceReceiptSha256":"bad"}),
        ] {
            let mut invalid = serde_json::to_value(&origin).unwrap();
            for (k, v) in value.as_object().unwrap() {
                invalid[k] = v.clone();
            }
            assert!(serde_json::from_value::<WarmupOrigin>(invalid)
                .unwrap()
                .validate()
                .is_err());
        }
        let mut unknown = serde_json::to_value(origin).unwrap();
        unknown["currentSignal"] = json!(true);
        assert!(serde_json::from_value::<WarmupOrigin>(unknown).is_err());
    }
    #[test]
    fn consumed_window_requires_all_declared_source_cursors_and_does_not_skip_a_gap() {
        let (_dir, service, scope, pipeline) =
            super::super::super::data_plans::tests::fixture_service(None);
        let context = SideEffectContext::new(
            crate::ports::AuthorityContext::local_cli(),
            IdempotencyKey::new("warmup-key").unwrap(),
        );
        let plan = data_plans::save(&service, scope, pipeline, &context).unwrap();
        let keys = source_keys(&plan).unwrap();
        assert!(!keys.is_empty());
        let mut checkpoint:EvaluationCheckpoint=serde_json::from_value(json!({"runId":"r","planRevisionId":plan.revision_id,
            "generation":1,"cursors":{},"states":{},"validCounts":{},"checkpointSha256":"a".repeat(64)})).unwrap();
        let window = EvaluationWindow {
            event_start_ms: 10,
            event_end_ms: 15,
            as_of_ms: 20,
        };
        assert!(!consumed_window(&plan, &checkpoint, &window).unwrap());
        for k in &keys {
            checkpoint.cursors.insert(k.clone(), 15);
        }
        assert!(consumed_window(&plan, &checkpoint, &window).unwrap());
        checkpoint.cursors.insert(keys.first().unwrap().clone(), 14);
        assert!(!consumed_window(&plan, &checkpoint, &window).unwrap());
    }
    #[test]
    fn prefix_requires_every_original_fact_and_rejects_changed_history() {
        let (_dir, service, _, _) = super::super::super::data_plans::tests::fixture_service(None);
        let context = SideEffectContext::new(
            crate::ports::AuthorityContext::local_cli(),
            IdempotencyKey::new("prefix-test").unwrap(),
        );
        let facts = BTreeMap::from([
            ("first".into(), json!({"contentSha256":"original.first"})),
            ("second".into(), json!({"contentSha256":"original.second"})),
        ]);
        assert!(!verify_facts(&service, &facts).unwrap());
        service
            .runtime()
            .storage
            .put_json(SOURCES, "first", facts["first"].clone(), &context)
            .unwrap();
        assert!(!verify_facts(&service, &facts).unwrap());
        service
            .runtime()
            .storage
            .put_json(SOURCES, "second", facts["second"].clone(), &context)
            .unwrap();
        assert!(verify_facts(&service, &facts).unwrap());
        service
            .runtime()
            .storage
            .put_json(
                SOURCES,
                "first",
                json!({"contentSha256":"revised"}),
                &context,
            )
            .unwrap();
        assert_eq!(
            verify_facts(&service, &facts).unwrap_err(),
            "data_evaluation_warmup_source_revised"
        );
    }
    #[test]
    fn prefix_facts_remain_atomic_commit_preconditions() {
        let (_dir, base, _, _) = super::super::super::data_plans::tests::fixture_service(None);
        let service = request_service(&base);
        let context = SideEffectContext::new(
            crate::ports::AuthorityContext::local_cli(),
            IdempotencyKey::new("prefix-atomic").unwrap(),
        );
        let original = json!({"contentSha256":"original"});
        service
            .runtime()
            .storage
            .put_json(SOURCES, "fact", original.clone(), &context)
            .unwrap();
        *service
            .data_step_request
            .as_ref()
            .unwrap()
            .warmup_expectations
            .lock()
            .unwrap() = vec![StorageExpectation::new(SOURCES, "fact", Some(original))];
        service
            .runtime()
            .storage
            .put_json(
                SOURCES,
                "fact",
                json!({"contentSha256":"changed_after_validation"}),
                &context,
            )
            .unwrap();
        let mut expectations = vec![StorageExpectation::new(RUNS, "new-run", None)];
        expectations.extend(
            service
                .data_step_request
                .as_ref()
                .unwrap()
                .warmup_expectations
                .lock()
                .unwrap()
                .clone(),
        );
        assert_ne!(
            service
                .runtime()
                .storage
                .put_json_batch(
                    &[StorageWrite::new(
                        RUNS,
                        "new-run",
                        json!({"seed":"must not commit"}),
                        context
                    )],
                    &expectations
                )
                .unwrap(),
            ComparePutOutcome::Updated
        );
        assert!(service
            .runtime()
            .storage
            .get_json(RUNS, "new-run")
            .unwrap()
            .is_none());
    }
    #[test]
    fn restored_origin_cannot_claim_unconsumed_or_same_plan_history() {
        let value = json!({"runId":"new","planRevisionId":"new-plan","generation":1,"cursors":{"root":2000},
            "states":{},"validCounts":{},"checkpointSha256":"a".repeat(64),
            "warmupOrigin":{"schemaVersion":"tradeassembly.entry_warmup_origin.v1","sourceReceiptSha256":"a".repeat(64),
                "sourceCheckpointSha256":"b".repeat(64),"sourceFactsSha256":"c".repeat(64),
                "sourcePlanRevisionId":"old-plan","eventStartMs":1000,"eventEndMs":2000}});
        let mut checkpoint: EvaluationCheckpoint = serde_json::from_value(value).unwrap();
        validate_state_availability(&checkpoint).unwrap();
        checkpoint.cursors.insert("root".into(), 1999);
        assert_eq!(
            validate_state_availability(&checkpoint).unwrap_err(),
            "data_evaluation_warmup_origin_invalid"
        );
        checkpoint.cursors.insert("root".into(), 2000);
        checkpoint
            .warmup_origin
            .as_mut()
            .unwrap()
            .source_plan_revision_id = "new-plan".into();
        assert_eq!(
            validate_state_availability(&checkpoint).unwrap_err(),
            "data_evaluation_warmup_origin_invalid"
        );
    }
}
