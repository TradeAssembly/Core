//! A finite, opted-in group of independent reads for one owned position.
//! Every member still enters its normal authorized HTTP route. This internal
//! descriptor is not an arbitrary request proxy or financial admission.
use super::*;
use crate::service::data_collections::{self, CollectionCommand};

const INTERNAL: &str = "internal:management-observation-inputs@1";
const VERSION: &str = "tradeassembly.management_observation_inputs.v1";
const CAPTURE: &str = "/product/strategies/data-captures/create";
const COLLECTION: &str = "/product/strategies/data-collections/create";
const CAPTURE_KEY: &str = "completed_bars";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Batch {
    schema_version: String,
    capture: Child,
    collections: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    input_batch: Batch,
    account_mode: String,
    idempotency_key: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadResponse {
    status: u16,
    body: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    schema_version: String,
    reads: BTreeMap<String, ReadResponse>,
    execution_authority_granted: bool,
}
fn expected_collection(
    b: &Binding,
    entry: &OrderPlan,
    key: &str,
    role: &str,
) -> Result<Value, String> {
    let schema = match role {
        "account_data" => ACCOUNT_SCHEMA,
        "instrument_metadata" => METADATA_SCHEMA,
        "pricing_quote" => QUOTE_SCHEMA,
        _ => return Err("management_input_batch_role_invalid".into()),
    };
    let command = CollectionCommand {
        scope: entry.command.scope.clone(),
        capability_graph_revision: entry.command.capability_graph_revision.clone(),
        acquisition_node: entry
            .policy
            .source_nodes
            .get(role)
            .ok_or("management_automation_source_missing")?
            .clone(),
        mode: "paper".into(),
        response_schema_ref: schema.into(),
        input: b
            .config
            .input(role, &entry.order.instrument.instrument_id)?,
    };
    // Fixed existing roles; their explicit input templates depend only on the
    // original owned instrument/literals, never on another read's response.
    Ok(json!({"collection":command,"accountMode":"paper","idempotencyKey":format!("{key}:{role}")}))
}
fn validate_shape(b: &Binding, entry: &OrderPlan, child: &Child) -> Result<Envelope, String> {
    if serde_json::to_vec(&child.body)
        .map_err(|_| "management_input_batch_invalid")?
        .len()
        > 32_768
    {
        return Err("management_automation_child_too_large".into());
    }
    let envelope: Envelope =
        serde_json::from_value(child.body.clone()).map_err(|_| "management_input_batch_invalid")?;
    let source = match &child.action {
        Action::AcquireInputs { source } => source,
        _ => return Err("management_input_batch_action_invalid".into()),
    };
    let capture = &envelope.input_batch.capture;
    if b.config.observation_acquisition.is_none()
        || child.path != INTERNAL
        || envelope.account_mode != "paper"
        || envelope.idempotency_key != child.key
        || envelope.input_batch.schema_version != VERSION
        || capture.path != CAPTURE
        || capture.key != child.key
        || capture.action
            != (Action::Reversal {
                step: reversal::Action::CaptureSegment {
                    source: source.clone(),
                },
            })
        || capture.body.as_object().is_none_or(|body| {
            body.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
                != std::collections::BTreeSet::from(["accountMode", "capture", "idempotencyKey"])
        })
        || capture.body["accountMode"] != "paper"
        || capture.body["idempotencyKey"] != child.key
        || envelope
            .input_batch
            .collections
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != ROLES
    {
        return Err("management_input_batch_binding_invalid".into());
    }
    for role in ROLES {
        if envelope.input_batch.collections[role]
            != expected_collection(b, entry, &child.key, role)?
        {
            return Err("management_input_batch_collection_binding_invalid".into());
        }
    }
    Ok(envelope)
}
pub(super) fn plan(
    b: &Binding,
    entry: &OrderPlan,
    source: &str,
    capture: Child,
) -> Result<Child, String> {
    let collections = ROLES
        .into_iter()
        .map(|role| {
            Ok((
                role.into(),
                expected_collection(b, entry, &capture.key, role)?,
            ))
        })
        .collect::<Result<_, String>>()?;
    let envelope = Envelope {
        input_batch: Batch {
            schema_version: VERSION.into(),
            capture: capture.clone(),
            collections,
        },
        account_mode: "paper".into(),
        idempotency_key: capture.key.clone(),
    };
    let child = Child {
        key: capture.key,
        path: INTERNAL.into(),
        body: value(&envelope)?,
        action: Action::AcquireInputs {
            source: source.into(),
        },
    };
    validate_shape(b, entry, &child)?;
    Ok(child)
}
fn validate(
    s: &TradeAssemblyService,
    b: &Binding,
    state: &State,
    child: &Child,
) -> Result<Envelope, String> {
    s.require_background_worker_current()?;
    let work = state
        .work
        .get(state.cursor)
        .ok_or("management_automation_work_missing")?;
    if work.kind != Kind::Position {
        return Err("management_automation_position_binding_invalid".into());
    }
    let (_, position) = load(s, &work.id)?;
    let entry = read_owned(s, &position.entry_plan_id)?;
    if position.activation_id != b.activation || !position.reservations.is_empty() {
        return Err("management_automation_position_binding_invalid".into());
    }
    let envelope = validate_shape(b, &entry, child)?;
    let Action::AcquireInputs { source } = &child.action else {
        unreachable!()
    };
    reversal::validate_capture_child(
        s,
        b,
        state,
        &envelope.input_batch.capture,
        source,
        &position,
        &entry,
    )?;
    Ok(envelope)
}
fn requests(envelope: &Envelope) -> BTreeMap<String, (String, Value)> {
    let mut requests = BTreeMap::from([(
        CAPTURE_KEY.into(),
        (CAPTURE.into(), envelope.input_batch.capture.body.clone()),
    )]);
    requests.extend(
        envelope
            .input_batch
            .collections
            .iter()
            .map(|(role, body)| (role.clone(), (COLLECTION.into(), body.clone()))),
    );
    requests
}
fn run_parallel(
    requests: &BTreeMap<String, (String, Value)>,
    dispatch: impl Fn(&str, &Value) -> ServiceResponse + Sync,
) -> BTreeMap<String, ReadResponse> {
    // Exactly four normal C3 invocations. Individual host operation deadlines,
    // sandboxes, scoped credentials and provider evidence remain unchanged.
    std::thread::scope(|scope| {
        let handles = requests
            .iter()
            .map(|(role, (path, body))| {
                let dispatch = &dispatch;
                (role.clone(), scope.spawn(move || dispatch(path, body)))
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|(role, handle)| {
                let response = handle.join().unwrap_or_else(|_| {
                    ServiceResponse::error(503, "management_input_read_panicked", true)
                });
                (
                    role,
                    ReadResponse {
                        status: response.status,
                        body: response.body,
                    },
                )
            })
            .collect()
    })
}
pub(super) fn retryable_response(response: &ServiceResponse) -> bool {
    !matches!(response.status, 401 | 403)
        && response.body["error"]["retryable"].as_bool() == Some(true)
}
fn response(reads: BTreeMap<String, ReadResponse>) -> ServiceResponse {
    let success = reads.len() == 4 && reads.values().all(|r| (200..300).contains(&r.status));
    let terminal = reads
        .values()
        .find(|r| {
            !(200..300).contains(&r.status)
                && (matches!(r.status, 401 | 403)
                    || r.body["error"]["retryable"].as_bool() != Some(true))
        })
        .cloned();
    let body = match value(&Response {
        schema_version: VERSION.into(),
        reads,
        execution_authority_granted: false,
    }) {
        Ok(body) => body,
        Err(_) => {
            return ServiceResponse::error(503, "management_input_batch_response_invalid", false)
        }
    };
    let response = if success {
        ServiceResponse::ok(body)
    } else {
        let mut error = if let Some(terminal) = terminal {
            // Preserve a known refusal, especially a per-read authority denial.
            // It must not be converted into a retryable unknown batch outcome.
            ServiceResponse {
                status: terminal.status,
                body: terminal.body,
            }
        } else {
            ServiceResponse::error(503, "management_input_batch_incomplete", true)
        };
        if matches!(error.status, 401 | 403) {
            error.body["error"]["retryable"] = json!(false);
        }
        error.body["readResults"] = body;
        error
    };
    if serde_json::to_vec(&response.body).is_ok_and(|bytes| bytes.len() <= MAX_BYTES) {
        response
    } else {
        ServiceResponse::error(503, "management_input_batch_response_too_large", false)
    }
}
pub(super) fn execute(
    s: &TradeAssemblyService,
    b: &Binding,
    state: &State,
    child: &Child,
) -> ServiceResponse {
    let envelope =
        match crate::service::package_validation::with_round(s, |s| validate(s, b, state, child)) {
            Ok(v) => v,
            Err(error) => return ServiceResponse::conflict(&error),
        };
    response(run_parallel(&requests(&envelope), |path, body| {
        s.handle_http("POST", path, body.clone())
    }))
}
fn successful_reads(response: &ServiceResponse) -> Result<Response, String> {
    let returned: Response = serde_json::from_value(response.body.clone())
        .map_err(|_| "management_input_batch_response_invalid")?;
    let mut keys = ROLES.to_vec();
    keys.push(CAPTURE_KEY);
    keys.sort();
    if response.status != 200
        || returned.schema_version != VERSION
        || returned.execution_authority_granted
        || returned
            .reads
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != keys
        || returned
            .reads
            .values()
            .any(|r| !(200..300).contains(&r.status))
    {
        return Err("management_input_batch_incomplete".into());
    }
    Ok(returned)
}
pub(super) fn apply(
    s: &TradeAssemblyService,
    b: &Binding,
    state: &mut State,
    child: &Child,
    response: &ServiceResponse,
) -> Result<(), String> {
    let returned = successful_reads(response)?;
    let envelope = validate(s, b, state, child)?;
    let mut ids = BTreeMap::new();
    for role in ROLES {
        let summary: &Value = &returned.reads[role].body["collection"];
        let id = summary["collectionId"]
            .as_str()
            .ok_or("management_input_batch_collection_missing")?;
        let actual = data_collections::current(s, id)?;
        let expected: CollectionCommand =
            serde_json::from_value(envelope.input_batch.collections[role]["collection"].clone())
                .map_err(|_| "management_input_batch_collection_invalid")?;
        if actual.command != expected || value(&data_collections::summary(&actual))? != *summary {
            return Err("management_input_batch_collection_result_mismatch".into());
        }
        ids.insert(role.into(), id.into());
    }
    // Normal acquisition receipts were individually durable before this join.
    // Publish no partial manager progress: all identities/current bindings must
    // pass, then save the capture and all three original evidence IDs together.
    let mut next = state.clone();
    let Action::AcquireInputs { source } = &child.action else {
        unreachable!()
    };
    let captured = &returned.reads[CAPTURE_KEY];
    reversal::apply_response(
        s,
        b,
        &mut next,
        &envelope.input_batch.capture,
        &reversal::Action::CaptureSegment {
            source: source.clone(),
        },
        &ServiceResponse {
            status: captured.status,
            body: captured.body.clone(),
        },
    )?;
    next.work
        .get_mut(next.cursor)
        .ok_or("management_automation_work_missing")?
        .collections
        .extend(ids);
    *state = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    };
    use std::time::{Duration, Instant};
    fn fixture() -> (Binding, OrderPlan, Child) {
        let (_dir, s, f, p) = crate::service::order_plans::tests::fixture();
        let mut entry = crate::service::order_plans::tests::plan(&s, &f, &p, "signal");
        entry.policy.source_nodes = ROLES
            .into_iter()
            .map(|role| (role.into(), format!("source_{role}")))
            .collect();
        let binding = Binding {
            activation: "owned-activation".into(),
            config: Config {
                observation_acquisition: Some(
                    typed_cycle::ObservationAcquisition::BoundedIndependentReadsV1,
                ),
                reversal_evaluation: None,
                calendar_rollover: None,
                cadence_ms: 1000,
                max_positions_per_round: 2,
                source_inputs: ROLES
                    .into_iter()
                    .map(|role| {
                        (
                            role.into(),
                            BTreeMap::from([(
                                "instrument_id".into(),
                                typed_cycle::InputValue::TradeInstrumentId {},
                            )]),
                        )
                    })
                    .collect(),
            },
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let capture = Child {
            key: "retained-capture-key".into(),
            path: CAPTURE.into(),
            body: json!({"capture":{"policy":{"scope":entry.command.scope},"input":{"cutoff":1000}},"accountMode":"paper","idempotencyKey":"retained-capture-key"}),
            action: Action::Reversal {
                step: reversal::Action::CaptureSegment {
                    source: "bars".into(),
                },
            },
        };
        (binding, entry, capture)
    }
    fn planned() -> (Binding, OrderPlan, Child) {
        let (b, e, c) = fixture();
        let p = plan(&b, &e, "bars", c).unwrap();
        (b, e, p)
    }
    #[test]
    fn input_batch_preserves_capture_and_all_owned_source_commands_without_substitution() {
        let (b, e, capture) = fixture();
        let before = capture.clone();
        let child = plan(&b, &e, "bars", capture).unwrap();
        let env = validate_shape(&b, &e, &child).unwrap();
        assert_eq!(env.input_batch.capture, before);
        assert_eq!(child.key, before.key);
        assert_eq!(requests(&env).len(), 4);
        for role in ROLES {
            let command = &env.input_batch.collections[role];
            assert_eq!(command["collection"]["input"]["instrument_id"], "SPY");
            assert_eq!(command["idempotencyKey"], format!("{}:{role}", before.key));
            assert_eq!(
                command["collection"]["acquisitionNode"],
                format!("source_{role}")
            );
        }
        let persisted = value(&child).unwrap();
        let restored: Child = serde_json::from_value(persisted.clone()).unwrap();
        assert_eq!(value(&restored).unwrap(), persisted);
        assert_eq!(
            requests(&validate_shape(&b, &e, &restored).unwrap()),
            requests(&env),
            "restart never regenerates read keys or cutoff"
        );
    }
    #[test]
    fn input_batch_is_explicit_and_legacy_configuration_bytes_remain_unchanged() {
        let (mut b, e, capture) = fixture();
        b.config.observation_acquisition = None;
        let old = value(&b.config).unwrap();
        assert!(old.get("observationAcquisition").is_none());
        assert_eq!(
            value(&serde_json::from_value::<Config>(old.clone()).unwrap()).unwrap(),
            old
        );
        assert!(plan(&b, &e, "bars", capture).is_err());
        let mut unknown = old;
        unknown["observationAcquisition"] = json!("unbounded_concurrent_mutations");
        assert!(serde_json::from_value::<Config>(unknown).is_err());
    }
    #[test]
    fn input_batch_rejects_changed_scope_source_instrument_mode_schema_key_and_extra_fields() {
        let (b, e, child) = planned();
        for (pointer, value) in [
            (
                "/inputBatch/collections/account_data/collection/scope/strategyId",
                json!("foreign"),
            ),
            (
                "/inputBatch/collections/account_data/collection/acquisitionNode",
                json!("other"),
            ),
            (
                "/inputBatch/collections/instrument_metadata/collection/input/instrument_id",
                json!("OTHER"),
            ),
            (
                "/inputBatch/collections/pricing_quote/accountMode",
                json!("live"),
            ),
            (
                "/inputBatch/collections/pricing_quote/collection/responseSchemaRef",
                json!("other-schema"),
            ),
            (
                "/inputBatch/collections/pricing_quote/idempotencyKey",
                json!("new-key"),
            ),
            ("/inputBatch/capture/path", json!(SUBMIT)),
            (
                "/inputBatch/capture/body/idempotencyKey",
                json!("other-key"),
            ),
        ] {
            let mut altered = child.clone();
            *altered.body.pointer_mut(pointer).unwrap() = value;
            assert!(validate_shape(&b, &e, &altered).is_err(), "{pointer}");
        }
        let mut missing = child.clone();
        missing.body["inputBatch"]["collections"]
            .as_object_mut()
            .unwrap()
            .remove("account_data");
        assert!(validate_shape(&b, &e, &missing).is_err());
        let mut extra = child;
        extra.body["inputBatch"]["capture"]["body"]["brokerSubmission"] = json!({});
        assert!(validate_shape(&b, &e, &extra).is_err());
    }
    #[test]
    fn input_batch_four_independent_calls_overlap_with_exact_bodies_and_original_times() {
        let (b, e, c) = planned();
        let env = validate_shape(&b, &e, &c).unwrap();
        let requests = requests(&env);
        let barrier = Barrier::new(4);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let started = Instant::now();
        let reads = run_parallel(&requests, |path, body| {
            assert!([CAPTURE, COLLECTION].contains(&path));
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            barrier.wait();
            std::thread::sleep(Duration::from_millis(20));
            active.fetch_sub(1, Ordering::SeqCst);
            ServiceResponse::ok(json!({"exactRequest":body,"observedAtMs":1234,"knownAtMs":5678}))
        });
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        for (role, r) in &reads {
            assert_eq!(r.body["exactRequest"], requests[role].1);
            assert_eq!(r.body["observedAtMs"], 1234);
            assert_eq!(r.body["knownAtMs"], 5678);
        }
        assert_eq!(successful_reads(&response(reads)).unwrap().reads.len(), 4);
        eprintln!(
            "bounded-four-read-wall-us={}",
            started.elapsed().as_micros()
        );
    }
    #[test]
    fn input_batch_preserves_terminal_authority_denial_without_retry_or_partial_join() {
        let (b, e, c) = planned();
        let req = requests(&validate_shape(&b, &e, &c).unwrap());
        for role in ROLES.into_iter().chain([CAPTURE_KEY]) {
            let mut reads = run_parallel(&req, |_, body| ServiceResponse::ok(body.clone()));
            reads.insert(role.into(), ReadResponse {
                status: 403,
                body: json!({"error":{"code":"finance_authority_denied","retryable":true,"details":{"reason":"warden_signed_receipt_invalid"}}}),
            });
            let denied = response(reads);
            assert_eq!(denied.status, 403);
            assert_eq!(denied.body["error"]["code"], "finance_authority_denied");
            assert_eq!(denied.body["error"]["retryable"], false);
            assert!(!retryable_response(&denied));
            assert!(successful_reads(&denied).is_err());
        }
        assert!(retryable_response(&ServiceResponse::error(
            503,
            "temporary",
            true
        )));
        assert!(!retryable_response(&ServiceResponse::error(
            503, "invalid", false
        )));
    }
    #[test]
    fn input_batch_partial_failures_panics_and_oversized_results_never_join_successfully() {
        let (b, e, c) = planned();
        let req = requests(&validate_shape(&b, &e, &c).unwrap());
        for failing in [
            "account_data",
            "instrument_metadata",
            "pricing_quote",
            CAPTURE_KEY,
        ] {
            let mut reads = run_parallel(&req, |_, body| ServiceResponse::ok(body.clone()));
            reads.get_mut(failing).unwrap().status = 503;
            let result = response(reads);
            assert_eq!(result.status, 503);
            assert!(successful_reads(&result).is_err());
        }
        let reads = run_parallel(&req, |_, _| panic!("synthetic read panic"));
        assert_eq!(response(reads).status, 503);
        let reads = run_parallel(&req, |_, _| {
            ServiceResponse::ok(json!({"oversized":"x".repeat(MAX_BYTES)}))
        });
        assert!(successful_reads(&response(reads)).is_err());
    }
    #[test]
    fn input_batch_rejects_missing_or_inconsistent_member_responses_and_authority_claims() {
        let (b, e, c) = planned();
        let req = requests(&validate_shape(&b, &e, &c).unwrap());
        let good = response(run_parallel(&req, |_, body| {
            ServiceResponse::ok(body.clone())
        }));
        let mut missing = good.clone();
        missing.body["reads"]
            .as_object_mut()
            .unwrap()
            .remove("pricing_quote");
        assert!(successful_reads(&missing).is_err());
        let mut granted = good.clone();
        granted.body["executionAuthorityGranted"] = json!(true);
        assert!(successful_reads(&granted).is_err());
        let mut wrong = good;
        wrong.body["schemaVersion"] = json!("future");
        assert!(successful_reads(&wrong).is_err());
    }
    #[test]
    fn input_batch_preserves_original_partial_read_on_actual_storage_restart() {
        let (dir, service, _, _) = crate::service::order_plans::tests::fixture();
        let (mut b, _, child) = planned();
        // Storage-only reconstruction test; no manager/C3 invocation or
        // authentication resolution is bypassed in production.
        b.owner = owner(&service).unwrap();
        b.config_hash = hash(&b.config).unwrap();
        let mut state:State=serde_json::from_value(json!({"schemaVersion":"tradeassembly.management_automation.v1","activationId":b.activation,"ownerSha256":b.owner,"configSha256":b.config_hash,"coreRevision":b.revision,"occurrenceId":"occurrence","round":1,"generation":0,"childSequence":1,"nextDueMs":1,"positionCursor":null,"work":[],"cursor":0,"pending":child,"error":null,"failures":0,"stateSha256":""})).unwrap();
        state
            .work
            .push(Work::new(Kind::Position, "owned-position".into()));
        save(&service, &b, None, &mut state, None).unwrap();
        let old = Some(value(&state).unwrap());
        retain_reversal_child(&mut state, 1000, 1000, "synthetic_partial_read".into());
        save(&service,&b,old,&mut state,Some((&child,json!({"retryRetainedOriginalChild":true,"observed":{"status":503,"body":{"readResults":"original-partial-results"}}})))).unwrap();
        let persisted = value(&state).unwrap();
        drop(service);
        let db = dir.path().join("cash.db").to_string_lossy().into_owned();
        let restarted =
            TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db))
                .for_authenticated_invocation("fixture", "cash-owner", None, None);
        let (_, loaded) = load_state(&restarted, &b).unwrap().unwrap();
        assert_eq!(value(&loaded).unwrap(), persisted);
        assert_eq!(loaded.pending.as_ref(), Some(&child));
        assert!(loaded.work[0].collections.is_empty());
        let outcomes = restarted.runtime().storage.list_json(CHILDREN).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].1["response"]["observed"]["body"]["readResults"],
            "original-partial-results"
        );
        for namespace in [CASH, PLANS, TRIGGERS, submission::STATE] {
            assert!(restarted
                .runtime()
                .storage
                .list_json(namespace)
                .unwrap()
                .is_empty());
        }
    }
    #[test]
    fn input_batch_preserves_existing_command_size_bound_before_dispatch() {
        let (b, e, mut child) = planned();
        child.body["inputBatch"]["capture"]["body"]["capture"]["input"]["padding"] =
            json!("x".repeat(32768));
        assert_eq!(
            validate_shape(&b, &e, &child).unwrap_err(),
            "management_automation_child_too_large"
        );
    }
    #[test]
    fn input_batch_partial_response_cannot_mutate_manager_state() {
        let (_dir, s, _, _) = crate::service::order_plans::tests::fixture();
        let (b, _, child) = planned();
        let mut state:State=serde_json::from_value(json!({"schemaVersion":"tradeassembly.management_automation.v1","activationId":"owned-activation","ownerSha256":"owner","configSha256":"config","coreRevision":"revision","occurrenceId":"occurrence","round":1,"generation":0,"childSequence":1,"nextDueMs":1,"positionCursor":null,"work":[],"cursor":0,"pending":child,"error":null,"failures":0,"stateSha256":""})).unwrap();
        let before = value(&state).unwrap();
        let partial = ServiceResponse::internal_error("synthetic_partial_read");
        assert!(apply(&s, &b, &mut state, &child, &partial).is_err());
        assert_eq!(value(&state).unwrap(), before);
    }
}
