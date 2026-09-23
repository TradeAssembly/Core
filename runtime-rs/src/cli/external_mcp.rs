//! Connection-owned external agent session; never serializes its capability.
use crate::agent_runner::{
    external_session::ExternalAgentSession, VerifiedAgentMcpExecutionContext, DEFAULT_HEARTBEAT_MS,
};
use crate::service::TradeAssemblyService;
use serde_json::{json, Value};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

pub const ATTACH: &str = "tradeassembly.agent.session.attach";
pub const DETACH: &str = "tradeassembly.agent.session.detach";

#[derive(Default)]
struct State {
    session: Option<ExternalAgentSession>,
    request: Option<Value>,
    failure: Option<String>,
}

/// The stdio connection owns this guard. Dropping it stops renewal and revokes
/// the session; an abrupt process death instead relies on lease expiration.
pub struct ExternalMcpConnection {
    state: Arc<Mutex<State>>,
    stop: mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ExternalMcpConnection {
    pub fn new() -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let (stop, receive) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("external-agent-lease".into())
            .spawn(move || {
                while matches!(
                    receive.recv_timeout(Duration::from_millis(DEFAULT_HEARTBEAT_MS)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let Ok(mut state) = shared.lock() else {
                        break;
                    };
                    if state.failure.is_none() {
                        if let Some(session) = state.session.as_mut() {
                            if let Err(code) = session.renew() {
                                state.failure = Some(code);
                            }
                        }
                    }
                }
            })
            .map_err(|_| "external_agent_heartbeat_unavailable")?;
        Ok(Self {
            state,
            stop,
            worker: Some(worker),
        })
    }

    pub fn execution_context(&self) -> Result<Option<VerifiedAgentMcpExecutionContext>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "external_agent_session_unavailable")?;
        if let Some(code) = &state.failure {
            return Err(code.clone());
        }
        state
            .session
            .as_ref()
            .map(ExternalAgentSession::verified_context)
            .transpose()
    }

    pub fn attach(
        &self,
        service: &TradeAssemblyService,
        arguments: &Value,
    ) -> Result<Value, String> {
        let text = |key: &str| {
            arguments[key]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("external_attach_{key}_required"))
        };
        let deployment = text("deployment_id")?;
        let activation = text("activation_id")?;
        let key = text("idempotency_key")?;
        let owner = service
            .responsible_human_identity()
            .ok_or("external_attach_identity_required")?;
        let request =
            json!({"deployment":deployment,"activation":activation,"key":key,"owner":owner});
        let mut state = self
            .state
            .lock()
            .map_err(|_| "external_agent_session_unavailable")?;
        if let Some(code) = &state.failure {
            return Err(code.clone());
        }
        if state.session.is_some() && state.request.as_ref() != Some(&request) {
            return Err("external_agent_connection_already_attached".into());
        }
        if state.session.is_none() {
            let session = service
                .attach_external_agent_session(deployment, activation, key)
                .map_err(|response| {
                    response.body["error"]["code"]
                        .as_str()
                        .unwrap_or("external_attach_denied")
                        .to_string()
                })?;
            state.session = Some(session);
            state.request = Some(request);
        }
        let context = state
            .session
            .as_ref()
            .ok_or("external_agent_session_unavailable")?
            .verified_context()?;
        Ok(
            json!({"status":"attached","deploymentId":context.deployment_id(),"runId":context.run_id(),"mode":context.mode(),"orchestrator":"external_agent","executor":"external_client","redacted":true}),
        )
    }

    pub fn detach(&self) -> Result<Value, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "external_agent_session_unavailable")?;
        let result = state
            .session
            .take()
            .map(|mut session| session.close())
            .transpose();
        state.failure = None;
        state.request = None;
        result?;
        Ok(json!({"status":"detached","reconciliationRequired":true}))
    }
}

impl Drop for ExternalMcpConnection {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = self.detach();
    }
}
