<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-connect — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Feature separation, dispatch policy, placeholder encoding, and several API hygiene gaps remain. Missing byte annotations are proven; wire-size multipliers and blanket auth-error collapse are not.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 17 | 14 | 0 | 0 | 1 | 0 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — client feature cannot build without server

Status: `confirmed-open`. Current risk: `medium`.

The documented client-only feature recipe removes server registration while client modules import server types unconditionally. Source proves the inconsistency; no compiler was run.

Evidence: [crates/shamir-connect/Cargo.toml:18](../../../../../crates/shamir-connect/Cargo.toml#L18); [crates/shamir-connect/src/lib.rs:29](../../../../../crates/shamir-connect/src/lib.rs#L29); [crates/shamir-connect/src/client/changepw.rs:11](../../../../../crates/shamir-connect/src/client/changepw.rs#L11); [crates/shamir-connect/README.md:24](../../../../../crates/shamir-connect/README.md#L24).

<a id="review-2"></a>

### Claim 2 — dispatch_request lacks its documented twin's rate-limit gate

Status: `confirmed-open`. Current risk: `medium`.

Policy asymmetry remains public; the live server chooses the protected variant.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:100](../../../../../crates/shamir-connect/src/server/dispatch.rs#L100); [crates/shamir-connect/src/server/dispatch.rs:153](../../../../../crates/shamir-connect/src/server/dispatch.rs#L153).

Grouping/duplicate: `security-crypto.md#1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — encode_details_canonical is a broken placeholder public API

Status: `confirmed-open`. Current risk: `low`.

The signature uses DefaultConfig and the body returns empty bytes. No production caller was found.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:355](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L355); [crates/shamir-connect/src/server/audit_chain.rs:360](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L360).

<a id="review-4"></a>

### Claim 4 — RequestHandler::handle Err(String) flows verbatim onto the wire

Status: `confirmed-open`. Current risk: `low`.

The raw String contract and forwarding remain; production includes formatted decoder errors. An embedder can leak internal diagnostics. However, AUTH §14 concerns authentication privacy and does not require every application error to become AuthFailed.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:30](../../../../../crates/shamir-connect/src/server/dispatch.rs#L30); [crates/shamir-connect/src/server/dispatch.rs:168](../../../../../crates/shamir-connect/src/server/dispatch.rs#L168); [crates/shamir-server/src/db_handler/handler.rs:344](../../../../../crates/shamir-server/src/db_handler/handler.rs#L344); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:898](../../../../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L898).

<a id="review-5"></a>

### Claim 5 — PushEnvelope.data is not serde_bytes-wrapped

Status: `confirmed-open`. Current risk: `low`.

Option<Vec<u8>> lacks byte serialization, while neighboring byte fields explicitly opt in. Production serializes this envelope and the local round-trip test cannot pin array versus bin. The exact pinned encoder behavior and 3–5x claim remain unverified.

Evidence: [crates/shamir-connect/src/common/push_envelope.rs:32](../../../../../crates/shamir-connect/src/common/push_envelope.rs#L32); [crates/shamir-connect/src/common/envelope.rs:100](../../../../../crates/shamir-connect/src/common/envelope.rs#L100); [crates/shamir-connect/src/common/tests/push_envelope_tests.rs:26](../../../../../crates/shamir-connect/src/common/tests/push_envelope_tests.rs#L26); [crates/shamir-server/src/subscriptions/push.rs:95](../../../../../crates/shamir-server/src/subscriptions/push.rs#L95); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

<a id="review-6"></a>

### Claim 6 — Ticket wire version is a magic literal with asymmetric encrypt/decrypt validation

Status: `confirmed-open`. Current risk: `low`.

Encryption accepts the supplied version; decryption rejects non-v2. Production issuers explicitly use 2, so unusable tickets require a different public caller or a future migration mistake.

Evidence: [crates/shamir-connect/src/server/ticket.rs:176](../../../../../crates/shamir-connect/src/server/ticket.rs#L176); [crates/shamir-connect/src/server/ticket.rs:188](../../../../../crates/shamir-connect/src/server/ticket.rs#L188); [crates/shamir-connect/src/server/ticket.rs:224](../../../../../crates/shamir-connect/src/server/ticket.rs#L224); [crates/shamir-connect/src/server/resume.rs:467](../../../../../crates/shamir-connect/src/server/resume.rs#L467).

<a id="review-7"></a>

### Claim 7 — No byte-exact vectors for auth_message_cp and bootstrap payload

Status: `confirmed-open`. Current risk: `low`.

Neither construction appears in the vector manifest or registered vector assertions. Existing round-trip/signature tests are not independent cross-language byte oracles.

Evidence: [crates/shamir-connect/src/common/changepw.rs:54](../../../../../crates/shamir-connect/src/common/changepw.rs#L54); [crates/shamir-connect/src/common/bootstrap_message.rs:21](../../../../../crates/shamir-connect/src/common/bootstrap_message.rs#L21); [crates/shamir-connect/test-vectors/README.md:45](../../../../../crates/shamir-connect/test-vectors/README.md#L45); [crates/shamir-connect/src/common/tests/test_vectors_tests.rs:37](../../../../../crates/shamir-connect/src/common/tests/test_vectors_tests.rs#L37).

<a id="review-8"></a>

### Claim 8 — validate_client_kdf_safe returns String; its diagnostic is discarded

Status: `confirmed-open`. Current risk: `nit`.

String error and discarded message remain. Current stricter memory/time validation precedes this outer cap, so the claimed runtime loss of distinct outer-cap diagnostics is not currently reachable in handshake.

Evidence: [crates/shamir-connect/src/common/kdf_params.rs:77](../../../../../crates/shamir-connect/src/common/kdf_params.rs#L77); [crates/shamir-connect/src/common/kdf_params.rs:72](../../../../../crates/shamir-connect/src/common/kdf_params.rs#L72); [crates/shamir-connect/src/client/handshake.rs:209](../../../../../crates/shamir-connect/src/client/handshake.rs#L209).

<a id="review-9"></a>

### Claim 9 — Session id is zero-initialized and stamped externally; changepw takes an unchecked second id

Status: `confirmed-open`. Current risk: `low`.

The construction/stamping protocol and stale explanation remain. The verifier does not compare its sid argument to session.session_id. Production passes the stored id, so mismatches concern direct API misuse.

Evidence: [crates/shamir-connect/src/server/session.rs:243](../../../../../crates/shamir-connect/src/server/session.rs#L243); [crates/shamir-connect/src/server/session.rs:440](../../../../../crates/shamir-connect/src/server/session.rs#L440); [crates/shamir-connect/src/server/changepw.rs:117](../../../../../crates/shamir-connect/src/server/changepw.rs#L117); [crates/shamir-connect/src/server/changepw.rs:147](../../../../../crates/shamir-connect/src/server/changepw.rs#L147); [crates/shamir-server/src/db_handler/admin.rs:519](../../../../../crates/shamir-server/src/db_handler/admin.rs#L519).

<a id="review-10-1"></a>

### Claim 10.1 — AuthMessage fixed-capacity arithmetic

Status: `confirmed-open`. Current risk: `nit`.

Requested capacity is 142 plus username length; actual fixed bytes are 143, not the report's 144.

Evidence: [crates/shamir-connect/src/common/auth_message.rs:82](../../../../../crates/shamir-connect/src/common/auth_message.rs#L82); [crates/shamir-connect/src/common/domain_tags.rs:17](../../../../../crates/shamir-connect/src/common/domain_tags.rs#L17).

Grouping/duplicate: `correctness-tdd.md#12.1`. This row is not another independent defect.

<a id="review-10-2"></a>

### Claim 10.2 — Stale test-vectors/auth_v1 path

Status: `confirmed-open`. Current risk: `nit`.

The nonexistent directory reference remains.

Evidence: [crates/shamir-connect/src/common/auth_message.rs:6](../../../../../crates/shamir-connect/src/common/auth_message.rs#L6).

Grouping/duplicate: `style-claude-md.md#5`. This row is not another independent defect.

<a id="review-10-3"></a>

### Claim 10.3 — RateLimiter trait doc says sliding-window

Status: `confirmed-open`. Current risk: `nit`.

The trait label remains inconsistent with the token-bucket implementation.

Evidence: [crates/shamir-connect/src/server/rate_limit.rs:89](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L89); [crates/shamir-connect/src/server/rate_limit.rs:148](../../../../../crates/shamir-connect/src/server/rate_limit.rs#L148).

<a id="review-10-4"></a>

### Claim 10.4 — PushKind round-trip omits Ready

Status: `confirmed-open`. Current risk: `nit`.

The local variant loop still lists four of five variants. Ready is used in production; this is a narrow local coverage omission, not proof of repository-wide absence.

Evidence: [crates/shamir-connect/src/common/tests/push_envelope_tests.rs:5](../../../../../crates/shamir-connect/src/common/tests/push_envelope_tests.rs#L5); [crates/shamir-server/src/subscriptions/bridge.rs:380](../../../../../crates/shamir-server/src/subscriptions/bridge.rs#L380).

<a id="review-10-5"></a>

### Claim 10.5 — RequestEnvelopeRef hard-codes 32 instead of SESSION_ID_BYTES

Status: `confirmed-open`. Current risk: `nit`.

The literal array width remains. It currently matches the protocol width; this is maintenance consistency debt.

Evidence: [crates/shamir-connect/src/common/envelope.rs:95](../../../../../crates/shamir-connect/src/common/envelope.rs#L95).

<a id="review-10-6"></a>

### Claim 10.6 — kdf_upgrade_required Option<bool> has three states

Status: `not-applicable`. Current risk: —.

The fields deliberately represent an optional wire hint; absence and a present Boolean are representable states. No ambiguous consumer behavior is shown. Simplifying the type is a preference, not a demonstrated defect.

Evidence: [crates/shamir-connect/src/client/handshake.rs:72](../../../../../crates/shamir-connect/src/client/handshake.rs#L72); [crates/shamir-connect/src/server/handshake.rs:101](../../../../../crates/shamir-connect/src/server/handshake.rs#L101); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:872](../../../../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L872).

<a id="review-summary-wire-guarantees"></a>

### Claim Summary.wire-guarantees — Eight byte-exact vectors and owning/borrowed/view compatibility

Status: `refuted`. Current risk: —.

Vector files and compatibility assertions are registered, but the blanket strong-corpus claim misses contradictions: the auth vector declares 149 bytes while the actual tag/layout yields 148 for alice, and a header test compares 14 bytes with a 13-byte literal. Compatibility tests exist; no execution result is claimed.

Evidence: [crates/shamir-connect/src/common/tests/mod.rs:10](../../../../../crates/shamir-connect/src/common/tests/mod.rs#L10); [crates/shamir-connect/src/common/tests/auth_message_tests.rs:127](../../../../../crates/shamir-connect/src/common/tests/auth_message_tests.rs#L127); [crates/shamir-connect/src/common/tests/auth_message_tests.rs:200](../../../../../crates/shamir-connect/src/common/tests/auth_message_tests.rs#L200); [crates/shamir-connect/test-vectors/auth_message_default.toml:21](../../../../../crates/shamir-connect/test-vectors/auth_message_default.toml#L21); [crates/shamir-connect/tests/integration_session.rs:333](../../../../../crates/shamir-connect/tests/integration_session.rs#L333).

<a id="review-summary-builder-only"></a>

### Claim Summary.builder-only — No serde_json or constructed queries in connect

Status: `not-applicable`. Current risk: —.

Connect forwards opaque request bytes and no serde_json use was found in its source. Query-building rules are not implicated by these envelopes.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:163](../../../../../crates/shamir-connect/src/server/dispatch.rs#L163); [crates/shamir-connect/Cargo.toml:34](../../../../../crates/shamir-connect/Cargo.toml#L34).

## Corrections and qualified non-findings

- Use a real value type such as a typed serializable map; rmp_serde::Value is not established as an available API.
- Changing push serialization is a wire-compatibility change, not merely a local allocation optimization. Validate existing clients before migration.
- Do not apply authentication-only error vocabulary wholesale to application/query errors.
- The auth header and fixed-size arithmetic are off by one, not two; reconcile tests and metadata without changing established wire bytes accidentally.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-connect -- API & wire-protocol design

## Summary

The wire layer is in strong shape overall: canonical byte strings (`auth_message`, `identity_input`, rotation/bootstrap payloads) are hand-serialized with domain-separated tags, pinned by 8 byte-exact cross-language test vectors, and the msgpack envelopes (`Request`/`Response`/`Error`/`TicketWire`) have wire-compat tests between owning/borrowed/view forms. No `serde_json` exists anywhere in the crate and no query is ever constructed here (req/res are opaque blobs), so the builder-only query-construction rule is trivially satisfied. The findings below are interface-quality issues: a broken feature matrix (the advertised client-only build cannot compile), a behavioral divergence between the two public dispatch entry points, one broken placeholder public function, and several wire-format/versioning hygiene gaps.

## Findings

### 1. `client` feature cannot build without `server` — client modules unconditionally import `crate::server::*`
- File:line: `crates/shamir-connect/Cargo.toml:16-19`; `src/lib.rs:25-31`; `src/client/handshake.rs:27`; `src/client/bootstrap.rs:17-19`; `src/client/changepw.rs:11`; `src/client/rotation.rs:20`; `README.md:16-17,23-24`
- Severity: high
- Issue: The manifest declares `client = []` and `server = [...]` as independent features, and the README advertises `default-features = false, features = ["client"]` as a "client-only SDK (smaller binary, no server-only deps)". But every file in `src/client/` imports types from `crate::server` (`RotationInProgressPayload`, `BootstrapChallenge`/`BootstrapRequest`, `ChangePwRequest`, `IdentityRotationEvent`), and `pub mod server` is `#[cfg(feature = "server")]`-gated in `lib.rs`. Under `--no-default-features --features client` the `server` module does not exist, so the client module fails to compile with unresolved-import errors.
- Failure scenario: an embedder follows the README's client-only recipe; the build breaks. The feature contract promised by Cargo.toml/README is unfulfillable as declared (it only compiles today because both features are on by default).
- Suggested fix: either encode the dependency as `client = ["server"]` (and fix the README claim), or move the shared wire views (`RotationInProgressPayload`, `BootstrapChallenge/Hello/Request`, `ChangePwRequest`, `IdentityRotationEvent`) into `common/` so `client` genuinely stands alone.

### 2. `dispatch_request` (owning variant) lacks the post-auth rate-limit gate its documented twin `dispatch_request_view` enforces
- File:line: `src/server/dispatch.rs:69-110` vs `dispatch.rs:150-160` (doc claim "Functionally identical" at `dispatch.rs:117`)
- Severity: medium
- Issue: `dispatch_request_view` runs the task-#608 per-session `check_post_auth_rate_limit` gate before invoking the handler; `dispatch_request` — exported side-by-side from `server/mod.rs:28` and documented as the per-request entry point with "same §7.5 validity check, same handler dispatch, same outcome shape" — does not. The two public entry points are not behaviorally identical.
- Failure scenario: latent today (shamir-server routes through `dispatch_request_view` only), but any future transport/embedder taking the owning-`RequestEnvelope` path silently bypasses the post-auth rate limiter; the doc actively asserts equivalence, so nothing signals the trap.
- Suggested fix: add the same `check_post_auth_rate_limit` call to `dispatch_request`, or deprecate/remove the owning variant and make `_view` the only entry point.

### 3. `encode_details_canonical` is a broken placeholder public API (wrong parameter type, stub body)
- File:line: `src/server/audit_chain.rs:355-361`
- Severity: medium
- Issue: The doc promises "encode a `BTreeMap<String, msgpack-Value>` as canonical msgpack for use as `details_canonical_msgpack`", but the parameter is `&BTreeMap<String, rmp_serde::config::DefaultConfig>` — a serializer *config* type, not a value type — and the body is `let _ = map; Vec::new()`. It compiles, is `pub`, has zero callers and zero tests, and always returns an empty Vec.
- Failure scenario: a caller wiring the audit chain follows the doc, passes a real details map, and silently hashes empty `details_canonical_msgpack` into every audit-chain HMAC — the canonical-bytes contract that any second implementation must reproduce byte-identically (spec §3.3) is not actually implemented.
- Suggested fix: implement it over `&BTreeMap<String, rmp_serde::Value>` (or a typed details struct) with `rmp_serde::to_vec_named`, or delete the function until a real implementation exists.

### 4. `RequestHandler::handle`'s `Err(String)` flows verbatim onto the wire, bypassing the crate's own error-collapsing discipline
- File:line: `src/server/dispatch.rs:29-45, 100-109, 163-172`; cf. `src/common/error.rs:1-6, 86-102`
- Severity: medium
- Issue: `error.rs` is explicit that anything sent to a peer collapses to a generic string (spec §14.1/§14.4, enforced via `Error::to_wire`), and the crate's own protocol errors use the fixed §14 vocabulary (`session_expired`, `session_invalidated`, `rate_limited`). But the central public handler contract returns `Result<Vec<u8>, String>` and `ErrorEnvelope::new(request_id, err)` transmits the handler's string with no collapsing, vocabulary check, or even a doc warning.
- Failure scenario: a handler returns `format!("{e:?}")` of an internal error; internal paths/types leak to the client in the `error` field — exactly what the privacy rules in this crate's own error module forbid.
- Suggested fix: type the handler error as the crate `Error` (route through `to_wire` in `dispatch_request*`), or at minimum document that the string must come from the §14 vocabulary and add a test pinning the allowed set.

### 5. `PushEnvelope.data` is not `serde_bytes`-wrapped — wire bloat and inconsistency with every other byte field in the crate
- File:line: `src/common/push_envelope.rs:31-36`; contrast `src/common/envelope.rs:25-32` and `src/server/ticket.rs:56-66`
- Severity: medium
- Issue: `sid`, `req`, and all `TicketPlain` byte fields use `serde_bytes` (msgpack `bin`), but `PushEnvelope.data: Option<Vec<u8>>` — documented as carrying "MessagePack-encoded records, keys, etc.", i.e. the largest payloads on the wire — serializes as a msgpack *array of integers* (~3-5× larger per byte under rmp-serde). The round-trip test only proves Rust-to-Rust consistency, so the array-vs-bin choice silently becomes part of the cross-language contract a JS client must reproduce.
- Failure scenario: a subscription delivering record payloads pays a multi-fold size penalty per push; a second implementation that naturally encodes `bin` (following the crate's own dominant pattern) produces different bytes and fails interop.
- Suggested fix: `#[serde(with = "serde_bytes")]` on an `Option<Vec<u8>>` via a custom module (or switch to `serde_bytes::ByteBuf`) and pin the frame with a byte-exact test.

### 6. Ticket wire version is a magic literal with asymmetric encrypt/decrypt validation
- File:line: `src/server/ticket.rs:224-226` (decrypt rejects `!= 2`), `ticket.rs:168-193` (encrypt accepts any `plain.version`), `ticket.rs:176` (AAD binds `plain.version`); `src/server/resume.rs:262-265, 397, 467` (hard-coded `2`)
- Severity: low
- Issue: `TicketPlain.version` is a public `u8` with no constant for the v2 value. `encrypt_ticket_with_cipher` will happily encrypt and AAD-bind any version the caller set, while every decrypt path (and `process_resume` step 2) rejects anything but `2`.
- Failure scenario: a caller constructs a ticket with `version = 3` (or a future v2 → v3 migration misses one of the four `2` literals): tickets are issued that can never be resumed, failing closed but opaquely at first resume.
- Suggested fix: add `pub const TICKET_WIRE_VERSION: u8 = 2;`, validate `plain.version == TICKET_WIRE_VERSION` inside `encrypt_ticket_with_cipher`, and use the constant at all four sites.

### 7. No byte-exact vectors for two signed canonical strings: `auth_message_cp` and the bootstrap payload
- File:line: `src/common/changepw.rs:1-18, 54-84`; `src/common/bootstrap_message.rs:1-36`; `test-vectors/README.md:43-52`; `src/common/tests/test_vectors_tests.rs` (8 vectors, neither included)
- Severity: low
- Issue: The changepw module doc demands `auth_message_cp` be "byte-exactly reproduced by both sides", and `build_bootstrap_input` is the payload the client pins the server identity against — yet both are covered only by Rust-to-Rust round-trips (`integration_changepw.rs`, `integration_bootstrap.rs`). The vector suite whose stated purpose is protecting "cross-language interop" (its own header) omits exactly these two composite constructions, unlike `auth_message`, `identity_input`, and the rotation payloads.
- Failure scenario: a TS/browser client implements changePassword or bootstrap from the prose layout; a length-prefix or field-order slip fails only at runtime against the real server, with no pinned bytes to diff against.
- Suggested fix: add `auth_message_cp_default.{json,toml}` and `bootstrap_input_default.{json,toml}` pairs plus assertions in `test_vectors_tests.rs`, per the README's own "Adding new vectors" recipe.

### 8. `validate_client_kdf_safe` returns `Result<(), String>`; the sole caller discards the diagnostic
- File:line: `src/common/kdf_params.rs:77-91`; `src/client/handshake.rs:210-212`
- Severity: low
- Issue: A public library API returns a bare-`String` error, against the crate's own convention (thiserror `Error` enum everywhere else, CLAUDE.md error-handling rules), and its carefully-written downgrade-attack message is thrown away at the only call site (`if let Err(_msg)` → bare `Error::KdfParamsRejected`).
- Failure scenario: an operator debugging why a handshake rejects a server's KDF params gets no distinguishable reason (memory cap vs time cap), and future callers propagate `String` errors into higher-level APIs.
- Suggested fix: return `Result<(), Error>` with a `KdfLimit { field, limit }`-style variant (or reuse `Error::KdfParamsRejected`), and log/attach the reason at the call site.

### 9. `Session::session_id` is zero-initialized and stamped externally; stale doc leaves a redundant, unchecked `session_id` parameter
- File:line: `src/server/session.rs:126-138, 232-259` (zero init), `session.rs:435-451` (stamped at `SessionStore::insert`); `src/server/changepw.rs:113-127` (stale doc), `src/server/changepw.rs:1-7` (names nonexistent `verify_and_apply_change_password`)
- Severity: low
- Issue: `verify_change_password_request_with_sid`'s doc says the explicit `session_id` parameter exists "because `Session` does not carry its own id" — it does now (public field, stamped by the store). So the API takes two independent ids that are never cross-checked, and `Session::new` hands out a session whose `session_id` is all zeros (and whose `hmac_key()` is therefore derived from zeros) unless the caller knows to route through `SessionStore::insert`.
- Failure scenario: an embedder constructs a `Session` directly (or passes the wrong sid alongside the session): destructive-op confirmation tags and `auth_message_cp` bind to a zeroed/mismatched session id, with no error.
- Suggested fix: take the sid in `Session::new` (drop the external stamping), make `verify_change_password_request_with_sid` read `session.session_id` (or `debug_assert_eq!` the two), and refresh the stale module/function doc names.

### 10. Nits
- `src/common/auth_message.rs:80-82` — capacity comment/compute say the fixed part is `142 + username_len`; the actual fixed layout is 144 bytes (the crate's own test asserts 149 for a 5-byte username). Harmless under-allocation, but the comment math is wrong.
- `src/common/auth_message.rs:6` — doc points at `test-vectors/auth_v1/`; the real location is `test-vectors/auth_message_default.{json,toml}`.
- `src/server/rate_limit.rs:89` — `RateLimiter` trait doc says "sliding-window rate limiter"; the implementation (and module doc) is a token bucket.
- `src/common/tests/push_envelope_tests.rs:4-10` — round-trip iterates 4 of 5 `PushKind` variants; `Ready` is never exercised.
- `src/common/envelope.rs:95` — `RequestEnvelopeRef.session_id: &'a [u8; 32]` hard-codes `32` while the rest of the crate uses `limits::SESSION_ID_BYTES`.
- `src/client/handshake.rs:73` / `src/server/handshake.rs:103` — `kdf_upgrade_required: Option<bool>` models a boolean flag in three states; plain `bool` (or an enum) would remove the `Some(false)` ambiguity.

</details>
