#!/usr/bin/env bash
# Parser-hardening tests for scripts/bench_gate.sh (F-60, #886).
#
# These tests do NOT run cargo bench. They source bench_gate.sh (which exposes
# its validation functions via a BASH_SOURCE sourcing guard) and exercise the
# pure-text-processing logic against synthetic JSON-lines / expected-keys
# fixtures:
#
#   build_expected_keys   — dynamic key set derived from WORKLOADS
#   validate_parsed_cells — missing / duplicate / stray / all-correct / empty
#   gate_check_cell       — ok / regression / unbacked-fail / unbacked-ok
#
# Usage:
#   ./scripts/tests/bench_gate_parser_test.sh
#
# Exits 0 if every assertion passes, 1 otherwise.

set -u

TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE_SCRIPT="$(cd "$TEST_DIR/.." && pwd)/bench_gate.sh"

if [ ! -f "$GATE_SCRIPT" ]; then
    echo "FAIL: cannot find $GATE_SCRIPT" >&2
    exit 1
fi

# Source the gate script. The BASH_SOURCE guard inside bench_gate.sh detects
# that it is being sourced (not executed) and returns before the main flow,
# so cargo bench never runs — only the function definitions + WORKLOADS load.
# shellcheck source=/dev/null
. "$GATE_SCRIPT"

# Confirm the functions actually loaded.
if [ "$(type -t build_expected_keys 2>/dev/null)" != "function" ] \
    || [ "$(type -t validate_parsed_cells 2>/dev/null)" != "function" ] \
    || [ "$(type -t gate_check_cell 2>/dev/null)" != "function" ]; then
    echo "FAIL: bench_gate.sh functions did not load on source" >&2
    exit 1
fi

PASS=0
FAIL=0
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

ok()   { printf '  ok   %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  FAIL %s\n' "$1" >&2; FAIL=$((FAIL + 1)); }

# ---------------------------------------------------------------------------
# Build the canonical expected-keys file once from WORKLOADS.
# ---------------------------------------------------------------------------
EXPECTED="$WORK/expected.keys"
build_expected_keys > "$EXPECTED"
EXPECTED_COUNT="$(wc -l < "$EXPECTED" | tr -d ' ')"

# Helper: write one JSON-lines line for a "<bench>::<cell>" key + ns value.
emit_line() {
    # $1 = "bench::cell"  $2 = ns_per_op
    local k="$1" ns="$2"
    local bench="${k%%::*}"
    local cell="${k#*::}"
    printf '{"bench_name":"%s","cell_id":"%s","ns_per_op":%s}\n' "$bench" "$cell" "$ns"
}

# Helper: build a complete all-correct fixture (one line per expected key).
build_all_correct() {
    local out="$1"
    : > "$out"
    while IFS= read -r k; do
        emit_line "$k" 12345 >> "$out"
    done < "$EXPECTED"
}

# ===================================================================
# build_expected_keys
# ===================================================================

# Key count matches WORKLOADS dynamically (not a hardcoded literal).
if [ "$EXPECTED_COUNT" -eq 9 ]; then
    ok "build_expected_keys emits 9 keys (matches the 9 WORKLOADS entries)"
else
    bad "build_expected_keys emitted $EXPECTED_COUNT keys, expected 9"
fi

# Keys have the "<bench>::<workload_id>" shape (contain "::").
if grep -q '::' "$EXPECTED" && ! grep -qv '::' "$EXPECTED"; then
    ok "all expected keys contain exactly one '::' separator"
else
    bad "expected keys have malformed shape (see $EXPECTED)"
fi

# ===================================================================
# validate_parsed_cells — all correct
# ===================================================================
{
    f="$WORK/all_correct.jsonl"
    build_all_correct "$f"
    if validate_parsed_cells "$f" "$EXPECTED" >/dev/null 2>&1; then
        ok "all-correct fixture (one line per key) validates → exit 0"
    else
        bad "all-correct fixture should pass validation but failed"
    fi
}

