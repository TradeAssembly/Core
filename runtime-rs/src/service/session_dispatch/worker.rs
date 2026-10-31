//! Explicit owned local worker intent, not a serialized principal or grant.
use super::*;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
const INTENTS: &str = "session_dispatch_worker_intents_v1";
const MAX_WORKERS: usize = 16;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    schema_version: String,
    job_id: String,
    owner_sha256: String,
    core_revision: String,
    enabled: bool,
    record_sha256: String,
}
fn digest(i: &Intent) -> Result<String, String> {
    let mut copy = i.clone();
    copy.record_sha256.clear();
    hash(&copy)
}
fn intent(s: &TradeAssemblyService, id: &str) -> Result<Option<Intent>, String> {
    let Some(v) = s.runtime().storage.get_json(INTENTS, id)? else {
        return Ok(None);
    };
    let i: Intent =
        serde_json::from_value(v).map_err(|_| "session_dispatch_worker_intent_invalid")?;
    let (j, _) = read(s, id)?;
    if i.schema_version != "tradeassembly.session_dispatch.worker_intent.v1"
        || i.job_id != id
        || i.owner_sha256 != j.owner_sha256
        || i.core_revision != j.core_revision
        || i.record_sha256 != digest(&i)?
    {
        return Err("session_dispatch_worker_intent_invalid".into());
    }
    Ok(Some(i))
}
fn set(s: &TradeAssemblyService, id: &str, enabled: bool) -> Result<(), String> {
    let who = owner(s)?;
    let (j, _) = read(s, id)?;
    if j.core_revision != revision()? || j.owner_sha256 != who {
        return Err("session_dispatch_source_changed".into());
    }
    let old = s.runtime().storage.get_json(INTENTS, id)?;
    if old.is_some() {
        let i = intent(s, id)?.ok_or("session_dispatch_worker_intent_invalid")?;
        if i.enabled == enabled {
            return Ok(());
        }
    }
    let mut i = Intent {
        schema_version: "tradeassembly.session_dispatch.worker_intent.v1".into(),
        job_id: id.into(),
        owner_sha256: who,
        core_revision: j.core_revision,
        enabled,
        record_sha256: String::new(),
    };
    i.record_sha256 = digest(&i)?;
    let key = format!("worker-intent:{}", hash(&(&old, &i))?);
    if s.runtime().storage.put_json_batch(
        &[StorageWrite::new(INTENTS, id, value(&i)?, context(&key)?)],
        &[StorageExpectation::new(INTENTS, id, old)],
    )? != ComparePutOutcome::Updated
    {
        return Err("session_dispatch_worker_intent_conflict".into());
    }
    Ok(())
}
struct Wake {
    sequence: Mutex<u64>,
    changed: Condvar,
    #[cfg(test)]
    halted: std::sync::atomic::AtomicBool,
}
impl Wake {
    fn new() -> Self {
        Self {
            sequence: Mutex::new(0),
            changed: Condvar::new(),
            #[cfg(test)]
            halted: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn notify(&self) {
        let mut n = self.sequence.lock().unwrap_or_else(|p| p.into_inner());
        *n = n.wrapping_add(1);
        self.changed.notify_all();
    }
    fn wait(&self, observed: u64, ms: u64) {
        let n = self.sequence.lock().unwrap_or_else(|p| p.into_inner());
        if *n == observed {
            let _unused = self
                .changed
                .wait_timeout(n, Duration::from_millis(ms.min(60_000)))
                .unwrap_or_else(|p| p.into_inner());
        }
    }
}
struct Slot {
    wake: Arc<Wake>,
    running: bool,
    iterations: u64,
    last_error: Option<String>,
}
fn registry() -> &'static Mutex<BTreeMap<String, Slot>> {
    static SLOTS: OnceLock<Mutex<BTreeMap<String, Slot>>> = OnceLock::new();
    SLOTS.get_or_init(Default::default)
}
fn prune(slots: &mut BTreeMap<String, Slot>) {
    while slots.len() >= 128 {
        let Some(old) = slots
            .iter()
            .find(|(_, v)| !v.running)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        slots.remove(&old);
    }
}
fn key(s: &TradeAssemblyService, id: &str) -> Result<String, String> {
    let db = std::path::Path::new(s.db())
        .canonicalize()
        .map_err(|_| "session_dispatch_database_identity_unavailable")?;
    hash(&(db, id))
}
pub(super) fn status(s: &TradeAssemblyService, id: &str) -> Value {
    let Ok(key) = key(s, id) else {
        return json!({"running":false,"lastError":"session_dispatch_database_identity_unavailable"});
    };
    let slots = registry().lock().unwrap_or_else(|p| p.into_inner());
    match slots.get(&key) {
        Some(v) => json!({"running":v.running,"iterations":v.iterations,"lastError":v.last_error}),
        None => json!({"running":false,"iterations":0,"lastError":null}),
    }
}
pub(super) fn wake(s: &TradeAssemblyService, id: &str) {
    if let Ok(k) = key(s, id) {
        if let Some(v) = registry().lock().unwrap_or_else(|p| p.into_inner()).get(&k) {
            v.wake.notify();
        }
    }
}
pub(super) fn enabled(s: &TradeAssemblyService, id: &str) -> Result<bool, String> {
    Ok(intent(s, id)?.is_some_and(|i| i.enabled))
}
pub(super) fn start(s: &TradeAssemblyService, id: &str) -> Result<(), String> {
    if s.agent_mcp_execution_context().is_some() {
        return Err("session_dispatch_background_agent_delegation_unsupported".into());
    }
    owner(s)?;
    let (j, _) = read(s, id)?;
    if j.core_revision != revision()? {
        return Err("session_dispatch_source_changed".into());
    }
    if j.status.terminal() {
        return Err("session_dispatch_job_terminal".into());
    }
    if j.paused {
        return Err("session_dispatch_job_paused".into());
    }
    validate_request(s, &j.request)?;
    let _ = key(s, id)?;
    set(s, id, true)?;
    spawn(s.clone(), id.into())
}
pub(super) fn stop(s: &TradeAssemblyService, id: &str) -> Result<(), String> {
    set(s, id, false)?;
    wake(s, id);
    Ok(())
}
fn spawn(service: TradeAssemblyService, id: String) -> Result<(), String> {
    let k = key(&service, &id)?;
    let wake = {
        let mut slots = registry().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = slots.get(&k).filter(|v| v.running) {
            v.wake.notify();
            return Ok(());
        }
        if slots.values().filter(|v| v.running).count() >= MAX_WORKERS {
            return Err("session_dispatch_worker_capacity_exceeded".into());
        }
        // Bounded ephemeral diagnostics; durable job/intent evidence is retained.
        prune(&mut slots);
        let wake = Arc::new(Wake::new());
        slots.insert(
            k.clone(),
            Slot {
                wake: wake.clone(),
                running: true,
                iterations: 0,
                last_error: None,
            },
        );
        wake
    };
    let failed_key = k.clone();
    let spawned = std::thread::Builder::new()
        .name("ta-session-dispatch".into())
        .spawn(move || {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_loop(&service, &id, &k, &wake)
            }));
            let error = match run {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("session_dispatch_worker_panicked".into()),
            };
            if let Some(v) = registry()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_mut(&k)
            {
                v.running = false;
                v.last_error = error.clone();
            }
            // Close the pause/unpause and stop/start race at worker exit. Only
            // graceful stop or revision contention may resume the same retained
            // work; identity/policy/source refusals remain stopped and visible.
            let retry =
                error.is_none() || error.as_deref() == Some("session_dispatch_revision_conflict");
            if retry
                && owner(&service).is_ok()
                && enabled(&service, &id) == Ok(true)
                && read(&service, &id).is_ok_and(|(j, _)| !j.paused && !j.status.terminal())
            {
                if let Err(code) = spawn(service.clone(), id.clone()) {
                    if let Some(v) = registry()
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get_mut(&k)
                        .filter(|v| !v.running)
                    {
                        v.last_error = Some(code);
                    }
                }
            }
        });
    if spawned.is_err() {
        if let Some(v) = registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&failed_key)
        {
            v.running = false;
            v.last_error = Some("session_dispatch_worker_spawn_failed".into());
        }
        return Err("session_dispatch_worker_spawn_failed".into());
    }
    Ok(())
}
fn run_loop(s: &TradeAssemblyService, id: &str, k: &str, wake: &Wake) -> Result<(), String> {
    let mut nonce = [0u8; 16];
    use rand::RngCore;
    rand::rngs::OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| "session_dispatch_worker_nonce_unavailable")?;
    let transport_run = hash(&nonce)?;
    let mut iteration = 0u64;
    loop {
        #[cfg(test)]
        if wake.halted.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("fixture_worker_stopped_without_disabling_intent".into());
        }
        let observed = *wake.sequence.lock().unwrap_or_else(|p| p.into_inner());
        owner(s)?;
        if !enabled(s, id)? {
            return Ok(());
        }
        let (j, _) = read(s, id)?;
        if j.status.terminal() || j.paused {
            return Ok(());
        }
        iteration = iteration
            .checked_add(1)
            .ok_or("session_dispatch_worker_iteration_exhausted")?;
        // Fresh transport command; the durable domain child ID/body never changes.
        let response=s.handle_http("POST","/scheduler/run",json!({"accountMode":"paper","sessionDispatch":{"kind":"advance","jobId":id},"idempotencyKey":format!("session-worker:{transport_run}:{iteration}")}));
        if let Some(v) = registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(k)
        {
            v.iterations = iteration;
        }
        if response.status >= 400 || response.body["ok"] == false {
            return Err(super::super::service_error_code(&response.body)
                .unwrap_or_else(|| "session_dispatch_worker_command_failed".into()));
        }
        let (j, _) = read(s, id)?;
        if j.status.terminal() || j.paused {
            return Ok(());
        }
        let schedule = session_schedules::current(s, &j.request.occurrence_id)?;
        let now = s.runtime().clock.now_ms();
        let due = if j.started_at_ms.is_none() {
            schedule.occurrence.due_at_ms.unwrap_or(now)
        } else {
            now
        };
        let target = due.max(j.next_attempt_at_ms);
        let delay = if target > now {
            (target - now) as u64
        } else if j.pending.is_some() {
            5000
        } else {
            1
        };
        wake.wait(observed, delay);
    }
}
pub(super) fn resume_owned(s: &TradeAssemblyService) -> Result<usize, String> {
    owner(s)?;
    let rows = s.runtime().storage.list_json_page(INTENTS, None, 129)?;
    if rows.len() > 128 {
        return Err("session_dispatch_resume_limit_exceeded".into());
    }
    let mut count = 0;
    for (id, _) in rows {
        // A different owner's intent is not authority and cannot be read or resumed.
        if read(s, &id).is_err() {
            continue;
        }
        if !enabled(s, &id)? {
            continue;
        }
        let (j, _) = read(s, &id)?;
        if j.status.terminal() || j.paused {
            continue;
        }
        match start(s, &id) {
            Ok(()) => count += 1,
            Err(error) => {
                let k = key(s, &id)?;
                let mut slots = registry().lock().unwrap_or_else(|p| p.into_inner());
                prune(&mut slots);
                slots.insert(
                    k,
                    Slot {
                        wake: Arc::new(Wake::new()),
                        running: false,
                        iterations: 0,
                        last_error: Some(error),
                    },
                );
            }
        }
    }
    Ok(count)
}
pub(crate) fn resume_local(s: &TradeAssemblyService) {
    if s.runtime_manifest().is_none_or(|m| m.profile != "local") {
        return;
    }
    if !s
        .runtime()
        .storage
        .list_json_page(INTENTS, None, 1)
        .is_ok_and(|rows| !rows.is_empty())
    {
        return;
    }
    let Ok(db) = std::path::Path::new(s.db()).canonicalize() else {
        return;
    };
    // Never create/regenerate identity or obtain a principal from a job record.
    let Ok(identity) = crate::local_owner_identity::LocalOwnerIdentity::existing_for_database(&db)
    else {
        eprintln!("session dispatch startup resume refused: session_dispatch_identity_unavailable");
        return;
    };
    let scoped =
        s.for_authenticated_invocation(&identity.issuer, &identity.stable_identity_id, None, None);
    if let Err(code) = resume_owned(&scoped) {
        eprintln!("session dispatch startup resume refused: {code}");
    }
}

#[cfg(test)]
pub(super) fn halt_for_test(s: &TradeAssemblyService, id: &str) {
    let k = key(s, id).unwrap();
    let slots = registry().lock().unwrap();
    let v = slots.get(&k).unwrap();
    v.wake
        .halted
        .store(true, std::sync::atomic::Ordering::SeqCst);
    v.wake.notify();
}
