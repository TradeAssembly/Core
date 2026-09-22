//! Concrete, local admission boundary for live broker orders.
//!
//! This module deliberately stops at the finance-authority C5 boundary.  It
//! does not dispatch to a provider and it never treats an accepted submission
//! as a fill.

use super::{build_order_intent, load_current_state, required_value, BrokerSubmissionDependencies};
use crate::broker_submission::{prices, risk};
use crate::control_plane::ControlPlaneCommandEnvelope;
use crate::finance_authority::FinanceAuthorityPort;
use crate::ports::broker_submission::{BrokerPreparedBinding, BrokerSubmissionPermit};
use crate::ports::{
    AuthorityContext, BrokerSubmissionPort, ImmutablePutOutcome, PluginOperationRequest,
    PluginOperationResponse, SideEffectContext,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const INTENTS_NS: &str = "broker_order_intents";

pub struct LocalBrokerSubmissionBoundary {
    deps: BrokerSubmissionDependencies,
    authority: Arc<dyn FinanceAuthorityPort>,
}

impl LocalBrokerSubmissionBoundary {
    pub fn new(
        deps: BrokerSubmissionDependencies,
        authority: Arc<dyn FinanceAuthorityPort>,
    ) -> Self {
        Self { deps, authority }
    }

    fn receipt_reference(request: &PluginOperationRequest) -> Result<(String, String), String> {
        let refs: Vec<&str> = request
            .evidence_refs
            .iter()
            .filter(|reference| reference.starts_with("plugin-receipt:"))
            .map(String::as_str)
            .collect();
        if refs.len() != 1 {
            return Err("broker_price_receipt_required".into());
        }
        let receipt_id = refs[0]
            .strip_prefix("plugin-receipt:")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "broker_price_receipt_required".to_string())?;
        Ok((refs[0].to_string(), receipt_id.to_string()))
    }

    fn persist_intent(
        &self,
        key: &str,
        intent: &Value,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        let outcome = self.deps.storage.put_json_if_absent(
            INTENTS_NS,
            &stable_key(key),
            intent.clone(),
            context,
        )?;
        if outcome == ImmutablePutOutcome::AlreadyPresent {
            let existing = self
                .deps
                .storage
                .get_json(INTENTS_NS, &stable_key(key))?
                .ok_or_else(|| "broker_order_intent_record_missing".to_string())?;
            if existing != *intent {
                return Err("broker_order_intent_conflict".into());
            }
        }
        Ok(())
    }
}

impl BrokerSubmissionPort for LocalBrokerSubmissionBoundary {
    fn verify_available(&self) -> Result<(), String> {
        self.authority.verify_broker_boundary()
    }

    fn admit(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
        prepared: &BrokerPreparedBinding,
    ) -> Result<BrokerSubmissionPermit, String> {
        let (price_ref, receipt_id) = Self::receipt_reference(request)?;
        let state = load_current_state(&self.deps, request, context)?;
        let (base_intent, _) = build_order_intent(&state, request, context, prepared)?;
        let symbol = required_value(&base_intent["order"], "symbol")?;
        let price_evidence = prices::load_price_evidence(&self.deps, &state, symbol, &receipt_id)?;
        let quantity = base_intent["order"]["quantityMicros"]
            .as_i64()
            .ok_or_else(|| "invalid_order_quantity".to_string())?;
        let price = price_evidence["priceMicros"]
            .as_i64()
            .ok_or_else(|| "invalid_order_price".to_string())?;
        let risk_evidence =
            risk::enforce_order_limits(quantity, price, &state.config["riskLimits"])?;

        let mut intent = base_intent.clone();
        intent["priceEvidence"] = price_evidence;
        intent["riskEvidence"] = risk_evidence;
        let intent_digest = crate::spec::canonical_hash(&intent)?;

        // Re-read all trusted state after receipt/risk work.  A lease, mandate,
        // or configuration change during that work invalidates this attempt.
        let current = load_current_state(&self.deps, request, context)?;
        let (fresh_base, _) = build_order_intent(&current, request, context, prepared)?;
        if fresh_base != base_intent {
            return Err("broker_submission_state_changed".into());
        }
        let fresh_price = prices::load_price_evidence(&self.deps, &current, symbol, &receipt_id)?;
        let fresh_risk = risk::enforce_order_limits(
            quantity,
            fresh_price["priceMicros"]
                .as_i64()
                .ok_or_else(|| "invalid_order_price".to_string())?,
            &current.config["riskLimits"],
        )?;
        if fresh_price != intent["priceEvidence"] || fresh_risk != intent["riskEvidence"] {
            return Err("broker_submission_evidence_changed".into());
        }

        self.persist_intent(context.idempotency_key.as_str(), &intent, context)?;
        let authority = AuthorityContext {
            actor: current.actor.subject.clone(),
            surface: "broker_boundary".into(),
            account_mode: "live".into(),
        };
        let envelope = ControlPlaneCommandEnvelope {
            schema_version: "tradeassembly.control_plane.command.v1".into(),
            command_id: format!("cmd_{}", hash_text(context.idempotency_key.as_str())),
            correlation_id: request.correlation_id.clone(),
            command_name: "order.submit.live".into(),
            command_group: "order".into(),
            source_interface: "broker_boundary".into(),
            target_object: Some(current.mandate.binding.account_ref.clone()),
            side_effect_class: "live_order".into(),
            authority,
            idempotency_key: context.idempotency_key.clone(),
            idempotency_requirement: "required".into(),
            expected_sequence: None,
            evidence_refs: vec![price_ref, format!("broker-intent:{intent_digest}")],
            payload_hash: intent_digest,
            payload_preview: intent,
        };
        self.authority
            .prepare_broker_submission(self.deps.storage.as_ref(), &envelope)?;
        // C5 is a network round trip. Recheck the trusted lease, controls,
        // mandate, configuration, and price before handing out the permit.
        let post_c5 = load_current_state(&self.deps, request, context)?;
        let (post_c5_base, _) = build_order_intent(&post_c5, request, context, prepared)?;
        if post_c5_base != base_intent {
            return Err("broker_submission_state_changed_after_authorization".into());
        }
        let post_c5_price = prices::load_price_evidence(&self.deps, &post_c5, symbol, &receipt_id)?;
        let post_c5_risk = risk::enforce_order_limits(
            quantity,
            post_c5_price["priceMicros"]
                .as_i64()
                .ok_or_else(|| "invalid_order_price".to_string())?,
            &post_c5.config["riskLimits"],
        )?;
        if post_c5_price != envelope.payload_preview["priceEvidence"]
            || post_c5_risk != envelope.payload_preview["riskEvidence"]
        {
            return Err("broker_submission_evidence_changed_after_authorization".into());
        }
        Ok(BrokerSubmissionPermit { envelope })
    }

    fn complete(
        &self,
        permit: &BrokerSubmissionPermit,
        response: &PluginOperationResponse,
    ) -> Result<(), String> {
        if response.correlation_id != permit.envelope.effective_correlation_id() {
            return Err("broker_submission_response_correlation_mismatch".into());
        }
        if response.reconciliation_required {
            return Err("plugin_order_reconciliation_required".into());
        }
        super::observation::validate_order_identity(&permit.envelope.payload_preview, response)?;
        self.authority
            .complete_broker_submission(self.deps.storage.as_ref(), &permit.envelope, 202)
    }
}

fn stable_key(value: &str) -> String {
    format!("intent_{}", hash_text(value))
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}
