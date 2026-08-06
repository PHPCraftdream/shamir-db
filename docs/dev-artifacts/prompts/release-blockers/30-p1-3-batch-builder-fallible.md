# Brief — P1-3 (#1016): remove panic-by-default from Batch builders

## Context

S.H.A.M.I.R. Database, `crates/shamir-query-builder`. Source: review
2026-08-05 §P1-3. Already investigated and root-caused this task myself
before writing this brief — the scope is smaller and more precise than
the task description alone suggests, follow this brief's exact file list
rather than re-deriving it.

## The exact problem (already diagnosed)

There are **exactly 5** places where a batch-builder type's `IntoBatchOp`
(or `From<...> for BatchOp`) implementation calls `.build().expect(...)`
instead of propagating the `Result` — meaning `Batch::update`/`upsert`/
`delete`/`op` (which accept `impl IntoBatchOp`) can panic **at the moment
an operation is added to a batch**, before `Batch::build()`/
`try_build()` is ever called, if the builder is missing a required field:

1. `crates/shamir-query-builder/src/batch/into_batch_op.rs:49-56` —
   `impl IntoBatchOp for crate::write::Update`
2. `crates/shamir-query-builder/src/batch/into_batch_op.rs:64-71` —
   `impl IntoBatchOp for crate::write::Upsert`
3. `crates/shamir-query-builder/src/batch/into_batch_op.rs:79-86` —
   `impl IntoBatchOp for crate::write::Delete`
4. `crates/shamir-query-builder/src/ddl/schema.rs:501-516` — BOTH
   `impl From<AddSchemaRuleBuilder> for BatchOp` AND
   `impl IntoBatchOp for AddSchemaRuleBuilder`
5. `crates/shamir-query-builder/src/ddl/replication.rs:283-298` — BOTH
   `impl From<AlterSubscriptionBuilder> for BatchOp` AND
   `impl IntoBatchOp for AlterSubscriptionBuilder`

**Every other `IntoBatchOp` impl in the crate wraps an already-fully-
specified DTO whose conversion is genuinely infallible** (`BatchOp`,
`ReadQuery`, `InsertOp`, `UpdateOp`, `SetOp`, `DeleteOp`, `CallOp`,
`SubBatchOp`, etc.) — do NOT touch those, they are not part of this bug.

**The fallible counterpart already exists and is fully implemented** —
`crates/shamir-query-builder/src/batch/try_into_batch_op.rs` defines
`TryIntoBatchOp`, implemented for exactly these same 5 types, already
used by `Batch::try_update`/`try_update_after`/`try_upsert`/
`try_upsert_after`/`try_delete`/`try_delete_after`/`try_op`/`try_op_after`
in `crates/shamir-query-builder/src/batch/batch.rs` (search for `// ──
fallible write-op insertion` and `try_op` in that file). This task is
NOT about designing a new fallible path — it already exists, well-tested
— it is about **retiring the panicking one** so the fallible path becomes
the only way to convert these 5 builder types.

## The fix

