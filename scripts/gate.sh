#!/usr/bin/env bash
set -euo pipefail
cargo fmt
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