# ===================================================================
# validate_parsed_cells — missing key (the core fail-closed gap #1)
# ===================================================================
{
    f="$WORK/missing.jsonl"
    # Drop the first expected key — simulate a bench whose output format
    # drifted so the parser matched zero lines for that cell.
    first_key="$(head -1 "$EXPECTED")"
    tail -n +2 "$EXPECTED" | while IFS= read -r k; do
        emit_line "$k" 12345
    done > "$f"
    if validate_parsed_cells "$f" "$EXPECTED" >/dev/null 2>&1; then
        bad "missing-key fixture should FAIL but passed (fail-OPEN bug!)"
    else
        # Confirm the diagnostic names the specific missing key.
        if validate_parsed_cells "$f" "$EXPECTED" 2>&1 | grep -Fq -- "$first_key"; then
            ok "missing-key fixture fails and names the missing key in the diagnostic"
        else
            bad "missing-key fixture fails but diagnostic doesn't name the missing key"
        fi
    fi
}

# ===================================================================
# validate_parsed_cells — duplicate key (fail-closed gap #3)
# ===================================================================
{
    f="$WORK/dup.jsonl"
    build_all_correct "$f"
    # Append a second copy of the first key.
    emit_line "$(head -1 "$EXPECTED")" 99999 >> "$f"
    if validate_parsed_cells "$f" "$EXPECTED" >/dev/null 2>&1; then
        bad "duplicate-key fixture should FAIL but passed"
    else
        ok "duplicate-key fixture (key appears 2x) fails → exit 1"
    fi
}

# ===================================================================
# validate_parsed_cells — stray/unexpected key
# ===================================================================
{
    f="$WORK/stray.jsonl"
    build_all_correct "$f"
    emit_line "mystery_bench::not_in_workloads" 99999 >> "$f"
    if validate_parsed_cells "$f" "$EXPECTED" >/dev/null 2>&1; then
        bad "stray-key fixture should FAIL but passed"
    else
        ok "stray-key fixture (unexpected key) fails → exit 1"
    fi
}

# ===================================================================
# validate_parsed_cells — empty file (every key missing)
# ===================================================================
{
    f="$WORK/empty.jsonl"
    : > "$f"
    if validate_parsed_cells "$f" "$EXPECTED" >/dev/null 2>&1; then
        bad "empty fixture should FAIL but passed"
    else
        ok "empty fixture fails (all $EXPECTED_COUNT keys reported missing)"
    fi
}

# ===================================================================
# gate_check_cell — ok (within threshold)
# ===================================================================
{
    gate_check_cell "fake::cell" "110" "100" "25" "0" >/dev/null 2>&1
    st="$GATE_CELL_STATUS"
    if [ "$st" = "ok" ]; then
        ok "10% over baseline, 25% threshold → ok"
    else
        bad "10% over baseline should be ok, got '$st'"
    fi
}

# ===================================================================
# gate_check_cell — regression (exceeds threshold)
# ===================================================================
{
    gate_check_cell "fake::cell" "200" "100" "25" "0" >/dev/null 2>&1
    st="$GATE_CELL_STATUS"
    if [ "$st" = "regression" ]; then
        ok "100% over baseline, 25% threshold → regression"
    else
        bad "100% over baseline should be regression, got '$st'"
    fi
}

# ===================================================================
# gate_check_cell — unbacked, default (fail-closed gap #2)
# ===================================================================
{
    gate_check_cell "fake::cell" "150" "" "25" "0" >/dev/null 2>&1
    st="$GATE_CELL_STATUS"
    if [ "$st" = "unbacked-fail" ]; then
        ok "no baseline entry, allow_new=0 → unbacked-fail (gate fails by default)"
    else
        bad "no baseline + default should be unbacked-fail, got '$st'"
    fi
}

# ===================================================================
# gate_check_cell — unbacked, --allow-new-cells (opt-in override)
# ===================================================================
{
    gate_check_cell "fake::cell" "150" "" "25" "1" >/dev/null 2>&1
    st="$GATE_CELL_STATUS"
    if [ "$st" = "unbacked-ok" ]; then
        ok "no baseline entry, allow_new=1 → unbacked-ok (--allow-new-cells defers)"
    else
        bad "no baseline + --allow-new-cells should be unbacked-ok, got '$st'"
    fi
}

# ===================================================================
# gate_check_cell — diagnostic names the unbacked key and the override flag
# ===================================================================
{
    err="$(gate_check_cell "mybench::mywork" "42" "" "25" "0" 2>&1)"
    if printf '%s' "$err" | grep -Fq -- "mybench::mywork" && printf '%s' "$err" | grep -Fq -- "--allow-new-cells"; then
        ok "unbacked-fail diagnostic names the key and mentions --allow-new-cells"
    else
        bad "unbacked-fail diagnostic missing key name or --allow-new-cells hint: $err"
    fi
}

# ===================================================================
echo ""
echo "results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
