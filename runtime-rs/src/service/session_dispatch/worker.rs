//! Explicit owned local worker intent, not a serialized principal or grant.
use super::*;
use crate::service::execution::automatic_health;
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
    let mut result = match slots.get(&key) {
        Some(v) => json!({"running":v.running,"iterations":v.iterations,"lastError":v.last_error}),
        None => json!({"running":false,"iterations":0,"lastError":null}),
    };
    drop(slots);
    if let Ok((job, _)) = read(s, id) {
        let health = automatic_health::status_job(s, &job);
        if health["enabled"] == true || health["status"] == "blocked" {
            result["accountHealthRefresh"] = health;
        }
    }
    result
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
fn management_work(s: &TradeAssemblyService, j: &Job) -> Result<bool, String> {
    let _timing = crate::service::order_plans::financial_timing("worker.management_work");
    crate::service::order_plans::management::automation::has_work(s, j)
}
pub(super) fn has_work(s: &TradeAssemblyService, j: &Job) -> Result<bool, String> {
    let _timing = crate::service::order_plans::financial_timing("worker.has_work");
    Ok(!j.status.terminal() || automatic_health::needs_job_turn(s, j)? || management_work(s, j)?)
}
#[derive(Debug)]
struct SchedulingDecision {
    has_work: bool,
    management_due: Option<i64>,
}
// A decision consumes one fresh management read for both eligibility and due
// time. It is discarded before the next iteration; normal child admission
// independently rechecks the owned intent, binding, current sources and clock.
fn scheduling_decision(
    terminal: bool,
    health_work: bool,
    management: impl FnOnce() -> Result<
        crate::service::order_plans::management::automation::SchedulingState,
        String,
    >,
) -> Result<SchedulingDecision, String> {
    let management = management()?;
    Ok(SchedulingDecision {
        has_work: !terminal || health_work || management.has_work,
        management_due: management.has_work.then_some(management.next_due_ms),
    })
}
fn start_admission(
    terminal: bool,
    management_work: impl FnOnce() -> Result<bool, String>,
    validate_entry: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if terminal {
        if !management_work()? {
            return Err("session_dispatch_job_terminal".into());
        }
        return Ok(());
    }
    validate_entry()
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
    if j.paused {
        return Err("session_dispatch_job_paused".into());
    }
    if !automatic_health::needs_job_turn(s, &j)? {
        start_admission(
            j.status.terminal(),
            || management_work(s, &j),
            || validate_request(s, &j.request).map(|_| ()),
        )?;
    }
    // An opted-in stale/degraded run may start only the health-read work first.
    // The normal calendar/config/financial checks still run before any trading.
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
                && read(&service, &id)
                    .is_ok_and(|(j, _)| !j.paused && has_work(&service, &j) == Ok(true))
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
fn wake_deadline(
    terminal: bool,
    management_due: Option<i64>,
    entry_due: impl FnOnce() -> Result<i64, String>,
) -> Result<i64, String> {
    if terminal {
        if let Some(due) = management_due {
            return Ok(due);
        }
    }
    let entry = entry_due()?;
    Ok(management_due.map_or(entry, |due| entry.min(due)))
}
// Set only here, never from a request body, durable intent or remote principal.
fn background_service(s: &TradeAssemblyService, id: &str) -> TradeAssemblyService {
    let mut scoped = s.clone();
    scoped.background_worker_job = Some(id.into());
    scoped
}
fn after_health(s: &TradeAssemblyService, id: &str) -> Result<Option<Job>, String> {
    // A completed health read is evidence, not permission for the next turn.
    owner(s)?;
    if !enabled(s, id)? {
        return Ok(None);
    }
    let (job, _) = read(s, id)?;
    if job.paused {
        return Ok(None);
    }
    if job.core_revision != revision()? {
        return Err("session_dispatch_source_changed".into());
    }
    Ok(Some(job))
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
        if j.paused {
            return Ok(());
        }
        let health = automatic_health::advance_worker_job(s, &j)?;
        if !health.ready {
            let now = s.runtime().clock.trusted_now_ms()?;
            wake.wait(
                observed,
                health
                    .next_attempt_at_ms
                    .saturating_sub(now)
                    .clamp(1, 60_000) as u64,
            );
            continue;
        }
        let Some(j) = after_health(s, id)? else {
            return Ok(());
        };
        let health_work = j.status.terminal() && automatic_health::needs_job_turn(s, &j)?;
        let scheduling = scheduling_decision(j.status.terminal(), health_work, || {
            crate::service::order_plans::management::automation::scheduling_state(s, &j)
        })?;
        if !scheduling.has_work {
            return Ok(());
        }
        iteration = iteration
            .checked_add(1)
            .ok_or("session_dispatch_worker_iteration_exhausted")?;
        let now = s.runtime().clock.trusted_now_ms()?;
        let management_ready = scheduling.management_due.is_some_and(|due| due <= now);
        // Existing positions get bounded turns while selection/warmup is pending too.
        let manage = j.status.terminal() || (management_ready && iteration.is_multiple_of(2));
        // Fresh transport command; the durable domain child ID/body never changes.
        let background = background_service(s, id);
        let response=background.handle_http("POST","/scheduler/run",json!({"accountMode":"paper","sessionDispatch":{"kind":if manage{"manage"}else{"advance"},"jobId":id},"idempotencyKey":format!("session-worker:{transport_run}:{iteration}")}));
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
        if j.paused {
            return Ok(());
        }
        // A long computation may cross the health TTL. Return to the read gate
        // before asking management's normal currentness checks about more work.
        if automatic_health::needs_job_turn(s, &j)? {
            continue;
        }
        let health_work = j.status.terminal() && automatic_health::needs_job_turn(s, &j)?;
        let scheduling = scheduling_decision(j.status.terminal(), health_work, || {
            crate::service::order_plans::management::automation::scheduling_state(s, &j)
        })?;
        if !scheduling.has_work {
            return Ok(());
        }
        let now = s.runtime().clock.now_ms();
        let management_remaining = scheduling.management_due.is_some();
        let management_due = scheduling.management_due;
        let mut target = wake_deadline(j.status.terminal(), management_due, || {
            // Nonterminal entry jobs retain their original validation and due time.
            let schedule = session_schedules::current(s, &j.request.occurrence_id)?;
            let due = if j.started_at_ms.is_none() {
                schedule.occurrence.due_at_ms.unwrap_or(now)
            } else {
                now
            };
            Ok(due.max(j.next_attempt_at_ms))
        })?;
        if manage {
            if let Some(next) = response.body["body"]["management"]["nextAttemptAtMs"].as_i64() {
                target = target.max(next)
            }
        }
        let health = automatic_health::status_job(s, &j);
        if let Some(due) = health["nextAttemptAtMs"]
            .as_i64()
            .filter(|_| health["enabled"] == true)
        {
            target = target.min(due);
        }
        let delay = if target > now {
            (target - now) as u64
        } else if j.pending.is_some() && !management_remaining {
            5000
        } else {
            1
        };
        wake.wait(observed, delay);
    }
}
fn record_resume_failure(s: &TradeAssemblyService, id: &str, error: String) -> Result<(), String> {
    let k = key(s, id)?;
    let mut slots = registry().lock().unwrap_or_else(|p| p.into_inner());
    prune(&mut slots);
    if let Some(slot) = slots.get_mut(&k) {
        // Do not replace an independently running worker's wake/identity.
        slot.last_error = Some(error);
    } else {
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
    Ok(())
}
pub(super) fn resume_owned(s: &TradeAssemblyService) -> Result<usize, String> {
    resume_owned_with(s, start)
}
fn resume_owned_with(
    s: &TradeAssemblyService,
    mut start_job: impl FnMut(&TradeAssemblyService, &str) -> Result<(), String>,
) -> Result<usize, String> {
    // Owner resolution and global inventory bounds still fail the whole sweep.
    owner(s)?;
    let rows = s.runtime().storage.list_json_page(INTENTS, None, 129)?;
    if rows.len() > 128 {
        return Err("session_dispatch_resume_limit_exceeded".into());
    }
    let mut count = 0;
    for (id, _) in rows {
        // Never diagnose or start a foreign/unverifiable job using its intent.
        if read(s, &id).is_err() {
            continue;
        }
        let result = (|| {
            if !enabled(s, &id)? {
                return Ok(false);
            }
            let (job, _) = read(s, &id)?;
            if job.paused || !has_work(s, &job)? {
                return Ok(false);
            }
            start_job(s, &id)?;
            Ok(true)
        })();
        match result {
            Ok(true) => count += 1,
            Ok(false) => {}
            Err(error) => record_resume_failure(s, &id, error)?,
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

#[cfg(test)]
mod management_wake_tests {
    use super::*;
    #[test]
    fn terminal_owned_management_does_not_read_stale_entry_schedule() {
        assert_eq!(
            wake_deadline(true, Some(1234), || panic!(
                "original schedule must not be read"
            ))
            .unwrap(),
            1234
        );
    }
    #[test]
    fn nonterminal_entry_validation_and_earliest_management_turn_are_preserved() {
        assert_eq!(wake_deadline(false, Some(1234), || Ok(2000)).unwrap(), 1234);
        assert_eq!(
            wake_deadline(false, Some(1234), || Err("original_schedule_stale".into())).unwrap_err(),
            "original_schedule_stale"
        );
        assert_eq!(wake_deadline(false, None, || Ok(2000)).unwrap(), 2000);
    }
}

#[cfg(test)]
mod management_start_tests {
    use super::start_admission;

    #[test]
    fn terminal_managed_start_skips_original_entry_calendar() {
        assert_eq!(
            start_admission(
                true,
                || Ok(true),
                || panic!("old calendar must not be read")
            ),
            Ok(())
        );
    }

    #[test]
    fn nonterminal_start_preserves_entry_calendar_validation() {
        let called = std::cell::Cell::new(false);
        let result = start_admission(
            false,
            || panic!("entry admission must not require management work"),
            || {
                called.set(true);
                Err("original_schedule_stale".into())
            },
        );
        assert!(called.get());
        assert_eq!(result.unwrap_err(), "original_schedule_stale");
        assert_eq!(start_admission(false, || Ok(false), || Ok(())), Ok(()));
    }

    #[test]
    fn terminal_start_requires_current_management_work() {
        assert_eq!(
            start_admission(
                true,
                || Ok(false),
                || panic!("old calendar must not be read")
            )
            .unwrap_err(),
            "session_dispatch_job_terminal"
        );
        assert_eq!(
            start_admission(
                true,
                || Err("managed_controls_blocked".into()),
                || panic!("old calendar must not be read"),
            )
            .unwrap_err(),
            "managed_controls_blocked"
        );
    }
}

#[cfg(test)]
mod health_worker_recovery_tests {
    include!("worker_health_recovery_tests.rs");

    #[test]
    fn scheduling_decision_reads_management_once_and_preserves_entry_health_and_wake_priority() {
        use crate::service::order_plans::management::automation::SchedulingState;
        for terminal in [false, true] {
            for health_work in [false, true] {
                for management_work in [false, true] {
                    let reads = std::cell::Cell::new(0);
                    let decision = scheduling_decision(terminal, health_work, || {
                        reads.set(reads.get() + 1);
                        Ok(SchedulingState {
                            has_work: management_work,
                            next_due_ms: 1234,
                        })
                    })
                    .unwrap();
                    assert_eq!(reads.get(), 1);
                    assert_eq!(
                        decision.has_work,
                        !terminal || health_work || management_work
                    );
                    assert_eq!(decision.management_due, management_work.then_some(1234));
                    if terminal && management_work {
                        assert_eq!(
                            wake_deadline(terminal, decision.management_due, || panic!(
                                "terminal management must not read the old entry calendar"
                            ))
                            .unwrap(),
                            1234
                        );
                    } else {
                        assert_eq!(
                            wake_deadline(terminal, decision.management_due, || Ok(2000)).unwrap(),
                            if management_work { 1234 } else { 2000 }
                        );
                    }
                }
            }
        }
        assert_eq!(
            scheduling_decision(false, true, || Err("current_source_refused".into())).unwrap_err(),
            "current_source_refused"
        );
    }

    #[test]
    fn advisory_readiness_never_authorizes_a_child_after_stop_pause_or_source_change() {
        for change in ["stop", "pause", "source"] {
            let (_dir, s) = fixture();
            let id = retain(&s, &format!("advisory-{change}"));
            let (_release_tx, release_rx) = mpsc::channel();
            let port = Arc::new(HeldHealth {
                entered: Mutex::new(None),
                release: Mutex::new(release_rx),
                health_reads: AtomicUsize::new(0),
                next_children: AtomicUsize::new(0),
                clock: s.runtime().clock.clone(),
            });
            let s = install_held_health(&s, port.clone());
            let (job, _) = read(&s, &id).unwrap();
            let advisory = scheduling_decision(job.status.terminal(), false, || {
                crate::service::order_plans::management::automation::scheduling_state(&s, &job)
            })
            .unwrap();
            assert!(advisory.has_work);
            let background = background_service(&s, &id);
            let expected = match change {
                "stop" => {
                    stop(&s, &id).unwrap();
                    "automatic_health_worker_intent_disabled"
                }
                "pause" => {
                    update(&s, &id, |j| j.paused = true);
                    "automatic_health_job_intent_stale"
                }
                _ => {
                    // A retained job from another Core source cannot use this
                    // iteration's earlier read to dispatch a normal HTTP child.
                    update(&s, &id, |j| j.core_revision = "f".repeat(40));
                    "automatic_health_job_intent_stale"
                }
            };
            let denied = background.handle_http(
                "POST",
                "/plugins/instances/worker.health/operations/next:invoke",
                json!({"mode":"paper","purpose":"account_health","input":{},
                    "idempotencyKey":"advisory-must-not-dispatch"}),
            );
            assert_eq!(denied.status, 409, "{}", denied.body);
            assert_eq!(denied.body["error"]["code"], expected);
            assert_eq!(port.next_children.load(Ordering::SeqCst), 0);
            assert_eq!(port.health_reads.load(Ordering::SeqCst), 0);
        }
    }
}
