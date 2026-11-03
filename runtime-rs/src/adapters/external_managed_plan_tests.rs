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
    plan_lane: (bool, bool),
) {
    let (actual_math, metadata_round_only) = plan_lane;
    use crate::service::{
        data_collections as collections, order_plans as plans, session_schedules as schedules,
    };
    let provisional=s.runtime().plugins.get_instance("fixture.broker.v2").unwrap().unwrap()["configuration"]["nonAtomicSettlement"]==true;
    let math_pack_sha = actual_math.then(|| s.runtime().plugins.get_instance("managed-real-calculation").unwrap().unwrap()["activePackageSha256"].as_str().unwrap().to_owned());
    let minute_math = math_pack_sha.as_deref() == Some(MANAGED_MINUTE_PACK_SHA);
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
    let entry_cash_before=s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap();
    let settled = post(
        "/product/strategies/order-plans/settle",
        json!({"scope":scope,"planId":entry,"accountMode":"paper","idempotencyKey":"managed-plan-entry-final"}),
    );
    if provisional {
        assert_eq!(settled["cashReconciled"],false,"{settled}");assert_eq!(settled["releasedHold"],"0");
        assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(),entry_cash_before);
        let original=plans::submission::source(s,entry).unwrap().state().unwrap().unwrap();assert_eq!(original.phase,plans::submission::Phase::Unknown);assert_eq!(original.retained_cash.to_string(),"41");
        let foreign=s.for_authenticated_invocation("fixture","foreign",None,None);let count=fs::read_to_string(root.join("synthetic-broker/observation.log")).unwrap();
        let denied=foreign.call_mcp_tool("tradeassembly.strategy.order_plan.settle",json!({"scope":scope,"planId":entry,"accountMode":"paper","idempotencyKey":"foreign-provisional-observation"}));assert_eq!(denied["isError"],true,"{denied}");assert_eq!(fs::read_to_string(root.join("synthetic-broker/observation.log")).unwrap(),count);
    }
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
        if s.runtime().plugins.get_instance("fixture.broker.v2").unwrap().unwrap()["configuration"]["scenario"] == "managed_plan_c5_profile" {
            return post(crate::service::management_windows::PREPARE,
                json!({"managementWindow":{"scope":scope,"positionId":id,"triggerRef":"preopen","executionCalendarCollectionId":calendar.collection_id},"accountMode":"paper","idempotencyKey":"c5-profile-owned-window"}));
        }
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
    if metadata_round_only {
        assert!(!actual_math && !stale && !provisional);
        // A prior successful plan is not a grant for another validation round.
        let replay_body = json!({"managedPositionPlan":command,"accountMode":"paper","idempotencyKey":"managed-plan-prepare"});
        let replay = post("/product/strategies/managed-position-plans/prepare", replay_body.clone());
        assert_eq!(replay["plan"], prepared["plan"]);
        let graph = s.runtime().storage.get_json("capability_graph_revisions", revision).unwrap().unwrap();
        let instance = graph["graph"]["nodes"].as_array().unwrap().iter()
            .find(|n| n["nodeId"]=="exec_signal").unwrap()["selected"]["pluginInstanceRef"]
            .as_str().unwrap().to_owned();
        let original = s.runtime().plugins.get_instance(&instance).unwrap().unwrap();
        let before = s.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap();
        let mut revoked = original.clone();
        revoked["enabled"] = json!(false);
        s.runtime().plugins.put_instance(&instance, revoked, &ctx("managed-round-revoke")).unwrap();
        let refused = s.handle_http("POST", "/product/strategies/managed-position-plans/prepare", replay_body.clone());
        assert!(refused.status>=400, "revoked source replay was accepted: {}", refused.body);
        assert_eq!(s.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap(), before);
        s.runtime().plugins.put_instance(&instance, original, &ctx("managed-round-restore-fixture")).unwrap();
        let restored = post("/product/strategies/managed-position-plans/prepare", replay_body);
        assert_eq!(restored["plan"], prepared["plan"]);
        write_screen_interface_evidence("managed_plan", "metadata-round-replay-revocation.json",
            &json!({"original":prepared["plan"],"replay":replay["plan"],"revokedRefusalStatus":refused.status,
                "revokedRefusal":refused.body,"restored":restored["plan"],"compiledPlanRowsUnchangedOnRefusal":true,
                "scope":"native authored generic fixture; no real provider or account"}));
        let schedule = prepare_execution_schedule();
        let occurrence = schedule["schedule"]["occurrence"]["occurrenceId"].as_str().unwrap();
        let window = || plans::management::fixture_execution_window(s, entry, occurrence);
        let original_window = window().unwrap();
        assert_eq!(window().unwrap(), original_window);
        let managed_window = crate::service::management_windows::prepare(s,
            crate::service::management_windows::Command { scope:scope.clone(),position_id:id.into(),
                trigger_ref:"preopen".into(),execution_calendar_collection_id:schedules::current(s, occurrence).unwrap().command.execution_calendar_collection_id },
            &ctx("window-round-owned-position")).unwrap();
        let owned_window = || crate::service::management_windows::current(s, &managed_window.window_id);
        let original_owned_window = owned_window().unwrap();
        assert_eq!(owned_window().unwrap(), original_owned_window);
        let calendar_instance = graph["graph"]["nodes"].as_array().unwrap().iter()
            .find(|n| n["nodeId"] == "calendar").unwrap()["selected"]["pluginInstanceRef"]
            .as_str().unwrap();
        let calendar_original = s.runtime().plugins.get_instance(calendar_instance).unwrap().unwrap();
        let before_effects = s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap();
        let before_holds = s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap();
        let mut refusals = Vec::new();
        for mutation in ["revocation", "configuration", "credential_revision"] {
            let mut changed = calendar_original.clone();
            match mutation {
                "revocation" => changed["enabled"] = json!(false),
                "configuration" => changed["configuration"]["fixtureWindowChanged"] = json!(true),
                "credential_revision" => changed["credentialRevision"] = json!(2),
                _ => unreachable!(),
            }
            s.runtime().plugins.put_instance(calendar_instance, changed, &ctx("window-round-invalidate")).unwrap();
            let error = window().expect_err("previous window proof must not authorize changed source");
            let owned_error = owned_window().expect_err("owned window must independently reject changed source");
            refusals.push(json!({"mutation":mutation,"error":error,"ownedWindowError":owned_error}));
            assert_eq!(s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap(), before_effects);
            assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(), before_holds);
            s.runtime().plugins.put_instance(calendar_instance, calendar_original.clone(), &ctx("window-round-restore")).unwrap();
            assert_eq!(window().unwrap(), original_window);
            assert_eq!(owned_window().unwrap(), original_owned_window);
        }
        let installed = s.runtime().plugin_packages.get(calendar_original["activePackageSha256"].as_str().unwrap()).unwrap().unwrap();
        let executable = std::path::Path::new(&installed.executable_path);
        assert!(executable.canonicalize().unwrap().starts_with(root.canonicalize().unwrap()), "only this test's private installed fixture may be changed");
        let original_bytes = fs::read(executable).unwrap();
        let mut changed_bytes = original_bytes.clone(); changed_bytes.push(0);
        fs::write(executable, changed_bytes).unwrap();
        let artifact_refusal = owned_window();
        fs::write(executable, original_bytes).unwrap();
        assert!(artifact_refusal.is_err(), "new validation round must reject changed artifact bytes");
        assert_eq!(owned_window().unwrap(), original_owned_window);
        let foreign = s.for_authenticated_invocation("fixture", "window-foreign-owner", None, None);
        assert!(plans::management::fixture_execution_window(&foreign, entry, occurrence).is_err());
        assert!(crate::service::management_windows::current(&foreign, &managed_window.window_id).is_err());
        assert_eq!(s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap(), before_effects);
        assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(), before_holds);
        write_screen_interface_evidence("managed_plan", "window-round-invalidation.json",
            &json!({"window":original_window.0,"deadline":original_window.1,"refusals":refusals,
                "restoredExactWindow":true,"ownedWindow":managed_window,"tamperedArtifactRefusal":artifact_refusal.unwrap_err(),
                "financialRowsUnchanged":true,"foreignOwnerRefused":true,
                "scope":"synthetic native window validation; no real account or provider"}));
        return; // This dedicated regression does not qualify a close lifecycle.
    }
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
    let mut entry_refusals = None;
    let mut execution_schedule = None;
    let mut checkpoint = Value::Null;
    let mut receipts = Vec::new();
    // Generic parity frames remain completed one-ms observations. The actual
    // math lane declares a prospective SYNTHETIC24h-grid phase before warmup;
    // this is not an exchange/UTC calendar or a user daily-market strategy.
    std::thread::sleep(std::time::Duration::from_millis(4));
    let scheduled_at = s.runtime().clock.trusted_now_ms().unwrap();
    let end = if minute_math {
        scheduled_at.checked_add(120000).unwrap().div_euclid(60000).checked_add(1).unwrap().checked_mul(60000).unwrap()
    } else if actual_math {
        scheduled_at.checked_add(120000).unwrap()
    } else {
        (scheduled_at - 2) & !1
    };
    let count = if actual_math { 4 } else if provisional { 3 } else { 2 };
    let mut provisional_fresh_end = None;
    let interval = if minute_math { 60000 } else if actual_math { 86400000 } else { 1 };
    let grid_label = if minute_math { "synthetic-minute-grid" } else { "synthetic-daily-grid" };
    let mut math_final_value = 100;
    let mut frame_timing = Vec::new();
    if actual_math {
        let pinned:data_plans::StoredDataPlan=serde_json::from_value(prepared["plan"].clone()).unwrap();
        let snapshot=&pinned.content.plan.request.nodes.iter().find(|node|node.graph_node_id=="exec_signal").unwrap().parameters["snapshot"];
        assert_eq!(snapshot[if minute_math {"fixtureFallingMinute"} else {"fixtureFallingDaily"}],true);
        assert_eq!(snapshot["interval"],interval);
        let original_observed=snapshot["observedAtMs"].as_i64().unwrap();
        // The exact pinned source definition then yields103,102,101,100 at
        // final-3days,...,final. No price or timestamp is rewritten after warmup.
        if minute_math {
            let offset = (end-original_observed).div_euclid(interval);
            assert!((0..=8).contains(&offset),"minute fixture preparation exceeded its bounded declared oracle horizon");
            math_final_value -= offset;
        } else {
            assert!((0..interval).contains(&(end-original_observed)),"synthetic phase moved beyond the pinned oracle's domain");
        }
        eprintln!("synthetic declared-interval math phase selected at {scheduled_at}: historical ends {:?}; final completed endpoint {end}; source observedAt {original_observed}",[end-3*interval,end-2*interval,end-interval]);
        write_screen_interface_evidence("managed_plan",&format!("{grid_label}-plan.json"),&json!({"selectedAtMs":scheduled_at,"intervalMs":interval,"originalPublishedSnapshot":snapshot,"expectedFinalValue":math_final_value,"warmupEventEndsMs":[end-3*interval,end-2*interval,end-interval],"finalEventEndMs":end,"unchangedMaxReversalAgeMs":30000,"exchangeCalendarAlignmentClaimed":false,"userDailyBtcStrategyQualified":false,"qualification":"prospective phase of authored SPY numeric-series functional fixture at declared interval; no market availability claim"}));
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
        if provisional && index == 1 {
            refresh_managed_math_account_health(s,root,"before-final");
            execution_schedule=Some(prepare_execution_schedule());
            // Start a new, contiguous bounded run after the negative preflight.
            // No gap is forward-filled and no old receipt is redated.
            checkpoint = Value::Null;
            provisional_fresh_end = Some((s.runtime().clock.trusted_now_ms().unwrap() - 2) & !1);
        }
        let t = if provisional && index > 0 {
            provisional_fresh_end.unwrap() + index - 1
        } else if actual_math {
            end - (3 - index) * interval
        } else {
            end + index
        };
        let mut waits = Vec::new();
        // The daily fixture's three warmup bars are historical. With real
        // minute aliases, an intermediate warmup bar can also be prospective.
        // Wait for its actual completion rather than asking the host to accept
        // future data or changing the immutable source/plan after publication.
        if minute_math && index < 3 {
            loop {
                let now = restored.runtime().clock.trusted_now_ms().unwrap();
                assert!(now < end.checked_add(30000).unwrap(), "minute warmup exceeded the final frame deadline");
                if now >= t { break; }
                let wait_ms = (t-now).min(45000);
                waits.push(json!({"waitStartedAtMs":now,"targetEventEndMs":t,"sleepMs":wait_ms}));
                write_screen_interface_evidence("managed_plan", &format!("{grid_label}-warmup-{index}-wait-{}.json", waits.len()), &json!({"finalEventEndMs":end,"waits":waits}));
                std::thread::sleep(std::time::Duration::from_millis(u64::try_from(wait_ms).unwrap()));
            }
        }
        if actual_math && index == 3 {
            loop {
                let now=restored.runtime().clock.trusted_now_ms().unwrap();
                assert!(now < end.checked_add(30000).unwrap(),"cold warmup overran the final synthetic bar's unchanged freshness deadline");
                if now >= end { break; }
                let wait_ms=(end-now).min(45000);
                eprintln!("synthetic declared-interval math waiting at {now} for completed endpoint {end}: sleep {wait_ms} ms (maximum 45000)");
                waits.push(json!({"waitStartedAtMs":now,"targetEventEndMs":end,"sleepMs":wait_ms}));
                write_screen_interface_evidence("managed_plan",&format!("{grid_label}-wait-{}.json",waits.len()),&json!({"selectedAtMs":scheduled_at,"finalEventEndMs":end,"waits":waits}));
                std::thread::sleep(std::time::Duration::from_millis(u64::try_from(wait_ms).unwrap()));
            }
            // Managed-plan currentness checks account health before invoking the
            // calculation. These are genuine reads after the completed-bar wait,
            // never a timestamp rewrite or a freshness exception.
            let preparation_started = restored.runtime().clock.trusted_now_ms().unwrap();
            refresh_managed_math_account_health(s, root, "before-final");
            execution_schedule = Some(prepare_execution_schedule());
            let preparation_finished = restored.runtime().clock.trusted_now_ms().unwrap();
            eprintln!("synthetic declared-interval final preparation {preparation_started}..{preparation_finished}: both health reads and execution calendar ready; final endpoint {end}");
            write_screen_interface_evidence("managed_plan", &format!("{grid_label}-final-preparation.json"), &json!({"finalEventEndMs":end,"preparationStartedAtMs":preparation_started,"preparationFinishedAtMs":preparation_finished,"healthPhase":"before-final","providerHealthReadsPerSource":3,"executionSchedule":execution_schedule,"financialCloseAttempted":false}));
            assert!(preparation_finished < end.checked_add(30000).unwrap(), "real health/calendar preparation exhausted the unchanged final freshness budget");
        }
        let requested_at=restored.runtime().clock.trusted_now_ms().unwrap();
        assert!(t<=requested_at,"fixture cannot request a future event window");
        if actual_math {eprintln!("synthetic declared-interval math frame {index} completed window {}..{t}; actual request at {requested_at}",t-interval);}
        let run_id = if provisional && index > 0 { "managed-position-reversal-fresh" } else { "managed-position-reversal" };
        let r=restored.handle_http("POST","/product/strategies/data-runs/step",json!({"step":{"runId":run_id,"planRevisionId":plan,"window":{"eventStartMs":t-interval,"eventEndMs":t,"asOfMs":requested_at},"expectedCheckpointSha256":checkpoint},"idempotencyKey":format!("managed-plan-frame-{index}")}));
        let processed_at=restored.runtime().clock.trusted_now_ms().unwrap();
        if actual_math {
            frame_timing.push(json!({"frame":index,"eventStartMs":t-interval,"eventEndMs":t,"requestedAtMs":requested_at,"processedAtMs":processed_at,"responseStatus":r.status,"waits":waits}));
            eprintln!("synthetic declared-interval math frame {index} response at {processed_at}; event age {} ms",processed_at-t);
            write_screen_interface_evidence("managed_plan",&format!("{grid_label}-frame-{index}.json"),&json!({"selectedAtMs":scheduled_at,"finalEventEndMs":end,"frames":frame_timing}));
        }
        assert_eq!(r.status, 200, "{}", r.body);
        if actual_math && index==3 {assert!(processed_at-end<30000,"final actual calculation exceeded the unchanged reversal freshness deadline");}
        assert_eq!(
            r.body["receipt"]["signals"][0]["entry"],
            json!({"status":"ready","value":false})
        );
        assert_eq!(
            r.body["receipt"]["signals"][0]["exit"],
            if if actual_math { index < 3 } else { index == 0 || provisional && index == 1 } {
                json!({"status":"warming_up"})
            } else {
                json!({"status":"ready","value":true})
            }
        );
        checkpoint = r.body["receipt"]["checkpoint"]["checkpointSha256"].clone();
        if actual_math {
            assert_managed_math_frame(&r.body["receipt"], index, math_final_value);
        }
        receipts.push(r.body["receipt"].clone());
        if (actual_math||provisional) && index == 0 {
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
        if entry_refusals.is_none(){entry_refusals=Some(check_entry_refusals_and_pause(receipts.last().unwrap()));}
        if execution_schedule.is_none(){execution_schedule=Some(prepare_execution_schedule());}
    }
    let (noentry, select) = entry_refusals.expect("entry refusals and entry pause completed");
    let schedule = execution_schedule.expect("fresh execution calendar prepared before close");
    let occurrence = schedule["managementWindow"]["windowId"].as_str()
        .unwrap_or_else(|| schedule["schedule"]["occurrence"]["occurrenceId"].as_str().unwrap());
    let advance = |receipt: &Value, key: &str| {
        let ids = managed_plan_fixture_observations(s, scope, revision, key);
        s.handle_http("POST",plans::management::ADVANCE,json!({"scope":scope,"advance":{"executionOccurrenceId":occurrence,"positionId":id,"accountCollectionId":ids["account"],"metadataCollectionId":ids["metadata"],"quoteCollectionId":ids["quote"],"reversalSignalReceiptSha256":receipt["receiptSha256"]},"accountMode":"paper","idempotencyKey":key}))
    };
    let submits = fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap();
    let unready = if actual_math || provisional {
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
    if provider["configuration"]["scenario"]=="managed_plan_c5_profile" {
        let command=serde_json::from_value(decision["submission"].clone()).unwrap();
        let (source,request,context)=plans::transport::request(s,&command).unwrap();
        let cash=s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap();
        let states=s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap();
        let profile=crate::broker_submission::v2::profile_validation_round(&source,&request,&context).unwrap();
        assert_eq!(fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap(),submits);
        assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(),cash);
        assert_eq!(s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap(),states);
        eprintln!("C5_VALIDATION_PROFILE {profile}");
        write_screen_interface_evidence("c5_validation","pure-validation-cost.json",&profile);
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
    let close_cash_before=s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap();
    let close_state_before=s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap();
    let final_posting = post(
        "/product/strategies/order-plans/settle",
        json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-plan-close-final"}),
    );
    if provisional {
        assert_eq!(final_posting["cashReconciled"],false);assert_eq!(final_posting["releasedHold"],"0");
        assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(),close_cash_before);
        assert_eq!(s.runtime().storage.list_json("execution_order_plan_submissions_v2").unwrap(),close_state_before);
        let owner=s.invocation_owner().unwrap();let restored=crate::service::TradeAssemblyService::from_runtime(s.db(),(*s.runtime()).clone()).for_authenticated_invocation(&owner.issuer,&owner.subject,None,None);
        let calls_before=fs::read_to_string(root.join("synthetic-broker/observation.log")).unwrap();
        let assert_replay = |status: u16, response: &Value| {
            assert_eq!(status, 200, "{}", response);
            assert_eq!(response["duplicate"], true);
            assert!(response["commandId"].is_string());
            let mut domain = response.clone();
            domain.as_object_mut().unwrap().remove("duplicate");
            domain.as_object_mut().unwrap().remove("commandId");
            assert_eq!(domain, final_posting, "original domain receipt remains immutable");
        };
        let retry=restored.handle_http("POST","/product/strategies/order-plans/settle",json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-plan-close-final"}));assert_replay(retry.status, &retry.body);assert_eq!(fs::read_to_string(root.join("synthetic-broker/observation.log")).unwrap(),calls_before);
        let repeated=restored.handle_http("POST","/product/strategies/order-plans/settle",json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-plan-close-new-read"}));assert_eq!(repeated.status,200,"{}",repeated.body);assert_eq!(repeated.body["releasedHold"],"0");
        assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(),close_cash_before);
        let replay=restored.handle_http("POST","/product/strategies/order-plans/settle",json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"managed-plan-close-final"}));assert_replay(replay.status, &replay.body);
        write_screen_interface_evidence("provisional","cash-holds-and-recovery.json",&json!({"entryObservation":settled,"closeObservation":final_posting,"newRead":repeated.body,"originalReplay":replay.body,"cashLedgerUnchangedByObservations":true,"submissionStateUnchangedByObservations":true,"actualCoreOsRestartQualified":false,"serviceRecreationQualified":true,"cashReconciled":false,"realProviderCalls":0}));
    }
    let inspected = s.handle_http(
        "GET",
        &format!("/product/strategies/positions/{id}"),
        json!({}),
    );
    assert_eq!(inspected.status, 200, "{}", inspected.body);
    let partial=s.runtime().plugins.get_instance("fixture.broker.v2").unwrap().unwrap()["configuration"]["partialClose"]==true;
    assert_eq!(inspected.body["position"]["closedQuantity"],if partial{"0.2"}else{"0.4"});
    if provisional{assert!(inspected.body["position"]["reservations"].as_object().unwrap().is_empty());}

    let calls = fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap();
    assert_eq!(calls.lines().filter(|v| *v == "submit").count(), 2);
    assert_eq!(calls.lines().filter(|v| *v == "lookup").count(), 2);
    write_screen_interface_evidence(
        "managed_plan",
        "generic-reversal-owned-removed-member.json",
        &json!({"entrySettlement":settled,"position":registered,"replacementSelection":replacement,"normalRemovedSubjectRefusal":normal.body,"wrongPositionRefusal":wrong.body,"managedPlan":prepared,"frames":receipts,"entryRefusal":noentry.body,"selectionRefusal":select.body,"unreadyRefusal":unready.map(|v|v.body),"advanced":advanced.body,"submitted":submitted,"finalPosting":final_posting,"nonAtomicObservationOnly":provisional,"journal":inspected.body,"syntheticSubmits":2,"syntheticLookups":2,"realBrokerCalls":0,"actualMaTsiMathQualified":actual_math,"declaredIntervalMs":interval,"syntheticFrameTiming":frame_timing,"exchangeCalendarAlignmentQualified":false,"userDailyBtcStrategyQualified":false,"automaticReversalQualified":false,"crossPositionReceiptLineageNegativeQualified":false,"actualPackSha256":math_pack_sha,"qualification":if provisional{"native synthetic C5 entry/close with V3 recovery and non-atomic V2 observations; unsettled owned-position management; cash holds preserved, service recreation, original receipt replay and no duplicate close; no coherent cash reconciliation, automatic/OS restart or real provider claim"}else if actual_math{"actual native pinned-pack EMA2/TSI2,2 explicit fixture at declared interval, independent finite Core close C5; synthetic source/provider/mock Warden/traced sandbox; no auto/remote/OS-process restart claim"}else{"actual Core local API/native authored generic reversal source; manual finite management, independent normal close C5; no actual MA/TSI/process restart/remote producer qualification"}}),
    );
    if provider["configuration"]["scenario"] == "managed_plan_provisional_v7" {
        exercise_native_provider_management_v7(s,scope,entry,close,id,root);
    }

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

