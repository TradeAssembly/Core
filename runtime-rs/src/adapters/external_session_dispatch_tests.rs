// Real native fake-data process fixture; not live provider/broker qualification.
fn configure_dispatch_calendar_fixture(s:&crate::service::TradeAssemblyService,spec:&mut Value){
 let binding=json!({"capability_requirement_ref":"calendar","calendar_ref":"fixture.exchange","timezone":"Etc/UTC","session_scope":"fixture_primary","max_age_ms":86400000});
 spec["stages"]["market_clock"]["policy"]["session_triggers"]=json!([{"version":"tradeassembly.session_trigger.v1","trigger_id":"preopen","calendar":binding,"anchor":"open","offset_ms":-300000,"missed_run":{"policy":"catch_up","max_lateness_ms":60000},"retry":{"max_attempts":3,"backoff_ms":1000},"execution_window":{"calendar":binding,"open_offset_ms":0,"close_offset_ms":0},"selections":[{"signal_requirement_ref":"signal_primary","selector_ref":"screened"}]}]);
 let mut req=spec["capability_requirements"]["required"][0].clone();req["requirement_id"]=json!("calendar");req["capability"]=json!("test.session.calendar@1");req["stage_refs"]=json!(["market_clock"]);req["substep_refs"]=json!([]);req["dependency_refs"]=json!([]);
 for(k,v)in[("schema_refs",json!([tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA])),("data_shapes",json!([])),("fields",json!([])),("input_paths",json!([])),("operations",json!([])),("deterministic",json!(false)),("replayable",json!(false))]{req["constraints"][k]=v;}
 req["constraints"].as_object_mut().unwrap().remove("timeframe");req["constraints"].as_object_mut().unwrap().remove("max_freshness");spec["capability_requirements"]["required"].as_array_mut().unwrap().push(req);
 s.runtime().storage.put_json("entitlement_grants","test.session.calendar@1",json!({"grantId":"test.session.calendar@1","profile":"local_full","source":"test","capability":"test.session.calendar@1","effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("dispatch-calendar-grant")).unwrap();
}
fn exercise_session_dispatch_fixture(base:&crate::service::TradeAssemblyService,scope:&data_plans::PlanScope,revision:&str,case:&str){
 use crate::service::{session_schedules as schedules,session_dispatch as dispatch};
 crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
 let calendar=crate::service::data_collections::acquire(base,crate::service::data_collections::CollectionCommand{scope:scope.clone(),capability_graph_revision:revision.into(),acquisition_node:"calendar".into(),mode:"paper".into(),response_schema_ref:tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA.into(),input:json!({})},&ctx("dispatch-calendar-read")).unwrap();
 let now=base.runtime().clock.now_ms();let date=chrono::DateTime::from_timestamp_millis(now).unwrap().date_naive().to_string();
 let c=schedules::ScheduleCommand{scope:scope.clone(),capability_graph_revision:revision.into(),mode:"paper".into(),trigger_ref:"preopen".into(),session_date:date,calendar_collection_id:calendar.collection_id.clone(),execution_calendar_collection_id:calendar.collection_id,knowledge_cutoff_ms:now};
 let response=base.handle_http("POST","/product/strategies/session-schedules/prepare",json!({"schedule":c,"idempotencyKey":"dispatch-schedule"}));assert_eq!(response.status,200,"{}",response.body);
 let occurrence=response.body["schedule"]["occurrence"]["occurrenceId"].as_str().unwrap().to_string();
 let canonical_db=std::path::Path::new(base.db()).canonicalize().unwrap();
 let owner=crate::local_owner_identity::LocalOwnerIdentity::for_database(&canonical_db).unwrap();
 let shift=Arc::new(std::sync::atomic::AtomicI64::new(0));
 let mut runtime=(*base.runtime()).clone();runtime.identity=Arc::new(crate::adapters::local::system::LocalOwnerIdentityPort::new(canonical_db.to_str().unwrap()).unwrap());runtime.clock=Arc::new(AdvancedClock{base:runtime.clock.clone(),advance:shift.clone()});
 runtime.plugin_operations=Arc::new(crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(runtime.storage.clone(),runtime.plugins.clone(),runtime.credentials.clone(),runtime.plugin_packages.clone(),runtime.clock.clone(),Arc::new(TracedFixtureSandbox)));
 runtime.finance_authority=Arc::new(FixtureSchedulerAuthority);
 let mut service=crate::service::TradeAssemblyService::from_runtime(base.db(),runtime).for_authenticated_invocation(&owner.issuer,&owner.stable_identity_id,None,None);
 service.bind_owned_object("strategy",&scope.strategy_id).unwrap();for p in service.runtime().plugins.list_instances().unwrap(){service.bind_owned_object("plugin_instance",p["instanceRef"].as_str().unwrap()).unwrap();}
 service.runtime().storage.put_json("positions","previous-member",json!({"symbol":"OLD","quantity":2,"state":"open"}),&ctx("existing-managed-position")).unwrap();service.runtime().storage.put_json("orders","unrelated-pending",json!({"symbol":"OLD","state":"pending"}),&ctx("existing-pending-order")).unwrap();
 let positions=service.runtime().storage.list_json("positions").unwrap();let orders=service.runtime().storage.list_json("orders").unwrap();
 let request=dispatch::DispatchRequest{occurrence_id:occurrence,scope:scope.clone(),capability_graph_revision:revision.into(),mode:"paper".into(),subjects:dispatch::SubjectPolicy::AllSelected{max_members:1},event_start_ms:10*86400000,event_end_ms:12*86400000,interval_ms:86400000,max_frames_per_step:2,source_captures_by_subject:BTreeMap::new()};
 if case=="session_process" {exercise_compiled_session_process(&service,&request,case);return;}
 let remote_runner=crate::service::data_runner::DurableCoreDataRunner::new(service.clone(),"fixture.release".into()).unwrap();
 let remote_clock=service.runtime().clock.clone();
 let remote_command=|id:&str,remote:dispatch::RemoteRequest| crate::service::data_runner::DataRunnerCommand { schema_version:crate::service::data_runner::DATA_RUNNER_SCHEMA_V3.into(),core_release:"fixture.release".into(),workspace_id:"fixture.workspace".into(),command_id:id.into(),authority_ref:"provenance.only".into(),idempotency_key:IdempotencyKey::new(id).unwrap(),mode:crate::service::data_runner::DataRunnerMode::Paper,submitted_at_ms:remote_clock.now_ms(),operation:crate::service::data_runner::DataRunnerOperation::DispatchSession{dispatch:remote} };
 let remote_enqueue=remote_command("remote-enqueue",dispatch::RemoteRequest::Enqueue{job:request.clone()});
 if case=="session_eval" {let binding=remote_runner.mutation_binding(&remote_enqueue).unwrap();assert_eq!(binding.action,"scheduler.run");assert_eq!(binding.resource_kind.as_deref(),Some("scheduler"));let r=remote_runner.dispatch(&remote_enqueue).unwrap();assert_eq!(r.state,crate::service::data_runner::DataRunnerState::Completed,"{r:?}");write_screen_interface_evidence(case,"remote-enqueue.json",&serde_json::to_value(r).unwrap());}
 let job=dispatch::enqueue(&service,request.clone()).unwrap();
 if case=="session_closed" {assert_eq!(job.status,dispatch::Status::Closed);let before=service.runtime().storage.list_json("plugin_operation_receipts").unwrap();assert_eq!(dispatch::advance(&service,&job.job_id).unwrap().status,dispatch::Status::Closed);assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),before);write_screen_interface_evidence(case,"dispatch-final.json",&serde_json::to_value(job).unwrap());return;}
 assert_eq!(job.status,dispatch::Status::Pending);
 let receipts=service.runtime().storage.list_json("plugin_operation_receipts").unwrap();let pending=dispatch::advance(&service,&job.job_id).unwrap();assert_eq!(pending.status,dispatch::Status::Pending);assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),receipts);
 assert_eq!(dispatch::enqueue(&service,request.clone()).unwrap().job_id,job.job_id);
 let mut conflict=request.clone();conflict.event_end_ms+=86400000;assert_eq!(dispatch::enqueue(&service,conflict).unwrap_err(),"session_dispatch_occurrence_conflict");
 let foreign=service.for_authenticated_invocation("fixture","foreign",None,None);let refused=dispatch::run(&foreign,json!({"kind":"inspect","jobId":job.job_id}));assert_eq!(refused["ok"],false);
 if case=="session_missed" {shift.store(360001,std::sync::atomic::Ordering::SeqCst);let before=service.runtime().storage.list_json("plugin_operation_receipts").unwrap();let missed=dispatch::advance(&service,&job.job_id).unwrap();assert_eq!(missed.status,dispatch::Status::SkippedMissed);assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),before);assert_eq!(service.runtime().storage.list_json("positions").unwrap(),positions);assert_eq!(service.runtime().storage.list_json("orders").unwrap(),orders);write_screen_interface_evidence(case,"dispatch-final.json",&serde_json::to_value(missed).unwrap());return;}
 let background=matches!(case,"session_background"|"session_resume");
 if background {
  let start=service.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"start_worker","jobId":job.job_id},"idempotencyKey":"background-start"}));assert_eq!(start.body["ok"],true,"{}",start.body);
  let deadline=std::time::Instant::now()+std::time::Duration::from_secs(180);
  loop {let status=dispatch::run(&service,json!({"kind":"inspect","jobId":job.job_id}));if status["body"]["worker"]["iterations"].as_u64().unwrap()>=1{break}assert_eq!(status["body"]["worker"]["running"],true,"{status}");assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(200));}
  assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),receipts);
  if case=="session_resume" {
   dispatch::halt_worker_for_test(&service,&job.job_id);
   loop{let view=dispatch::run(&service,json!({"kind":"inspect","jobId":job.job_id}));if view["body"]["worker"]["running"]==false{assert_eq!(view["body"]["worker"]["lastError"],"fixture_worker_stopped_without_disabling_intent");write_screen_interface_evidence(case,"worker-before-service-recreation.json",&view);break}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(200));}
   let intent=service.runtime().storage.list_json("session_dispatch_worker_intents_v1").unwrap();assert_eq!(intent.len(),1);assert_eq!(intent[0].1["enabled"],true);
   let reopened=crate::service::TradeAssemblyService::from_runtime(service.db(),(*service.runtime()).clone()).for_authenticated_invocation(&owner.issuer,&owner.stable_identity_id,None,None);service=reopened;
   let resumed=service.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"resume_workers"},"idempotencyKey":"explicit-owned-worker-resume"}));assert_eq!(resumed.body["ok"],true,"{}",resumed.body);assert_eq!(resumed.body["body"]["workersResumed"],1);assert_eq!(service.runtime().storage.list_json("session_dispatch_worker_intents_v1").unwrap(),intent);write_screen_interface_evidence(case,"worker-resumed-existing-intent.json",&resumed.body);
   loop{let view=dispatch::run(&service,json!({"kind":"inspect","jobId":job.job_id}));if view["body"]["worker"]["iterations"].as_u64().unwrap()>=1{break}assert_eq!(view["body"]["worker"]["running"],true,"{view}");assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(200));}
  }
  let held=service.runtime().leases.acquire(&format!("session-dispatch:{}",job.job_id),"fixture-competing-worker",service.runtime().clock.now_ms(),600000).unwrap().unwrap();
  shift.store(300001,std::sync::atomic::Ordering::SeqCst);dispatch::wake_for_test(&service,&job.job_id);
  loop {let status=dispatch::run(&service,json!({"kind":"inspect","jobId":job.job_id}));if status["body"]["worker"]["iterations"].as_u64().unwrap()>=2{break}assert_eq!(status["body"]["worker"]["running"],true,"{status}");assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(200));}
  assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),receipts,"competing lease prevents provider effects");service.runtime().leases.release(&held).unwrap();dispatch::wake_for_test(&service,&job.job_id);
 }else{shift.store(300001,std::sync::atomic::Ordering::SeqCst);let first=dispatch::advance(&service,&job.job_id).unwrap();assert_eq!(first.status,dispatch::Status::Running,"{first:?}");}

 if case=="session_eval"{
  let db=rusqlite::Connection::open(service.db()).unwrap();db.execute_batch("CREATE TRIGGER session_progress_loss BEFORE UPDATE ON tradeassembly_kv WHEN NEW.namespace='session_dispatch_jobs_v1' AND json_extract(NEW.value_json,'$.results.\"screen:selected\"') IS NOT NULL BEGIN SELECT RAISE(ABORT,'injected phase receipt loss'); END;").unwrap();
  assert!(dispatch::advance(&service,&job.job_id).is_err());db.execute_batch("DROP TRIGGER session_progress_loss;").unwrap();drop(db);
  let before=service.runtime().storage.list_json("plugin_operation_receipts").unwrap();
  shift.store(420002,std::sync::atomic::Ordering::SeqCst);
  let reopened=crate::service::TradeAssemblyService::from_runtime(service.db(),(*service.runtime()).clone()).for_authenticated_invocation(&owner.issuer,&owner.stable_identity_id,None,None);
  let recovered=dispatch::advance(&reopened,&job.job_id).unwrap();assert_eq!(recovered.status,dispatch::Status::Running);assert_eq!(reopened.runtime().storage.list_json("plugin_operation_receipts").unwrap(),before,"phase recovery must reuse original domain result");
 }
 let mut final_job=None;
 let deadline=std::time::Instant::now()+std::time::Duration::from_secs(180);
 for n in 0..if background {10000}else{8} {
  let response=if case=="session_eval" {let c=remote_command(&format!("remote-advance-{n}"),dispatch::RemoteRequest::Advance{job_id:job.job_id.clone()});let receipt=remote_runner.dispatch(&c).unwrap();assert_eq!(receipt.state,crate::service::data_runner::DataRunnerState::Completed,"{receipt:?}");let crate::service::data_runner::DataRunnerResult::Dispatch{job:result}=receipt.result.unwrap() else{panic!("wrong result")};crate::service::ServiceResponse::ok(json!({"ok":true,"body":{"job":result}}))}else if background {crate::service::ServiceResponse::ok(dispatch::run(&service,json!({"kind":"inspect","jobId":job.job_id})))}else{service.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"advance","jobId":job.job_id},"idempotencyKey":format!("dispatch-public-{n}")}))};assert_eq!(response.status,200,"{}",response.body);assert_eq!(response.body["ok"],true,"{}",response.body);
  if background {assert!(std::time::Instant::now()<deadline,"background deadline");}
  let current:dispatch::Job=serde_json::from_value(response.body["body"]["job"].clone()).unwrap();
  if matches!(current.status,dispatch::Status::Complete|dispatch::Status::CompleteEmpty|dispatch::Status::Partial|dispatch::Status::Failed){final_job=Some(current);break;}
  if background {assert_eq!(response.body["body"]["worker"]["running"],true,"{}",response.body);std::thread::sleep(std::time::Duration::from_millis(200));}
 }
 let final_job=final_job.expect("bounded dispatch reaches a typed terminal result");let expected=match case{"session_empty"=>dispatch::Status::CompleteEmpty,"session_partial"=>dispatch::Status::Partial,_=>dispatch::Status::Complete};assert_eq!(final_job.status,expected,"{final_job:?}");
 if case=="session_eval" {exercise_scheduler_ack_recovery(&service,&remote_enqueue);let reopened=crate::service::data_runner::DurableCoreDataRunner::new(service.clone(),"fixture.release".into()).unwrap();let original=reopened.dispatch(&remote_enqueue).unwrap();assert_eq!(original.state,crate::service::data_runner::DataRunnerState::Completed);assert_eq!(original.result.as_ref().unwrap(),remote_runner.dispatch(&remote_enqueue).unwrap().result.as_ref().unwrap());write_screen_interface_evidence(case,"remote-original-after-progress.json",&serde_json::to_value(original).unwrap());}
 let before=service.runtime().storage.list_json("plugin_operation_receipts").unwrap();assert_eq!(dispatch::advance(&service,&job.job_id).unwrap().status,expected);assert_eq!(dispatch::enqueue(&service,request).unwrap().job_id,job.job_id);assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),before);
 if matches!(case,"session_eval"|"session_background"|"session_resume"){let receipts=service.runtime().storage.list_json("data_evaluation_receipts_v1").unwrap();assert_eq!(receipts.len(),1);assert_eq!(receipts[0].1["signals"][1]["entry"],json!({"status":"ready","value":true}));write_screen_interface_evidence(case,"dispatch-evaluation.json",&receipts[0].1);}
 else{assert!(service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap().is_empty());}
 assert_eq!(service.runtime().storage.list_json("positions").unwrap(),positions);assert_eq!(service.runtime().storage.list_json("orders").unwrap(),orders);
 if background {let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);loop{let view=dispatch::run(&service,json!({"kind":"inspect","jobId":job.job_id}));if view["body"]["worker"]["running"]==false{assert!(view["body"]["worker"]["lastError"].is_null(),"{view}");write_screen_interface_evidence(case,"dispatch-worker-stopped.json",&view);break}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(200));}}
 write_screen_interface_evidence(case,"dispatch-final.json",&serde_json::to_value(final_job).unwrap());
}

