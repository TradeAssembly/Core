use super::canonical::{canonical_hash, raw_hash};
use super::diagnostic::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySpecSourceFamily {
    V3,
    PredecessorV2,
    SkeletalTradeAssemblyV2,
    AmbiguousV2,
    Unknown,
}

impl StrategySpecSourceFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V3 => "strategy_spec_v3",
            Self::PredecessorV2 => "strategy_contract_v2",
            Self::SkeletalTradeAssemblyV2 => "tradeassembly_skeletal_v2",
            Self::AmbiguousV2 => "ambiguous_v2",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationQuestion {
    pub code: String,
    pub pointer: String,
    pub question: String,
    pub blocking: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationDiff {
    pub source_pointer: String,
    pub target_pointer: Option<String>,
    pub disposition: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationResult {
    pub source_family: StrategySpecSourceFamily,
    pub source_schema_id: String,
    pub source_representation: MigrationSourceRepresentation,
    pub original_document: String,
    pub original_hash: String,
    pub source_canonical_hash: Option<String>,
    pub proposed_v3_draft: Option<Value>,
    pub execution_config_proposal: BTreeMap<String, Value>,
    pub diagnostics: Vec<Diagnostic>,
    pub questions: Vec<MigrationQuestion>,
    pub diff: Vec<MigrationDiff>,
    pub requires_review: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationSourceRepresentation {
    IngressBytes,
    NormalizedJsonValue,
}

pub fn detect_source_family(value: &Value) -> StrategySpecSourceFamily {
    let Some(object) = value.as_object() else {
        return StrategySpecSourceFamily::Unknown;
    };
    if object.get("spec_version").and_then(Value::as_str) == Some("3.0") {
        return if object.contains_key("signal_market_requirements")
            && object.contains_key("trade_market_requirements")
            && object.contains_key("allocation")
            && object.contains_key("portfolio_scope")
        {
            StrategySpecSourceFamily::V3
        } else {
            StrategySpecSourceFamily::Unknown
        };
    }
    if object.get("spec_version").and_then(Value::as_str) != Some("2.0") {
        return StrategySpecSourceFamily::Unknown;
    }
    let stages = object.get("stages").and_then(Value::as_object);
    let predecessor = object.contains_key("capabilities")
        && stages.is_some_and(|stages| {
            ["screener", "exit", "order", "price"]
                .iter()
                .any(|stage| stages.contains_key(*stage))
        });
    let skeletal = object.contains_key("market_requirements")
        && object.contains_key("capability_requirements")
        && stages.is_some_and(|stages| {
            ["universe", "exit_policy", "order_strategy", "price_policy"]
                .iter()
                .any(|stage| stages.contains_key(*stage))
        });
    match (predecessor, skeletal) {
        (true, true) => StrategySpecSourceFamily::AmbiguousV2,
        (true, false) => StrategySpecSourceFamily::PredecessorV2,
        (false, true) => StrategySpecSourceFamily::SkeletalTradeAssemblyV2,
        (false, false) => StrategySpecSourceFamily::Unknown,
    }
}

pub fn migrate_value(value: &Value) -> MigrationResult {
    let original = serde_json::to_vec(value).expect("JSON value serializes");
    migrate_parts(
        &original,
        value.clone(),
        MigrationSourceRepresentation::NormalizedJsonValue,
    )
}

pub fn migrate_bytes(bytes: &[u8]) -> Result<MigrationResult, String> {
    let value = serde_json::from_slice::<Value>(bytes)
        .map_err(|error| format!("migration source is not valid JSON: {error}"))?;
    Ok(migrate_parts(
        bytes,
        value,
        MigrationSourceRepresentation::IngressBytes,
    ))
}

fn migrate_parts(
    original: &[u8],
    value: Value,
    source_representation: MigrationSourceRepresentation,
) -> MigrationResult {
    let source_family = detect_source_family(&value);
    let mut execution_config_proposal = BTreeMap::new();
    extract_execution_config(&value, "", &mut execution_config_proposal);
    let mut diagnostics = Vec::new();
    let mut questions = Vec::new();
    let mut diff = Vec::new();

    let proposed_v3_draft = match source_family {
        StrategySpecSourceFamily::V3 => Some(value.clone()),
        StrategySpecSourceFamily::PredecessorV2 => {
            questions.extend(predecessor_questions(&value));
            diff.extend(predecessor_diff());
            None
        }
        StrategySpecSourceFamily::SkeletalTradeAssemblyV2 => {
            questions.extend(skeletal_questions(&value));
            diff.extend(skeletal_diff());
            None
        }
        StrategySpecSourceFamily::AmbiguousV2 => {
            diagnostics.push(Diagnostic::error(
                1,
                "MIGRATION_SOURCE_AMBIGUOUS",
                "/spec_version",
                "payload contains discriminator fields from both incompatible V2 families",
            ));
            questions.push(MigrationQuestion {
                code: "MIGRATION_SOURCE_SELECTION_REQUIRED".to_string(),
                pointer: "/spec_version".to_string(),
                question: "Which original V2 schema produced this document?".to_string(),
                blocking: true,
            });
            None
        }
        StrategySpecSourceFamily::Unknown => {
            diagnostics.push(Diagnostic::error(
                1,
                "MIGRATION_SOURCE_UNKNOWN",
                "",
                "document does not exactly match StrategyContractV2 or skeletal TradeAssembly V2",
            ));
            None
        }
    };
    for pointer in execution_config_proposal.keys() {
        diff.push(MigrationDiff {
            source_pointer: pointer.clone(),
            target_pointer: Some(format!(
                "/strategy_execution_config_revision/extracted{}",
                pointer
            )),
            disposition: "extract_for_review".to_string(),
        });
    }

    MigrationResult {
        source_family,
        source_schema_id: source_family.as_str().to_string(),
        source_representation,
        original_document: String::from_utf8_lossy(original).into_owned(),
        original_hash: raw_hash(original),
        source_canonical_hash: canonical_hash(&value).ok(),
        proposed_v3_draft,
        execution_config_proposal,
        diagnostics,
        questions,
        diff,
        requires_review: source_family != StrategySpecSourceFamily::V3,
    }
}

fn extract_execution_config(value: &Value, pointer: &str, extracted: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_pointer = format!("{pointer}/{}", escape_pointer(key));
                if is_execution_config_key(key) {
                    extracted.insert(child_pointer.clone(), child.clone());
                } else {
                    extract_execution_config(child, &child_pointer, extracted);
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                extract_execution_config(child, &format!("{pointer}/{index}"), extracted);
            }
        }
        _ => {}
    }
}

fn is_execution_config_key(key: &str) -> bool {
    let normalized = key.replace('_', "").to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "providerref"
            | "pluginref"
            | "plugin"
            | "credentialref"
            | "accountref"
            | "brokeraccountid"
            | "targetnotional"
            | "maxnotional"
            | "maxdailyloss"
            | "maxorderquantity"
            | "riskbudget"
            | "capital"
            | "leverage"
            | "mode"
    )
}

