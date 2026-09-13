#!/usr/bin/env bash
# Copyright (c) 2026 OptionLab LLC. All rights reserved.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

report_path="${TRADEASSEMBLY_LANGUAGE_INVARIANT_REPORT:-.sdlc/language-invariant-report.json}"

exec cargo run --manifest-path Cargo.toml --package tradeassembly-xtask -- \
  check-whitelist --format json --report "$report_path" "$@"
