# Bug — UPSERT MERGE silently overwrites `created_at` on every merge

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## The bug (self-documented in the existing code as accepted-but-wrong debt)

`crates/shamir-engine/src/table/write_exec.rs`'s `execute_set_tx` (the
UPSERT executor, ~lines 845-934) has this existing comment at lines
859-870:

```rust
// ③.2d UPSERT-path: apply transforms with is_insert=true.
// `op.value` is a FULL record (insert-semantics): AutoNow + AutoNowAdd
// both apply.  For the MERGE branch this means AutoNowAdd may write
// `created_at` into the set-map, which then merges over the old record.
// TODO(③.2d): UPSERT MERGE created_at: if the existing record already
// has created_at and the caller did not supply one, the absence-guard in
// AutoNowAdd will stamp it into the set-map, overwriting the old value.
// A correct fix requires knowing at transform-time whether we are in the
// INSERT or MERGE branch — which we can't know until after the key lookup.
// For now we use is_insert=true (full-record semantics per brief) and
// accept that UPSERT MERGE may inadvertently overwrite created_at when
// the caller omits it. Track and fix in a follow-up if needed.
```

Concretely: `apply_transforms(resolved_value, ..., is_insert=true)` runs
BEFORE `lookup_existing_for_set` (called later, at line ~951) decides
whether this upsert is an INSERT (new row) or a MERGE (existing row). An
`AutoNowAdd` rule's absence-guard checks whether the CALLER's INCOMING
value already has `created_at` — if the caller omits it (the common
case, since `created_at` is usually meant to be stamped once at true
creation), the transform stamps a FRESH `created_at` into `set_map`
regardless of branch. On the INSERT branch this is correct (a genuinely
new row needs its creation timestamp). On the MERGE branch this is WRONG:
`set_map` is later patched onto the EXISTING record's bytes via
`merge_storage_bytes` (line ~975), so the fresh timestamp silently
OVERWRITES the original, real `created_at` of the row being merged —
every upsert that merges an existing row, on any table with an
`AutoNowAdd` rule, corrupts that row's creation timestamp. No error, no
warning — this is the single highest-value fix in this session's
correctness-bug sweep per the source audit's own ranking (small blast
radius, known fix shape, silent data corruption otherwise).

## The fix — restructure so the INSERT/MERGE decision happens BEFORE transforms

