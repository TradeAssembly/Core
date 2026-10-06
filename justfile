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
    cargo test --locked -p tradeassembly-runtime --test runner_compatibility --test journal_read_contract
