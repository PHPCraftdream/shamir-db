# #1100 HIGH — DELETE/UPDATE planned from a stale snapshot never removes the record's CURRENT posting

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background — confirmed by the orchestrator's own investigation before writing this brief

`crates/shamir-engine/src/table/table_manager_tx_ops.rs`'s `delete_tx`
(~line 1390) reads the record's value via `read_one_tx_bytes(id,
Some(&*tx))` — the value **as of this transaction's own snapshot**, under
`Snapshot` isolation. It then plans unique/regular-index `RemovePosting`
ops (`plan_delete_ops`/`plan_base_index_delete_ops`) FROM that
snapshot-time value. `update_tx`/`update_tx_bytes` have the equivalent
shape for the "old" half of their old→new diff (search for
`read_one_tx_bytes`/`old_view`/`old_inner` in the same file).

**The bug**: if the record was updated by ANOTHER, concurrently-committed
tx AFTER this tx's snapshot was taken but BEFORE this tx deletes (or
updates) it, the planner computes the removal key from the OLD (pre-that-
update) value — not the record's REAL current durable unique/index key.
The op that should remove the record's ACTUAL current posting is never
generated at all.

### Reproduction (already traced by the orchestrator, confirm it reproduces before doing anything else — this is the Red step)

```
tx0: INSERT R{email:"y"}; COMMIT                    // "y" -> R
tx2: BEGIN (snapshot taken here, sees email:"y")
tx1: UPDATE R SET email="z"; COMMIT                 // "y" removed, "z" -> R
tx2: DELETE R; COMMIT                               // plans Remove("y") from ITS stale snapshot
```

`Remove("y")` is a no-op at commit (already durably gone — `"y"` was
removed by `tx1`). **No op is EVER planned for `"z"`** — the record's
actual current posting. Result: posting `"z" -> R` survives `R`'s deletion
permanently. Every later `INSERT W{email:"z"}` fails with `DuplicateKey`
naming a record (`R`) that no longer exists, until a manual reindex.
`lookup_by_unique_index("z")` returns a dangling rid forever.

Write this reproduction as a new `#[tokio::test]` FIRST
(`crates/shamir-engine/src/tx/tests/`, new file
`p1100_stale_snapshot_delete_posting.rs`, following this directory's
existing layout convention — `mod.rs` is a manifest only, see
`p1097_remove_posting_owner.rs`/`p1096_tx_aware_unique_check.rs` for the
established test-file shape) and confirm it actually fails on the current
`master` before writing any fix. Report the failure mode you observe
(does the second `INSERT W{email:"z"}` get a spurious `DuplicateKey`? does
`lookup_by_unique_index("z")` return a dangling rid pointing at the
deleted `R`? confirm which, or both).

## Why this needs a DIFFERENT fix shape than `#1097`/`#1098`/`#1099`'s owner-comparison/read-order fixes

`#1097`'s `owner: Option<[u8; 16]>` field on `RemovePosting` lets
`pre_commit.rs`'s Step 1 correctly judge whether an EXISTING `RemovePosting`
op should be applied. **This bug is about an op that should exist but was
never planned in the first place** — no amount of owner-tagging a
non-existent op fixes a missing op. The fix has to either (a) make the
planner read the record's CURRENT durable value instead of the tx's
snapshot value when planning removal ops, or (b) detect at commit time
that a staged `Remove`/key-changing `Set` for a record's index-relevant
fields doesn't match what the record's CURRENT durable value would
require, and re-derive the correct removal op then.

## The existing precedent this codebase already has for "re-derive against current durable state at commit time" — read before choosing an approach

