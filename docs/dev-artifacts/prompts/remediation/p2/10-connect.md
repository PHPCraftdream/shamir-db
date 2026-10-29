# P2-10 - shamir-connect: auth oracles and vectors, ticket bounds, session/password-change invariants, fallible entropy

Source: P2 revalidation of the active findings against master `7f04e096`. Read first: `CLAUDE.md`, the card sections in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (find a card with `grep -n "^### <ID>" <file>`; its file:line references are stale, locate code by symbol), and the verified table in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md`.

## Goal

Correct the authentication-message oracles and add byte-exact vectors without changing wire bytes, bound and version the ticket format, keep session identity and cached HMAC keys coherent, and make entropy/clock failures fallible.

This is one change set for shamir-connect: 26 findings, effort units 45 (S=1, M=2, L=4). They are adjacent (same crate, overlapping files), so they are solved together and share tests and review. Public accepted-input and API changes need owner answers; downstream shamir-server/shamir-client migrations are part of the PR when an API becomes fallible.

## Scope

Crates: `shamir-connect`. Dependents are type-checked with `cargo check` when a public signature changes.

## Acceptance

- The auth-message oracles use the real 13-byte tag and 148-byte message; wire bytes do not change; byte-exact vectors cover change-password and bootstrap payloads.
- The ticket version is validated on issuance and covered by a version-3 rejection test; ciphertext length cannot truncate silently.
- Session id, cached HMAC key and change-password nonce are coherent through the public lifecycle; entropy and clock failures are fallible, with downstream migration (owner answers).

## Coordination with other tasks

Suggested wave 3; starts after P2-09, P2-18 are merged.
- Starts after P2-09 and P2-18 (handshake.rs, domain_tags.rs, downstream client/server migration when an API becomes fallible).

## Decisions

Answers for the cards marked OWNER DECISION, recorded by the orchestrator (reviewer recommendation, accepted or overridden). A card without an answer is skipped and reported. Cards: 2B5AF3F2, 387EAA05, 7D41FB60, 3380EB1C, E0A24E86, F34827AC, 3BAF631D, E4631CD9, C7F16BFB.

- RV-CONNECT-2B5AF3F2: (no answer yet - skip)
- RV-CONNECT-387EAA05: (no answer yet - skip)
- RV-CONNECT-7D41FB60: (no answer yet - skip)
- RV-CONNECT-3380EB1C: (no answer yet - skip)
- RV-CONNECT-E0A24E86: (no answer yet - skip)
- RV-CONNECT-F34827AC: (no answer yet - skip)
- RV-CONNECT-3BAF631D: (no answer yet - skip)
- RV-CONNECT-E4631CD9: (no answer yet - skip)
- RV-CONNECT-C7F16BFB: (no answer yet - skip)

## Findings (ordered by risk, then size)

### RV-CONNECT-2B5AF3F2 - Session id is zero-initialized and stamped externally; changepw takes an unchecked second id

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/state/session.rs:244,285-288,491-497; crates/shamir-connect/src/server/auth/changepw.rs:115-144
- Recipe: Bind immutable session identity before HMAC caching and use that identity for password-change proof binding.
- Note: Pre-insertion cache warming and unchecked second SID remain possible.

### RV-CONNECT-387EAA05 - Library expect panics for OS RNG failure and pre-epoch clock

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/primitives/crypto.rs:66-69; crates/shamir-connect/src/common/runtime/time.rs:18-21
- Recipe: Introduce fallible entropy and clock APIs and propagate typed environmental errors through callers.
- Note: Environmental expect branches remain in the connect library.
- Second pass: Authentication reaches both environmental panic helpers; fallible APIs require typed error propagation and public-contract decisions.

### RV-CONNECT-7D41FB60 - Folded bundle items

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `L/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/messages/auth_message.rs:6; crates/shamir-connect/src/common/primitives/kdf_params.rs:77; crates/shamir-connect/src/common/primitives/crypto.rs:69; crates/shamir-connect/src/server/auth/handshake.rs:244
- Recipe: Resolve the four component cards and close this aggregate without duplicating implementation work.
- Note: Regraded from nit because the bundle includes environmental panics; all folded components survive.

### RV-CONNECT-3380EB1C - Long-lived server secrets are plain arrays outside the zeroization policy

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/infra/config.rs:26-31; crates/shamir-connect/src/server/state/resume.rs:145-153
- Recipe: Use zeroizing ownership for server and resume secrets while preserving immutable key/cache consistency.
- Note: Resume keys became private, but raw arrays still have no wiping Drop.

