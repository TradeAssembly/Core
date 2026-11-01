// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Synthetic-only frozen-Core process qualification. Never a provider/demo runner.
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
const MAX_MESSAGE: usize = 1_048_576;
const MAX_ROWS: usize = 4096;
const MAX_DB_BYTES: usize = 16 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(120);
static DRIVER_DEADLINE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
fn remaining(timeout: Duration) -> Result<Duration> {
    match DRIVER_DEADLINE.get() {
        Some(deadline) => deadline
            .checked_duration_since(Instant::now())
            .map(|left| left.min(timeout))
            .ok_or("fixture_driver_timeout".into()),
        None => Ok(timeout),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    root: PathBuf,
    binary: PathBuf,
    binary_sha256: String,
    revision: String,
    job: Value,
    evidence: PathBuf,
    warden_origin: String,
    srt: PathBuf,
    srt_sha256: String,
}
fn require(ok: bool, code: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(code.into())
    }
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn sha256(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "fixture_binary_unreadable")?;
    require(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() > 0
            && metadata.len() <= 1_073_741_824,
        "fixture_binary_file_invalid",
    )?;
    let mut source = File::open(path).map_err(|_| "fixture_binary_unreadable")?;
    let mut total = 0u64;
    let mut hash = Sha256::new();
    let mut chunk = [0; 65536];
    loop {
        let n = source
            .read(&mut chunk)
            .map_err(|_| "fixture_binary_unreadable")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        require(
            total <= metadata.len(),
            "fixture_binary_changed_during_hash",
        )?;
        hash.update(&chunk[..n]);
    }
    require(
        total == metadata.len(),
        "fixture_binary_changed_during_hash",
    )?;
    Ok(format!("{:x}", hash.finalize()))
}
fn loopback_origin(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "http"
            && u.host_str() == Some("127.0.0.1")
            && u.port().is_some_and(|p| p > 0)
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|_| "fixture_path_unavailable")?;
    require(
        !meta.file_type().is_symlink()
            && if directory {
                meta.is_dir()
            } else {
                meta.is_file()
            },
        "fixture_path_kind_invalid",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        require(
            meta.permissions().mode() & 0o077 == 0,
            "fixture_path_not_private",
        )?;
    }
    Ok(())
}
fn new_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "fixture_evidence_create_failed".into())
}
fn save(out: &Path, name: &str, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "fixture_evidence_encoding_failed")?;
    require(bytes.len() <= MAX_DB_BYTES, "fixture_evidence_too_large")?;
    let mut file = new_file(&out.join(format!("{name}.json")))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|_| "fixture_evidence_write_failed".into())
}
#[derive(Clone, Copy)]
enum Namespace {
    Orders,
    Positions,
    Operations,
    Jobs,
    Commands,
    Intents,
    Evaluations,
}
impl Namespace {
    fn name(self) -> &'static str {
        match self {
            Self::Orders => "orders",
            Self::Positions => "positions",
            Self::Operations => "plugin_operation_receipts",
            Self::Jobs => "session_dispatch_jobs_v1",
            Self::Commands => "control_plane_commands",
            Self::Intents => "session_dispatch_worker_intents_v1",
            Self::Evaluations => "data_evaluation_receipts_v1",
        }
    }
}
fn rows(db: &Path, ns: Namespace) -> Result<Vec<(String, Value)>> {
    let connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "fixture_database_read_failed")?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|_| "fixture_database_timeout_failed")?;
    let mut query = connection.prepare("SELECT item_key, value_json FROM tradeassembly_kv WHERE namespace = ?1 ORDER BY item_key LIMIT 4097")
        .map_err(|_| "fixture_database_query_failed")?;
    let mut cursor = query
        .query([ns.name()])
        .map_err(|_| "fixture_database_query_failed")?;
    let mut result = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = cursor.next().map_err(|_| "fixture_database_read_failed")? {
        require(result.len() < MAX_ROWS, "fixture_database_row_limit")?;
        let key_ref = row.get_ref(0).map_err(|_| "fixture_database_row_invalid")?;
        let raw_ref = row.get_ref(1).map_err(|_| "fixture_database_row_invalid")?;
        let key = key_ref
            .as_str()
            .map_err(|_| "fixture_database_row_invalid")?;
        let raw = raw_ref
            .as_str()
            .map_err(|_| "fixture_database_row_invalid")?;
        bytes = bytes
            .checked_add(key.len())
            .and_then(|n| n.checked_add(raw.len()))
            .ok_or("fixture_database_byte_limit")?;
        require(bytes <= MAX_DB_BYTES, "fixture_database_byte_limit")?;
        result.push((
            key.to_owned(),
            serde_json::from_str(raw).map_err(|_| "fixture_database_json_invalid")?,
        ));
    }
    Ok(result)
}

