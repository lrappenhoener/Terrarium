#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
runner=(cargo run --release -p sim-runner --)

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release

"${runner[@]}" --game dummy --ticks 5000 --verify-determinism
"${runner[@]}" --game jungle --ticks 5000 --verify-determinism --verify-save-load \
    --roundtrip-at 1 --roundtrip-at 64 --roundtrip-at 2500 \
    --max-step-allocs 0 --budget-p99-us 10000 --dump-hashes "$scratch/hashes.csv"
grep -Fx '1000,0x5317cfd6e2e12024' "$scratch/hashes.csv"
grep -Fx '5000,0x19eb3e87d333006d' "$scratch/hashes.csv"

"${runner[@]}" --game jungle --ticks 2500 --save-snapshot-at 2500 --snapshot-dir "$scratch/snaps"
"${runner[@]}" --game jungle --load-snapshot "$scratch/snaps/jungle-seed42-t2500.snap" --ticks 5000 \
    --max-step-allocs 0 | tee "$scratch/resume.txt"
grep -Fx 'state_hash 0x19eb3e87d333006d' "$scratch/resume.txt"

"${runner[@]}" --game jungle --ticks 3000 --commands games/jungle-sim/data/commands.toml \
    --record-replay "$scratch/run.simr"
"${runner[@]}" --replay "$scratch/run.simr"
"${runner[@]}" --game jungle --ticks 5000 --profile-phases --worst-ticks 10
status=0
"${runner[@]}" --game dummy --ticks 100 --verify-save-load >"$scratch/dummy.txt" 2>&1 || status=$?
test "$status" -eq 1
grep -F 'unsupported feature: snapshots' "$scratch/dummy.txt"

cargo run --profile release-checked -p sim-runner -- --game jungle --ticks 50000 \
    --verify-determinism | tee "$scratch/checked.txt"
grep -Fx 'Tick 50000 (50000 ausgeführt)' "$scratch/checked.txt"
grep -Fx 'Determinismus OK (500 Checkpoints)' "$scratch/checked.txt"
