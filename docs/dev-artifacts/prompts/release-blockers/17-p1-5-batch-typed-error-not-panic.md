# Brief — P1-5: Query Builder panics via the main Batch API instead of a typed error

Task: #965 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P1-5. This is a real production API change in `shamir-query-builder` — the public, published builder library — not a test-only task. Read this brief in full before touching any code.

## Root cause, precisely — read before starting

**The fallible builders and their typed error ALREADY EXIST.** This is not
"add validation from scratch" — it's "stop discarding validation that
already runs." Exactly 5 builder types in `crates/shamir-query-builder/src`
have a `build()` method that already returns `Result<_, BuilderError>`
(`crates/shamir-query-builder/src/write/builder_error.rs` — read this file's
own doc comment in full, it names all 5 and is authoritative):

1. `crate::write::Delete::build()` → `Err(BuilderError::MissingWhereClause)`
2. `crate::write::Update::build()` → `Err(BuilderError::MissingSetValue)`
3. `crate::write::Upsert::build()` → `Err(BuilderError::MissingKey)` /
   `Err(BuilderError::MissingValue)`
4. `crate::ddl::AddSchemaRuleBuilder::build()` (`ddl/schema.rs` ~line 475-500)
   → `Err(BuilderError::MissingRule)`
5. `crate::ddl::AlterSubscriptionBuilder::build()` (`ddl/replication.rs`
   ~line 270-280) → `Err(BuilderError::MissingAction)`

