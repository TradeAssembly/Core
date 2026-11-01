//! Explicit bounded configuration for owned-position reversal observations.
//! Formula execution and checkpoint continuity belong to the normal evaluator;
//! input materialization never supplies provider evidence, state or authority.
use super::*;

const VERSION: &str = "tradeassembly.management_reversal_evaluation.v1";
const MAX_SOURCES: usize = 8;
const MAX_FIELDS: usize = 32;
const MAX_CONFIG_BYTES: usize = 32_768;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    pub schema_version: String,
    pub origin_policy: OriginPolicy,
    pub completion_lag_ms: i64,
    pub max_frames_per_step: u64,
    pub max_segment_observations: u64,
    pub source_bindings: BTreeMap<String, Source>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OriginPolicy {
    OriginalEntryCapture,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Source {
    pub acquisition_node: String,
    pub interval_ms: u64,
    pub origin: Origin,
    pub parameter_extracts: BTreeMap<String, String>,
    pub input: BTreeMap<String, Input>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Origin {
    Parameter(ParameterOrigin),
    Admitted(AdmittedOrigin),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ParameterOrigin {
    pub payload_pointer: String,
    pub replay_parameter: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AdmittedOrigin {
    AdmittedCaptureStart {},
}
/// Literal and trade-instrument inputs preserve the existing typed-cycle enum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Input {
    Existing(typed_cycle::InputValue),
    Window(WindowInput),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WindowInput {
    EventStartMs {},
    EventEndMs {},
    AsOfMs {},
    IntervalMs {},
    OriginStartMs {},
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !matches!(
            value,
            "config"
                | "credentials"
                | "authority"
                | "accountRef"
                | "account_ref"
                | "checkpoint"
                | "priorState"
                | "prior_state"
                | "providerResponse"
        )
}
fn node(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn pointer(value: &str) -> bool {
    if !value.starts_with('/') || value.len() > 1024 || value.chars().any(char::is_control) {
        return false;
    }
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return false;
        }
    }
    true
}
impl Config {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION
            || !(0..=30_000).contains(&self.completion_lag_ms)
            || !(1..=64).contains(&self.max_frames_per_step)
            || !(1..=256).contains(&self.max_segment_observations)
            || self.source_bindings.is_empty()
            || self.source_bindings.len() > MAX_SOURCES
            || serde_json::to_vec(self)
                .map_err(|_| "management_reversal_config_invalid")?
                .len()
                > MAX_CONFIG_BYTES
        {
            return Err("management_reversal_config_invalid".into());
        }
        for (replay_node, source) in &self.source_bindings {
            if !node(replay_node)
                || !node(&source.acquisition_node)
                || source.interval_ms == 0
                || source.interval_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
                || source.input.len() > MAX_FIELDS
                || source.parameter_extracts.is_empty()
                || source.parameter_extracts.len() > MAX_FIELDS
                || match &source.origin {
                    Origin::Parameter(origin) => {
                        !name(&origin.replay_parameter)
                            || !pointer(&origin.payload_pointer)
                            || source.parameter_extracts.get(&origin.replay_parameter)
                                != Some(&origin.payload_pointer)
                    }
                    Origin::Admitted(AdmittedOrigin::AdmittedCaptureStart {}) => false,
                }
                || source
                    .parameter_extracts
                    .iter()
                    .any(|(key, value)| !name(key) || !pointer(value))
            {
                return Err("management_reversal_source_binding_invalid".into());
            }
            validate_input_template(&source.input)?;
        }
        Ok(())
    }
    pub(crate) fn validate_reversal_age(&self, max_age_ms: i64) -> Result<(), String> {
        self.validate()?;
        if max_age_ms <= 0 || self.completion_lag_ms >= max_age_ms {
            return Err("management_reversal_completion_lag_exceeds_freshness".into());
        }
        Ok(())
    }
}
pub(crate) fn validate_input_template(template: &BTreeMap<String, Input>) -> Result<(), String> {
    if template.len() > MAX_FIELDS {
        return Err("management_reversal_input_invalid".into());
    }
    for (key, input) in template {
        if !name(key) {
            return Err("management_reversal_input_invalid".into());
        }
        if let Input::Existing(typed_cycle::InputValue::Literal { value }) = input {
            crate::service::data_captures::capturable_payload(value)?;
            tradeassembly_plugin_sdk::data_package::metadata_budget(value)?;
        }
    }
    Ok(())
}
pub(crate) fn materialize_input(
    template: &BTreeMap<String, Input>,
    interval_ms: u64,
    instrument: &str,
    window: &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
    origin_start_ms: i64,
) -> Result<Value, String> {
    validate_input_template(template)?;
    let max = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64;
    if origin_start_ms <= 0
        || origin_start_ms > window.event_start_ms
        || instrument.is_empty()
        || instrument.len() > 256
        || instrument.chars().any(char::is_control)
        || window.event_start_ms <= 0
        || window.event_start_ms >= window.event_end_ms
        || window.event_end_ms > window.as_of_ms
        || window.as_of_ms > max
        || interval_ms == 0
        || interval_ms > max as u64
        || !((window.event_end_ms - window.event_start_ms) as u64).is_multiple_of(interval_ms)
    {
        return Err("management_reversal_window_invalid".into());
    }
    Ok(Value::Object(
        template
            .iter()
            .map(|(key, input)| {
                let value = match input {
                    Input::Existing(typed_cycle::InputValue::Literal { value }) => value.clone(),
                    Input::Existing(typed_cycle::InputValue::TradeInstrumentId {}) => {
                        json!(instrument)
                    }
                    Input::Window(WindowInput::EventStartMs {}) => json!(window.event_start_ms),
                    Input::Window(WindowInput::EventEndMs {}) => json!(window.event_end_ms),
                    Input::Window(WindowInput::AsOfMs {}) => json!(window.as_of_ms),
                    Input::Window(WindowInput::IntervalMs {}) => json!(interval_ms),
                    Input::Window(WindowInput::OriginStartMs {}) => json!(origin_start_ms),
                };
                (key.clone(), value)
            })
            .collect(),
    ))
}
impl Source {
    pub(crate) fn materialize_input(
        &self,
        instrument: &str,
        window: &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
        origin_start_ms: i64,
    ) -> Result<Value, String> {
        materialize_input(
            &self.input,
            self.interval_ms,
            instrument,
            window,
            origin_start_ms,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(json!({
        "schemaVersion":VERSION,"originPolicy":"original_entry_capture",
        "completionLagMs":100,"maxFramesPerStep":64,"maxSegmentObservations":256,
        "sourceBindings":{"example_source":{
            "acquisitionNode":"example_acquire","intervalMs":60000,
            "origin":{"payloadPointer":"/originStartMs","replayParameter":"originStartMs"},
            "parameterExtracts":{"originStartMs":"/originStartMs","values":"/values"},
            "input":{"instrument":{"kind":"trade_instrument_id"},
                "start":{"kind":"event_start_ms"},"end":{"kind":"event_end_ms"},
                "cutoff":{"kind":"as_of_ms"},"interval":{"kind":"interval_ms"},
                "limit":{"kind":"literal","value":256}}
        }}}))
        .unwrap()
    }
    #[test]
    fn explicit_admitted_origin_is_closed_and_legacy_parameter_origin_bytes_are_unchanged() {
        let legacy = r#"{"payloadPointer":"/originStartMs","replayParameter":"originStartMs"}"#;
        let origin: Origin = serde_json::from_str(legacy).unwrap();
        assert_eq!(serde_json::to_string(&origin).unwrap(), legacy);
        let admitted: Origin =
            serde_json::from_value(json!({"kind":"admitted_capture_start"})).unwrap();
        assert_eq!(
            serde_json::to_value(&admitted).unwrap(),
            json!({"kind":"admitted_capture_start"})
        );
        let mut c = config();
        c.source_bindings.get_mut("example_source").unwrap().origin = admitted;
        c.source_bindings
            .get_mut("example_source")
            .unwrap()
            .parameter_extracts
            .remove("originStartMs");
        c.validate().unwrap();
        for bad in [
            json!({}),
            json!({"kind":"market_open"}),
            json!({"kind":"admitted_capture_start","payloadPointer":"/invented"}),
        ] {
            assert!(serde_json::from_value::<Origin>(bad).is_err());
        }
    }
    #[test]
    fn stream_identity_keeps_exact_input_template_and_owned_trade_symbol() {
        let c = config();
        let source = &c.source_bindings["example_source"];
        let value = binding_value(source).unwrap();
        assert!(value.get("input").is_none());
        assert_eq!(
            value["inputTemplate"],
            serde_json::to_value(&source.input).unwrap()
        );
        validate_input_template(&source.input).unwrap();
        let window = tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
            event_start_ms: 100,
            event_end_ms: 60100,
            as_of_ms: 60101,
        };
        assert_eq!(
            materialize_input(&source.input, 60000, "OWNED-TRADE", &window, 100).unwrap()
                ["instrument"],
            "OWNED-TRADE"
        );
        let mut invalid = source.input.clone();
        invalid.insert(
            "credentials".into(),
            Input::Existing(typed_cycle::InputValue::Literal { value: json!("no") }),
        );
        assert!(validate_input_template(&invalid).is_err());
    }
    #[test]
    fn reversal_configuration_is_closed_and_bounded() {
        let c = config();
        c.validate_reversal_age(30_000).unwrap();
        let original = serde_json::to_value(&c).unwrap();
        for (field, value) in [
            ("formula", json!("not executable")),
            ("state", json!({})),
            ("checkpointSha256", json!("a".repeat(64))),
        ] {
            let mut v = original.clone();
            v[field] = value;
            assert!(serde_json::from_value::<Config>(v).is_err());
        }
        let mut bad = c.clone();
        bad.max_frames_per_step = 65;
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.max_segment_observations = 257;
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.completion_lag_ms = 30_000;
        assert!(bad.validate_reversal_age(30_000).is_err());
    }
    #[test]
    fn reversal_origin_and_inputs_do_not_accept_unbound_or_secret_material() {
        let mut c = config();
        if let Origin::Parameter(origin) =
            &mut c.source_bindings.get_mut("example_source").unwrap().origin
        {
            origin.payload_pointer = "/different".into();
        }
        assert!(c.validate().is_err());
        let mut c = config();
        c.source_bindings
            .get_mut("example_source")
            .unwrap()
            .input
            .insert(
                "payload".into(),
                Input::Existing(typed_cycle::InputValue::Literal {
                    value: json!({"credentials":"forbidden"}),
                }),
            );
        assert!(c.validate().is_err());
        assert!(!pointer("/invalid~2escape"));
        assert!(pointer("/valid~1escaped~0pointer"));
    }
    #[test]
    fn reversal_input_materialization_is_exact_and_never_infers_calendar_times() {
        let c = config();
        let source = &c.source_bindings["example_source"];
        let window = tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
            event_start_ms: 1_700_000_000_123,
            event_end_ms: 1_700_000_060_123,
            as_of_ms: 1_700_000_060_456,
        };
        assert_eq!(
            source
                .materialize_input("OWNED", &window, window.event_start_ms)
                .unwrap(),
            json!({
                "instrument":"OWNED","start":window.event_start_ms,"end":window.event_end_ms,
                "cutoff":window.as_of_ms,"interval":60000,"limit":256,
            })
        );
        let mut future = window.clone();
        future.as_of_ms = future.event_end_ms - 1;
        assert!(source
            .materialize_input("OWNED", &future, window.event_start_ms)
            .is_err());
    }
}

/// Bounded per-position continuation. Immutable capture segments stay in the
/// producer repository; this head never embeds provider points or formula state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Progress {
    streams: BTreeMap<String, StreamHead>,
    pending_captures: BTreeMap<String, String>,
    plan_revision_id: Option<String>,
    run_id: String,
    cursor_ms: Option<i64>,
    checkpoint_sha256: Option<String>,
    signal_receipt_sha256: Option<String>,
    waiting_for_input: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamHead {
    stream_id: String,
    binding_sha256: String,
    seed_capture_id: String,
    head_sha256: String,
    original_origin_ms: i64,
    event_end_ms: i64,
    interval_ms: u64,
    context: crate::data_pipeline::DataContext,
}
impl Progress {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.streams.len() > MAX_SOURCES
            || self.pending_captures.len() > MAX_SOURCES
            || self.run_id.is_empty()
            || self.run_id.len() > 128
            || self.run_id.chars().any(char::is_control)
            || self
                .plan_revision_id
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 256)
            || self.checkpoint_sha256.as_ref().is_some_and(|v| !digest(v))
            || self
                .signal_receipt_sha256
                .as_ref()
                .is_some_and(|v| !digest(v))
            || self.pending_captures.iter().any(|(key, value)| {
                !self.streams.contains_key(key) || value.len() > 256 || value.is_empty()
            })
            || self.streams.values().any(|head| {
                head.context.instruments.len() > 32
                    || head.context.instruments.is_empty()
                    || head.interval_ms == 0
                    || head.interval_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
                    || head.original_origin_ms <= 0
                    || head.original_origin_ms >= head.event_end_ms
                    || head.event_end_ms
                        > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
                    || head.context.interval_ms != Some(head.interval_ms)
                    || !digest(&head.binding_sha256)
                    || !digest(&head.head_sha256)
                    || [&head.stream_id, &head.seed_capture_id]
                        .iter()
                        .any(|v| v.is_empty() || v.len() > 256)
            })
        {
            return Err("management_reversal_state_invalid".into());
        }
        Ok(())
    }
}
/// Step descriptors select finite Core routes only, never provider operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    CreateStream { source: String },
    CaptureSegment { source: String },
    AppendSegment { source: String },
    PreparePlan {},
    Evaluate {},
}

