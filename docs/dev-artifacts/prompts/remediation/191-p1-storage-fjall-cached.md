# P1 stage F2 — p1-storage-fjall-cached

## Goal

Fjall pinned visibility/journal errors, cached capability and bounds. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F2): Pinned Fjall reads respect the publication watermark; WriteBatch journal encode/write errors fail the batch; `submit` never blocks a runtime worker on a full queue; CachedStore advertises only the capabilities it really provides; Async write-behind is bounded; CachedStore runs the backend-agnostic batch contract suite.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-STORAGE-CC8D9EB5 — Pinned Fjall raw reads bypass the publication watermark used to advertise native batch visibility atomicity

- R0 verdict: `confirmed-open`, risk `high`; evidence: Cargo.lock:1327; crates/shamir-storage/src/stores/storage_fjall.rs:587; changed by: none
- R0 note (includes the fix site): OBSERVED: Fjall remains pinned to 3.1.6; local dependency source applies batch entries before publication, while raw get passes SeqNo::MAX through to memtable lookup; the shim uses raw reads and still advertises atomic visibility. INFERRED: preemption during apply exposes a partial batch. Fix site: use publication-bounded snapshots or withdraw the guarantee in crates/shamir-storage/src/stores/storage_fjall.rs.

### RV-STORAGE-EC3A171B — Pinned Fjall WriteBatch discards journal encoding/write errors before reporting successful application

- R0 verdict: `confirmed-open`, risk `high`; evidence: Cargo.lock:1327; crates/shamir-storage/src/stores/storage_fjall.rs:200; changed by: none
- R0 note (includes the fix site): OBSERVED: HEAD still pins Fjall 3.1.6, and its locally cached batch implementation discards write_batch's Result before persistence and memtable application; the shim propagates only commit's returned error. INFERRED: a transient journal failure followed by successful persistence can still be acknowledged. Fix site: crates/shamir-storage/src/stores/storage_fjall.rs and its backend dependency selection; require propagation of the underlying journal error.

### RV-STORAGE-9E9A0A71 — FjallStore::submit blocks the tokio executor thread when the 1024-slot worker queue fills

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_fjall.rs:118; crates/shamir-storage/src/stores/storage_fjall.rs:353; changed by: none
- R0 note (includes the fix site): OBSERVED: insert/transact submission uses synchronous send into a 1024-slot SyncSender before awaiting the reply; INFERRED: saturation blocks the executing runtime thread, not merely the task. Fix site: make queue admission asynchronous or off-runtime in crates/shamir-storage/src/stores/storage_fjall.rs.

### RV-STORAGE-007DEF6B — Cache wrapper capability forwarding overpromises whole-batch visibility

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_cached.rs:402; crates/shamir-storage/src/stores/storage_cached.rs:801; changed by: none
- R0 note (includes the fix site): OBSERVED: capability forwarding remains true when inner advertises it; the new single owner still publishes transaction cache actions individually, and cache-hit reads do not validate the epoch. INFERRED: preemption between actions exposes new A with old B. Fix site: fence batch publication/read visibility or downgrade the capability in crates/shamir-storage/src/stores/storage_cached.rs.

### RV-STORAGE-E02C8A80 — CachedStore WriteMode::Async uses an UNBOUNDED write-behind channel

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_cached.rs:600; crates/shamir-storage/src/stores/storage_cached.rs:685; changed by: none
- R0 note (includes the fix site): OBSERVED: the rewritten owner still accepts value-bearing commands through an unbounded channel without admission limits; each caller now awaits its acknowledgement, limiting sequential producers but not unbounded concurrent submissions behind a stalled owner. Fix site: bound command admission and retained bytes in crates/shamir-storage/src/stores/storage_cached.rs.

### RV-STORAGE-FA461E63 — TDD gap: CachedStore (both write modes) never runs the backend-agnostic batch contract suite

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-storage/src/tests/types_tests.rs:38; crates/shamir-storage/src/tests/stores/storage_cached_tests/mod.rs:3; changed by: none
- R0 note (includes the fix site): OBSERVED: repository-wide helper references remain limited to InMemory, MemBuffer and Fjall; newly registered Cached ordering and worker-death tests do not invoke the common suite. Fix site: crates/shamir-storage/src/tests/stores/storage_cached_tests/basic_tests.rs, invoking the helper for both modes.

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
