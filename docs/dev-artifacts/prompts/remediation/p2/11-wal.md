# P2-11 - shamir-wal: leader starvation, retention pin, bounded recovery, rollback schedule, cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Prevent leader starvation in group commit, keep the zero-version retention pin across mixed batches and segments, bound recovery work, stop the destructive rollback schedule, and correct benchmark claims.

This is one change set for shamir-wal: 15 findings, effort units 28 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Keep the P0 cancellation/fail-stop ticket semantics and the P1 corruption and typed-I/O policies.

## Scope

Crates: `shamir-wal`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- Sustained arrivals cannot keep the elected leader's append future from returning (test); P0 cancellation/fail-stop ticket semantics are preserved.
- Zero-version retention pins survive mixed zero/positive batches and segments (test); recovery is bounded; the rollback schedule is no longer destructive; benchmark claims match what is covered.

## Coordination with other tasks

Suggested wave 1; no strict predecessor.
- Keeps P0 cancellation/fail-stop ticket semantics and the P1 corruption/typed-I/O policies.

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: A25EDE9D, DD492F5C, 0843B96B, 3F10E8A1, 5DFE355F.

- RV-WAL-A25EDE9D: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Record accepted backlog-dependent decoded-memory debt and schedule a cross-crate bounded-recovery project. A cursor alone cannot preserve startup commit-version sorting, interner-before-finalization ordering, full-entry cold journal certification and live window reseeding with bounded memory. Those changes exceed this WAL-only brief. Continue CC3131A9 incremental frame parsing, but do not claim end-to-end bounded recovery.
- RV-WAL-DD492F5C: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Document that data and sibling WAL directories, sidecars, parent-directory replacement and restored copies are writable only by the database identity and trusted operators/tooling. Specify restrictive Unix permissions and Windows ACLs. Explain that CRC detects corruption but does not authenticate writers and that TLS/volume encryption do not provide WAL authenticity. Update WAL crate docs and the security data-protection guide; authorize that documentation scope extension. No format/runtime/dependency changes or behavioral tests are required.
- RV-WAL-0843B96B: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Record accepted public-namespace debt. Keep segment_meta publicly importable: removing an empty public module still breaks downstream imports without runtime benefit. Do not widen private helpers for documentation. Reconsider only during explicitly compatibility-breaking API cleanup, independently of sidecar correctness repairs.
- RV-WAL-3F10E8A1: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Record accepted operator-diagnostic path exposure at the embedded library boundary. Preserve WAL path context and typed I/O causes; no specific remote WAL disclosure route has been established, and ticketed append display errors are already path-free. Sanitize only at a demonstrated external transport boundary rather than weakening WAL diagnostics or the P1 typed-I/O policy.
- RV-WAL-5DFE355F: **SKIP** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Record accepted legacy public test-hook surface, not a supported administrative contract. Keep mark_poisoned unchanged: restricting it breaks public callers, repository-only consumer review cannot exclude external use and misuse requires an in-process segment holder. Avoid API churn for this nit while fixing actual append/rollback concurrency behavior.

## Findings (ordered by risk, then size)

### RV-WAL-A25EDE9D - replay materializes the entire WAL as decoded entries in one Vec

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment_set/segment_set.rs:518; crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:162
- Recipe: Introduce bounded incremental replay and migrate startup and live gap recovery consumers.
- Note: Decoded retained WAL still accumulates in one vector.

### RV-WAL-59E3DBA1 - Leader tenure is unbounded under sustained arrival — the leader's own append future cannot resolve

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:218,226,300
- Recipe: Bound leader tenure with safe follower re-election and sustained-arrival completion coverage.
- Note: Leader checks its ticket only after drain-until-empty returns.

### RV-WAL-67475C86 - truncate_below partial failure discards progress and strands claimed files until restart

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment_set/segment_set.rs:548,589
- Recipe: Retain retryable deletion claims and reconcile per-file outcomes with the sealed registry.
- Note: Hard unlink failure loses reported progress and strands remaining claims.

### RV-WAL-79483DA7 - WalSegment::append_batch rollback and bytes_written accounting are unsafe under concurrent callers, while the bench claims correctness coverage of exactly that shape

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:299,318,345,373; crates/shamir-wal/benches/segment_set_lock.rs:112
- Recipe: Serialize offset capture, write, rollback and accounting; add concurrent failure and replay coverage.
- Note: Direct concurrent callers retain destructive rollback schedule outside production single-leader model.

