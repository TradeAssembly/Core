// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::seed::{BtcSeedConfig, LocalStore, SeedService};

#[test]
fn btc_seed_is_idempotent_and_uses_stable_records() {
    let mut store = LocalStore::default();
    let mut service = SeedService::new(&mut store);

    let first = service
        .seed_btc_exit_demo(BtcSeedConfig::new("sim", "paper"))
        .expect("first seed");
    let second = service
        .seed_btc_exit_demo(BtcSeedConfig::new("sim", "paper"))
        .expect("second seed");

    assert_eq!(first, second);
    assert_eq!(first.strategy_id, "strat_btcfastexitdemo");
    assert_eq!(first.version_id, "ver_btcfastexitdemo");
    assert_eq!(first.config_id, "cfg_btcfastexitdemo");
    assert_eq!(service.store().strategy_count("BTC fast exit demo"), 1);
    assert_eq!(
        service.store().config_provider("cfg_btcfastexitdemo"),
        Some("sim")
    );
    assert_eq!(service.store().event_count("seed.enforced"), 2);
}

#[test]
fn btc_seed_updates_mode_without_creating_duplicates() {
    let mut store = LocalStore::default();
    let mut service = SeedService::new(&mut store);

    let first = service
        .seed_btc_exit_demo(BtcSeedConfig::new("sim", "paper"))
        .expect("first seed");
    let updated = service
        .seed_btc_exit_demo(BtcSeedConfig::new("sim", "backtest"))
        .expect("updated seed");

    assert_eq!(updated, first);
    assert_eq!(
        service.store().config_provider(&first.config_id),
        Some("sim")
    );
    assert_eq!(
        service.store().config_mode(&first.config_id),
        Some("backtest")
    );
    assert_eq!(service.store().config_count(&first.strategy_id), 1);
}

#[test]
fn btc_seed_fails_closed_when_requested_provider_is_missing() {
    let mut store = LocalStore::default();
    let mut service = SeedService::new(&mut store);

    let error = service
        .seed_btc_exit_demo(BtcSeedConfig::new("missing-provider", "paper"))
        .expect_err("missing provider");

    assert!(error.contains("seed provider is not configured"));
}
