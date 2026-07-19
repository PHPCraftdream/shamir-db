# Cleanup tail A follow-up — missing ScalarResolver tests for `when`/`bind`/`over`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

The previous stage of this task (brief:
`docs/dev-artifacts/prompts/cleanup-tail-a/01-coercing-sets-scalar-resolver-structural-eq.md`)
threaded a real `ScalarResolver` into 5 contexts that previously defaulted to
builtins-only. Verification confirmed 2 of the 5 got regression tests proving
a user-registered scalar now resolves (group-SELECT in
`crates/shamir-engine/src/query/read/tests/exec_tests.rs`'s
`aggregate_all_user_scalar_resolves`, and SELECT projection in
`crates/shamir-engine/src/query/read/tests/select_projection_tests.rs`'s
`project_value_user_scalar_resolves_through_projection`). The OTHER 3 sites —
all in `crates/shamir-engine/src/query/batch/query_runner.rs` — got the same
code fix (`.with_scalars(...)` added to their `FilterContext` builder chains)
but NO test coverage:

1. **`when` guard / cascade skip** — `resolve_skip` (~line 128-163 in
   `query_runner.rs`), now takes a `scalars: ScalarResolver` param and
   threads it via `.with_scalars(scalars)` (~line 160).
2. **Sub-batch `bind`** — `QueryRunner`'s bind-resolution block (~line
   345-352), `.with_scalars(self.resolver.scalar_resolver())`.
3. **`for_each`'s `over`** — `QueryRunner`'s over-resolution block (~line
   500-506), `.with_scalars(self.resolver.scalar_resolver())`.

Read the actual current code at each site before writing tests — line
numbers above are approximate (from before your session).

## The task

Add ONE regression test per site (3 new tests total) proving a
user-registered scalar function is now resolvable through that context,
where it previously silently fell back to builtins-only (Null / cascaded
skip that doesn't reflect the real value).

**Test harness**: use the existing shared test fixtures in
`crates/shamir-engine/src/query/batch/tests/executor_tests/common.rs`
(`TestResolver`/`setup_resolver`, `TxTestResolver`) as your starting point.
Neither currently overrides `TableResolver::scalar_resolver()` — the
default impl in `crates/shamir-engine/src/query/batch/executor_traits.rs`
(~line 34-36) returns `ScalarResolver::builtins_only()`. You will need a
resolver variant that returns a `ScalarResolver` backed by a
`UserScalarLayer` with one registered scalar — mirror the
`resolver_with_user_scalar()` helper already added in this session's prior
stage (see `crates/shamir-engine/src/query/read/tests/exec_tests.rs` and
`select_projection_tests.rs` for the exact `UserScalarLayer::new()` +
`FnEntry::pure(...)` + `ScalarResolver::new(Arc::new(layer))` pattern — copy
its shape, do not reinvent it). Decide whether to add a `scalar_resolver()`
override to a NEW small test-only resolver struct (co-located in the test
file that needs it, or added to `common.rs` if genuinely shared across all
three) — do not modify `TestResolver`/`TxTestResolver` in a way that breaks
any of their many existing callers across the `tests/` tree; adding a new
struct alongside them is safer than changing their behavior.

**Existing test files to extend** (find the exact test structure by reading
each file first — mirror its existing style, don't restructure):

1. `crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs`
   — add a test where a `when` filter references a user-registered scalar
   (e.g. `$fn: my_double` applied to a literal or field, compared against an
   expected doubled value) and assert the op executes (or is skipped)
   according to the CORRECT (scalar-resolved) result — not the builtins-only
   fallback result. The clearest version: pick a scalar whose result
   flips the `when` condition's truth value depending on whether it resolves
   correctly vs falls back — e.g. a scalar that must return a specific
   non-Null value for an `$eq` comparison in the `when` filter to be `true`;
   under the old builtins-only fallback the function call would be unknown
   and the comparison would evaluate `false` (or however this codebase's
   filter eval treats an unresolvable `$fn` — confirm by reading
   `resolve_filter_query`'s unknown-function handling before writing the
   assertion, don't guess).
2. `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` — add a
   test where a sub-batch's `bind` value uses a user-registered scalar and
   the bound param carries the CORRECT resolved value into the sub-batch
   (assert on the sub-batch's result reflecting the scalar's real output,
   not Null).
3. `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` — add a
   test where `for_each`'s `over` expression uses a user-registered scalar
   and the iteration count / iterated values reflect the CORRECT resolved
   result, not a Null-collapsed one.

Each test must be a genuine regression test: reading the diff alone should
make clear that WITHOUT the `.with_scalars(...)` fix in `query_runner.rs`,
the test would fail (the user scalar would resolve to Null / unknown-function
behavior instead of its real computed value). If you find that any of these
three contexts genuinely cannot observe the difference (e.g. the site is
unreachable in a way that makes a real regression test impossible), explain
exactly why in your summary — do not fabricate a test that can't actually
fail pre-fix.

## Out of scope

- Do NOT modify `query_runner.rs`, `batch_execute.rs`, `aggregate.rs`,
  `select_projection.rs`, `exec.rs`, `read_exec.rs`, `read_index_scan.rs`,
  `read_temporal.rs`, `filter_node.rs`, or `compare.rs` — the implementation
  for all three fixes from the prior stage is already complete and verified.
  This stage is TEST-ONLY.
- Do NOT touch `crates/shamir-engine/src/query/read/tests/exec_tests.rs` or
  `select_projection_tests.rs` — their user-scalar tests (sites 4/5) are
  already done.
- Do NOT re-implement or duplicate the `resolver_with_user_scalar()` helper
  pattern differently across the three new test files — factor it into
  `common.rs` if all three tests can share one resolver-with-scalar struct,
  otherwise keep each local but consistent in shape.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @engine --full` green, including the 3 new tests.
- `cargo fmt --all -- --check` clean (scoped to touched files is fine).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- For each of the 3 new tests, state explicitly what result the test would
  observe under the PRE-fix builtins-only fallback vs the POST-fix
  correctly-resolved behavior — proving the test is non-vacuous.

Do NOT commit or run any git-mutating command — the orchestrator (a separate
process) reviews your diff and commits it. Only edit files and run read-only
git / test / build commands.