1. **Delete the 5 panicking impls** listed above (both `IntoBatchOp for
   Update/Upsert/Delete` in `into_batch_op.rs`, and BOTH the `From<...>
   for BatchOp` AND `IntoBatchOp for ...` impls in `schema.rs` and
   `replication.rs` — 5 files, but note `schema.rs`/`replication.rs` each
   have TWO impls to remove). This is the "удалить" (delete) option the
   task names — prefer it over a `*_unchecked` rename: a trait `impl`
   doesn't have a meaningful "rename", and deletion is the more honest
   fix (it makes the panic-prone path a **compile error** at every call
   site instead of a runtime trap, which is strictly better for an alpha
   where breaking changes are cheap, per the task's own framing). If you
   find a genuinely good reason a rename-based approach is better after
   investigating actual call sites, you may do that instead — but justify
   it in your final report rather than defaulting to it.
2. **Fix every call site that breaks.** Deleting these impls will produce
   compile errors everywhere `Batch::update`/`upsert`/`delete`/`op` (or
   the `.into()` conversion) was called with one of the 5 builder types.
   `grep` the whole repo (not just this crate — check
   `crates/shamir-engine`, `crates/shamir-server`, any doc examples in
   `docs/`, and this crate's own `tests/`) for such call sites and migrate
   each to the `try_*` equivalent (`try_update`/`try_upsert`/`try_delete`/
   `try_op`, `_after` variants where relevant), propagating the
   `Result<Handle, BuilderError>` appropriately for that call site's
   context (`?` in a function already returning a compatible `Result`,
   `.unwrap()`/`.expect(...)` only in test code where panicking on a
   genuinely-malformed literal is acceptable test-authoring style, never
   in library/production code).
3. **TS SDK parity check** (`crates/shamir-client-ts`) — the task
   explicitly asks for this. Investigate: does the TypeScript batch
   builder (`crates/shamir-client-ts/src/core/builders/`) have an
   equivalent "throws synchronously when adding an operation with a
   missing required field" pattern for its own update/upsert/delete/
   schema-rule/subscription builders? Note TS has no Rust-style
   Result-vs-panic distinction — a thrown exception IS the idiomatic TS
   error channel, so "parity" does NOT necessarily mean "add a Result
   type to TS". Investigate what TS actually does today and report:
   - If TS already throws a catchable, typed error (not an unreachable
     invariant panic) at the same point — that's already correct
     behavior for the language, no change needed, say so explicitly.
   - If TS has its own silent-failure or wrong-behavior gap analogous to
     Rust's problem (e.g., silently drops the op, or throws an
     un-typed/generic error), fix it to throw a typed, catchable error
     consistent with the rest of the TS SDK's error conventions (check
     `crates/shamir-client-ts/src/core/errors.ts` for the existing error
     type hierarchy — use that, don't invent a new one).
   - If TS's builders don't have this pattern at all (e.g., they're
     already infallible by construction, or required fields are
     constructor params rather than optional builder calls) — say so,
     no change needed.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` + `thiserror` conventions already
  established by `BuilderError` in this crate — reuse it, don't invent a
  new error type.
- This is a **removal + call-site migration** task, not a redesign —
  resist the urge to also touch the ~75 OTHER `Batch::*` methods that
  accept `impl IntoBatchOp` for genuinely-infallible types (create_db,
  create_index, etc.) — they are correctly infallible today and out of
  scope.
- Add/extend tests proving: (a) the 5 deleted impls are actually gone
  (a call site using the OLD panicking path should now fail to compile —
  you can't easily "test a compile error" in a unit test, but you CAN
  add a doc-comment or a `tests/ui`-style note; more usefully, add/extend
  tests proving the `try_*` paths correctly return `Err(BuilderError)`
  for each of the 5 types when a required field is missing, if such
  tests don't already exist — check `try_into_batch_op_tests`
  first, mentioned in an existing doc comment, before adding duplicates).
- Gate: `cargo fmt -p shamir-query-builder -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` (workspace-wide
  because call-site fixes may span other crates), `./scripts/test.sh
  -p shamir-query-builder -p shamir-engine -p shamir-server --full`
  (adjust the crate list if your call-site grep finds usages elsewhere —
  report which crates you actually needed to touch). Use the wrapper,
  never raw `cargo test`/`cargo nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] The 5 panicking `IntoBatchOp`/`From<...> for BatchOp` impls are
      gone (or, if you chose a different approach, clearly justified why).
- [ ] Every call site across the workspace that broke as a result is
      migrated to the `try_*` fallible path, with sensible error
      propagation for its context.
- [ ] TS SDK investigated and either confirmed already-correct or fixed,
      with your finding stated explicitly in the final report either way.
- [ ] Tests proving the fallible paths return typed errors for all 5
      builder types (reusing pre-existing tests where they already
      cover this).
- [ ] fmt/clippy/test gates actually run, real pass/fail output reported.
