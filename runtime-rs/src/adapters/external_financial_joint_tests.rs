// Opt-in bootstrap only. Financial effects must run through frozen Core/SRT.
fn prepare_financial_joint_driver(
    s: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
    scope: &data_plans::PlanScope,
    graph: &str,
    prepared: &Value,
    signal: &Value,
    warden_origin: &str,
) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    fn copy(source: &std::path::Path, target: &std::path::Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            assert!(!entry.file_type().unwrap().is_symlink());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    fs::write(root.join("SYNTHETIC_FINANCIAL_REMOTE_FIXTURE"), "synthetic only\n").unwrap();
    copy(&root.join("package-store"), &root.join("reach-artifacts/plugins"));
    let state = root.join(".tradeassembly");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("warden.token"), "fake-financial-fixture-token-at-least-thirty-two-bytes").unwrap();
    let key = root.join("fixture-node.seed");
    fs::write(&key, URL_SAFE_NO_PAD.encode([7u8; 32])).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        for path in [&key, &state.join("warden.token")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let request = json!({"root":root,"scope":scope,"graph":graph,"prepared":prepared,"signal":signal,"wardenOrigin":warden_origin,"brokerOrigin":std::env::var("FINANCIAL_V4_BROKER_ORIGIN").unwrap(),"qualification":"authored bootstrap only; driver must use frozen Core + enforcing SRT"});
    write_screen_interface_evidence("financial_joint", "bootstrap.json", &request);
    let script = std::env::var("FINANCIAL_V4_JOINT_DRIVER").unwrap();
    assert!(std::path::Path::new(&script).is_absolute());
    let mut child = std::process::Command::new("python3").arg(script).arg("--drive")
        .current_dir(root).stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    fs::write(root.join("financial-driver.stdout.log"), &output.stdout).unwrap();
    fs::write(root.join("financial-driver.stderr.log"), &output.stderr).unwrap();
    assert!(output.status.success(), "financial joint failed, retained root {}: {}", root.display(), String::from_utf8_lossy(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "PASS");
    assert_eq!(s.db(), root.join("service.db").to_str().unwrap());
    write_screen_interface_evidence("financial_joint", "result.json", &result);
}

#[test]
#[ignore = "explicit both-ready financial Relay fixture and enforcing SRT gate"]
fn financial_v4_actual_core_relay_http_broker_joint() {
    for key in ["FINANCIAL_V4_JOINT_DRIVER", "FINANCIAL_V4_BROKER_ORIGIN", "FINANCIAL_V4_JOINT_CONFIG", "FINANCIAL_V4_JOINT_READY"] {
        assert!(std::env::var_os(key).is_some(), "missing joint input {key}");
    }
    assert_eq!(std::env::var("FINANCIAL_V4_JOINT_READY").unwrap(), "yes");
    let config: Value = serde_json::from_slice(&fs::read(std::env::var("FINANCIAL_V4_JOINT_CONFIG").unwrap()).unwrap()).unwrap();
    // Bootstrap authored state for the exact independently verified frozen CLI.
    crate::build_identity::install(config["coreRevision"].as_str().unwrap()).unwrap();
    native_execution_plan_fixture(Some("financial_joint"));
}

struct FinancialWitnessClock{base:Arc<dyn ClockPort>,offset:Arc<std::sync::atomic::AtomicI64>}
impl VersionedPort for FinancialWitnessClock{fn descriptors(&self)->Vec<PortDescriptor>{vec![]}}
impl ClockPort for FinancialWitnessClock{fn now_ms(&self)->i64{self.base.now_ms()+self.offset.load(std::sync::atomic::Ordering::SeqCst)}}
fn exercise_financial_witness_refusal(s:&crate::service::TradeAssemblyService, command:crate::service::order_plans::PrepareCommand,offset:Option<Arc<std::sync::atomic::AtomicI64>>){
 use crate::service::data_runner::*;
 let runner=DurableCoreDataRunner::new(s.clone(),"fixture.release".into()).unwrap();
 let c=DataRunnerCommand{schema_version:DATA_RUNNER_SCHEMA_V4.into(),core_release:"fixture.release".into(),workspace_id:"fixture.financial".into(),command_id:"witness-revoked".into(),authority_ref:"provenance.only".into(),idempotency_key:IdempotencyKey::new("witness-revoked").unwrap(),mode:DataRunnerMode::Paper,submitted_at_ms:s.runtime().clock.now_ms(),operation:DataRunnerOperation::Financial{financial:crate::service::order_plans::remote::FinancialOperation::Prepare{order_plan:command}}};
 let witness=runner.verify_financial_dispatch(&c).unwrap();runner.assert_financial_witness_custody(&c,&witness);
 let operations=s.runtime().storage.list_json("plugin_operation_receipts").unwrap();
 let case=if let Some(clock)=offset{clock.store(31000,std::sync::atomic::Ordering::SeqCst);"expired-after-verification"}else{let mut source=s.runtime().plugins.get_instance("capture.source").unwrap().unwrap();source["enabled"]=json!(false);s.runtime().plugins.put_instance("capture.source",source,&ctx("witness-revocation")).unwrap();"revocation-before-effect"};
 let receipt=runner.dispatch_verified(&c,witness).unwrap();assert_eq!(receipt.state,DataRunnerState::Failed);assert!(receipt.result.is_none());assert_eq!(s.runtime().storage.list_json("plugin_operation_receipts").unwrap(),operations);assert!(s.runtime().storage.list_json("execution_order_plans_v1").unwrap().is_empty());assert!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap().is_empty());
 if case=="expired-after-verification"{assert_eq!(receipt.error.as_deref(),Some("order_plan_observation_stale"));}
 write_screen_interface_evidence("financial_witness",&format!("{case}.json"),&json!({"receipt":receipt,"newPluginInvocations":0,"plans":0,"cashHolds":0,"brokerSubmissions":0}));
}
#[test]
#[ignore="focused opaque financial witness currentness under native source revocation"]
fn financial_dispatch_witness_cannot_bypass_current_source_revocation(){crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();native_execution_plan_fixture(Some("financial_token_revoke"));}

#[test]
#[ignore="focused opaque financial witness expires before actual preparation"]
fn financial_dispatch_witness_cannot_bypass_expired_quote(){crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();native_execution_plan_fixture(Some("financial_token_expire"));}

#[test]
#[ignore="explicit both-ready settlement V5 Relay fixture and enforcing SRT gate"]
fn settlement_v5_actual_core_relay_http_provider_joint(){
 for key in ["FINANCIAL_V4_JOINT_DRIVER","FINANCIAL_V4_BROKER_ORIGIN","FINANCIAL_V4_JOINT_CONFIG","FINANCIAL_V4_JOINT_READY"]{assert!(std::env::var_os(key).is_some(),"missing joint input {key}");}
 assert_eq!(std::env::var("FINANCIAL_V4_JOINT_READY").unwrap(),"yes");let config:Value=serde_json::from_slice(&fs::read(std::env::var("FINANCIAL_V4_JOINT_CONFIG").unwrap()).unwrap()).unwrap();assert_eq!(config["jointContract"],"settlement-v5");crate::build_identity::install(config["coreRevision"].as_str().unwrap()).unwrap();native_execution_plan_fixture(Some("financial_settlement_joint"));
}
