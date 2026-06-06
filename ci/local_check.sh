#!/usr/bin/env bash
# Canonical local CI: fmt + clippy(-D warnings) + tests + wasm-core guard.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
# The sans-io core must stay wasm-safe for extension consumers.
cargo check --no-default-features --target wasm32-wasip2
