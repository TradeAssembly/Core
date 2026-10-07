use super::canonical::{canonical_bytes, canonical_hash};
use super::diagnostic::{Diagnostic, ValidationReport};
use super::migration::{detect_source_family, StrategySpecSourceFamily};
use super::model::*;
use cel_interpreter::{Context, Program, Value as CelValue};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const ENVELOPE_NAMESPACES: &[&str] = &[
    "run",
    "assets",
    "market_data",
    "indicators",
    "signals",
    "intent",
    "sizing",
    "risk",
    "execution",
    "positions",
    "allocations",
    "state",
    "audit",
    "extensions",
];

pub fn validate(payload: &Value) -> ValidationReport {
    match canonical_bytes(payload) {
        Ok(bytes) => validate_bytes(&bytes),
        Err(error) => ValidationReport {
            valid: false,
            source_family: None,
            spec_hash: None,
            diagnostics: vec![Diagnostic::error(1, "SPEC_JSON_INVALID", "", error)],
        },
    }
}

pub fn validate_bytes(bytes: &[u8]) -> ValidationReport {
    let value = match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => value,
        Err(error) => {
            return ValidationReport {
                valid: false,
                source_family: None,
                spec_hash: None,
                diagnostics: vec![Diagnostic::error(
                    1,
                    "SPEC_JSON_INVALID",
                    "",
                    format!(
                        "invalid JSON at line {} column {}: {error}",
                        error.line(),
                        error.column()
                    ),
                )],
            };
        }
    };
    let family = detect_source_family(&value);
    let mut diagnostics = runtime_boundary_diagnostics(&value);
    if family != StrategySpecSourceFamily::V3 {
        let (code, message) = match family {
            StrategySpecSourceFamily::PredecessorV2 => (
                "SPEC_V2_IMPORT_REQUIRED",
                "predecessor StrategyContractV2 is an import source and cannot be published",
            ),
            StrategySpecSourceFamily::SkeletalTradeAssemblyV2 => (
                "SPEC_V2_IMPORT_REQUIRED",
                "skeletal TradeAssembly V2 is an import source and cannot be published",
            ),
            StrategySpecSourceFamily::AmbiguousV2 => (
                "SPEC_SOURCE_AMBIGUOUS",
                "payload matches both V2 families; select the exact source before migration",
            ),
            StrategySpecSourceFamily::Unknown => (
                "SPEC_VERSION_UNSUPPORTED",
                "payload is not the literal StrategySpec V3 contract",
            ),
            StrategySpecSourceFamily::V3 => unreachable!(),
        };
        diagnostics.push(Diagnostic::error(1, code, "/spec_version", message));
        sort_diagnostics(&mut diagnostics);
        return ValidationReport {
            valid: false,
            source_family: Some(family.as_str().to_string()),
            spec_hash: None,
            diagnostics,
        };
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let spec: StrategySpec = match serde_path_to_error::deserialize(&mut deserializer) {
        Ok(spec) => spec,
        Err(error) => {
            diagnostics.push(Diagnostic::error(
                2,
                "SPEC_STRUCTURE_INVALID",
                serde_path_to_pointer(&error.path().to_string()),
                stable_structure_message(error.inner()),
            ));
            sort_diagnostics(&mut diagnostics);
            return ValidationReport {
                valid: false,
                source_family: Some(family.as_str().to_string()),
                spec_hash: None,
                diagnostics,
            };
        }
    };

    validate_core_semantics(&spec, &mut diagnostics);
    validate_dataflow(&spec, &mut diagnostics);
    validate_expressions(&spec, &mut diagnostics);
    validate_capabilities(&spec, &mut diagnostics);
    sort_diagnostics(&mut diagnostics);
    let valid = diagnostics.is_empty();
    ValidationReport {
        valid,
        source_family: Some(family.as_str().to_string()),
        spec_hash: valid.then(|| canonical_hash(&value).expect("parsed JSON canonicalizes")),
        diagnostics,
    }
}

fn stable_structure_message(error: &serde_json::Error) -> String {
    let message = error.to_string();
    message
        .rsplit_once(" at line ")
        .map(|(stable, _location)| stable.to_string())
        .unwrap_or(message)
}

pub fn runtime_boundary_diagnostics(value: &Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_runtime_boundary(value, "", &mut diagnostics);
    diagnostics
}

fn collect_runtime_boundary(value: &Value, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_pointer = format!("{pointer}/{}", escape_pointer(key));
                if is_reserved_authority_key(key, &child_pointer) {
                    diagnostics.push(Diagnostic::error(
                        3,
                        "SPEC_AUTHORITY_RESERVED",
                        child_pointer.clone(),
                        "runtime authority or account-specific configuration is not portable StrategySpec data",
                    ));
                }
                collect_runtime_boundary(child, &child_pointer, diagnostics);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_runtime_boundary(child, &format!("{pointer}/{index}"), diagnostics);
            }
        }
        _ => {}
    }
}

fn is_reserved_authority_key(key: &str, pointer: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if pointer == "/allocation/mode" {
        return false;
    }

    const RESERVED_SUFFIXES: &[&str] = &[
        "credential",
        "credentials",
        "credentialref",
        "credentialhandle",
        "provider",
        "providerref",
        "broker",
        "brokerref",
        "plugin",
        "pluginref",
        "plugininstance",
        "plugininstanceid",
        "account",
        "accountref",
        "accountid",
        "brokeraccount",
        "brokeraccountid",
        "venueaccount",
        "accountbinding",
        "accountbindingid",
        "activation",
        "activationid",
        "acknowledgement",
        "acknowledgementid",
        "acknowledgementids",
        "liveauthority",
        "entitlement",
        "entitlements",
        "rolegrant",
        "mandate",
        "legalapproval",
        "riskbudget",
        "riskscale",
        "capital",
        "maxnotional",
        "targetnotional",
        "leverage",
        "buyingpower",
        "executionmode",
        "runtimehealth",
        "runtimecursor",
        "runtimestate",
        "leaseid",
        "orderid",
        "fillid",
        "positionid",
    ];

    normalized == "mode"
        || key
            .rsplit('.')
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case("mode"))
        || RESERVED_SUFFIXES
            .iter()
            .any(|reserved| normalized == *reserved || normalized.ends_with(reserved))
}