fn completed_boundary(origin: i64, interval: u64, now: i64, lag: i64) -> Result<i64, String> {
    let max = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64;
    if origin <= 0
        || origin > max
        || now < origin
        || now > max
        || lag < 0
        || interval == 0
        || interval > max as u64
    {
        return Err("management_reversal_grid_invalid".into());
    }
    let available = now
        .checked_sub(lag)
        .filter(|t| *t >= origin)
        .ok_or("management_reversal_no_completed_frame")?;
    let span = (available - origin) / interval as i64 * interval as i64;
    origin
        .checked_add(span)
        .ok_or_else(|| "management_reversal_grid_invalid".into())
}
fn step_window(
    cursor: i64,
    head_end: i64,
    interval: u64,
    max_frames: u64,
    as_of: i64,
) -> Result<tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow, String> {
    let max = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64;
    if cursor <= 0
        || cursor >= head_end
        || head_end > as_of
        || as_of > max
        || !(1..=64).contains(&max_frames)
        || interval == 0
        || interval > max as u64
        || (head_end - cursor) as u64 % interval != 0
    {
        return Err("management_reversal_window_invalid".into());
    }
    let count = ((head_end - cursor) as u64 / interval).min(max_frames);
    let end = interval
        .checked_mul(count)
        .and_then(|delta| cursor.checked_add(delta as i64))
        .ok_or("management_reversal_window_invalid")?;
    Ok(
        tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
            event_start_ms: cursor,
            event_end_ms: end,
            as_of_ms: as_of,
        },
    )
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    #[test]
    fn one_hundred_fifty_frame_bootstrap_keeps_exact_checkpoint_successor_windows() {
        let origin = 1_700_000_000_123;
        let interval = 86_400_000;
        let target = origin + 150 * interval;
        let mut cursor = origin;
        let mut lengths = Vec::new();
        while cursor < target {
            let window = step_window(cursor, target, interval as u64, 64, target + 1).unwrap();
            assert_eq!(window.event_start_ms, cursor);
            lengths.push((window.event_end_ms - window.event_start_ms) / interval);
            cursor = window.event_end_ms;
        }
        assert_eq!(lengths, vec![64, 64, 22]);
        assert_eq!(cursor, target);
        let successor = step_window(
            cursor,
            target + interval,
            interval as u64,
            64,
            target + interval + 1,
        )
        .unwrap();
        assert_eq!(successor.event_start_ms, target);
        assert_eq!(successor.event_end_ms, target + interval);
    }
    #[test]
    fn selected_trade_alias_is_still_a_context_dependency_when_referenced() {
        use crate::selection_strategy::*;
        let template = SelectionStrategy {
            schema_version: "tradeassembly.selection_strategy.v1".into(),
            selections: BTreeMap::new(),
            trade_selection_ref: "trade".into(),
            nodes: vec![NodeTemplate {
                graph_node_id: "node".into(),
                parameters: json!({}),
                context: ContextTemplate::Subject {
                    interval_ms: Some(1),
                },
                input_contexts: BTreeMap::from([(
                    "context".into(),
                    ContextTemplate::Selected {
                        selection_ref: "trade".into(),
                        interval_ms: Some(1),
                    },
                )]),
            }],
            bindings: vec![],
            entry: RuleTemplate::IsTrue {
                value: OperandTemplate::Boolean { value: false },
            },
            exit: RuleTemplate::IsTrue {
                value: OperandTemplate::Boolean { value: true },
            },
        };
        assert_eq!(
            context_aliases(&template),
            std::collections::BTreeSet::from(["trade".into()])
        );
    }
    #[test]
    fn only_unknown_child_outcomes_require_original_key_recovery() {
        assert!(unknown_response(&ServiceResponse::conflict(
            "idempotency_in_progress"
        )));
        assert!(unknown_response(&ServiceResponse::conflict(
            "outcome_uncertain_reconciliation_required"
        )));
        assert!(!unknown_response(&ServiceResponse::forbidden(
            "policy_denied"
        )));
        assert!(!unknown_response(&ServiceResponse::bad_request(
            "unsupported_input"
        )));
    }
    #[test]
    fn grid_uses_provider_witnessed_origin_and_explicit_lag() {
        let origin = 1_700_000_000_123;
        assert_eq!(
            completed_boundary(origin, 60000, origin + 120_099, 100).unwrap(),
            origin + 60000
        );
        assert_eq!(
            completed_boundary(origin, 60000, origin + 120_100, 100).unwrap(),
            origin + 120000
        );
        assert!(completed_boundary(origin, 0, origin, 0).is_err());
        assert!(completed_boundary(origin, 60000, origin, 1).is_err());
    }
}