#[cfg(unix)]
fn nonblocking(fd: RawFd) -> Result<()> {
    // The descriptor is borrowed from a live owned child pipe for this call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    require(flags >= 0, "fixture_pipe_flags_failed")?;
    // Preserve every existing status flag and only add nonblocking I/O.
    require(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "fixture_pipe_flags_failed",
    )
}
#[cfg(unix)]
fn ready(fd: RawFd, events: i16, deadline: Instant) -> Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("fixture_pipe_timeout")?;
        let millis = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // One borrowed live descriptor; poll neither closes nor takes ownership.
        let result = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if result < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        require(result > 0, "fixture_pipe_timeout")?;
        require(
            descriptor.revents & libc::POLLNVAL == 0,
            "fixture_pipe_invalid",
        )?;
        return Ok(());
    }
}
#[cfg(unix)]
fn send<W: Write + AsRawFd>(writer: &mut W, value: &Value, timeout: Duration) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| "fixture_mcp_encoding_failed")?;
    require(bytes.len() < MAX_MESSAGE, "fixture_mcp_request_too_large")?;
    bytes.push(b'\n');
    let deadline = Instant::now() + remaining(timeout)?;
    let mut offset = 0;
    while offset < bytes.len() {
        ready(writer.as_raw_fd(), libc::POLLOUT, deadline)?;
        match writer.write(&bytes[offset..]) {
            Ok(0) => return Err("fixture_mcp_input_closed".into()),
            Ok(n) => offset += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("fixture_mcp_input_failed".into()),
        }
    }
    Ok(())
}
#[cfg(unix)]
fn receive<R: Read + AsRawFd>(
    reader: &mut R,
    buffered: &mut Vec<u8>,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + remaining(timeout)?;
    loop {
        if let Some(end) = buffered.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = buffered.drain(..=end).collect();
            return serde_json::from_slice(&line).map_err(|_| "fixture_mcp_json_invalid".into());
        }
        require(
            buffered.len() < MAX_MESSAGE,
            "fixture_mcp_response_too_large",
        )?;
        ready(reader.as_raw_fd(), libc::POLLIN, deadline)?;
        let mut chunk = [0; 4096];
        match reader.read(&mut chunk) {
            Ok(0) => return Err("fixture_mcp_output_closed".into()),
            Ok(n) => {
                require(
                    buffered.len() + n <= MAX_MESSAGE,
                    "fixture_mcp_response_too_large",
                )?;
                buffered.extend_from_slice(&chunk[..n]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("fixture_mcp_output_failed".into()),
        }
    }
}
struct CoreProcess {
    child: Child,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    buffered: Vec<u8>,
    stopped: bool,
}
impl CoreProcess {
    fn spawn(cfg: &Config, name: &str, mcp: bool) -> Result<Self> {
        let mut command = Command::new(&cfg.binary);
        command.arg("--db").arg(cfg.root.join("service.db"));
        if mcp {
            command.args(["mcp", "serve", "--transport", "stdio"]);
        } else {
            command.args(["api", "--host", "127.0.0.1", "--port", "0"]);
        }
        command.current_dir(&cfg.root).env_clear();
        for key in ["HOME", "PATH", "TMPDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("TRADEASSEMBLY_AUTH_PROFILE", "local_owner")
            .env(
                "TRADEASSEMBLY_ARTIFACT_ROOT",
                cfg.root.join("process-artifacts"),
            )
            .env("TRADEASSEMBLY_WARDEN_SIDECAR_URL", &cfg.warden_origin)
            .env("TRADEASSEMBLY_PLUGIN_SANDBOX_COMMAND", &cfg.srt)
            .env("TRADEASSEMBLY_PLUGIN_SANDBOX_ALLOW_LOCAL_EGRESS", "true")
            .env("TRADEASSEMBLY_TELEMETRY_ENABLED", "false")
            .stderr(Stdio::from(new_file(
                &cfg.evidence.join(format!("{name}.stderr.log")),
            )?));
        if mcp {
            command.stdin(Stdio::piped()).stdout(Stdio::piped());
        } else {
            command.stdin(Stdio::null()).stdout(Stdio::from(new_file(
                &cfg.evidence.join(format!("{name}.stdout.log")),
            )?));
        }
        let child = command.spawn().map_err(|_| "fixture_core_spawn_failed")?;
        let mut process = Self {
            child,
            input: None,
            output: None,
            buffered: Vec::new(),
            stopped: false,
        };
        process.input = process.child.stdin.take();
        process.output = process.child.stdout.take();
        #[cfg(unix)]
        {
            if let Some(pipe) = &process.input {
                nonblocking(pipe.as_raw_fd())?;
            }
            if let Some(pipe) = &process.output {
                nonblocking(pipe.as_raw_fd())?;
            }
        }
        Ok(process)
    }
    fn alive(&mut self) -> Result<()> {
        require(
            !self.stopped
                && self
                    .child
                    .try_wait()
                    .map_err(|_| "fixture_core_wait_failed")?
                    .is_none(),
            "fixture_core_exited_unexpectedly",
        )
    }
    fn stop(&mut self) -> Result<bool> {
        if self.stopped {
            return Ok(false);
        }
        let mut killed = false;
        if self
            .child
            .try_wait()
            .map_err(|_| "fixture_core_wait_failed")?
            .is_none()
        {
            match self.child.kill() {
                Ok(()) => killed = true,
                Err(_)
                    if self
                        .child
                        .try_wait()
                        .map_err(|_| "fixture_core_wait_failed")?
                        .is_none() =>
                {
                    return Err("fixture_core_kill_failed".into())
                }
                Err(_) => {}
            }
        }
        // Kill before closing stdin so normal shutdown cannot replace the
        // explicit child-kill boundary this fixture is intended to exercise.
        drop(self.input.take());
        drop(self.output.take());
        let deadline = Instant::now() + Duration::from_secs(20);
        while self
            .child
            .try_wait()
            .map_err(|_| "fixture_core_wait_failed")?
            .is_none()
        {
            require(Instant::now() < deadline, "fixture_core_join_timeout")?;
            std::thread::sleep(Duration::from_millis(20));
        }
        self.stopped = true;
        Ok(killed)
    }
    #[cfg(unix)]
    fn send(&mut self, value: &Value) -> Result<()> {
        send(
            self.input.as_mut().ok_or("fixture_mcp_input_missing")?,
            value,
            IO_TIMEOUT,
        )
    }
    #[cfg(unix)]
    fn receive(&mut self) -> Result<Value> {
        receive(
            self.output.as_mut().ok_or("fixture_mcp_output_missing")?,
            &mut self.buffered,
            IO_TIMEOUT,
        )
    }
    #[cfg(not(unix))]
    fn send(&mut self, _: &Value) -> Result<()> {
        Err("fixture_enforcing_srt_requires_unix".into())
    }
    #[cfg(not(unix))]
    fn receive(&mut self) -> Result<Value> {
        Err("fixture_enforcing_srt_requires_unix".into())
    }
    fn initialize(&mut self) -> Result<()> {
        self.send(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}))?;
        let reply = self.receive()?;
        require(
            reply["id"] == 0 && reply.get("error").is_none(),
            "fixture_mcp_initialize_failed",
        )?;
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
    }
    fn call(&mut self, id: u64, args: &Value) -> Result<Value> {
        self.send(&rpc(id, args))?;
        let reply = self.receive()?;
        require(
            reply["id"] == id
                && reply.get("error").is_none()
                && reply["result"]["isError"] == false,
            "fixture_mcp_tool_refused",
        )?;
        Ok(reply)
    }
}
impl Drop for CoreProcess {
    fn drop(&mut self) {
        if self.stop().is_err() {
            eprintln!("fixture_owned_core_cleanup_failed");
        }
    }
}
fn rpc(id: u64, args: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"tradeassembly.execution.scheduler.run","arguments":args}})
}
fn until<T>(
    process: &mut CoreProcess,
    timeout: Duration,
    mut check: impl FnMut() -> Result<Option<T>>,
) -> Result<T> {
    let deadline = Instant::now() + remaining(timeout)?;
    loop {
        if let Some(value) = check()? {
            return Ok(value);
        }
        if process.alive().is_err() {
            // In the deliberate broken-pipe case the server can exit between
            // our read and status check. Reopen/read once after observed exit;
            // only actual committed bytes can satisfy the condition.
            if let Some(value) = check()? {
                return Ok(value);
            }
            return Err("fixture_core_exited_before_commit".into());
        }
        require(Instant::now() < deadline, "fixture_domain_commit_timeout")?;
        std::thread::sleep(Duration::from_millis(200));
    }
}
fn check_receipt(receipt: &Value) -> Result<()> {
    require(
        receipt["status"] == "ready" && receipt["framesProcessed"] == 2,
        "fixture_evaluation_not_ready",
    )?;
    let signals = receipt["signals"]
        .as_array()
        .ok_or("fixture_signals_missing")?;
    require(
        signals.len() == 2
            && signals[0]["entry"]["status"] == "warming_up"
            && signals[1]["entry"] == json!({"status":"ready","value":true})
            && signals.iter().all(|s| {
                s["tradeInstruments"]
                    .as_array()
                    .is_some_and(|i| i.len() == 1 && i[0]["instrumentId"] == "A")
            }),
        "fixture_signal_evidence_invalid",
    )
}
fn run(cfg: Config) -> Result<Value> {
    DRIVER_DEADLINE
        .set(Instant::now() + Duration::from_secs(600))
        .map_err(|_| "fixture_driver_already_started")?;
    require(cfg!(unix), "fixture_enforcing_srt_requires_unix")?;
    require(
        hex(&cfg.revision, 40) && hex(&cfg.binary_sha256, 64) && hex(&cfg.srt_sha256, 64),
        "fixture_pin_invalid",
    )?;
    require(
        loopback_origin(&cfg.warden_origin),
        "fixture_warden_not_loopback",
    )?;
    for path in [&cfg.root, &cfg.binary, &cfg.srt, &cfg.evidence] {
        require(path.is_absolute(), "fixture_path_not_absolute")?;
    }
    require(
        cfg.root
            .canonicalize()
            .map_err(|_| "fixture_root_unavailable")?
            == cfg.root,
        "fixture_root_not_canonical",
    )?;
    private(&cfg.root, true)?;
    let marker = cfg.root.join("SYNTHETIC_SESSION_PROCESS_FIXTURE");
    private(&marker, false)?;
    require(
        fs::metadata(&marker)
            .map_err(|_| "fixture_marker_unreadable")?
            .len()
            == b"synthetic only\n".len() as u64,
        "fixture_synthetic_marker_missing",
    )?;
    require(
        fs::read(&marker).map_err(|_| "fixture_marker_unreadable")? == b"synthetic only\n",
        "fixture_synthetic_marker_missing",
    )?;
    private(&cfg.root.join(".tradeassembly"), true)?;
    private(&cfg.root.join(".tradeassembly/warden.token"), false)?;
    require(
        sha256(&cfg.binary)? == cfg.binary_sha256 && sha256(&cfg.srt)? == cfg.srt_sha256,
        "fixture_binary_pin_changed",
    )?;
    require(
        cfg.job.is_object()
            && cfg.job["mode"] == "paper"
            && cfg
                .job
                .get("executionActivationId")
                .is_none_or(Value::is_null),
        "fixture_computation_only_job_required",
    )?;
    fs::create_dir(&cfg.evidence).map_err(|_| "fixture_evidence_must_be_new")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cfg.evidence, fs::Permissions::from_mode(0o700))
            .map_err(|_| "fixture_evidence_permissions_failed")?;
    }
    let db = cfg.root.join("service.db");
    let original = json!({"sessionDispatch":{"kind":"enqueue","job":cfg.job},"idempotencyKey":"process-enqueue-original"});
    let orders = rows(&db, Namespace::Orders)?;
    let positions = rows(&db, Namespace::Positions)?;
    require(
        orders.is_empty() && positions.is_empty(),
        "fixture_financial_state_not_empty",
    )?;
    let initial_operations = rows(&db, Namespace::Operations)?;
    let mut lost = CoreProcess::spawn(&cfg, "lost-enqueue", true)?;
    lost.initialize()?;
    // No reader thread or duplicate descriptor exists. Drop the owned reply
    // pipe before enqueue: response delivery is impossible, not merely unread.
    drop(lost.output.take());
    lost.buffered.clear();
    lost.send(&rpc(1, &original))?;
    let stored = until(&mut lost, IO_TIMEOUT, || {
        Ok(rows(&db, Namespace::Jobs)?
            .into_iter()
            .find(|(_, v)| v["request"] == cfg.job && v["queueMessageId"].is_string())
            .map(|(_, v)| v))
    })?;
    require(
        stored["coreRevision"] == cfg.revision,
        "fixture_stored_core_revision_mismatch",
    )?;
    let job_id = stored["jobId"]
        .as_str()
        .ok_or("fixture_job_id_missing")?
        .to_string();
    until(&mut lost, IO_TIMEOUT, || {
        Ok(rows(&db, Namespace::Commands)?
            .iter()
            .any(|(_, v)| {
                v["envelope"]["idempotency_key"] == "process-enqueue-original"
                    && v["response_status"] == 200
            })
            .then_some(()))
    })?;
    lost.stop()?;
    save(&cfg.evidence, "lost-response-original-job", &stored)?;
    let mut mcp = CoreProcess::spawn(&cfg, "replay-and-enable", true)?;
    mcp.initialize()?;
    let replay = mcp.call(2, &original)?;
    let replayed = &replay["result"]["structuredContent"]["body"]["job"];
    require(
        replayed["jobId"] == job_id && replayed["createdAtMs"] == stored["createdAtMs"],
        "fixture_original_enqueue_not_recovered",
    )?;
    require(
        rows(&db, Namespace::Jobs)?.len() == 1
            && rows(&db, Namespace::Operations)? == initial_operations,
        "fixture_enqueue_replayed_effects",
    )?;
    let enabled = mcp.call(3, &json!({"sessionDispatch":{"kind":"start_worker","jobId":job_id},"idempotencyKey":"process-start-worker-original"}))?;
    save(&cfg.evidence, "mcp-replay", &replay)?;
    save(&cfg.evidence, "mcp-enable", &enabled)?;
    let intent = rows(&db, Namespace::Intents)?;
    require(
        intent.len() == 1 && intent[0].1["enabled"] == true,
        "fixture_enabled_intent_missing",
    )?;
    let before = rows(&db, Namespace::Jobs)?
        .into_iter()
        .next()
        .ok_or("fixture_job_missing")?
        .1;
    require(
        before["startedAtMs"].is_null() && before["request"] == cfg.job,
        "fixture_due_consumed_before_restart",
    )?;
    require(
        rows(&db, Namespace::Operations)? == initial_operations,
        "fixture_worker_started_early",
    )?;
    let stopped_pid = mcp.child.id();
    require(mcp.stop()?, "fixture_mcp_explicit_kill_missing")?;
    save(
        &cfg.evidence,
        "intent-before-core-restart",
        &json!({"intent":intent,"job":before,"stoppedPid":stopped_pid}),
    )?;
    let mut api = CoreProcess::spawn(&cfg, "normal-api-startup", false)?;
    let api_pid = api.child.id();
    let final_job = until(&mut api, Duration::from_secs(300), || {
        Ok(rows(&db, Namespace::Jobs)?
            .into_iter()
            .find(|(_, v)| {
                v["jobId"] == job_id
                    && matches!(
                        v["status"].as_str(),
                        Some("complete" | "failed" | "partial" | "uncertain" | "skipped_missed")
                    )
            })
            .map(|(_, v)| v))
    })?;
    require(
        final_job["status"] == "complete"
            && final_job["createdAtMs"] == stored["createdAtMs"]
            && final_job["coreRevision"] == cfg.revision
            && final_job["request"] == cfg.job,
        "fixture_original_job_not_completed",
    )?;
    require(
        rows(&db, Namespace::Intents)? == intent,
        "fixture_worker_intent_changed",
    )?;
    let evaluations = rows(&db, Namespace::Evaluations)?;
    require(evaluations.len() == 1, "fixture_evaluation_count_invalid")?;
    check_receipt(&evaluations[0].1)?;
    require(
        rows(&db, Namespace::Orders)? == orders && rows(&db, Namespace::Positions)? == positions,
        "fixture_financial_state_changed",
    )?;
    save(
        &cfg.evidence,
        "normal-api-startup-completed-job",
        &final_job,
    )?;
    save(
        &cfg.evidence,
        "normal-api-startup-evaluation",
        &evaluations[0].1,
    )?;
    require(api.stop()?, "fixture_api_explicit_kill_missing")?;
    let operations = rows(&db, Namespace::Operations)?;
    let finished_job = rows(&db, Namespace::Jobs)?;
    let mut api2 = CoreProcess::spawn(&cfg, "completed-job-restart", false)?;
    let completed_restart_pid = api2.child.id();
    let observed_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < observed_until {
        api2.alive()?;
        std::thread::sleep(Duration::from_millis(100));
    }
    require(
        rows(&db, Namespace::Operations)? == operations
            && rows(&db, Namespace::Jobs)? == finished_job
            && rows(&db, Namespace::Intents)? == intent
            && rows(&db, Namespace::Orders)? == orders
            && rows(&db, Namespace::Positions)? == positions,
        "fixture_completed_restart_replayed_effects",
    )?;
    require(api2.stop()?, "fixture_completed_api_explicit_kill_missing")?;
    save(
        &cfg.evidence,
        "result",
        &json!({"status":"PASS","coreSourceRevision":cfg.revision,"binarySha256":cfg.binary_sha256,"srtSha256":cfg.srt_sha256,
        "wholeResponseLoss":"owned MCP stdout pipe closed before enqueue after initialization; durable original request recovered in a fresh Core process",
        "actualCoreProcessRestart":"MCP process killed before due; normal CLI API startup resumed the unchanged enabled intent and evaluated plugins through configured SRT",
        "apiPid":api_pid,"completedRestartPid":completed_restart_pid,"explicitCoreChildKills":3,"completedRestartNoNewProviderInvocation":true,"originalJobAndIntentPreserved":true,"nativeFrames":2,"warmingThenReady":true,
        "provider":"authored loopback fixture","identity":"protected synthetic installation owner","warden":"real HTTP adapter to synthetic mock",
        "srtLocalEgress":"explicit loopback fixture opt-in","brokerOrders":false,"brokerOrderCount":0,"releaseQualified":false,"relayTransportResponseLoss":"NOT exercised by this Core MCP case"}),
    )?;
    Ok(json!({"status":"PASS","jobId":job_id,"evidence":cfg.evidence}))
}
#[cfg(unix)]
fn input() -> Result<Vec<u8>> {
    let descriptor = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| "fixture_input_unreadable")?;
    let mut source = File::from(descriptor);
    nonblocking(source.as_raw_fd())?;
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut bytes = Vec::new();
    loop {
        ready(source.as_raw_fd(), libc::POLLIN, deadline)?;
        let mut chunk = [0; 4096];
        match source.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(n) => {
                require(bytes.len() + n <= MAX_MESSAGE, "fixture_input_too_large")?;
                bytes.extend_from_slice(&chunk[..n]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("fixture_input_unreadable".into()),
        }
    }
}
#[cfg(not(unix))]
fn input() -> Result<Vec<u8>> {
    Err("fixture_enforcing_srt_requires_unix".into())
}
fn main() {
    let result = (|| {
        let bytes = input()?;
        let cfg: Config = serde_json::from_slice(&bytes).map_err(|_| "fixture_input_invalid")?;
        run(cfg)
    })();
    match result {
        Ok(value) => println!("{value}"),
        Err(code) => {
            eprintln!("session_process_driver_failed:{code}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_selected_record_refuses_before_owned_copy_or_json_parse() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("bounded.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tradeassembly_kv (namespace TEXT, item_key TEXT, value_json TEXT);",
            )
            .unwrap();
        // Invalid JSON makes the order of validation observable. Records in a
        // different namespace remain outside this closed read selection.
        connection
            .execute(
                "INSERT INTO tradeassembly_kv VALUES (?1, ?2, ?3)",
                ("positions", "large", "x".repeat(MAX_DB_BYTES + 1)),
            )
            .unwrap();
        assert!(rows(&db, Namespace::Orders).unwrap().is_empty());
        assert_eq!(
            rows(&db, Namespace::Positions).unwrap_err(),
            "fixture_database_byte_limit"
        );
        connection
            .execute("DELETE FROM tradeassembly_kv", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO tradeassembly_kv VALUES (?1, ?2, ?3)",
                ("positions", "small", "not-json"),
            )
            .unwrap();
        assert_eq!(
            rows(&db, Namespace::Positions).unwrap_err(),
            "fixture_database_json_invalid"
        );
    }
    #[test]
    fn explicit_pins_loopback_and_closed_input_are_required() {
        assert!(hex(&"a".repeat(40), 40));
        assert!(!hex(&"A".repeat(40), 40));
        assert!(loopback_origin("http://127.0.0.1:1234"));
        for value in [
            "https://127.0.0.1:1234",
            "http://localhost:1234",
            "http://127.0.0.1:1234/path",
            "http://user@127.0.0.1:1234",
        ] {
            assert!(!loopback_origin(value));
        }
        let mut value = json!({"root":"/synthetic","binary":"/core","binarySha256":"a".repeat(64),"revision":"b".repeat(40),"job":{},"evidence":"/evidence","wardenOrigin":"http://127.0.0.1:1234","srt":"/srt","srtSha256":"c".repeat(64)});
        assert!(serde_json::from_value::<Config>(value.clone()).is_ok());
        value["credentials"] = json!({});
        assert!(serde_json::from_value::<Config>(value).is_err());
    }
    #[test]
    fn warming_ready_subject_and_exact_frame_count_cannot_be_inferred() {
        let mut value = json!({"status":"ready","framesProcessed":2,"signals":[{"entry":{"status":"warming_up"},"tradeInstruments":[{"instrumentId":"A"}]},{"entry":{"status":"ready","value":true},"tradeInstruments":[{"instrumentId":"A"}]}]});
        check_receipt(&value).unwrap();
        value["signals"][1]["tradeInstruments"][0]["instrumentId"] = json!("B");
        assert!(check_receipt(&value).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn owned_nonblocking_io_is_bounded_and_preserves_following_response() {
        use std::os::unix::net::UnixStream;
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        nonblocking(writer.as_raw_fd()).unwrap();
        nonblocking(reader.as_raw_fd()).unwrap();
        let mut buffer = Vec::new();
        assert!(receive(&mut reader, &mut buffer, Duration::from_millis(5)).is_err());
        send(&mut writer, &json!({"id":1}), Duration::from_secs(1)).unwrap();
        send(&mut writer, &json!({"id":2}), Duration::from_secs(1)).unwrap();
        assert_eq!(
            receive(&mut reader, &mut buffer, Duration::from_secs(1)).unwrap()["id"],
            1
        );
        assert_eq!(
            receive(&mut reader, &mut buffer, Duration::from_secs(1)).unwrap()["id"],
            2
        );
        assert!(send(
            &mut writer,
            &json!("x".repeat(MAX_MESSAGE)),
            Duration::from_secs(1)
        )
        .is_err());
        let mut filled = 0usize;
        loop {
            match writer.write(&[0; 8192]) {
                Ok(n) => {
                    filled += n;
                    assert!(n > 0 && filled <= MAX_DB_BYTES);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("socket fill failed: {e}"),
            }
        }
        assert!(
            send(&mut writer, &json!({"id":3}), Duration::from_millis(5)).is_err(),
            "a nonreading child must not block stdin writes indefinitely"
        );
    }
}