Read `execute_set_tx` in FULL (roughly lines 845-1050 — through the
MERGE branch's validator/staging logic and the INSERT branch below it)
before making any change, to understand the exact data flow.

The key insight that makes this fixable without a large redesign:
`lookup_existing_for_set` (`write_helpers.rs:431-...`) takes `key_fields:
&[(Vec<u64>, InnerValue)]` — and `key_fields` is derived ONLY from
`op.key` (via the `layered`/`intern_via_layered` interning block, lines
~895-916), NOT from `resolved_value`/`transforms` at all. This means the
existing/new-row lookup can be performed BEFORE `apply_transforms` runs,
with no missing information.

Restructure `execute_set_tx`'s control flow to:

1. Resolve inline `$fn` computed fields in the value FIRST, exactly as
   today (`resolve_computed_record`, line ~856) — this step is unrelated
   to the bug and should NOT move.
2. Compute `key_fields` from `op.key` (the interning block currently at
   lines ~895-916) EARLIER than it runs today — specifically, before the
   `apply_transforms` call. This may require splitting the current single
   `{ ... }` block (lines 895-934) that computes `key_fields`, `set_map`,
   AND `new_bytes_fresh` together — pull `key_fields`'s computation out on
   its own, keeping `set_map`/`new_bytes_fresh` computed AFTER transforms
   (step 4 below), since those DO depend on the (correctly-transformed)
   `resolved_value`.
3. Call `self.lookup_existing_for_set(&key_fields, batch_size).await?`
   (currently at line ~951-953) at this earlier point, to determine
   `found: Option<(RecordId, InnerValue)>` — i.e. whether this is an
   INSERT (`found.is_none()`) or a MERGE (`found.is_some()`).
4. NOW call `apply_transforms(resolved_value.to_mut(), &transforms,
   builtin_scalars(), now_ns, is_insert)` with `is_insert =
   found.is_none()` — the ACTUAL branch, not the hardcoded `true`. This
   is the one-line semantic fix that closes the bug, made possible by
   steps 2-3 running the lookup first.
5. Compute `set_map`/`new_bytes_fresh` from the (now correctly
   transformed) `resolved_value`, same as today, just after the lookup
   instead of before.
6. The rest of the function (the MERGE branch reading `old_bytes` via
   `read_one_tx_bytes`, `merge_storage_bytes`, change detection, validator
   run, `update_tx_bytes` staging; the INSERT branch's
   `insert_tx_many_bytes`) is UNCHANGED — you already have `found` from
   step 3, so the existing `if let Some((id, _existing)) = found { ...
   MERGE ... } else { ... INSERT ... }` structure stays exactly as-is,
   just no longer needs its own separate `lookup_existing_for_set` call
   (reuse the `found` computed in step 3).

**Do not change**: the tree-free/byte-level merge machinery (`W3d`),
`merge_storage_bytes`, the change-detection byte compare, the validator
gating (`has_upsert_validators`) or `run_validators_qv` call shape, or the
INSERT branch's `query_value_to_storage_bytes`/`insert_tx_many_bytes`
machinery — this fix is entirely about WHEN the lookup happens relative
to WHEN transforms run, not about any of that downstream logic.

Delete the stale `TODO(③.2d)` comment once fixed (replace with a short
note explaining the actual resolution, matching this codebase's "no
comments explaining WHAT, only non-obvious WHY" convention — if you keep
any comment here, it should explain why the lookup now runs before
transforms, not restate the bug).

## Tests (find the existing UPSERT/SET test file — likely
## `crates/shamir-engine/src/table/tests/` under a name involving
## `set`/`upsert`; follow its conventions for schema/transform setup and
## SET-op construction)

1. **The exact bug, closed**: a table with an `AutoNowAdd` rule on
   `created_at`. Insert a row (real `created_at` gets stamped, note its
   value). Then UPSERT (SET) that SAME key with a value that OMITS
   `created_at` — this must MERGE (not insert), and the row's
   `created_at` must be UNCHANGED after the merge (assert it equals the
   originally-stamped value, not a new "now" timestamp).
2. **Regression — INSERT branch unaffected**: a genuinely NEW key upserted
   for the first time (no existing row) — `created_at` must still be
   correctly stamped via `AutoNowAdd`, exactly as before this fix (the
   INSERT branch's behavior must not change).
3. **Regression — explicit `created_at` on MERGE**: an UPSERT that
   explicitly SUPPLIES a `created_at` value in the merge — that supplied
   value should still be used (transforms/`AutoNowAdd`'s absence-guard
   only fires when the field is OMITTED — this should be unaffected by
   the fix, since the guard's own semantics don't change, only WHEN
   `is_insert` is correctly `false` for a merge).
4. Any other existing `execute_set_tx`/UPSERT tests must continue to pass
   unchanged — this is a control-flow reordering, not a behavior change
   for any other transform (`AutoNow` without the add-only guard, plain
   `default`, etc. — check whether `AutoNow` itself has the same
   is_insert-sensitivity and, if so, confirm its MERGE-branch behavior is
   either unaffected or also correctly improved by this fix; the brief's
   primary target is `AutoNowAdd`/`created_at` specifically, but don't
   break `AutoNow`'s own existing correct behavior either way).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-engine`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the exact bug scenario (merge omitting
  `created_at`) now preserves the original value, (b) the INSERT branch's
  behavior is provably unchanged (the regression test), (c) walk through
  why moving `lookup_existing_for_set` earlier does not change ANY other
  observable behavior of `execute_set_tx` (e.g. confirm it doesn't
  duplicate a read, doesn't change transaction/locking semantics, doesn't
  change error ordering in a way any existing test would notice).

## Out of scope

- Do NOT touch nested-path defaults/transforms (a separate, already-known
  gap tracked elsewhere) or fail-open computed-default error handling (a
  separate, already-known gap).
- Do NOT touch the INSERT-only `execute_insert_tx` path — this bug is
  specific to `execute_set_tx`'s UPSERT MERGE branch.
- Do NOT touch anything unrelated to `execute_set_tx`'s transform-ordering
  and its direct test coverage.