const CREATE_STREAM: &str = "/product/strategies/managed-data-streams/create";
const APPEND_STREAM: &str = "/product/strategies/managed-data-streams/append";
const MANAGED_STEP: &str = "/product/strategies/managed-data-runs/step";

fn binding_value(source: &Source) -> Result<Value, String> {
    let mut value =
        serde_json::to_value(source).map_err(|_| "management_reversal_config_invalid")?;
    let map = value
        .as_object_mut()
        .ok_or("management_reversal_config_invalid")?;
    let input = map
        .remove("input")
        .ok_or("management_reversal_config_invalid")?;
    map.insert("inputTemplate".into(), input);
    Ok(value)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn string(value: &Value, field: &str) -> Result<String, String> {
    value[field]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| "management_reversal_child_result_invalid".into())
}
fn stream_view(value: &Value) -> Result<StreamHead, String> {
    let view = StreamHead {
        stream_id: string(value, "streamId")?,
        binding_sha256: string(value, "bindingSha256")?,
        seed_capture_id: string(value, "seedCaptureId")?,
        head_sha256: string(value, "headSha256")?,
        original_origin_ms: value["originalOriginMs"]
            .as_i64()
            .ok_or("management_reversal_child_result_invalid")?,
        event_end_ms: value["eventEndMs"]
            .as_i64()
            .ok_or("management_reversal_child_result_invalid")?,
        interval_ms: value["intervalMs"]
            .as_u64()
            .ok_or("management_reversal_child_result_invalid")?,
        context: serde_json::from_value(value["context"].clone())
            .map_err(|_| "management_reversal_child_result_invalid")?,
    };
    if !digest(&view.binding_sha256)
        || !digest(&view.head_sha256)
        || view.original_origin_ms <= 0
        || view.original_origin_ms >= view.event_end_ms
        || view.interval_ms == 0
        || view.interval_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
        || view.event_end_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
        || (view.event_end_ms - view.original_origin_ms) as u64 % view.interval_ms != 0
        || view.context.interval_ms != Some(view.interval_ms)
        || view.context.instruments.is_empty()
        || view.context.instruments.len() > 32
    {
        return Err("management_reversal_child_result_invalid".into());
    }
    Ok(view)
}

