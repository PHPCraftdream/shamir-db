# P1 stage F15 — p1-numa-linux

## Goal

NUMA Linux probe: CPU_SET bound, cpulist parsing, errors, tests. Fix the open P1 review cards below at their root cause. Acceptance (from P1-FIX-PLAN.md, row F15): CPU sets are bounded by `CPU_SETSIZE`; `parse_cpulist` rejects unbounded ranges; probe errors are reported; Linux tests do not assume slot 0; error paths are tested; inline tests move to `tests/`.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the R0 re-decision below (2026-10-26, against master after the P0 program) is authoritative for where the code lives now and which cards are still open.

## Cards

### RV-NUMA-3378686A — `CPU_SET` without a `CPU_SETSIZE` bound: silent mask truncation (glibc) / out-of-bounds write (musl)

- R0 verdict: `confirmed-open`, risk `medium`; evidence: crates/shamir-numa/src/platform/linux.rs:139; Cargo.lock:1923; changed by: none
- R0 note (includes the fix site): OBSERVED: Discovered CPU IDs reach CPU_SET without a mask-domain check; locally inspected pinned libc 0.2.186 uses checked array indexing. INFERRED: An out-of-domain CPU ID panics rather than returning AffinityError; the title's truncation/out-of-bounds-write allegation is unsupported. Fix site: crates/shamir-numa/src/platform/linux.rs, rejecting or safely representing CPUs beyond the fixed mask domain.

### RV-NUMA-DC25AAC1 — Linux host tests assume topology slot 0 is always pinnable and populated

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-numa/tests/linux_topology.rs:17; crates/shamir-numa/src/platform/linux.rs:188; changed by: none
- R0 note (includes the fix site): OBSERVED: Tests still require slot-zero pinning and a populated first node without checking eligible affinity, despite best-effort empty-node discovery. INFERRED: Restricted cpusets or empty first-node CPU lists invalidate these host assumptions. Fix sites: crates/shamir-numa/tests/linux_topology.rs and crates/shamir-numa/src/platform/linux.rs, using eligible topology or deterministic fixtures.

### RV-NUMA-CF3796D2 — `parse_cpulist` expands unbounded ranges -- malformed input can panic/abort the process

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-numa/src/platform/cpulist.rs:29; crates/shamir-numa/src/platform/linux.rs:57; changed by: none
- R0 note (includes the fix site): OBSERVED: Inclusive ranges and repeated tokens expand into an unbudgeted intermediate Vec before sorting/deduplication; production probe callers still read trusted sysfs. INFERRED: Extreme valid ranges can exhaust allocation capacity; no abort threshold was executed. Fix site: crates/shamir-numa/src/platform/cpulist.rs, bounding range and cumulative expansion before allocation.

### RV-NUMA-5CCCBB1A — `probe()` swallows every per-node cpulist I/O error, contradicting its doc and yielding a silently broken topology

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-numa/src/platform/linux.rs:75; crates/shamir-numa/src/platform/detect.rs:25; changed by: none
- R0 note (includes the fix site): OBSERVED: Every per-node read error becomes an empty CPU list, including Syscall errors beyond the documented missing-file exception; detect accepts the topology based only on node count. Fix site: crates/shamir-numa/src/platform/linux.rs, distinguishing missing files from other read failures.

### RV-NUMA-C46D0E41 — No error-path tests for the Linux probe layer; error branches never compiled on the primary gate

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-numa/src/platform/linux.rs:167; crates/shamir-numa/src/platform/linux.rs:183; changed by: none
- R0 note (includes the fix site): OBSERVED: Probe still hardcodes sysfs reads and its Linux tests exercise host-success paths without deterministic read-error classification or sparse-node fixtures. Configured Ubuntu jobs refute the blanket noncompilation allegation, not the missing error-path oracle. Fix site: crates/shamir-numa/src/platform/linux.rs, adding a read seam for deterministic error-path coverage.

### RV-NUMA-43F38611 — Inline `#[cfg(test)] mod tests` in an implementation file

- R0 verdict: `confirmed-open`, risk `low`; evidence: crates/shamir-numa/src/platform/linux.rs:179; crates/shamir-numa/src/platform/mod.rs:6; changed by: none
- R0 note (includes the fix site): OBSERVED: The Linux implementation still contains two inline tests and remains selected under Linux cfg, so the explicit test-layout violation persists. Fix site: crates/shamir-numa/src/platform/linux.rs, moving those tests into the existing external test-module layout while retaining Linux gating.

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
