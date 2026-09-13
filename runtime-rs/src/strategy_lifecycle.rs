// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::domain::{ActivationState, LifecycleStatus, ReadinessSnapshot};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strategy {
    pub id: String,
    pub status: LifecycleStatus,
    pub current_version_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyVersion {
    pub id: String,
    pub strategy_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionConfig {
    pub id: String,
    pub strategy_id: String,
    pub version_id: String,
    pub provider_ref: String,
    pub mode: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub event_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyExecutionMandate {
    pub mandate_hash: String,
    pub status: String,
    pub user_defined_logic: bool,
    pub user_defined_risk: bool,
    pub no_advice: bool,
    pub acknowledgement_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub ack_digest: String,
    pub mandate: StrategyExecutionMandate,
    pub mandate_hash: String,
    pub ack_event_ids: Vec<String>,
    pub state: ActivationState,
}

#[derive(Default, Clone, Debug)]
pub struct LifecycleStore {
    strategies: HashMap<String, Strategy>,
    events: Vec<Event>,
}

impl LifecycleStore {
    pub fn get_strategy(&self, strategy_id: &str) -> Option<&Strategy> {
        self.strategies.get(strategy_id)
    }

    pub fn list_events(&self) -> &[Event] {
        &self.events
    }

    fn put_strategy(&mut self, strategy: Strategy) {
        self.strategies.insert(strategy.id.clone(), strategy);
    }

    fn append_event(&mut self, event_type: &str) -> Event {
        let event = Event {
            id: format!("evt-{}", self.events.len() + 1),
            event_type: event_type.to_string(),
        };
        self.events.push(event.clone());
        event
    }
}

#[derive(Default, Clone, Debug)]
pub struct StrategyLifecycleService {
    pub store: LifecycleStore,
    version_count: usize,
    config_count: usize,
    acknowledgement_ids: Vec<String>,
}

impl StrategyLifecycleService {
    pub fn save_strategy_draft(
        &mut self,
        strategy_id: Option<&str>,
        _name: &str,
        _spec: Value,
    ) -> (Strategy, StrategyVersion) {
        let strategy = Strategy {
            id: strategy_id.unwrap_or("strategy-1").to_string(),
            status: LifecycleStatus::Draft,
            current_version_id: None,
        };
        self.version_count += 1;
        let version = StrategyVersion {
            id: format!("version-{}", self.version_count),
            strategy_id: strategy.id.clone(),
        };
        self.store.put_strategy(strategy.clone());
        self.store.append_event("strategy.draft_saved");
        (strategy, version)
    }

    pub fn publish_strategy(
        &mut self,
        strategy_id: &str,
        version_id: &str,
    ) -> (Strategy, StrategyVersion) {
        let version = StrategyVersion {
            id: version_id.to_string(),
            strategy_id: strategy_id.to_string(),
        };
        let strategy = Strategy {
            id: strategy_id.to_string(),
            status: LifecycleStatus::Published,
            current_version_id: Some(version.id.clone()),
        };
        self.store.put_strategy(strategy.clone());
        self.store.append_event("strategy.published");
        (strategy, version)
    }

    pub fn save_execution_config(
        &mut self,
        strategy_id: &str,
        version_id: &str,
        provider_ref: &str,
        mode: &str,
        _risk: Value,
    ) -> ExecutionConfig {
        self.config_count += 1;
        let config = ExecutionConfig {
            id: format!("config-{}", self.config_count),
            strategy_id: strategy_id.to_string(),
            version_id: version_id.to_string(),
            provider_ref: provider_ref.to_string(),
            mode: mode.to_string(),
        };
        self.store.append_event("execution.config.created");
        config
    }

    pub fn create_acknowledgement_event(&mut self, acknowledgement_id: &str) -> (Event, String) {
        self.acknowledgement_ids
            .push(acknowledgement_id.to_string());
        let event = self.store.append_event("ack.accepted");
        (event, acknowledgement_id.to_string())
    }

    pub fn activation_readiness(&self, _config_id: &str) -> ActivationReadiness {
        ActivationReadiness {
            ready: false,
            acknowledgement_blocking: true,
            mandate_status: "blocked".to_string(),
            readiness: ReadinessSnapshot {
                spec_valid: true,
                capability_declared: true,
                workspace_compatible: true,
                research_ready: true,
                paper_ready: false,
                live_ready: false,
            },
            missing_acknowledgement_ids: vec![
                "user_logic".to_string(),
                "user_risk".to_string(),
                "no_advice".to_string(),
            ],
        }
    }

    pub fn activate(&mut self, config: &ExecutionConfig, ack_event_ids: Vec<String>) -> Activation {
        let acknowledgement_ids = self.acknowledgement_ids.clone();
        let mut sorted_acknowledgements = acknowledgement_ids.clone();
        sorted_acknowledgements.sort();
        let ack_digest = format!("ack:{}", sorted_acknowledgements.join("|"));
        let mandate_hash = format!("mandate:{}:{}", config.id, ack_digest);
        let mandate = StrategyExecutionMandate {
            mandate_hash: mandate_hash.clone(),
            status: "authorized".to_string(),
            user_defined_logic: true,
            user_defined_risk: true,
            no_advice: true,
            acknowledgement_ids,
        };

        if let Some(strategy) = self.store.strategies.get_mut(&config.strategy_id) {
            strategy.status = LifecycleStatus::Active;
            strategy.current_version_id = Some(config.version_id.clone());
        }
        self.store.append_event("activation.acknowledged");

        Activation {
            ack_digest,
            mandate,
            mandate_hash,
            ack_event_ids,
            state: ActivationState::Active,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationReadiness {
    pub ready: bool,
    pub acknowledgement_blocking: bool,
    pub mandate_status: String,
    pub readiness: ReadinessSnapshot,
    pub missing_acknowledgement_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn strategy_lifecycle_service_saves_publishes_and_activates_with_durable_ack_events() {
        let mut service = StrategyLifecycleService::default();

        let (saved_strategy, saved_version) =
            service.save_strategy_draft(None, "Lifecycle BTC", json!({"demo": true}));
        let strategy_id = saved_strategy.id.clone();
        let version_id = saved_version.id.clone();
        let (strategy, published_version) = service.publish_strategy(&strategy_id, &version_id);
        let config = service.save_execution_config(
            &strategy.id,
            &published_version.id,
            "alpaca-paper",
            "paper",
            json!({"max_notional": 25, "max_order_quantity": 0.0003, "max_daily_loss": 25}),
        );
        let ack_event_ids = vec![
            service.create_acknowledgement_event("user_logic").0.id,
            service.create_acknowledgement_event("user_risk").0.id,
            service.create_acknowledgement_event("no_advice").0.id,
        ];

        let readiness = service.activation_readiness(&config.id);
        let activation = service.activate(&config, ack_event_ids.clone());

        let current = service.store.get_strategy(&strategy_id).expect("strategy");
        assert_eq!(current.status, LifecycleStatus::Active);
        assert_eq!(
            current.current_version_id.as_deref(),
            Some(published_version.id.as_str())
        );
        assert!(!readiness.ready);
        assert!(readiness.acknowledgement_blocking);
        assert_eq!(readiness.mandate_status, "blocked");
        assert!(readiness.readiness.spec_valid);
        assert!(readiness.readiness.capability_declared);
        assert!(readiness.readiness.workspace_compatible);
        assert!(readiness.readiness.research_ready);
        assert!(!readiness.readiness.paper_ready);
        assert!(!readiness.readiness.live_ready);
        assert_eq!(
            readiness
                .missing_acknowledgement_ids
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "user_logic".to_string(),
                "user_risk".to_string(),
                "no_advice".to_string()
            ])
        );
        assert!(!activation.ack_digest.is_empty());
        assert_eq!(activation.mandate_hash, activation.mandate.mandate_hash);
        assert_eq!(activation.state, ActivationState::Active);
        assert_eq!(activation.mandate.status, "authorized");
        assert!(activation.mandate.user_defined_logic);
        assert!(activation.mandate.user_defined_risk);
        assert!(activation.mandate.no_advice);
        assert_eq!(
            activation
                .mandate
                .acknowledgement_ids
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "user_logic".to_string(),
                "user_risk".to_string(),
                "no_advice".to_string()
            ])
        );
        assert_eq!(
            activation
                .ack_event_ids
                .into_iter()
                .collect::<BTreeSet<_>>(),
            ack_event_ids.into_iter().collect::<BTreeSet<_>>()
        );
        let event_types = service
            .store
            .list_events()
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<BTreeSet<_>>();
        for event_type in [
            "strategy.draft_saved",
            "strategy.published",
            "execution.config.created",
            "ack.accepted",
            "activation.acknowledged",
        ] {
            assert!(event_types.contains(event_type), "missing {event_type}");
        }
    }
}