// Mock transport commits before losing its acknowledgment. Core must reconcile
// from durable local storage before asking for another delivery. This is not
// actual Relay SQL/HTTP qualification (owned by the separate joint gate).
fn exercise_scheduler_ack_recovery(service:&crate::service::TradeAssemblyService,command:&crate::service::data_runner::DataRunnerCommand){
 use crate::service::{reach_data::ReachDataCommand,reach_data_consumer::*};
 use crate::ports::InstallationSignerPort;
 struct Transport {record:std::sync::Mutex<DataDelivery>,claims:std::sync::atomic::AtomicUsize,records:std::sync::Mutex<Vec<String>>}
 impl ReachDataTransport for Transport {
  fn claim(&self,_:&DataNodeCall)->Result<Option<DataDelivery>,String>{self.claims.fetch_add(1,std::sync::atomic::Ordering::SeqCst);Ok(Some(self.record.lock().unwrap().clone()))}
  fn accept(&self,_:&DataNodeCall)->Result<DataDelivery,String>{let mut r=self.record.lock().unwrap();r.state=DataDeliveryState::Accepted;r.revision=2;Ok(r.clone())}
  fn record(&self,r:&DataNodeRecord)->Result<DataDelivery,String>{let mut nonces=self.records.lock().unwrap();assert!(!nonces.contains(&r.request.nonce));nonces.push(r.request.nonce.clone());let mut stored=self.record.lock().unwrap();if let Some(prior)=&stored.receipt{assert_eq!(prior,&r.receipt);}stored.receipt=Some(r.receipt.clone());stored.state=DataDeliveryState::Completed;stored.revision=3;if nonces.len()==1{Err("fixture_committed_ack_lost".into())}else{Ok(stored.clone())}}
 }
 let s=service.clone().with_hosted_connection(crate::runtime_config::RuntimeConfig::local(service.db()),"fixture.actor".into(),None);
 let node=crate::ports::ReachNodeScope{tenant_id:"fixture.tenant".into(),workspace_id:command.workspace_id.clone(),node_id:"fixture.node".into(),actor_ref:"fixture.actor".into()};
 let source=source_binding(&s,&node).unwrap();let runner=crate::service::data_runner::DurableCoreDataRunner::new(s.clone(),command.core_release.clone()).unwrap();let now=s.runtime().clock.now_ms();
 let c=ReachDataCommand{schema:command.reach_schema().into(),node:node.clone(),request_id:command.command_id.clone(),idempotency_key:command.idempotency_key.as_str().into(),issued_at_ms:now,expires_at_ms:now+100000,source_binding_sha256:source,resource:runner.mutation_binding(command).unwrap(),command:command.clone()};c.validate().unwrap();
 let transport=Arc::new(Transport{record:std::sync::Mutex::new(DataDelivery{command_sha256:c.command_sha256().unwrap(),command:c,state:DataDeliveryState::Delivered,revision:1,receipt:None}),claims:std::sync::atomic::AtomicUsize::new(0),records:std::sync::Mutex::new(vec![])});
 let signer:Arc<dyn InstallationSignerPort>=Arc::new(crate::adapters::connected_runner::Ed25519InstallationSigner::from_seed(node.node_id.clone(),[7;32]).unwrap());
 let make=|| ReachDataConsumer::new(s.clone(),command.core_release.clone(),node.clone(),signer.clone(),transport.clone(),Arc::new(crate::adapters::connected_runner::OsRunnerEntropy)).unwrap();
 let before=s.runtime().storage.list_json("plugin_operation_receipts").unwrap();
 assert_eq!(make().poll_once().unwrap_err(),"fixture_committed_ack_lost");
 let recovered=make().poll_once().unwrap();assert!(matches!(recovered,ReachDataPoll::Recorded{..}));
 assert_eq!(transport.claims.load(std::sync::atomic::Ordering::SeqCst),1,"pending ack is reconciled before claim");assert_eq!(transport.records.lock().unwrap().len(),2);
 assert_eq!(s.runtime().storage.list_json("plugin_operation_receipts").unwrap(),before,"ack recovery must not recalculate");
 write_screen_interface_evidence("session_eval","remote-ack-recovery.json",&serde_json::to_value(recovered).unwrap());
}

struct FixtureSchedulerAuthority;
impl crate::finance_authority::FinanceAuthorityPort for FixtureSchedulerAuthority {
 fn command_policy_sha256(&self)->Result<String,String>{crate::data_pipeline::canonical_sha256(&json!({"fixturePolicy":"local-scheduler-only-v1"})).map_err(|e|e.to_string())}
 fn prepare_command(&self,s:&dyn StoragePort,e:&crate::control_plane::ControlPlaneCommandEnvelope)->Result<(),String>{crate::finance_authority::TestFinanceAuthority.prepare_command(s,e)}
 fn complete_command(&self,s:&dyn StoragePort,e:&crate::control_plane::ControlPlaneCommandEnvelope,status:u16)->Result<(),String>{crate::finance_authority::TestFinanceAuthority.complete_command(s,e,status)}
 fn audit(&self,s:&dyn StoragePort)->Value{crate::finance_authority::TestFinanceAuthority.audit(s)}
}
