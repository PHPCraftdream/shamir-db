# S5c-X PR6b-2 round 3, review finding 6 — zero-floor unpublished-read hazard in MVCC read paths

Worktree: worktrees/s5cx-follower-receipt-2
Branch: feat/s5cx-follower-receipt-2, clean tree @ 0721b609

⛔ NEVER run git reset / checkout / clean / stash / restore / rm, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
Do NOT run cargo/rustc/nextest or any build or test. Only the orchestrator runs `cargo fmt` at the very end.
Read and follow .agents/skills/rust-intel/SKILL.md if readable; CLAUDE.md conventions (imports at top, tests under tests/ dirs, TDD).

FILE SCOPE: crates/shamir-tx/src/mvcc_store/** and its tests. Do NOT touch shamir-engine receipt/repo files (another worker owns them) — if the only sound fix lives in the engine's receipt activation path, STOP that part and report it instead of editing out of scope.

THE HAZARD (from review, OBSERVED): the follower receipt pipeline (shamir-engine) durably writes history rows for local versions 1..N in an atomic batch, and only LATER publishes them (raises `gate.last_committed()`). On a FRESH follower repository the gate floor is 0, and the production read paths treat floor==0 as "bootstrap — unrestricted": `MvccStore::get_current_bytes` (crates/shamir-tx/src/mvcc_store/mod.rs:1323-1380: `let ov_cap = if floor > 0 { floor } else { u64::MAX }`, cold path `seek_latest_version` scans ALL history and returns it, and the `floor > 0 && v > floor` guard is skipped when floor==0). `TableManager::get_bytes` uses it (crates/shamir-engine/src/table/table_manager_crud.rs:710-712). Result: materialized-but-unpublished rows are READABLE while both watermarks are 0. The existing primitive test avoids this by raising the floor first (crates/shamir-tx/src/mvcc_store/write_receipt/tests/publication_bound_tests.rs:53-55).

REQUIRED:

1. INVESTIGATE first (grep/read, no guessing): how floor is seeded on real repos — `RepoTxGate::with_watermark` callers, recovery markers (`load_last_committed`), `gate.raise_published` callers, `publish_materialized`. Determine whether ANY legitimate path serves reads while floor==0 AND durable history rows exist (leader bootstrap: does recovery seed the floor before serving? do any tests rely on floor==0 unrestricted reads of history? grep tests constructing `RepoTxGate::fresh()` + materialize without raising the floor).
2. Implement the minimal sound fix so that materialized-but-unpublished versions are invisible to EVERY read path (point `get_current`/`get_current_bytes`, snapshot `get_at`/`get_at_many`, `current_stream`) EVEN when the gate floor is 0 — provided a real repository can be in that state. Two candidate directions; pick after investigation and justify in a code comment naming invariant I3 (history readers must not expose materialized-but-unpublished versions):
   a. Tighten the floor==0 bootstrap special-case (treat floor 0 as "nothing above 0 visible") — SAFE only if no legit path reads pre-floor history at floor 0 (verify recovery seeds floor before serving).
   b. Keep floor==0 unrestricted ONLY when the history store holds no rows above 0 that lack publication evidence — i.e. a cheap durable "published bound" consulted when floor==0 (would need a new gate/marker plumbing — more invasive; only if (a) is unsafe).
3. TESTS (add to crates/shamir-tx/src/mvcc_store/write_receipt/tests/publication_bound_tests.rs if it has room, else a new file in that tests/ dir wired via its mod.rs — check the ≤7-entry ratchet): fresh gate floor 0 + `materialize_committed_to_history` at version 1 (or 2): assert `get_current`, `get_current_bytes`, `get_at(key, 0)`... read them all and assert the unpublished row is INVISIBLE on every path; then `publish_materialized` → visible. NEGATIVE CONTROL: a companion assertion/test that demonstrates the pre-fix behavior would have exposed the row (e.g. via the raw history read contrast, mirroring the existing `materialized_rows_and_ts_are_durable_before_publish` pattern).
4. Run the ratchet: keep every touched file ≤1000 lines / folders ≤7 entries; do not reformat unrelated code.

FINAL MESSAGE: the chosen fix and why the alternative was rejected (with the evidence you found in step 1, file:line), every changed read path with file:line, the new tests and what each asserts, the negative-control mechanism, and anything you could not verify without a build.