const MANAGED_MINUTE_MATH_CASE: &str = "financial_screen_managed_plan_math_minute";
const MANAGED_MINUTE_PACK_SHA: &str =
    "a9c3ee822e5072f2890b33be47db50ff5269bda93882f856e393ffed57a1deb3";
fn actual_managed_math_case(case: &str) -> bool {
    matches!(case, "financial_screen_managed_plan_math" | MANAGED_MINUTE_MATH_CASE)
}
fn install_managed_minute_pack(s: &crate::service::TradeAssemblyService) -> InstalledPluginPackage {
    let path = std::path::PathBuf::from(
        std::env::var_os("MANAGED_MINUTE_TEST_PACKAGE")
            .expect("explicit immutable0.74.1 pack archive required"),
    );
    assert_eq!(sha256(&fs::read(&path).unwrap()), MANAGED_MINUTE_PACK_SHA);
    let installed = s.handle_http("POST", "/plugins/packages", json!({
        "source":{"type":"package","locator":path,"package":{"type":"file","locator":path}},
        "integrity":{"packageSha256":MANAGED_MINUTE_PACK_SHA,"manifestSha256":"b6fe45003d80c87604e1acb98568b35791a76556a67cb67f58d40a73386bdcf8"},
        "offline":true,"idempotencyKey":"managed-minute-public-package"
    }));
    assert_eq!(installed.status, 200, "{}", installed.body);
    let p = s.runtime().plugin_packages.get(MANAGED_MINUTE_PACK_SHA).unwrap().unwrap();
    assert_eq!(p.plugin_ref, "tradeassembly.default-indicators");
    assert_eq!(p.version, "0.74.1");
    let created = s.handle_http("POST", "/plugins/instances", json!({
        "pluginRef":p.plugin_ref,"instanceRef":"managed-real-calculation","enabled":false,
        "configuration":{},"idempotencyKey":"managed-minute-public-instance"
    }));
    assert_eq!(created.status, 200, "{}", created.body);
    let enabled = s.handle_http("POST", "/plugins/instances/managed-real-calculation:enable", json!({"idempotencyKey":"managed-minute-public-enable"}));
    assert_eq!(enabled.status, 200, "{}", enabled.body);
    assert_eq!(enabled.body["instance"]["activePackageSha256"], MANAGED_MINUTE_PACK_SHA);
    write_screen_interface_evidence("managed_plan", "minute-public-package-admission.json", &json!({
        "packageSha256":p.package_sha256,"manifestSha256":p.manifest_sha256,
        "version":p.version,"publicInstallStatus":installed.status,"publicCreateStatus":created.status,
        "publicEnableStatus":enabled.status,"registryFallbackUsed":false,
        "qualification":"normal service public lifecycle in declared synthetic native fixture; separate normal compiled CLI/real Warden proof retained"
    }));
    p
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
    let minute = pack.package_sha256 == MANAGED_MINUTE_PACK_SHA;
    assert!(minute || pack.package_sha256 == MANAGED_MATH_PACK_SHA);
    let interval = if minute { 60000 } else { 86400000 };
    let config = &mut spec["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY];
    let mut template: SelectionStrategy = serde_json::from_value(config.clone()).unwrap();
    template.nodes[0].context = ContextTemplate::Subject {
        interval_ms: Some(interval),
    };
    for (node, parameters) in [
        ("managed_ema", json!({"period":2})),
        ("managed_tsi", json!({"firstPeriod":2,"secondPeriod":2})),
    ] {
        template.nodes.push(NodeTemplate {
            graph_node_id: node.into(),
            parameters,
            context: ContextTemplate::Subject {
                interval_ms: Some(interval),
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
        ("managed_ema", if minute { "indicator.ema.minute" } else { "indicator.ema" }),
        ("managed_tsi", if minute { "indicator.tsi.minute" } else { "indicator.tsi" }),
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
        // The current free pack must resolve through the host-owned, verified
        // package policy. The historical daily fixture keeps its explicit grant.
        if !minute {
            s.runtime().storage.put_json("entitlement_grants",capability,json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("managed-real-math-grants")).unwrap();
        }
    }
}
fn assert_managed_math_frame(receipt: &Value, index: i64, final_value: i64) {
    let point =
        |node: &str| &receipt["outputs"][node][0]["outputs"]["value"]["points"][0]["result"];
    // Independent oracle for authored prices103,102,101,100:
    // EMA2 seeds102.5, then alpha2/3 gives101.5 and100.5.
    // All changes=-1: both seeded EMA2 cascades yield signed=-1, abs=1;
    // TSI2,2 first becomes ready on sample4 and equals100*(-1)/1=-100.
    assert_eq!(point("exec_signal")["value"], json!(final_value + 3 - index));
    if index == 0 {
        assert_eq!(point("managed_ema")["state"], "warming_up");
    } else {
        let expected = final_value as f64 + 3.5 - index as f64;
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

#[test]
#[ignore = "explicit immutable0.74.1 minute aliases/public admission/native managed reversal; requires MANAGED_MINUTE_TEST_PACKAGE"]
fn native_managed_plan_real_pack_minute_ema_tsi_reversal_survives_removed_membership() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some(MANAGED_MINUTE_MATH_CASE));
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


fn exercise_native_provider_management_v7(s:&crate::service::TradeAssemblyService,scope:&data_plans::PlanScope,entry:&str,close:&str,position:&str,root:&std::path::Path) {
    use crate::service::{data_runner::*,order_plans::management::provider_remote::ProviderManagementOperation as M};
    for instance in ["fixture.broker.v2","fixture.execution.facts"] {
        let response=s.handle_http("POST",&format!("/plugins/instances/{instance}/health:refresh"),json!({"idempotencyKey":format!("native-v7-health-{instance}")}));assert_eq!(response.status,200,"{}",response.body);
    }
    let cash=s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap();let submits=fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap().lines().filter(|x|*x=="submit").count();assert_eq!(submits,2);
    for (key,operation) in [
        ("observe-entry",M::ObserveSettlement{scope:scope.clone(),plan_id:entry.into()}),
        ("register-original",M::Register{scope:scope.clone(),entry_plan_id:entry.into(),activation_id:"fixture-v2-activation".into()}),
        ("lookup-close",M::ReconcileClose{scope:scope.clone(),plan_id:close.into()}),
        ("observe-close",M::SettleClose{scope:scope.clone(),plan_id:close.into()}),
        ("inspect-position",M::Inspect{scope:scope.clone(),position_id:position.into()}),
    ] {
        let runner=DurableCoreDataRunner::new(s.clone(),"fixture.release".into()).unwrap();
        let command=DataRunnerCommand{schema_version:DATA_RUNNER_SCHEMA_V7.into(),core_release:"fixture.release".into(),workspace_id:"fixture.provider-management".into(),command_id:key.into(),authority_ref:"provenance.only".into(),idempotency_key:IdempotencyKey::new(format!("native-v7-{key}")).unwrap(),mode:DataRunnerMode::Paper,submitted_at_ms:s.runtime().clock.now_ms(),operation:DataRunnerOperation::ProviderManagement{management:operation}};
        let foreign=DurableCoreDataRunner::new(s.for_authenticated_invocation("fixture","foreign",None,None),"fixture.release".into()).unwrap();let receipts=s.runtime().storage.list_json("plugin_operation_receipts").unwrap();assert!(foreign.mutation_binding(&command).is_err());assert_eq!(s.runtime().storage.list_json("plugin_operation_receipts").unwrap(),receipts);
        let witness=runner.verify_financial_dispatch(&command).unwrap();runner.assert_financial_witness_custody(&command,&witness);let receipt=runner.dispatch_verified(&command,witness).unwrap();assert_eq!(receipt.state,DataRunnerState::Completed,"{}",serde_json::to_string(&receipt).unwrap());
        let Some(DataRunnerResult::ProviderManagement{snapshot})=&receipt.result else{panic!("V7 result required")};assert_eq!(snapshot.schema_version,"tradeassembly.provider-management-snapshot.v7");assert!(!snapshot.execution_authority_granted);
        if key=="observe-entry"||key=="observe-close" {assert!(snapshot.position.is_none());assert!(snapshot.phase.is_none());let observation=snapshot.observation.as_ref().unwrap();assert!(!observation.cash_reconciled);assert_eq!(observation.released_hold.to_string(),"0");}
        if key=="inspect-position" {assert_eq!(snapshot.position.as_ref().unwrap().closed_quantity.to_string(),"0.4");assert!(snapshot.journal_included);assert!(!snapshot.journal.is_empty());}
        let source=s.invocation_owner().unwrap();let restored=DurableCoreDataRunner::new(crate::service::TradeAssemblyService::from_runtime(s.db(),(*s.runtime()).clone()).for_authenticated_invocation(&source.issuer,&source.subject,None,None),"fixture.release".into()).unwrap();let before=fs::read_to_string(root.join("synthetic-broker/observation.log")).unwrap();assert_eq!(restored.dispatch(&command).unwrap(),receipt);assert_eq!(fs::read_to_string(root.join("synthetic-broker/observation.log")).unwrap(),before);
        let binding=restored.reconciliation_binding(&command).unwrap();assert_eq!(binding.schema_version,"tradeassembly.core-data-mutation-binding.v7");assert_eq!(binding.resource_kind.as_deref(),command.resource_kind());
        write_screen_interface_evidence("provider_management_v7",&format!("{key}.json"),&json!({"command":command,"binding":binding,"receipt":receipt,"qualification":"actual Core/native authored provider; no signed Relay, whole-Core process, enforcing SRT or real provider claim"}));
    }
    assert_eq!(s.runtime().storage.list_json("execution_cash_reservations_v1").unwrap(),cash);assert_eq!(fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap().lines().filter(|x|*x=="submit").count(),submits);
}
#[test]
#[ignore="explicit native V7 observation/register/lookup/inspect and immutable recovery; no real broker"]
fn native_provider_management_v7_preserves_receipts_and_cash_holds(){crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();native_execution_plan_fixture(Some("financial_screen_managed_plan_provisional_v7"));}

#[test]
#[ignore = "explicit native managed-plan replay/revocation only; stops before reversal/close"]
fn native_managed_plan_metadata_round_replay_and_revocation() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some("financial_screen_managed_plan_metadata_round"));
}

#[test]
#[ignore="bounded actual owned-close pure C5 validation comparison with wide pinned calculation catalog; no close submission"]
fn native_owned_close_c5_validation_round_cost(){
 crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
 let evidence=std::path::PathBuf::from(std::env::var_os("SCREEN_INTERFACE_EVIDENCE_DIR").expect("explicit fresh profiling evidence directory"))
  .join("financial_screen_managed_plan_c5_profile/c5_validation/pure-validation-cost.json");
 assert!(!evidence.exists(),"profile requires a fresh evidence path");
 native_execution_plan_fixture(Some("financial_screen_managed_plan_c5_profile"));
 let result:Value=serde_json::from_slice(&fs::read(evidence).expect("profiling branch must actually execute")).unwrap();
 assert_eq!(result["samePlanAndIntent"],true);
 assert_eq!(result["durableIntentWritten"],false);
 assert_eq!(result["brokerCalls"],0);
}
