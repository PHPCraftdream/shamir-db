#!/usr/bin/env bash
# One-pass "how fast is the codebase" sweep.
#
# Runs every real [[bench]] target in the workspace ONCE in Criterion's
# default QUICK mode (sample_size=10, measurement=1s, warm_up=1s — set by
# shamir_bench_utils::tune), then harvests each bench's `mean.point_estimate`
# (nanoseconds PER ITERATION) from Criterion's estimates.json and sums them
# into a single aggregate speed number.
#
# Why ns/op and NOT wall-clock: Criterion is time-adaptive — a time-capped
# run (`--profile-time`) just does more iterations when the code is faster,
# so total wall time is ~constant and useless as a speed signal. The
# per-iteration estimate (ns/op) is already normalized by iteration count,
# so it DROPS when the code gets faster. SUM(ns/op) over all benches is the
# aggregate: lower = faster codebase. This is the metric to diff before/after
# an optimization — NOT the sweep wall time.
#
# NOTE: this is a rough single-pass aggregate, NOT the /opti methodology
# (which compares baseline vs. post-change with full statistical rigor per
# bench). Use it for a quick "did the whole system get faster/slower" read.
#
# Usage:
#   ./scripts/bench-quick-all.sh                 # whole workspace
#   ./scripts/bench-quick-all.sh shamir-engine   # one crate only
#
# Uses the dedicated bench target dir (CLAUDE.md rule) so it never
# invalidates the debug/test incremental cache.

set -uo pipefail

ONLY_CRATE="${1:-}"
BENCH_TARGET_DIR="D:/dev/rust/.cargo-target-bench"

cd "$(dirname "$0")/.."

# Discover every (crate, bench-name) pair from [[bench]] sections directly,
# so we never accidentally invoke a plain `unittests` binary (those don't
# understand Criterion flags).
targets=()
for f in $(grep -rl '\[\[bench\]\]' --include=Cargo.toml crates/*/Cargo.toml); do
    crate=$(basename "$(dirname "$f")")
    if [[ -n "$ONLY_CRATE" && "$crate" != "$ONLY_CRATE" ]]; then
        continue
    fi
    names=$(grep -A2 '\[\[bench\]\]' "$f" | grep '^name' | sed 's/name = "\(.*\)"/\1/')
    for n in $names; do
        targets+=("$crate:$n")
    done
done

echo "==> ${#targets[@]} bench targets (QUICK mode), harvesting ns/op"
echo "==> target dir: $BENCH_TARGET_DIR"

# Marker: only estimates.json newer than this belong to THIS sweep — keeps
# stale/foreign criterion output out of the aggregate.
mkdir -p "$BENCH_TARGET_DIR"
marker="$BENCH_TARGET_DIR/.bench_sweep_marker"
: > "$marker"
sleep 1

start=$(date +%s)
fails=0
for t in "${targets[@]}"; do
    crate="${t%%:*}"
    name="${t##*:}"
    echo "--- $crate::$name ---"
    if ! CARGO_TARGET_DIR="$BENCH_TARGET_DIR" cargo bench -p "$crate" --bench "$name" 2>&1 | tail -2; then
        echo "!!! $crate::$name FAILED (continuing)"
        fails=$((fails + 1))
    fi
done
end=$(date +%s)

# Harvest: every fresh estimates.json → (ns/op, identity), sorted heaviest
# first, and the grand total.
echo
echo "==> per-bench mean ns/op (heaviest first):"
tmp=$(mktemp)
find "$BENCH_TARGET_DIR/criterion" -name estimates.json -path '*/new/*' -newer "$marker" 2>/dev/null \
    | while IFS= read -r f; do
        id=$(echo "$f" | sed "s|.*/criterion/||; s|/new/estimates.json||")
        val=$(jq -r '.mean.point_estimate' "$f" 2>/dev/null)
        [[ -n "$val" ]] && printf "%s\t%s\n" "$val" "$id"
    done | sort -rn > "$tmp"

head -50 "$tmp" | awk -F'\t' '{printf "  %14.1f ns  %s\n", $1, $2}'
count=$(wc -l < "$tmp")
total=$(awk -F'\t' '{s+=$1} END {printf "%.1f", s}' "$tmp")
rm -f "$tmp"

elapsed=$((end - start))
echo
echo "==> $count bench points, AGGREGATE SUM(mean ns/op) = $total ns"
echo "==> sweep wall time: ${elapsed}s ($((elapsed / 60))m $((elapsed % 60))s), $fails target(s) failed"
