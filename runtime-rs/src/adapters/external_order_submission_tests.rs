// Authored protected run + native synthetic adapter. No real broker or OS-sandbox claim.
fn install_v2_broker_fixture(s:&crate::service::TradeAssemblyService,root:&std::path::Path,scenario:&str)->InstalledPluginPackage {
 use crate::adapters::plugin_packages::data_package_tests::{archive_files,materials};
 use tradeassembly_plugin_sdk::broker_v2 as wire;
 use std::os::unix::fs::PermissionsExt;
 let id="fixture.broker.v2";let state=root.join("synthetic-broker");fs::create_dir(&state).unwrap();fs::set_permissions(&state,fs::Permissions::from_mode(0o700)).unwrap();fs::write(state.join("SYNTHETIC_BROKER_ONLY"),b"synthetic only\n").unwrap();
 let template=crate::data_pipeline::tests::fixture().0.nodes.into_iter().find_map(|n|n.selected).unwrap();
 let lookup="schema://tradeassembly/broker-lookup@2";
 let mut operations=vec![];
 for (op,cap,purpose,effect,input) in [(crate::broker_submission::v2::SUBMIT_OPERATION,crate::broker_submission::v2::SUBMIT_CAPABILITY,"paper_trading","write",wire::SUBMIT_SCHEMA),(crate::broker_submission::v2::LOOKUP_OPERATION,crate::broker_submission::v2::LOOKUP_CAPABILITY,"recovery","read",lookup)] {
  let mut v=serde_json::to_value(&template.operation).unwrap();v["id"]=json!(op);v["capability"]=json!(cap);v["purpose"]=json!(purpose);v["effect"]=json!(effect);v["protocol"]=json!("stdio");v["apfActionId"]=json!(if effect=="write"{"order.submit.paper"}else{"broker.order.observe"});v["financeResourceType"]=json!("broker_account");v["dependencies"]=json!([]);v["credentialGrantRequired"]=json!(true);v["accountBindingRequired"]=json!(true);v["traits"]=json!({"modes":["paper"],"instrumentFamilies":["equity"],"inputSchemaRefs":[input],"outputSchemaRefs":[wire::OBSERVATION_SCHEMA],"deterministic":false,"replayable":false});operations.push(v);
 }
 let manifest=json!({"metadata":{"id":id,"version":"1","provider":{"id":"fixture.execution.facts","name":"Synthetic paper broker only"}},"capabilities":operations.iter().map(|v|json!({"id":v["capability"]})).collect::<Vec<_>>(),"operations":operations,"permissions":[],"configuration":{"fields":[]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
 let binary=fixture_executable();let mb=serde_json::to_vec(&manifest).unwrap();let mut d=materials().0["package"].clone();d["plugin"]["id"]=json!(id);d["manifest"]["sha256"]=json!(sha256(&mb));d["targets"][0]["binary"]["sha256"]=json!(sha256(binary));d["responseSchemas"]=json!([]);
 let mut files=vec![("manifest.json".to_string(),mb),("bin/plugin".into(),binary.to_vec())];
 for (i,(id,schema)) in [(wire::SUBMIT_SCHEMA,wire::submit_schema()),(wire::OBSERVATION_SCHEMA,wire::observation_schema()),(lookup,json!({"type":"object","additionalProperties":false,"required":["schemaVersion","clientOrderId","canonicalAccountSha256"],"properties":{"schemaVersion":{"const":"tradeassembly.broker_lookup.v2"},"clientOrderId":{"type":"string"},"canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}}}))].into_iter().enumerate(){let path=format!("broker-schema-{i}.json");let bytes=serde_json::to_vec(&schema).unwrap();d["responseSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":id,"document":{"path":path,"sha256":sha256(&bytes)}}));files.push((path,bytes));}
 files.push(("tradeassembly-plugin.json".into(),serde_json::to_vec(&d).unwrap()));let archive=archive_files(files.iter().map(|(n,b)|(n.as_str(),b.clone())).collect());let path=root.join("synthetic-broker.tgz");fs::write(&path,&archive).unwrap();
 let package=s.runtime().plugin_packages.install(&PluginPackageInstallRequest{source_type:"file".into(),locator:path.to_string_lossy().into(),package_sha256:sha256(&archive),manifest_sha256:canonical_sha256(&manifest).unwrap(),offline:true}).unwrap();
 s.runtime().plugins.install_manifest(id,json!({"pluginRef":id,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx("broker-fixture-package")).unwrap();
 s.runtime().credentials.store("synthetic-broker-handle",&BTreeMap::from([("api_key".into(),"fake-no-access".into()),("api_secret".into(),"fake-no-access-secret".into())])).unwrap();
 let mut instance=json!({"instanceRef":id,"pluginRef":id,"providerRef":"fixture.execution.facts","accountRef":"account://fixture.broker.v2/fixture.paper.provider","enabled":true,"activePackageSha256":package.package_sha256,"configuration":{"fixtureStore":state,"scenario":scenario,"mode":"paper"},"credentialRef":"synthetic-broker-handle","credentialRevision":1,"accountMode":"paper","credentialStatus":{"configured":true},"health":{"state":"ready","checkedAtMs":s.runtime().clock.now_ms(),"connectivityChecked":true,"evidenceCurrent":true,"account":{"id":"fixture.paper.provider","mode":"paper","status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}}});instance["health"]["binding"]=crate::live_execution_checks::live_health_evidence_binding(&instance);s.runtime().plugins.put_instance(id,instance,&ctx("broker-fixture-instance")).unwrap();
 for capability in [crate::broker_submission::v2::SUBMIT_CAPABILITY,crate::broker_submission::v2::LOOKUP_CAPABILITY]{s.runtime().storage.put_json("entitlement_grants",capability,json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("broker-fixture-grant")).unwrap();}
 package
}

fn exercise_native_v2_broker(s:&crate::service::TradeAssemblyService,scope:&data_plans::PlanScope,revision:&str,plan_id:&str,policy:&Value,root:&std::path::Path,scenario:&str){
 // This is an explicitly authored protected deterministic-run fixture. The
 // normal typed activation/compiler producer is a separate integration gate.
 let source=crate::service::order_plans::submission::source(s,plan_id).unwrap();let p=source.historical().unwrap();let rev=s.runtime().storage.get_json("capability_graph_revisions",revision).unwrap().unwrap();
 let activation="fixture-v2-activation";let attempt="fixture-v2-attempt";let config="fixture-v2-config";let owner="fixture-v2-worker";
 let lease=s.runtime().leases.acquire("fixture-v2-execution",owner,s.runtime().clock.now_ms(),180000).unwrap().unwrap();
 let config_value=json!({"configId":config,"mode":"paper","orchestrator":"deterministic","symbol":"SPY","accountRef":"account://fixture.broker.v2/fixture.paper.provider","executionContractVersion":"tradeassembly.typed_execution.v2","orderPlanPolicySha256":canonical_sha256(policy).unwrap(),"orderPlanBrokerNode":"exec_broker"});
 s.runtime().storage.put_json("execution_configs",config,config_value.clone(),&ctx("authored-run")).unwrap();
 let mut run=config_value;run["activationId"]=json!(activation);run["state"]=json!("active");run["strategyId"]=json!(scope.strategy_id);run["strategyVersionId"]=json!(scope.strategy_version_id);run["strategySpecHash"]=json!(scope.strategy_spec_hash);run["capabilityGraphRevisionId"]=json!(revision);run["capabilityGraphFingerprint"]=rev["graphFingerprint"].clone();run["localLiveAuthority"]=Value::Null;run["orderPlanApprovals"]=json!({(plan_id):{"planId":plan_id,"entryId":p.entry_id,"signalReceiptSha256":p.command.signal_receipt_sha256,"canonicalAccountSha256":p.canonical_account.as_ref().unwrap().identity_sha256,"leaseResource":lease.resource,"leaseOwner":lease.owner,"leaseFence":lease.fencing_token}});
 for(ns,key)in[("execution_runs",format!("run_{activation}")),("execution_activations",activation.into())]{s.runtime().storage.put_json(ns,&key,run.clone(),&ctx("authored-run")).unwrap();}
 s.runtime().storage.put_json("execution_attempts",attempt,json!({"attemptId":attempt,"activationId":activation,"state":"running","owner":owner,"leaseResource":lease.resource,"fencingToken":lease.fencing_token}),&ctx("authored-run")).unwrap();
 s.runtime().storage.put_json("execution_controls",activation,json!({"activationId":activation,"newEntriesPaused":false}),&ctx("authored-run")).unwrap();s.runtime().storage.put_json("execution_reconciliation",activation,json!({"activationId":activation,"state":"clear","newEntriesPaused":false}),&ctx("authored-run")).unwrap();
 let command=json!({"planId":plan_id,"activationId":activation,"attemptId":attempt,"brokerRequirementRef":"exec_broker"});
 let foreign=s.for_authenticated_invocation("fixture","foreign",None,None);let refused=foreign.call_mcp_tool("tradeassembly.strategy.order_plan.submit",json!({"scope":scope,"submission":command,"accountMode":"paper","idempotencyKey":"foreign-v2"}));assert_eq!(refused["isError"],true,"{refused}");assert!(!root.join("synthetic-broker/calls.log").exists());
 let submit=|key:&str|s.handle_http("POST","/product/strategies/order-plans/submit",json!({"scope":scope,"submission":command,"accountMode":"paper","idempotencyKey":key}));
 let first=submit("native-v2-submit");eprintln!("native V2 {scenario} response: {} {}",first.status,first.body);
 let calls=||fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap_or_default();
 assert_eq!(calls().lines().filter(|v|*v=="submit").count(),1,"{}",first.body);
 let record=source.state().unwrap().unwrap();
 if scenario=="reject"{assert_eq!(record.phase,crate::service::order_plans::submission::Phase::Rejected);assert_eq!(record.retained_cash.to_string(),"0");}
 else {
  assert!(matches!(record.phase,crate::service::order_plans::submission::Phase::Sending|crate::service::order_plans::submission::Phase::Unknown));assert_eq!(record.retained_cash.to_string(),"41");
  let release=s.handle_http("POST","/product/strategies/order-plans/release",json!({"scope":scope,"planId":plan_id,"accountMode":"paper","idempotencyKey":"unsafe-release"}));assert_ne!(release.status,200,"{}",release.body);
  let retry=submit("native-v2-new-transport");assert_ne!(retry.status,200,"{}",retry.body);assert_eq!(calls().lines().filter(|v|*v=="submit").count(),1);
  let recovered=s.handle_http("POST","/product/strategies/order-plans/reconcile",json!({"scope":scope,"planId":plan_id,"accountMode":"paper","idempotencyKey":"native-v2-reconcile"}));assert_eq!(recovered.status,200,"{}",recovered.body);assert_eq!(source.state().unwrap().unwrap().phase,crate::service::order_plans::submission::Phase::Consumed);assert_eq!(source.state().unwrap().unwrap().retained_cash.to_string(),"41");
  let replay=s.call_mcp_tool("tradeassembly.strategy.order_plan.reconcile",json!({"scope":scope,"planId":plan_id,"accountMode":"paper","idempotencyKey":"native-v2-reconcile"}));assert_ne!(replay["isError"],true,"{replay}");assert_eq!(calls().lines().filter(|v|*v=="lookup").count(),1);write_screen_interface_evidence("execution_v2","recovery.json",&recovered.body);
 }
 let _=submit("native-v2-final-retry");assert_eq!(calls().lines().filter(|v|*v=="submit").count(),1);write_screen_interface_evidence("execution_v2",&format!("{scenario}.json"),&json!({"first":first.body,"state":crate::service::order_plans::submission::summary(source.state().unwrap().unwrap()),"calls":calls(),"qualification":"synthetic native adapter; authored protected run; no production OS sandbox or real broker"}));
}

#[test]
#[ignore="explicit native synthetic C5 fixture"]
fn native_v2_c5_rejection_and_unknown_acceptance_reconcile_without_resubmission(){for case in ["reject","timeout_after_accept"]{native_execution_plan_fixture(Some(case));}}
