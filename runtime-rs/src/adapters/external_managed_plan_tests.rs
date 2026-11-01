include!("external_managed_close_outcome_tests.rs");
// Generic reversal plumbing qualification only; not actual MA/TSI math.
fn configure_managed_plan_native_spec(
    s: &crate::service::TradeAssemblyService,
    spec: &mut Value,
    bindings: &mut Value,
    replay: &InstalledPluginPackage,
) {
    use crate::selection_strategy::*;
    let config = &mut spec["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY];
    let mut template: SelectionStrategy = serde_json::from_value(config.clone()).unwrap();
    template.nodes[0].context = ContextTemplate::Subject {
        interval_ms: Some(1),
    };
    let operand = |sample| OperandTemplate::Output {
        node: "exec_signal".into(),
        port: "value".into(),
        instrument: InstrumentRef::Subject,
        sample,
    };
    template.exit = RuleTemplate::Compare {
        left: operand(crate::data_strategy::Sample::Current),
        op: crate::data_strategy::Comparison::Less,
        right: operand(crate::data_strategy::Sample::Previous),
    };
    *config = serde_json::to_value(template).unwrap();
    if !spec["capability_requirements"]["required"].as_array().unwrap().iter()
        .any(|requirement| requirement["requirement_id"] == "calendar")
    {
        configure_dispatch_calendar_fixture(s, spec);
    }
    spec["stages"]["market_clock"]["policy"]["session_triggers"][0]["offset_ms"] = json!(0);
    spec["stages"]["market_clock"]["policy"]["session_triggers"][0]["missed_run"]
        ["max_lateness_ms"] = json!(600000);
    bindings["calendar"] = json!({"pluginInstanceRef":replay.plugin_ref,"pluginRef":replay.plugin_ref,"operationId":"calendar"});
    spec["stages"]["exit_policy"]["enabled"] = json!(true);
    spec["stages"]["exit_policy"]["policy"] = json!({"monitoring":["synthetic"],"triggers":[{"id":"reversal","kind":"indicator","scope":"full_position"}],"composition":"priority","precedence":["reversal"],"end_of_data":"preserve_open_positions"});
    spec["stages"]["exit_policy"]["config"]["tradeassembly.managed_exit.v1"] = json!({"rules":{"schemaVersion":"tradeassembly.position_exit_rules.v1","priceBasis":"bid","stopLossRatio":null,"trailingRatio":null,"trailingStart":null,"reversalSignal":"typed.exit","precedence":["reversal"],"maxQuoteAgeMs":30000},"entryBasis":"provider_final_gross_excluding_fees","feeCashCap":"1","maxLimitSlippageRatio":"0.01","timeInForce":"day","maxReversalAgeMs":30000});
}
fn managed_plan_fixture_observations(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    revision: &str,
    key: &str,
) -> BTreeMap<String, String> {
    use crate::service::data_collections as c;
    let mut ids = BTreeMap::new();
    for (name, node, schema) in [
        (
            "account",
            "exec_account",
            tradeassembly_plugin_sdk::execution_snapshot::ACCOUNT_SCHEMA,
        ),
        (
            "metadata",
            "exec_metadata",
            tradeassembly_plugin_sdk::execution_snapshot::METADATA_SCHEMA,
        ),
        (
            "quote",
            "exec_quote",
            tradeassembly_plugin_sdk::execution_snapshot::QUOTE_SCHEMA,
        ),
    ] {
        let r = c::acquire(
            s,
            c::CollectionCommand {
                scope: scope.clone(),
                capability_graph_revision: revision.into(),
                acquisition_node: node.into(),
                mode: "paper".into(),
                response_schema_ref: schema.into(),
                input: json!({"fixturePhase":"managed_trigger"}),
            },
            &ctx(&format!("{key}-{name}")),
        )
        .unwrap();
        ids.insert(name.into(), r.collection_id);
    }
    ids
}
fn exercise_native_managed_plan(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    revision: &str,
    entry: &str,
    root: &std::path::Path,
    stale: bool,
    actual_math: bool,
) {
    use crate::service::{
        data_collections as collections, order_plans as plans, session_schedules as schedules,
    };
    let post = |path: &str, body: Value| {
        let r = s.handle_http("POST", path, body);
        let mut trace = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("managed-plan-http.trace.jsonl"))
            .unwrap();
        writeln!(trace, "{}", json!({"path":path,"status":r.status,"body":r.body})).unwrap();
        trace.sync_all().unwrap();
        assert_eq!(r.status, 200, "{path}: {}", r.body);
        r.body
    };
    post(
        "/product/strategies/order-plans/reconcile",
        json!({"scope":scope,"planId":entry,"accountMode":"paper","idempotencyKey":"managed-plan-entry-lookup"}),
    );
    let settled = post(
        "/product/strategies/order-plans/settle",
        json!({"scope":scope,"planId":entry,"accountMode":"paper","idempotencyKey":"managed-plan-entry-final"}),
    );
    let registered = post(
        plans::management::REGISTER,
        json!({"scope":scope,"register":{"entryPlanId":entry,"activationId":"fixture-v2-activation"},"accountMode":"paper","idempotencyKey":"managed-plan-register"}),
    );
    let id = registered["position"]["positionId"].as_str().unwrap();
    assert_eq!(registered["position"]["quantity"], "0.4");
    let replacement =
        prepare_dynamic_execution_plan(s, scope, revision, "managed-plan-replacement", "B");
    let selected =
        &replacement["plan"]["content"]["selectionSource"]["command"]["selectionReceipts"];
    assert!(selected["selected"].is_string(), "{replacement}");
    let before_plans = s
        .runtime()
        .storage
        .list_json("compiled_data_plan_revisions_v1")
        .unwrap();
    let normal=s.handle_http("POST","/product/strategies/selection-plans/prepare",json!({"selectionPlan":{"scope":scope,"capabilityGraphRevision":revision,"mode":"paper","subjectInstrumentId":"SPY","selectionReceipts":selected,"sourceCaptures":{}},"accountMode":"paper","idempotencyKey":"removed-normal-subject"}));
    assert_eq!(normal.status, 409, "{}", normal.body);
    let command = json!({"schemaVersion":"tradeassembly.managed_position_plan.v1","scope":scope,"managedPositionId":id,"selectionReceipts":selected,"sourceCaptures":{}});
    let mut wrong = command.clone();
    wrong["managedPositionId"] = json!("nonexistent-owned-position");
    let wrong=s.handle_http("POST","/product/strategies/managed-position-plans/prepare",json!({"managedPositionPlan":wrong,"accountMode":"paper","idempotencyKey":"managed-plan-wrong-position"}));
    assert_eq!(wrong.status, 409, "{}", wrong.body);
    assert_eq!(
        s.runtime()
            .storage
            .list_json("compiled_data_plan_revisions_v1")
            .unwrap(),
        before_plans
    );
    let prepared = post(
        "/product/strategies/managed-position-plans/prepare",
        json!({"managedPositionPlan":command,"accountMode":"paper","idempotencyKey":"managed-plan-prepare"}),
    );
    assert_eq!(
        prepared["plan"]["content"]["selectionSource"]["managedPositionId"],
        id
    );
    let plan = prepared["plan"]["revisionId"].as_str().unwrap();
    // Preserve the normal finite routes and exact refusal bodies. Only the
    // actual-math lane moves these nonfinancial checks out of the final budget.
    let check_entry_refusals_and_pause = |receipt: &Value| {
        let ids = managed_plan_fixture_observations(s, scope, revision, "managed-plan-entry-refusal");
        let noentry=s.handle_http("POST","/product/strategies/order-plans/prepare",json!({"orderPlan":{"scope":scope,"capabilityGraphRevision":revision,"mode":"paper","reserveCash":true,"signalReceiptSha256":receipt["receiptSha256"],"accountCollectionId":ids["account"],"metadataCollectionId":ids["metadata"],"quoteCollectionId":ids["quote"]},"accountMode":"paper","idempotencyKey":"managed-plan-cannot-enter"}));
        assert_eq!(noentry.status, 409, "{}", noentry.body);
        assert_eq!(
            noentry.body["error"]["code"],
            "order_plan_managed_exit_evidence_cannot_enter"
        );
        let select=s.handle_http("POST","/product/strategies/order-plans/select",json!({"scope":scope,"selection":{"activationId":"fixture-v2-activation","planRevisionId":plan},"accountMode":"paper","idempotencyKey":"managed-plan-cannot-select"}));
        assert_eq!(select.status, 409, "{}", select.body);
        post(
            "/product/strategy-execution-activations/control",
            json!({"activationId":"fixture-v2-activation","action":"pause_entries","accountMode":"paper","idempotencyKey":"managed-plan-pause-entries"}),
        );
        (noentry, select)
    };
    let prepare_execution_schedule = || {
        let calendar = collections::acquire(
            s,
            collections::CollectionCommand {
                scope: scope.clone(),
                capability_graph_revision: revision.into(),
                acquisition_node: "calendar".into(),
                mode: "paper".into(),
                response_schema_ref:
                    tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA.into(),
                input: json!({}),
            },
            &ctx("managed-plan-calendar"),
        )
        .unwrap();
        let now = s.runtime().clock.now_ms();
        let date = chrono::DateTime::from_timestamp_millis(now)
            .unwrap()
            .date_naive()
            .to_string();
        post(
            "/product/strategies/session-schedules/prepare",
            json!({"schedule":schedules::ScheduleCommand{scope:scope.clone(),capability_graph_revision:revision.into(),mode:"paper".into(),trigger_ref:"preopen".into(),session_date:date,calendar_collection_id:calendar.collection_id.clone(),execution_calendar_collection_id:calendar.collection_id,knowledge_cutoff_ms:now},"idempotencyKey":"managed-plan-schedule"}),
        )
    };
    let mut entry_refusals = None;
    let mut execution_schedule = None;
    let mut checkpoint = Value::Null;
    let mut receipts = Vec::new();
    // Generic parity frames remain completed one-ms observations. The actual
    // math lane declares a prospective SYNTHETIC24h-grid phase before warmup;
    // this is not an exchange/UTC calendar or a user daily-market strategy.
    std::thread::sleep(std::time::Duration::from_millis(4));
    let scheduled_at = s.runtime().clock.trusted_now_ms().unwrap();
    let end = if actual_math {
        scheduled_at.checked_add(120000).unwrap()
    } else {
        (scheduled_at - 2) & !1
    };
    let count = if actual_math { 4 } else { 2 };
    let interval = if actual_math { 86400000 } else { 1 };
    let mut frame_timing = Vec::new();
    if actual_math {
        let pinned:data_plans::StoredDataPlan=serde_json::from_value(prepared["plan"].clone()).unwrap();
        let snapshot=&pinned.content.plan.request.nodes.iter().find(|node|node.graph_node_id=="exec_signal").unwrap().parameters["snapshot"];
        assert_eq!(snapshot["fixtureFallingDaily"],true);
        assert_eq!(snapshot["interval"],86400000);
        let original_observed=snapshot["observedAtMs"].as_i64().unwrap();
        // The exact pinned source definition then yields103,102,101,100 at
        // final-3days,...,final. No price or timestamp is rewritten after warmup.
        assert!((0..interval).contains(&(end-original_observed)),"synthetic phase moved beyond the pinned oracle's domain");
        eprintln!("synthetic 24h math phase selected at {scheduled_at}: historical ends {:?}; final completed endpoint {end}; source observedAt {original_observed}",[end-3*interval,end-2*interval,end-interval]);
        write_screen_interface_evidence("managed_plan","synthetic-daily-grid-plan.json",&json!({"selectedAtMs":scheduled_at,"intervalMs":interval,"originalPublishedSnapshot":snapshot,"warmupEventEndsMs":[end-3*interval,end-2*interval,end-interval],"finalEventEndMs":end,"unchangedMaxReversalAgeMs":30000,"exchangeCalendarAlignmentClaimed":false,"userDailyBtcStrategyQualified":false,"qualification":"prospective phase of authored SPY numeric-series functional fixture; literal24h intervals, no market availability claim"}));
    }
    assert!(
        end >= registered["position"]["entryTime"]["knownAtMs"]
            .as_i64()
            .unwrap()
    );
    for index in 0..count {
        let restored =
            crate::service::TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone())
                .for_authenticated_invocation(
                    &s.invocation_owner().unwrap().issuer,
                    &s.invocation_owner().unwrap().subject,
                    None,
                    None,
                );
        let t = if actual_math {
            end - (3 - index) * interval
        } else {
            end + index
        };
        let mut waits = Vec::new();
        if actual_math && index == 3 {
            loop {
                let now=restored.runtime().clock.trusted_now_ms().unwrap();
                assert!(now < end.checked_add(30000).unwrap(),"cold warmup overran the final synthetic bar's unchanged freshness deadline");
                if now >= end { break; }
                let wait_ms=(end-now).min(45000);
                eprintln!("synthetic 24h math waiting at {now} for completed endpoint {end}: sleep {wait_ms} ms (maximum 45000)");
                waits.push(json!({"waitStartedAtMs":now,"targetEventEndMs":end,"sleepMs":wait_ms}));
                write_screen_interface_evidence("managed_plan",&format!("synthetic-daily-grid-wait-{}.json",waits.len()),&json!({"selectedAtMs":scheduled_at,"finalEventEndMs":end,"waits":waits}));
                std::thread::sleep(std::time::Duration::from_millis(u64::try_from(wait_ms).unwrap()));
            }
            // Managed-plan currentness checks account health before invoking the
            // calculation. These are genuine reads after the completed-bar wait,
            // never a timestamp rewrite or a freshness exception.
            let preparation_started = restored.runtime().clock.trusted_now_ms().unwrap();
            refresh_managed_math_account_health(s, root, "before-final");
            execution_schedule = Some(prepare_execution_schedule());
            let preparation_finished = restored.runtime().clock.trusted_now_ms().unwrap();
            eprintln!("synthetic 24h final preparation {preparation_started}..{preparation_finished}: both health reads and execution calendar ready; final endpoint {end}");
            write_screen_interface_evidence("managed_plan", "synthetic-daily-grid-final-preparation.json", &json!({"finalEventEndMs":end,"preparationStartedAtMs":preparation_started,"preparationFinishedAtMs":preparation_finished,"healthPhase":"before-final","providerHealthReadsPerSource":3,"executionSchedule":execution_schedule,"financialCloseAttempted":false}));
            assert!(preparation_finished < end.checked_add(30000).unwrap(), "real health/calendar preparation exhausted the unchanged final freshness budget");
        }
        let requested_at=restored.runtime().clock.trusted_now_ms().unwrap();
        assert!(t<=requested_at,"fixture cannot request a future event window");
        if actual_math {eprintln!("synthetic 24h math frame {index} completed window {}..{t}; actual request at {requested_at}",t-interval);}
        let r=restored.handle_http("POST","/product/strategies/data-runs/step",json!({"step":{"runId":"managed-position-reversal","planRevisionId":plan,"window":{"eventStartMs":t-interval,"eventEndMs":t,"asOfMs":requested_at},"expectedCheckpointSha256":checkpoint},"idempotencyKey":format!("managed-plan-frame-{index}")}));
        let processed_at=restored.runtime().clock.trusted_now_ms().unwrap();
        if actual_math {
            frame_timing.push(json!({"frame":index,"eventStartMs":t-interval,"eventEndMs":t,"requestedAtMs":requested_at,"processedAtMs":processed_at,"responseStatus":r.status,"waits":waits}));
            eprintln!("synthetic 24h math frame {index} response at {processed_at}; event age {} ms",processed_at-t);
            write_screen_interface_evidence("managed_plan",&format!("synthetic-daily-grid-frame-{index}.json"),&json!({"selectedAtMs":scheduled_at,"finalEventEndMs":end,"frames":frame_timing}));
        }
        assert_eq!(r.status, 200, "{}", r.body);
        if actual_math && index==3 {assert!(processed_at-end<30000,"final actual calculation exceeded the unchanged reversal freshness deadline");}
        assert_eq!(
            r.body["receipt"]["signals"][0]["entry"],
            json!({"status":"ready","value":false})
        );
        assert_eq!(
            r.body["receipt"]["signals"][0]["exit"],
            if if actual_math { index < 3 } else { index == 0 } {
                json!({"status":"warming_up"})
            } else {
                json!({"status":"ready","value":true})
            }
        );
        checkpoint = r.body["receipt"]["checkpoint"]["checkpointSha256"].clone();
        if actual_math {
            assert_managed_math_frame(&r.body["receipt"], index);
        }
        receipts.push(r.body["receipt"].clone());
        if actual_math && index == 0 {
            refresh_managed_math_account_health(s, root, "before-entry-refusal");
            entry_refusals = Some(check_entry_refusals_and_pause(receipts.last().unwrap()));
        }
    }
    if actual_math {
        // This normal graph check and the collection acquisitions below must
        // retain the exact saved selection after genuine provider refreshes.
        let epoch=chrono::DateTime::from_timestamp_millis(s.runtime().clock.now_ms()).unwrap().to_rfc3339();
        let graph=s.handle_http("POST","/capability-graph-revisions/currentness",json!({"revisionId":revision,"evaluationEpoch":epoch}));
        assert_eq!(graph.status,200,"{}",graph.body);
        assert_eq!(graph.body["current"],true,"{}",graph.body);
    }
    if !actual_math {
        entry_refusals = Some(check_entry_refusals_and_pause(receipts.last().unwrap()));
        execution_schedule = Some(prepare_execution_schedule());
    }
    let (noentry, select) = entry_refusals.expect("entry refusals and entry pause completed");
    let schedule = execution_schedule.expect("fresh execution calendar prepared before close");
    let advance = |receipt: &Value, key: &str| {
        let ids = managed_plan_fixture_observations(s, scope, revision, key);
        s.handle_http("POST",plans::management::ADVANCE,json!({"scope":scope,"advance":{"executionOccurrenceId":schedule["schedule"]["occurrence"]["occurrenceId"],"positionId":id,"accountCollectionId":ids["account"],"metadataCollectionId":ids["metadata"],"quoteCollectionId":ids["quote"],"reversalSignalReceiptSha256":receipt["receiptSha256"]},"accountMode":"paper","idempotencyKey":key}))
    };
    let submits = fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap();
    let unready = if actual_math {
        None
    } else {
        Some(advance(&receipts[0], "managed-plan-unready"))
    };
    if let Some(unready) = &unready {
        assert_eq!(unready.status, 409, "{}", unready.body);
        assert_eq!(unready.body["error"]["code"], "managed_reversal_not_ready");
    }
    if stale {
        // Dedicated explicit negative: wait for real expiry, then obtain fresh quotes.
        std::thread::sleep(std::time::Duration::from_millis(30001));
        let refused = advance(&receipts[receipts.len() - 1], "managed-plan-stale");
        assert_eq!(refused.status, 409, "{}", refused.body);
        assert_eq!(
            refused.body["error"]["code"],
            "managed_reversal_not_current"
        );
        assert_eq!(
            fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap(),
            submits
        );
        write_screen_interface_evidence(
            "managed_plan",
            "generic-reversal-stale-refused.json",
            &refused.body,
        );
        return;
    }
    let advanced = advance(&receipts[receipts.len() - 1], "managed-plan-ready");
    assert_eq!(advanced.status, 200, "{}", advanced.body);
    let decision = &advanced.body["decision"];
    assert!(decision["planId"].is_string(), "{}", advanced.body);
    let close = decision["planId"].as_str().unwrap();
    assert_eq!(
        fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap(),
        submits,
        "preparation must not submit"
    );
    let provider=s.runtime().plugins.get_instance("fixture.broker.v2").unwrap().unwrap();
    if let Some(scenario)=provider["configuration"]["scenario"].as_str().filter(|v|v.starts_with("managed_plan_close_")){
        exercise_native_owned_close_outcome(s,scope,id,entry,decision,root,scenario);
        return;
    }
    let submitted = post(
        "/product/strategies/order-plans/submit",
        json!({"scope":scope,"submission":decision["submission"],"accountMode":"paper","idempotencyKey":"managed-plan-close-submit"}),
    );
    post(
        "/product/strategies/order-plans/reconcile",
        json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-plan-close-lookup"}),
    );
    let final_posting = post(
        "/product/strategies/order-plans/settle",
        json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-plan-close-final"}),
    );
    let inspected = s.handle_http(
        "GET",
        &format!("/product/strategies/positions/{id}"),
        json!({}),
    );
    assert_eq!(inspected.status, 200, "{}", inspected.body);
    assert_eq!(inspected.body["position"]["closedQuantity"], "0.4");
    let calls = fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap();
    assert_eq!(calls.lines().filter(|v| *v == "submit").count(), 2);
    assert_eq!(calls.lines().filter(|v| *v == "lookup").count(), 2);
    write_screen_interface_evidence(
        "managed_plan",
        "generic-reversal-owned-removed-member.json",
        &json!({"entrySettlement":settled,"position":registered,"replacementSelection":replacement,"normalRemovedSubjectRefusal":normal.body,"wrongPositionRefusal":wrong.body,"managedPlan":prepared,"frames":receipts,"entryRefusal":noentry.body,"selectionRefusal":select.body,"unreadyRefusal":unready.map(|v|v.body),"advanced":advanced.body,"submitted":submitted,"finalPosting":final_posting,"journal":inspected.body,"syntheticSubmits":2,"syntheticLookups":2,"realBrokerCalls":0,"actualMaTsiMathQualified":actual_math,"synthetic24hFrameTiming":frame_timing,"exchangeCalendarAlignmentQualified":false,"userDailyBtcStrategyQualified":false,"automaticReversalQualified":false,"crossPositionReceiptLineageNegativeQualified":false,"actualPackSha256":if actual_math{Some(MANAGED_MATH_PACK_SHA)}else{None},"qualification":if actual_math{"actual native pinned-pack EMA2/TSI2,2 explicit fixture; four daily frames, independent finite Core close C5; synthetic source/provider/mock Warden/traced sandbox; no auto/remote/process restart claim"}else{"actual Core local API/native authored generic reversal source; manual finite management, independent normal close C5; no actual MA/TSI/process restart/remote producer qualification"}}),
    );
}
#[test]
#[ignore = "explicit native generic managed-plan removed-membership/reversal/C5/final-settlement fixture"]
fn native_managed_plan_generic_reversal_survives_removed_membership() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some("financial_screen_managed_plan"));
}
#[test]
#[ignore = "explicit native generic managed-plan stale receipt negative; includes 30-second real expiry"]
fn native_managed_plan_generic_reversal_stale_refuses() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some("financial_screen_managed_plan_stale"));
}