fn predecessor_questions(value: &Value) -> Vec<MigrationQuestion> {
    let mut questions = vec![
        question(
            "MIGRATION_MARKET_IDS_REQUIRED",
            "/signal_market_requirements",
            "Confirm stable IDs and complete data quality constraints for every signal market.",
        ),
        question(
            "MIGRATION_CAPABILITY_IDS_REQUIRED",
            "/capabilities",
            "Map broad V2 capability keys to granular, versioned V3 requirements and operations.",
        ),
        question(
            "MIGRATION_STAGE_CONTRACTS_REQUIRED",
            "/stages",
            "Confirm V3 output, state, failure, expression, and policy contracts for every stage.",
        ),
    ];
    if value.pointer("/trade_market_requirements").is_none() {
        questions.push(question(
            "MIGRATION_TRADE_MARKETS_REQUIRED",
            "/trade_market_requirements",
            "Declare the markets and instrument families this strategy may trade.",
        ));
    }
    questions
}

fn skeletal_questions(_value: &Value) -> Vec<MigrationQuestion> {
    vec![
        question(
            "MIGRATION_SIGNAL_TRADE_SPLIT_REQUIRED",
            "/market_requirements",
            "Separate observed signal markets from markets and instruments the strategy may trade.",
        ),
        question(
            "MIGRATION_CAPABILITY_GRANULARITY_REQUIRED",
            "/capability_requirements",
            "Confirm granular versioned capability, operation, dependency, and readiness requirements.",
        ),
        question(
            "MIGRATION_STAGE_SUBSTEPS_REQUIRED",
            "/stages",
            "Confirm ordered substeps, dataflow, schemas, state, expressions, and typed policies.",
        ),
        question(
            "MIGRATION_ALLOCATION_REQUIRED",
            "/allocation",
            "Choose a portable relative allocation mode; concrete capital remains in execution config.",
        ),
    ]
}

