Task: G2 (task #526) — keyset-pagination short-page-on-stale-index-entries
fix. Found during task #496's `@fl` review (former task #518). **This
brief covers ONLY the short-page fix; the record-id tie-breaker (former
#517) is EXPLICITLY OUT OF SCOPE here** — see the note at the end of this
brief for why, and the follow-up task filed for it separately.

## Context (re-investigate — line numbers may have shifted)

`crates/shamir-index/src/legacy/sorted_index_manager.rs::lookup_range_first_k`
(currently ~line 688) walks the sorted index in value order and collects
the first `k` **physical index entries** (record ids) beyond a seek
boundary, stopping the instant it has `k` of them. Its caller,
`crates/shamir-engine/src/table/read_index_scan.rs::read_keyset_seek`
(currently ~line 443), then fetches each returned id's record body via
`get_many_bytes` (line ~469) and DROPS any id whose fetch comes back
`None` (line ~472-476: `if let Some(bytes) = opt { matched.push(...) }`).

**The bug:** if some of the `k` physical index entries turn out to be
stale (the index still lists a record id that's been deleted, or whose
current MVCC-visible version no longer matches what the index encodes —
eventual/lagging index consistency), `get_many_bytes` returns `None` for
those, and they're silently dropped from `matched` — but `lookup_range_first_k`
already stopped scanning at `k` PHYSICAL entries, never knowing some of them
would turn out to be dead. The final page can come back with FEWER than
`limit` records even though more genuinely-live records exist further
along in the index range that were never fetched.

## Fix

`lookup_range_first_k` (or `read_keyset_seek`, whichever is the more
natural place — investigate and pick) needs to keep advancing through the
index stream until either (a) `limit` **live** records have been
collected, or (b) the range is exhausted. Two viable shapes:

1. **Push the liveness check down into `lookup_range_first_k` itself** —
   requires it to become "aware" of whether a candidate id is live, which
   likely means passing it something that can answer that question (a
   closure, or the `get_many_bytes`-equivalent capability) — this couples
   the index-layer function to the storage-fetch layer, which may not be
   the cleanest shape (`sorted_index_manager.rs` currently has no
   dependency on record-body fetching).
2. **Loop at the `read_keyset_seek` call site**: call `lookup_range_first_k`
   for a batch of ids, fetch bodies, keep the live ones; if fewer than
   `limit` survived AND the previous batch wasn't already short of `limit`
   physical entries (i.e., there's more range left to scan), re-seek from
   the last-seen physical key and pull another batch, accumulating live
   records until `limit` is reached or the range is truly exhausted.
   This keeps the index/storage layering clean (each layer does what it
   already does) at the cost of a slightly more involved loop at the call
   site.

Prefer option 2 unless investigation reveals option 1 is meaningfully
simpler in the current code shape — use your judgment, but justify the
choice in your report.

**Correctness invariant to preserve:** the final result must still come
back in the SAME the value-ordered (ORDER BY direction) sequence as
today — don't reorder while looping. And this must not become an
unbounded scan: if the range is genuinely exhausted with fewer than
`limit` live records (there just aren't that many more rows), return
what was found — that's a normal short LAST page, not a bug. The bug is
specifically "silently returning short when live rows existed further in
the range and were never looked at."

## TDD/regression requirement

1. A test proving the CURRENT bug: seed a table with N records under a
   sorted index, delete or otherwise make-stale a strategic subset of
   entries near the start of the seek range (e.g. delete every other
   record in the first `limit` physical entries) such that naive
   first-`limit`-physical-entries collection would under-fill, then call
   the keyset-seek read path with that seek key/limit and assert the
   returned page actually has `limit` records (assuming enough live
   records exist further in the range) — this test MUST fail against the
   current (pre-fix) code and pass after the fix.
2. A test confirming a genuine last-page short-return still works
   correctly (range truly exhausted with fewer than `limit` live rows —
   should NOT loop forever, should return what exists).
3. Existing keyset-seek/pagination tests must stay green.

## Explicitly OUT OF SCOPE for this task

**Do NOT touch anything related to using `record_id` as a tie-breaker for
rows sharing the same ORDER BY value across page boundaries** (this was
investigated by the orchestrator and found to require a client-visible
wire-protocol change — `Pagination::After`'s `key: Vec<QueryValue>` /
TS client's `key: WireValue[]` is already a shipped client contract per
`crates/shamir-client-ts/src/core/types/query.ts:117` and exercised in
`crates/shamir-client-ts/src/core/builders/__tests__/query.test.ts` and
`crates/shamir-query-builder/src/query/tests/query_tests.rs` — extending
the seek-key shape needs a deliberate protocol decision, not a mechanical
backend fix). This is filed as its own separate design/investigation task.
If you notice anything else in this area that looks related to ties, leave
it alone and note it in your report rather than fixing it here.

## Test scope

```
./scripts/test.sh -p shamir-engine -p shamir-index
```

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-engine -p shamir-index
```

Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Implementation] Status: fixed
  > Which fix shape was chosen (option 1 vs 2 above) and why
  > New regression test(s) added, confirmed to fail pre-fix / pass post-fix
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-engine -p shamir-index: pass/fail
```