fn validate_core_semantics(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    require_nonempty(&spec.strategy_id, "/strategy_id", diagnostics);
    require_nonempty(&spec.name, "/name", diagnostics);
    unique_strings(
        spec.labels.iter().map(String::as_str),
        "/labels",
        "SPEC_DUPLICATE_LABEL",
        diagnostics,
    );

    let mut signal_ids = BTreeSet::new();
    for (index, market) in spec.signal_market_requirements.iter().enumerate() {
        let pointer = format!("/signal_market_requirements/{index}");
        unique_id(
            &mut signal_ids,
            &market.requirement_id,
            &format!("{pointer}/requirement_id"),
            diagnostics,
        );
        if market.data_shapes.is_empty() || market.fields.is_empty() || market.timeframes.is_empty()
        {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_SIGNAL_MARKET_INCOMPLETE",
                pointer.clone(),
                "signal market requires data shapes, fields, and timeframes",
            ));
        }
        if market.selectors.is_empty() {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_SIGNAL_SELECTOR_REQUIRED",
                format!("{pointer}/selectors"),
                "signal market must declare a normalized, symbolic, or dynamic selector",
            ));
        }
        for (selector_index, selector) in market.selectors.iter().enumerate() {
            let selector_pointer = format!("{pointer}/selectors/{selector_index}");
            let shape_count = [
                selector.normalized_id.is_some(),
                selector.alias.is_some(),
                selector.expression.is_some(),
            ]
            .into_iter()
            .filter(|set| *set)
            .count();
            if shape_count != 1 {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_SELECTOR_SHAPE_INVALID",
                    selector_pointer,
                    "selector must set exactly one identity or dynamic expression",
                ));
            }
        }
    }
    if signal_ids.is_empty() {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_SIGNAL_MARKET_REQUIRED",
            "/signal_market_requirements",
            "at least one signal market requirement is required",
        ));
    }

    let mut trade_ids = BTreeSet::new();
    let capability_ids = capability_ids(spec);
    for (index, market) in spec.trade_market_requirements.iter().enumerate() {
        let pointer = format!("/trade_market_requirements/{index}");
        unique_id(
            &mut trade_ids,
            &market.requirement_id,
            &format!("{pointer}/requirement_id"),
            diagnostics,
        );
        if market.asset_classes.is_empty() || market.instrument_families.is_empty() {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_TRADE_MARKET_INCOMPLETE",
                pointer.clone(),
                "trade market requires asset classes and instrument families",
            ));
        }
        if !(market.supports_single_leg
            || market.supports_multi_leg
            || market.supports_mixed_instruments)
        {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_TRADE_SHAPE_REQUIRED",
                pointer.clone(),
                "trade market must permit at least one leg shape",
            ));
        }
        for (ref_index, reference) in market.signal_requirement_refs.iter().enumerate() {
            if !signal_ids.contains(reference) {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_SIGNAL_REF_MISSING",
                    format!("{pointer}/signal_requirement_refs/{ref_index}"),
                    format!("unknown signal market requirement '{reference}'"),
                ));
            }
        }
        for (ref_index, reference) in market.required_capability_refs.iter().enumerate() {
            if !capability_ids.contains(reference) {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_CAPABILITY_REF_MISSING",
                    format!("{pointer}/required_capability_refs/{ref_index}"),
                    format!("unknown capability requirement '{reference}'"),
                ));
            }
        }
    }
    if trade_ids.is_empty() {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_TRADE_MARKET_REQUIRED",
            "/trade_market_requirements",
            "at least one trade market requirement is required",
        ));
    }

    validate_variables(spec, diagnostics);
    validate_allocation(spec, &capability_ids, diagnostics);
    validate_stage_semantics(spec, &capability_ids, diagnostics);

    if spec.portfolio_scope == PortfolioScope::Account
        && !spec
            .capability_requirements
            .required
            .iter()
            .any(|requirement| requirement.capability == "portfolio.account.read@1")
    {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_ACCOUNT_SCOPE_CAPABILITY_REQUIRED",
            "/portfolio_scope",
            "account scope requires portfolio.account.read@1",
        ));
    }
}

fn validate_variables(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    for (id, variable) in &spec.variables {
        let pointer = format!("/variables/{}", escape_pointer(id));
        require_nonempty(id, &pointer, diagnostics);
        let type_ok = match variable.variable_type {
            VariableType::Integer => variable.default.as_i64().is_some(),
            VariableType::Decimal => variable.default.as_str().is_some_and(valid_decimal_string),
            VariableType::Boolean => variable.default.is_boolean(),
            VariableType::String => variable.default.is_string(),
            VariableType::Duration => variable
                .default
                .as_str()
                .is_some_and(|value| !value.trim().is_empty()),
            VariableType::Enum => variable
                .default
                .as_str()
                .is_some_and(|value| variable.options.iter().any(|item| item == value)),
            VariableType::Json => true,
        };
        if !type_ok {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_VARIABLE_DEFAULT_TYPE",
                format!("{pointer}/default"),
                "default does not match the declared variable type",
            ));
        }
        if let Some(substep_ref) = &variable.substep_ref {
            if !stage_has_substep(spec.stages.view(&variable.stage_ref), substep_ref) {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_SUBSTEP_REF_MISSING",
                    format!("{pointer}/substep_ref"),
                    format!("unknown substep '{substep_ref}' in referenced stage"),
                ));
            }
        }
    }
}

