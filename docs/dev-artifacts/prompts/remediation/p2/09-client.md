# P2-09 - shamir-client: malformed rows and response frames, interner traversal, resume options, cleanup

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Turn malformed id-keyed rows and undecodable response frames into errors instead of empty records and hung waiters, zeroize the resume ticket, and cut interner traversal and encode copies.

This is one change set for shamir-client: 16 findings, effort units 33 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. RV-CLIENT-A911BA9E shares the RecordView boundary with the engine; do not change the engine side here.

## Scope

Crates: `shamir-client`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- A malformed id-keyed row yields a typed error; an undecodable response frame fails its waiters promptly instead of hanging them, and is logged above debug level.
- The resume ticket and its wire copy are Zeroizing (owner answer); interner traversal and encode copies are reduced (allocation test or measured).

## Coordination with other tasks

Suggested wave 1; no strict predecessor.
- api.rs and transport.rs are also edited by P2-18 (79910BA5, 8999F6B7): P2-18 starts after P2-09. messagepack.rs (9DEFDE43) is also edited by P2-12.

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 8997B366, 0157DABC.

- RV-CLIENT-8997B366: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Preserve ResumeOptions.ticket: Vec<u8> for existing struct-literal callers and add a secure-options alternative owning Zeroizing<Vec<u8>> with a resume entry point sharing one protected handshake implementation. In the legacy async entry point, transfer ticket ownership into Zeroizing before the first internal await; explicitly document its raw-options exposure before polling and recommend secure options for retained or cancellable options. Protect wire copies, serialization scratch including partial-error paths, and received rotated-ticket buffers on success, failure and cancellation. Preserve exact wire bytes and identity-proof-before-ticket-disclosure ordering. CI must observe erasure through safe test seams across success, early failure and cancellation; coordinate native-owned buffer guarantees with P2-03. Evidence: crates/shamir-client/src/client/options.rs:100-109; crates/shamir-client/src/client/connect.rs:611-633,670-677,709-735.
- RV-CLIENT-0157DABC: **IMPLEMENT** (rush reviewer (read-only), accepted by orchestrator, 2026-10-29). Keep ClientError::RequestIdMismatch with its existing fields and display text, documenting it as reserved and not currently emitted. Do not emit it for legitimate late responses or redesign multiplexing in this documentation card; keep request-ID exhaustion and malformed-envelope remediation separate. No new behavioral test is needed. Evidence: crates/shamir-client/src/error.rs:95-97; crates/shamir-client/src/client/transport.rs:623-632.

## Findings (ordered by risk, then size)

### RV-CLIENT-A911BA9E - Malformed id-keyed rows can be returned successfully as empty or partial records

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `L/code`.
- Evidence at `7f04e096`: crates/shamir-types/src/record_view/lens/view.rs:41-45,268-304; crates/shamir-types/src/codecs/interned/codec.rs:197-203; crates/shamir-client/src/interner_cache/ops.rs:691-715
- Recipe: Use fallible complete traversal with nested-map, cardinality and consumption validation while preserving unknown-ID refresh.
- Note: Allocation caps do not prevent successful empty or partial records.
- Second pass: Truncated traversal still becomes successful empty or partial Direct records; allocation caps and refresh do not validate completeness.

### RV-CLIENT-56E4E99D - Undecodable response frames are dropped at `debug` level; default-unbounded waiters hang

- Verdict: `STILL_OPEN`; risk `medium`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/transport.rs:467-475,599-605
- Recipe: Treat invalid envelopes as terminal protocol failures and drain waiters; test a peer that remains connected.
- Note: Raised from low because malformed responses can cause indefinite request-liveness loss.
- Second pass: Malformed envelopes are skipped without shutdown; an open connection leaves timeout-free requests unresolved.

### RV-CLIENT-8997B366 - `ResumeOptions::ticket` and its wire copy are not `Zeroizing`

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-client/src/client/options.rs:106; crates/shamir-client/src/client/connect.rs:677,709-715
- Recipe: Protect resume ticket ownership, serialized payloads and framing scratch on all exits while retaining wire bytes.
- Note: Identity-proof ordering was fixed separately; ordinary credential allocations remain.

### RV-CLIENT-267BD36E - `get_ddl_op_status`: comment/doc/behavior three-way contradiction, dead `DbResponse::Error` arm, zero coverage

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/api.rs:425-439,602-603; crates/shamir-client/src/visibility/error_mapping.rs:21-30
- Recipe: Remove the unreachable special error arm, document current behavior and test unknown versus unsupported status requests.
- Note: Roundtrip maps errors before the method can match them; no method coverage was found.

### RV-CLIENT-3D598780 - Ambient interner-epoch advertisement is all-or-nothing per batch

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/api.rs:256-262,637-638
- Recipe: For v2 peers fill missing per-repository epoch entries without overwriting explicit values and test mixed maps.
- Note: The added version gate retains the all-or-nothing map gate.

