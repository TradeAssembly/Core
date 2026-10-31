// Synthetic installed-node channel fixture; no external Relay or real broker.
thread_local! { static MANAGEMENT_V6_EVIDENCE_INDEX: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
thread_local! { static MANAGEMENT_V6_NATIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
fn management_v6_enabled() -> bool {
    MANAGEMENT_V6_NATIVE.with(|v| v.get())
}
thread_local! { static MANAGEMENT_V6_SEAL_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
fn management_v6_seal_failure_enabled() -> bool {
    MANAGEMENT_V6_SEAL_FAILURE.with(|v| v.get())
}
/// Delegates the original storage port, including its atomic/durability methods.
/// Only the chosen V6 close result's first immutable snapshot write fails.
struct ManagementV6SealFailureStorage {
    inner: std::sync::Arc<dyn crate::ports::StoragePort>,
    command_sha256: String,
    close_plan_id: String,
    failures: std::sync::atomic::AtomicUsize,
}
impl crate::ports::VersionedPort for ManagementV6SealFailureStorage {
    fn descriptors(&self) -> Vec<crate::ports::PortDescriptor> {
        self.inner.descriptors()
    }
}
impl crate::ports::StoragePort for ManagementV6SealFailureStorage {
    fn adapter_name(&self) -> &'static str {
        self.inner.adapter_name()
    }
    fn put_json(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &crate::ports::SideEffectContext,
    ) -> Result<(), String> {
        self.inner.put_json(namespace, key, value, context)
    }
    fn put_json_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &crate::ports::SideEffectContext,
    ) -> Result<crate::ports::ImmutablePutOutcome, String> {
        if namespace == "management_runner_snapshots_v1"
            && key == self.command_sha256
            && self
                .failures
                .compare_exchange(
                    0,
                    1,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
        {
            assert_eq!(value["snapshot"]["closePlanId"], self.close_plan_id);
            assert_eq!(value["snapshot"]["operation"], "strategy.order_plan.submit");
            // Prove injection happens AFTER the normal C5 child's durable result,
            // never in admission or in the broker/provider call itself.
            let state = self
                .inner
                .get_json("execution_order_plan_submissions_v2", &self.close_plan_id)?
                .expect("normal close state committed before outcome sealing");
            assert_eq!(state["phase"], "open");
            let original = state["originalKey"].as_str().expect("original C5 key");
            assert!(
                self.inner
                    .get_json("plugin_operation_receipts", original)?
                    .is_some(),
                "normal native C5 receipt must precede injected snapshot failure"
            );
            assert!(self.inner.get_json(namespace, key)?.is_none());
            return Err("fixture_management_v6_snapshot_seal_once".into());
        }
        self.inner
            .put_json_if_absent(namespace, key, value, context)
    }
    fn compare_and_put_json(
        &self,
        namespace: &str,
        key: &str,
        expected: Value,
        replacement: Value,
        context: &crate::ports::SideEffectContext,
    ) -> Result<crate::ports::ComparePutOutcome, String> {
        self.inner
            .compare_and_put_json(namespace, key, expected, replacement, context)
    }
    fn put_json_batch(
        &self,
        writes: &[crate::ports::StorageWrite],
        expectations: &[crate::ports::StorageExpectation],
    ) -> Result<crate::ports::ComparePutOutcome, String> {
        self.inner.put_json_batch(writes, expectations)
    }
    fn get_json(&self, namespace: &str, key: &str) -> Result<Option<Value>, String> {
        self.inner.get_json(namespace, key)
    }
    fn list_json(&self, namespace: &str) -> Result<Vec<(String, Value)>, String> {
        self.inner.list_json(namespace)
    }
    fn list_json_page(
        &self,
        namespace: &str,
        after_key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Value)>, String> {
        self.inner.list_json_page(namespace, after_key, limit)
    }
    fn execution_checkpoint_snapshot(&self, destination: &std::path::Path) -> Result<(), String> {
        self.inner.execution_checkpoint_snapshot(destination)
    }
    fn public_workspace_snapshot(
        &self,
        namespaces: &[&str],
        max_rows: usize,
        max_bytes: usize,
    ) -> Result<Vec<(String, String, Value)>, String> {
        self.inner
            .public_workspace_snapshot(namespaces, max_rows, max_bytes)
    }
    fn import_public_workspace(
        &self,
        writes: &[crate::ports::StorageWrite],
        digest: &str,
        bootstrap: &[crate::ports::StorageExpectation],
    ) -> Result<crate::ports::ImmutablePutOutcome, String> {
        self.inner
            .import_public_workspace(writes, digest, bootstrap)
    }
    fn clear_namespace(&self, namespace: &str) -> Result<(), String> {
        self.inner.clear_namespace(namespace)
    }
}

struct ManagementV6Transport {
    delivery: std::sync::Mutex<crate::service::reach_data_consumer::DataDelivery>,
    nonces: std::sync::Mutex<std::collections::BTreeSet<String>>,
    lose_record_once: std::sync::atomic::AtomicBool,
    recorded_calls: std::sync::Mutex<Vec<crate::service::reach_data_consumer::DataNodeRecord>>,
}
impl ManagementV6Transport {
    fn verify(&self, request: &crate::service::reach_data_consumer::DataNodeCall) {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        use ed25519_dalek::{Signature, Verifier as _};
        let delivery = self.delivery.lock().unwrap();
        assert_eq!(request.node, delivery.command.node);
        assert_eq!(
            request.source_binding_sha256,
            delivery.command.source_binding_sha256
        );
        assert!(self.nonces.lock().unwrap().insert(request.nonce.clone()));
        let signature =
            Signature::from_slice(&URL_SAFE_NO_PAD.decode(&request.signature).unwrap()).unwrap();
        let payload = request.signing_payload().unwrap();
        let message = [
            crate::ports::INSTALLATION_AUTHENTICATION_DOMAIN.as_bytes(),
            payload.as_bytes(),
        ]
        .concat();
        ed25519_dalek::SigningKey::from_bytes(&[7; 32])
            .verifying_key()
            .verify(&message, &signature)
            .unwrap();
    }
}
impl crate::service::reach_data_consumer::ReachDataTransport for ManagementV6Transport {
    fn claim(
        &self,
        request: &crate::service::reach_data_consumer::DataNodeCall,
    ) -> Result<Option<crate::service::reach_data_consumer::DataDelivery>, String> {
        self.verify(request);
        let d = self.delivery.lock().unwrap();
        Ok(
            if matches!(
                d.state,
                crate::service::reach_data_consumer::DataDeliveryState::Completed
                    | crate::service::reach_data_consumer::DataDeliveryState::Failed
            ) {
                None
            } else {
                Some(d.clone())
            },
        )
    }
    fn accept(
        &self,
        request: &crate::service::reach_data_consumer::DataNodeCall,
    ) -> Result<crate::service::reach_data_consumer::DataDelivery, String> {
        self.verify(request);
        let mut d = self.delivery.lock().unwrap();
        assert_eq!(
            request.command_sha256.as_deref(),
            Some(d.command_sha256.as_str())
        );
        assert_eq!(request.expected_revision, Some(d.revision));
        if let Some(original) = &d.receipt {
            assert_eq!(
                original.state,
                crate::service::data_runner::DataRunnerState::Uncertain
            );
            d.state = crate::service::reach_data_consumer::DataDeliveryState::Uncertain;
        } else {
            d.state = crate::service::reach_data_consumer::DataDeliveryState::Accepted;
        }
        d.revision += 1;
        Ok(d.clone())
    }
    fn record(
        &self,
        request: &crate::service::reach_data_consumer::DataNodeRecord,
    ) -> Result<crate::service::reach_data_consumer::DataDelivery, String> {
        self.verify(&request.request);
        self.recorded_calls.lock().unwrap().push(request.clone());
        let mut d = self.delivery.lock().unwrap();
        assert_eq!(
            request.request.command_sha256.as_deref(),
            Some(d.command_sha256.as_str())
        );
        assert_eq!(
            request.request.receipt_sha256.as_deref(),
            Some(request.receipt.receipt_sha256.as_str())
        );
        if let Some(original) = &d.receipt {
            assert_eq!(original, &request.receipt);
            return Ok(d.clone());
        }
        assert_eq!(request.request.expected_revision, Some(d.revision));
        d.revision += 1;
        d.state = match request.receipt.state {
            crate::service::data_runner::DataRunnerState::Completed => {
                crate::service::reach_data_consumer::DataDeliveryState::Completed
            }
            crate::service::data_runner::DataRunnerState::Failed => {
                crate::service::reach_data_consumer::DataDeliveryState::Failed
            }
            _ => crate::service::reach_data_consumer::DataDeliveryState::Uncertain,
        };
        d.receipt = Some(request.receipt.clone());
        if self
            .lose_record_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            Err("fixture_committed_record_response_lost".into())
        } else {
            Ok(d.clone())
        }
    }
}
fn management_v6_http(
    s: &crate::service::TradeAssemblyService,
    method: &str,
    path: &str,
    body: Value,
) -> crate::service::ServiceResponse {
    use crate::service::{
        data_runner::*, order_plans::management::remote::ManagementOperation as M,
    };
    if !management_v6_enabled() {
        return s.handle_http(method, path, body);
    }
    let scope = body
        .get("scope")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .unwrap();
    let operation = match path {
        "/product/strategies/positions/register" => M::Register {
            scope: scope.unwrap(),
            entry_plan_id: body["register"]["entryPlanId"].as_str().unwrap().into(),
            activation_id: body["register"]["activationId"].as_str().unwrap().into(),
        },
        "/product/strategies/positions/advance" => M::Advance {
            scope: scope.unwrap(),
            advance: serde_json::from_value(body["advance"].clone()).unwrap(),
        },
        "/product/strategies/order-plans/submit" => M::SubmitClose {
            scope: scope.unwrap(),
            submission: serde_json::from_value(body["submission"].clone()).unwrap(),
        },
        "/product/strategies/order-plans/reconcile" | "/product/strategies/order-plans/settle" => {
            let plan = crate::service::order_plans::submission::source(
                s,
                body["planId"].as_str().unwrap(),
            )
            .unwrap()
            .historical()
            .unwrap();
            if plan.management.is_none() {
                return s.handle_http(method, path, body);
            }
            if path.ends_with("reconcile") {
                M::ReconcileClose {
                    scope: scope.unwrap(),
                    plan_id: plan.plan_id,
                }
            } else {
                M::SettleClose {
                    scope: scope.unwrap(),
                    plan_id: plan.plan_id,
                }
            }
        }
        path if method == "GET" && path.starts_with("/product/strategies/positions/") => {
            let position_id = path
                .trim_start_matches("/product/strategies/positions/")
                .to_owned();
            let raw = s
                .runtime()
                .storage
                .get_json("execution_managed_positions_v1", &position_id)
                .unwrap()
                .unwrap();
            let entry = crate::service::order_plans::submission::source(
                s,
                raw["entryPlanId"].as_str().unwrap(),
            )
            .unwrap()
            .historical()
            .unwrap();
            M::Inspect {
                scope: entry.command.scope,
                position_id,
            }
        }
        _ => return s.handle_http(method, path, body),
    };
    let who = s.invocation_owner().unwrap();
    let mut scoped = s.clone().with_hosted_connection(
        crate::runtime_config::RuntimeConfig::local(s.db()),
        who.subject.clone(),
        None,
    );
    let key = body["idempotencyKey"]
        .as_str()
        .unwrap_or("management-v6-inspect");
    let command_key =
        crate::data_pipeline::canonical_sha256(&("fixture.management.v6", key)).unwrap();
    let command = if let Some(old) = s
        .runtime()
        .storage
        .get_json("data_runner_commands_v1", &command_key)
        .unwrap()
    {
        let old: DataRunnerCommand = serde_json::from_value(old).unwrap();
        assert_eq!(
            serde_json::to_value(&old.operation).unwrap(),
            json!({"kind":"manage_position","management":operation})
        );
        old
    } else {
        DataRunnerCommand {
            schema_version: DATA_RUNNER_SCHEMA_V6.into(),
            core_release: "fixture.release".into(),
            workspace_id: "fixture.management.v6".into(),
            command_id: key.into(),
            authority_ref: "provenance.only".into(),
            idempotency_key: IdempotencyKey::new(key).unwrap(),
            mode: DataRunnerMode::Paper,
            submitted_at_ms: s.runtime().clock.now_ms(),
            operation: DataRunnerOperation::ManagePosition {
                management: operation,
            },
        }
    };
    let fault_storage = if management_v6_seal_failure_enabled() && key == "managed-close-submit" {
        let DataRunnerOperation::ManagePosition {
            management: M::SubmitClose { submission, .. },
        } = &command.operation
        else {
            panic!("seal fault is scoped to the chosen SubmitClose")
        };
        let storage = std::sync::Arc::new(ManagementV6SealFailureStorage {
            inner: s.runtime().storage.clone(),
            command_sha256: command.canonical_hash().unwrap(),
            close_plan_id: submission.plan_id.clone(),
            failures: std::sync::atomic::AtomicUsize::new(0),
        });
        let mut runtime = (*scoped.runtime()).clone();
        runtime.storage = storage.clone();
        scoped = crate::service::TradeAssemblyService::from_runtime(s.db(), runtime)
            .with_hosted_connection(
                crate::runtime_config::RuntimeConfig::local(s.db()),
                who.subject.clone(),
                None,
            )
            .for_authenticated_invocation(&who.issuer, &who.subject, None, None);
        Some(storage)
    } else {
        None
    };
    let runner = DurableCoreDataRunner::new(scoped.clone(), "fixture.release".into()).unwrap();
    let before = s
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap();
    let foreign = DurableCoreDataRunner::new(
        scoped.for_authenticated_invocation("fixture", "foreign", None, None),
        "fixture.release".into(),
    )
    .unwrap();
    assert!(foreign.mutation_binding(&command).is_err());
    assert_eq!(
        s.runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap(),
        before
    );
    let binding = match runner.mutation_binding(&command) {
        Ok(v) => v,
        Err(e) => return crate::service::ServiceResponse::conflict(&e),
    };
    assert_eq!(
        binding.resource_kind.as_deref(),
        Some("strategy_managed_position")
    );
    let node = crate::ports::ReachNodeScope {
        tenant_id: "fixture.tenant".into(),
        workspace_id: command.workspace_id.clone(),
        node_id: "fixture.management.node".into(),
        actor_ref: "fixture.actor".into(),
    };
    let signer = std::sync::Arc::new(
        crate::adapters::connected_runner::Ed25519InstallationSigner::from_seed(
            node.node_id.clone(),
            [7; 32],
        )
        .unwrap(),
    );
    let entropy = std::sync::Arc::new(crate::adapters::connected_runner::OsRunnerEntropy);
    let producer = crate::service::reach_data_proofs::DataStatementProducer::new(
        &scoped,
        &node,
        signer.as_ref(),
        entropy.as_ref(),
        "fixture.release",
    )
    .unwrap();
    let preparation = producer
        .prepare_command(command.clone(), 1, 300000)
        .unwrap();
    assert_eq!(
        preparation.schema,
        "tradeassembly.relay.reach-data-preparation/v6"
    );
    let delivery = crate::service::reach_data_consumer::DataDelivery {
        command_sha256: preparation.command.command_sha256().unwrap(),
        command: preparation.command.clone(),
        state: crate::service::reach_data_consumer::DataDeliveryState::Delivered,
        revision: 1,
        receipt: None,
    };
    let transport = std::sync::Arc::new(ManagementV6Transport {
        delivery: std::sync::Mutex::new(delivery),
        nonces: Default::default(),
        lose_record_once: std::sync::atomic::AtomicBool::new(true),
        recorded_calls: Default::default(),
    });
    let consumer = crate::service::reach_data_consumer::ReachDataConsumer::new(
        scoped.clone(),
        "fixture.release".into(),
        node.clone(),
        signer.clone(),
        transport.clone(),
        entropy.clone(),
    )
    .unwrap();
    assert_eq!(
        consumer.poll_once().unwrap_err(),
        "fixture_committed_record_response_lost"
    );
    let restored =
        crate::service::TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone())
            .with_hosted_connection(
                crate::runtime_config::RuntimeConfig::local(s.db()),
                who.subject.clone(),
                None,
            )
            .for_authenticated_invocation(&who.issuer, &who.subject, None, None);
    let recovered = crate::service::reach_data_consumer::ReachDataConsumer::new(
        restored.clone(),
        "fixture.release".into(),
        node.clone(),
        signer.clone(),
        transport.clone(),
        entropy.clone(),
    )
    .unwrap();
    assert!(matches!(
        recovered.poll_once().unwrap(),
        crate::service::reach_data_consumer::ReachDataPoll::Recorded { .. }
    ));
    let receipt = runner
        .receipt(&command.workspace_id, &command.command_id)
        .unwrap()
        .unwrap();
    if let Some(storage) = &fault_storage {
        assert_eq!(
            storage.failures.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(receipt.state, DataRunnerState::Uncertain, "{receipt:?}");
        assert!(receipt.result.is_none());
        assert_eq!(receipt.error.as_deref(), Some("management_runner_outcome_seal_failed_reconciliation_required:fixture_management_v6_snapshot_seal_once"));
        assert_eq!(
            transport.delivery.lock().unwrap().receipt.as_ref(),
            Some(&receipt)
        );
        assert!(s
            .runtime()
            .storage
            .get_json(
                "management_runner_snapshots_v1",
                &command.canonical_hash().unwrap()
            )
            .unwrap()
            .is_none());
        let quiet = s
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap();
        // A fresh consumer claims the original uncertain delivery and signs a
        // fresh node call for the SAME immutable receipt, without another child.
        let retry_consumer = crate::service::reach_data_consumer::ReachDataConsumer::new(
            restored.clone(),
            "fixture.release".into(),
            node,
            signer,
            transport.clone(),
            entropy,
        )
        .unwrap();
        assert!(matches!(
            retry_consumer.poll_once().unwrap(),
            crate::service::reach_data_consumer::ReachDataPoll::Recorded {
                state: crate::service::reach_data_consumer::DataDeliveryState::Uncertain,
                ..
            }
        ));
        let retry_runner = DurableCoreDataRunner::new(restored, "fixture.release".into()).unwrap();
        assert_eq!(retry_runner.dispatch(&command).unwrap(), receipt);
        assert_eq!(
            runner
                .receipt(&command.workspace_id, &command.command_id)
                .unwrap()
                .as_ref(),
            Some(&receipt)
        );
        assert_eq!(
            s.runtime()
                .storage
                .list_json("plugin_operation_receipts")
                .unwrap(),
            quiet
        );
        assert!(s
            .runtime()
            .storage
            .get_json(
                "management_runner_snapshots_v1",
                &command.canonical_hash().unwrap()
            )
            .unwrap()
            .is_none());
        assert_eq!(
            storage.failures.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let signed_records = transport.recorded_calls.lock().unwrap().clone();
        assert_eq!(signed_records.len(), 3);
        assert!(signed_records
            .iter()
            .all(|record| record.receipt == receipt));
        write_screen_interface_evidence(
            "management_v6_seal_failure",
            "uncertain-signed-original-and-no-resubmit.json",
            &json!({
                "preparation":preparation,"receipt":receipt,"signedRecords":signed_records,"injectedAfterCommittedC5":true,"oneSnapshotWriteFailure":true,
                "recordResponseLost":true,"consumerRecreated":true,"freshConsumerOriginalRetry":true,"providerReceiptsUnchangedOnRetry":true,
                "actualProcessRestartQualified":false,"actualRelayQualified":false
            }),
        );
        let mut response =
            crate::service::ServiceResponse::conflict("management_v6_fixture_uncertain");
        response.body["runnerReceipt"] = serde_json::to_value(&receipt).unwrap();
        response.body["status"] = json!("uncertain");
        response.body["executionAuthorityGranted"] = json!(false);
        return response;
    }
    assert_eq!(receipt.state, DataRunnerState::Completed, "{receipt:?}");
    let quiet = s
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap();
    assert_eq!(runner.dispatch(&command).unwrap(), receipt);
    assert_eq!(
        s.runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap(),
        quiet
    );
    // Lose only the accepted receipt's final head after its immutable snapshot.
    let accepted = s
        .runtime()
        .storage
        .get_json(
            "data_runner_receipt_revisions_v1",
            receipt.previous_receipt_sha256.as_deref().unwrap(),
        )
        .unwrap()
        .unwrap();
    s.runtime()
        .storage
        .put_json(
            "data_runner_receipts_v1",
            &command_key,
            accepted,
            &ctx("management-v6-fixture-lost-final-head"),
        )
        .unwrap();
    let recovered_receipt = DurableCoreDataRunner::new(restored, "fixture.release".into())
        .unwrap()
        .dispatch(&command)
        .unwrap();
    assert_eq!(recovered_receipt, receipt);
    assert_eq!(
        s.runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap(),
        quiet
    );
    // A same-command retry is a distinct observation of the immutable result.
    // Preserve every observation instead of overwriting or colliding with it.
    let evidence_index = MANAGEMENT_V6_EVIDENCE_INDEX.with(|v| {
        let next = v.get() + 1;
        v.set(next);
        next
    });
    write_screen_interface_evidence(
        "management_v6",
        &format!("{key}-{evidence_index}.json"),
        &json!({"preparation":preparation,"receipt":receipt,"consumerRestarted":true,"recordResponseLost":true,"sealedSnapshotRecovery":true,"actualRelayQualified":false,"actualProcessRestartQualified":false}),
    );
    let Some(DataRunnerResult::Management { snapshot }) = receipt.result else {
        panic!("management snapshot required")
    };
    let decision = snapshot
        .submission
        .as_ref()
        .map(|submission| json!({"planId":snapshot.close_plan_id,"submission":submission}));
    let settlement = snapshot.settlement.as_ref();
    crate::service::ServiceResponse::ok(
        json!({"position":snapshot.position,"decision":decision,"submission":{"phase":snapshot.phase},"journal":snapshot.journal,"grossProceeds":settlement.map(|v|v.gross_proceeds),"feeCash":settlement.map(|v|v.fee_cash),"receiptSha256":settlement.map(|v|&v.receipt_sha256),"executionAuthorityGranted":false}),
    )
}
#[test]
#[ignore = "explicit synthetic native V6 producer/consumer, separate close C5, settlement, lost acknowledgment and sealed outcome recovery"]
fn native_management_v6_entry_close_settlement_and_owner_journal() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    MANAGEMENT_V6_NATIVE.with(|v| v.set(true));
    native_execution_plan_fixture(Some("financial_screen_auto_managed"));
    MANAGEMENT_V6_NATIVE.with(|v| v.set(false));
}
#[test]
#[ignore = "explicit finite clean-binary V6 process refusal fixture; requires DATA_RUNNER_PROCESS_BINARY"]
fn management_v6_actual_process_refuses_legacy_and_foreign_position_without_effects() {
    let binary =
        std::env::var_os("DATA_RUNNER_PROCESS_BINARY").expect("explicit selected clean binary");
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("service.db");
    let mut pids = std::collections::BTreeSet::new();
    for version in [
        "tradeassembly.core-data-runner.v5",
        "tradeassembly.core-data-runner.v6",
    ] {
        let c = json!({"schemaVersion":version,"coreRelease":"fixture.release","workspaceId":"fixture.workspace","commandId":"fixture.foreign","authorityRef":"provenance.only","idempotencyKey":"fixture.foreign","mode":"paper","submittedAtMs":chrono::Utc::now().timestamp_millis(),"operation":{"kind":"manage_position","management":{"kind":"inspect","scope":{"strategyId":"foreign","strategyVersionId":"version","strategySpecHash":"spec"},"positionId":"foreign"}}});
        let mut child = std::process::Command::new(&binary)
            .args([
                "--data-only",
                "--db",
                db.to_str().unwrap(),
                "--core-release",
                "fixture.release",
            ])
            .current_dir(dir.path())
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        pids.insert(child.id());
        writeln!(
            child.stdin.take().unwrap(),
            "{}",
            json!({"operation":"data_binding","command":c})
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("bounded V6 refusal child exceeded30s");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output["ok"], false, "{output}");
        let store = crate::adapters::local::test_runtime(db.to_str().unwrap()).storage;
        for ns in [
            "execution_managed_positions_v1",
            "execution_order_plan_submissions_v2",
            "data_runner_commands_v1",
            "plugin_operation_receipts",
        ] {
            assert!(store.list_json(ns).unwrap().is_empty());
        }
    }
    assert_eq!(pids.len(), 2);
}

/// Separate recovery path: the failed seal is NOT a completed close response.
fn exercise_management_v6_seal_failure_recovery(
    s: &crate::service::TradeAssemblyService,
    scope: &crate::service::data_plans::PlanScope,
    root: &std::path::Path,
    position_id: &str,
    decision: &Value,
    uncertain_response: &crate::service::ServiceResponse,
) {
    use crate::service::data_runner::{
        DataRunnerCommand, DataRunnerReceipt, DataRunnerState, DurableCoreDataRunner,
    };
    assert!(management_v6_seal_failure_enabled());
    assert_eq!(
        uncertain_response.status, 409,
        "{}",
        uncertain_response.body
    );
    assert_eq!(uncertain_response.body["status"], "uncertain");
    let uncertain: DataRunnerReceipt =
        serde_json::from_value(uncertain_response.body["runnerReceipt"].clone()).unwrap();
    assert_eq!(uncertain.state, DataRunnerState::Uncertain);
    assert!(uncertain.result.is_none());
    assert_eq!(uncertain.error.as_deref(), Some("management_runner_outcome_seal_failed_reconciliation_required:fixture_management_v6_snapshot_seal_once"));
    let command_key =
        crate::data_pipeline::canonical_sha256(&("fixture.management.v6", "managed-close-submit"))
            .unwrap();
    let command: DataRunnerCommand = serde_json::from_value(
        s.runtime()
            .storage
            .get_json("data_runner_commands_v1", &command_key)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(command.canonical_hash().unwrap(), uncertain.command_sha256);
    let close = decision["planId"].as_str().unwrap();
    let source = crate::service::order_plans::submission::source(s, close).unwrap();
    let state = source.state().unwrap().unwrap();
    assert_eq!(
        state.phase,
        crate::service::order_plans::submission::Phase::Open
    );
    assert!(state.settlement_receipt_sha256.is_none());
    assert!(state.retained_cash.positive());
    let position = s
        .runtime()
        .storage
        .get_json("execution_managed_positions_v1", position_id)
        .unwrap()
        .unwrap();
    assert_eq!(position["closedQuantity"], "0");
    assert!(position["reservations"].get(close).is_some());
    let count_calls = |name: &str| {
        fs::read_to_string(root.join("synthetic-broker/calls.log"))
            .unwrap()
            .lines()
            .filter(|line| *line == name)
            .count()
    };
    assert_eq!(
        count_calls("submit"),
        2,
        "one entry and one committed close"
    );
    assert_eq!(
        count_calls("lookup"),
        1,
        "only the prior entry lookup so far"
    );
    assert_eq!(
        fs::read_to_string(root.join("synthetic-broker/close-settlement.log"))
            .unwrap_or_default()
            .lines()
            .count(),
        0
    );
    let quiet = s
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap();
    let refused = management_v6_http(
        s,
        "POST",
        "/product/strategies/order-plans/submit",
        json!({
            "scope":scope,"submission":decision["submission"],"accountMode":"paper","idempotencyKey":"managed-close-fresh-after-seal-failure"
        }),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.body["error"]["code"],
        "management_runner_close_already_submitted_reconcile_required"
    );
    assert_eq!(
        s.runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap(),
        quiet
    );
    assert_eq!(count_calls("submit"), 2);
    let reconciled = management_v6_http(
        s,
        "POST",
        "/product/strategies/order-plans/reconcile",
        json!({
            "scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-close-seal-recovery-lookup"
        }),
    );
    assert_eq!(reconciled.status, 200, "{}", reconciled.body);
    let settled = management_v6_http(
        s,
        "POST",
        "/product/strategies/order-plans/settle",
        json!({
            "scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-close-seal-recovery-final-posting"
        }),
    );
    assert_eq!(settled.status, 200, "{}", settled.body);
    assert_eq!(settled.body["grossProceeds"], "36");
    let inspected = s.handle_http(
        "GET",
        &format!("/product/strategies/positions/{position_id}"),
        json!({}),
    );
    assert_eq!(inspected.status, 200, "{}", inspected.body);
    assert_eq!(inspected.body["position"]["closedQuantity"], "0.4");
    assert_eq!(inspected.body["position"]["reservations"], json!({}));
    assert_eq!(
        count_calls("submit"),
        2,
        "explicit recovery never submits a second close"
    );
    assert_eq!(
        count_calls("lookup"),
        2,
        "explicit close lookup recovered the original order"
    );
    assert_eq!(
        fs::read_to_string(root.join("synthetic-broker/close-settlement.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let runner = DurableCoreDataRunner::new(s.clone(), "fixture.release".into()).unwrap();
    let after = s
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap();
    assert_eq!(
        runner
            .receipt(&command.workspace_id, &command.command_id)
            .unwrap()
            .as_ref(),
        Some(&uncertain)
    );
    assert_eq!(runner.dispatch(&command).unwrap(), uncertain);
    assert!(s
        .runtime()
        .storage
        .get_json(
            "management_runner_snapshots_v1",
            &command.canonical_hash().unwrap()
        )
        .unwrap()
        .is_none());
    assert_eq!(
        s.runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap(),
        after
    );
    write_screen_interface_evidence(
        "management_v6_seal_failure",
        "explicit-original-close-recovery.json",
        &json!({
            "originalSubmitReceipt":uncertain,"freshKeySubmitRefusal":refused.body,"explicitReconcile":reconciled.body,
            "explicitSettlement":settled.body,"finalPosition":inspected.body,
            "originalReceiptRemainsUncertain":true,"originalSnapshotRemainsUnavailable":true,
            "syntheticSubmits":2,"syntheticLookups":2,"closeFinalPostings":1,
            "faultAfterCommittedC5":true,"consumerRecreated":true,"actualProcessRestartQualified":false,
            "actualRelayQualified":false,"realBrokerCalls":0,
            "qualification":"synthetic actual native Core C3/C5 with a single injected post-child snapshot storage failure; uncertain signed original outcome preserved, no repeat submit, separately authorized lookup/final settlement recovered the original close"
        }),
    );
}

#[test]
#[ignore = "explicit native V6 post-C5 snapshot seal failure, signed uncertain receipt and separate original-close recovery"]
fn native_management_v6_post_effect_seal_failure_is_uncertain_and_never_resubmits() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    MANAGEMENT_V6_NATIVE.with(|v| v.set(true));
    MANAGEMENT_V6_SEAL_FAILURE.with(|v| v.set(true));
    native_execution_plan_fixture(Some("financial_screen_auto_managed"));
    MANAGEMENT_V6_SEAL_FAILURE.with(|v| v.set(false));
    MANAGEMENT_V6_NATIVE.with(|v| v.set(false));
}