**The bug**: each of these 5 types ALSO has an `impl IntoBatchOp for T` (in
`crates/shamir-query-builder/src/batch/into_batch_op.rs` for the first 3,
`ddl/schema.rs` ~line 512-516 and `ddl/replication.rs` ~line 294-298 for the
last 2) AND a `impl From<T> for BatchOp`, and BOTH call `.build().expect(...)`
— discarding the `Result` and panicking. `Batch`'s ergonomic fluent methods
(`Batch::update`, `.upsert`, `.delete`, `.add_schema_rule`, and the generic
`.op()` escape hatch used for `AlterSubscriptionBuilder` today) all take
`impl IntoBatchOp` and call `.into_batch_op()` EAGERLY at the call site —
BEFORE `Batch::try_build()` is ever reached — so there is currently NO way
to get a typed error out of the fluent path; only manually calling `.build()`
yourself (bypassing the fluent `Batch::update(...)` call entirely, per the
`From` impl's own doc comment at `replication.rs` ~line 285-288) avoids the
panic today. That workaround is not discoverable/ergonomic — most callers
will naturally chain the builder straight into `Batch::update(...)`.

**Every OTHER `IntoBatchOp`/`From<...> for BatchOp` impl in the crate is
fine and OUT OF SCOPE** — confirmed by grepping every
`impl IntoBatchOp for` / `impl From<...> for BatchOp` site in
`crates/shamir-query-builder/src` (70+ hits): all the others either wrap an
already-fully-specified struct (all fields set at construction, `.build()`
is infallible / returns the DTO directly, no `Result`) or (for `Query`,
`crate::write::Insert`, `CreateIndex`) have a "permissive" `build()` that
never fails (documented separately in the review as its own, different,
lower-priority concern — do NOT touch `Query`/`Insert`/`CreateIndex` here).

## The fix — additive, not a redesign

Do **NOT** change the signature of any existing `Batch` method (`.update`,
`.upsert`, `.delete`, `.add_schema_rule`, `.op`, etc.) — that's a ~60-method
blast radius across the whole crate and every consumer (`shamir-db`,
`shamir-server`, every existing test) for zero net benefit, since those
methods stay useful for callers who already know their builder is
well-formed. Leave the existing panic-based `IntoBatchOp`/`From` impls for
the 5 types EXACTLY as they are (still documented, still available,
unchanged behavior) — this is additive, not a replacement.

**Add a new, parallel fallible path**:

1. A new trait, e.g. `TryIntoBatchOp` (new file
   `crates/shamir-query-builder/src/batch/try_into_batch_op.rs`, registered
   in `batch/mod.rs` alongside the existing `into_batch_op` module):
   ```rust
   pub trait TryIntoBatchOp {
       fn try_into_batch_op(self) -> Result<BatchOp, BuilderError>;
   }
   ```
   Implement it for exactly the 5 types above — each implementation is a
   ONE-LINE delegation to the type's own already-correct `.build()`
   (e.g. for `Update`: `Ok(BatchOp::Update(self.build()?))`; for
   `AlterSubscriptionBuilder`/`AddSchemaRuleBuilder`, `.build()` already
   returns `BatchOp` directly per their existing signatures — check each
   type's exact `build()` return type before writing the impl, do not
   assume they're identical in shape).

2. New `Batch` methods (in `batch/batch.rs`, near their existing
   non-fallible counterparts) mirroring the existing naming:
   - `pub fn try_update(&mut self, alias: impl Into<String>, op: crate::write::Update) -> Result<Handle, BuilderError>`
   - `pub fn try_upsert(&mut self, alias: impl Into<String>, op: crate::write::Upsert) -> Result<Handle, BuilderError>`
   - `pub fn try_delete(&mut self, alias: impl Into<String>, op: crate::write::Delete) -> Result<Handle, BuilderError>`
   - `pub fn try_add_schema_rule(&mut self, alias: impl Into<String>, op: crate::ddl::AddSchemaRuleBuilder) -> Result<Handle, BuilderError>`
   - A generic `pub fn try_op(&mut self, alias: impl Into<String>, op: impl TryIntoBatchOp) -> Result<Handle, BuilderError>` — the fallible mirror of the existing `Batch::op` escape hatch, and the ONLY way `AlterSubscriptionBuilder` needs (it has no dedicated non-try method today either, so no dedicated `try_alter_subscription` is needed — `try_op` covers it, consistent with how the PANICKING path handles it today via plain `.op(...)`).

   Each named method's body: call the builder's own `.build()`, propagate
   the error with `?`, then call the SAME internal `add_entry` (or
   `self.add_entry(...)`) the existing non-try sibling uses — do not
   duplicate `add_entry`'s logic. `try_op` calls `op.try_into_batch_op()?`
   then `add_entry`.

3. Do not add `_after`/`_silent` variants of the new `try_*` methods unless
   trivial to mirror — check if the existing non-try siblings have them and
   match that surface exactly for consistency, but don't invent NEW
   variants (e.g. `try_update_after`) unless the existing `update_after`
   pattern is trivial to replicate; if it adds meaningful complexity, skip
   it and note the gap in your report rather than guessing.

## Verification

- `TryIntoBatchOp` and the new `Batch::try_*` methods must be exported from
  the crate the same way `IntoBatchOp` and `Batch` already are (check
  `lib.rs`/`batch/mod.rs` re-exports).
- Tests (new file, e.g.
  `crates/shamir-query-builder/src/batch/tests/try_into_batch_op_tests.rs`,
  registered in `batch/tests/mod.rs` per repo test-layout convention — see
  `crates/shamir-query-builder/src/batch/tests/mod.rs` for the pattern):
  for EACH of the 5 builder types, ONE test that omits the required field
  and asserts `try_update`/`try_upsert`/`try_delete`/`try_add_schema_rule`/
  `try_op` returns `Err(BuilderError::Missing...)` (the SPECIFIC variant,
  not just "is an Err") — and does NOT panic. Also ONE happy-path test per
  type (all required fields set) asserting `Ok(Handle)` and that the
  resulting `Batch` (via `.build()`/`.try_build()`) contains the expected
  op — a regression guard proving the new path produces the SAME wire
  shape as the existing panicking path for well-formed input.

## Gate (MANDATORY — this is production code, not test-only)

```
cargo fmt -p shamir-query-builder -- --check
cargo clippy -p shamir-query-builder --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder
```

All three must pass before reporting done. If `fmt --check` fails, run
`cargo fmt -p shamir-query-builder` (scoped to this crate only, per
CLAUDE.md — never a repo-wide `cargo fmt --all`).

## Scope discipline

- Do NOT touch `Query`/`crate::write::Insert`/`CreateIndex`'s permissive
  `build()` behavior — a separate, lower-priority concern per the review,
  not this task.
- Do NOT change any EXISTING method signature in `batch.rs` or any existing
  `IntoBatchOp`/`From<...> for BatchOp` impl — additive only.
- Do NOT touch `Batch::try_build()` (batch-DAG validation, a different,
  already-correct concern) — this task is about the EARLIER panic at
  `.update(...)`/`.upsert(...)`/etc. call time, before `try_build()` is ever
  reached.
- Do NOT unify `BuilderError` (write-domain) with `BuildError` (batch-DAG
  domain) — `builder_error.rs`'s own doc comment explicitly documents these
  as deliberately separate error families; keep them separate.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

List every new type/method added and its exact signature. For each of the
5 builders, confirm both the negative (typed error, no panic) and positive
(correct wire shape) test pass. Give exact `cargo fmt --check` /
`cargo clippy` / `./scripts/test.sh -p shamir-query-builder` output.
