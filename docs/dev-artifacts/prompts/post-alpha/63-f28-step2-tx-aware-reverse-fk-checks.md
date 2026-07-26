# Brief for F-28 Step 2 (#829, P1) — tx-aware reverse-FK checks (fixes D1)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-28 Step 1 (#828, landed, commit `02464f12`) removed the HRTB constraint
that used to force the reverse-FK check/plan calls to run OUTSIDE any tx
scope in `crates/shamir-engine/src/query/batch/query_runner.rs`. The 3
call sites (`check_fk_restrict` ~line 1150, `plan_cascade` ~line 1168,
`plan_fk_on_update` ~line 1051 — re-verify exact lines, may have shifted)
still run BEFORE either the explicit-tx or implicit-tx branch even begins,
reading `TableManager::list_stream` (latest-COMMITTED state only) and
`read_one_tx_bytes(id, None)` (`fk_actions.rs` ~line 514) — completely
blind to the CURRENT transaction's own staged writes.

This causes a real, DETERMINISTIC bug (no concurrency needed) — **D1**:

1. A transactional batch `[delete child; delete parent]` under
   `ON DELETE RESTRICT`: the RESTRICT gate's `child_has_reference`
   (`fk_restrict.rs` ~line 236) scans the child table's COMMITTED state,
   still sees the not-yet-committed-but-already-staged-deleted child row,
   and WRONGLY rejects the parent delete with `fk_restrict` — even though,
   by the time the transaction commits, the child will already be gone.
2. A transactional batch `[insert child referencing parent; delete
   parent]` under `ON DELETE CASCADE`: `collect_parent_values`/
   `discover_action_refs` (`fk_actions.rs` ~line 842, 900) read committed
   state only, so a child row inserted EARLIER IN THE SAME TRANSACTION is
   invisible to the cascade discovery/plan step — the cascade can silently
   fail to fan out to it, leaving an ORPHANED row after commit.

## The fix

Thread `tx: &shamir_tx::TxContext` through the reverse-FK probe/plan
functions and switch their reads to the tx-aware primitives that already
exist and already do exactly what's needed — no new read primitive, no
new `TableResolver` method:

- `TableManager::list_stream(n)` → `TableManager::list_stream_tx(Some(tx), n)`
  (`table_manager_streaming.rs` ~line 235-258) — same yielded shape
  (`Vec<(RecordId, RecordCow)>`), PLUS: (a) records SSI predicate deps when
  `tx.isolation == Serializable` (irrelevant here, these are always
  Snapshot implicit txs today, harmless no-op), and (b) overlays the tx's
  OWN staged writes (`merge_stream_with_tx_overlay`) — a staged delete is
  hidden from the stream, a staged insert is injected, a staged update
  yields the staged bytes. This is EXACTLY the read-your-own-writes
  semantics D1 needs.
- `fk_actions.rs`'s `parent_table.read_one_tx_bytes(*id, None)` (~line 514)
  → `Some(tx)`.

### Concrete call-site changes

1. **`fk_restrict.rs`**: add `tx: &shamir_tx::TxContext` to
   `check_fk_restrict` and `collect_parent_values` (its private helper that
   actually does the `list_stream` call, ~line 187-230); switch
   `collect_parent_values`'s scan to `list_stream_tx(Some(tx), batch_size)`.
   `child_has_reference` (~line 236-271) also needs `tx` threaded through —
   its full-scan FALLBACK path (~line 258-269, taken only when no index
   covers the field) needs `list_stream_tx(Some(tx), ...)` too. Its FAST
   PATH (`lookup_by_index`, ~line 244-254) is committed-index-only and has
   a subtler gap: **investigate and fix symmetrically with
   `ValidatorDb::exists_in_table`'s existing pattern**
   (`crates/shamir-engine/src/validator/validator_db.rs` ~line 218-308) —
   when the index returns EMPTY, `exists_in_table` additionally probes
   `staged_field_matches` (a scan over `tx.write_set` for a staged `Set`
   op matching `field == value`) before concluding "no match", because a
   staged-but-not-yet-committed insert is never in the index (indexing
   happens at commit). Mirror this exact pattern in `child_has_reference`
   — when `lookup_by_index` returns empty ids, additionally check
   `tx.write_set` for a staged insert/update matching the child field
   before returning `false`. (`discover_restrict_refs`, the schema-
   discovery helper, stays resolver-based and pre-tx — it's DDL-scoped,
   not row-scoped, a different and much weaker race, explicitly out of
   scope for this task.)