fn context_aliases(
    template: &crate::selection_strategy::SelectionStrategy,
) -> std::collections::BTreeSet<String> {
    let mut context_aliases = std::collections::BTreeSet::new();
    for node in &template.nodes {
        for context in std::iter::once(&node.context).chain(node.input_contexts.values()) {
            if let crate::selection_strategy::ContextTemplate::Selected { selection_ref, .. } =
                context
            {
                context_aliases.insert(selection_ref.clone());
            }
        }
    }
    context_aliases
}
pub(super) fn validate_bindings(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    graph: &str,
    config: &Config,
) -> Result<(), String> {
    config.validate()?;
    let (spec, template) = crate::service::selection_plans::template(s, scope)?
        .ok_or("management_reversal_selection_template_required")?;
    // The first stream contract admits static/literal independent contexts.
    // Every Selected reference, including trade alias used as context, matters.
    let context_aliases = context_aliases(&template);
    for alias in &context_aliases {
        let binding = template
            .selections
            .get(alias)
            .ok_or("management_reversal_context_selection_missing")?;
        let selector = spec
            .signal_market_requirements
            .iter()
            .find(|m| m.requirement_id == binding.market_requirement_ref)
            .and_then(|m| {
                m.selectors
                    .iter()
                    .find(|v| v.selector_id == binding.selector_ref)
            })
            .ok_or("management_reversal_context_selection_missing")?;
        if selector.selector_type == tradeassembly_strategy::spec::SelectorType::Screened {
            return Err("management_reversal_screened_context_continuity_unsupported".into());
        }
    }
    let revision = crate::service::capability_graph::get_revision(s, graph);
    let resolved_graph: crate::ports::CapabilityGraphResolution =
        serde_json::from_value(revision["revision"]["graph"].clone())
            .map_err(|_| "management_reversal_graph_invalid")?;
    let intervals = config
        .source_bindings
        .values()
        .map(|source| source.interval_ms)
        .collect::<std::collections::BTreeSet<_>>();
    if intervals.len() != 1 {
        return Err("management_reversal_context_interval_mismatch".into());
    }
    let interval = *intervals
        .first()
        .ok_or("management_reversal_source_binding_invalid")?;
    let mut roots = std::collections::BTreeSet::new();
    for node in &template.nodes {
        for context in std::iter::once(&node.context).chain(node.input_contexts.values()) {
            let actual = match context {
                crate::selection_strategy::ContextTemplate::Subject { interval_ms }
                | crate::selection_strategy::ContextTemplate::Selected { interval_ms, .. } => {
                    *interval_ms
                }
                crate::selection_strategy::ContextTemplate::Literal { context } => {
                    context.interval_ms
                }
            };
            if actual != Some(interval) {
                return Err("management_reversal_context_interval_mismatch".into());
            }
        }
        let selected = resolved_graph
            .nodes
            .iter()
            .find(|entry| entry.node_id == node.graph_node_id)
            .and_then(|entry| entry.selected.as_ref())
            .ok_or("management_reversal_source_binding_invalid")?;
        let pin = crate::service::data_binding::pin(s, selected)?;
        let metadata = s
            .runtime()
            .plugin_packages
            .operation_data(&pin.package_sha256, &pin.operation_id)?
            .ok_or("management_reversal_operation_contract_missing")?;
        // A stateless input-free provider has no formula state to reconstruct.
        // Keep all declared V3 semantics when present; transforms still require
        // qualified V3 history/state and sources must prove complete captures.
        if metadata.inputs.is_empty() && metadata.state.is_none() {
            roots.insert(node.graph_node_id.clone());
        } else if metadata.v3().is_none() {
            return Err("management_reversal_v3_history_contract_required".into());
        }
    }
    if roots != config.source_bindings.keys().cloned().collect() {
        return Err("management_reversal_source_binding_invalid".into());
    }
    // The producer is the one registry for verified package/contract/pin rules.
    // Its helper independently requires exact root source binding and schema.
    for (root, source) in &config.source_bindings {
        crate::service::managed_data_streams::validate_automatic_binding(
            s,
            scope,
            graph,
            root,
            &binding_value(source)?,
        )?;
    }
    Ok(())
}

