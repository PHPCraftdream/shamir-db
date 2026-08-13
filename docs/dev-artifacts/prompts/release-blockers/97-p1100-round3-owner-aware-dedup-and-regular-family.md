# #1100 round 3 (HIGH) — rederive_stale_value_ops_post_stage is owner-blind for unique UPDATE dedup and drops ALL regular-index removals

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Found by a final adversarial review of the whole session's accumulated work
(commit range `b556913f..HEAD`), reproduced with real tests that were run,
confirmed failing, then removed before the review's report was written.
Both bugs below live in `crates/shamir-engine/src/tx/pre_commit.rs`'s
`rederive_stale_value_ops_post_stage` function (added by `#1100`, currently
around lines 1850-2148 — re-read the whole function before editing, it may
have shifted).

## Bug A — unique-family UPDATE dedup is owner-blind, re-opens #1100

The `KvOp::Set` (UPDATE) branch's dedup against `tx.index_write_set` (around
`:2089-2116`, the `is_staged` closure ending in `staged_key == &index_key` at
`:2110`) compares staged ops to the re-derived op **by index key only** —
ignoring the op's owner, and ignoring whether the staged op is a
`RemovePosting` or a `SetPosting` relative to what the re-derived `op` is.

For the **unique** family the index key is owner-free (unlike the regular
family's posting key, which embeds the record id via `build_posting_key`).
So a re-derived `RemovePosting(K, owner=Z)` can be silently suppressed by an
unrelated, already-staged op for the SAME key but a DIFFERENT owner —
`RemovePosting(K, owner=Y)`. `#1097` round 3's stale-removal detection then
looks at that staged op, sees its declared owner `Y` doesn't match the
current durable owner (`Z`), and **retracts** it (`stale_remove_indices` +
`retain()`, around `:786`). Net result: nobody ever removes `K` — a
permanent dangling posting, exactly the bug `#1100` exists to fix.

### Confirmed reproduction shape (table `t`, unique index `by_email`)

```
tx0: INSERT Y{email:"k"}; INSERT Z{email:"m"}; COMMIT
tx2: BEGIN                                    // snapshot: Y{email:"k"}, Z{email:"m"}
tx1: DELETE Y; UPDATE Z SET email="k"; COMMIT   // durable after tx1: "k"->Z, "m" free
tx2: DELETE Y;                                  // stale -> stages RemovePosting("k", owner=Y)
     UPDATE Z SET name="n";                     // tx2's stale snapshot of Z still has email:"m",
                                                 // so the staged UPDATE op for Z carries no unique
                                                 // removal/set for "k" or "m" at all
     COMMIT
```

Expected once tx2 commits: `"k"` is free (Z's ACTUAL email is `"m"`, Y no
longer exists). Actual (current buggy behavior): `lookup_by_unique_index("k")`
keeps returning Y's now-dangling record id forever — Bug A's dedup silently
drops the re-derived `RemovePosting("k", owner=Z)` that
`rederive_stale_value_ops_post_stage` computes for Z (because it collides by
key alone with the already-staged, later-retracted `RemovePosting("k",
owner=Y)`). A subsequent `INSERT W{email:"k"}` then fails with `DuplicateKey`
naming a record (Y) whose data no longer carries that value at all.

### Fix for Bug A

Make the UPDATE branch's dedup **owner-aware and kind-aware**, matching the
pattern the `KvOp::Remove` (DELETE) branch already uses correctly
(`staged_removals_by_rid` at `:1914-1930`, keyed by `(rid, key)`). Concretely:
compare a re-derived `RemovePosting` against staged `RemovePosting`s by
`(key, owner)`, and a re-derived `SetPosting` against staged `SetPosting`s by
`(key, value)` (or owner, whichever correctly identifies "this is genuinely
the same claim" for that op kind) — never collapse a `RemovePosting` and a
`SetPosting` into the same dedup bucket just because they share a key, and
never let a different owner's op suppress this one's.

## Bug B — regular (non-unique) indexes are never fixed by #1100 at all

The DELETE branch's append filter (`:1979-1984`) only appends a re-derived
removal when it matches `IndexWriteOp::RemovePosting { owner: Some(ref
owner_bytes), .. }` — anything with `owner: None` is silently dropped
(falls through the `if let` with no `else`, no log, nothing).

`current_removals` comes from `mgr.plan_record_deleted(...)`
(`crates/shamir-index/src/base_index/index_manager.rs:2673`), which for the
**regular/hash family always emits `owner: None`**
(`index_manager.rs:2703-2707` — confirmed by direct reading: `ops.push(
IndexWriteOp::RemovePosting { key: posting_key, provenance:
regular_provenance(&def), owner: None })`). Only
`index_manager_unique.rs`'s unique-family planning populates `owner: Some(...)`.

So **every regular-index removal this function computes is silently
discarded** — the `plan_record_deleted` call for the regular family is pure
wasted work here, and despite `#1100`'s own general framing ("DELETE planned
from a stale snapshot value never removes the record's CURRENT posting —
permanent dangling posting"), the fix never actually applies to regular
(non-unique) indexes. Sorted/FTS indexes are not even planned in this
function at all (out of scope for this brief — regular/hash only, matching
what `plan_record_deleted`/`plan_record_updated` already cover here).

### Confirmed reproduction shape (same shape as #1100's ORIGINAL brief, but
### with `create_index` instead of `create_unique_index`)

```
tx0: INSERT R{email:"y"}; COMMIT
tx2: BEGIN
tx1: UPDATE R SET email="z"; COMMIT
tx2: DELETE R; COMMIT
```

After tx2 commits, a regular-index lookup on `"z"` still returns R's
(now-deleted) record id — a dangling regular-index posting, the general case
of the exact bug class `#1100` was supposed to close entirely.

### Fix for Bug B

The regular family's posting key already embeds the record id (via
`build_posting_key`, confirmed in `index_manager.rs`), so `owner: None` is
not itself wrong for that family — the append/dedup logic here just cannot
assume `owner: Some(...)` is how to identify "is this genuinely the same
claim". Restructure the append filter so it does not require `owner:
Some(...)` to append at all — dedup/identity for the regular family should
use the posting key itself (which is already unique per record for that
family), while the unique family continues to use `(key, owner)` per Bug A's
fix. Do not change `plan_record_deleted`/`index_manager.rs`'s `owner: None`
convention for the regular family — that's an established, intentional
design choice elsewhere in the codebase; fix the consumer in
`pre_commit.rs`, not the producer.

## Also review while touching this area (LOW, fold in if a real gap is found)

`pre_commit.rs:786-791` only rolls back `released`/`ever_released` when
`!live.contains_key(&k)` after a stale-removal retraction. If a LATER
`SetPosting` in the same walk re-claims the key, `ever_released` stays
`true` even though the removal op that originally set it was proven stale
and retracted — so Step 2's `released_and_touched` tolerance (`:876-881`)
could in principle be granted on the strength of a plan that no longer
exists. No end-to-end divergence has been demonstrated from this alone (a
prior investigation this session found every path it tried ends up saved by
the reclaiming `SetPosting`'s last-write-wins semantics) — but given Bug A
above sits in exactly this seam, review it while you're in this code and
harden it ONLY if you can construct (and then keep, as a regression test) a
genuine failing case. Do not speculatively rewrite the retraction logic
without a concrete repro — if you can't build one, say so plainly and leave
it alone.

## Required tests

Add tests for BOTH bug shapes above to
`crates/shamir-engine/src/tx/tests/p1100_stale_snapshot_delete_posting.rs`
(the existing #1100 test file), following this project's test-organisation
conventions. Each new test must:
- genuinely fail on the current (pre-this-brief) code,
- pass after your fix,
- be proven via mutation testing (temporarily revert your fix locally,
  confirm red, restore, confirm green) before you finalize.

Also re-run the existing `p1100_stale_snapshot_delete_posting.rs`,
`p1097_*`, and `p1101_*` test files to confirm no regression — the fix must
not change behavior for the cases those tests already cover correctly.

## Gate

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

All three must pass clean. Report real pass/fail counts, and confirm both
new reproduction tests were genuinely mutation-tested (red without the fix,
green with it) — not just "written and passing".