2. **`fk_actions.rs`**: add `tx: &shamir_tx::TxContext` to `plan_cascade`,
   `plan_cascade_recursive` (~line 218), `plan_cascade_for_ids`
   (~line 456), and their `collect_parent_values`/`discover_action_refs`
   helpers (~line 842, 900) — same `list_stream` → `list_stream_tx`
   swap at ~line 341, 598, 916. Fix the `read_one_tx_bytes(*id, None)` at
   ~line 514 to pass `Some(tx)`.
3. **`fk_on_update.rs`**: same pattern — add `tx` param to
   `plan_fk_on_update` and its internal probes, swap any `list_stream`/
   `read_one_tx_bytes(_, None)` calls to their tx-aware equivalents. Read
   this file in full first (not excerpted above) to find its exact
   call-site shapes — mirror `fk_actions.rs`'s treatment.
4. **`query_runner.rs`**: move the 3 call sites (`check_fk_restrict`,
   `plan_cascade`, `plan_fk_on_update`) from BEFORE the tx-branch match
   into AFTER a `tx: &TxContext` is available in EACH branch:
   - Explicit-tx branch (`Some(tx) => { ... }`): the plan/check call moves
     inside this arm, passing the existing `tx`.
   - Implicit branch (`None => { ... }`, now straight-line since Step 1):
     the plan/check call moves to AFTER `repo.begin_implicit_batch_tx(...)`
     returns `(mut tx, _guard)`, passing `&tx`.
   Since the discovery/planning logic itself is now duplicated per-branch
   (it wasn't shared before either — check whether the pre-Step-1 code
   already had this duplication or whether Step 1's straight-line rewrite
   changed that shape) — use judgment on whether a small shared helper
   reduces duplication without over-engineering; a modest amount of
   parallel structure between the two branches is already this file's
   existing convention (see how `apply_cascade_plan`/
   `apply_fk_update_plan` calls are already duplicated per-branch today).
5. **Doc comments**: rewrite `fk_restrict.rs`'s `## TOCTOU caveat` block
   (~line 9-19), `fk_actions.rs`'s equivalent (~line 16-33), and
   `fk_on_update.rs`'s (~line 51-59) to accurately state: the in-tx
   read-your-own-writes gap (D1) is now CLOSED by this task; the
   CROSS-transaction race (a genuinely concurrent OTHER transaction's
   write landing between this check and the eventual commit) remains open
   and is tracked as F-28 Step 3/4/5 (#830/#831/#832) — do not claim it's
   fully closed, that's a separate, larger piece of work.

## Tests

**MANDATORY, test-then-fix in the same commit**:

1. Transactional `[delete child row; delete parent row]` under
   `ON DELETE RESTRICT` — must now SUCCEED (confirm it currently fails
   with `fk_restrict` before your fix, as your own verification step).
2. Transactional `[insert child row referencing parent; delete parent
   row]` under `ON DELETE CASCADE` — after commit, assert NO orphan
   remains (the child row must have been cascaded/deleted, or the
   transaction must reject cleanly — check which behavior is actually
   correct given `plan_cascade`'s design: does discovering the
   NOW-VISIBLE staged child mean it gets included in the cascade plan and
   deleted alongside the parent? Reason through this and assert the
   CORRECT outcome, not just "no crash").
3. A CASCADE test where a child row referencing the same parent is staged
   as an UPDATE (changing its FK value to point elsewhere) earlier in the
   same tx — assert the plan correctly reflects the UPDATED reference, not
   the stale committed one.
4. If you implement the `child_has_reference`/`exists_in_table`-style
   staged-overlay fix for the index fast-path (point 1 above), add a
   focused test: a child row inserted (staged) referencing the parent,
   with an index covering the child field — `lookup_by_index` returns
   empty (index not updated until commit), the staged-overlay probe must
   still find the reference.
5. Existing tests: `fk_restrict_tests.rs`, `fk_actions_tests.rs`,
   `fk_on_update_tests.rs`, and the `declarative_schema_fk_*_e2e.rs`
   suite must all still pass — these functions' signatures are changing
   (`tx` param added), so their test call sites need updating too (purely
   mechanical for tests that don't exercise the new behavior).

## Constraints

- Do NOT touch the schema-discovery helpers (`discover_restrict_refs`,
  `discover_action_refs`'s table-enumeration part) — they stay
  resolver-based, pre-tx; only the ROW-level probes become tx-aware.
- Do NOT attempt to close the cross-transaction race in this task — that
  is F-28 Step 3/4/5, explicitly separate, larger work.
- Do NOT touch `query_runner.rs`'s Step-1 straight-line
  begin/execute/commit shape beyond moving the 3 call sites — no other
  restructuring.
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
```
