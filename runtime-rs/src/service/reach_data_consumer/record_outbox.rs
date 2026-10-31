//! One durable acknowledgment slot per independently verified node/source.
//! Never persist a caller principal or replay a transport signature.
use super::*;
use crate::ports::{
    AuthorityContext, ComparePutOutcome, IdempotencyKey, SideEffectContext, StorageExpectation,
    StorageWrite,
};
const OUTBOX: &str = "reach_data_record_outbox_v1";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingAck {
    delivery: DataDelivery,
    receipt: DataRunnerReceipt,
}
impl ReachDataConsumer {
    fn outbox_key(&self) -> Result<String, String> {
        canonical_sha256(&(
            "tradeassembly.reach-record-outbox.v1",
            &self.node,
            &self.source,
        ))
        .map_err(|_| "reach_data_encoding_invalid".into())
    }
    fn outbox_write(
        &self,
        prior: Option<serde_json::Value>,
        next: serde_json::Value,
        id: &str,
    ) -> Result<(), String> {
        let key = self.outbox_key()?;
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new(format!("reach-record-outbox:{key}:{id}"))?,
        );
        match self.service.runtime().storage.put_json_batch(
            &[StorageWrite::new(OUTBOX, &key, next, ctx)],
            &[StorageExpectation::new(OUTBOX, &key, prior)],
        )? {
            ComparePutOutcome::Updated => Ok(()),
            ComparePutOutcome::Conflict => Err("reach_data_record_outbox_conflict".into()),
        }
    }
    pub(super) fn stage_ack(
        &self,
        delivery: &DataDelivery,
        receipt: &DataRunnerReceipt,
    ) -> Result<(), String> {
        let old = self
            .service
            .runtime()
            .storage
            .get_json(OUTBOX, &self.outbox_key()?)?;
        let next = json!({"pending":PendingAck{delivery:delivery.clone(),receipt:receipt.clone()}});
        if old.as_ref() == Some(&next) {
            return Ok(());
        }
        if old.as_ref().is_some_and(|v| v.get("pending").is_some()) {
            return Err("reach_data_record_outbox_busy".into());
        }
        self.outbox_write(old, next, &format!("pending:{}", receipt.receipt_sha256))
    }
    pub(super) fn reconcile_pending_ack(&self) -> Result<Option<ReachDataPoll>, String> {
        let Some(old) = self
            .service
            .runtime()
            .storage
            .get_json(OUTBOX, &self.outbox_key()?)?
        else {
            return Ok(None);
        };
        let Some(raw) = old.get("pending") else {
            return Ok(None);
        };
        if serde_json::to_vec(raw)
            .map_err(|_| "reach_data_encoding_invalid")?
            .len()
            > 655360
        {
            return Err("reach_data_record_outbox_invalid".into());
        }
        let p: PendingAck =
            serde_json::from_value(raw.clone()).map_err(|_| "reach_data_record_outbox_invalid")?;
        p.delivery.validate()?;
        let c = &p.delivery.command;
        if c.node != self.node
            || c.source_binding_sha256 != self.source
            || source_binding(&self.service, &self.node)? != self.source
        {
            return Err("reach_data_source_scope_unavailable".into());
        }
        let current = self.runner.reconciliation_binding(&c.command)?;
        c.validate_reconciliation_bindings(
            &self.node,
            &self.source,
            &current,
            self.service.runtime().clock.now_ms(),
        )?;
        if self
            .runner
            .receipt(&c.node.workspace_id, &c.command.command_id)?
            .as_ref()
            != Some(&p.receipt)
        {
            return Err("reach_data_record_outbox_receipt_changed".into());
        }
        // Fresh nonce/signature, original receipt and accepted revision. The
        // server must recognize this original record after a committed/lost ack.
        let request = self.signed(
            DataNodeOperation::Record,
            c.request_id.clone(),
            Some(p.delivery.command_sha256.clone()),
            Some(p.receipt.receipt_sha256.clone()),
            Some(p.delivery.revision),
        )?;
        let ack = self.transport.record(&DataNodeRecord {
            request,
            receipt: p.receipt.clone(),
        })?;
        ack.validate()?;
        if serde_json::to_value(&ack.command).map_err(|_| "reach_data_encoding_invalid")?
            != serde_json::to_value(&p.delivery.command)
                .map_err(|_| "reach_data_encoding_invalid")?
            || ack.command_sha256 != p.delivery.command_sha256
            || ack.revision < p.delivery.revision
            || ack.receipt.as_ref() != Some(&p.receipt)
        {
            return Err("reach_data_record_ack_invalid".into());
        }
        self.outbox_write(
            Some(old),
            json!({"acknowledged":p.receipt.receipt_sha256}),
            &format!("ack:{}", p.receipt.receipt_sha256),
        )?;
        Ok(Some(ReachDataPoll::Recorded {
            request_id: c.request_id.clone(),
            state: ack.state,
            receipt_sha256: p.receipt.receipt_sha256,
        }))
    }
}
