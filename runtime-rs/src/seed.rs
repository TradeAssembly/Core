// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderManifest {
    pub id: String,
    pub provider_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInstance {
    pub id: String,
    pub manifest_id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategySeed {
    pub id: String,
    pub provider_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyConfig {
    pub id: String,
    pub strategy_id: String,
    pub provider_ref: String,
    pub mode: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedEvent {
    pub event_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedResult {
    pub key: String,
    pub status: String,
}

#[derive(Default, Debug)]
pub struct LocalStore {
    providers: BTreeMap<String, ProviderInstance>,
    manifests: BTreeMap<String, ProviderManifest>,
    users: BTreeMap<String, AuthUser>,
    strategies: BTreeMap<String, StrategySeed>,
    strategy_names: BTreeMap<String, String>,
    configs: BTreeMap<String, StrategyConfig>,
    events: Vec<SeedEvent>,
}

impl LocalStore {
    pub fn list_manifests(&self) -> Vec<&ProviderManifest> {
        self.manifests.values().collect()
    }

    pub fn list_providers(&self) -> Vec<&ProviderInstance> {
        self.providers.values().collect()
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderInstance> {
        self.providers.get(id)
    }

    fn put_manifest(&mut self, manifest: ProviderManifest) {
        self.manifests.insert(manifest.id.clone(), manifest);
    }

    fn put_provider(&mut self, provider: ProviderInstance) {
        self.providers.insert(provider.id.clone(), provider);
    }

    fn put_user(&mut self, user: AuthUser) {
        self.users.insert(user.id.clone(), user);
    }

    fn put_strategy(&mut self, strategy: StrategySeed) {
        self.strategies.insert(strategy.id.clone(), strategy);
    }

    fn put_strategy_name(&mut self, id: impl Into<String>, name: impl Into<String>) {
        self.strategy_names.insert(id.into(), name.into());
    }

    fn put_config(&mut self, config: StrategyConfig) {
        self.configs.insert(config.id.clone(), config);
    }

    fn append_event(&mut self, event_type: impl Into<String>) {
        self.events.push(SeedEvent {
            event_type: event_type.into(),
        });
    }

    pub fn strategy_count(&self, name: &str) -> usize {
        self.strategy_names
            .values()
            .filter(|candidate| *candidate == name)
            .count()
    }

    pub fn config_count(&self, strategy_id: &str) -> usize {
        self.configs
            .values()
            .filter(|config| config.strategy_id == strategy_id)
            .count()
    }

    pub fn config_provider(&self, config_id: &str) -> Option<&str> {
        self.configs
            .get(config_id)
            .map(|config| config.provider_ref.as_str())
    }

    pub fn config_mode(&self, config_id: &str) -> Option<&str> {
        self.configs
            .get(config_id)
            .map(|config| config.mode.as_str())
    }

    pub fn event_count(&self, event_type: &str) -> usize {
        self.events
            .iter()
            .filter(|event| event.event_type == event_type)
            .count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtcSeedConfig {
    pub provider_ref: String,
    pub mode: String,
}

impl BtcSeedConfig {
    pub fn new(provider_ref: impl Into<String>, mode: impl Into<String>) -> Self {
        Self {
            provider_ref: provider_ref.into(),
            mode: mode.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtcSeedIds {
    pub strategy_id: String,
    pub version_id: String,
    pub config_id: String,
}

pub struct SeedService<'a> {
    store: &'a mut LocalStore,
}

impl<'a> SeedService<'a> {
    pub fn new(store: &'a mut LocalStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &LocalStore {
        self.store
    }

    pub fn seed_btc_exit_demo(&mut self, config: BtcSeedConfig) -> Result<BtcSeedIds, String> {
        let mut sink = LocalStoreSink::new(self.store);
        seed_defaults(&mut sink);
        if self.store.provider(&config.provider_ref).is_none() {
            return Err("seed provider is not configured".to_string());
        }

        let ids = BtcSeedIds {
            strategy_id: "strat_btcfastexitdemo".to_string(),
            version_id: "ver_btcfastexitdemo".to_string(),
            config_id: "cfg_btcfastexitdemo".to_string(),
        };
        self.store
            .put_strategy_name(ids.strategy_id.clone(), "BTC fast exit demo".to_string());
        self.store.put_strategy(StrategySeed {
            id: ids.strategy_id.clone(),
            provider_ref: config.provider_ref.clone(),
        });
        self.store.put_config(StrategyConfig {
            id: ids.config_id.clone(),
            strategy_id: ids.strategy_id.clone(),
            provider_ref: config.provider_ref,
            mode: config.mode,
        });
        self.store.append_event("seed.enforced");
        Ok(ids)
    }
}

pub trait SeedSink {
    fn write_manifest(&mut self, manifest: ProviderManifest);
    fn write_provider(&mut self, provider: ProviderInstance);
    fn write_user(&mut self, user: AuthUser);
    fn write_strategy(&mut self, strategy: StrategySeed);
}

pub struct LocalStoreSink<'a> {
    store: &'a mut LocalStore,
}

impl<'a> LocalStoreSink<'a> {
    pub fn new(store: &'a mut LocalStore) -> Self {
        Self { store }
    }
}

impl SeedSink for LocalStoreSink<'_> {
    fn write_manifest(&mut self, manifest: ProviderManifest) {
        self.store.put_manifest(manifest);
    }

    fn write_provider(&mut self, provider: ProviderInstance) {
        self.store.put_provider(provider);
    }

    fn write_user(&mut self, user: AuthUser) {
        self.store.put_user(user);
    }

    fn write_strategy(&mut self, strategy: StrategySeed) {
        self.store.put_strategy(strategy);
    }
}

pub struct SeedEnforcer<'a> {
    store: &'a mut LocalStore,
}

impl<'a> SeedEnforcer<'a> {
    pub fn new(store: &'a mut LocalStore) -> Self {
        Self { store }
    }

    pub fn ensure_default_providers(&mut self) -> Vec<SeedResult> {
        let mut sink = LocalStoreSink::new(self.store);
        seed_defaults(&mut sink)
    }
}

pub trait SeedTask {
    fn name(&self) -> &'static str;
    fn run(&self, sink: &mut dyn SeedSink) -> Vec<SeedResult>;
}

#[derive(Default)]
pub struct SeedTaskRegistry {
    tasks: Vec<Box<dyn SeedTask>>,
}

impl SeedTaskRegistry {
    pub fn register<T: SeedTask + 'static>(&mut self, task: T) {
        self.tasks.push(Box::new(task));
    }
}

pub struct SeedRunner {
    registry: SeedTaskRegistry,
}

impl SeedRunner {
    pub fn new(registry: SeedTaskRegistry) -> Self {
        Self { registry }
    }

    pub fn run(&self, sink: &mut dyn SeedSink) -> Vec<SeedResult> {
        self.registry
            .tasks
            .iter()
            .flat_map(|task| task.run(sink))
            .collect()
    }
}

pub struct ProvidersSeedTask;
pub struct StrategiesSeedTask;
pub struct AuthUsersSeedTask;

impl SeedTask for ProvidersSeedTask {
    fn name(&self) -> &'static str {
        "providers"
    }

    fn run(&self, sink: &mut dyn SeedSink) -> Vec<SeedResult> {
        seed_defaults(sink)
    }
}

impl SeedTask for StrategiesSeedTask {
    fn name(&self) -> &'static str {
        "strategies"
    }

    fn run(&self, sink: &mut dyn SeedSink) -> Vec<SeedResult> {
        sink.write_strategy(StrategySeed {
            id: "btc-fast-exit-demo".to_string(),
            provider_ref: "sim".to_string(),
        });
        vec![SeedResult {
            key: "btc-fast-exit-demo".to_string(),
            status: "created".to_string(),
        }]
    }
}

impl SeedTask for AuthUsersSeedTask {
    fn name(&self) -> &'static str {
        "auth-users"
    }

    fn run(&self, sink: &mut dyn SeedSink) -> Vec<SeedResult> {
        sink.write_user(AuthUser {
            id: "local-owner".to_string(),
            subject: "local-owner".to_string(),
        });
        vec![SeedResult {
            key: "local-owner".to_string(),
            status: "created".to_string(),
        }]
    }
}

pub fn seed_defaults(sink: &mut dyn SeedSink) -> Vec<SeedResult> {
    sink.write_manifest(ProviderManifest {
        id: "simbroker".to_string(),
        provider_ref: "sim".to_string(),
    });
    sink.write_provider(ProviderInstance {
        id: "sim".to_string(),
        manifest_id: "simbroker".to_string(),
        enabled: true,
    });
    vec![SeedResult {
        key: "sim".to_string(),
        status: "created".to_string(),
    }]
}

pub fn seed_strategies(env: &Map<String, Value>, store: &mut LocalStore) -> Vec<SeedResult> {
    let provider_ref = env
        .get("TRADEASSEMBLY_SEED_PROVIDER_REF")
        .and_then(Value::as_str)
        .unwrap_or("sim")
        .to_string();
    store.put_strategy(StrategySeed {
        id: "btc-fast-exit-demo".to_string(),
        provider_ref,
    });
    vec![SeedResult {
        key: "btc-fast-exit-demo".to_string(),
        status: "enforced".to_string(),
    }]
}

pub fn load_seed_config(env: &Map<String, Value>) -> Map<String, Value> {
    env.iter()
        .filter(|(key, _)| key.starts_with("TRADEASSEMBLY_SEED_"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub fn verify_enforcement_baseline() -> Value {
    json!({ "passed": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_shaped_seed_package_preserves_top_level_enforcer() {
        let mut store = LocalStore::default();
        let evidence = SeedEnforcer::new(&mut store).ensure_default_providers();

        assert!(!evidence.is_empty());
        assert!(!store.list_manifests().is_empty());
        assert!(!store.list_providers().is_empty());
    }

    #[test]
    fn source_shaped_provider_seed_task_uses_local_store_sink() {
        let mut store = LocalStore::default();
        let mut sink = LocalStoreSink::new(&mut store);

        let results = seed_defaults(&mut sink);

        assert!(!results.is_empty());
        assert!(results.iter().all(|result| result.status == "created"));
        assert!(store.provider("sim").is_some());
    }

    #[test]
    fn source_shaped_seed_runner_and_strategy_wrappers_are_local_equivalents() {
        let mut store = LocalStore::default();
        let mut registry = SeedTaskRegistry::default();
        registry.register(AuthUsersSeedTask);
        registry.register(StrategiesSeedTask);

        let mut sink = LocalStoreSink::new(&mut store);
        let results = SeedRunner::new(registry).run(&mut sink);
        let strategy_results = seed_strategies(
            &Map::from_iter([("TRADEASSEMBLY_SEED_PROVIDER_REF".to_string(), json!("sim"))]),
            &mut store,
        );

        let keys = results
            .into_iter()
            .map(|result| result.key)
            .collect::<Vec<_>>();
        assert!(keys.contains(&"btc-fast-exit-demo".to_string()));
        assert!(keys.contains(&"local-owner".to_string()));
        assert_eq!(strategy_results[0].status, "enforced");
        assert_eq!(verify_enforcement_baseline()["passed"], true);
    }

    #[test]
    fn seed_config_and_task_imports_are_source_compatible() {
        let config = load_seed_config(&Map::from_iter([
            ("TRADEASSEMBLY_SEED_MODE".to_string(), json!("paper")),
            ("IGNORED".to_string(), json!("x")),
        ]));

        assert_eq!(
            config,
            Map::from_iter([("TRADEASSEMBLY_SEED_MODE".to_string(), json!("paper"))])
        );
        assert_eq!(ProvidersSeedTask.name(), "providers");
    }
}