### RV-CONNECT-74705EAB - Registered auth-message header and length oracles contradict the actual domain tag

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/primitives/domain_tags.rs:17; crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:127,200; crates/shamir-connect/test-vectors/auth_message_default.toml:21
- Recipe: Correct header and length oracles to 13 and 148 bytes without changing wire bytes.
- Note: Registered assertions contradict the serialized tag; no tests executed.

### RV-CONNECT-781E7475 - No byte-exact vectors for auth_message_cp and bootstrap payload

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/tests/crypto_tests/test_vectors_tests.rs:36-45; crates/shamir-connect/src/common/messages/changepw.rs:54; crates/shamir-connect/src/common/messages/bootstrap_message.rs:21
- Recipe: Add independently derived complete-byte password-change and bootstrap payload fixtures and registered assertions.
- Note: Neither canonical builder has the named complete-byte vector.

### RV-CONNECT-E0A24E86 - Ticket wire version is a magic literal with asymmetric encrypt/decrypt validation

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/state/ticket.rs:172,188,224
- Recipe: Centralize the supported ticket version and reject unsupported versions during encryption.
- Note: Public encryption accepts versions that decryption rejects.

### RV-CONNECT-F34827AC - Ticket ciphertext length silently truncates at u16

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/state/ticket.rs:114-120,132-135
- Recipe: Make ticket serialization reject ciphertext lengths above u16::MAX and test boundary round trips.
- Note: Oversized public ciphertext writes all bytes with a truncated prefix.

### RV-CONNECT-F43EB41E - Consolidated error-path test gaps

- Verdict: `PARTIAL`; risk `low`; fix size/kind `M/test`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/tests/infra_tests/durable_counters_tests.rs:141-164; crates/shamir-connect/src/server/tests/session_flow/changepw_challenge_tests.rs:125-156; crates/shamir-connect/src/server/tests/security/rate_limit_tests.rs:143-149; crates/shamir-connect/src/server/tests/security/lockout_tests.rs:186-192
- Recipe: Add failing snapshot load/save oracles and complete password-change exit-path buffer-wipe coverage.
- Note: Counter fault injection, reversed-clock and policy-error wipe tests now exist; snapshot failure coverage remains absent.
- Second pass: Counter fault oracles exist; snapshot fixtures still always succeed and complete wipe-exit coverage remains outstanding.

### RV-CONNECT-CD221FBC - Vacuous variant assertions in the all-zero nonce tests

- Verdict: `STILL_OPEN`; risk `low`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:149-151,173-175
- Recipe: Assert both matches expressions and pin the intended nonce error diagnostics.
- Note: Tests require an error but discard the variant checks.

### RV-CONNECT-3BAF631D - start_change_password_challenge accepts an all-zero client_nonce_cp

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/auth/changepw.rs:55-74,130-145; crates/shamir-connect/src/common/messages/changepw.rs:59-60
- Recipe: Reject an all-zero client nonce before replacing pending challenge state and test preservation of the previous challenge.
- Note: Issuance accepts zero; later proof construction rejects it after consumption.

### RV-CONNECT-699EBFCF - Mid-function use statements violate imports-at-top

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/ops/admin.rs:326-329; crates/shamir-connect/src/server/tests/security/post_auth_rate_limit_tests.rs:97-99
- Recipe: Hoist the named Entry and concurrency-test imports to their respective file headers.
- Note: Named function-local imports remain without a documented exception.

