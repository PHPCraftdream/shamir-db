#!/usr/bin/env bash
# Centralised test runner — the SINGLE blessed entry point.
#
# Enforces the CLAUDE.md test conventions so nobody (human or sub-agent)
# reaches for raw `cargo test --workspace --tests 2>&1 | grep ...` again.
# Three rules baked in:
#   1. cargo-nextest, not cargo test  — real-time progress, per-test timeout.
#   2. No pipe to grep                — nextest streams output directly.
#   3. Per-crate / per-scope by default — fast signal; full workspace opt-in.
#
# Usage:
#   ./scripts/test.sh                          # lib tests, all crates
#   ./scripts/test.sh --full                   # lib + integration + e2e, all crates
#   ./scripts/test.sh -p shamir-tx             # one crate (lib only)
#   ./scripts/test.sh -p shamir-tx --full      # one crate (lib + tests)
#   ./scripts/test.sh -- mvcc                  # filter by test name
#   ./scripts/test.sh -p shamir-tx -p shamir-engine   # multiple crates
#
# Named scopes (preset groups — see SCOPES below):
#   ./scripts/test.sh @tx                      # shamir-tx only, lib
#   ./scripts/test.sh @oracle                  # tx + engine (Version Oracle area)
#   ./scripts/test.sh @oracle --full           # + integration tests
#   ./scripts/test.sh @e2e                     # all e2e suites (--full implied)
#   ./scripts/test.sh @types @tx               # combine scopes
#   ./scripts/test.sh @oracle -- watermark     # scope + name filter
#
# Available scopes (extend as needed):
#   @tx       — shamir-tx
#   @engine   — shamir-engine
#   @oracle   — shamir-tx + shamir-engine (Version Oracle area)
#   @types    — shamir-types + shamir-collections
#   @storage  — shamir-storage + shamir-wal
#   @server   — shamir-server + shamir-connect
#   @e2e      — shamir-db + shamir-server (forces --full)
#   @all      — every workspace crate (explicit; same as no -p)
#
# Power-user: pass `-E '<nextest-filter-expression>'` for arbitrary
# nextest filter expressions (e.g. `'package(shamir-tx) and test(/mvcc.*/)'`).
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

# ---------------------------------------------------------------------------
# Scope dictionary — short names → list of -p <crate> args.
# Add entries as the codebase grows; keep them short and topical.
# ---------------------------------------------------------------------------
scope_args() {
    case "$1" in
        @tx)       echo "-p shamir-tx" ;;
        @engine)   echo "-p shamir-engine" ;;
        @oracle)   echo "-p shamir-tx -p shamir-engine" ;;
        @types)    echo "-p shamir-types -p shamir-collections" ;;
        @storage)  echo "-p shamir-storage -p shamir-wal" ;;
        @server)   echo "-p shamir-server -p shamir-connect" ;;
        @e2e)      echo "-p shamir-db -p shamir-server" ;;
        @all)      echo "" ;;  # nextest default = workspace
        @*)
            echo "ERROR: unknown scope '$1'. See ./scripts/test.sh --help." >&2
            return 1
            ;;
        *)
            echo "ERROR: scope_args called with non-scope '$1'" >&2
            return 1
            ;;
    esac
}

print_help() {
    sed -n '/^# Usage:/,/^# Hang protection/p' "$0" | sed 's/^# //;s/^#//'
    exit 0
}

mode="lib"            # default: lib tests only — fastest signal
forces_full=""        # named scopes can force --full (e.g. @e2e)
extra_args=()
scope_seen=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            print_help
            ;;
        --full|-f)
            mode="full"
            shift
            ;;
        --lib|-l)
            mode="lib"
            shift
            ;;
        @*)
            # Named scope — expand to one or more -p args.
            expanded=$(scope_args "$1") || exit 2
            scope_seen=1
            # Scopes like @e2e imply --full (integration suites only exist
            # in --tests builds).
            if [[ "$1" == "@e2e" ]]; then
                forces_full=1
            fi
            # shellcheck disable=SC2206
            for arg in $expanded; do
                extra_args+=("$arg")
            done
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

if [[ -n "$forces_full" ]]; then
    mode="full"
fi

case "$mode" in
    lib)
        nextest_args=(--lib --no-fail-fast)
        ;;
    full)
        nextest_args=(--no-fail-fast)
        ;;
esac

echo "» cargo nextest run ${nextest_args[*]} ${extra_args[*]:-}" >&2
# Bless this run for the cargo-test-guard runner — nextest itself bypasses
# the runner, but if it launches anything through cargo, the env var
# threads through so the guard lets it pass.
export SHAMIR_TEST_BLESSED=1
exec cargo nextest run "${nextest_args[@]}" "${extra_args[@]}"
