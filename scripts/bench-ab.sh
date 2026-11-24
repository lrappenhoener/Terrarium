#!/usr/bin/env bash
# A/B benchmark of two commits in the same session.
#
#   scripts/bench-ab.sh <commit-a> <commit-b> [runner arguments...]
#
# Builds both commits in separate worktrees (release profile), runs them
# alternately (A, B, A, B, ...) and prints median, minimum and maximum of TPS
# and p99 plus the relative difference of the medians. With valgrind available
# it also counts the instructions of a short run with callgrind.
#
# Environment: PAIRS (default 5), CALLGRIND_TICKS (default 500),
# KEEP_WORKTREES=1 keeps the build directories.
set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <commit-a> <commit-b> [runner arguments...]" >&2
    exit 2
fi
commit_a=$1
commit_b=$2
shift 2
args=("$@")
if [ "${#args[@]}" -eq 0 ]; then
    args=(--game jungle --ticks 5000)
fi
pairs=${PAIRS:-5}
callgrind_ticks=${CALLGRIND_TICKS:-500}
root=$(git rev-parse --show-toplevel)
work=$(mktemp -d)

cleanup() {
    if [ "${KEEP_WORKTREES:-0}" != 1 ]; then
        git -C "$root" worktree remove --force "$work/a" 2>/dev/null || true
        git -C "$root" worktree remove --force "$work/b" 2>/dev/null || true
        rm -rf -- "$work"
    fi
}
trap cleanup EXIT

for side in a b; do
    commit=commit_$side
    git -C "$root" worktree add --quiet --detach "$work/$side" "${!commit}"
    echo "build $side: $(git -C "$work/$side" rev-parse --short HEAD)" >&2
    (cd "$work/$side" && CARGO_TARGET_DIR="$work/target-$side" cargo build --quiet --release -p sim-runner)
done

run() {
    "$work/target-$1/release/sim-runner" "${args[@]}" --report-format json \
        | jq -r '[.average_tps, .latency_us.p99, .state_hash] | @tsv'
}

: >"$work/a.tsv"
: >"$work/b.tsv"
for i in $(seq 1 "$pairs"); do
    for side in a b; do
        line=$(run "$side")
        echo "$line" >>"$work/$side.tsv"
        echo "pair $i $side: $line" >&2
    done
done

stats() {
    sort -g | awk '{v[NR] = $1} END {
        m = (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2
        printf "%.3f %.3f %.3f\n", m, v[1], v[NR]
    }'
}

printf '%-6s %12s %12s %12s %12s %12s %12s  %s\n' commit tps_median tps_min tps_max p99_median p99_min p99_max state_hash
for side in a b; do
    commit=commit_$side
    read -r tm tn tx < <(cut -f1 "$work/$side.tsv" | stats)
    read -r pm pn px < <(cut -f2 "$work/$side.tsv" | stats)
    hash=$(cut -f3 "$work/$side.tsv" | sort -u | paste -sd,)
    printf '%-6s %12s %12s %12s %12s %12s %12s  %s\n' "$side" "$tm" "$tn" "$tx" "$pm" "$pn" "$px" "$hash"
    eval "tps_$side=$tm p99_$side=$pm"
done
awk -v a="$tps_a" -v b="$tps_b" -v pa="$p99_a" -v pb="$p99_b" 'BEGIN {
    printf "TPS B vs A: %+.2f %%   p99 B vs A: %+.2f %%\n", (b - a) / a * 100, (pb - pa) / pa * 100
}'

if command -v valgrind >/dev/null 2>&1; then
    short=()
    skip=0
    for arg in "${args[@]}"; do
        if [ "$skip" = 1 ]; then
            skip=0
            continue
        fi
        if [ "$arg" = --ticks ]; then
            skip=1
            continue
        fi
        short+=("$arg")
    done
    short+=(--ticks "$callgrind_ticks")
    for side in a b; do
        valgrind --tool=callgrind --callgrind-out-file="$work/callgrind-$side.out" \
            "$work/target-$side/release/sim-runner" "${short[@]}" >/dev/null 2>"$work/callgrind-$side.log"
        count=$(grep -E 'Collected :' "$work/callgrind-$side.log" | awk '{print $NF}')
        eval "ir_$side=$count"
        echo "instructions $side (${callgrind_ticks} ticks): $count"
    done
    awk -v a="$ir_a" -v b="$ir_b" 'BEGIN { printf "instructions B vs A: %+.3f %%\n", (b - a) / a * 100 }'
else
    echo "valgrind not available: no instruction counts"
fi
