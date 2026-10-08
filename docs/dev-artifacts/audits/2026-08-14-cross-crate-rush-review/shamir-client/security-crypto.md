<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-client — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Resume authentication and secret-buffer hygiene remain real security issues. Unlimited-recursion and exporter-binding reassurance are refuted by pinned dependency and server source.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 15 | 8 | 0 | 0 | 1 | 1 | 5 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `Client::resume` hands the bearer ticket to an unverified peer — no server-identity check exists on the resume path

Status: `confirmed-open`. Current risk: `high`.

An active endpoint MITM can receive the ticket over accepted TLS. The real server checks binding strength and counters, not equality to the original exporter.

Evidence: [crates/shamir-client/src/client.rs:866](../../../../../crates/shamir-client/src/client.rs#L866); [crates/shamir-client/src/client.rs:895](../../../../../crates/shamir-client/src/client.rs#L895); [crates/shamir-connect/src/server/resume.rs:310](../../../../../crates/shamir-connect/src/server/resume.rs#L310); [crates/shamir-connect/src/server/resume.rs:324](../../../../../crates/shamir-connect/src/server/resume.rs#L324).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable

Status: `confirmed-open`. Current risk: `medium`.

Client fields and handling are absent. Current server wire also omits both signals; process_auth_ok does not invoke rotation recovery merely because fields are forwarded.

Evidence: [crates/shamir-client/src/wire_frames.rs:66](../../../../../crates/shamir-client/src/wire_frames.rs#L66); [crates/shamir-client/src/client.rs:603](../../../../../crates/shamir-client/src/client.rs#L603); [crates/shamir-server/src/connection/handshake.rs:632](../../../../../crates/shamir-server/src/connection/handshake.rs#L632); [crates/shamir-connect/src/client/handshake.rs:266](../../../../../crates/shamir-connect/src/client/handshake.rs#L266).

Grouping/duplicate: `SUMMARY.md#3.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Cleartext password persists in unzeroized serialization buffers after `create_scram_user`

Status: `confirmed-open`. Current risk: `medium`.

Plaintext is serialized into retained scratch and ordinary request/envelope vectors; transport framing adds another unwiped copy. Typed SecretString wiping does not cover them.

Evidence: [crates/shamir-client/src/client.rs:1104](../../../../../crates/shamir-client/src/client.rs#L1104); [crates/shamir-client/src/client.rs:1252](../../../../../crates/shamir-client/src/client.rs#L1252); [crates/shamir-transport-tcp/src/framing.rs:160](../../../../../crates/shamir-transport-tcp/src/framing.rs#L160).

Grouping/duplicate: `SUMMARY.md#3.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Early-buffer push map is unbounded in distinct `sub_id`s (server-keyed memory growth)

Status: `confirmed-open`. Current risk: `medium`.

A hostile authenticated or resume-substituted peer can create unlimited distinct buffer keys; the 256-envelope cap applies only per key.

Evidence: [crates/shamir-client/src/client.rs:349](../../../../../crates/shamir-client/src/client.rs#L349); [crates/shamir-client/src/subscription.rs:30](../../../../../crates/shamir-client/src/subscription.rs#L30).

Grouping/duplicate: `SUMMARY.md#4.3`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — No depth guard on nested msgpack decode of peer frames

Status: `refuted`. Current risk: —.

Pinned rmp-serde 1.3.1 initializes depth to 1024 and decrements it for arrays/maps, returning DepthLimitExceeded. Value recursion uses that deserializer.

Evidence: [Cargo.lock:2949](../../../../../Cargo.lock#L2949); [crates/shamir-client/src/client.rs:1292](../../../../../crates/shamir-client/src/client.rs#L1292); [crates/shamir-types/src/types/value.rs:283](../../../../../crates/shamir-types/src/types/value.rs#L283).

Grouping/duplicate: `SUMMARY.md#3.4`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — No negative-path coverage for pin verification or resume anywhere in the crate's suite

Status: `confirmed-open`. Current risk: `low`.

Client tests use TOFU without a pin; in-crate resume tests only serialize frames. External server resume coverage does not exercise wrong-pin refusal.

Evidence: [crates/shamir-client/src/tests/interner_cache_tests.rs:98](../../../../../crates/shamir-client/src/tests/interner_cache_tests.rs#L98); [crates/shamir-client/src/tests/resume_wire_tests.rs:1](../../../../../crates/shamir-client/src/tests/resume_wire_tests.rs#L1); [crates/shamir-server/tests/duplex_e2e.rs:239](../../../../../crates/shamir-server/tests/duplex_e2e.rs#L239).

Grouping/duplicate: `SUMMARY.md#3.5`. This row is not another independent defect.

<a id="review-nits-client-rs-603"></a>

### Claim Nits/client.rs:603 — `resume` extracts the TLS exporter solely to prove availability, then discards it while `WireResumeInit.binding_mode` declares `BindingMode::TlsExporter`

Status: `confirmed-open`. Current risk: `nit`.

The value is discarded. BindingMode reports transport capability, not a client proof of the exporter; server derives its own connection exporter.

Evidence: [crates/shamir-client/src/client.rs:877](../../../../../crates/shamir-client/src/client.rs#L877); [crates/shamir-client/src/client.rs:893](../../../../../crates/shamir-client/src/client.rs#L893); [crates/shamir-server/src/connection/handshake.rs:133](../../../../../crates/shamir-server/src/connection/handshake.rs#L133).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-nits-client-rs-935-943"></a>

### Claim Nits/client.rs:935-943 — `get_ddl_op_status` comment says `not_supported` is treated as "feature unavailable rather than a hard error" but the code returns `Err(ClientError::Protocol(..))`

Status: `confirmed-open`. Current risk: `low`.

The comment remains misleading, but the Protocol branch never executes because roundtrip first returns a structured Db error.

Evidence: [crates/shamir-client/src/client.rs:1209](../../../../../crates/shamir-client/src/client.rs#L1209); [crates/shamir-client/src/client.rs:1293](../../../../../crates/shamir-client/src/client.rs#L1293).

Grouping/duplicate: `SUMMARY.md#1.7`. This row is not another independent defect.

<a id="review-nits-error-rs-28-29"></a>

### Claim Nits/error.rs:28-29 — `ClientError::RequestIdMismatch` is never constructed anywhere; dead public error surface

Status: `confirmed-open`. Current risk: `nit`.

The variant remains declared; demux removes by rid and drops unknown IDs without constructing it.

Evidence: [crates/shamir-client/src/error.rs:29](../../../../../crates/shamir-client/src/error.rs#L29); [crates/shamir-client/src/client.rs:389](../../../../../crates/shamir-client/src/client.rs#L389).

Grouping/duplicate: `SUMMARY.md#5.5`. This row is not another independent defect.

<a id="review-nf-1"></a>

### Claim NF.1 — `connect` fail-closed

Status: `not-applicable`. Current risk: —.

Without a pin or explicit TOFU permission, HandshakeBuilder::build returns an error; successful full authentication checks SCRAM, pin, and identity signature.

Evidence: [crates/shamir-connect/src/client/handshake.rs:149](../../../../../crates/shamir-connect/src/client/handshake.rs#L149); [crates/shamir-connect/src/client/handshake.rs:258](../../../../../crates/shamir-connect/src/client/handshake.rs#L258).

Grouping/duplicate: `SUMMARY.md#NF.1`. This row is not another independent defect.

<a id="review-nf-2"></a>

### Claim NF.2 — KDF-DoS: server-supplied Argon2id params are double-capped before allocation

Status: `not-applicable`. Current risk: —.

Both validations precede derivation. Effective protocol caps are 256 MiB and eight passes, stricter than outer 512 MiB/sixteen-pass limits.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](../../../../../crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/common/types.rs:123](../../../../../crates/shamir-connect/src/common/types.rs#L123); [crates/shamir-connect/src/common/kdf_params.rs:77](../../../../../crates/shamir-connect/src/common/kdf_params.rs#L77).

Grouping/duplicate: `SUMMARY.md#NF.2`. This row is not another independent defect.

<a id="review-nf-3"></a>

### Claim NF.3 — Comparison hygiene and CSPRNG-backed resume nonce generation

Status: `not-applicable`. Current risk: —.

SCRAM signature and pin use constant_time_eq; identity uses strict Ed25519 verification. Resume uses rand::rng with pinned rand 0.9.4.

Evidence: [crates/shamir-connect/src/client/handshake.rs:260](../../../../../crates/shamir-connect/src/client/handshake.rs#L260); [crates/shamir-connect/src/common/crypto.rs:249](../../../../../crates/shamir-connect/src/common/crypto.rs#L249); [crates/shamir-client/src/client.rs:884](../../../../../crates/shamir-client/src/client.rs#L884); [Cargo.lock:2692](../../../../../Cargo.lock#L2692).

Grouping/duplicate: `SUMMARY.md#NF.3`. This row is not another independent defect.

<a id="review-nf-4"></a>

### Claim NF.4 — Untrusted-input size bounds and fixed-size wire-field validation

Status: `not-applicable`. Current risk: —.

Frame reads enforce 16 MiB before payload allocation; supplied salt, nonce, signature, public-key, and session-id lengths produce protocol errors on mismatch.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:110](../../../../../crates/shamir-transport-tcp/src/framing.rs#L110); [crates/shamir-client/src/client.rs:513](../../../../../crates/shamir-client/src/client.rs#L513); [crates/shamir-client/src/client.rs:563](../../../../../crates/shamir-client/src/client.rs#L563); [crates/shamir-client/src/client.rs:905](../../../../../crates/shamir-client/src/client.rs#L905).

Grouping/duplicate: `SUMMARY.md#NF.4`. This row is not another independent defect.

<a id="review-nf-5"></a>

### Claim NF.5 — `unsafe`: none in this crate; dependency framing has a sound SAFETY argument

Status: `unverified`. Current risk: —.

No client unsafe code exists. The dependency's set_len/reset mechanism and comment were read, but a comment alone does not prove the broader unsafe soundness guarantee.

Evidence: [crates/shamir-client/src/lib.rs:30](../../../../../crates/shamir-client/src/lib.rs#L30); [crates/shamir-transport-tcp/src/framing.rs:118](../../../../../crates/shamir-transport-tcp/src/framing.rs#L118); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129).

Grouping/duplicate: `SUMMARY.md#NF.4`. This row is not another independent defect.

<a id="review-nf-6"></a>

### Claim NF.6 — Typed-builder query construction and §9.4 name/id discipline

Status: `not-applicable`. Current risk: —.

Client query construction uses typed builders, with no serde_json/json macros. Numeric-looking names are looked up as strings; normal cache merges consume server IDs.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:268](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L268); [crates/shamir-client/src/interner_cache.rs:115](../../../../../crates/shamir-client/src/interner_cache.rs#L115); [crates/shamir-client/src/interner_cache_ops.rs:309](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L309).

Grouping/duplicate: `SUMMARY.md#NF.5`. This row is not another independent defect.

## Corrections and qualified non-findings

- The api review's 'not currently exploitable because of exporter binding' assertion is contradicted by process_resume and the #512 decision; binding-mode anti-downgrade does not authenticate the server.
- Checking a signed resume_ok after sending a bearer ticket prevents neither initial disclosure nor an attacker using the stolen ticket first.
- Documentation-only carry-through clarification is not a security fix for unauthenticated resume.
- Rotation recovery requires coordinated server wire emission, client cryptographic validation, and explicit operator/user approval; pass-through alone is insufficient.
- Secret-buffer risk requires heap/core/swap access or another memory-disclosure capability; it is not demonstrated remote exfiltration.
- A 1024-level dependency bound refutes unlimited recursion but does not experimentally prove target-stack safety.
- No rejected/rotated-ticket test 'anywhere' is false: crates/shamir-connect/tests/integration_resume.rs:437 and :984 provide relevant server-library coverage. Client security wiring still lacks it.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client -- Security & crypto boundary

## Summary

The full `connect` path is solid: TLS 1.3-only with an accept-any-cert verifier is compensated by three client-side checks (SCRAM mutual auth, TOFU/pin comparison, Ed25519 `verify_strict` over the TLS exporter), KDF parameters are double-capped before Argon2id runs, frame sizes are bounded, and there is no `unsafe` in this crate. The largest gap is the `resume` path, which re-uses the accept-any-cert TLS config but performs **none** of the server-identity checks before disclosing the bearer resumption ticket, and the SDK additionally drops the protocol's `rotation_in_progress` / `kdf_upgrade_required` signals, leaving pinned clients with no in-band recovery after server key rotation. Remaining findings are secret-hygiene (unzeroized serialization copies of the `create_scram_user` password) and bounded-but-real resource/recurse-hardening gaps.

## Findings

### 1. `Client::resume` hands the bearer ticket to an unverified peer — no server-identity check exists on the resume path
- **File:line:** `crates/shamir-client/src/client.rs:588-677` (esp. 591-604, 616-629, 663); `crates/shamir-client/src/wire_frames.rs:40-64`
- **Severity:** high
- **Issue:** `make_client_config_no_ca()` (shamir-transport-tcp/src/tls.rs:63-69) accepts *any* server certificate; all server authentication lives in the SCRAM handshake (`process_auth_ok`: mutual-auth proof + pin + Ed25519 over the TLS exporter). `Client::resume` re-uses that accept-any-cert TLS config but performs none of those checks: `WireResumeOk` carries no `server_pub_key`/`identity_sig`, `ResumeOptions::pinned_hash` is stored verbatim into the client (line 663) without ever being compared to anything, and the TLS exporter is extracted only to be discarded (`let _exporter`, line 603) while `WireResumeInit.binding_mode` still *claims* `TlsExporter`. The very first authenticated payload sent on this connection is the long-lived bearer ticket itself.
- **Failure scenario:** an active MITM terminates the TLS connection (any cert is accepted), reads `WireResumeInit.ticket` in cleartext, and now owns the victim's credential — it can open its own authenticated session against the real server (the server-side first-use-wins counter bounds *concurrent* use, but the attacker acts first and the victim's resume loudly fails) or transparently proxy the session. Every MITM protection the `connect` path provides is bypassed simply by inducing the client to resume. Note the server-side #512 / finding-1d analysis (shamir-connect/src/server/resume.rs:324-361) covers *already-stolen* tickets; it does not cover this path *creating* stolen tickets — `connect` would never disclose the ticket to a MITM, `resume` does.
- **Suggested fix:** authenticate the server before/at resume completion: extend `WireResumeOk` with `server_pub_key` + Ed25519 `identity_sig` over `(client_nonce, session_id, expires_at_ns, new-connection TLS exporter)` and verify strictly against `ResumeOptions::pinned_hash` before returning a usable `Client` (fail with a `ServerIdentityChanged`-style error on mismatch). This does not conflict with the #512 decision (it authenticates the server to the client; it does not bind the ticket to the exporter). Interim: document that `resume` must only run over networks where the server endpoint is otherwise authenticated.

### 2. SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable
- **File:line:** `crates/shamir-client/src/wire_frames.rs:66-86` (`WireAuthOk` lacks both fields); `crates/shamir-client/src/client.rs:506-516` (hard-codes `rotation_in_progress: None, kdf_upgrade_required: None`)
- **Severity:** medium
- **Issue:** the protocol and the connect crate support both signals: `AuthOkView` carries them (shamir-connect/src/server/handshake.rs:99-103), the server library can attach them (`with_rotation_in_progress` / `with_kdf_upgrade_required`, `complete_auth_ok`), and the client side of shamir-connect has `verify_rotation_in_progress` for exactly the orphan-client case. `shamir-client` neither deserializes nor forwards either field — msgpack decode drops them, and the values handed to `process_auth_ok` are hard-coded `None`.
- **Failure scenario:** server performs an identity-key rotation (the mechanism §6.5 exists for) → every Rust-SDK client (and the napi/Node binding wrapping this crate 1:1) that persisted a pin gets `ClientError::Handshake("ServerIdentityChanged")` on every connect, forever, with no in-band recovery even though the protocol defines one; similarly clients are never told to upgrade stale Argon2id parameters. Availability/operational impact, fail-closed rather than exploitable — but it turns routine key rotation into a manual pin-wipe for all SDK users.
- **Suggested fix:** add both fields to `WireAuthOk` with `#[serde(default)]`, pass them through to `ServerAuthOk`, and surface them on the SDK result (e.g. `Client::connect` returns/exposes the rotation payload after `verify_rotation_in_progress` succeeds, plus a `kdf_upgrade_required()` accessor).

### 3. Cleartext password persists in unzeroized serialization buffers after `create_scram_user`
- **File:line:** `crates/shamir-client/src/client.rs:975-989` (thread-local `REQ_BUF`, `req_bytes` clone, `envelope_bytes`), `crates/shamir-client/src/client.rs:828-842`
- **Severity:** medium
- **Issue:** `roundtrip` serializes the `DbRequest` — for `CreateScramUser` the wire field is the plaintext password — into (a) the **thread-local** `REQ_BUF`, which retains the bytes after the call (only `clear()`ed at the *next* request on that thread, i.e. persists until overwrite/thread exit), (b) `buf.clone()` → `req_bytes`, a plain `Vec`, and (c) `envelope_bytes`, another plain `Vec`. The `drop(req)` at line 842 and its comment ("wipe ASAP") wipe only the typed `SecretString`; at least three unzeroized heap copies of the password outlive the call, defeating the zeroize discipline the surrounding code (and the crate's dependency posture: `Zeroizing` password, `Zeroizing` ticket) is written to provide.
- **Failure scenario:** heap inspection / core dump / swap capture after a `create_scram_user` recovers the new user's cleartext password long after the request completed — precisely what the wipe-on-drop effort elsewhere is meant to prevent.
- **Suggested fix:** zeroize the occupied region of `REQ_BUF` after the clone (or immediately after `write_frame` completes), and route secret-bearing requests' `req_bytes`/`envelope_bytes` through `Zeroizing` (or explicit `.zeroize()` on all exit paths of `roundtrip`).

### 4. Early-buffer push map is unbounded in distinct `sub_id`s (server-keyed memory growth)
- **File:line:** `crates/shamir-client/src/client.rs:259-270`; `crates/shamir-client/src/subscription.rs:30-33`
- **Severity:** low
- **Issue:** per-`sub` entries are capped (`EARLY_BUFFER_CAP = 256` envelopes each), but the `early_buffer` map itself has no cap on the number of distinct sub ids, and the keys come from the peer. A misbehaving (authenticated) server can push frames with fresh random sub ids → one map entry per id, up to 256 envelopes per entry, unbounded total client memory. The registered-subscription path is properly bounded by the mpsc cap; only the pre-registration buffer leaks growth.
- **Failure scenario:** hostile server floods pushes with unique sub ids on a long-lived connection → client RSS grows without bound until OOM.
- **Suggested fix:** cap total buffered envelopes (or total distinct buffered sub ids, e.g. 64) across the map; drop-and-`warn!` beyond it, as the per-sub path already does.

### 5. No depth guard on nested msgpack decode of peer frames
- **File:line:** `crates/shamir-client/src/client.rs:121-135` (`decode_frame`), `client.rs:240` (`PushEnvelope::from_slice`), `client.rs:1018` (`DbResponse` decode)
- **Severity:** low
- **Issue:** server-supplied frames (bounded at 16 MiB) are decoded via recursive serde deserialization into `DbResponse`/`PushEnvelope`, whose `QueryValue`/`Value` trees recurse per nesting level with no depth limit (no `recursion_limit`/depth check anywhere in the crate). A frame of deeply nested arrays/maps can exhaust the task's stack during decode. Threat requires an authenticated (or, per finding 1, MITM-substituted) server, and the exposure is symmetric server-side — but the client is the "untrusted-input handler" for everything the peer sends.
- **Failure scenario:** hostile peer sends a ~16 MiB frame of nested seqs → stack overflow → client process abort (DoS).
- **Suggested fix:** verify rmp-serde's current recursion behavior for the pinned version; if unguarded, add a pre-decode structural depth check (e.g. lightweight scanner rejecting nesting beyond a sane bound) or decode in a `spawn_blocking` with a large stack as mitigation.

### 6. No negative-path coverage for pin verification or resume anywhere in the crate's suite
- **File:line:** all tests use `accept_new_host: true, trusted_pin: None` (`tests/smoke.rs:97-98`, `src/tests/cursor_stream_tests.rs:99-100`, `src/tests/interner_cache_tests.rs:98-99`, `src/tests/ambient_sync_tests.rs:101-102`, `src/tests/v2_passthrough_tests.rs:94-95`, all `tests/*_e2e.rs`); `Client::resume` has only serialization-level tests in-crate (`src/tests/resume_wire_tests.rs`) and a single happy-path e2e outside the crate (`shamir-server/tests/duplex_e2e.rs:239`)
- **Severity:** low
- **Issue:** the SDK's own wiring of the security-critical knobs is untested: no test connects with `trusted_pin: Some(..)` (happy path), no test asserts a pin mismatch is refused with `ServerIdentityChanged`, no test asserts `trusted_pin: None + accept_new_host: false` fails closed, and the `pin_capture` callback + `.expect` at client.rs:539-542 and the `ResumeOptions::pinned_hash` plumbing (which finding 1 shows is a no-op check) are never exercised.
- **Suggested fix:** add e2e tests: (a) pinned connect against the pinned server succeeds; (b) pinned connect after server key rotation (or against a second server instance) fails with the identity-changed error; (c) no pin + TOFU refused fails closed; (d) an in-crate resume e2e (happy path + a wrong-pin variant once finding 1's fix lands).

### Nits
- `client.rs:603` — `resume` extracts the TLS exporter solely to prove availability, then discards it while `WireResumeInit.binding_mode` declares `BindingMode::TlsExporter`; the binding claim is decorative on this path (subsumed by finding 1's fix, which should actually use the exporter).
- `client.rs:935-943` — `get_ddl_op_status` comment says `not_supported` is treated as "feature unavailable rather than a hard error" but the code returns `Err(ClientError::Protocol(..))`; comment/behavior mismatch (correctness, not security).
- `error.rs:28-29` — `ClientError::RequestIdMismatch` is never constructed anywhere; dead public error surface.

## Non-findings (checked, clean)

- `connect` fail-closed: `HandshakeBuilder::build` errors when neither pin nor `accept_new_host` is set (shamir-connect/src/client/handshake.rs:148-153) — the SDK cannot silently skip server auth on the full path.
- KDF-DoS: server-supplied Argon2id params are double-capped *before* allocation (`validate_client_limits` + `validate_client_kdf_safe`, hard ceiling 512 MiB / 16 passes) inside `process_challenge` — a MITM cannot OOM the client via challenge params.
- Comparison hygiene: all secret comparisons (server signature, pin) use `constant_time_eq`; the client only *computes* HMAC tags (`compute_tag_hex`) and never compares them, so no client-side timing oracle exists. Nonce generation for resume uses `rand::rng()` (CSPRNG-backed).
- Untrusted-input size bounds: all frame reads enforce `MAX_FRAME_SIZE_DEFAULT` (16 MiB); fixed-size wire fields (`salt`, nonces, signatures, `session_id`) are `try_into`-checked with protocol errors on mismatch — no panics on malformed input.
- `unsafe`: none in this crate. (The one `unsafe` in the dependency path, `read_frame_into`'s `set_len` in shamir-transport-tcp/src/framing.rs:118-136, carries a sound SAFETY argument with an uninit-leak defense-in-depth reset.)
- Query construction goes through the typed builder everywhere (`interner_cache_ops.rs` uses QueryValue *accessors* only for parsing server responses) — CLAUDE.md's builder-only rule holds; §9.4 "ids only from server responses / names never parsed as numbers" discipline is consistently documented and implemented.

</details>
