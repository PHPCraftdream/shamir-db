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
# FAIL-CLOSED PARSER (F-60, #886): after parsing, the JSON-lines file is
# structurally validated in EVERY mode — every expected workload cell must
# produce exactly one line, no duplicates, no stray keys. A format drift
# that makes the parser match zero lines for a cell is a HARD ERROR, not
# silently ignored. In gate mode, a cell with no baseline entry is also a
# hard error by default (a gate that silently skips cells it doesn't
# recognize is not actually gating) — pass --allow-new-cells to override
# when deliberately introducing a new workload before its baseline exists.
#
# Usage:
#   ./scripts/bench_gate.sh                    # gate: compare vs bench-baseline.json, exit non-zero on regression
#   ./scripts/bench_gate.sh --capture-baseline # (re)write bench-baseline.json from a fresh run, exit 0
#   ./scripts/bench_gate.sh --json-only        # just emit the per-cell JSON lines, no baseline comparison
#   ./scripts/bench_gate.sh --allow-new-cells  # gate mode: permit cells that have no baseline entry yet (do not fail on them)
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

# ---------------------------------------------------------------------------
# The 9 named workload categories (F-53d brief), one representative cell
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

# ---------------------------------------------------------------------------
# build_expected_keys — emit one "<bench>::<workload_id>" key per line for
# each WORKLOADS entry. This is the canonical key shape the awk parser writes
# (bench_name = the bench binary; cell_id = the workload id) and the one the
# baseline JSON uses — derived dynamically from WORKLOADS so the expected set
# can never drift out of sync with the workload list (no hardcoded count).
# ---------------------------------------------------------------------------
build_expected_keys() {
    local entry rest bench workload_id
    for entry in $WORKLOADS; do
        rest="${entry#*::}"
        bench="${rest%%::*}"
        workload_id="${rest#*::}"
        printf '%s::%s\n' "$bench" "$workload_id"
    done
}

# ---------------------------------------------------------------------------
# validate_parsed_cells — fail-closed structural validation of the parsed
# JSON-lines file. Runs in EVERY mode (gate / capture / json-only): a
# malformed parse is a bug regardless of what the caller does with the data.
#
# Args:
#   $1  json_lines_file     the file produced by the per-workload awk parse
#   $2  expected_keys_file  one "<bench>::<workload_id>" key per line
#
# Every expected key must appear EXACTLY once — a zero count means the bench
# crashed silently or its stdout format drifted past the parser (NOT silently
# ignored); a count > 1 means the bench printed an extra data row or WORKLOADS
# has a copy-paste duplicate. No key outside the expected set may appear
# either (a defensive check — structurally it shouldn't happen given the
# parser runs once per WORKLOADS entry, but if a bench legitimately prints
# multiple rows the caller needs to know). Returns 0 on success, 1 (with a
# diagnostic on stderr) on any violation.
# ---------------------------------------------------------------------------
validate_parsed_cells() {
    local json_file="$1"
    local expected_file="$2"
    local errors=0
    local missing_list="" dup_list="" stray_list=""

    # Extract the "<bench>::<cell>" key from every parsed line for counting.
    local actual_file bn ci line
    actual_file="$(mktemp)"
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        bn="$(printf '%s' "$line" | awk -F'"bench_name":"' '{print $2}' | awk -F'"' '{print $1}')"
        ci="$(printf '%s' "$line" | awk -F'"cell_id":"' '{print $2}' | awk -F'"' '{print $1}')"
        printf '%s::%s\n' "$bn" "$ci"
    done < "$json_file" > "$actual_file"

    # Missing (zero) / duplicate (>1) expected keys.
    local exp cnt
    while IFS= read -r exp; do
        [ -z "$exp" ] && continue
        cnt="$(grep -Fxc -- "$exp" "$actual_file" 2>/dev/null)"
        [ -z "$cnt" ] && cnt=0
        if [ "$cnt" -eq 0 ]; then
            missing_list="${missing_list}  - ${exp}"$'\n'
            errors=$((errors + 1))
        elif [ "$cnt" -gt 1 ]; then
            dup_list="${dup_list}  - ${exp} (${cnt} occurrences)"$'\n'
            errors=$((errors + 1))
        fi
    done < "$expected_file"

    # Stray keys — present in the parse but not in the expected set.
    local act
    while IFS= read -r act; do
        [ -z "$act" ] && continue
        if ! grep -Fxq -- "$act" "$expected_file" 2>/dev/null; then
            stray_list="${stray_list}  - ${act}"$'\n'
            errors=$((errors + 1))
        fi
    done < "$actual_file"

    rm -f "$actual_file"

    if [ "$errors" -gt 0 ]; then
        echo "!! FAIL: parsed bench output failed structural validation (${errors} problem(s))" >&2
        echo "   a release-blocking gate must see exactly one line per expected" >&2
        echo "   workload cell and nothing else:" >&2
        if [ -n "$missing_list" ]; then
            echo "   MISSING — parser produced no line for these expected workloads" >&2
            echo "   (the bench crashed silently or its stdout format no longer" >&2
            echo "   matches the parser; NOT silently ignored):" >&2
            printf '%s' "$missing_list" >&2
        fi
        if [ -n "$dup_list" ]; then
            echo "   DUPLICATE — these keys appeared more than once:" >&2
            printf '%s' "$dup_list" >&2
        fi
        if [ -n "$stray_list" ]; then
            echo "   UNEXPECTED — these keys are not in the expected WORKLOADS set:" >&2
            printf '%s' "$stray_list" >&2
        fi
        return 1
    fi
    return 0
}