### RV-WAL-8021EB80 - MemSink batch-max tagging and engine truncation coverage

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_sink.rs:308,417; crates/shamir-wal/src/segment_set/segment_set.rs:530,549
- Recipe: Preserve unversioned-record pins across batching, sealing and reopen with mixed-version Mem/File tests.
- Note: Positive-version behavior exists; mixed zero/positive contents still lose their pin.
- Second pass: Batch and segment maxima still discard mixed unversioned-record pin information; cover Mem/File sealing and reopen.

### RV-WAL-CC3131A9 - Recovery reads whole segment files without a size cap

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:506,726; crates/shamir-wal/src/entry/segment_meta.rs:135
- Recipe: Bound sidecar reads and parse segment frames incrementally with allocation limits.
- Note: Whole-file buffering remains in repair, replay and sidecar reading.

### RV-WAL-DD492F5C - Unauthenticated WAL replay is an injection surface; the data directory is trusted assumption is implicit and undocumented

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:313,759; crates/shamir-wal/src/entry/segment_meta.rs:153; docs/guide-docs/security/data-protection.md:27
- Recipe: Document trusted directory writers, required ACLs and the absence of WAL authenticity guarantees.
- Note: CRC remains unkeyed; encryption guidance does not define a writer trust boundary.

### RV-WAL-0843B96B - pub mod segment_meta exports nothing public

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wal/src/lib.rs:53; crates/shamir-wal/src/entry/segment_meta.rs:11,125,143
- Recipe: Remove the empty public segment_meta namespace and align documentation visibility.
- Note: Public module exposes only private or crate-private implementation items.

### RV-WAL-3735F642 - MemSink holds a std::sync::Mutex across O(N) CPU work inside async fns

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_sink.rs:391,415,438
- Recipe: Use immutable snapshots and move corpus processing outside runtime-worker critical sections.
- Note: Eligibility probe is atomic; replay and truncation still perform O(N) work under the mutex.
- Second pass: Atomic eligibility probe is improved; replay decoding and truncation still perform corpus-sized work under a synchronous mutex.

### RV-WAL-3F10E8A1 - Error strings embed absolute filesystem paths

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:212,709
- Recipe: Define operator diagnostic versus public-error policy and redact externally surfaced paths.
- Note: Typed I/O errors still embed paths; remote disclosure route is not established.

### RV-WAL-5DFE355F - WalSegment::mark_poisoned is an un-gated public test hook

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:459,462
- Recipe: Gate or restrict mark_poisoned after consumer review, or explicitly support it as an administrative API.
- Note: Test-oriented poison hook remains ungated and public.
- Second pass: Test-described poison setter is ungated and public; restricting that public API requires consumer and owner review.

### RV-WAL-0CAFAF79 - Vestigial, unexplained #[allow(dead_code)] on a public type

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:147,186
- Recipe: Remove broad dead_code allowances and retain only specifically justified exceptions if necessary.
- Note: Unexplained allowances remain on both public type and impl.
- Second pass: Both broad dead_code allowances remain unexplained; remove or narrowly justify them.

### RV-WAL-C37B110A - Per-segment seq counter restarts at 0 on reopen — returned last_seq can collide with pre-existing frames

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-wal/src/segment/wal_segment.rs:234,251
- Recipe: Explicitly define returned sequence numbers as nonpersistent per-handle ordinals.
- Note: Downgraded: scope documentation debt; commit-watermark restoration does not restore sequence counter.

### RV-WAL-C66039FC - Mid-function use statements in tests (imports-at-top rule)

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-wal/src/tests/wal_group_commit_tests.rs:225,256,273,442
- Recipe: Hoist Duration imports into the test file header.
- Note: Named function-local imports remain.

### RV-WAL-FE36ADFC - WalOpV2::IndexPut/IndexDel serialize idx_id as a constant 0 with semantics deferred

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-wal/src/entry/wal_entry_v2.rs:80; crates/shamir-engine/src/tx/recovery/v2.rs:183
- Recipe: Document the existing reserved and ignored idx_id behavior without changing serialization.
- Note: Downgraded: current replay uses the complete supplied key; remaining issue is stale documentation.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- Cards marked `OWNER DECISION` are implemented only if the "Decisions" section below contains the answer; otherwise skip them and say so in the final report.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p shamir-wal -- --check`; `cargo clippy -p shamir-wal --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