fn validate_allocation(
    spec: &StrategySpec,
    capability_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let allocation = &spec.allocation;
    let valid = match allocation.mode {
        AllocationMode::None => {
            allocation.targets.is_empty()
                && allocation.formula.is_none()
                && allocation.operation_ref.is_none()
                && allocation.capability_requirement_refs.is_empty()
        }
        AllocationMode::RelativeWeights => {
            !allocation.targets.is_empty()
                && allocation.targets.iter().all(|target| {
                    target
                        .relative_weight
                        .as_deref()
                        .is_some_and(valid_decimal_string)
                })
        }
        AllocationMode::Formula => allocation.formula.is_some(),
        AllocationMode::Operation => {
            allocation.operation_ref.is_some()
                && !allocation.capability_requirement_refs.is_empty()
                && allocation
                    .capability_requirement_refs
                    .iter()
                    .all(|reference| capability_ids.contains(reference))
        }
    };
    if !valid {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_ALLOCATION_INVALID",
            "/allocation",
            "allocation fields do not match the selected portable allocation mode",
        ));
    }
}

fn validate_stage_semantics(
    spec: &StrategySpec,
    capability_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for stage_name in StageName::VALIDATION_ORDER {
        let stage = spec.stages.view(&stage_name);
        let pointer = format!("/stages/{}", stage_name.as_str());
        if matches!(
            stage_name,
            StageName::Risk
                | StageName::OrderStrategy
                | StageName::PricePolicy
                | StageName::MarketClock
        ) && !stage.enabled
        {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_REQUIRED_STAGE_DISABLED",
                format!("{pointer}/enabled"),
                "publication requires this fail-closed stage to be enabled",
            ));
        }
        for (field, schema_ref) in [
            ("input_schema_ref", stage.input_schema_ref),
            ("output_schema_ref", stage.output_schema_ref),
        ] {
            if schema_ref.is_some_and(|reference| !valid_schema_ref(reference)) {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_SCHEMA_REF_INVALID",
                    format!("{pointer}/{field}"),
                    "schema refs must use schema://<namespace>/<name>@<version>",
                ));
            }
        }
        for key in stage.config.keys() {
            if !key.contains('.') {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_EXTENSION_NAMESPACE_REQUIRED",
                    format!("{pointer}/config/{}", escape_pointer(key)),
                    "open configuration keys must use a registered namespace",
                ));
            }
        }
        let mut substep_ids = BTreeSet::new();
        for (index, substep) in stage.substeps.iter().enumerate() {
            let substep_pointer = format!("{pointer}/substeps/{index}");
            unique_id(
                &mut substep_ids,
                &substep.id,
                &format!("{substep_pointer}/id"),
                diagnostics,
            );
            for (field, schema_ref) in [
                ("input_schema_ref", substep.input_schema_ref.as_deref()),
                ("output_schema_ref", substep.output_schema_ref.as_deref()),
            ] {
                if schema_ref.is_some_and(|reference| !valid_schema_ref(reference)) {
                    diagnostics.push(Diagnostic::error(
                        3,
                        "SPEC_SCHEMA_REF_INVALID",
                        format!("{substep_pointer}/{field}"),
                        "schema refs must use schema://<namespace>/<name>@<version>",
                    ));
                }
            }
            if substep.failure_policy == FailurePolicy::SkipOptional && !substep.optional {
                diagnostics.push(Diagnostic::error(
                    3,
                    "SPEC_FAILURE_POLICY_INVALID",
                    format!("{substep_pointer}/failure_policy"),
                    "skip_optional is permitted only for optional substeps",
                ));
            }
            for (ref_index, reference) in substep.capability_requirement_refs.iter().enumerate() {
                if !capability_ids.contains(reference) {
                    diagnostics.push(Diagnostic::error(
                        3,
                        "SPEC_CAPABILITY_REF_MISSING",
                        format!("{substep_pointer}/capability_requirement_refs/{ref_index}"),
                        format!("unknown capability requirement '{reference}'"),
                    ));
                }
            }
            for key in substep.config.keys() {
                if !key.contains('.') {
                    diagnostics.push(Diagnostic::error(
                        3,
                        "SPEC_EXTENSION_NAMESPACE_REQUIRED",
                        format!("{substep_pointer}/config/{}", escape_pointer(key)),
                        "open configuration keys must use a registered namespace",
                    ));
                }
            }
            if let Some(state) = &substep.state {
                if !valid_schema_ref(&state.schema_ref) {
                    diagnostics.push(Diagnostic::error(
                        3,
                        "SPEC_SCHEMA_REF_INVALID",
                        format!("{substep_pointer}/state/schema_ref"),
                        "state schema refs must use schema://<namespace>/<name>@<version>",
                    ));
                }
                if state.sharing_scope == StateSharingScope::Account {
                    let authorized = state
                        .sharing_authorization_capability_ref
                        .as_ref()
                        .is_some_and(|reference| capability_ids.contains(reference));
                    if spec.portfolio_scope != PortfolioScope::Account || !authorized {
                        diagnostics.push(Diagnostic::error(
                            3,
                            "SPEC_ACCOUNT_STATE_CAPABILITY_REQUIRED",
                            format!("{substep_pointer}/state/sharing_scope"),
                            "account-shared state requires account portfolio scope and an explicit declared capability; runtime authority is still required",
                        ));
                    }
                }
            }
        }
    }
    validate_exit_policy(spec, capability_ids, diagnostics);
    validate_order_policy(spec, diagnostics);
    validate_price_policy(spec, diagnostics);
    validate_clock_policy(spec, capability_ids, diagnostics);
}

fn validate_exit_policy(
    spec: &StrategySpec,
    capability_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let policy = &spec.stages.exit_policy.policy;
    if policy.monitoring.is_empty() || policy.triggers.is_empty() {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_EXIT_POLICY_INVALID",
            "/stages/exit_policy/policy",
            "exit policy requires monitoring modes and typed triggers",
        ));
    }
    let ids = policy
        .triggers
        .iter()
        .map(|trigger| trigger.id.as_str())
        .collect::<BTreeSet<_>>();
    if policy.composition == ExitComposition::Priority
        && (policy.precedence.is_empty()
            || policy
                .precedence
                .iter()
                .any(|reference| !ids.contains(reference.as_str())))
    {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_EXIT_PRECEDENCE_INVALID",
            "/stages/exit_policy/policy/precedence",
            "priority composition must list only declared trigger IDs",
        ));
    }
    for (index, trigger) in policy.triggers.iter().enumerate() {
        if trigger.kind == ExitTriggerKind::Expression && trigger.expression.is_none() {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_EXIT_EXPRESSION_REQUIRED",
                format!("/stages/exit_policy/policy/triggers/{index}/expression"),
                "expression trigger requires a typed expression",
            ));
        }
        if trigger.kind == ExitTriggerKind::Operation
            && !trigger
                .capability_requirement_ref
                .as_ref()
                .is_some_and(|reference| capability_ids.contains(reference))
        {
            diagnostics.push(Diagnostic::error(
                3,
                "SPEC_EXIT_CAPABILITY_REQUIRED",
                format!("/stages/exit_policy/policy/triggers/{index}/capability_requirement_ref"),
                "operation trigger requires a declared capability requirement",
            ));
        }
    }
}

