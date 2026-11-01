# P34-07b - shamir-index: bounded-memory vector snapshot dump and load (RV-INDEX-4DC29EFB)

Source: the P2 revalidation card carried over in `docs/dev-artifacts/prompts/remediation/p34/07-index.md` ("Carried over from the P2 sprint", RV-INDEX-4DC29EFB, DEFERRED by the P2 owner decision) and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`. Read first: `CLAUDE.md`, `crates/shamir-index/src/vector/snapshot/mod.rs` (layout and pillars), `codec.rs` (dump), `load.rs` (load, `flip_generation`, `publish_manifest_without_prune`), `manifest.rs`.

## Goal

Dump and load of a vector index snapshot hold the whole graph in RAM several times today: `dump_snapshot_with_gen` reads both `hnsw_rs` dump files (and the u8 pair of a fitted quantized adapter) completely with `std::fs::read`, slices them, accumulates EVERY chunk in one `Vec<KvOp>` and applies one `transact`; `load_snapshot` reassembles whole sections in memory before writing temp files, and the sidecar maps are cloned. Make the peak extra memory of both directions O(chunk size x batch), independent of graph size, while keeping the on-disk format and the publication coherence exactly as they are.

## Binding behaviour

1. The persisted format does not change: keys, chunk header and per-chunk crc32, the cross-section crc32 (same byte order: graph, data, qgraph, qdata), sidecar, manifest, `SNAPSHOT_FORMAT_VERSION`. A snapshot written by the old code loads, and a new dump is byte-identical to the old dump of the same adapter.
2. Publication coherence and the repaired generation/prune ordering stay as they are: the chunks and the sidecar of generation N are written first (in bounded batches), the manifest that points at N is the commit point and is written LAST, the previous generation is pruned only after the manifest flip (`flip_generation`, `publish_manifest_without_prune`). Before changing anything read how a crash between the chunk writes and the manifest write is handled today (unreferenced chunks of an unpublished generation) and say in the design note whether an orphan sweep exists; if the streaming write widens that window add the sweep, otherwise leave it.
3. Dump: read each dump file in `CHUNK_SIZE` pieces inside `spawn_blocking`, compute the per-chunk crc and update the cross-section hasher incrementally, emit chunk records in bounded batches; the temporary dump directory is removed at the end. Load: append chunks to the temporary section file one at a time (per-chunk crc checked as before, cross-section crc incrementally), then give the paths to `hnsw_rs` as before. No whole-section `Vec`.
4. Sidecar ownership: serialize and write the sidecar without cloning the maps (the card recipe: "move sidecar ownership"); evidence lines of the card: codec.rs 368-369, 440-441 and load.rs 120-124, 216-220 at the P2 revalidation commit, re-locate by symbol.
5. Error semantics are unchanged: `SnapshotError::{Io, Corrupt, VersionMismatch, ...}` for the same inputs, a missing or short chunk is detected exactly as before.

## Deliverables

- A short design note committed with the change at `docs/dev-artifacts/audits/2026-11-01-p34-07b-streaming-snapshot/DESIGN.md`: the current memory profile with file:line, the target profile, the write/commit/prune ordering and crash analysis, the test plan. Write it FIRST and implement what it says.
- Tests (they must fail on the old code, red first in CI): (a) byte parity of the new dump against a reference built by the old straightforward algorithm kept inside the test; (b) a memory oracle that measures the PEAK number of outstanding bytes (allocated minus freed, high-water mark) on the dumping thread with a counting allocator (reuse the thread-bound counting allocator pattern that already exists in the shamir-index test support, or in `crates/shamir-types/src/types/tests/allocation_counter.rs`; one `#[global_allocator]` per test binary, check what the index lib test binary already has) over a graph large enough that the old implementation exceeds the bound by a wide margin, for dump and for load; (c) crash order: a failure injected after the chunks and before the manifest leaves the previous generation loadable, a failure after the manifest does not lose the new generation; (d) a flipped byte in a chunk and a missing chunk fail exactly as before.
- The before/after memory numbers come from the CI run of (b); do not claim measured numbers you did not observe.

## Scope

Crate `shamir-index`, files `crates/shamir-index/src/vector/snapshot/*` and its tests (new files and folders allowed for the streaming helpers and the tests; respect the layout limits), plus the design note. Public signatures of `dump_snapshot`, `dump_snapshot_with_gen`, `load_snapshot` and friends do not change.

## Coordination with other tasks

The other identity-v2 stages are merged. Nothing else edits the snapshot module.

## Decisions

None recorded beyond the card recipe: "Stream bounded snapshot chunks and move sidecar ownership while preserving generation coherence." The P2 owner deferral is lifted by the P34 plan; design first.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-index. Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