const MANAGED_MATH_PACK_SHA: &str =
    "91e9f96dff7cfa8c567a7b86a2dc1a340ac14fd912d71b94df72bd67b7b95d06";
fn install_managed_math_pack(s: &crate::service::TradeAssemblyService) -> InstalledPluginPackage {
    let path = std::path::PathBuf::from(
        std::env::var_os("MANAGED_MATH_TEST_PACKAGE")
            .expect("explicit immutable0.69.0 pack archive required"),
    );
    assert_eq!(
        sha256(&fs::read(&path).unwrap()),
        MANAGED_MATH_PACK_SHA,
        "do not substitute a moving pack"
    );
    let p = s
        .runtime()
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: MANAGED_MATH_PACK_SHA.into(),
            manifest_sha256: "74192f68757593a8f3d7f7b81a7df1ccddef788c2866333a94666b2c3197c0c4"
                .into(),
            offline: true,
        })
        .unwrap();
    assert_eq!(p.version, "0.69.0");
    assert_eq!(p.plugin_ref, "tradeassembly.default-indicators");
    s.runtime().plugins.install_manifest(&p.plugin_ref,json!({"pluginRef":p.plugin_ref,"manifest":p.manifest,"manifestDigest":p.manifest_sha256,"package":{"packageSha256":p.package_sha256},"trust":{"level":"test"}}),&ctx("managed-real-pack-install")).unwrap();
    s.runtime().plugins.put_instance("managed-real-calculation",json!({"instanceRef":"managed-real-calculation","pluginRef":p.plugin_ref,"providerRef":p.plugin_ref,"enabled":true,"activePackageSha256":p.package_sha256,"configuration":{}}),&ctx("managed-real-pack-instance")).unwrap();
    p
}
fn configure_managed_math_native_spec(
    s: &crate::service::TradeAssemblyService,
    spec: &mut Value,
    bindings: &mut Value,
    pack: &InstalledPluginPackage,
) {
    use crate::data_pipeline::PortBinding;
    use crate::selection_strategy::*;
    let config = &mut spec["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY];
    let mut template: SelectionStrategy = serde_json::from_value(config.clone()).unwrap();
    template.nodes[0].context = ContextTemplate::Subject {
        interval_ms: Some(86400000),
    };
    for (node, parameters) in [
        ("managed_ema", json!({"period":2})),
        ("managed_tsi", json!({"firstPeriod":2,"secondPeriod":2})),
    ] {
        template.nodes.push(NodeTemplate {
            graph_node_id: node.into(),
            parameters,
            context: ContextTemplate::Subject {
                interval_ms: Some(86400000),
            },
            input_contexts: BTreeMap::new(),
        });
        template.bindings.push(PortBinding {
            producer_node: "exec_signal".into(),
            output_port: "value".into(),
            consumer_node: node.into(),
            input_port: "source".into(),
        });
    }
    let output = |node: &str, sample| OperandTemplate::Output {
        node: node.into(),
        port: "value".into(),
        instrument: InstrumentRef::Subject,
        sample,
    };
    template.exit = RuleTemplate::All {
        rules: vec![
            RuleTemplate::Compare {
                left: output("managed_ema", crate::data_strategy::Sample::Current),
                op: crate::data_strategy::Comparison::Less,
                right: output("managed_ema", crate::data_strategy::Sample::Previous),
            },
            RuleTemplate::Compare {
                left: output("managed_tsi", crate::data_strategy::Sample::Current),
                op: crate::data_strategy::Comparison::Less,
                right: OperandTemplate::Number { value: 0.0 },
            },
        ],
    };
    *config = serde_json::to_value(template).unwrap();
    for (node, operation) in [
        ("managed_ema", "indicator.ema"),
        ("managed_tsi", "indicator.tsi"),
    ] {
        let mut r = spec["capability_requirements"]["required"][0].clone();
        r["requirement_id"] = json!(node);
        let capability = pack.manifest["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|op| op["id"] == operation)
            .unwrap()["capability"]
            .as_str()
            .unwrap();
        r["capability"] = json!(capability);
        r["stage_refs"] = json!(["evaluate"]);
        r["substep_refs"] = json!([]);
        r["dependency_refs"] = json!([]);
        // The package advertises schemas and determinism in legacy traits.
        // Series/time compatibility remains mandatory in its pinned V3 ports
        // and in each actual evaluation batch, not an undeclared legacy trait.
        r["constraints"] = json!({"schema_refs":["schema://tradeassembly/number-series@1"],"data_shapes":[],"fields":[],"input_paths":[],"operations":[operation],"deterministic":true,"replayable":true});
        spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .push(r);
        bindings[node] = json!({"pluginInstanceRef":"managed-real-calculation","pluginRef":pack.plugin_ref,"operationId":operation});
        s.runtime().storage.put_json("entitlement_grants",capability,json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("managed-real-math-grants")).unwrap();
    }
}
fn assert_managed_math_frame(receipt: &Value, index: i64) {
    let point =
        |node: &str| &receipt["outputs"][node][0]["outputs"]["value"]["points"][0]["result"];
    // Independent oracle for authored prices103,102,101,100:
    // EMA2 seeds102.5, then alpha2/3 gives101.5 and100.5.
    // All changes=-1: both seeded EMA2 cascades yield signed=-1, abs=1;
    // TSI2,2 first becomes ready on sample4 and equals100*(-1)/1=-100.
    assert_eq!(point("exec_signal")["value"], json!(103 - index));
    if index == 0 {
        assert_eq!(point("managed_ema")["state"], "warming_up");
    } else {
        let expected = 103.5 - index as f64;
        let actual = point("managed_ema")["value"].as_f64().unwrap();
        assert!(
            (actual - expected).abs() < 1e-12,
            "EMA sample{index}: {actual}"
        );
    }
    if index < 3 {
        assert_eq!(point("managed_tsi")["state"], "warming_up");
    } else {
        let actual = point("managed_tsi")["value"].as_f64().unwrap();
        assert!((actual + 100.0).abs() < 1e-12, "TSI sample{index}: {actual}");
    }
}
#[test]
#[ignore = "explicit actual immutable-pack EMA2/TSI2,2 native managed reversal fixture; requires MANAGED_MATH_TEST_PACKAGE"]
fn native_managed_plan_real_pack_ema_tsi_reversal_survives_removed_membership() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some("financial_screen_managed_plan_math"));
}