`crates/shamir-engine/src/tx/pre_commit.rs`'s
`rederive_base_index_ops_post_stage` (~line 1579) already does something
structurally similar: for each `KvOp::Set` this tx staged, it reads the
record's **pre-tx durable value** via `read_pre_tx_bytes` (~line 986 —
reads directly from `data_store`, bypassing the tx's own snapshot
entirely) and re-plans `plan_record_updated`/`plan_record_updated_unique`
against it. **This is NOT itself a fix for #1100** — it's gated by
`if mgr.generation() == stage_gen { continue; }` (~line 1600), meaning it
only re-derives when the base_index DEFINITION set changed (a
CREATE/DROP INDEX raced this tx), never when the record's own VALUE
changed underneath a stale snapshot, which is #1100's actual root cause —
these are two different staleness sources entirely. **But it IS the
established technique to reuse**: `read_pre_tx_bytes`'s "read current
durable state directly from `data_store`, bypassing the tx snapshot" is
exactly the primitive #1100 needs, just triggered by a different
condition and covering `KvOp::Remove` too (which
`rederive_base_index_ops_post_stage` currently doesn't handle at all —
check whether it silently ignores `Remove`, since its `match kvop` only
shows a `KvOp::Set` arm in the code the orchestrator read; if there's no
`Remove` arm, that's ALSO relevant background, though fixing that gap
directly is out of scope here unless your own investigation finds it's
the same root cause).

## Investigate before choosing a fix shape — do not implement free-hand

1. Read `delete_tx`/`update_tx`/`update_tx_bytes`'s full planning call
   sites in `table_manager_tx_ops.rs`, and `plan_delete_ops`/
   `plan_base_index_delete_ops`/`plan_record_updated`/
   `plan_record_updated_unique`'s signatures in `shamir-index`.
2. Read `rederive_base_index_ops_post_stage` in FULL (`pre_commit.rs`,
   ~1579-1700+), including how it handles the `KvOp::Set` case, whether it
   has (or lacks) a `KvOp::Remove` case, and how `appended` ops get merged
   back into `tx.index_write_set` afterward (search for where `appended`
   is used after this function returns).
3. Decide: is the right fix (a) a NEW, similarly-shaped commit-time
   re-derivation pass — read each staged `Remove`'s (and key-changing
   `Set`'s) record's CURRENT durable value via `read_pre_tx_bytes`-style
   read, compare against what THIS tx's own snapshot-time value produced,
   and append any MISSING removal op the current value's diff reveals —
   or (b) something narrower scoped just to `delete_tx`/`update_tx`'s
   OWN planning call, reading current durable state directly at stage
   time instead of via `read_one_tx_bytes(id, Some(tx))`. Consider: (b)
   changes stage-time semantics (a plain `Snapshot`-isolation read no
   longer reflects the tx's own snapshot for THIS specific purpose, which
   may have implications you need to think through — e.g. does it break
   `Snapshot` isolation's contract for anything else this same read
   result feeds?), while (a) mirrors the ALREADY-established commit-time
   re-derivation pattern more closely and keeps `Snapshot` isolation's
   read semantics untouched everywhere else. State your choice and full
   reasoning in your final summary — this decision matters more than the
   line-level implementation.
4. Whichever shape you choose, this MUST be triggered by "did the
   record's CURRENT durable value differ from what this tx's snapshot saw
   AT THE FIELDS THIS TABLE'S INDEXES CARE ABOUT" — not by a broader
   "always re-derive on every commit" pass (that would be needlessly
   expensive on the common, uncontended case) and not by the EXISTING
   `mgr.generation() == stage_gen` gate (wrong signal entirely — that
   detects DEFINITION changes, not VALUE changes).

## Tests

1. The reproduction from the Background section — must FAIL on current
   `master`, PASS after your fix (the TDD Red→Green this whole session
   has used throughout).
2. A test that a delete against a record whose value did NOT change
   concurrently still removes its posting correctly (no regression —
   don't let a broader fix accidentally re-derive/duplicate ops for the
   common, uncontended case).
3. The equivalent scenario for `update_tx`/`update_tx_bytes` (not just
   `delete_tx`) — a concurrent update changing the unique key underneath
   this tx's stale snapshot, then this tx updates the SAME record's
   NON-unique field, must still correctly remove the record's real
   current unique posting if the update also touches that field (check
   whether `update_tx`'s old→new diff has the identical stale-snapshot
   exposure `delete_tx` does — trace it yourself, don't assume).
4. Mutation-test your own fix (this session's established discipline for
   every commit-time correctness fix): temporarily disable the
   re-derivation/current-state-read logic, confirm the reproduction test
   fails, restore, confirm it passes and the full gate is green.

## Gate

```
cargo fmt -p shamir-engine -p shamir-index -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-index -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index -p shamir-tx --full
```

All three must pass clean. Report the real pass/fail counts, the fix
shape you chose and why, and confirmation of the mutation test in your
final summary.
