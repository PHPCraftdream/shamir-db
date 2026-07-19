# Honesty fixes — warn-log the fail-open computed default and the Null-collapsing Call params

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

This brief covers TWO small, independent fixes from a read-only release
audit. Both are "make an existing silent behavior discoverable via logs"
fixes — **do NOT change the underlying fail-open/Null-collapse behavior
itself in either case** (that would be a bigger, separately-considered
change); only add a `warn!` log at the point the silence currently
happens.

---

## Fix 1 — fail-open `ComputedDefault` evaluation error is completely silent

### The bug

`crates/shamir-engine/src/table/write_helpers.rs`'s `apply_transforms`,
in the `TransformSpec::ComputedDefault(expr)` arm (~lines 240-260 — read
the whole arm, including its own doc comment above `apply_transforms` at
~lines 202-206 which already documents this as an intentional fail-open
choice, citing "the scalar-bridge fail-open precedent from Phase B"):

```rust
if let Ok(v) = eval_write_value(expr, &literal, scalars) {
    m.insert(field.clone(), v);
}
```

If `eval_write_value` returns `Err`, the `if let Ok(...)` simply does
nothing — the record is inserted/updated WITHOUT the computed-default
field ever being stamped, and there is zero diagnostic anywhere: no log,
no error surfaced to the caller, nothing. A typo'd or failing default
expression (e.g. referencing a scalar that doesn't exist, or a `$ref` to
a field that isn't present) silently produces incomplete records forever,
and an operator has no way to discover this short of noticing missing
data downstream.

### The fix

Add a `log::warn!` call in the `Err` branch, naming the field path and
the error, e.g.:

```rust
match eval_write_value(expr, &literal, scalars) {
    Ok(v) => {
        m.insert(field.clone(), v);
    }
    Err(e) => {
        log::warn!(
            "computed default for field '{}' failed to evaluate, \
             skipping stamp (fail-open): {e}",
            field
        );
    }
}
```

(Adapt the exact wording/format to match this codebase's existing
`log::warn!`/`log::error!` call conventions — grep a few existing
`log::warn!` sites in `crates/shamir-engine/src` for the established
style, e.g. whether they include a `target:`, structured fields, etc.,
and mirror it.) **Do NOT change the fail-open behavior itself** — the
record must still be written without the field, exactly as before; this
fix only makes the silence audible via logs.

Also check `crates/shamir-engine/src/validator/schema/schema_validator.rs`
(~lines 90-92) — the report notes user-registered scalars are NOT
available in computed defaults (builtins only), so a default referencing
a user scalar never works. Confirm whether this ALSO routes through the
same `eval_write_value`/`Err` path above (in which case the SAME warn log
you just added already covers it — a user-scalar-reference failure is
just one flavor of "evaluation failed") or whether it's a genuinely
separate code path needing its own warn log. Read both sites before
deciding; do not add a redundant second log call if one already covers
both cases.

### Tests

1. A computed-default expression that evaluates successfully must
   continue to stamp correctly (regression — no behavior change on the
   happy path).
2. A computed-default expression that FAILS to evaluate (e.g. references
   an unknown scalar, or a `$ref` to an absent field) must still silently
   skip the stamp (the record is written without the field, no error
   surfaced to the caller — fail-open preserved) BUT must now produce a
   `warn!`-level log line. If this codebase has an existing log-capture
   test harness/convention (grep for one — e.g. a test-scoped log
   subscriber, `env_logger` test init, or similar), use it to assert the
   log line was emitted. If NO such harness/convention exists anywhere in
   the codebase, it is acceptable to verify by code inspection alone
   (confirm the `log::warn!` call is reachable and correctly formatted)
   rather than inventing a new logging-test mechanism from scratch —
   explain this choice in your summary either way.

---

## Fix 2 — `$ref`/`$fn`/`$expr`/`$cond` in `Call` params silently collapse to `Null`

### The bug

`crates/shamir-db/src/shamir_db/execute/helpers.rs`'s
`filter_value_to_query_value` (~lines 206-284) — read the function's own
doc comment (~lines 206-220) first, it already explains WHY this
collapse happens (no `RecordRef`/`Interner`/`FilterContext` in scope for
this record-free resolver — a genuine, documented, out-of-scope
limitation, not a bug in the collapse decision itself). The catch-all arm
at the bottom (~line 282): `_ => QueryValue::Null` silently converts ANY
`$ref`/`$fn`/`$expr`/`$cond` marker in a `Call` op's positional params to
`Null`. A function invoked from a batch with such a dynamic param
silently receives `Null` instead of the intended computed value — no
error, no warning, invisible to the caller.

### The fix

Add a `log::warn!` call in that catch-all arm, naming which marker
variant collapsed (e.g. "FnCall", "Expr", "Cond", "FieldRef" — whichever
`FilterValue` variant matched the wildcard) so an operator can discover
this in logs rather than silently getting wrong function behavior:

```rust
_ => {
    log::warn!(
        "Call param resolver: {:?} marker is not supported here \
         (no record context for $ref/$fn/$expr/$cond) — collapsing to Null",
        fv
    );
    QueryValue::Null
}
```

(Adapt formatting to match this codebase's existing `log::warn!`
conventions, same as Fix 1 — check whether `FilterValue`'s `Debug` output
is reasonable to log directly, or whether you should match on the
specific variant name instead for a cleaner message; use your judgement.)
**Do NOT change the Null-collapse behavior itself** — per the function's
own doc comment, wiring real resolution here is explicitly out of scope
(would require plumbing a record + interner into every `Call`-param call
site); this fix only makes the collapse discoverable via logs.

### Tests

1. A `Call` op with a plain literal / `$query` param must continue to
   resolve correctly (regression — these are NOT part of the collapse,
   they already work).
2. A `Call` op with a `$ref`/`$fn`/`$expr`/`$cond` param must still
   collapse to `Null` (fail-open/collapse-preserved) BUT must now produce
   a `warn!`-level log line naming which marker type triggered it. Same
   guidance as Fix 1 regarding log-capture test conventions — verify by
   whatever mechanism this codebase already supports, or by code
   inspection if none exists.

---

## Verification (MANDATORY before you report done, for BOTH fixes)

- `./scripts/test.sh @engine --full` green (covers Fix 1) and
  `./scripts/test.sh -p shamir-db --full` green (covers Fix 2) — including
  all new/modified tests.
- `cargo fmt --all -- --check` clean (or scoped to touched crates, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) NEITHER fix changed the underlying fail-open/
  Null-collapse BEHAVIOR — only added logging, (b) both new log calls
  follow this codebase's existing `log::warn!` conventions (cite the
  existing call sites you matched style against).

## Out of scope

- Do NOT change fail-open to fail-closed for `ComputedDefault` — that is
  a bigger behavior change requiring separate discussion, not part of
  this task.
- Do NOT wire real `$ref`/`$fn`/`$expr`/`$cond` resolution into
  `Call`-param positional args — explicitly out of scope per the
  function's own doc comment.
- Do NOT invent a new log-capture test harness/framework if one doesn't
  already exist in this codebase — verify by inspection instead and say
  so.
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, or the already-completed DDL-time-rejection
  / Call-in-tx fixes (task 3a, commit `bdd7bbb3`).
