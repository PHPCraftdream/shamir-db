# F-73 (#900) — make commit-time index re-derivation fail closed

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The bug

`rederive_index2_ops_post_stage(tx: &mut TxContext, repo: &RepoInstance)`
(`crates/shamir-engine/src/tx/pre_commit.rs`, called once from
`pre_commit_prelock`, currently around line 607) returns `()` — it has no
way to report failure — and its body swallows every error class it
encounters:

- Storage reads: `Err(_) => {}` (index2 path) / the equivalent arm in the
  sorted-index path — a non-`NotFound` `data_store.get` error is silently
  treated as "skip this record."
- Backend planning results: `if let Ok(ops) = backend.plan_insert_tx(...)`
  / `plan_update_tx` / `plan_delete_tx` / `sorted_mgr.plan_record_created`
  / `plan_record_updated` / `plan_record_deleted` — an `Err` from any of
  these silently contributes nothing.
- Record decode: `let Ok(new_rec) = InnerValue::from_bytes(&v) else {
  continue };` / the equivalent for `old_rec` — a decode failure silently
  skips the record instead of surfacing as corruption.
- Malformed staged key: `let Some(rid) = RecordId::try_from_bytes(&k) else
  { continue };` — treated as "not our problem" rather than an internal
  invariant violation (a staged key that doesn't decode as a `RecordId` at
  commit time is a bug, not a normal runtime condition).

**Consequence:** a tx stages before a CREATE INDEX; the new index
finishes backfill; at commit, the generation change triggers
re-derivation (this exact function); a transient read error (or a decode
error, or a malformed key) makes it silently skip a row. The data
mutation still proceeds to WAL/Phase 5a — commit reports **SUCCESS** —
and the index is now permanently diverged from the table's data, with no
error ever surfaced to the caller or the operator.

**This is the same fail-open class F-55 (#881, commit `f9eed337`) and
F-65 (#891, commit `28d39f31`) already fixed** for FK reverse-cache
discovery and FK indexed-action fast paths respectively. Read both commits
in full before writing any code — they establish the fix shape (propagate
instead of swallow, distinguish `NotFound` from a real error, construct a
typed error rather than a generic string) and the reasoning style this fix
must match.

## The fix

1. Change `rederive_index2_ops_post_stage`'s signature to
   `-> Result<(), TxError>` (the same error type `pre_commit_prelock`
   itself returns — check that function's current `Result<PreLockResult,
   TxError>` signature) and propagate every error listed above with `?`
   (or an explicit `map_err`/`ok_or_else` where the source isn't already a
   `TxError`), instead of swallowing it.
2. `NotFound` stays the ONLY case treated as "this is an insert, not an
   update" — that is the PROVEN semantics at this specific call site
   (Phase 5a hasn't run yet, so the store still holds the pre-tx value;
   `NotFound` genuinely means "no pre-tx row"). Do not extend this
   blanket-fallback treatment to any OTHER error variant.
3. A malformed staged key (`RecordId::try_from_bytes` failing) or a
   record that fails to decode (`InnerValue::from_bytes` failing) at
   commit time is an internal invariant violation — the staging path
   guarantees well-formed keys/values reach here — so treat it as an
   error (fail the tx), not a `continue`.
4. Update `pre_commit_prelock`'s call site (`rederive_index2_ops_post_stage(tx,
   repo).await;`) to `rederive_index2_ops_post_stage(tx, repo).await?;` —
   confirm this runs BEFORE Phase 4's WAL begin (it already does; the
   call site's own doc comment says so) so an early-returned error means
   the tx aborts with NO WAL entry and NO data/index mutation published.
   Verify this ordering explicitly, don't just assume the comment is
   still accurate — code and comments drift.
5. Apply the identical treatment to the sorted-index re-derivation block
   in the same function (the second half, gated by `tx.sorted_stage_gens`)
   — it has the exact same swallow shape (`Err(_) => {}` on
   `data_store.get`, `if let Ok(ops) = sorted_mgr.plan_record_*`). Both
   halves are one function; fix both, they're one commit.
6. Do not change the generation-gate short-circuits (`if reg.generation()
   == stage_gen { continue; }` and the sorted equivalent) — those are
   legitimate zero-cost fast-path skips (no index2/sorted backend was
   registered since stage), not error handling. Only the error-handling
   `Err`/`else`/`if let Ok` arms inside the per-record loops are in scope.

## Fault-injection seam — reuse, don't invent

This codebase has two established conventions for injecting a fault at a
specific point for a red-then-green test — use whichever fits each site,
do not invent a third mechanism:

- A `OnceLock`-backed hook object, e.g.
  `TEST_READ_ONE_TX_BYTES_FAILURE` / `ReadOneTxBytesFailHook`
  (`crates/shamir-engine/src/table/table_manager_streaming.rs`, consumed
  by `crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs`)
  — good for "make the next matching read fail."
- A simple tx-id-targeted `AtomicU64`/`AtomicBool` flag, e.g.
  `FAIL_PHASE_5C_TX_ID` (`crates/shamir-engine/src/tx/commit_phases.rs`,
  consumed by `crates/shamir-engine/src/tx/tests/commit_async_visibility_tests.rs`)
  — good for "make phase X fail only for tx N," which composes cleanly
  with a test that needs the failure to hit exactly one of several
  concurrent/sequenced transactions.

## Definition of done

- Fault-injection tests for **at least two distinct swallow sites**: one
  storage-read error (a non-`NotFound` `data_store.get` failure) and one
  decode error (`InnerValue::from_bytes` or `RecordId::try_from_bytes`
  failing), each proven on BOTH families (index2 and sorted) — so at
  minimum 4 targeted fault tests, more if you find it clearer to split
  insert/update/delete distinctly.
- Each test must first be run against the PRE-fix code path to confirm it
  is a genuine red (the tx commits successfully and the index silently
  diverges) before the fix lands — do not accept a test that would also
  pass unmodified against today's swallowing code; that would prove
  nothing. State in the commit message which tests you personally
  confirmed red-then-green this way.
- Each test asserts: the commit call returns `Err`, AND no partial
  state was published — no WAL entry for the tx, no data mutation
  visible, no partial/incorrect index posting. Check this via whatever
  this codebase's existing commit-abort tests assert (grep
  `commit_async_visibility_tests.rs` and similar for the pattern).
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine -p shamir-tx --full` green.
- Do not touch the generation-gate short-circuit logic or any other
  function in `pre_commit.rs` beyond `rederive_index2_ops_post_stage` and
  its direct call site.
- Do not run this task concurrently with any other task touching
  `pre_commit.rs` or `commit_phases.rs`.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
