#!/usr/bin/env bash
# F-53d (#877) — release-blocking performance gate.
#
# Runs a fixed, representative set of bench-scale-tool cells (one per named
# workload category — see WORKLOADS below), parses each cell's `ns/op` from
# the bench binary's own stdout (the harness's normal fixed-iteration output
# — no upstream `bench-scale-tool` changes needed, see the module docs in
# `docs/dev-artifacts/prompts/post-alpha/110-f53d-ci-perf-gates-self-hosted.md`),
# emits one JSON line per cell (this IS the "CI-output mode" — a thin
# repo-local wrapper around the existing text output, per that brief's item
# 1: `bench-scale-tool` is consumed here as the published crates.io package,
# not vendored, so it is never patched directly), and compares the fresh
# number against a committed baseline (`bench-baseline.json`, repo root).
#
# Usage:
#   ./scripts/bench_gate.sh                    # gate: compare vs bench-baseline.json, exit non-zero on regression
#   ./scripts/bench_gate.sh --capture-baseline # (re)write bench-baseline.json from a fresh run, exit 0
#   ./scripts/bench_gate.sh --json-only        # just emit the per-cell JSON lines, no baseline comparison
#
# Env:
#   BENCH_GATE_THRESHOLD_PCT   override the regression threshold (default 25 — see the
#                              runbook / brief for why 25% is the chosen first-cut number).
#   CARGO_TARGET_DIR           forwarded to `cargo bench` unchanged if already set;
#                              otherwise defaults to an isolated dir per CLAUDE.md's
#                              bench-cache-isolation rule so this never invalidates the
#                              debug/test incremental cache.
#
# This script assumes the target cells are ALREADY CALIBRATED in the
# committed `bench-iters.txt` (the fixed-iteration count is the whole point
# of bench-scale-tool — see that file's own header). It does not calibrate;
# calibration is a separate, occasional, hand-run step
# (`cargo bench -p <crate> --bench <bench> -- --calibrate <secs>`) done when
# a cell's count has drifted, same as every other bench in this workspace.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE_FILE="$REPO_ROOT/bench-baseline.json"
THRESHOLD_PCT="${BENCH_GATE_THRESHOLD_PCT:-25}"

if [ -z "${CARGO_TARGET_DIR:-}" ]; then
    export CARGO_TARGET_DIR="$REPO_ROOT/.cargo-target-bench"
fi

MODE="gate"
for arg in "$@"; do
    case "$arg" in
        --capture-baseline) MODE="capture" ;;
        --json-only) MODE="json-only" ;;
        -h|--help)
            sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "!! unknown argument: $arg (see --help)" >&2
            exit 2
            ;;
    esac
done

# ---------------------------------------------------------------------------
# The 8 named workload categories (F-53d brief), one representative cell
# each. Each entry is "<crate>::<bench-binary>::<workload-id>" — the
# workload-id matches EXACTLY the `<bin>::<id>` key bench-scale-tool writes
# to `bench-iters.txt` (see that file's own header comment for the format).
#
#   category            crate            bench binary          workload id
#   ------------------  ---------------  ---------------------  --------------------------------------------
#   point get           shamir-db        engine_perf            cache_hit_get/1000
#   point set           shamir-db        engine_perf            set_existing_with_index/1000
#   scans               shamir-db        engine_perf             count_all_no_filter/1000
#   indexed lookup      shamir-db        engine_perf             min_only_with_index/1000
#   ORDER BY LIMIT K    shamir-engine    order_by_pipeline       order_by_indexed_field_limit_100/score_asc_limit_100
#   commit (percentile  shamir-engine    tx_pipeline             commit_tx/phases/baseline_empty
#     proxy — see note below)
#   FK cascade workload shamir-engine    fk_cascade_index        cascade_index_fast_path/5000
#   cursor pages deep   shamir-engine    cursor_pages_depth      pages_deep/10
#   startup/recovery    shamir-wal       wal_startup_open        with_sidecar/segs_64
#
# NOTE on "commit percentiles": bench-scale-tool is a fixed-iteration
# harness (see its own module docs) — it reports one ns/op per cell, not a
# percentile distribution. There is no p50/p95/p99 SAMPLE in this model.
# `commit_tx/phases/baseline_empty` (the cheapest, most stable commit-path
# cell) stands in as the commit-latency proxy for this first-cut gate. A
# genuine percentile gate would need a different bench-scale-tool workload
# shape (repeated per-call sampling instead of bulk-timed iteration) — out
# of scope for this task; noted here and in the runbook as a known gap.
WORKLOADS="
shamir-db::engine_perf::cache_hit_get/1000
shamir-db::engine_perf::set_existing_with_index/1000
shamir-db::engine_perf::count_all_no_filter/1000
shamir-db::engine_perf::min_only_with_index/1000
shamir-engine::order_by_pipeline::order_by_indexed_field_limit_100/score_asc_limit_100
shamir-engine::tx_pipeline::commit_tx/phases/baseline_empty
shamir-engine::fk_cascade_index::cascade_index_fast_path/5000
shamir-engine::cursor_pages_depth::pages_deep/10
shamir-wal::wal_startup_open::with_sidecar/segs_64
"

JSON_LINES_FILE="$(mktemp)"
trap 'rm -f "$JSON_LINES_FILE"' EXIT