fn predecessor_diff() -> Vec<MigrationDiff> {
    [
        ("/capabilities", "/capability_requirements"),
        ("/stages/screener", "/stages/universe"),
        ("/stages/exit", "/stages/exit_policy"),
        ("/stages/order", "/stages/order_strategy"),
        ("/stages/price", "/stages/price_policy"),
    ]
    .into_iter()
    .map(|(source, target)| MigrationDiff {
        source_pointer: source.to_string(),
        target_pointer: Some(target.to_string()),
        disposition: "requires_review".to_string(),
    })
    .collect()
}

fn skeletal_diff() -> Vec<MigrationDiff> {
    vec![MigrationDiff {
        source_pointer: "/market_requirements".to_string(),
        target_pointer: None,
        disposition: "split_requires_review".to_string(),
    }]
}

fn question(code: &str, pointer: &str, text: &str) -> MigrationQuestion {
    MigrationQuestion {
        code: code.to_string(),
        pointer: pointer.to_string(),
        question: text.to_string(),
        blocking: true,
    }
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_only_the_two_named_v2_families_and_ambiguity() {
        let predecessor = json!({
            "spec_version": "2.0",
            "capabilities": {"required": []},
            "stages": {"screener": {}}
        });
        let skeletal = json!({
            "spec_version": "2.0",
            "market_requirements": {},
            "capability_requirements": {"required": []},
            "stages": {"universe": {}}
        });
        let ambiguous = json!({
            "spec_version": "2.0",
            "capabilities": {"required": []},
            "market_requirements": {},
            "capability_requirements": {"required": []},
            "stages": {"screener": {}, "universe": {}}
        });
        assert_eq!(
            detect_source_family(&predecessor),
            StrategySpecSourceFamily::PredecessorV2
        );
        assert_eq!(
            detect_source_family(&skeletal),
            StrategySpecSourceFamily::SkeletalTradeAssemblyV2
        );
        assert_eq!(
            detect_source_family(&ambiguous),
            StrategySpecSourceFamily::AmbiguousV2
        );
        assert_eq!(
            detect_source_family(&json!({"schemaVersion": "tradeassembly.strategy.v1"})),
            StrategySpecSourceFamily::Unknown
        );
    }

    #[test]
    fn migration_preserves_bytes_extracts_config_and_never_invents_a_draft() {
        let bytes = br#"{ "spec_version":"2.0", "market_requirements":{}, "capability_requirements":{"required":[]}, "stages":{"universe":{}}, "risk":{"max_notional":25} }"#;
        let result = migrate_bytes(bytes).unwrap();
        assert_eq!(result.original_document.as_bytes(), bytes);
        assert_eq!(result.original_hash, raw_hash(bytes));
        assert!(result.proposed_v3_draft.is_none());
        assert_eq!(result.execution_config_proposal["/risk/max_notional"], 25);
        assert!(result.questions.iter().all(|question| question.blocking));
    }
}