### RV-CONNECT-C8866EB1 - Stale doc references to nonexistent artifact paths and old test name

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/doc`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/messages/auth_message.rs:6; crates/shamir-connect/src/common/messages/envelope.rs:90; crates/shamir-connect/tests/session/integration_session.rs:333
- Recipe: Point documentation to the flat fixture directory and the exact owning_and_view compatibility test name.
- Note: Both named stale references remain.

### RV-CONNECT-E4631CD9 - validate_client_kdf_safe returns String; its diagnostic is discarded

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `M/code`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/primitives/kdf_params.rs:77-88; crates/shamir-connect/src/client/handshake.rs:214-216
- Recipe: Return the common typed KDF error instead of allocated discarded String diagnostics.
- Note: Earlier stricter validation limits current handshake reachability.

### RV-CONNECT-349809B3 - Username normalization documentation promises sharp-s folding contradicted by pinned dependency and registered oracle

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/user/username.rs:8-10,41-46; crates/shamir-connect/tests/identity/integration_username_precis.rs:30-41
- Recipe: Document Lowercase_Mapping and preserved sharp-s without changing username identity semantics.
- Note: Production documentation contradicts the registered identity oracle.
- Second pass: Production sharp-s-folding prose contradicts the registered preserved-sharp-s oracle; correct prose without changing identity semantics.

### RV-CONNECT-4496810D - Stale Debug label reports permissions as &lt;RwLock&gt;

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/state/session.rs:112,144-149
- Recipe: Replace the stale RwLock Debug placeholder with an accurate immutable-snapshot label.
- Note: Debug misdescribes the actual permissions field.

### RV-CONNECT-66EA32BD - RequestEnvelopeRef hard-codes 32 instead of SESSION_ID_BYTES

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/messages/envelope.rs:95; crates/shamir-connect/src/server/state/session.rs:133
- Recipe: Use limits::SESSION_ID_BYTES for the borrowed envelope session-id array length.
- Note: Consistency debt only; current sizes agree.
- Second pass: Borrowed envelope length is hard-coded while the canonical constant currently also equals 32; no wire-format decision is needed.

### RV-CONNECT-68CAD3B6 - lockout.rs DashMaps use the default RandomState hasher

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/limits/lockout.rs:254-257
- Recipe: Declare an explicit approved Fx hasher for both lockout DashMaps.
- Note: Default-hasher convention violation remains; no slowdown measured.

### RV-CONNECT-850A9AFD - Dead suppressors keeping unused imports alive in server/handshake.rs

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/auth/handshake.rs:244,423-425
- Recipe: Remove the no-op function reference, dead documentation-target helper and imports kept solely by them.
- Note: Both named suppressors remain.

### RV-CONNECT-8B168854 - AuditError is hand-rolled with Display equal to Debug

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/infra/audit_chain.rs:429-435
- Recipe: Derive thiserror::Error with explicit diagnostic messages for AuditError variants.
- Note: Manual Display still delegates to Debug.

### RV-CONNECT-90EC2E05 - RateLimiter trait doc says sliding-window

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/limits/rate_limit.rs:89,375-384
- Recipe: Replace sliding-window terminology with token-bucket terminology in the trait and module documentation.
- Note: Documentation still names a different algorithm.

### RV-CONNECT-A57D515B - Argon2Semaphore exposes blocking Mutex+Condvar acquisition as an async adoption trap

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/limits/argon2_semaphore.rs:10-12,80-101
- Recipe: Require blocking acquisition inside spawn_blocking for async callers and correct the nonexistent wait reference.
- Note: Blocking is documented, but async adoption guidance remains incomplete.

### RV-CONNECT-C75B520F - PushKind round-trip omits Ready

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/test`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/tests/message_tests/push_envelope_tests.rs:5-10; crates/shamir-server/src/engine/subscriptions/core/bridge.rs:577
- Recipe: Include Ready in the local PushKind round-trip loop.
- Note: The local omission remains for a production-emitted variant.

### RV-CONNECT-C7F16BFB - Unused unicode-normalization dependency

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/dep`; **OWNER DECISION**.
- Evidence at `7f04e096`: crates/shamir-connect/Cargo.toml:40; crates/shamir-connect/src/common/user/username.rs:44-47
- Recipe: Remove the unused direct unicode-normalization dependency while retaining PRECIS normalization.
- Note: No direct unicode_normalization source usage was found.

### RV-CONNECT-E6CF11C8 - AuthMessage fixed-capacity arithmetic

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/code`.
- Evidence at `7f04e096`: crates/shamir-connect/src/common/messages/auth_message.rs:82-97; crates/shamir-connect/src/common/primitives/domain_tags.rs:17
- Recipe: Derive exact capacity from AUTH_V1.len() and the serialized fixed field sizes.
- Note: Requested capacity is one byte short; allocator cost was not measured.

### RV-CONNECT-E71CB30A - finalize_change_password doc header describes verification

- Verdict: `STILL_OPEN`; risk `nit`; fix size/kind `S/doc`.
- Evidence at `7f04e096`: crates/shamir-connect/src/server/auth/changepw.rs:83-101
- Recipe: Separate verification documentation from the session-invalidation finalization helper documentation.
- Note: The attached header still promises work the helper does not perform.
- Second pass: Finalization inherits a verification header, but production verification remains separate and precedes persistence.

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

`cargo fmt -p shamir-connect -- --check`; `cargo clippy -p shamir-connect --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Report real exit codes.

## Final report

Per card: DONE / ALREADY-FIXED / SKIPPED(owner decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed crates you needed; anything that needs a build or test run.