fn validate_order_policy(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    let policy = &spec.stages.order_strategy.policy;
    if policy.allowed_order_types.is_empty()
        || (policy.allowed_order_types.contains(&OrderType::MultiLeg)
            && policy.multi_leg_mode == MultiLegMode::NotApplicable)
    {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_ORDER_POLICY_INVALID",
            "/stages/order_strategy/policy",
            "order policy requires allowed types and an explicit compatible multi-leg mode",
        ));
    }
}

fn validate_price_policy(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    let policy = &spec.stages.price_policy.policy;
    if policy.snapshot_inputs.is_empty()
        || policy.source_precedence.is_empty()
        || policy.max_staleness.trim().is_empty()
    {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_PRICE_POLICY_INVALID",
            "/stages/price_policy/policy",
            "price policy requires snapshot inputs, source precedence, and staleness",
        ));
    }
    for (index, path) in policy.snapshot_inputs.iter().enumerate() {
        validate_envelope_path(
            path,
            &format!("/stages/price_policy/policy/snapshot_inputs/{index}"),
            diagnostics,
        );
    }
}

fn validate_clock_policy(
    spec: &StrategySpec,
    capability_ids: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let policy = &spec.stages.market_clock.policy;
    let schedule_ref_valid = policy
        .schedule_capability_requirement_ref
        .as_ref()
        .is_none_or(|reference| capability_ids.contains(reference));
    if policy.timezone.trim().is_empty()
        || policy.calendar_refs.is_empty()
        || policy.windows.is_empty()
        || policy.days.is_empty()
        || !schedule_ref_valid
        || policy
            .windows
            .iter()
            .any(|window| window.start >= window.end)
    {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_CLOCK_POLICY_INVALID",
            "/stages/market_clock/policy",
            "clock policy requires timezone, calendar, ordered exclusive windows, days, and valid capability refs",
        ));
    }
}

#[derive(Clone)]
struct Access {
    node: String,
    stage: StageName,
    stage_index: usize,
    substep_index: usize,
    path: String,
    pointer: String,
    capability_refs: Vec<String>,
    substep_id: Option<String>,
    schema_ref: Option<String>,
}

