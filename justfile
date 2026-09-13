# Copyright (c) 2026 OptionLab LLC. All rights reserved.
default:
    @just --list

verify:
    cargo xtask verify

setup:
    cargo xtask setup

check:
    cargo xtask architecture-core
