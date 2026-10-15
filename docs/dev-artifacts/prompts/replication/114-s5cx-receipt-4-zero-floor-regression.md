S5c-X PR6b-2 round 4: fix the zero-floor visibility REGRESSION in the MVCC read paths and prove it with tests.

⛔ NO git writes. NO cargo/rustc/nextest builds or tests. Orchestrator runs cargo fmt at the end.
FIRST: save this brief verbatim to docs/dev-artifacts/prompts/replication/114-s5cx-receipt-4-zero-floor-regression.md.

FILE SCOPE (nothing else): crates/shamir-tx/src/mvcc_store/{mod.rs, mvcc_history.rs, version_entry.rs} and crates/shamir-tx/src/mvcc_store/write_receipt/tests/publication_bound_tests.rs (plus mod.rs manifests if a new test file is genuinely needed — prefer extending the existing one).

BACKGROUND (round-3 reviewer P1 #3): the round-3 fix at mod.rs cold path (~:1344-1350) takes `seek_latest_version` (the MAX version) and `.filter(|&h| h <= floor)` — if the max is unpublished (h > floor), `hist_v` becomes None and with no overlay the fn returns `Ok(None)` at ~:1371 WITHOUT consulting older PUBLISHED history. That makes older published rows read as absent whenever a newer unpublished version exists. The bounded scan that is needed already exists: `MvccStore::scan_history_newest(key, snapshot)` in mvcc_history.rs:21-42 (newest version ≤ snapshot, returns (version, value)).

REQUIRED:

1. Fix the cold point-read path in `get_current_bytes` (mod.rs ~1323-1393) so that:
   - an unpublished newer version NEVER becomes the seeded cell and is never returned (I3 holds), AND
   - the newest PUBLISHED history version (≤ floor) IS found and used exactly as before the round-3 change (seed + read).
   Preferred shape: replace the `seek_latest_version(...).filter(h <= floor)` call with a single bounded helper `seek_latest_published(key, floor)` (new `pub(super) async fn` in mvcc_history.rs implemented over `iter_range_stream(Some(enc(key,0)), Some(enc(key,floor)))` — same shape as `scan_history_newest`/`seek_latest_version`; do NOT reuse scan_history_newest if its value semantics differ, but a version-only max over the bounded range is enough). This ALSO shrinks mod.rs (layout: mod.rs is over-limit and must be net-zero-or-negative vs origin/master this round — verify with `git show origin/master:crates/shamir-tx/src/mvcc_store/mod.rs | wc -l` = 1720; current 1734; your helper move must bring it to ≤1720).
2. Make `get_current_many` consistent (mod.rs ~1666): the cold-slot classification still has the `floor > 0 &&` exemption; since the cold slot delegates to the fixed `get_current_bytes`, simply drop the `floor > 0 &&` so a version above the floor (including at floor 0) routes to the (now correct) cold path — same rule as the point read.
3. Re-check `version_entry.rs` `v > floor` skip: correct as-is (stream skips above-floor). No change unless you find an inconsistency.
4. TESTS in publication_bound_tests.rs (extend the file; keep ≤1000 lines):
   - `older_published_history_visible_beside_unpublished_version` (THE regression test): fresh gate floor 0 → publish v1 (materialize+publish) → then `materialize_committed_to_history` v2 WITHOUT publishing. Assert: `get_current`/`get_current_bytes` return the v1 VALUE (not None!), `get_at(k, 1)` == v1, `get_at_many` == v1, `current_stream` emits k with the v1 value, the ts/as-of path still resolves (`version_at_or_before_ts(u64::MAX)` reachable — assert it is Some(≥1) only if the ts index semantics allow; otherwise assert via get_at). Then publish v2 → all paths return v2.
   - `zero_floor_hides_unpublished_only` (keep the round-3 property): unpublished v1 on a NEW key at floor 0 → invisible everywhere (reuse the existing two tests' assertions; keep them passing — if the existing `materialized_unpublished_invisible_at_zero_floor` still passes with the fixed helper, leave it).
   - Make sure each new test names the regression in its doc comment.
5. LAYOUT: mod.rs must end ≤1720 lines (move the cold-scan logic into mvcc_history.rs; mvcc_history.rs is 674 lines — plenty of room; keep imports at top; no new files).

FINAL MESSAGE: the exact new read-path shape with file:line, why unpublished rows stay invisible AND published rows are found, the test list with what each asserts, before/after line counts of mod.rs and mvcc_history.rs, and anything unverifiable without a build.