pub(super) fn validate_entry_sources(
    s: &TradeAssemblyService,
    plan: &crate::service::data_plans::StoredDataPlan,
    config: &Config,
) -> Result<(), String> {
    config.validate()?;
    if plan.content.plan.request.trade_instruments.len() != 1 {
        return Err("management_reversal_single_trade_subject_required".into());
    }
    let owned_trade = &plan.content.plan.request.trade_instruments[0].instrument_id;
    let roots = plan
        .content
        .plan
        .contracts
        .iter()
        .filter(|c| c.contract.inputs.is_empty())
        .map(|c| c.graph_node_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if roots != config.source_bindings.keys().cloned().collect() {
        return Err("management_reversal_source_binding_invalid".into());
    }
    for (root, binding) in &config.source_bindings {
        let Some(crate::service::data_captures::SourceAuthentication::HostAcquisition {
            capture_id,
            ..
        }) = plan.content.source_authentication.get(root)
        else {
            return Err("management_reversal_original_capture_required".into());
        };
        let capture = crate::service::data_captures::current(s, capture_id)?;
        crate::service::data_captures::verified_input_witness(&capture)?;
        let source: crate::service::managed_data_streams::Source =
            serde_json::from_value(binding_value(binding)?)
                .map_err(|_| "management_reversal_source_binding_invalid")?;
        crate::service::managed_data_streams::validate_capture_input(
            &source,
            &capture,
            capture.policy.event_start_ms,
            owned_trade,
        )?;

        if capture.policy.scope != plan.content.scope
            || capture.policy.replay_node != *root
            || capture.policy.acquisition_node != binding.acquisition_node
            || capture.policy.context.interval_ms != Some(binding.interval_ms)
            || capture.policy.parameter_extracts != binding.parameter_extracts
            || match &binding.origin {
                Origin::Parameter(origin) => {
                    capture
                        .acquisition_response
                        .payload
                        .pointer(&origin.payload_pointer)
                        .and_then(Value::as_i64)
                        != Some(capture.policy.event_start_ms)
                        || capture
                            .replay_parameters
                            .get(&origin.replay_parameter)
                            .and_then(Value::as_i64)
                            != Some(capture.policy.event_start_ms)
                }
                Origin::Admitted(AdmittedOrigin::AdmittedCaptureStart {}) => false,
            }
        {
            return Err("management_reversal_original_origin_or_source_mismatch".into());
        }
    }
    Ok(())
}

fn finite_child(state: &mut State, path: &str, body: Value, step: Action) -> Result<Child, String> {
    child(state, path, body, super::Action::Reversal { step })
}

pub(super) fn next_child(
    s: &TradeAssemblyService,
    config: &Config,
    state: &mut State,
    entry: &OrderPlan,
    position: &Position,
) -> Result<WindowStep, String> {
    config.validate_reversal_age(position.policy.max_reversal_age_ms)?;
    if position.policy.rules.reversal_signal.as_deref() != Some("typed.exit") {
        return Err("management_reversal_policy_binding_invalid".into());
    }
    let id = &position.position_id;
    if !state.reversal_heads.contains_key(id) {
        if state.reversal_heads.len() >= MAX_ENTRIES {
            return Err("management_reversal_position_limit".into());
        }
        state.reversal_heads.insert(
            id.clone(),
            Progress {
                streams: BTreeMap::new(),
                pending_captures: BTreeMap::new(),
                plan_revision_id: None,
                run_id: format!("managed-reversal:{}", hash(&(id, &state.config_sha256))?),
                cursor_ms: None,
                checkpoint_sha256: None,
                signal_receipt_sha256: None,
                waiting_for_input: false,
            },
        );
    }
    let progress = state
        .reversal_heads
        .get(id)
        .cloned()
        .ok_or("management_reversal_state_missing")?;
    for (root, binding) in &config.source_bindings {
        if !progress.streams.contains_key(root) {
            let mut stream = binding_value(binding)?;
            stream["schemaVersion"] = json!("tradeassembly.managed_data_stream_create.v1");
            stream["positionId"] = json!(id);
            stream["replayNode"] = json!(root);
            return finite_child(
                state,
                CREATE_STREAM,
                json!({"scope":entry.command.scope,"stream":stream}),
                Action::CreateStream {
                    source: root.clone(),
                },
            )
            .map(WindowStep::Child);
        }
    }
    for (root, head) in &progress.streams {
        let source = config
            .source_bindings
            .get(root)
            .ok_or("management_reversal_source_binding_invalid")?;
        let actual = crate::service::managed_data_streams::binding(
            s,
            &head.stream_id,
            &entry.command.scope,
        )?;
        if hash(&actual.source.input_template)? != hash(&source.input)?
            || hash(&actual)? != head.binding_sha256
        {
            return Err("management_reversal_stream_input_template_changed".into());
        }
    }
    let first = progress
        .streams
        .values()
        .next()
        .ok_or("management_reversal_source_binding_invalid")?;
    if progress.streams.len() != config.source_bindings.len()
        || progress.streams.iter().any(|(root, head)| {
            !config.source_bindings.contains_key(root)
                || head.interval_ms != first.interval_ms
                || head.original_origin_ms != first.original_origin_ms
                || head.interval_ms != config.source_bindings[root].interval_ms
        })
    {
        return Err("management_reversal_context_interval_or_grid_mismatch".into());
    }
    if progress.plan_revision_id.is_none() {
        let streams = progress
            .streams
            .iter()
            .map(|(root, head)| (root.clone(), head.stream_id.clone()))
            .collect::<BTreeMap<_, _>>();
        return finite_child(state,crate::service::selection_plans::MANAGED_PATH,json!({
            "managedPositionPlan":{"schemaVersion":"tradeassembly.managed_position_plan.v2",
                "scope":entry.command.scope,"managedPositionId":id,"selectionReceipts":{},"sourceStreams":streams}
        }),Action::PreparePlan {}).map(WindowStep::Child);
    }
    // Bootstrap and catch-up steps consume already admitted source heads before
    // another acquisition. State/cursors remain the normal atomic evaluator's.
    let cursor = progress.cursor_ms.unwrap_or(first.original_origin_ms);
    let available = progress
        .streams
        .values()
        .map(|head| head.event_end_ms)
        .min()
        .ok_or("management_reversal_state_missing")?;
    if cursor < available && !progress.waiting_for_input {
        let now = s.runtime().clock.trusted_now_ms()?;
        let window = step_window(
            cursor,
            available,
            first.interval_ms,
            config.max_frames_per_step,
            now,
        )?;
        let heads = progress
            .streams
            .iter()
            .map(|(root, head)| (root.clone(), head.head_sha256.clone()))
            .collect::<BTreeMap<_, _>>();
        return finite_child(
            state,
            MANAGED_STEP,
            json!({"scope":entry.command.scope,"managedStep":{
                "schemaVersion":"tradeassembly.managed_stream_evaluation_step.v1",
                "step":{"runId":progress.run_id,"planRevisionId":progress.plan_revision_id,
                    "window":window,"expectedCheckpointSha256":progress.checkpoint_sha256},
                "streamHeads":heads,"contextSelectionReceipts":{}
            }}),
            Action::Evaluate {},
        )
        .map(WindowStep::Child);
    }
    let now = s.runtime().clock.trusted_now_ms()?;
    let latest = completed_boundary(
        first.original_origin_ms,
        first.interval_ms,
        now,
        config.completion_lag_ms,
    )?;
    for (root, capture) in &progress.pending_captures {
        let head = progress
            .streams
            .get(root)
            .ok_or("management_reversal_state_missing")?;
        return finite_child(state,APPEND_STREAM,json!({"scope":entry.command.scope,"append":{
            "schemaVersion":"tradeassembly.managed_data_stream_append.v1","streamId":head.stream_id,
            "expectedHeadSha256":head.head_sha256,"captureId":capture
        }}),Action::AppendSegment { source:root.clone() }).map(WindowStep::Child);
    }
    // Catch up the oldest admitted source first; one fast-changing source must
    // never starve the common frame barrier's other required inputs.
    let mut roots = config.source_bindings.keys().collect::<Vec<_>>();
    roots.sort_by_key(|root| (progress.streams[*root].event_end_ms, (*root).clone()));
    for root in roots {
        let binding = &config.source_bindings[root];
        let head = &progress.streams[root];
        if head.event_end_ms < latest {
            let remaining = ((latest - head.event_end_ms) as u64 / head.interval_ms)
                .min(config.max_segment_observations);
            let end = head
                .event_end_ms
                .checked_add(
                    i64::try_from(
                        remaining
                            .checked_mul(head.interval_ms)
                            .ok_or("management_reversal_grid_invalid")?,
                    )
                    .map_err(|_| "management_reversal_grid_invalid")?,
                )
                .ok_or("management_reversal_grid_invalid")?;
            let window = tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
                event_start_ms: head.event_end_ms,
                event_end_ms: end,
                as_of_ms: now,
            };
            let input = binding.materialize_input(
                &entry.order.instrument.instrument_id,
                &window,
                head.original_origin_ms,
            )?;
            return finite_child(state,"/product/strategies/data-captures/create",json!({"capture":{
                "receiptVersion":"tradeassembly.data_capture_receipt.v2",
                "policy":{"scope":entry.command.scope,"capabilityGraphRevision":entry.command.capability_graph_revision,
                    "mode":"paper","acquisitionNode":binding.acquisition_node,"replayNode":root,
                    "parameterExtracts":binding.parameter_extracts,"context":head.context,
                    "eventStartMs":window.event_start_ms,"eventEndMs":window.event_end_ms,
                    "gapPolicy":"strict_fixed_interval"},"input":input
            }}),Action::CaptureSegment { source:root.clone() }).map(WindowStep::Child);
        }
    }
    if let Some(receipt) = progress.signal_receipt_sha256 {
        // This existing authoritative guard checks actual ready exit, current
        // source/window/clocks and post-entry causal watermark; never fake false.
        reversal(s, position, entry, Some(&receipt), now)?;
        return Ok(WindowStep::Ready(receipt));
    }
    if progress.waiting_for_input {
        Err("management_reversal_waiting_for_input".into())
    } else {
        Err("management_reversal_warming_up".into())
    }
}

