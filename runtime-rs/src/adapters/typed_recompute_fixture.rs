// Authored diagnostic native fault fixtures. The original local SQLite DB and
// all actual ports are retained; only one post-evaluator atomic seal fails.
struct TypedRecomputeSealFault {
    inner: std::sync::Arc<dyn crate::ports::StoragePort>,
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    start_response_lost: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl crate::ports::VersionedPort for TypedRecomputeSealFault {
    fn descriptors(&self) -> Vec<crate::ports::PortDescriptor> {
        self.inner.descriptors()
    }
}
impl crate::ports::StoragePort for TypedRecomputeSealFault {
    fn adapter_name(&self) -> &'static str {
        self.inner.adapter_name()
    }
    fn put_json(
        &self,
        n: &str,
        k: &str,
        v: Value,
        c: &crate::ports::SideEffectContext,
    ) -> Result<(), String> {
        self.inner.put_json(n, k, v, c)
    }
    fn put_json_if_absent(
        &self,
        n: &str,
        k: &str,
        v: Value,
        c: &crate::ports::SideEffectContext,
    ) -> Result<crate::ports::ImmutablePutOutcome, String> {
        self.inner.put_json_if_absent(n, k, v, c)
    }
    fn compare_and_put_json(
        &self,
        n: &str,
        k: &str,
        e: Value,
        v: Value,
        c: &crate::ports::SideEffectContext,
    ) -> Result<crate::ports::ComparePutOutcome, String> {
        self.inner.compare_and_put_json(n, k, e, v, c)
    }
    fn put_json_batch(
        &self,
        w: &[crate::ports::StorageWrite],
        e: &[crate::ports::StorageExpectation],
    ) -> Result<crate::ports::ComparePutOutcome, String> {
        let result = w.iter().find(|row| {
            row.namespace == "typed_backtest_recomputations_v1"
                && row.value["progress"]["receipts"]
                    .as_array()
                    .is_some_and(|receipts| receipts.len() == 1)
        });
        if let Some(result) = result.filter(|_| {
            w.iter()
                .any(|row| row.namespace == "typed_backtest_recompute_outcomes_v1")
                && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst)
        }) {
            let sha = result.value["progress"]["receipts"][0].as_str().unwrap();
            assert!(
                self.inner
                    .get_json("data_evaluation_receipt_hash_index_v1", sha)?
                    .is_some(),
                "fault must follow actual normal evaluator receipt/checkpoint commit"
            );
            return Err("fixture_typed_recompute_atomic_seal_once".into());
        }
        let result = self.inner.put_json_batch(w, e)?;
        if result == crate::ports::ComparePutOutcome::Updated
            && w.iter().any(|row| {
                row.namespace == "typed_backtest_recomputations_v1"
                    && row.value["phase"] == "queued"
            })
            && self
                .start_response_lost
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            // The actual request/state/outcome transaction committed. Report
            // an uncertain storage response so Start must recover those bytes.
            return Err("fixture_typed_recompute_start_response_lost_once".into());
        }
        Ok(result)
    }
    fn get_json(&self, n: &str, k: &str) -> Result<Option<Value>, String> {
        self.inner.get_json(n, k)
    }
    fn list_json(&self, n: &str) -> Result<Vec<(String, Value)>, String> {
        self.inner.list_json(n)
    }
    fn list_json_page(
        &self,
        n: &str,
        k: Option<&str>,
        l: usize,
    ) -> Result<Vec<(String, Value)>, String> {
        self.inner.list_json_page(n, k, l)
    }
    fn execution_checkpoint_snapshot(&self, p: &std::path::Path) -> Result<(), String> {
        self.inner.execution_checkpoint_snapshot(p)
    }
    fn public_workspace_snapshot(
        &self,
        n: &[&str],
        r: usize,
        b: usize,
    ) -> Result<Vec<(String, String, Value)>, String> {
        self.inner.public_workspace_snapshot(n, r, b)
    }
    fn import_public_workspace(
        &self,
        w: &[crate::ports::StorageWrite],
        d: &str,
        b: &[crate::ports::StorageExpectation],
    ) -> Result<crate::ports::ImmutablePutOutcome, String> {
        self.inner.import_public_workspace(w, d, b)
    }
    fn clear_namespace(&self, n: &str) -> Result<(), String> {
        self.inner.clear_namespace(n)
    }
}
// Pause one actual acquisition before delegating it. This controls only
// thread scheduling; lease storage, fences, trusted time and expiry are normal.
struct TypedRecomputeLeasePause {
    resource: String,
    entered: std::sync::mpsc::SyncSender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}
