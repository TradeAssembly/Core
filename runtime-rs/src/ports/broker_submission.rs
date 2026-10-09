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

    fn complete(
        &self,
        permit: &BrokerSubmissionPermit,
        response: &PluginOperationResponse,
    ) -> Result<(), String>;
}
