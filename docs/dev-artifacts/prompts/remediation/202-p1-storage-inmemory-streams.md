# P1 stage F3 — p1-storage-inmemory-streams

## Goal

InMemory/stream contracts: batch_size 0, cursor, streaming. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F3): `batch_size == 0` has one specified behaviour on every backend; InMemory range resume uses an excluded cursor; reverse and full-scan streams yield incrementally instead of materialising the range; a racing duplicate on `set` is not swallowed.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-STORAGE-5E99BEDE — batch_size == 0 is unspecified: InMemoryStore yields empty batches forever; fjall/cached silently return zero results

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_in_memory.rs:263; crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:145; changed by: none
- R0 note (includes the fix site): OBSERVED: zero-sized nonempty InMemory full/prefix scans drain nothing and repeatedly yield empty batches; Cached/Fjall take(0) terminate, and MemBuffer still forwards zero before merge normalization. Fix site: establish and enforce one zero-batch policy at stream entry points, beginning in crates/shamir-storage/src/stores/storage_in_memory.rs and storage_membuffer/store_impl.rs.

### RV-STORAGE-822DBD98 — InMemoryStore::set: remove+re-insert update path built on a false premise — swallows a racing Duplicate

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_in_memory.rs:223; crates/shamir-storage/src/stores/storage_in_memory.rs:231; changed by: none
- R0 note (includes the fix site): OBSERVED: existing-key set still removes then reinserts, ignores insertion failure, and get probes the same tree without a fence; INFERRED: concurrent readers can observe an absence between two present states. Fix site: replace the remove/reinsert update in crates/shamir-storage/src/stores/storage_in_memory.rs with atomic replacement semantics.

### RV-STORAGE-B4524AFA — InMemoryStore::iter_range_stream resumes inclusive + blind-skip instead of the mandated Bound::Excluded cursor

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_in_memory.rs:295; crates/shamir-storage/src/stores/storage_in_memory.rs:304; changed by: none
- R0 note (includes the fix site): OBSERVED: range resumption remains inclusive followed by unconditional first-item skipping; INFERRED: after yielding A and deleting it, the next size-one pull skips surviving B. Fix site: use an excluded resume bound in crates/shamir-storage/src/stores/storage_in_memory.rs.

### RV-STORAGE-D00D27DE — Reverse range streams drain the ENTIRE range into RAM before reversing; not overridden by InMemory/Cached/Mirrored

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/core/types.rs:543; crates/shamir-storage/src/stores/storage_mirrored.rs:437; changed by: none
- R0 note (includes the fix site): OBSERVED: default reverse consumes the entire forward range before yielding, InMemory/Cached still inherit it, and Mirrored delegates to its primary. Fix site: implement bounded reverse cursors in crates/shamir-storage/src/stores/storage_in_memory.rs and storage_cached.rs rather than relying on core/types.rs default_reverse.

### RV-STORAGE-01F46FA6 — InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole corpus before the first yield

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_in_memory.rs:252; crates/shamir-storage/src/stores/storage_in_memory.rs:339; changed by: none
- R0 note (includes the fix site): OBSERVED: full and prefix streams collect every matching key/value handle during construction, then drain vector prefixes; Bytes payloads are shared, but handle allocation remains corpus-sized. Fix site: use bounded lazy cursors in crates/shamir-storage/src/stores/storage_in_memory.rs.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
