# P1 stage F1 — p1-storage-membuffer

## Goal

MemBuffer drain/transact/cache coherence. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F1): A drain or transact never republishes an older snapshot over a newer flushed/committed value; `remove`/`set` existed/created flags are exact for keys resident only in inner; Drop of a non-empty dirty buffer is observable; `max_entries` is enforced; transact drains only what it needs; error paths and the audit telemetry are tested.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-STORAGE-E47DBF27 — Concurrent MemBuffer drains can overwrite a newer successfully flushed value with an older snapshot

- R0 verdict: `confirmed-open`, risk `high`; evidence: crates/shamir-storage/src/stores/storage_membuffer/core.rs:553; crates/shamir-storage/src/stores/storage_membuffer/revision.rs:147; changed by: none
- R0 note (includes the fix site): OBSERVED: drains still capture values before backing submission; added revision guards count simultaneous writers rather than exclude or order them. INFERRED: an old pre-submission snapshot can still overwrite a newer completed flush after its dirty entry is removed. Fix site: order or version-check backing application across drains/transact in crates/shamir-storage/src/stores/storage_membuffer/core.rs.

### RV-STORAGE-D11464BC — MemBufferStore::transact post-commit cache republish clobbers a concurrent writer's fresher value — lasting stale read

- R0 verdict: `fixed` — CLOSED, change nothing for it. OBSERVED: transact no longer republishes cache values, reads consult dirty first, and clean cache fills require matching revision tags; the named unconditional transaction-cache overwrite mechanism is removed.

### RV-STORAGE-EAB39827 — MemBufferStore::remove / remove_many misreport the existed flag for keys resident only in inner

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_membuffer/core.rs:250; crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:412; changed by: none
- R0 note (includes the fix site): OBSERVED: both removal paths use existed_hint, which never probes inner; an inner-only key still produces false before tombstoning. Fix site: crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs, with authoritative existence handling shared by both removals.

### RV-STORAGE-ADBB0C5B — set/remove created/existed flag precision varies by backend and write mode, with no capability disclosure

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_membuffer/core.rs:250; crates/shamir-storage/src/core/types.rs:154; changed by: none
- R0 note (includes the fix site): OBSERVED: MemBuffer existence hints still never consult inner on a local miss, while Store promises created-versus-updated precision; Fjall also retains separate existence probe and mutation. Fix site: reconcile flags with the public contract or disclose precision capabilities in crates/shamir-storage/src/stores/storage_membuffer/core.rs and core/types.rs.

### RV-STORAGE-707BD4FD — MemBufferStore::Drop silently discards a non-empty dirty buffer — zero observability

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_membuffer/core.rs:395; crates/shamir-storage/src/stores/storage_membuffer/core.rs:665; changed by: none
- R0 note (includes the fix site): OBSERVED: Drop only sets shutdown and notifies; the flusher exits on its next top-of-loop check without draining all remaining entries or reporting their loss. Fix site: add explicit shutdown/drain ownership and dirty-loss observability in crates/shamir-storage/src/stores/storage_membuffer/core.rs.

### RV-STORAGE-491185FB — MemBufferStore::transact drains the ENTIRE dirty buffer before every transact

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:274; crates/shamir-storage/src/stores/storage_membuffer/core.rs:647; changed by: none
- R0 note (includes the fix site): OBSERVED: every nonempty transact still calls drain_all, which repeatedly snapshots with usize::MAX independently of touched keys; revision guards do not narrow that work. Fix site: redesign touched-key draining together with backing-write ordering in crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs and core.rs.

### RV-STORAGE-C985620C — Persisted MemBuffer max_entries configuration is never enforced despite its hard-cap documentation

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-storage/src/stores/storage_membuffer/core.rs:24; crates/shamir-storage/src/stores/storage_membuffer/core.rs:278; changed by: none
- R0 note (includes the fix site): OBSERVED: max_entries is still documented as a hard cap, persisted/applied, but explicitly informational to build_cache, which configures only weighted max_bytes. Fix site: enforce entry count or correct the configuration contract in crates/shamir-storage/src/stores/storage_membuffer/core.rs.

### RV-STORAGE-995FD07D — Missing error-path tests; audit-§2.2 telemetry is written but never read

- R0 verdict: `partially-fixed`, risk `medium`; evidence: crates/shamir-storage/src/stores/storage_membuffer/core.rs:411; crates/shamir-storage/src/tests/stores/storage_cached_tests/worker_death_tests.rs:111; changed by: 973d0a0a (#269)
- R0 note (includes the fix site): OBSERVED: Cached worker-abort coverage now checks queued and subsequent operations fail and durability fallback runs, replacing part of the former worker-loss gap; MemBuffer flush_errors remains write-only, and MemBuffer backing-failure, Fjall submit-failure and partial-copy oracles remain absent. Fix site: expose/assert telemetry in crates/shamir-storage/src/stores/storage_membuffer/core.rs and extend the corresponding storage test modules.

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