FAILED_BUILDS=0

for entry in $WORKLOADS; do
    crate="${entry%%::*}"
    rest="${entry#*::}"
    bench="${rest%%::*}"
    workload_id="${rest#*::}"

    echo "==> running ${crate}::${bench} (workload \`${workload_id}\`)" >&2
    stdout="$(cd "$REPO_ROOT" && cargo bench -p "$crate" --bench "$bench" -- --scale 1 "$workload_id" 2>&2)"
    status=$?
    if [ "$status" -ne 0 ]; then
        echo "!! ${crate}::${bench} exited $status" >&2
        FAILED_BUILDS=$((FAILED_BUILDS + 1))
        continue
    fi

    # Parse the harness's fixed-iteration data lines:
    #   "{id} {n} iters {ms} ms {ns_per} ns/op"
    # Same 7-whitespace-token shape bench-scale-tool's own bench-cli
    # (`parse_bench_output`) recognizes — duplicated here (not imported)
    # because bench-cli is a separate crates.io binary, not a library this
    # repo depends on.
    echo "$stdout" | awk -v bench="$bench" '
        NF == 7 && $3 == "iters" && $5 == "ms" && $7 == "ns/op" {
            printf "{\"bench_name\":\"%s\",\"cell_id\":\"%s\",\"ns_per_op\":%s}\n", bench, $1, $6
        }
    ' >> "$JSON_LINES_FILE"
done

echo "" >&2
echo "== per-cell JSON (bench CI-output mode) ==" >&2
cat "$JSON_LINES_FILE"

if [ "$FAILED_BUILDS" -gt 0 ]; then
    echo "!! $FAILED_BUILDS bench target(s) failed to run — see above" >&2
    exit 1
fi

if [ "$MODE" = "json-only" ]; then
    exit 0
fi

if [ "$MODE" = "capture" ]; then
    {
        echo "{"
        first=1
        while IFS= read -r line; do
            bench_name=$(echo "$line" | awk -F'"bench_name":"' '{print $2}' | awk -F'"' '{print $1}')
            cell_id=$(echo "$line" | awk -F'"cell_id":"' '{print $2}' | awk -F'"' '{print $1}')
            ns=$(echo "$line" | awk -F'"ns_per_op":' '{print $2}' | tr -d '}')
            key="${bench_name}::${cell_id}"
            [ "$first" -eq 1 ] || echo ","
            first=0
            printf '  "%s": %s' "$key" "$ns"
        done < "$JSON_LINES_FILE"
        echo ""
        echo "}"
    } > "$BASELINE_FILE"
    echo "" >&2
    echo "==> baseline captured: $BASELINE_FILE" >&2
    cat "$BASELINE_FILE" >&2
    exit 0
fi

# ---------------------------------------------------------------------------
# Gate mode: compare each fresh cell against the committed baseline.
# ---------------------------------------------------------------------------

if [ ! -f "$BASELINE_FILE" ]; then
    echo "!! no baseline found at $BASELINE_FILE — run with --capture-baseline first" >&2
    exit 1
fi

echo "" >&2
echo "== comparing against baseline (threshold ${THRESHOLD_PCT}%) ==" >&2

REGRESSIONS=0
while IFS= read -r line; do
    [ -z "$line" ] && continue
    bench_name=$(echo "$line" | awk -F'"bench_name":"' '{print $2}' | awk -F'"' '{print $1}')
    cell_id=$(echo "$line" | awk -F'"cell_id":"' '{print $2}' | awk -F'"' '{print $1}')
    ns=$(echo "$line" | awk -F'"ns_per_op":' '{print $2}' | tr -d '}')
    key="${bench_name}::${cell_id}"

    # Extract this key's baseline value from the flat JSON object
    # (`"<key>": <number>` per line, see the --capture-baseline writer above).
    baseline_ns=$(grep -F "\"$key\":" "$BASELINE_FILE" | head -1 | awk -F': ' '{print $2}' | tr -d ',' | tr -d ' ')

    if [ -z "$baseline_ns" ]; then
        echo "   (new) $key = ${ns} ns/op — no baseline entry, not gated" >&2
        continue
    fi

    pct=$(awk -v fresh="$ns" -v base="$baseline_ns" 'BEGIN { if (base == 0) { print "0"; } else { printf "%.2f", (fresh - base) / base * 100.0 } }')
    is_regression=$(awk -v pct="$pct" -v thr="$THRESHOLD_PCT" 'BEGIN { print (pct > thr) ? "1" : "0" }')

    if [ "$is_regression" = "1" ]; then
        echo "   REGRESSION  $key: baseline=${baseline_ns} fresh=${ns} (+${pct}%, threshold ${THRESHOLD_PCT}%)" >&2
        REGRESSIONS=$((REGRESSIONS + 1))
    else
        echo "   ok          $key: baseline=${baseline_ns} fresh=${ns} (${pct}%)" >&2
    fi
done < "$JSON_LINES_FILE"

echo "" >&2
if [ "$REGRESSIONS" -gt 0 ]; then
    echo "!! $REGRESSIONS cell(s) regressed beyond ${THRESHOLD_PCT}% — gate FAILED" >&2
    exit 1
fi

echo "==> all cells within ${THRESHOLD_PCT}% of baseline — gate PASSED" >&2
exit 0