# ---------------------------------------------------------------------------
# gate_check_cell — compare one fresh cell against its baseline. Sets the
# global GATE_CELL_STATUS to one of:
#   ok             within threshold
#   regression     exceeds threshold
#   unbacked-fail  no baseline entry — gate FAILS (default)
#   unbacked-ok    no baseline entry, allowed via --allow-new-cells
# Prints a diagnostic line to stderr in every case.
#
# Args: $1=key  $2=fresh_ns  $3=baseline_ns_or_empty  $4=threshold  $5=allow_new(0/1)
# ---------------------------------------------------------------------------
gate_check_cell() {
    local key="$1" fresh_ns="$2" baseline_ns="$3" threshold="$4" allow_new="$5"
    if [ -z "$baseline_ns" ]; then
        if [ "$allow_new" -eq 1 ]; then
            echo "   (new)      $key = ${fresh_ns} ns/op — no baseline entry, allowed via --allow-new-cells" >&2
            GATE_CELL_STATUS="unbacked-ok"
            return 0
        fi
        echo "   UNBACKED   $key = ${fresh_ns} ns/op — no baseline entry (capture one with --capture-baseline, or pass --allow-new-cells)" >&2
        GATE_CELL_STATUS="unbacked-fail"
        return 0
    fi
    local pct is_reg
    pct=$(awk -v fresh="$fresh_ns" -v base="$baseline_ns" 'BEGIN { if (base == 0) { print "0"; } else { printf "%.2f", (fresh - base) / base * 100.0 } }')
    is_reg=$(awk -v pct="$pct" -v thr="$threshold" 'BEGIN { print (pct > thr) ? "1" : "0" }')
    if [ "$is_reg" = "1" ]; then
        echo "   REGRESSION $key: baseline=${baseline_ns} fresh=${fresh_ns} (+${pct}%, threshold ${threshold}%)" >&2
        GATE_CELL_STATUS="regression"
    else
        echo "   ok         $key: baseline=${baseline_ns} fresh=${fresh_ns} (${pct}%)" >&2
        GATE_CELL_STATUS="ok"
    fi
    return 0
}

# ===========================================================================
# Main flow — skipped when this file is sourced by the parser test
# (scripts/tests/bench_gate_parser_test.sh), so only the functions above load.
# ===========================================================================
if [ "${BASH_SOURCE[0]:-$0}" != "$0" ]; then
    return 0 2>/dev/null || true
fi

MODE="gate"
ALLOW_NEW_CELLS=0
for arg in "$@"; do
    case "$arg" in
        --capture-baseline) MODE="capture" ;;
        --json-only) MODE="json-only" ;;
        --allow-new-cells) ALLOW_NEW_CELLS=1 ;;
        -h|--help)
            sed -n '2,/^set -u$/p' "$0" | sed '/^set -u$/d; s/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "!! unknown argument: $arg (see --help)" >&2
            exit 2
            ;;
    esac
done

JSON_LINES_FILE="$(mktemp)"
EXPECTED_KEYS_FILE="$(mktemp)"
trap 'rm -f "$JSON_LINES_FILE" "$EXPECTED_KEYS_FILE"' EXIT
build_expected_keys > "$EXPECTED_KEYS_FILE"

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

# Fail-closed structural validation — runs in EVERY mode (gate / capture /
# json-only). A malformed parse is a bug regardless of what comes next.
if ! validate_parsed_cells "$JSON_LINES_FILE" "$EXPECTED_KEYS_FILE"; then
    exit 1
fi

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
UNBACKED_CELLS=0
while IFS= read -r line; do
    [ -z "$line" ] && continue
    bench_name=$(echo "$line" | awk -F'"bench_name":"' '{print $2}' | awk -F'"' '{print $1}')
    cell_id=$(echo "$line" | awk -F'"cell_id":"' '{print $2}' | awk -F'"' '{print $1}')
    ns=$(echo "$line" | awk -F'"ns_per_op":' '{print $2}' | tr -d '}')
    key="${bench_name}::${cell_id}"

    # Extract this key's baseline value from the flat JSON object
    # (`"<key>": <number>` per line, see the --capture-baseline writer above).
    baseline_ns=$(grep -F "\"$key\":" "$BASELINE_FILE" | head -1 | awk -F': ' '{print $2}' | tr -d ',' | tr -d ' ')

    gate_check_cell "$key" "$ns" "$baseline_ns" "$THRESHOLD_PCT" "$ALLOW_NEW_CELLS"
    case "$GATE_CELL_STATUS" in
        regression)    REGRESSIONS=$((REGRESSIONS + 1)) ;;
        unbacked-fail) UNBACKED_CELLS=$((UNBACKED_CELLS + 1)) ;;
    esac
done < "$JSON_LINES_FILE"

echo "" >&2
if [ "$REGRESSIONS" -gt 0 ] || [ "$UNBACKED_CELLS" -gt 0 ]; then
    echo "!! $REGRESSIONS cell(s) regressed, $UNBACKED_CELLS cell(s) unbacked — gate FAILED" >&2
    exit 1
fi

echo "==> all cells within ${THRESHOLD_PCT}% of baseline — gate PASSED" >&2
exit 0
