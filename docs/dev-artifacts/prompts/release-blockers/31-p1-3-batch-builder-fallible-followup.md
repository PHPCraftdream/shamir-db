# Follow-up brief — P1-3 (#1016): finish the mechanical call-site migration

## Context

Round 1+2 on session `t1016-batch-fallible` correctly removed the 5
panicking `IntoBatchOp`/`From<...> for BatchOp` impls (confirmed —
`Update`/`Upsert`/`Delete` in `into_batch_op.rs`,
`AddSchemaRuleBuilder` in `schema.rs`, `AlterSubscriptionBuilder` in
`replication.rs`) and migrated ~25 call sites. Good work, keep it.

**84 compile errors remain**, confirmed by running
`cargo check --workspace --all-targets` myself just now:

```
40  E0277  shamir_query_builder::write::Update: IntoBatchOp not satisfied
34  E0277  shamir_query_builder::write::Delete: IntoBatchOp not satisfied
 5  E0277  shamir_query_builder::write::Upsert: IntoBatchOp not satisfied
 4  E0277  AlterSubscriptionBuilder: IntoBatchOp not satisfied
 1  E0277  AddSchemaRuleBuilder: IntoBatchOp not satisfied
```

All in: `shamir-engine` (73 errors, mostly
`crates/shamir-engine/src/query/batch/tests/`), `shamir-db` (10 errors,
mostly `crates/shamir-db/tests/` and one in
`tests/purge_history`... actually a lib test), `shamir-server` (2
errors). Your own last report said automated sed/Python-script attempts
at these were **introducing new syntax errors** — stop that approach
entirely for this pass.

## The fix — same mechanical pattern, every time, but do it by hand/edit-tool, not scripted regex

Every single error is the exact same shape: a call site like
`batch.update(alias, some_update_builder)` (or `.upsert(...)`,
`.delete(...)`, `.update_after(...)`, `.upsert_after(...)`,
`.delete_after(...)`, or `.op(alias, some_schema_rule_or_subscription_builder)`)
no longer compiles because the plain method's `impl IntoBatchOp` bound
isn't satisfied for these 5 builder types anymore. The fix at EACH site:

- `.update(alias, op)` → `.try_update(alias, op).unwrap()`
- `.update_after(alias, op, after)` → `.try_update_after(alias, op, after).unwrap()`
- `.upsert(...)` → `.try_upsert(...).unwrap()` (and `_after` variant)
- `.delete(...)` → `.try_delete(...).unwrap()` (and `_after` variant)
- `.op(alias, schema_rule_or_subscription_builder)` →
  `.try_op(alias, schema_rule_or_subscription_builder).unwrap()`

**In test code, `.unwrap()` is correct and matches the existing style you
already used for the first ~25 sites — do NOT propagate `Result` through
test function signatures unless a given test function already returns
`Result` (rare; check each site).** For the 2 `shamir-server` errors and
any other genuinely-production (non-test) call site, check the
surrounding function's return type — if it already returns a
`Result`-compatible type, use `?` instead of `.unwrap()`; otherwise
`.unwrap()`/`.expect("...")` is fine if the call site is guaranteed
well-formed by construction (e.g., a hardcoded literal, not
user-supplied data) — use judgement, but default to `.unwrap()` matching
your own prior pattern.

**Work file-by-file, using `cargo check --workspace --all-targets`
output as ground truth for exact file+line, and make each edit with the
Edit/MultiEdit tool reading real surrounding context — not a blind
sed/regex pass.** Balanced-parens call sites vary in formatting
(multi-line, chained, nested) enough that scripted regex is exactly what
went wrong last round. Fix one file at a time, re-run `cargo check` after
each file (or after a batch of a few files) to confirm you haven't
introduced a new error, rather than doing all 84 blind then discovering
breakage at the end.

**Do not stop and ask again — finish this autonomously.** The pattern is
fully specified above; there is no judgement call left to make except
`.unwrap()` vs `?` at the 1-2 production call sites, which this brief
already resolves.

## After all compile errors are fixed

1. **TS SDK parity check** — if you haven't done this yet (your last
   report didn't mention it), do it now: investigate
   `crates/shamir-client-ts/src/core/builders/` for an equivalent
   panic-by-default (in TS terms: throw-a-non-catchable-or-generic-error)
   pattern in its own update/upsert/delete/schema-rule/subscription
   builders, per the original brief's exact instructions (§3 "TS SDK
   parity check" in `docs/dev-artifacts/prompts/release-blockers/30-p1-3-batch-builder-fallible.md`).
   Report your finding either way (already fine / fixed / not
   applicable).
2. **Gates, run for real:**
   `cargo fmt -p shamir-query-builder -- --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `./scripts/test.sh -p shamir-query-builder -p shamir-engine -p
   shamir-db -p shamir-server --full`. Confirm zero compile errors first
   via `cargo check --workspace --all-targets` before running the gates.
   Use the wrapper for tests, never raw `cargo test`/`cargo nextest run`.

## Constraints

Same as the original brief: `CLAUDE.md` conventions, no stray files at
repo root, no destructive git commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Definition of done

- [ ] `cargo check --workspace --all-targets` — zero errors.
- [ ] TS SDK parity check done and reported.
- [ ] fmt/clippy/test gates actually run, real pass/fail summary
      reported.