pub(super) fn apply_response(
    s: &TradeAssemblyService,
    binding: &Binding,
    state: &mut State,
    child: &Child,
    action: &Action,
    response: &ServiceResponse,
) -> Result<(), String> {
    let work = state
        .work
        .get(state.cursor)
        .ok_or("management_automation_work_missing")?;
    if work.kind != Kind::Position {
        return Err("management_reversal_position_required".into());
    }
    let (_, position) = load(s, &work.id)?;
    let entry = read_owned(s, &position.entry_plan_id)?;
    if position.activation_id != binding.activation
        || entry.command.scope
            != serde_json::from_value::<data_plans::PlanScope>(child.body["scope"].clone())
                .map_err(|_| "management_reversal_scope_invalid")?
    {
        return Err("management_reversal_child_scope_mismatch".into());
    }
    let config = binding
        .config
        .reversal_evaluation
        .as_ref()
        .ok_or("management_reversal_disabled")?;
    let progress = state
        .reversal_heads
        .get_mut(&work.id)
        .ok_or("management_reversal_state_missing")?;
    match action {
        Action::CreateStream { source } => {
            let source_config = config
                .source_bindings
                .get(source)
                .ok_or("management_reversal_source_binding_invalid")?;
            let view = stream_view(&response.body["stream"])?;
            if view.interval_ms != source_config.interval_ms
                || child.body["stream"]["replayNode"] != *source
                || child.body["stream"]["positionId"] != position.position_id
                || child.body["stream"]["acquisitionNode"] != source_config.acquisition_node
                || progress.streams.contains_key(source)
            {
                return Err("management_reversal_child_result_binding_invalid".into());
            }
            progress.streams.insert(source.clone(), view);
        }
        Action::CaptureSegment { source } => {
            let returned: crate::service::data_captures::CaptureReceipt =
                serde_json::from_value(response.body["capture"].clone())
                    .map_err(|_| "management_reversal_capture_result_invalid")?;
            let requested: crate::service::data_captures::CaptureCommand =
                serde_json::from_value(child.body["capture"].clone())
                    .map_err(|_| "management_reversal_capture_request_invalid")?;
            let actual = crate::service::data_captures::current(s, &returned.capture_id)?;
            let head = progress
                .streams
                .get(source)
                .ok_or("management_reversal_state_missing")?;
            if returned != actual
                || actual.policy != requested.policy
                || actual.policy.scope != entry.command.scope
                || actual.policy.replay_node != *source
                || actual.policy.event_start_ms != head.event_end_ms
                || actual.policy.context != head.context
            {
                return Err("management_reversal_capture_result_binding_invalid".into());
            }
            progress
                .pending_captures
                .insert(source.clone(), actual.capture_id);
        }
        Action::AppendSegment { source } => {
            let before = progress
                .streams
                .get(source)
                .ok_or("management_reversal_state_missing")?;
            let view = stream_view(&response.body["stream"])?;
            let capture = progress
                .pending_captures
                .get(source)
                .ok_or("management_reversal_capture_missing")?;
            let actual = crate::service::data_captures::current(s, capture)?;
            if child.body["append"]["streamId"] != before.stream_id
                || child.body["append"]["expectedHeadSha256"] != before.head_sha256
                || child.body["append"]["captureId"] != *capture
                || view.stream_id != before.stream_id
                || view.binding_sha256 != before.binding_sha256
                || view.seed_capture_id != before.seed_capture_id
                || view.original_origin_ms != before.original_origin_ms
                || view.interval_ms != before.interval_ms
                || view.context != before.context
                || view.event_end_ms != actual.policy.event_end_ms
                || actual.policy.event_start_ms != before.event_end_ms
                || view.event_end_ms <= before.event_end_ms
                || view.head_sha256 == before.head_sha256
            {
                return Err("management_reversal_append_result_binding_invalid".into());
            }
            progress.streams.insert(source.clone(), view);
            progress.pending_captures.remove(source);
            progress.signal_receipt_sha256 = None;
            progress.waiting_for_input = false;
        }
        Action::PreparePlan {} => {
            let returned: crate::service::data_plans::StoredDataPlan =
                serde_json::from_value(response.body["plan"].clone())
                    .map_err(|_| "management_reversal_plan_result_invalid")?;
            let actual = crate::service::data_plans::current(s, &returned.revision_id)?;
            if returned != actual
                || actual.content.scope != entry.command.scope
                || actual
                    .content
                    .selection_source
                    .as_ref()
                    .and_then(|v| v.managed_position_id.as_deref())
                    != Some(position.position_id.as_str())
                || crate::service::data_strategy::program_for_content(s, &actual.content)?
                    .is_none_or(|p| {
                        p.entry
                            != (crate::data_strategy::Rule::IsTrue {
                                value: crate::data_strategy::Operand::Boolean { value: false },
                            })
                    })
            {
                return Err("management_reversal_plan_result_binding_invalid".into());
            }
            progress.plan_revision_id = Some(actual.revision_id);
        }
        Action::Evaluate {} => {
            let requested: crate::service::data_evaluation::EvaluationStep =
                serde_json::from_value(child.body["managedStep"]["step"].clone())
                    .map_err(|_| "management_reversal_step_request_invalid")?;
            let returned: crate::service::data_evaluation::EvaluationReceipt =
                serde_json::from_value(response.body["receipt"].clone())
                    .map_err(|_| "management_reversal_step_result_invalid")?;
            let (plan, actual) =
                crate::service::data_evaluation::indexed_receipt(s, &returned.receipt_sha256)?;
            if returned != actual
                || Some(&plan.revision_id) != progress.plan_revision_id.as_ref()
                || requested.run_id != progress.run_id
                || requested.plan_revision_id != plan.revision_id
                || requested.expected_checkpoint_sha256 != progress.checkpoint_sha256
                || actual.checkpoint.run_id != requested.run_id
                || actual.checkpoint.plan_revision_id != plan.revision_id
                || actual.request_authority.as_ref().is_none_or(|authority| {
                    hash(&json!({"request":requested,"authority":authority}))
                        .ok()
                        .as_deref()
                        != Some(actual.request_sha256.as_str())
                })
            {
                return Err("management_reversal_step_result_binding_invalid".into());
            }
            let old_cursor = progress
                .cursor_ms
                .unwrap_or_else(|| progress.streams.values().next().unwrap().original_origin_ms);
            let cursors = actual
                .checkpoint
                .cursors
                .values()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            let cursor = if actual.frames_processed == 0 {
                old_cursor
            } else {
                if cursors.len() != 1 {
                    return Err("management_reversal_source_cursor_mismatch".into());
                }
                *cursors
                    .first()
                    .ok_or("management_reversal_source_cursor_mismatch")?
            };
            if cursor < old_cursor
                || cursor > requested.window.event_end_ms
                || requested.window.event_start_ms != old_cursor
            {
                return Err("management_reversal_source_cursor_mismatch".into());
            }
            progress.checkpoint_sha256 = Some(actual.checkpoint.checkpoint_sha256.clone());
            progress.cursor_ms = Some(cursor);
            progress.waiting_for_input = cursor == old_cursor;
            progress.signal_receipt_sha256 = None;
            if let Some(frame) = actual.signals.last() {
                if frame.event_end_ms == cursor
                    && frame.entry == (crate::data_strategy::SignalValue::Ready { value: false })
                    && matches!(frame.exit, crate::data_strategy::SignalValue::Ready { .. })
                    && frame.trade_instruments.len() == 1
                    && frame.trade_instruments[0].instrument_id
                        == entry.order.instrument.instrument_id
                {
                    progress.signal_receipt_sha256 = Some(actual.receipt_sha256);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn unknown_response(response: &ServiceResponse) -> bool {
    if response.status >= 500 {
        return true;
    }
    crate::service::service_error_code(&response.body).is_some_and(|code| {
        code == "idempotency_in_progress"
            || code.contains("uncertain")
            || code.contains("reconciliation_required")
    })
}
