# P34-01 - shamir-wal: replay bounds, reopen watermark, sink causes, public surface, docs

Source: P3/P4 revalidation of the active findings against master `07bc804c` plus the P2 carry-overs (`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-SPRINT-REPORT-2026-10-30.md`). Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` and `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P4.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), the verified tables in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3-REVALIDATION-2026-10-30.md` / `P4-REVALIDATION-2026-10-30.md`, and the board `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P34-FIX-TASKS.md`.

## Goal

Stop the append path from persisting frames that later hard-fail replay, make the public surface honest (segment_meta and mark_poisoned crate-private, retired-API narratives removed from the module headers), document the reopen watermark and the legacy sink-cause loss, and hoist the remaining test imports. Bounded replay (A25EDE9D) and absolute paths in errors (3F10E8A1) are accepted and not changed.

This is one change set for shamir-wal: 9 active findings, effort units 18 (S=1, M=2, L=4). They are adjacent (same crates, overlapping files), so they are solved together and share tests and review. Decisions marked IMPLEMENT were taken by the design decider (@oh) on 2026-10-30 and are binding; SKIP means the card is accepted as it is; ESCALATE waits for the owner.

## Scope

Crates: `shamir-wal`. Dependents are type-checked with `cargo check` when a public signature changes.

Files outside these crates that the evidence names (edit only these, and only as the cards require):

- `crates/shamir-engine/src/tx/drain/drainer/drain_step.rs` (A25EDE9D)

## Acceptance

- A supported-header, invalid-body append returns a typed error and writes nothing; a following replay of the segment is unaffected (regression named after 4470BEF3).
- segment_meta and WalSegment::mark_poisoned are crate-private (mark_poisoned test-only); every dependent still passes `cargo check`; documentation no longer presents them as public.
- Documentation-only cards (48749F97, C55C2D47, 79167DD7) change comments only. C55C2D47 must not add the underlying error text to the returned error: that would put WAL file paths into client-facing errors.

## Coordination with other tasks

Wave 1; no strict predecessor.
- A25EDE9D names shamir-engine drain_step.rs as evidence but its decision is SKIP: no engine edit in this stage.
- Keep the P0 cancellation/fail-stop ticket semantics and the P1 corruption and typed-I/O policies.

## Decisions

Recorded by the orchestrator from the design decider (@oh) on 2026-10-30. Cards: RV-WAL-A25EDE9D, RV-WAL-48749F97, RV-WAL-C55C2D47-FACET, RV-WAL-0843B96B, RV-WAL-3F10E8A1, RV-WAL-5DFE355F. The full decision paragraph sits in the finding below.

- L3-RV-WAL-A25EDE9D: **SKIP** (no change).
- L3-RV-WAL-48749F97: **IMPLEMENT** (change as decided).
- L3-RV-WAL-C55C2D47-FACET: **IMPLEMENT** (change as decided).
- L4-RV-WAL-0843B96B: **IMPLEMENT** (change as decided).
- L4-RV-WAL-3F10E8A1: **SKIP** (no change).
- L4-RV-WAL-5DFE355F: **IMPLEMENT** (change as decided).

## Findings (ordered by risk, then size)

### L3-RV-WAL-A25EDE9D - replay materializes the entire WAL as decoded entries in one Vec

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted backlog-dependent decoded-memory debt, unchanged since P2. SegmentSet::replay gathers every segment into one Vec (segment_set.rs:562-577), and its consumers (startup version sort, interner-before-finalization ordering, cold-journal certification, live window reseed; drain_step.rs:160-172) need the full ordered set. Bounded consumption is a separately scheduled cross-crate bounded-recovery project. Do not claim end-to-end bounded recovery.
- Evidence at `07bc804c`: crates/shamir-wal/src/segment_set/segment_set.rs:562-577; crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:160-172
- Recipe: Introduce bounded replay consumption and migrate recovery and synchronization consumers while preserving ordering.
- Verifier note: Decoded backlog accumulation remains reachable through recovery and synchronization; bounded consumption must preserve ordering.

### L3-RV-WAL-4470BEF3 - Append path accepts payloads that produce well-formed frames which then hard-fail replay

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`.
- Evidence at `07bc804c`: crates/shamir-wal/src/segment/wal_segment.rs:293-305,333-343,367-374,574,798
- Recipe: Reject malformed encoded bodies before mutation and add a supported-header invalid-body append/replay regression.
- Verifier note: Supported-header invalid bodies are pinned then persisted and fail replay; reject decode failure before mutation and add a no-write regression.

