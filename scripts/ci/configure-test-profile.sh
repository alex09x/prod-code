#!/usr/bin/env bash
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_ENVIRONMENT:-}" = github-hosted
# Real analyzer regressions retain their interactive deadlines. Optimize their embedded
# parser/type-checker dependencies identically in the cache-warming job and every test shard
# so Mitts ActionKeys match across jobs while keeping the disposable VM bounded.
mkdir -p .cargo
if ! grep -Fq '# ci-disposable-opt-level-1' .cargo/config.toml 2>/dev/null; then
    cat >> .cargo/config.toml <<'EOF'

# ci-disposable-opt-level-1
[profile.dev]
opt-level = 1
[profile.dev.package."*"]
opt-level = 1
[profile.test]
opt-level = 1
[profile.test.package."*"]
opt-level = 1
EOF
fi
