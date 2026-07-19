# Honesty fixes — DDL-time rejection for nested-path transforms; reject `Call` inside a transaction

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

This brief covers TWO independent, small fixes from a read-only release
audit — both are "convert silent-wrong-behavior into an honest, explicit
error" fixes, not new features.

---

## Fix 1 — nested-path schema defaults/transforms are silently dropped at write time

### The bug

`crates/shamir-engine/src/table/write_helpers.rs`:
- `apply_defaults` (~lines 143-179): "MVP scope: single-segment paths
  only. Multi-segment paths (`["address","zip"]`) are silently skipped"
  — the loop at ~line 166-172 does `match path.first() { Some(f) if
  path.len() == 1 => f, _ => continue }`, i.e. a multi-segment default
  rule is silently ignored, every time, forever, with no error anywhere.
- `apply_transforms` (~lines 185-227): identical MVP gate, identical
  silent-skip (~lines 222-227), for `AutoNow`/`AutoNowAdd`/
  `ComputedDefault` transform rules.

**The asymmetry that makes this a real bug, not just a documented
limitation**: `crates/shamir-db/src/shamir_db/execute/admin_schema.rs`'s
`validate_unique_indexes` (~lines 99-162) ALREADY rejects a multi-segment
`unique` constraint at DDL time with a clear, coded error
(`unique_requires_index`, ~lines 128-137: `"unique on {:?}: only
single-segment field paths are supported for unique constraints"`) —
called from `set_table_schema`/`add_schema_rule` (search for
`validate_unique_indexes(` call sites, ~lines 278 and 393). So a user
declaring a nested-path `unique` gets an immediate, clear DDL-time error;
a user declaring a nested-path `default`/`auto_now`/`auto_now_add` (via
the exact same `FieldRuleDto`/`ConstraintsDto` schema-rule mechanism, see
`crates/shamir-query-types/src/admin/types/schema_ops.rs`) gets NO error
at all — the DDL SUCCEEDS, and the rule is then silently dropped on every
subsequent insert/update, forever.

### The fix

Add a sibling validation function (mirror `validate_unique_indexes`'s
shape and calling convention EXACTLY — same file, same DDL-time call
sites, same error-mapping style via `err_code(...)`) that walks the
incoming rule set and rejects any rule declaring `default`/`auto_now`/
`auto_now_add` (find the exact `ConstraintsDto` field names — read
`crates/shamir-query-types/src/admin/types/schema_ops.rs` in full first,
since `computed_default` may not be a separate boolean field — it may be
detected by the `default` field's value being a `FilterValue::FnCall`/
`Expr`/`Cond` marker vs. a plain literal; read `write_helpers.rs`'s
`TransformSpec` construction (search for where `ConstraintsDto` is
converted into `(Vec<String>, TransformSpec)` — likely in
`admin_schema.rs` or a schema-conversion helper) to understand exactly
which constraint fields map to which `TransformSpec` variant before
writing the validation) whose `rule.path.len() != 1`. Use an error code
of your choosing that's consistent with the codebase's `snake_case` coded
-error convention (e.g. `nested_path_transform_not_supported`), with a
message mirroring `unique_requires_index`'s wording style ("only
single-segment field paths are supported for default/auto_now/
computed-default rules").

Call this new validation from the SAME DDL entry points that already call
`validate_unique_indexes` (`set_table_schema`, `add_schema_rule` — the
two call sites at ~lines 278 and 393 in `admin_schema.rs`), so both
paths get the new check.

**Do NOT implement the recursive nested-path walker itself** — that is a
larger feature (actually supporting nested defaults/transforms) explicitly
out of scope. This fix is ONLY: reject at DDL time instead of silently
dropping at write time. The runtime `apply_defaults`/`apply_transforms`
MVP-gate behavior (silently skip a multi-segment path) stays exactly as
it is — it becomes unreachable in practice once DDL rejects such rules,
but do not remove the runtime guard itself (defense in depth; a stale
rule from before this fix landed, or a rule injected some other way,
should still be safely ignored at write time, not panic).

### Tests

1. **DDL-time rejection, closed**: attempt to `set_table_schema`/
   `add_schema_rule` with a multi-segment `default` rule (e.g.
   `["address", "zip"]`) — must be REJECTED with the new coded error, not
   silently accepted.