### L3-RV-WAL-48749F97 - Reopen resets the active commit maximum rather than restoring the documented cross-segment watermark

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/-`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Documentation only. On WalSegment::open (wal_segment.rs:195-204), WalSegment::max_committed (around wal_segment.rs:427) and the max_committed field doc (wal_segment.rs:155-160), state that a bare open of an existing non-empty segment starts with max_committed = 0 and the retention pin clear. The values cover only frames appended through this handle plus anything restored by SegmentSet::open, which replays the retained active frames and calls restore_retention (segment_set.rs:242-252). Truncation eligibility is decided only by SegmentSet. Callers who open a segment directly must not use the handle-local maximum as a durable cross-segment watermark. Point SegmentSet::max_committed's doc (segment_set.rs:721-723) at that distinction. Do not make open scan the file: that would replay the active segment twice at startup and add I/O to every rotation. Existing SegmentSet reopen tests (segment_set_tests.rs:274, retention tests :51/:96/:137) already pin the managed path; no new behavioural test needed. Out of scope: next_seq persistence (RV-WAL-C37B110A).
- Evidence at `07bc804c`: crates/shamir-wal/src/segment/wal_segment.rs:204-243,428-429; crates/shamir-wal/src/segment_set/segment_set.rs:249-252
- Verifier note: First pass missed public WalSegment::open resetting the retained maximum; SegmentSet restoration closes only the managed reopen path.

### L3-RV-WAL-C55C2D47-FACET - Group-commit callers lose the underlying sink cause

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Documentation only; closes the card as a documented limitation. On WalGroupCommit::append and append_many (wal_group_commit.rs:180-191, 291-312) and on From<WalAppendError> for DbError (wal_ticket.rs:45-48), document that the legacy entry points return DbError::Storage with fixed, path-free outcome text. This is deliberate: the classification and match shape are preserved, and WAL file paths do not reach client-visible errors. Callers that need the underlying sink cause use append_ticketed and WalTicket::failure_cause(), which keeps the original Arc<DbError> (wal_ticket.rs:133-141). Do not add the cause's Display to the returned string (that would leak paths and contradict L4-RV-WAL-3F10E8A1), do not switch to DbError::Io, and do not add a new variant. No behavioural test. Out of scope: migrating begin_grouped/begin_grouped_many to the ticketed API; the engine's commit_policy NotWritten conversion.
- Evidence at `07bc804c`: crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:190-191,312,348-355,389-402; crates/shamir-wal/src/segment/wal_ticket.rs:45-48,133-134
- Recipe: Preserve a source-bearing cause in legacy completion errors without changing physical-outcome classification.
- Verifier note: Tickets retain sink causes; legacy completion conversion still discards them into fixed Storage text.

### L3-RV-WAL-79167DD7 - Retired F5c KV-marker wire protocol still exported as public API, with docs describing it as live

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `07bc804c`: crates/shamir-wal/src/entry/wal_entry_v2.rs:3-7; crates/shamir-wal/src/segment/wal_segment.rs:3-7
- Recipe: Remove obsolete live-KV WalEntry and WalManager narratives from both module headers.
- Verifier note: Retired exports are absent, but two module headers still describe obsolete live APIs.

### L4-RV-WAL-0843B96B - pub mod segment_meta exports nothing public

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Change 'pub use entry::segment_meta;' to 'pub(crate) use entry::segment_meta;' in crates/shamir-wal/src/lib.rs:70. Keep every crate::segment_meta::... path inside the crate working. Fix the module and crate docs so they no longer present segment_meta as public API, and keep intra-doc links to its private items valid under the narrowed visibility. This overturns the P2 SKIP on new evidence: shamir-wal is publish = false (Cargo.toml:4), no workspace crate imports shamir_wal::segment_meta (server hits are comments only), and every item in the module is already pub(crate). Verify with cargo check -p shamir-wal --all-targets plus cargo check of shamir-tx, shamir-engine, shamir-db and shamir-server. No behaviour change and no test. Out of scope: sidecar format or correctness changes.
- Evidence at `07bc804c`: crates/shamir-wal/src/lib.rs:70; crates/shamir-wal/src/entry/segment_meta.rs:122-142
- Recipe: Restrict the internal segment_meta namespace and align documentation references with its visibility.
- Verifier note: Empty public namespace and private documentation links remain; narrowing a public module needs API approval.

### L4-RV-WAL-3F10E8A1 - Error strings embed absolute filesystem paths

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/decision`; **OWNER DECISION**.
- **Decision: SKIP** (@oh design decider, 2026-10-30; compat `n/a`, risk `low`). Accepted operator-diagnostic path exposure at the embedded library boundary, as decided in P2. Typed I/O errors keep the path plus an io::Error source (wal_segment.rs:44-60,204-232) per the P1 typed-I/O policy. Append errors are path-free (wal_ticket.rs:33-48). No WAL-to-client disclosure route has been shown. Sanitize only at a demonstrated external transport boundary. The C55C2D47 decision keeps legacy append errors path-free.
- Evidence at `07bc804c`: crates/shamir-wal/src/segment/wal_segment.rs:44-46; crates/shamir-wal/src/segment/wal_segment.rs:204-218
- Recipe: Define diagnostic disclosure policy and redact public path rendering while retaining protected operator context.
- Verifier note: Returned errors render supplied paths unchanged; a remote disclosure route is not established.

