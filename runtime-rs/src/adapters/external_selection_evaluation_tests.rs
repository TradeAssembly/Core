// Synthetic native process fixture, never provider/account/trading evidence.
fn configure_selected_evaluation_fixture(service:&crate::service::TradeAssemblyService,spec:&mut Value,request:&PipelineRequest){
    use crate::selection_strategy::*;
    let snapshot=json!({"instrument":"A","start":10*86400000i64,"interval":86400000,"observedAtMs":service.runtime().clock.now_ms(),"values":[1,2,3,4,5,6,7]});
    let mut nodes=request.nodes.iter().map(|n|NodeTemplate{graph_node_id:n.graph_node_id.clone(),parameters:json!({}),context:ContextTemplate::Subject{interval_ms:Some(86400000)},input_contexts:BTreeMap::new()}).collect::<Vec<_>>();
    nodes[0].parameters=json!({"digest":canonical_sha256(&snapshot).unwrap(),"snapshot":snapshot});
    let operand=|sample|OperandTemplate::Output{node:"derived".into(),port:"value".into(),instrument:InstrumentRef::Subject,sample};
    let template=SelectionStrategy{schema_version:"tradeassembly.selection_strategy.v1".into(),selections:BTreeMap::from([("selected".into(),SelectorRef{market_requirement_ref:"signal_primary".into(),selector_ref:"screened".into()})]),trade_selection_ref:"selected".into(),nodes,bindings:request.bindings.clone(),entry:RuleTemplate::Compare{left:operand(crate::data_strategy::Sample::Current),op:crate::data_strategy::Comparison::Greater,right:operand(crate::data_strategy::Sample::Previous)},exit:RuleTemplate::IsTrue{value:OperandTemplate::Boolean{value:false}}};
    spec["stages"]["evaluate"]["substeps"][0]["config"]=json!({(CONFIG_KEY):template});
}
fn exercise_selected_evaluation_fixture(service:&crate::service::TradeAssemblyService,selection:&crate::service::instrument_screens::ScreenReceipt,case:&str){
    use crate::service::{selection_plans::SelectionPlanCommand,data_evaluation::{EvaluationStep,EvaluationReceipt}};
    let command=SelectionPlanCommand{scope:selection.command.scope.clone(),capability_graph_revision:selection.command.capability_graph_revision.clone(),mode:"paper".into(),subject_instrument_id:"A".into(),selection_receipts:BTreeMap::from([("selected".into(),selection.selection_id.clone())]),source_captures:BTreeMap::new()};
    let orders=service.runtime().storage.list_json("orders").unwrap();let positions=service.runtime().storage.list_json("positions").unwrap();
    let before=service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap();
    let response=service.handle_http("POST","/product/strategies/selection-plans/prepare",json!({"selectionPlan":command,"idempotencyKey":"selected-native-prepare"}));
    if selection.instrument_set.status==tradeassembly_plugin_sdk::instrument_set::SelectionStatus::CompleteEmpty {
        assert_eq!(response.status,409,"{}",response.body);assert_eq!(response.body["error"]["code"],"selection_plan_no_eligible_instrument");
        assert_eq!(service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap(),before);
        assert_eq!(service.runtime().storage.list_json("orders").unwrap(),orders);assert_eq!(service.runtime().storage.list_json("positions").unwrap(),positions);
        write_screen_interface_evidence(case,"selected-empty-refusal.json",&response.body);return;
    }
    assert_eq!(response.status,200,"{}",response.body);
    if std::env::var_os("REACH_DATA_TEST_CORE_BINARY").is_some() {
        use crate::service::data_runner::{DataRunnerCommand,DataRunnerMode,DataRunnerOperation};
        let runner=DataRunnerCommand{schema_version:"tradeassembly.core-data-runner.v2".into(),core_release:"fixture.release".into(),workspace_id:"fixture.workspace".into(),command_id:"fixture.selected.create".into(),authority_ref:"provenance.only".into(),idempotency_key:IdempotencyKey::new("fixture.selected.create").unwrap(),mode:DataRunnerMode::Paper,submitted_at_ms:service.runtime().clock.now_ms(),operation:DataRunnerOperation::PrepareSelectionPlan{selection_plan:command.clone()}};
        let root=std::path::Path::new(service.db()).parent().unwrap();
        maybe_run_joint_reach_data_process(root,service,&runner,case);
        return;
    }

    let plan:data_plans::StoredDataPlan=serde_json::from_value(response.body["plan"].clone()).unwrap();
    assert_eq!(plan.content.plan.request.trade_instruments[0].instrument_id,"A");assert!(plan.content.plan.request.nodes.iter().all(|n|n.context.instruments[0].instrument_id=="A"));
    write_screen_interface_evidence(case,"selected-plan.json",&response.body);
    let step=EvaluationStep{run_id:"selected-native-run".into(),plan_revision_id:plan.revision_id.clone(),window:EvaluationWindow{event_start_ms:10*86400000,event_end_ms:16*86400000,as_of_ms:service.runtime().clock.now_ms()},expected_checkpoint_sha256:None};
    let response=service.handle_http("POST","/product/strategies/data-runs/step",json!({"step":step,"idempotencyKey":"selected-native-evaluate"}));assert_eq!(response.status,200,"{}",response.body);
    let receipt:EvaluationReceipt=serde_json::from_value(response.body["receipt"].clone()).unwrap();
    assert_eq!(receipt.frames_processed,6);assert_eq!(receipt.signals.len(),6);assert_eq!(receipt.signals[0].entry,crate::data_strategy::SignalValue::WarmingUp);assert_eq!(receipt.signals.last().unwrap().entry,crate::data_strategy::SignalValue::Ready{value:true});
    assert!(receipt.signals.iter().all(|s|s.trade_instruments[0].instrument_id=="A" && s.known_at_ms>=selection.instrument_set.known_at_ms && s.upstream_content_sha256.contains(&selection.receipt_sha256)));
    write_screen_interface_evidence(case,"selected-evaluation.json",&response.body);
    let db=service.db();let root=std::path::Path::new(db).parent().unwrap();
    let mut runtime=crate::adapters::local::test_runtime(db);
    runtime.plugin_packages=Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(root.join("package-store")).unwrap());
    runtime.plugin_operations=Arc::new(crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(runtime.storage.clone(),runtime.plugins.clone(),runtime.credentials.clone(),runtime.plugin_packages.clone(),runtime.clock.clone(),Arc::new(TracedFixtureSandbox)));
    let reopened=crate::service::TradeAssemblyService::from_runtime(db,runtime);
    assert_eq!(data_plans::current(&reopened,&plan.revision_id).unwrap(),plan);
    let mut next=step.clone();next.window.event_start_ms=16*86400000;next.window.event_end_ms=17*86400000;next.window.as_of_ms=reopened.runtime().clock.now_ms();next.expected_checkpoint_sha256=Some(receipt.checkpoint.checkpoint_sha256.clone());
    let resumed=reopened.handle_http("POST","/product/strategies/data-runs/step",json!({"step":next,"idempotencyKey":"selected-native-resume"}));assert_eq!(resumed.status,200,"{}",resumed.body);
    let resumed_receipt:EvaluationReceipt=serde_json::from_value(resumed.body["receipt"].clone()).unwrap();assert_eq!(resumed_receipt.frames_processed,1);assert_eq!(resumed_receipt.signals[0].entry,crate::data_strategy::SignalValue::Ready{value:true});
    write_screen_interface_evidence(case,"selected-service-restart.json",&resumed.body);

    let mut not_selected=command;not_selected.subject_instrument_id="B".into();let before=service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap();let refused=service.call_mcp_tool("tradeassembly.strategy.selection_plan.prepare",json!({"selectionPlan":not_selected,"idempotencyKey":"selected-native-deny"}));assert_eq!(refused["isError"],true);assert_eq!(service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap(),before);
    assert_eq!(service.runtime().storage.list_json("orders").unwrap(),orders);assert_eq!(service.runtime().storage.list_json("positions").unwrap(),positions);
    write_screen_interface_evidence(case,"selected-proof-scope.json",&json!({"selection":"actual authored native screener over loopback synthetic universe","evaluation":"actual native replay/calculation processes; source inputs explicitly authored and unverified, not provider captures","brokerOrders":false,"managedPositionEffects":false,"automaticSchedulerDispatch":false,"orderPlanning":false}));
}
