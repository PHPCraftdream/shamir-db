בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Flaky Tests Log

Tests that have been observed failing intermittently. Each entry: when
observed, suspected cause, current mitigation, target fix.

**Project policy:** `flaky не наш — нужно чинить причину`. Flaky tests
are bugs, not noise. Land a fix that removes the flake, not a retry/skip
workaround.

---

## Open

*(none)*

---

## Known Production Limitations

Not flaky tests — these are documented limitations of the current
stage cuts. Listed here for visibility alongside test stability
notes.

### tx commits do not produce crash-safe data writes (4.G.2 partial)

`WalEntryV2` now contains the data ops (4.G.2 closed the emission
side), but recovery code that reads V2 entries on open and replays
them is not yet implemented. A crash between commit_tx Phase 4 and
Phase 7 will lose tx writes despite the durable WAL marker.

Tracking: Stage 7 of `docs/pre-transactional/` plan.

### SSI conflict detection blind to tx-mode writes

`MvccStore::version_of` returns 0 for keys last written via Phase 5
`base.transact` (bypasses `set_versioned`). Real SSI conflict
detection fires only for keys touched by non-tx `set_versioned`
callers, which don't exist in production yet.

Tracking: Stage 5 — route Phase 5 writes through MvccStore.

---

## Resolved

### `index2::vector::hnsw_adapter::tests::delete_removes_from_results`

- **First observed:** 2026-05-28 during Stage 4.D.6.c.2 workspace test run.
- **Stale diagnosis corrected (2026-05-29):** the original entry claimed
  the test "passes in isolation" and only failed "under full parallel
  workspace test run". **This was false.** Verified: the test fails
  **in isolation, single-threaded**, on roughly 1-in-4-to-10 runs
  (CI red ~10-25%). It is not a parallelism / `hnsw_rs`-shared-state
  bug.
- **Real cause:** recall non-determinism on a degenerate 2-node graph.
  The test inserted 2 points, soft-deleted `rid(1)` (the entry point),
  searched `k=10`, and asserted `results.len() == 1`. After the entry
  point is tombstoned, HNSW search on a 2-node graph intermittently
  returns 0 survivors — an inherent recall artifact on a tiny graph,
  **not** a soft-delete bug (soft-delete itself works correctly). The
  assertion depended on recall reaching the single survivor, which it
  does not guarantee. `hnsw_rs` 0.3.4 has no seed API, so the graph
  topology is non-deterministic per run.
- **Fixed in:** this commit (assertion + graph-size change in
  `hnsw_adapter.rs`). The test now:
  1. builds a **non-degenerate** graph (10 points along x) so recall
     over the survivors is reliable;
  2. asserts the deleted rid is **absent** (the actual contract of
     `delete`, independent of recall);
  3. asserts the surviving nearest neighbour **is** found (recall
     sanity on a graph large enough that it holds).
  Verified deterministic: 50 single-threaded cargo runs + 100
  direct-binary runs, 0 failures. Matches the presence/absence pattern
  of the sibling fixes below.

Related to: prior commit `bacdd5d test: fix two flaky tests introduced
during stage 0.5 / 1.2.B` — same backend family, same root cause
(HNSW recall on a tiny graph asserted via a strict count).

### `recall_at_10_on_1k_vectors` + `dot_product_metric_normalized`

- **Fixed in:** `f608fb9` (during Stage 0.5 / 1.2.B refinement).
- **Cause:** `dot_product` test was assert'ing via HNSW recall; recall
  is probabilistic. Recall threshold for the 1k benchmark was too tight
  given random layer assignment.
- **Fix:** rewrote `dot_product_metric_normalized` as direct `ShamirDist`
  test (no HNSW involvement). Relaxed recall threshold for the 1k test
  to 0.5 with `ef_search=400`.

### `commit_staged_inserts_all_into_graph`

- **Fixed in:** `bacdd5d`.
- **Cause:** HNSW search on a 3-point graph occasionally misses points
  due to random layer assignment — the test asserted strict `== 3`
  results.
- **Fix:** assert "closest vector found" instead of strict count.

### `test_redb_transact_atomic`

- **Fixed in:** `bacdd5d`.
- **Cause:** observer used two separate `get` calls each opening own
  read_txn — could legitimately straddle commit boundary. Not a bug
  in `transact`, just observer-side snapshot semantics.
- **Fix:** rewrote observer to use `get_many` for atomic snapshot.

---

## How to add an entry

1. Record date, exact test path, observed failure rate.
2. Suspected root cause — be honest, mark `unknown` if not investigated.
3. Mitigation in effect (none / retry / ignored / serial).
4. Target fix plan with timeline (next sprint? after Stage X? unknown?).
5. Cross-reference any related commits.
