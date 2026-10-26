# Copyright (c) 2026 OptionLab LLC. All rights reserved.
default:
    @just --list

verify: verify-crypto-consumers
    cargo xtask verify

setup: prepare-crypto-consumers
    cargo xtask setup

check:
    cargo xtask check

build:
    cargo xtask build

# Owning wire and durable-journal contracts; no hosted dependency.
contract-test:
    cargo test --locked -p tradeassembly-core-binaries --test runner_compatibility
    cargo test --locked -p tradeassembly-runtime --test journal_read_contract
    cargo test --locked -p tradeassembly-runtime --test public_workspace
    cargo test --locked -p tradeassembly-runtime --test backtest_engine public_workspace_roundtrip
    cargo test --locked -p tradeassembly-runtime --lib authentication_method_is_

# Targeted iteration. Cargo selects dependencies; no clean, provider, or install.
build-target package binary:
    cargo build --locked --package '{{package}}' --bin '{{binary}}'

test-package package:
    cargo test --locked --package '{{package}}'

# Exact development artifacts consumed by the coordinator's local manifest.
build-artifacts:
    cargo build --locked --package tradeassembly-core-binaries --bin tradeassembly
    cargo build --locked --package tradeassembly-core-binaries --bin tradeassembly-core-runner

prepare-crypto-consumers cache="target/dependency-preparation/cache" output="vendor":
    @mkdir -p target/dependency-preparation
    @rustc --edition=2021 tools/prepare-crypto-consumers.rs -o target/dependency-preparation/prepare-crypto-consumers
    @target/dependency-preparation/prepare-crypto-consumers '{{cache}}' '{{output}}' --fetch

verify-crypto-consumers:
    @mkdir -p target/dependency-preparation
    @rustc --edition=2021 --test tools/prepare-crypto-consumers.rs -o target/dependency-preparation/source-tests
    @target/dependency-preparation/source-tests
    @rustc --edition=2021 tools/prepare-crypto-consumers.rs -o target/dependency-preparation/prepare-crypto-consumers
    @target/dependency-preparation/prepare-crypto-consumers target/dependency-preparation/cache vendor