fn validate_dataflow(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    let mut writes: BTreeMap<String, Access> = BTreeMap::new();
    let mut reads = Vec::new();
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for (stage_index, stage_name) in StageName::VALIDATION_ORDER.into_iter().enumerate() {
        let stage = spec.stages.view(&stage_name);
        let stage_node = format!("stage:{}", stage_name.as_str());
        collect_accesses(
            stage.reads,
            stage.writes,
            stage_index,
            0,
            stage_name.clone(),
            &stage_node,
            &format!("/stages/{}", stage_name.as_str()),
            &mut reads,
            &mut writes,
            diagnostics,
            &[],
            None,
            stage.input_schema_ref,
            stage.output_schema_ref,
        );
        for (substep_index, substep) in stage.substeps.iter().enumerate() {
            let node = format!("{stage_node}/{}", substep.id);
            collect_accesses(
                &substep.reads,
                &substep.writes,
                stage_index,
                substep_index + 1,
                stage_name.clone(),
                &node,
                &format!("/stages/{}/substeps/{substep_index}", stage_name.as_str()),
                &mut reads,
                &mut writes,
                diagnostics,
                &substep.capability_requirement_refs,
                Some(&substep.id),
                substep.input_schema_ref.as_deref(),
                substep.output_schema_ref.as_deref(),
            );
        }
    }

    for read in reads {
        if let Some(writer) = writes.get(&read.path) {
            graph
                .entry(writer.node.clone())
                .or_default()
                .insert(read.node.clone());
            if writer.stage_index > read.stage_index
                || (writer.stage_index == read.stage_index
                    && writer.substep_index >= read.substep_index)
            {
                diagnostics.push(Diagnostic::error(
                    4,
                    "SPEC_UNRESOLVED_READ",
                    read.pointer.clone(),
                    format!("'{}' is written only after it is read", read.path),
                ));
            }
            if let (Some(output), Some(input)) = (&writer.schema_ref, &read.schema_ref) {
                if output != input {
                    diagnostics.push(Diagnostic::error(
                        4,
                        "SPEC_SCHEMA_REF_MISMATCH",
                        schema_pointer_for_read(&read.pointer),
                        format!("reader schema '{input}' does not match writer schema '{output}'"),
                    ));
                }
            }
        } else if !declared_external_read(
            spec,
            &read.stage,
            read.substep_id.as_deref(),
            &read.capability_refs,
            &read.path,
        ) {
            diagnostics.push(Diagnostic::error(
                4,
                "SPEC_UNRESOLVED_READ",
                read.pointer.clone(),
                format!(
                    "'{}' has no earlier writer or declared external capability",
                    read.path
                ),
            ));
        }
    }
    if let Some(node) = graph_cycle(&graph) {
        diagnostics.push(Diagnostic::error(
            4,
            "SPEC_DATAFLOW_CYCLE",
            "/stages",
            format!("dataflow contains a cycle through '{node}'"),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_accesses(
    read_paths: &[String],
    write_paths: &[String],
    stage_index: usize,
    substep_index: usize,
    stage: StageName,
    node: &str,
    pointer: &str,
    reads: &mut Vec<Access>,
    writes: &mut BTreeMap<String, Access>,
    diagnostics: &mut Vec<Diagnostic>,
    capability_refs: &[String],
    substep_id: Option<&str>,
    input_schema_ref: Option<&str>,
    output_schema_ref: Option<&str>,
) {
    for (index, path) in read_paths.iter().enumerate() {
        validate_envelope_path(path, &format!("{pointer}/reads/{index}"), diagnostics);
        reads.push(Access {
            node: node.to_string(),
            stage: stage.clone(),
            stage_index,
            substep_index,
            path: path.clone(),
            pointer: format!("{pointer}/reads/{index}"),
            capability_refs: capability_refs.to_vec(),
            substep_id: substep_id.map(str::to_string),
            schema_ref: input_schema_ref.map(str::to_string),
        });
    }
    for (index, path) in write_paths.iter().enumerate() {
        let path_pointer = format!("{pointer}/writes/{index}");
        validate_envelope_path(path, &path_pointer, diagnostics);
        let access = Access {
            node: node.to_string(),
            stage: stage.clone(),
            stage_index,
            substep_index,
            path: path.clone(),
            pointer: path_pointer.clone(),
            capability_refs: capability_refs.to_vec(),
            substep_id: substep_id.map(str::to_string),
            schema_ref: output_schema_ref.map(str::to_string),
        };
        if writes.insert(path.clone(), access).is_some() {
            diagnostics.push(Diagnostic::error(
                4,
                "SPEC_DUPLICATE_WRITE",
                path_pointer,
                format!("'{path}' has more than one writer"),
            ));
        }
    }
}

fn schema_pointer_for_read(read_pointer: &str) -> String {
    read_pointer
        .split_once("/reads/")
        .map(|(owner, _)| format!("{owner}/input_schema_ref"))
        .unwrap_or_else(|| read_pointer.to_string())
}

fn declared_external_read(
    spec: &StrategySpec,
    stage: &StageName,
    substep_id: Option<&str>,
    capability_refs: &[String],
    path: &str,
) -> bool {
    let namespace = path.split('.').next().unwrap_or_default();
    if !matches!(
        namespace,
        "run" | "assets" | "market_data" | "positions" | "allocations"
    ) {
        return false;
    }
    let Some(substep_id) = substep_id else {
        return false;
    };
    capability_proves_external_read(
        &spec.capability_requirements,
        stage,
        substep_id,
        capability_refs,
        path,
    )
}

fn capability_proves_external_read(
    requirements: &CapabilityRequirements,
    stage: &StageName,
    substep_id: &str,
    capability_refs: &[String],
    path: &str,
) -> bool {
    requirements
        .required
        .iter()
        .chain(&requirements.optional)
        .any(|requirement| {
            capability_refs.contains(&requirement.requirement_id)
                && requirement.stage_refs.contains(stage)
                && requirement
                    .substep_refs
                    .iter()
                    .any(|declared| declared == substep_id)
                && requirement
                    .constraints
                    .input_paths
                    .iter()
                    .any(|declared| declared == path)
                && !requirement.constraints.data_shapes.is_empty()
                && (!requirement.constraints.fields.is_empty()
                    || !requirement.constraints.schema_refs.is_empty())
        })
}

fn validate_envelope_path(path: &str, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    let mut parts = path.split('.');
    let namespace = parts.next().unwrap_or_default();
    if !ENVELOPE_NAMESPACES.contains(&namespace)
        || parts.next().is_none()
        || path.split('.').any(|part| part.is_empty())
    {
        diagnostics.push(Diagnostic::error(
            4,
            "SPEC_DATA_PATH_INVALID",
            pointer,
            format!("'{path}' is not a schema-addressable data-envelope path"),
        ));
    }
}

fn validate_expressions(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    if let Some(expression) = &spec.allocation.formula {
        validate_expression(
            expression,
            "/allocation/formula",
            &spec.variables,
            diagnostics,
        );
    }
    for (market_index, market) in spec.signal_market_requirements.iter().enumerate() {
        for (selector_index, selector) in market.selectors.iter().enumerate() {
            if let Some(expression) = &selector.expression {
                validate_expression(
                    expression,
                    &format!(
                        "/signal_market_requirements/{market_index}/selectors/{selector_index}/expression"
                    ),
                    &spec.variables,
                    diagnostics,
                );
            }
        }
    }
    for stage_name in StageName::VALIDATION_ORDER {
        let stage = spec.stages.view(&stage_name);
        let pointer = format!("/stages/{}", stage_name.as_str());
        for (index, substep) in stage.substeps.iter().enumerate() {
            if let Some(expression) = &substep.expression {
                validate_expression(
                    expression,
                    &format!("{pointer}/substeps/{index}/expression"),
                    &spec.variables,
                    diagnostics,
                );
            }
        }
    }
    for (index, trigger) in spec.stages.exit_policy.policy.triggers.iter().enumerate() {
        if let Some(expression) = &trigger.expression {
            validate_expression(
                expression,
                &format!("/stages/exit_policy/policy/triggers/{index}/expression"),
                &spec.variables,
                diagnostics,
            );
        }
    }
    if let Some(expression) = &spec.stages.price_policy.policy.expression {
        validate_expression(
            expression,
            "/stages/price_policy/policy/expression",
            &spec.variables,
            diagnostics,
        );
    }
    if let Some(expression) = &spec.stages.market_clock.policy.eligibility_expression {
        validate_expression(
            expression,
            "/stages/market_clock/policy/eligibility_expression",
            &spec.variables,
            diagnostics,
        );
    }
}

fn validate_expression(
    expression: &Expression,
    pointer: &str,
    variables: &BTreeMap<String, Variable>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let source = expression.source.trim();
    if source.is_empty() || source.len() > 4096 {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_PROFILE_INVALID",
            pointer,
            "expression violates the bounded deterministic CEL profile",
        ));
        return;
    }
    let program = match std::panic::catch_unwind(|| Program::compile(source)) {
        Ok(Ok(program)) => program,
        Ok(Err(error)) => {
            diagnostics.push(Diagnostic::error(
                5,
                "SPEC_EXPRESSION_PARSE_INVALID",
                pointer,
                format!("CEL parse failed: {error}"),
            ));
            return;
        }
        Err(_) => {
            diagnostics.push(Diagnostic::error(
                5,
                "SPEC_EXPRESSION_PARSE_INVALID",
                pointer,
                "CEL parser rejected the expression",
            ));
            return;
        }
    };
    if contains_unbounded_cel_macro(source) {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_COST_UNBOUNDED",
            pointer,
            "CEL comprehensions are outside the bounded publication profile",
        ));
        return;
    }
    if contains_untyped_control_flow(source) {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_TYPECHECK_UNAVAILABLE",
            pointer,
            "conditional and short-circuit CEL require static branch typing that is not available",
        ));
        return;
    }
    let references = program.references();
    let allowed_functions = [
        "contains",
        "size",
        "max",
        "min",
        "startsWith",
        "endsWith",
        "string",
        "bytes",
        "double",
        "int",
        "uint",
    ];
    let unsupported_function = references
        .functions()
        .into_iter()
        .find(|function| !function.starts_with('_') && !allowed_functions.contains(function));
    if let Some(function) = unsupported_function {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_FUNCTION_UNAVAILABLE",
            pointer,
            format!("function '{function}' is outside the deterministic registry"),
        ));
        return;
    }
    let external_variables = references
        .variables()
        .into_iter()
        .filter(|variable| *variable != "variables")
        .collect::<Vec<_>>();
    if !external_variables.is_empty() {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_TYPECHECK_UNAVAILABLE",
            pointer,
            format!(
                "schema-backed CEL typing is unavailable for references: {}",
                external_variables.join(", ")
            ),
        ));
        return;
    }
    let derived_cost = conservative_expression_cost(source, &program, variables);
    if expression.max_cost == 0
        || expression.max_cost > 10_000
        || derived_cost > expression.max_cost
        || derived_cost > 1_000
    {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_COST_EXCEEDED",
            format!("{pointer}/max_cost"),
            format!("derived CEL cost {derived_cost} exceeds the declared or platform limit"),
        ));
        return;
    }
    let defaults = variables
        .iter()
        .map(|(id, variable)| (id.clone(), variable.default.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut context = Context::default();
    if let Err(error) = context.add_variable("variables", defaults) {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_CONTEXT_INVALID",
            pointer,
            format!("variable context cannot be typed for CEL: {error}"),
        ));
        return;
    }
    let result = match program.execute(&context) {
        Ok(result) => result,
        Err(error) => {
            diagnostics.push(Diagnostic::error(
                5,
                "SPEC_EXPRESSION_TYPECHECK_FAILED",
                pointer,
                format!("CEL evaluation against declared types failed: {error}"),
            ));
            return;
        }
    };
    if !cel_result_matches(&result, &expression.result_type) {
        diagnostics.push(Diagnostic::error(
            5,
            "SPEC_EXPRESSION_TYPE_MISMATCH",
            format!("{pointer}/result_type"),
            format!(
                "declared result type '{}' does not match CEL result type '{}'",
                expression.result_type,
                result.type_of()
            ),
        ));
    }
}

