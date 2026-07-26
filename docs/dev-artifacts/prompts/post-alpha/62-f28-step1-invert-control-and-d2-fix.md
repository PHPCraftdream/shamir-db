# Brief for F-28 Step 1 (#828, P1) — invert control on the implicit-tx path (S1A) + fix D2 (autocommit FK resolver=None bug)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is Step 1 of a 6-step decomposition (F-28) closing the gap between
what `crates/shamir-engine/src/query/batch/fk_restrict.rs`'s doc comment
(lines 9-19) claims is needed ("requires an `Arc<dyn TableResolver>` ...
which is a larger refactor tracked as a future task") and what a thorough
investigation (`@oh`, this session) found is ACTUALLY needed: a much
smaller mechanism that doesn't touch `Arc` at all.

**Verified technical fact**: `RepoInstance::run_implicit_batch_tx`'s
`stage` closure (`crates/shamir-engine/src/repo/repo_instance.rs`
~line 926-985) has signature
`F: for<'t> FnOnce(&'t mut TxContext) -> Pin<Box<dyn Future<Output = DbResult<WriteResult>> + Send + 't>>`.
The `dyn Future + Send + 't` trait object forces any captured reference's
lifetime to satisfy `'a: 't` for the UNIVERSALLY QUANTIFIED `'t`, which
collapses to `'a: 'static` — so a BORROWED `&dyn TableResolver` cannot be
captured (confirmed by direct compiler-error reproduction). An OWNED
`Arc<dyn TableResolver>` capture WOULD compile against this same
signature — but `QueryRunner.resolver` is `&'a dyn TableResolver`
(`query_runner.rs` ~line 275), not an `Arc`, and upgrading that specific
field to `Arc` ripples through ~12 signatures and ~25 test fixtures across
the codebase (a separate, much bigger, explicitly-rejected-for-this-task
alternative — do NOT do this; if you find yourself changing
`QueryRunner.resolver`'s type, stop and reconsider).

**The actual fix is smaller: invert control.** Don't fight the HRTB —
remove it from this call path entirely by splitting
`run_implicit_batch_tx` into an explicit `begin`/`commit` pair that the
caller drives with ordinary straight-line code (which CAN borrow
normally, no HRTB in the way).

## Part 1 — `begin_implicit_batch_tx` / `commit_implicit_batch_tx`

Add to `RepoInstance` (`repo_instance.rs`), right next to the existing
`begin_tx` (~line 776-807) and `commit_tx` (~line 823-828), which this
new pair wraps:

```rust
/// Open the implicit Snapshot batch tx exactly as `run_implicit_batch_tx`
/// does internally (set_actor + set_implicit(true)), returning the tx and
/// its SnapshotGuard so the CALLER can stage with ordinary straight-line
/// code that borrows freely (no HRTB) — see F-28 Step 1 (#828).
pub async fn begin_implicit_batch_tx(
    &self,
    actor: Actor,
    alias: &str,
) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
    let (mut tx, guard) = self
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: format!("implicit begin_tx: {}", e),
            code: None,
        })?;
    tx.set_actor(actor);
    tx.set_implicit(true);
    Ok((tx, guard))
}

/// Commit an implicit batch tx with the canonical BatchError mapping —
/// same error-precedence/coding as `run_implicit_batch_tx`'s existing
/// commit-error handling (UniqueViolation -> "unique_violation",
/// CasConflict -> "version_conflict" per FG-7). See F-28 Step 1 (#828).
pub async fn commit_implicit_batch_tx(
    &self,
    tx: shamir_tx::TxContext,
    alias: &str,
) -> Result<(), BatchError> {
    match self.commit_tx(tx).await {
        Ok(_outcome) => Ok(()),
        Err(commit_err) => {
            let (message, code) = match commit_err {
                crate::tx::CommitError::UniqueViolation { .. } => {
                    (commit_err.to_string(), Some("unique_violation".to_string()))
                }
                crate::tx::CommitError::CasConflict { .. } => {
                    (commit_err.to_string(), Some("version_conflict".to_string()))
                }
                other => (other.to_string(), None),
            };
            Err(BatchError::QueryError { alias: alias.to_string(), message, code })
        }
    }
}
```

Reimplement `run_implicit_batch_tx` itself as a thin wrapper over these
two (begin → `stage(&mut tx).await` with the SAME error mapping it uses
today → commit), so its ~14 existing callers across the workspace need NO
changes. Verify this by grepping for `run_implicit_batch_tx` callers
before/after — the count and call shape must be identical.

**Preserve exactly**: `tx.set_implicit(true)` (changefeed contract:
implicit ⇒ `tx_id == 0` — do not lose this), `tx.set_actor(actor)` (R2
provenance), RAII abort semantics (both `tx` and the `SnapshotGuard` must
still drop cleanly on any early return/error between begin and commit —
since callers will now hold these as plain local variables across `?`
points, verify no guard is accidentally dropped early or held past where
it should release).

## Part 2 — rewrite the 4 implicit-arm call sites + fix D2

In `crates/shamir-engine/src/query/batch/query_runner.rs`, the 4 implicit
(`None` branch) call sites for `BatchOp::Insert` (~line 981),
`BatchOp::Update` (~line 1091), `BatchOp::Delete` (~line 1210/1220ish —
re-locate exactly, other tasks in this campaign may have shifted line
numbers slightly), and `BatchOp::Set` (~line 1301) currently look like
this (Insert arm shown, others are structurally identical):

```rust
None => {
    let repo = self.resolver.resolve_repo(&table_ref.repo).await.map_err(...)?;
    let return_result = entry.return_result;
    // Move owned copies into the staging closure so the staged future
    // borrows ONLY the tx (the `for<'t>` HRTB requires no other
    // caller-scope borrows).
    let owned_op: shamir_query_types::write::InsertOp = op_ref.clone();
    let owned_table = table.clone();
    let owned_actor = self.actor.clone();
    repo.run_implicit_batch_tx(self.actor.clone(), alias, move |tx| {
        Box::pin(async move {
            owned_table.execute_insert_tx(&owned_op, tx, return_result, None, &owned_actor).await
        })
    }).await?
}
```

Rewrite to the straight-line form using Part 1's new methods — no owned
clones needed anymore since there's no HRTB closure to satisfy:

```rust
None => {
    let repo = self.resolver.resolve_repo(&table_ref.repo).await.map_err(...)?;
    let return_result = entry.return_result;
    let (mut tx, _guard) = repo.begin_implicit_batch_tx(self.actor.clone(), alias).await?;
    let wr = table
        .execute_insert_tx(op_ref, &mut tx, return_result, Some(self.resolver), &self.actor)
        .await
        .map_err(|e| BatchError::QueryError { alias: alias.to_string(), message: e.to_string(), code: e.code().map(str::to_owned) })?;
    repo.commit_implicit_batch_tx(tx, alias).await?;
    wr
}
```

**The D2 fix is the `Some(self.resolver)` instead of `None`** in the 4th
argument to each `execute_*_tx` call — do this for ALL 4 arms
(Insert/Update/Delete/Set). Match each arm's existing error-mapping style
exactly (some arms may already have their own `.map_err` closure shape
distinct from Insert's — copy each arm's OWN existing error handling, only
change the control-flow shape and the `None`→`Some(self.resolver)` swap,
not the error semantics).

**Why this matters (verify by tracing, don't just take this on faith)**:
`crates/shamir-engine/src/table/table_manager_validators.rs` (~line 148,
244) builds `ValidatorDb::new(tx, self, resolver)` where `resolver` was
always `None` on this implicit path. Trace forward:
`crates/shamir-engine/src/validator/validator_db.rs` (~line 205) —
`let Some(resolver) = self.resolver else { return Ok(false); };` (a doc
comment nearby claims this is "fail-open", i.e. skip the check — VERIFY
this claim is actually true or false by reading the CALLER of
`exists_in_table`/`exists_in`). Then
`crates/shamir-engine/src/validator/schema/schema_validator.rs`
(~line 161-186) — does `Ok(false)` from the FK existence check get
treated as "skip, no opinion" or as "the referenced row doesn't exist ⇒
`fk_violation`"? If it's the latter, EVERY autocommit insert/update into a
table with a schema-level `foreign_key` constraint is being wrongly
rejected today, regardless of whether the referenced row genuinely
exists — a real, independent bug, not merely a TOCTOU-adjacent nuance.
State your finding precisely in your final summary (confirmed real /
confirmed NOT a bug / found something different) — this drives whether a
SEPARATE, more urgent bug report needs to go to the user.

## Tests

**MANDATORY, test-then-fix in the SAME commit** (do not commit a
standalone failing test — confirm the bug reproduces against the
CURRENT code as your own verification step, then land the fix and the
now-passing regression test together, matching this campaign's
established discipline):

1. Schema with `foreign_key` on `child.parent_id -> parent.id`
   (`NoAction` or whichever default action is simplest to set up). Insert
   a valid parent row. Insert a valid child row referencing it via a
   **non-transactional (autocommit)** batch — i.e. NOT wrapped in an
   explicit `BeginTx`/`CommitTx`, the default single-batch-op path. Before
   your fix this should currently fail (confirm this, don't assume);
   after your fix it must succeed. Follow the existing FK e2e conventions
   in `crates/shamir-db/tests/declarative_schema_fk_*_e2e.rs` — their own
   comments already explain why they deliberately wrap inserts in
   transactional batches ("so the resolver is wired") — this new test is
   the missing autocommit counterpart those comments were implicitly
   flagging as a gap.
2. A regression guard that the REWRITTEN implicit-arm control flow still
   behaves identically for a case with NO FK constraints (plain
   autocommit insert/update/delete/set on an unconstrained table) — the
   refactor itself must not change behavior for the common case. Check
   whether existing tests already cover this broadly enough (likely
   yes, via the huge existing `@engine`/`@e2e`/`@server` suites) — if so,
   no NEW test is needed for this point, just confirm the full suite
   passes.
3. Verify (and correct if inaccurate) the "fail-open" doc-comment wording
   in `constraints.rs` (~line 82-91) and `schema_validator.rs`
   (~line 152-160) — once resolver is always `Some` on the writable
   autocommit path, is there ANY remaining path where `resolver` can
   legitimately be `None`? (e.g. an embedded `ShamirDb` used without a
   `PrincipalResolver`/similar, per this campaign's own precedent for
   "principal validation depends on launch mode" residuals.) If such a
   path exists, document it honestly instead of leaving a stale/inaccurate
   "fail-open" claim in place.

## Constraints

- Do NOT touch `QueryRunner.resolver`'s type (`&'a dyn TableResolver`) —
  changing it to `Arc` is the explicitly-rejected S1B alternative.
- Do NOT touch the reverse-FK probe functions themselves
  (`check_fk_restrict`, `plan_cascade`, `plan_fk_on_update`) — making them
  tx-aware is F-28 Step 2, a separate task, blocked on this one landing
  first.
- Do NOT change `run_implicit_batch_tx`'s public signature or behavior for
  its ~14 EXISTING callers — it must remain a drop-in wrapper.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh @engine
./scripts/test.sh @e2e
./scripts/test.sh -p shamir-server --full
```