2. Same for a multi-segment `auto_now`/`auto_now_add` rule.
3. **Regression — single-segment still works**: an existing single-
   segment `default`/`auto_now`/`auto_now_add` rule must continue to be
   accepted and to function exactly as before.
4. **Regression — unique's own existing rejection test** must continue to
   pass unchanged (you are adding a sibling function, not modifying
   `validate_unique_indexes`).

---

## Fix 2 — `Call` escapes the transaction it's declared inside

### The bug

`crates/shamir-engine/src/query/batch/query_runner.rs` (~lines 700-710) —
`Call` ops in a batch delegate to `FunctionInvoker` "(autocommit, no
tx)" — this happens EVEN WHEN the enclosing batch is `transactional:
true` or the op runs inside an interactive tx. The function's own DB
writes commit independently of the outer transaction and SURVIVE an
outer abort — the batch looks atomic (the caller declared
`transactional: true`) but is not: a `Call` inside it breaks the
atomicity guarantee silently, with no error, no warning.

**The exact precedent to mirror already exists in the same file**: search
`query_runner.rs` for `nested_tx_not_supported` (~line 331) — a
transactional sub-batch inside an open tx is ALREADY rejected with a
clear coded error, for the analogous reason (a case the engine can't yet
support correctly, converted from "silently do the wrong thing" to
"honestly refuse"). This fix does the SAME thing for `Call`.

### The fix

At the `Call`-op dispatch site in `query_runner.rs` (~lines 700-710),
before delegating to `FunctionInvoker`, check whether the CURRENT
execution context is `transactional: true` (non-tx batch) OR is running
inside an open interactive tx (mirror however `nested_tx_not_supported`'s
check determines "are we inside a tx" — read that code path in full
first). If so, return a new coded error `call_in_tx_not_supported` (or
whatever name matches the `nested_tx_not_supported`-style naming
convention exactly) instead of dispatching the `Call`. A `Call` inside a
NON-transactional batch (the common case today, and the only case that
was ever actually correct) must continue to work exactly as before —
this fix ONLY closes the silent-atomicity-violation case, it does not
touch the working case.

### Tests

1. **The exact bug, closed**: a batch with `transactional: true`
   containing a `Call` op — must be REJECTED with
   `call_in_tx_not_supported`, not silently executed non-atomically.
2. Same for a `Call` op submitted via `tx_execute_as`/inside an open
   interactive tx.
3. **Regression — the working case unaffected**: a `Call` op inside a
   PLAIN (non-transactional) batch must continue to execute exactly as
   before (same result, same non-tx semantics — this was never wrong,
   only the transactional case was).
4. Confirm the new check does not affect `nested_tx_not_supported`'s own
   existing behavior/tests (they are sibling checks, not the same one).

---

## Verification (MANDATORY before you report done, for BOTH fixes)

- `./scripts/test.sh @engine --full` green, including all new tests (this
  covers the `query_runner.rs` fix directly; confirm whether the DDL
  fix's tests live under `shamir-db` or `shamir-engine` and add the
  matching scope, e.g. `./scripts/test.sh @engine -p shamir-db --full` or
  similar — check the exact crate the `admin_schema.rs` tests live under
  and use the right scope).
- `cargo fmt --all -- --check` clean (or scoped to touched crates, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the DDL-time rejection mirrors
  `validate_unique_indexes`'s exact calling convention and error-code
  style, (b) the `Call`-in-tx rejection mirrors
  `nested_tx_not_supported`'s exact precedent shape, (c) neither fix
  changes any OTHER observable behavior — single-segment schema rules and
  non-transactional `Call` ops work identically to before.

## Out of scope

- Do NOT implement actual nested-path default/transform SUPPORT (the
  recursive walker) — only DDL-time rejection.
- Do NOT touch the fail-open computed-default error handling or the
  Null-collapsing `Call`-param issue — those are separate follow-up tasks
  (warn-log additions), not in scope here.
- Do NOT touch anything from the already-completed correctness-bug wave
  (FK on-update/cascade, `$contains_all`, Dec comparison layer, UPSERT
  `created_at`) or the already-completed concurrency-deadlock sweep
  (H1-H6, `rid_map`).
