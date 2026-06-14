#!/usr/bin/env bash
# Centralised test runner — the SINGLE blessed entry point.
#
# Enforces the CLAUDE.md test conventions so nobody (human or sub-agent)
# reaches for raw `cargo test --workspace --tests 2>&1 | grep ...` again.
# Three rules baked in:
#   1. cargo-nextest, not cargo test  — real-time progress, per-test timeout.
#   2. No pipe to grep                — bash redirect to file, then read file.
#   3. Per-crate by default            — fast signal; full workspace opt-in.
#
# Usage:
#   ./scripts/test.sh                          # lib tests, all crates
#   ./scripts/test.sh --full                   # lib + integration + e2e, all crates
#   ./scripts/test.sh -p shamir-tx             # one crate (lib only)
#   ./scripts/test.sh -p shamir-tx --full      # one crate (lib + tests)
#   ./scripts/test.sh -- mvcc                  # filter by test name
#
# Output: streams test results live; final summary line.
# Exit code: 0 on green, non-zero on any failure / timeout / panic.
#
# Hang protection comes from .config/nextest.toml:
#   default slow-timeout = 30s × 6 = 180s kill
#   wasm_function_* override = 120s × 2 = 240s kill (legit ~99s)
#   shamir-connect SCRAM    = 10s × 6 = 60s kill (Argon2)

set -u

if ! command -v cargo-nextest >/dev/null 2>&1 && ! cargo nextest --version >/dev/null 2>&1; then
    echo "ERROR: cargo-nextest is not installed." >&2
    echo "Install: cargo install cargo-nextest --locked" >&2
    exit 2
fi

mode="lib"            # default: lib tests only — fastest signal
extra_args=()         # forwarded to nextest

while [[ $# -gt 0 ]]; do
    case "$1" in
        --full|-f)
            mode="full"
            shift
            ;;
        --lib|-l)
            mode="lib"
            shift
            ;;
        --)
            shift
            extra_args+=("$@")
            break
            ;;
        *)
            extra_args+=("$1")
            shift
            ;;
    esac
done

case "$mode" in
    lib)
        nextest_args=(--lib --no-fail-fast)
        ;;
    full)
        nextest_args=(--no-fail-fast)
        ;;
esac

echo "» cargo nextest run ${nextest_args[*]} ${extra_args[*]:-}" >&2
exec cargo nextest run "${nextest_args[@]}" "${extra_args[@]}"