fn contains_unbounded_cel_macro(source: &str) -> bool {
    let compact = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    [".all(", ".exists(", ".exists_one(", ".map(", ".filter("]
        .iter()
        .any(|token| compact.contains(token))
}

fn contains_untyped_control_flow(source: &str) -> bool {
    source.contains('?') || source.contains("&&") || source.contains("||")
}

// Scalar syntax and declared defaults are deliberately overcounted. CEL
// constructs with data-dependent iteration are rejected instead of estimated.
fn conservative_expression_cost(
    source: &str,
    program: &Program,
    variables: &BTreeMap<String, Variable>,
) -> u32 {
    let lexical_units = source
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, '(' | ')' | '[' | ']' | '{' | '}' | ',' | '.')
        })
        .filter(|unit| !unit.is_empty())
        .count();
    let operators = source
        .chars()
        .filter(|character| {
            matches!(
                character,
                '+' | '-' | '*' | '/' | '%' | '<' | '>' | '=' | '!' | '?' | ':'
            )
        })
        .count();
    let references = program.references();
    let default_units = variables
        .values()
        .map(|variable| variable.default.to_string().len().div_ceil(8))
        .sum::<usize>();
    (1 + lexical_units
        + operators
        + references.variables().len()
        + references.functions().len()
        + default_units)
        .try_into()
        .unwrap_or(u32::MAX)
}

fn cel_result_matches(result: &CelValue, declared: &str) -> bool {
    match declared {
        "boolean" => matches!(result, CelValue::Bool(_)),
        "integer" => matches!(result, CelValue::Int(_) | CelValue::UInt(_)),
        "decimal" => matches!(
            result,
            CelValue::Float(_) | CelValue::Int(_) | CelValue::UInt(_)
        ),
        "string" => matches!(result, CelValue::String(_)),
        "json" => matches!(
            result,
            CelValue::Map(_) | CelValue::List(_) | CelValue::Null
        ),
        "duration" => false,
        _ => false,
    }
}

