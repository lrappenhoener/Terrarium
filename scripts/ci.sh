#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release
cargo run --release -p sim-runner -- --game dummy --ticks 5000 --verify-determinism
cargo run --profile release-checked -p sim-runner -- --game jungle --ticks 50000 --verify-determinism