### RV-CLIENT-5E5B78A0 - The claimed cursor mutex-skip optimization still locks on every yielded record

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/cursor_stream.rs:237-252
- Recipe: Publish the immutable cursor ID through a one-time cell and remove per-record locking while preserving cleanup.
- Note: The is_none check skips assignment, not mutex acquisition.

### RV-CLIENT-9DEFDE43 - `collect_map_keys` skips `Value::Set` (asymmetric with `qv_has_fn_marker`)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/interner_cache/ops.rs:480-491,511; crates/shamir-types/src/codecs/interned/messagepack.rs:974-978
- Recipe: Traverse Value::Set during key collection and add a cold-cache set-of-maps insertion regression.
- Note: Pre-touch and serialization still traverse different value shapes.

### RV-CLIENT-C93ADFED - Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-client/src/lib.rs:10-18; crates/shamir-client/src/client/options.rs:38,52; crates/shamir-client/Cargo.toml:49-52
- Recipe: Complete the ConnectOptions example and add an equivalent CI compile-checked example without enabling project-wide doctests.
- Note: The example still omits connect_timeout and request_timeout.
- Second pass: The illustration includes handshake_timeout but still omits connect_timeout and request_timeout; add CI compile coverage without changing doctest policy.

### RV-CLIENT-EB08BA4C - `collect_field_names` clones every field-name key of every record before dedup

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/interner_cache/ops.rs:390-393,483
- Recipe: Collect borrowed unique names per repository and allocate only when ownership is needed.
- Note: Every key occurrence is still cloned before deduplication.
- Second pass: Batch preparation still clones each key occurrence before deduplication and cache touching; deduplicate borrowed names first.

### RV-CLIENT-462B7413 - `encode_record_idmsgpack` pays an avoidable full-record copy per INSERT (`Bytes::to_vec`)

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/interner_cache/ops.rs:643-645
- Recipe: Replace Bytes::to_vec with consuming Bytes-to-Vec conversion while preserving exact wire bytes.
- Note: The explicit full-record copy remains.

### RV-CLIENT-153A118E - Push frames pay up to two failed envelope parses before `PushEnvelope` decode

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/transport.rs:94-107,540-544
- Recipe: Dispatch once using existing envelope discriminators without changing wire format and retain malformed-input coverage.
- Note: Push frames still follow failed response and error parses; overhead is unmeasured.

### RV-CLIENT-17C0D288 - Wrapping request IDs can replace an outstanding sender or miscorrelate a late response; debug panic is refuted

- Verdict: `CHANGED`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/api.rs:78-80,488,522
- Recipe: Prevent request-ID reuse within a connection and close or reconnect on exhaustion; test forced wrap and late responses.
- Note: scc insertion no longer replaces occupied senders, but ignored collisions and late-response reuse remain.
- Second pass: Occupied insertion no longer replaces senders, but ignored collisions, ID-only cleanup and late-response reuse remain.

### RV-CLIENT-66E9A5FC - Perf claims are unmeasured: no benches, no allocation-behaviour tests

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-client/src/client/api.rs:480-487; crates/shamir-client/Cargo.toml:42-55
- Recipe: Add allocation-count oracles and fixed-iteration client benchmarks with results recorded from CI.
- Note: Scratch wiping tests establish erasure, not allocation performance.

### RV-CLIENT-F1276114 - `connect_timeout_fires` depends on 10.255.255.1:9 being a silent black hole

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-client/src/tests/client/timeout_tests.rs:27-30,41-62,71-82
- Recipe: Inject a controlled pending connector into the establishment-timeout path instead of depending on private-address routing.
- Note: Both tests still require an environment-dependent connection attempt to remain pending.

### RV-CLIENT-0157DABC - Dead public API: `ClientError::RequestIdMismatch` is never constructed anywhere in the workspace

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-client/src/error.rs:97; crates/shamir-client/src/client/transport.rs:630-632
- Recipe: Document RequestIdMismatch as reserved and not emitted, or remove it under an explicit API compatibility decision.
- Note: Source searches found only the declaration.

### RV-CLIENT-E39C43D4 - Mid-body `use` statements in three `src/tests/` files

- Verdict: `PARTIAL`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-client/src/tests/receipts/batch_has_refs_tests.rs:19-20; crates/shamir-client/src/tests/wire/wire_version_tests.rs:8-9
- Recipe: Hoist the remaining ReadQuery import from read_op into the file header.
- Note: Two cited files are cleaned up; one body-local import remains.
- Second pass: ReadQuery remains imported inside read_op; hoisting is sufficient.

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

`cargo fmt -p shamir-client -- --check`; `cargo clippy -p shamir-client --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
