# P0 stage A1 — p0-storage-cache

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): A concurrent write is never masked by a cache fill; every cache mutation is ordered with the inner store; `flush` returns an error instead of hanging when the write worker died.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-STORAGE-98F6650D — MemBufferStore::get_many cache-fill can poison a Tombstone over a concurrent write — the #539 bug class survives in the vectored read

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:522; crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:534
- Revalidation note: AGREE — OBSERVED: vectored backing results still become cache entries, including Tombstones, without rechecking dirty writes after the await, and subsequent reads remain cache-first.

### RV-STORAGE-E70A0DDC — CachedStore: unordered, non-atomic cache-mutation sites can leave the cache permanently behind inner

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-storage/src/stores/storage_cached.rs:461; crates/shamir-storage/src/stores/storage_cached.rs:700
- Revalidation note: AGREE — OBSERVED: synchronous writes and transactions still publish cache mutations independently after awaited backing mutations, with no same-key publication ordering or coordination with the asynchronous queue.

### RV-STORAGE-897437C3 — CachedStore::flush can hang forever if the async write-worker task dies before draining

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-storage/src/stores/storage_cached.rs:86; crates/shamir-storage/src/stores/storage_cached.rs:413
- Revalidation note: AGREE — OBSERVED: backing-store unwind still bypasses pending decrement and notification in the detached worker, while flush completion waits only for pending to become zero.

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
