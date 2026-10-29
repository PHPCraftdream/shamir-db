# P2-05 - shamir-index (vector): snapshot compat/integrity, SQ8 alignment, replay/streaming, brute-force lifecycle

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Fix the 13 findings below in one coherent change set for shamir-index (effort units 26: S=1, M=2, L=4). They are adjacent: same crate and overlapping files, so they are solved together and share tests and review. Files under src/vector/**. Keep the P1 SQ8-intent carrier and the repaired generation/prune ordering.

## Scope

Crates: `shamir-index`. Dependents are type-checked with `cargo check` when a public signature changes.

## Decisions

Filled by the orchestrator before launch (owner answers). Empty means: skip the cards marked OWNER DECISION: 4DC29EFB, 9E0EC9A5, 8533E947.

- (none yet)

## Findings (ordered by risk, then size)

### RV-INDEX-4DC29EFB - Vector snapshot dump/load fully materializes the graph and sidecar in RAM

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/snapshot/codec.rs:368-369,440-441; crates/shamir-index/src/vector/snapshot/load.rs:120-124,216-220
- Recipe: Stream bounded snapshot chunks and move sidecar ownership while preserving generation coherence.
- Note: Whole-index intermediates remain; RSS multiplier unmeasured.
- Second pass: Whole-index intermediates remain on dump and restore; bounded streaming must preserve generation publication coherence.

### RV-INDEX-40BCFF22 - Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/snapshot/load.rs:46,94; crates/shamir-index/src/vector/tests/quant/quantization_snapshot_tests.rs:385-396
- Recipe: Add historical v1 disk mirrors and version dispatch, tested against genuine historical bytes.
- Note: Current-layout relabeling does not test historical compatibility.
- Second pass: Historical layouts differ; direct current-type decoding and relabelled current fixtures do not implement promised v1 compatibility.

### RV-INDEX-9E0EC9A5 - Snapshot load path can panic on corrupt-but-decodable persisted data; the sidecar carries no checksum

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/snapshot/load.rs:150-160,183-188,303; crates/shamir-index/src/vector/quant/quant_meta.rs:73-74
- Recipe: Validate persisted parameters fallibly and add versioned sidecar integrity coverage with corruption fixtures.
- Note: Graph-parser panic containment does not protect synchronous quantizer assertions.
- Second pass: Validly decoded malformed quantizer dimensions reach synchronous assertions; section CRC excludes sidecar parameters.

### RV-INDEX-C3FBD7AC - AArch64 safe SQ8 byte-slice callers can reach an alignment-requiring u32 lane load

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/quant/sq8.rs:222; crates/shamir-index/src/vector/simd/quantized.rs:454,623,772; rust-toolchain.toml:15
- Recipe: Replace typed NEON lane loads with alignment-safe loads and add offset-slice safety coverage in CI.
- Note: Conditional safe-API soundness defect; no deployed crash observed.
- Second pass: Length-only safe byte-slice callers still reach typed NEON lane loads; conditional soundness exposure, not an observed crash.

### RV-INDEX-FDC0161A - Vector delta-log replay on restart applies ops one at a time

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/snapshot/load.rs:560-573; crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:108-116
- Recipe: Introduce bounded ordered replay batches that amortize blocking dispatch without changing repeated-rid ordering.
- Note: Fail-closed recovery did not batch operation application.
- Second pass: Recovery still awaits individual operations and f32 blocking insertions; ordered batching must preserve repeated-rid semantics.

### RV-INDEX-0256E6B5 - `BruteForceAdapter::search` runs an O(N·dim) exact scan inline on the async runtime (pillar 2: CPU-bound → `spawn_blocking`)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/quant/brute_force.rs:262-330
- Recipe: Move owned exact-search work into spawn_blocking and verify runtime progress in CI.
- Note: Committed and staged scans plus sorting remain inline in the async method.

### RV-INDEX-55A6887F - `BruteForceAdapter` deep-clones the whole snapshot on every drained write batch

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/quant/brute_force.rs:84-92,175-180
- Recipe: Publish structurally shared immutable chunks instead of deep-cloning the whole snapshot.
- Note: Queue coalescing reduces publication count but not per-publication copying.

### RV-INDEX-6BE64AFC - `HnswAdapter` f32 small-index search clones every vector per query

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:571-595
- Recipe: Score borrowed or shared vector storage without nested concurrent-map lock acquisition.
- Note: Small f32 search still clones vectors before filtering.

### RV-INDEX-8533E947 - Snapshot load joins persisted `basename`/`qbasename` into temp file paths unsanitized; manifest/sidecar carry no integrity check

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/snapshot/load.rs:204,239-247; crates/shamir-index/src/vector/snapshot/quantized_load.rs:15-22; crates/shamir-index/src/model/meta/meta_envelope.rs:14-18,62-78
- Recipe: Validate single-component basenames and introduce versioned metadata integrity with a legacy-read policy.
- Note: Graph CRCs do not sanitize paths or protect all metadata; no remote write route established.

### RV-INDEX-B23D8FE7 - Inline `#[cfg(test)] mod tests` inside an implementation file

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/quant/quant_meta.rs:83-89; crates/shamir-index/src/vector/mod.rs:5,13
- Recipe: Move QuantMeta tests into the appropriate tests directory and preserve CI discovery.
- Note: Reachable implementation file still contains inline cfg(test) module.

### RV-INDEX-B424A8D5 - Actor `shutdown` discards the join result (panic payload), and `BruteForceAdapter::shutdown` adds a lock-poisoning expect

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-index/src/model/runtime/actor.rs:100-103; crates/shamir-index/src/vector/quant/brute_force.rs:129-133
- Recipe: Report join failures without changing signatures and avoid a poison panic in teardown-slot extraction.
- Note: Join errors are discarded; a production poison witness is not established.

### RV-INDEX-54363D3B - `BruteForceAdapter::join: std::sync::Mutex<Option<JoinHandle>>` lacks the inline contention-model comment CLAUDE.md requires per instance

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-index/src/vector/quant/brute_force.rs:59-65,131-133
- Recipe: Add the required inline teardown-only contention-model explanation beside the join mutex.
- Note: Guard ends before await; required per-instance explanation is absent.

### RV-INDEX-ABD60B4B - Stale crate-root invariant: "NO `std::sync::Mutex` / `RwLock` / `parking_lot`"

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-index/src/lib.rs:8-11; crates/shamir-index/src/vector/quant/brute_force.rs:64
- Recipe: Document the scoped hot-path locking policy and sanctioned lifecycle/DDL exceptions.
- Note: Downgraded from low: inaccurate documentation, not a demonstrated hot-path lock defect.

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

`cargo fmt -p shamir-index -- --check`; `cargo clippy -p shamir-index --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
