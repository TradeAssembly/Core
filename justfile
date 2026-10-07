# Copyright (c) 2026 OptionLab LLC. All rights reserved.
default:
    @just --list

verify:
    cargo xtask verify

setup:
    cargo xtask setup

check:
    cargo xtask check

build:
    cargo xtask build

# Owning wire and durable-journal contracts; no hosted dependency.
contract-test:
    cargo test --locked -p tradeassembly-core-binaries --test runner_compatibility
    cargo test --locked -p tradeassembly-runtime --test journal_read_contract

# Targeted iteration. Cargo selects dependencies; no clean, provider, or install.
build-target package binary:
    cargo build --locked --package '{{package}}' --bin '{{binary}}'

test-package package:
    cargo test --locked --package '{{package}}'

# Exact development artifacts consumed by the coordinator's local manifest.
build-artifacts:
    cargo build --locked --package tradeassembly-core-binaries --bin tradeassembly
    cargo build --locked --package tradeassembly-core-binaries --bin tradeassembly-core-runner
