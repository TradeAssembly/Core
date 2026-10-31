use crate::control_plane::ControlPlaneCommandEnvelope;
use crate::ports::{PluginOperationRequest, PluginOperationResponse, SideEffectContext};

/// Non-secret binding of the invocation that will actually reach the sink.
/// Created by external preparation, never decoded from caller arguments.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerPreparedBinding {
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub package_sha256: String,
    pub manifest_fingerprint: String,
    pub configuration_digest: String,
    pub credential_ref: String,
    pub credential_generation: u64,
}

/// The final policy boundary for a broker side effect.
///
/// Implementations must derive their envelope from trusted persisted state;
/// callers cannot supply or widen the envelope through plugin JSON.
pub struct BrokerSubmissionPermit {
    pub envelope: ControlPlaneCommandEnvelope,
}

pub trait BrokerSubmissionPort: Send + Sync {
    /// Check availability of the enforcing boundary, never authorization for an
    /// order. Unknown adapters must not advertise a protected dispatch path.
    fn verify_available(&self) -> Result<(), String> {
        Err("broker_submission_boundary_unavailable".into())
    }

    fn admit(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
        prepared: &BrokerPreparedBinding,
    ) -> Result<BrokerSubmissionPermit, String>;

    /// Recheck a permit after a local durability wait without another effect.
    fn revalidate_after_checkpoint(
        &self,
        _request: &PluginOperationRequest,
        _context: &SideEffectContext,
        _prepared: &BrokerPreparedBinding,
        _permit: &BrokerSubmissionPermit,
    ) -> Result<(), String> {
        Err("broker_checkpoint_revalidation_unsupported".into())
    }

    fn prepare_recovery_v2(
        &self,
        _source: &crate::broker_submission::v2::OrderPlanSource,
        _context: &SideEffectContext,
    ) -> Result<crate::broker_submission::v2::RecoveryPlan, String> {
        Err("broker_v2_recovery_unsupported".into())
    }
    fn admit_v2(
        &self,
        _request: &PluginOperationRequest,
        _context: &SideEffectContext,
        _prepared: &BrokerPreparedBinding,
        _source: &crate::broker_submission::v2::OrderPlanSource,
    ) -> Result<BrokerSubmissionPermit, String> {
        Err("broker_v2_boundary_unsupported".into())
    }
    fn revalidate_v2(
        &self,
        _request: &PluginOperationRequest,
        _context: &SideEffectContext,
        _prepared: &BrokerPreparedBinding,
        _permit: &BrokerSubmissionPermit,
        _source: &crate::broker_submission::v2::OrderPlanSource,
    ) -> Result<(), String> {
        Err("broker_v2_boundary_unsupported".into())
    }
    fn complete_v2(
        &self,
        _permit: &BrokerSubmissionPermit,
        _response: &PluginOperationResponse,
        _source: &crate::broker_submission::v2::OrderPlanSource,
    ) -> Result<(), String> {
        Err("broker_v2_boundary_unsupported".into())
    }
    fn complete(
        &self,
        permit: &BrokerSubmissionPermit,
        response: &PluginOperationResponse,
    ) -> Result<(), String>;
}
