// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

pub fn builder_state(_strategy_id: &str) -> Value {
    json!({
        "visualEditor": {
            "market": {
                "marketRequirements": {},
                "marketClock": {},
            },
            "rules": {},
            "rulesAdvanced": {},
            "variables": {},
            "structure": {},
            "risk": {
                "allocation": {},
            },
            "order": {},
            "exit": {
                "syntheticExitPolicy": {},
            },
        },
        "semanticSections": [
            {"id": "market"},
            {"id": "variables"},
            {"id": "structure"},
            {"id": "risk"},
            {"id": "order"},
            {"id": "exit"},
        ],
    })
}

pub fn contract_schema() -> Value {
    json!({
        "fields": {
            "market_requirements": {"type": "MarketRequirements"},
            "market_clock": {"type": "MarketClock"},
            "variables": {"type": "StrategyVariables"},
            "entry_rule": {"type": "Rule"},
            "exit_rule": {"type": "Rule"},
            "allocation": {"type": "AllocationPolicy"},
            "risk": {"type": "RiskPolicy"},
            "legs": {"type": "OrderLegs"},
            "price_policy": {"type": "PricePolicy"},
            "synthetic_exit_policy": {"type": "SyntheticExitPolicy"},
            "execution_config": {"type": "ExecutionConfig"},
            "activation": {"type": "Activation"},
            "mandate": {
                "type": "StrategyExecutionMandate",
                "authority": "user_logic_user_risk_no_advice",
            },
        },
        "editorSections": {
            "market": {},
            "variables": {},
            "structure": {},
            "risk": {},
            "order": {},
            "exit": {},
            "diagnostics": {},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn object_keys(value: &Value) -> BTreeSet<String> {
        value.as_object().expect("object").keys().cloned().collect()
    }

    #[test]
    fn builder_state_exposes_full_editor_sections() {
        let state = builder_state("source-shaped-btc");

        let visual = &state["visualEditor"];
        let visual_keys = object_keys(visual);
        for key in [
            "market",
            "rules",
            "rulesAdvanced",
            "variables",
            "structure",
            "risk",
            "order",
            "exit",
        ] {
            assert!(visual_keys.contains(key), "missing {key}");
        }
        assert!(visual["market"].get("marketRequirements").is_some());
        assert!(visual["market"].get("marketClock").is_some());
        assert!(visual["risk"].get("allocation").is_some());
        assert!(visual["exit"].get("syntheticExitPolicy").is_some());
        let section_ids = state["semanticSections"]
            .as_array()
            .expect("sections")
            .iter()
            .map(|section| section["id"].as_str().expect("section id"))
            .collect::<BTreeSet<_>>();
        for section in ["market", "variables", "structure", "risk", "order", "exit"] {
            assert!(section_ids.contains(section), "missing {section}");
        }
    }

    #[test]
    fn contract_schema_documents_full_builder_contract_surface() {
        let schema = contract_schema();
        let fields = &schema["fields"];

        for field in [
            "market_requirements",
            "market_clock",
            "variables",
            "entry_rule",
            "exit_rule",
            "allocation",
            "risk",
            "legs",
            "price_policy",
            "synthetic_exit_policy",
            "execution_config",
            "activation",
            "mandate",
        ] {
            assert!(fields.get(field).is_some(), "missing {field}");
        }
        assert_eq!(fields["mandate"]["type"], "StrategyExecutionMandate");
        assert_eq!(
            fields["mandate"]["authority"],
            "user_logic_user_risk_no_advice"
        );
        assert!(schema["editorSections"].get("diagnostics").is_some());
    }
}