fn validate_capabilities(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    let all = spec
        .capability_requirements
        .required
        .iter()
        .chain(&spec.capability_requirements.optional)
        .collect::<Vec<_>>();
    if spec.capability_requirements.required.is_empty() {
        diagnostics.push(Diagnostic::error(
            6,
            "SPEC_CAPABILITY_REQUIRED",
            "/capability_requirements/required",
            "at least one granular required capability is required",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut graph = BTreeMap::<String, BTreeSet<String>>::new();
    for (index, requirement) in all.iter().enumerate() {
        let pointer = format!("/capability_requirements/all/{index}");
        unique_id(
            &mut ids,
            &requirement.requirement_id,
            &format!("{pointer}/requirement_id"),
            diagnostics,
        );
        if !valid_versioned_name(&requirement.capability) {
            diagnostics.push(Diagnostic::error(
                6,
                "SPEC_CAPABILITY_NAME_INVALID",
                format!("{pointer}/capability"),
                "capability must be granular, namespaced, and versioned with @<version>",
            ));
        }
        if requirement.stage_refs.is_empty() {
            diagnostics.push(Diagnostic::error(
                6,
                "SPEC_CAPABILITY_STAGE_REQUIRED",
                format!("{pointer}/stage_refs"),
                "capability must identify exact consuming stages",
            ));
        }
        for (schema_index, schema_ref) in requirement.constraints.schema_refs.iter().enumerate() {
            if !valid_schema_ref(schema_ref) {
                diagnostics.push(Diagnostic::error(
                    6,
                    "SPEC_SCHEMA_REF_INVALID",
                    format!("{pointer}/constraints/schema_refs/{schema_index}"),
                    "capability schema refs must use schema://<namespace>/<name>@<version>",
                ));
            }
        }
        for reference in &requirement.dependency_refs {
            graph
                .entry(requirement.requirement_id.clone())
                .or_default()
                .insert(reference.clone());
        }
        for substep_ref in &requirement.substep_refs {
            if !requirement
                .stage_refs
                .iter()
                .any(|stage| stage_has_substep(spec.stages.view(stage), substep_ref))
            {
                diagnostics.push(Diagnostic::error(
                    6,
                    "SPEC_CAPABILITY_SUBSTEP_REF_MISSING",
                    format!("{pointer}/substep_refs"),
                    format!("unknown substep '{substep_ref}' in capability stage refs"),
                ));
            }
        }
    }
    for (owner, dependencies) in &graph {
        for dependency in dependencies {
            if !ids.contains(dependency) {
                diagnostics.push(Diagnostic::error(
                    6,
                    "SPEC_CAPABILITY_DEPENDENCY_MISSING",
                    "/capability_requirements",
                    format!("'{owner}' depends on unknown requirement '{dependency}'"),
                ));
            }
        }
    }
    if let Some(node) = graph_cycle(&graph) {
        diagnostics.push(Diagnostic::error(
            6,
            "SPEC_CAPABILITY_CYCLE",
            "/capability_requirements",
            format!("capability dependency graph cycles through '{node}'"),
        ));
    }
    for stage_name in StageName::VALIDATION_ORDER {
        for (index, substep) in spec.stages.view(&stage_name).substeps.iter().enumerate() {
            if !valid_versioned_name(&substep.operation_ref) {
                diagnostics.push(Diagnostic::error(
                    6,
                    "SPEC_OPERATION_REF_INVALID",
                    format!(
                        "/stages/{}/substeps/{index}/operation_ref",
                        stage_name.as_str()
                    ),
                    "operation_ref must be abstract, namespaced, and versioned",
                ));
            }
            if substep.capability_requirement_refs.is_empty() {
                diagnostics.push(Diagnostic::error(
                    6,
                    "SPEC_SUBSTEP_CAPABILITY_REQUIRED",
                    format!(
                        "/stages/{}/substeps/{index}/capability_requirement_refs",
                        stage_name.as_str()
                    ),
                    "every substep must reference a granular capability requirement",
                ));
            }
        }
    }
}

fn graph_cycle(graph: &BTreeMap<String, BTreeSet<String>>) -> Option<String> {
    fn visit(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Option<String> {
        if visiting.contains(node) {
            return Some(node.to_string());
        }
        if visited.contains(node) {
            return None;
        }
        visiting.insert(node.to_string());
        for next in graph.get(node).into_iter().flatten() {
            if let Some(cycle) = visit(next, graph, visiting, visited) {
                return Some(cycle);
            }
        }
        visiting.remove(node);
        visited.insert(node.to_string());
        None
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for node in graph.keys() {
        if let Some(cycle) = visit(node, graph, &mut visiting, &mut visited) {
            return Some(cycle);
        }
    }
    None
}

fn valid_versioned_name(value: &str) -> bool {
    let Some((name, version)) = value.rsplit_once('@') else {
        return false;
    };
    name.contains('.')
        && name.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        && !version.is_empty()
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '.')
}

fn valid_schema_ref(value: &str) -> bool {
    let Some(body) = value.strip_prefix("schema://") else {
        return false;
    };
    let Some((path, version)) = body.rsplit_once('@') else {
        return false;
    };
    path.split('/').count() >= 2
        && path.split('/').all(|part| {
            !part.is_empty()
                && part.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
        })
        && !version.is_empty()
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn capability_ids(spec: &StrategySpec) -> BTreeSet<String> {
    spec.capability_requirements
        .required
        .iter()
        .chain(&spec.capability_requirements.optional)
        .map(|requirement| requirement.requirement_id.clone())
        .collect()
}

fn stage_has_substep(stage: StageView<'_>, id: &str) -> bool {
    stage.substeps.iter().any(|substep| substep.id == id)
}

fn valid_decimal_string(value: &str) -> bool {
    !value.trim().is_empty()
        && value.parse::<f64>().is_ok()
        && !value.contains('e')
        && !value.contains('E')
}

fn unique_id(
    ids: &mut BTreeSet<String>,
    id: &str,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if id.trim().is_empty() || !ids.insert(id.to_string()) {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_ID_INVALID",
            pointer,
            "identifier must be non-empty and unique in its collection",
        ));
    }
}

fn unique_strings<'a>(
    values: impl Iterator<Item = &'a str>,
    pointer: &str,
    code: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen = BTreeSet::new();
    if values.into_iter().any(|value| !seen.insert(value)) {
        diagnostics.push(Diagnostic::error(3, code, pointer, "values must be unique"));
    }
}

fn require_nonempty(value: &str, pointer: &str, diagnostics: &mut Vec<Diagnostic>) {
    if value.trim().is_empty() {
        diagnostics.push(Diagnostic::error(
            3,
            "SPEC_STRING_EMPTY",
            pointer,
            "value must not be empty",
        ));
    }
}

fn serde_path_to_pointer(path: &str) -> String {
    if path.is_empty() || path == "." {
        return String::new();
    }
    let normalized = path.replace('[', ".").replace(']', "");
    format!(
        "/{}",
        normalized
            .trim_start_matches('.')
            .split('.')
            .filter(|part| !part.is_empty())
            .map(escape_pointer)
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| {
        (left.layer, &left.pointer, &left.code).cmp(&(right.layer, &right.pointer, &right.code))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authority_rejection_is_recursive_and_uses_json_pointer() {
        let diagnostics = runtime_boundary_diagnostics(&json!({
            "metadata": {"nested": [{"pluginInstanceId": "installed-1"}]}
        }));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "SPEC_AUTHORITY_RESERVED");
        assert_eq!(
            diagnostics[0].pointer,
            "/metadata/nested/0/pluginInstanceId"
        );
    }

    #[test]
    fn authority_rejection_covers_bare_aliases_and_namespaced_extensions() {
        for key in [
            "provider",
            "broker",
            "plugin",
            "mode",
            "risk_scale",
            "acme.plugin_ref",
            "acmeBrokerAccountId",
            "account",
            "acme.account",
            "runtime_state",
            "runtimeState",
        ] {
            let diagnostics = runtime_boundary_diagnostics(&json!({
                "metadata": {key: "runtime-binding"}
            }));
            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == "SPEC_AUTHORITY_RESERVED"
                        && diagnostic.pointer == format!("/metadata/{key}")
                }),
                "authority alias {key} bypassed the runtime boundary: {diagnostics:#?}"
            );
        }
    }

    #[test]
    fn portable_allocation_mode_is_not_runtime_authority() {
        assert!(runtime_boundary_diagnostics(&json!({
            "allocation": {"mode": "none"}
        }))
        .is_empty());
    }

    #[test]
    fn cel_parser_rejects_malformed_expression() {
        let expression = Expression {
            language: ExpressionLanguage::CelV1,
            source: "1 +".to_string(),
            result_type: "integer".to_string(),
            max_cost: 100,
        };
        let mut diagnostics = Vec::new();
        validate_expression(
            &expression,
            "/expression",
            &BTreeMap::new(),
            &mut diagnostics,
        );
        assert_eq!(diagnostics[0].code, "SPEC_EXPRESSION_PARSE_INVALID");
    }

    #[test]
    fn cel_evaluation_rejects_declared_result_type_mismatch() {
        let expression = Expression {
            language: ExpressionLanguage::CelV1,
            source: "1 + 2".to_string(),
            result_type: "boolean".to_string(),
            max_cost: 100,
        };
        let mut diagnostics = Vec::new();
        validate_expression(
            &expression,
            "/expression",
            &BTreeMap::new(),
            &mut diagnostics,
        );
        assert_eq!(diagnostics[0].code, "SPEC_EXPRESSION_TYPE_MISMATCH");
    }

    #[test]
    fn cel_comprehensions_fail_closed_as_unbounded() {
        let expression = Expression {
            language: ExpressionLanguage::CelV1,
            source: "[1, 2, 3].all(item, item > 0)".to_string(),
            result_type: "boolean".to_string(),
            max_cost: 1_000,
        };
        let mut diagnostics = Vec::new();
        validate_expression(
            &expression,
            "/expression",
            &BTreeMap::new(),
            &mut diagnostics,
        );
        assert_eq!(diagnostics[0].code, "SPEC_EXPRESSION_COST_UNBOUNDED");
    }

    #[test]
    fn deeply_nested_scalar_expression_cannot_bypass_cost_bound() {
        let source = std::iter::repeat_n("1", 64).collect::<Vec<_>>().join(" + ");
        let expression = Expression {
            language: ExpressionLanguage::CelV1,
            source,
            result_type: "integer".to_string(),
            max_cost: 10,
        };
        let mut diagnostics = Vec::new();
        validate_expression(
            &expression,
            "/expression",
            &BTreeMap::new(),
            &mut diagnostics,
        );
        assert_eq!(diagnostics[0].code, "SPEC_EXPRESSION_COST_EXCEEDED");
    }

    #[test]
    fn lazy_branches_fail_closed_without_static_typechecking() {
        let expression = Expression {
            language: ExpressionLanguage::CelV1,
            source: "true ? 1 : 'hidden type mismatch'".to_string(),
            result_type: "integer".to_string(),
            max_cost: 100,
        };
        let mut diagnostics = Vec::new();
        validate_expression(
            &expression,
            "/expression",
            &BTreeMap::new(),
            &mut diagnostics,
        );
        assert_eq!(diagnostics[0].code, "SPEC_EXPRESSION_TYPECHECK_UNAVAILABLE");
    }

    #[test]
    fn unrelated_stage_capability_does_not_bless_external_read() {
        let requirements = CapabilityRequirements {
            required: vec![CapabilityRequirement {
                requirement_id: "req_bars".to_string(),
                capability: "market_data.bars.read@1".to_string(),
                purpose: "bars".to_string(),
                required_for: vec![ReadinessLevel::Research],
                stage_refs: vec![StageName::Inputs],
                substep_refs: vec!["load_bars".to_string()],
                constraints: CapabilityConstraints {
                    instrument_families: vec![InstrumentFamily::Equity],
                    data_shapes: vec!["bars".to_string()],
                    operations: vec!["read".to_string()],
                    fields: vec!["close".to_string()],
                    input_paths: vec!["market_data.bars".to_string()],
                    schema_refs: vec!["schema://market-data/bars@1".to_string()],
                    timeframe: Some("1h".to_string()),
                    max_freshness: None,
                    max_latency_ms: None,
                    deterministic: true,
                    replayable: true,
                },
                dependency_refs: Vec::new(),
                fallback_policy: FallbackPolicy::None,
                policy_tags: Vec::new(),
            }],
            optional: Vec::new(),
        };
        assert!(!capability_proves_external_read(
            &requirements,
            &StageName::Inputs,
            "load_bars",
            &["req_bars".to_string()],
            "market_data.option_chain"
        ));
        assert!(!capability_proves_external_read(
            &requirements,
            &StageName::Inputs,
            "load_bars",
            &["unrelated".to_string()],
            "market_data.bars"
        ));
        assert!(!capability_proves_external_read(
            &requirements,
            &StageName::Inputs,
            "different_substep",
            &["req_bars".to_string()],
            "market_data.bars"
        ));
    }
}
