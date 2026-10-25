# P0 stage A2b — p0-mvcc-failed-history-write

## Goal

Card `RV-TX-2CB455A7` (failed `history.transact` leaves the RecordCell advanced with no rollback; the prior version is masked on point reads) in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` — read it whole including the 2026-10-24 revalidation paragraph. Code: `crates/shamir-tx/src/mvcc_store/` (store.rs ~:463 ~:536-553 ~:659-676 ~:789-802 ~:869-883, history/version_entry.rs ~:416 ~:632, history/mvcc_history.rs ~:906, mvcc_locks.rs `try_reserve`).

The design is DECIDED and BINDING (two rush reviewers + @oh final adjudication). Implement exactly this:

=== FINAL ADJUDICATION (@oh) — BINDING for A2b and A2c ===
(1) RV-TX-2CB455A7 (A2b): keep the cell bump-before-write; cell.version only grows and is an UPPER BOUND on the committed version, not proof that the version exists. A read that finds nothing at exactly cur_v takes that version as aborted and falls back: resolve_read(key, min(snapshot_or_floor, cur_v-1), 0) at all four exact-read sites (store.rs ~:463, version_entry.rs ~:416 and ~:632, mvcc_history.rs ~:906). When history.transact fails: remove every attempted version key and ts key BEFORE the VersionGuard settles as Aborted. If that removal fails, the version is not settled: gate.set_requires_reopen(), leave the floor stalled below it, refuse further writes (fail-stop). Reject restoring the cell (unsafe with concurrent writers / try_reserve lost update) and reject staged-publish-after-write (loses the conservative bump SSI relies on). Tests: T1 commit v5, fail write at v6 -> get_current/get_at(floor)/get_current_many/get_at_many/current_stream return v5; T2 two failures in a row; T3 set_versioned_many fault -> a None, k v5; T4 barrier-held W1 fails while W2 commits v7 -> reads v7; T5 try_reserve(k, snapshot=4) false after failure; T6 non-atomic store applies first Set then errors -> history.get(vk(6)) NotFound; T7 removal failure -> reopen required, floor below v6, no new writes; T8 rebuilding MvccStore from history shows no v6. T1 and T3 fail on HEAD.
(2) RV-WAL-A025886E (A2c): rule "a segment that has a sidecar never changes". seal_and_rotate: fsync, open the new active, swap it in, THEN write a v2 sidecar (WMT2) (move segment_set.rs ~:391-404 after ~:420) so a failed open leaves no sidecar. open: if the highest segment has a valid WMT2 sidecar treat it as sealed and create seq+1; it never goes through repair/reactivation. WMT1 sidecars are never trusted (replay instead). remove_blocking becomes cleanup only (warn). Tests: (a) seq0 with valid WMT2 sidecar and no seq1: open creates seq1, seq0 bytes unchanged, v60 lands in seq1, truncate_below(55) keeps it; (b) injected new-active open failure -> seq0 has no sidecar; (c) stale WMT1 max=10 on a segment holding v60 -> replay keeps v60; (d) (a)-(c) pass with remove_blocking forced to fail always.


Pay attention to: the cell is NEVER moved down or evicted; the four exact-read sites fall back through `resolve_read(key, min(snapshot_or_floor, cur_v-1), 0)` on an exact-version miss; compensating removal of the attempted version+ts keys happens BEFORE the VersionGuard settles as Aborted; a failed removal is fail-stop (`set_requires_reopen`, floor stalled below the version, further writes refused). Tests live in the module's tests folder (error_tests.rs or a new sibling), use fault-injecting stores and Notify barriers (no sleeps), and each must fail with only its own fix reverted — state the revert in the doc comment. Update every existing test that pinned the old masking behaviour.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
