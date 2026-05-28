בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Flaky Tests Log

Tests that have been observed failing intermittently. Each entry: when
observed, suspected cause, current mitigation, target fix.

**Project policy:** `flaky не наш — нужно чинить причину`. Flaky tests
are bugs, not noise. Land a fix that removes the flake, not a retry/skip
workaround.

---

## Open

### `index2::vector::hnsw_adapter::tests::delete_removes_from_results`

- **First observed:** 2026-05-28 during Stage 4.D.6.c.2 workspace test run.
- **Symptom:** passes in isolation (`cargo test -p shamir-engine --lib delete_removes_from_results`),
  fails intermittently under full parallel workspace test run.
- **Suspected cause:** HNSW graph state interaction with concurrent
  test parallelism — `hnsw_rs` internal state may share something
  across `HnswAdapter` instances when many tests run together
  (thread-local, static counters?). Or: the test's `assert!(result.len() < N)`
  threshold is too tight and depends on graph layer-assignment randomness.
- **Mitigation:** none yet; runs are passing on retry.
- **Target fix:** investigate after Stage 4.D.6 lands. Either:
  - Tighten `ef_search` / seed RNG deterministically.
  - Replace soft assertion with explicit check that the deleted rid
    is absent from the result set, not relying on `len`.
  - Move to serial test (`#[serial]`) if HNSW internals are non-thread-safe.

Related to: prior commit `bacdd5d test: fix two flaky tests introduced
during stage 0.5 / 1.2.B` — same backend family; this looks like a
sibling case.

---

## Resolved

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