// Actual native health reads for BOTH account-bound sources. The facts-only
// package flag installs its schema/operation/store before graph preparation.
fn refresh_managed_math_account_health(s:&crate::service::TradeAssemblyService,root:&std::path::Path,phase:&str){
    let expected=match phase{"bootstrap"=>1,"before-entry-refusal"=>2,"before-final"=>3,_=>panic!("unknown native math health phase")};
    let mut evidence=Vec::new();
    for (instance,store) in [("fixture.broker.v2","synthetic-broker"),("fixture.execution.facts","facts-health")]{
        let before=s.runtime().plugins.get_instance(instance).unwrap().unwrap();
        let binding=crate::live_execution_checks::live_health_evidence_binding(&before);
        let refreshed=s.handle_http("POST",&format!("/plugins/instances/{instance}/health:refresh"),json!({"idempotencyKey":format!("managed-math-health-{phase}-{instance}")}));
        assert_eq!(refreshed.status,200,"{}",refreshed.body);
        let after=s.runtime().plugins.get_instance(instance).unwrap().unwrap();
        for field in ["instanceRef","pluginRef","activePackageSha256","configuration","credentialRef","credentialRevision","accountRef","accountMode","lifecycleRevision"]{
            assert_eq!(before[field],after[field],"health changed {instance} identity field {field}");
        }
        assert_eq!(after["health"]["binding"],binding);
        assert_eq!(after["health"]["state"],"ready");
        assert_eq!(after["health"]["connectivityChecked"],true);
        assert_eq!(after["health"]["account"]["id"],"fixture.paper.provider");
        assert_eq!(after["health"]["account"]["mode"],"paper");
        assert_eq!(after["credentialStatus"]["revisionRef"],"credential-generation:1");
        assert_eq!(after["health"]["revision"],expected);
        assert!(after["health"]["checkedAtMs"].as_i64().unwrap()>=before["health"]["checkedAtMs"].as_i64().unwrap());
        if phase!="bootstrap"{
            assert_eq!(before["credentialStatus"]["revisionRef"],after["credentialStatus"]["revisionRef"],"post-bootstrap credential posture changed");
            assert_eq!(before["health"]["binding"],after["health"]["binding"]);
        }
        assert_eq!(fs::read_to_string(root.join(store).join("health.log")).unwrap().lines().count(),expected);
        evidence.push(json!({"instanceRef":instance,"normalHealthRefresh":refreshed.body,"sourceBindingPreserved":true,"credentialRevisionRef":after["credentialStatus"]["revisionRef"],"providerHealthReads":expected}));
    }
    write_screen_interface_evidence("managed_plan",&format!("provider-health-{phase}.json"),&json!({"phase":phase,"sources":evidence,"manuallyRedatedObservations":0,"realProviderCalls":0,"qualification":"actual normal account.health route/native authored synthetic paper observations for broker and facts; separate isolated provider stores; no freshness relaxation"}));
}
