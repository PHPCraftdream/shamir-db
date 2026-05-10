#!/usr/bin/env bash
# Single entry point for running every test in the workspace.
#
# Why a script instead of `cargo test --workspace`:
#   * Friendly summary (PASSED/FAILED totals) instead of scrolling
#     through 36+ "test result: ok" lines.
#   * Optional filtering by crate / test name (`scripts/test-all.sh shamir-server`).
#   * Honest about exit code: $? == 0 only if every test passes.
#   * Captures the full log to `target/test-all.log` so you can grep
#     a failure detail without re-running.
#
# Usage:
#   scripts/test-all.sh                 # everything
#   scripts/test-all.sh shamir-server   # one crate
#   scripts/test-all.sh -- --nocapture  # forward flags to cargo test
#   scripts/test-all.sh shamir-engine -- --test-threads=1
#
# Notes for Windows developers:
#   * Run from Git Bash / MSYS2. Cargo + rustc are picked up from PATH.
#   * Linking is the slowest part. With the `[profile.test]` block in
#     `.cargo/config.toml` (codegen-units=256, incremental=true,
#     debug=1) repeated runs are typically 5-15s per crate.

set -uo pipefail

cd "$(dirname "$0")/.."

LOG="target/test-all.log"
mkdir -p target
: > "$LOG"

# Split positional args: anything before `--` is a crate filter, anything
# after is forwarded to `cargo test`.
crates=()
forward=()
seen_dashdash=0
for arg in "$@"; do
    if [ "$seen_dashdash" -eq 1 ]; then
        forward+=("$arg")
    elif [ "$arg" = "--" ]; then
        seen_dashdash=1
    else
        crates+=("$arg")
    fi
done

# Build the cargo invocation.
if [ "${#crates[@]}" -eq 0 ]; then
    cargo_args=(test --workspace --tests)
    target_label="workspace"
else
    cargo_args=(test --tests)
    for c in "${crates[@]}"; do
        cargo_args+=(-p "$c")
    done
    target_label="${crates[*]}"
fi

if [ "${#forward[@]}" -gt 0 ]; then
    cargo_args+=(--)
    cargo_args+=("${forward[@]}")
fi

# Header.
printf '\033[1m== test-all: %s ==\033[0m\n' "$target_label"
printf 'cargo %s\n\n' "${cargo_args[*]}"

# Run, tee to log so the user sees live output AND we have the full
# transcript for the summary parser.
start_ns=$(date +%s)
cargo "${cargo_args[@]}" 2>&1 | tee "$LOG"
exit_code=${PIPESTATUS[0]}
elapsed=$(( $(date +%s) - start_ns ))

# Parse "test result: ok. N passed; M failed; ..." lines.
totals=$(awk '
    /^test result:/ {
        for (i = 1; i <= NF; i++) {
            if ($i == "passed;")  { p += $(i-1) }
            if ($i == "failed;")  { f += $(i-1) }
            if ($i == "ignored;") { ign += $(i-1) }
        }
    }
    END {
        printf "%d %d %d\n", p, f, ign
    }
' "$LOG")
read -r passed failed ignored <<<"$totals"

# Summary banner.
printf '\n\033[1m── summary ──\033[0m\n'
printf '   target:   %s\n' "$target_label"
printf '   elapsed:  %ds\n' "$elapsed"
printf '   passed:   %d\n' "$passed"
printf '   failed:   %d\n' "$failed"
printf '   ignored:  %d\n' "$ignored"
printf '   log:      %s\n' "$LOG"

# If cargo itself failed (build error, etc.) but no test counts were
# reported, surface the cargo failure clearly.
if [ "$exit_code" -ne 0 ] && [ "$passed" -eq 0 ] && [ "$failed" -eq 0 ]; then
    printf '\n\033[31mcargo failed before any tests could run (exit %d).\033[0m\n' "$exit_code"
    printf 'Last 20 lines of log:\n'
    tail -20 "$LOG"
    exit "$exit_code"
fi

if [ "$failed" -gt 0 ]; then
    printf '\n\033[31m%d test(s) failed. See %s for details.\033[0m\n' "$failed" "$LOG"
    exit 1
fi

if [ "$exit_code" -ne 0 ]; then
    # Cargo non-zero but tests didn't fail (e.g. doctest harness issue).
    printf '\n\033[33mcargo exit code %d but no test failures parsed — surfacing as an error.\033[0m\n' "$exit_code"
    exit "$exit_code"
fi

printf '\n\033[32mall green\033[0m\n'