struct TypedRecomputeLeaseOverlap {
    inner: std::sync::Arc<dyn crate::ports::LeaseRepository>,
    pause: std::sync::Arc<std::sync::Mutex<Option<TypedRecomputeLeasePause>>>,
}
impl crate::ports::VersionedPort for TypedRecomputeLeaseOverlap {
    fn descriptors(&self) -> Vec<crate::ports::PortDescriptor> {
        self.inner.descriptors()
    }
}
impl crate::ports::LeaseRepository for TypedRecomputeLeaseOverlap {
    fn current(&self, resource: &str) -> Result<Option<crate::ports::LeaseClaim>, String> {
        self.inner.current(resource)
    }
    fn acquire(
        &self,
        resource: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<crate::ports::LeaseClaim>, String> {
        let paused = {
            let mut slot = self
                .pause
                .lock()
                .map_err(|_| "fixture_recompute_pause_poisoned")?;
            if slot
                .as_ref()
                .is_some_and(|pause| pause.resource == resource)
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some(paused) = paused {
            paused
                .entered
                .send(())
                .map_err(|_| "fixture_recompute_pause_disconnected")?;
            paused
                .resume
                .recv_timeout(std::time::Duration::from_secs(60))
                .map_err(|_| "fixture_recompute_pause_timeout")?;
        }
        self.inner.acquire(resource, owner, now_ms, lease_ms)
    }
    fn renew(
        &self,
        claim: &crate::ports::LeaseClaim,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<crate::ports::LeaseClaim, String> {
        self.inner.renew(claim, now_ms, lease_ms)
    }
    fn release(&self, claim: &crate::ports::LeaseClaim) -> Result<(), String> {
        self.inner.release(claim)
    }
}
struct TypedRecomputePermission {
    inner: std::sync::Arc<dyn crate::finance_authority::FinanceAuthorityPort>,
    revoked: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl crate::finance_authority::FinanceAuthorityPort for TypedRecomputePermission {
    fn command_policy_sha256(&self) -> Result<String, String> {
        self.inner.command_policy_sha256()
    }
    fn prepare_command(
        &self,
        s: &dyn crate::ports::StoragePort,
        e: &crate::control_plane::ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        if e.command_name == "backtest.run.execute"
            && e.payload_preview["request"]["kind"] == "advance"
            && self.revoked.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err("denied:fixture_research_permission_revoked".into());
        }
        self.inner.prepare_command(s, e)
    }
    fn complete_command(
        &self,
        s: &dyn crate::ports::StoragePort,
        e: &crate::control_plane::ControlPlaneCommandEnvelope,
        status: u16,
    ) -> Result<(), String> {
        self.inner.complete_command(s, e, status)
    }
    fn audit(&self, s: &dyn crate::ports::StoragePort) -> Value {
        self.inner.audit(s)
    }
}
fn typed_recompute_native(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    id: &str,
    original: &crate::backtest_contracts::BacktestResult,
) -> Value {
    let calls = || {
        s.runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap()
            .len()
    };
    let before = calls();
    let old = s.handle_http(
        "POST",
        &format!("/backtests/{id}/replay"),
        json!({"idempotencyKey":"old-typed-replay"}),
    );
    assert_eq!(old.status, 409, "{}", old.body);
    assert_eq!(
        old.body["error"]["code"],
        "typed_backtest_recompute_command_required"
    );
    let old_mcp = s.call_mcp_tool("tradeassembly.backtest.replay", json!({"run_id":id}));
    assert_eq!(old_mcp["isError"], true, "{old_mcp}");
    assert_eq!(
        old_mcp["structuredContent"]["error"]["code"],
        "typed_backtest_recompute_command_required"
    );
    assert_eq!(old_mcp["structuredContent"]["details"]["httpStatus"], 409);
    let graphql=s.handle_http("POST","/graphql",json!({"operationName":"ReplayBacktest","query":"mutation ReplayBacktest($runId: String!) { replayBacktest(runId: $runId) }","variables":{"runId":id}}));
    assert!(
        serde_json::to_string(&graphql.body)
            .unwrap()
            .contains("typed_backtest_recompute_command_required"),
        "{}",
        graphql.body
    );
    assert_eq!(
        calls(),
        before,
        "all old typed replay surfaces refuse before plugin work"
    );
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let start_response_lost = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let revoked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut runtime = (*s.runtime()).clone();
    let lease_pause = std::sync::Arc::new(std::sync::Mutex::new(None));
    runtime.leases = std::sync::Arc::new(TypedRecomputeLeaseOverlap {
        inner: runtime.leases.clone(),
        pause: lease_pause.clone(),
    });
    runtime.storage = std::sync::Arc::new(TypedRecomputeSealFault {
        inner: runtime.storage.clone(),
        armed: armed.clone(),
        start_response_lost: start_response_lost.clone(),
    });
    runtime.finance_authority = std::sync::Arc::new(TypedRecomputePermission {
        inner: runtime.finance_authority.clone(),
        revoked: revoked.clone(),
    });
    let native = crate::service::TradeAssemblyService::from_runtime(s.db(), runtime.clone())
        .for_authenticated_invocation("fixture", "typed-research-owner", None, None);
    let manifest = s.runtime().backtests.get_manifest(id).unwrap().unwrap();
    let start = json!({"schemaVersion":"tradeassembly.typed_backtest_recompute_command.v1","kind":"start","scope":scope,"runId":id,"expectedManifestHash":manifest.manifest_hash,"expectedResultHash":original.result_hash,"idempotencyKey":"native-recompute-original"});
    let created = native.handle_http(
        "POST",
        &format!("/backtests/{id}/recomputations"),
        json!({"request":start}),
    );
    assert_eq!(created.status, 202, "{}", created.body);
    assert!(!start_response_lost.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        calls(),
        before,
        "Start recovery must not invoke the publisher"
    );
    // Both actual HTTP calls use the same current owner and explicit command.
    // Their single durable generation/outcome is independent of the outer C3
    // response cache; the post-commit fault above deterministically covers the
    // immutable admission-outcome recovery branch as well.
    let mut concurrent_start = start.clone();
    concurrent_start["idempotencyKey"] = json!("native-recompute-concurrent-start");
    let states_before = native
        .runtime()
        .storage
        .list_json("typed_backtest_recomputations_v1")
        .unwrap()
        .len();
    let barrier = std::sync::Barrier::new(2);
    let (left, right) = std::thread::scope(|threads| {
        let left = threads.spawn(|| {
            barrier.wait();
            native.handle_http(
                "POST",
                &format!("/backtests/{id}/recomputations"),
                json!({"request":concurrent_start}),
            )
        });
        let right = threads.spawn(|| {
            barrier.wait();
            native.handle_http(
                "POST",
                &format!("/backtests/{id}/recomputations"),
                json!({"request":concurrent_start}),
            )
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    for reply in [&left, &right] {
        assert_eq!(reply.status, 202, "{}", reply.body);
        assert_eq!(reply.body["generation"], 0);
    }
    assert_eq!(left.body["recomputationId"], right.body["recomputationId"]);
    assert_eq!(
        native
            .runtime()
            .storage
            .list_json("typed_backtest_recomputations_v1")
            .unwrap()
            .len(),
        states_before + 1
    );
    assert_eq!(calls(), before);
    let recompute = created.body["recomputationId"]
        .as_str()
        .unwrap()
        .to_string();
    let inspect_path = format!("/backtests/{id}/recomputations/{recompute}");
    let advance_path = format!("{inspect_path}/advance");
    let command = |state: &Value, key: &str| json!({"schemaVersion":"tradeassembly.typed_backtest_recompute_command.v1","kind":"advance","scope":scope,"runId":id,"recomputationId":recompute,"expectedStateSha256":state["stateSha256"],"idempotencyKey":key});
    let held = native
        .runtime()
        .leases
        .acquire(
            &format!("typed-recompute:{recompute}"),
            "fixture-competing-recompute-turn",
            native.runtime().clock.trusted_now_ms().unwrap(),
            120000,
        )
        .unwrap()
        .unwrap();
    let busy_command = command(&created.body, "native-recompute-busy");
    let busy = native.handle_http("POST", &advance_path, json!({"request":busy_command}));
    assert_eq!(busy.status, 503, "{}", busy.body);
    assert_eq!(busy.body["error"]["code"], "typed_recompute_busy");
    assert_eq!(busy.body["error"]["retryable"], true);
    let after_busy = native.handle_http("GET", &inspect_path, json!({}));
    assert_eq!(after_busy.status, 200, "{}", after_busy.body);
    assert_eq!(after_busy.body, created.body);
    assert_eq!(calls(), before);
    native.runtime().leases.release(&held).unwrap();
    // A busy outer command may be sealed by C3. The distinct currently
    // authorized turn below uses the unchanged state, never an unsealed-key
    // assumption. The post-child continuation later retains its exact step.
    let denied = command(&created.body, "native-recompute-denied");
    revoked.store(true, std::sync::atomic::Ordering::SeqCst);
    let refusal = native.handle_http("POST", &advance_path, json!({"request":denied}));
    assert_eq!(refusal.status, 403, "{}", refusal.body);
    assert_eq!(
        calls(),
        before,
        "normal C3 denial must precede any provider/evaluator work"
    );
    revoked.store(false, std::sync::atomic::Ordering::SeqCst);
    let first = command(&created.body, "native-recompute-advance-original");
    let lost = native.handle_http("POST", &advance_path, json!({"request":first}));
    assert_eq!(lost.status, 503, "{}", lost.body);
    assert_eq!(
        lost.body["error"]["code"],
        "typed_recompute_state_seal_reconciliation_required"
    );
    assert!(!armed.load(std::sync::atomic::Ordering::SeqCst));
    let after_child = calls();
    assert!(
        after_child > before,
        "actual new generation must invoke the publisher"
    );
    let pending = native.handle_http("GET", &inspect_path, json!({}));
    assert_eq!(pending.status, 200, "{}", pending.body);
    assert_eq!(pending.body["generation"], 0);
    assert_eq!(pending.body["pendingCommand"], first);
    // The original retry can be denied after the actual inner effect. Never
    // bypass that sealed C3 denial; a fresh explicit continuation keeps the
    // exact original pending inner step/key instead of making another step.
    revoked.store(true, std::sync::atomic::Ordering::SeqCst);
    let denied_retry = native.handle_http("POST", &advance_path, json!({"request":first}));
    assert_eq!(denied_retry.status, 403, "{}", denied_retry.body);
    assert_eq!(calls(), after_child);
    revoked.store(false, std::sync::atomic::Ordering::SeqCst);
    let restarted = crate::service::TradeAssemblyService::from_runtime(s.db(), runtime)
        .for_authenticated_invocation("fixture", "typed-research-owner", None, None);
    let resume = command(&pending.body, "native-recompute-authorized-continuation");
    let resumed = restarted.handle_http("POST", &advance_path, json!({"request":resume}));
    assert_eq!(resumed.status, 202, "{}", resumed.body);
    assert_eq!(resumed.body["generation"], 1);
    assert_eq!(resumed.body["framesProcessed"], 2);
    assert_eq!(
        calls(),
        after_child,
        "lost-seal recovery must read the original receipt, not invoke plugins"
    );
    let still_denied = restarted.handle_http("POST", &advance_path, json!({"request":first}));
    assert_eq!(still_denied.status, 403, "old outer denial remains denied");
    assert_eq!(calls(), after_child);
    // Delete only the outer generic cache, preserving protected workflow and
    // real evaluator storage. Both admission and original advance recover.
    restarted
        .runtime()
        .storage
        .clear_namespace(crate::control_plane::COMMAND_NS)
        .unwrap();
    let same = restarted.handle_http(
        "POST",
        &format!("/backtests/{id}/recomputations"),
        json!({"request":start}),
    );
    assert_eq!(same.status, 202, "{}", same.body);
    assert_eq!(same.body["recomputationId"], recompute);
    let recovered_original = restarted.handle_http("POST", &advance_path, json!({"request":first}));
    assert_eq!(
        recovered_original.status, 202,
        "{}",
        recovered_original.body
    );
    assert_eq!(recovered_original.body["generation"], 1);
    assert_eq!(calls(), after_child);
    let again_mcp = restarted.call_mcp_tool(
        "tradeassembly.backtest.recompute.start",
        json!({"request":start}),
    );
    assert_eq!(again_mcp["isError"], false, "{again_mcp}");
    assert_eq!(again_mcp["structuredContent"]["recomputationId"], recompute);
    assert_eq!(calls(), after_child);
    let foreign =
        restarted.for_authenticated_invocation("fixture", "foreign-research-owner", None, None);
    assert_eq!(
        foreign.handle_http("GET", &inspect_path, json!({})).status,
        404
    );
    assert_eq!(
        foreign
            .handle_http(
                "POST",
                &format!("/backtests/{id}/recomputations"),
                json!({"request":start})
            )
            .status,
        404
    );
    for name in [
        "tradeassembly.backtest.recompute.start",
        "tradeassembly.backtest.recompute.inspect",
    ] {
        let request = if name.ends_with("inspect") {
            json!({"runId":id,"recomputationId":recompute})
        } else {
            start.clone()
        };
        let refusal = foreign.call_mcp_tool(name, json!({"request":request}));
        assert_eq!(refusal["isError"], true, "{refusal}");
        assert_eq!(refusal["structuredContent"]["details"]["httpStatus"], 404);
    }
    assert_eq!(calls(), after_child);
    let second = command(&resumed.body, "native-recompute-second");
    let second_reply = restarted.call_mcp_tool(
        "tradeassembly.backtest.recompute.advance",
        json!({"request":second}),
    );
    assert_eq!(second_reply["isError"], false, "{second_reply}");
    let second_state = &second_reply["structuredContent"];
    assert_eq!(second_state["generation"], 2);
    let final_request = command(second_state, "native-recompute-final");
    let before_overlap = calls();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
    *lease_pause.lock().unwrap() = Some(TypedRecomputeLeasePause {
        resource: format!("typed-recompute:{recompute}"),
        entered: entered_tx,
        resume: resume_rx,
    });
    let (completed, overlapped, after_winner) = std::thread::scope(|threads| {
        let loser = threads.spawn(|| {
            restarted.handle_http("POST", &advance_path, json!({"request":final_request}))
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        // The loser has already observed no exact outcome, but has not
        // acquired its lease. The winner now commits the final generation,
        // seals its normal outer response and releases the actual lease.
        let winner = restarted.handle_http("POST", &advance_path, json!({"request":final_request}));
        let after_winner = calls();
        resume_tx.send(()).unwrap();
        (winner, loser.join().unwrap(), after_winner)
    });
    assert_eq!(completed.status, 200, "{}", completed.body);
    assert_eq!(overlapped.status, 200, "{}", overlapped.body);
    assert_eq!(overlapped.body, completed.body);
    assert_eq!(completed.body["generation"], 3);
    assert!(
        after_winner > before_overlap,
        "winner must perform the actual remaining plugin step"
    );
    assert_eq!(
        calls(),
        after_winner,
        "loser must recover the exact sealed outcome without another plugin call"
    );
    let cached_overlap =
        restarted.handle_http("POST", &advance_path, json!({"request":final_request}));
    assert_eq!(cached_overlap.status, 200, "{}", cached_overlap.body);
    assert_eq!(cached_overlap.body["proof"], completed.body["proof"]);
    assert_eq!(cached_overlap.body["generation"], 3);
    assert_eq!(
        calls(),
        after_winner,
        "same-key overlap must preserve cached success"
    );
    assert_eq!(completed.body["phase"], "completed");
    assert_eq!(
        completed.body["proof"]["originalResultHash"],
        original.result_hash
    );
    assert_eq!(
        completed.body["proof"]["evaluationReceiptSha256"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        completed.body["proof"]["semanticSha256"],
        original
            .content
            .typed_evaluation
            .as_ref()
            .unwrap()
            .semantic_sha256
    );
    assert_ne!(
        completed.body["proof"]["finalCheckpointSha256"],
        original
            .content
            .typed_evaluation
            .as_ref()
            .unwrap()
            .final_checkpoint_sha256
    );
    assert_eq!(
        s.runtime().backtests.get_result(id).unwrap().unwrap(),
        *original
    );
    let after_completed = calls();
    restarted
        .runtime()
        .storage
        .clear_namespace(crate::control_plane::COMMAND_NS)
        .unwrap();
    let recovered = restarted.handle_http("POST", &advance_path, json!({"request":final_request}));
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(recovered.body["proof"], completed.body["proof"]);
    assert_eq!(calls(), after_completed);
    revoked.store(true, std::sync::atomic::Ordering::SeqCst);
    let inspected = restarted.call_mcp_tool(
        "tradeassembly.backtest.recompute.inspect",
        json!({"request":{"runId":id,"recomputationId":recompute}}),
    );
    assert_eq!(inspected["isError"], false, "{inspected}");
    assert_eq!(inspected["structuredContent"]["phase"], "completed");
    assert_eq!(calls(), after_completed);
    json!({"originalReadReplayHttp":old.body,"originalReadReplayMcp":old_mcp,"originalReadReplayGraphql":graphql.body,"admission":created.body,"concurrentStart":[left.body,right.body],"busy":busy.body,"postChildSealFailure":lost.body,"pending":pending.body,"afterRestartContinuation":resumed.body,"sameKeyOverlap":overlapped.body,"cachedOverlap":cached_overlap.body,"completed":completed.body,"inspection":inspected,"scope":"diagnostic native publisher/C3 TestFinanceAuthority fixture, same durable SQLite data retained; no SRT, agent delegation or actual provider claim"})
}