### L4-RV-WAL-5DFE355F - WalSegment::mark_poisoned is an un-gated public test hook

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`; **OWNER DECISION**.
- **Decision: IMPLEMENT** (@oh design decider, 2026-10-30; compat `compatible`, risk `low`). Restrict WalSegment::mark_poisoned (wal_segment.rs:493-498) to #[cfg(test)] pub(crate) and change its doc to a crate-test fault hook. Production code keeps poisoning through the internal atomic on write/fsync failure, unchanged. This overturns the P2 SKIP on new evidence: every caller is in shamir-wal's src/tests tree, no bench, integration test or other crate calls it, and the crate is publish = false (Cargo.toml:4). Keep is_poisoned public. Verify with cargo check -p shamir-wal --all-targets and cargo check of dependents. Existing poison tests are the regression coverage; no new test. Out of scope: any change to the poison or rotation semantics.
- Evidence at `07bc804c`: crates/shamir-wal/src/segment/wal_segment.rs:493-498
- Recipe: Restrict mark_poisoned to crate-local tests or an approved test-util feature.
- Verifier note: Public ungated poison hook remains callable by a segment holder; restricting it changes public API.

### L4-RV-WAL-C66039FC - Mid-function use statements in tests (imports-at-top rule)

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/code`.
- Evidence at `07bc804c`: crates/shamir-wal/src/tests/wal_group_commit_tests.rs:5; crates/shamir-wal/src/tests/wal_group_commit/recoverability_tests.rs:418-419
- Recipe: Hoist remaining Future, Ordering and Poll imports into the recoverability test header.
- Verifier note: Original Duration sites are repaired; ordinary Future, Ordering and Poll body imports still need hoisting.

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

`cargo fmt -p shamir-wal -- --check`; `cargo clippy -p shamir-wal --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(decision) / SKIPPED(owner) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
