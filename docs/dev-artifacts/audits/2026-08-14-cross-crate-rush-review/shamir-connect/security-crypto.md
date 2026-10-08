<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-connect — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Dispatch asymmetry and secret-hygiene issues remain. The KDF distinction is explicitly accepted by contract. Several exploit and canonical-encoding guarantees require correction.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 7 | 0 | 0 | 1 | 0 | 3 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Public dispatch_request silently skips the post-auth rate limit

Status: `confirmed-open`. Current risk: `medium`.

Owned dispatch lacks the gate present in view dispatch. Alternate embedders can bypass it; the shipped server uses the gated path.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:100](../../../../../crates/shamir-connect/src/server/dispatch.rs#L100); [crates/shamir-connect/src/server/dispatch.rs:153](../../../../../crates/shamir-connect/src/server/dispatch.rs#L153); [crates/shamir-server/src/connection/request_loop.rs:340](../../../../../crates/shamir-server/src/connection/request_loop.rs#L340).

<a id="review-2"></a>

### Claim 2 — Long-lived server secrets are plain arrays outside the zeroization policy

Status: `confirmed-open`. Current risk: `low`.

Server secrets and ticket keys remain ordinary arrays without wiping Drop implementations. Debug is already redacted. Wiping would reduce residual copies, not protect live secrets from a process-memory compromise.

Evidence: [crates/shamir-connect/src/server/config.rs:26](../../../../../crates/shamir-connect/src/server/config.rs#L26); [crates/shamir-connect/src/server/config.rs:29](../../../../../crates/shamir-connect/src/server/config.rs#L29); [crates/shamir-connect/src/server/config.rs:34](../../../../../crates/shamir-connect/src/server/config.rs#L34); [crates/shamir-connect/src/server/resume.rs:130](../../../../../crates/shamir-connect/src/server/resume.rs#L130).

<a id="review-3"></a>

### Claim 3 — Client password buffers are not zeroized on early-error paths

Status: `confirmed-open`. Current risk: `low`.

Fallible validation and derivation precede wiping. This is a reachable hygiene violation, not standalone remote password extraction.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](../../../../../crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/client/bootstrap.rs:93](../../../../../crates/shamir-connect/src/client/bootstrap.rs#L93); [crates/shamir-connect/src/client/changepw.rs:60](../../../../../crates/shamir-connect/src/client/changepw.rs#L60).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — ServerIdentityState::rotate / try_finalize are non-atomic check-then-act

Status: `confirmed-open`. Current risk: `low`.

Snapshot-based mutators can overwrite concurrent state. The claim that every ticket then fails is too strong: acceptance compares the ticket to the atomic mirror, so matching versions can still pass.

Evidence: [crates/shamir-connect/src/server/rotation.rs:100](../../../../../crates/shamir-connect/src/server/rotation.rs#L100); [crates/shamir-connect/src/server/rotation.rs:169](../../../../../crates/shamir-connect/src/server/rotation.rs#L169); [crates/shamir-connect/src/server/rotation.rs:194](../../../../../crates/shamir-connect/src/server/rotation.rs#L194); [crates/shamir-connect/src/server/resume.rs:290](../../../../../crates/shamir-connect/src/server/resume.rs#L290).

Grouping/duplicate: `concurrency-lockfree.md#7`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Known-user challenge exposes per-user KDF params — accepted residual enumeration

Status: `not-applicable`. Current risk: —.

Different stored/current parameter tuples are directly distinguishable. AUTH_PROTOCOL §13.5 explicitly accepts this trade-off, and the implementation names it. Timing examples are unnecessary and unmeasured.

Evidence: [crates/shamir-connect/src/server/handshake.rs:155](../../../../../crates/shamir-connect/src/server/handshake.rs#L155); [crates/shamir-connect/src/server/handshake.rs:182](../../../../../crates/shamir-connect/src/server/handshake.rs#L182); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:877](../../../../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L877).

<a id="review-6"></a>

### Claim 6 — start_change_password_challenge accepts an all-zero client_nonce_cp

Status: `confirmed-open`. Current risk: `nit`.

Issuance still stores the nonce without validating it; verification's canonical builder rejects it later. This is fail-fast consistency debt, not a replay bypass.

Evidence: [crates/shamir-connect/src/server/changepw.rs:64](../../../../../crates/shamir-connect/src/server/changepw.rs#L64); [crates/shamir-connect/src/server/changepw.rs:74](../../../../../crates/shamir-connect/src/server/changepw.rs#L74); [crates/shamir-connect/src/server/changepw.rs:145](../../../../../crates/shamir-connect/src/server/changepw.rs#L145); [crates/shamir-connect/src/common/changepw.rs:59](../../../../../crates/shamir-connect/src/common/changepw.rs#L59).

<a id="review-7"></a>

### Claim 7 — encode_details_canonical is a dead placeholder with a broken signature

Status: `confirmed-open`. Current risk: `nit`.

The unused public helper still ignores its input and returns empty bytes.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:355](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L355); [crates/shamir-connect/src/server/audit_chain.rs:360](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L360).

Grouping/duplicate: `api-wire-protocol.md#3`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — canonical_bytes length prefixes truncate silently

Status: `confirmed-open`. Current risk: `low`.

Public arbitrary Strings and details bytes are narrowed without checks. Normal production fields are bounded by their producers, but the helper has no such input contract enforcement.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:102](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L102); [crates/shamir-connect/src/server/audit_chain.rs:104](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L104); [crates/shamir-connect/src/server/audit_chain.rs:113](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L113).

<a id="review-8-guarantee"></a>

### Claim 8.guarantee — Raw bytes preserve HMAC collision-safety and debug_assert catches oversized fields

Status: `refuted`. Current risk: —.

The assertion compares actual output size with a capacity computed from the same full lengths, so truncation does not trigger it. Encoding is non-injective for unrestricted fields: transport=256 NUL bytes,user=empty and transport=empty,user=256 NUL bytes have identical encoded segments. This is encoding ambiguity, not a cryptographic HMAC collision or a demonstrated network exploit.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:91](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L91); [crates/shamir-connect/src/server/audit_chain.rs:104](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L104); [crates/shamir-connect/src/server/audit_chain.rs:106](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L106); [crates/shamir-connect/src/server/audit_chain.rs:116](../../../../../crates/shamir-connect/src/server/audit_chain.rs#L116).

<a id="review-summary-crypto-core"></a>

### Claim Summary.crypto-core — Constant-time comparisons, unconditional fake derivation, strict verification, and no unsafe

Status: `not-applicable`. Current risk: —.

The wrapper calls ConstantTimeEq and verify_strict, and fake material is derived before choosing the real/fake path. No unsafe code was found. These establish implementation choices, not measured whole-handshake timing equivalence.

Evidence: [crates/shamir-connect/src/common/crypto.rs:131](../../../../../crates/shamir-connect/src/common/crypto.rs#L131); [crates/shamir-connect/src/common/crypto.rs:254](../../../../../crates/shamir-connect/src/common/crypto.rs#L254); [crates/shamir-connect/src/server/handshake.rs:146](../../../../../crates/shamir-connect/src/server/handshake.rs#L146); [crates/shamir-connect/src/common/scram.rs:102](../../../../../crates/shamir-connect/src/common/scram.rs#L102).

<a id="review-summary-injection"></a>

### Claim Summary.injection — Opaque request bodies and exact-match directory lookups

Status: `not-applicable`. Current risk: —.

Connect dispatch forwards opaque bytes and its reference directory performs exact keyed lookup. This scoped observation must not become a claim that application handlers have no injection or disclosure surfaces.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:163](../../../../../crates/shamir-connect/src/server/dispatch.rs#L163); [crates/shamir-connect/src/server/admin.rs:317](../../../../../crates/shamir-connect/src/server/admin.rs#L317).

## Corrections and qualified non-findings

- Zeroization does not prevent exposure of still-live keys in core dumps; distinguish residual memory hygiene from process compromise.
- Known-user KDF parameters are a documented accepted distinction, not an unclosed normative defect.
- Remove both the canonical-byte collision-safe assurance and the assertion-catches-overflow claim.
- Rotation races do not imply universal ticket-resume rejection, and no live server rotate invocation was found.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-connect -- Security & crypto boundary

## Summary

The auth core is disciplined: SCRAM-Argon2id with `subtle` constant-time proof checks, HKDF-derived anti-enumeration fake material computed unconditionally on both real and unknown-user paths, `verify_strict` Ed25519 everywhere, fail-closed wire-byte enums, zero `unsafe` blocks in the crate, and an unusually strong redaction/test culture (log-redaction CI gate, committed wire test-vectors, fake-path handshake benchmarks). No injection surface exists: request bodies are opaque msgpack handed to the application layer, and admin targets resolve through exact-match directory lookups, never string-assembled queries. Findings concentrate at the edges rather than the crypto itself: a publicly exported dispatch variant that silently omits the post-auth rate limit its sibling enforces, long-lived server secrets and client password buffers that escape the crate's own zeroization policy on specific paths, and a non-atomic check-then-act rotation on server identity state. Theme test coverage is strong (per-module security unit tests, integration full-auth/wrong-password/unknown-user/anti-downgrade, wire vectors, redaction gate); the one policy gap between the two dispatch entry points is untested precisely because it exists.

## Findings

### 1. Public `dispatch_request` silently skips the post-auth rate limit that `dispatch_request_view` enforces
- **File:line:** `crates/shamir-connect/src/server/dispatch.rs:69-110` (gate present only at `dispatch.rs:150-160`); publicly re-exported at `crates/shamir-connect/src/server/mod.rs:28`
- **Severity:** medium
- **Issue:** Task #608's per-session token bucket (`Session::check_post_auth_rate_limit`) is enforced only inside `dispatch_request_view`, yet the owning `dispatch_request` is exported as a peer API and the view variant's doc claims the two are "functionally identical: same §7.5 validity check, same handler dispatch, same outcome shape". The shipped server is unaffected today (`crates/shamir-server/src/connection/request_loop.rs` uses the view variant), but any embedder or transport binding that picks the owning variant loses the post-auth flood control with no error, warning, or type-level distinction.
- **Failure scenario:** An authenticated client drives unbounded request-rate through an integration built on `dispatch_request`; the rate limiter never fires and handler/DB resources are exhausted.
- **Suggested fix:** Implement `dispatch_request` by borrowing from the owned envelope and delegating to `dispatch_request_view` (or hoist the rate-limit gate into a shared internal helper both call); add a test asserting both entry points enforce identical per-request policy.

### 2. Long-lived server secrets are plain `[u8; 32]`, outside the crate's own zeroization policy
- **File:line:** `crates/shamir-connect/src/server/config.rs:29-31` (`server_secret`, `lockout_secret`); `crates/shamir-connect/src/server/resume.rs:130-131` (`ticket_key`, `ticket_key_previous`)
- **Severity:** low
- **Issue:** `common/crypto.rs`'s module contract says the layer "enforces zeroization on key material (`Zeroizing<[u8; 32]>`)" and all SCRAM-derived values comply — but the crown-jewel long-lived secrets (anti-enumeration HKDF IKM, lockout HMAC key, ticket AES-GCM keys) are bare arrays: they clone freely (`ServerSecrets: Clone`, `ResumeConfig` fields, `issue_initial_ticket(&[u8; 32])`) and are never wiped on drop, unlike everything derived from them.
- **Failure scenario:** A core dump, heap-swap inspection, or future `Debug`/serialization path captures process memory and recovers `server_secret` indefinitely, defeating the zeroization discipline applied to `salted_password`/`client_key`/`server_key`.
- **Suggested fix:** Wrap them in `Zeroizing<[u8; 32]>` (derive `Clone` only), keeping `&[u8]` views for internal use; `ResumeConfig` can retain the pre-scheduled ciphers plus zeroizing key copies.

### 3. Client password buffers are not zeroized on early-error paths, violating the documented contract
- **File:line:** `crates/shamir-connect/src/client/handshake.rs:202-232` (doc at line 201 promises "password is consumed and zeroized on return"; early returns at 209-215 precede `password.zeroize()` at 232); `crates/shamir-connect/src/client/bootstrap.rs:85-97` (validate/derive `?` paths); `crates/shamir-connect/src/client/changepw.rs:24-72` (both `old_password` and `new_password` escape un-zeroized whenever any `?` fires)
- **Severity:** low
- **Issue:** Zeroization happens only on the success path after `DerivedKeys::derive`. Every attacker-influenceable rejection — server sends KDF params above the client caps, all-zero server nonce, password-policy failure — leaves the raw password resident in caller memory.
- **Failure scenario:** A malicious server deliberately replies with `kdf_params_rejected`-triggering parameters; the client's password lingers in freed-but-unwiped heap that a later core dump or heap-grooming attacker can recover.
- **Suggested fix:** Zeroize on scope exit regardless of result — a small guard type wrapping each `&mut [u8]` password slice (zeroize on `Drop`), or explicit zeroize before each early `return`/`?`.

### 4. `ServerIdentityState::rotate` / `try_finalize` are non-atomic check-then-act over `ArcSwap`
- **File:line:** `crates/shamir-connect/src/server/rotation.rs:151-180` (`rotate`: load -> overlap check -> store), `184-199` (`try_finalize`: load -> store, does not rewrite `current_version_atomic`)
- **Severity:** low
- **Issue:** Both methods clone a snapshot, decide, then `store` a new inner with no CAS. Concurrent `rotate` x `rotate` double-rotates from the same base (one freshly generated keypair silently dropped; version advanced once); a stale-snapshot `try_finalize` landing after a `rotate` reverts `previous`/`rotation_until_ns` to pre-rotation state while `current_version_atomic` keeps the rotated value. Afterwards `is_ticket_version_acceptable` (consulted at `resume.rs:290`) compares tickets against a version the live keypair no longer carries. Given CLAUDE.md's concurrency ideology, a decide+store pair on shared identity state should be a single atomic step; the "HIGH-5 fix" pre-condition check is not itself synchronized.
- **Failure scenario:** Admin rotation triggered concurrently with the finalize sweep => overlap-window guarantees silently broken and every ticket-based resume rejected (self-DoS) until the next successful rotation.
- **Suggested fix:** Use `ArcSwap::compare_exchange`/`rcu` (or the sanctioned rare-admin `parking_lot::Mutex` pattern with an inline contention comment) so check+store is atomic; make `try_finalize` refresh the atomic mirror too; add a concurrency test for rotate/finalize interleavings.

### 5. Known-user challenge exposes per-user KDF params — residual enumeration channel for users below current defaults
- **File:line:** `crates/shamir-connect/src/server/handshake.rs:154-159` (effective_kdf selection), `174-185` (`ChallengeView`)
- **Severity:** low (accepted trade-off per spec §13.5 — recorded here so the decision stays visible)
- **Issue:** `challenge()` returns the real user's stored `kdf_params` (plus their salt) for known users and server defaults for unknown ones, and Argon2id wall-time scales with those params. After a server-wide KDF-default bump (the spec §13 upgrade flow), every not-yet-upgraded user is distinguishable from "unknown user" by reading one challenge field — or by timing the KDF phase, since the 50-75 ms padding floor (`common/latency.rs`) cannot mask multi-hundred-ms Argon2id deltas (e.g. 19 MB/t2 vs 128 MB/t4).
- **Failure scenario:** Targeted username enumeration of legacy-parameter accounts following a defaults bump.
- **Suggested fix:** None required if spec §13.5 consciously accepts this; otherwise pad the Argon2id phase to the params-independent worst case and document that callers must size `FIXED_FLOOR_MS` from the server's KDF *minimum*, not its defaults.

### 6. `start_change_password_challenge` accepts an all-zero `client_nonce_cp`
- **File:line:** `crates/shamir-connect/src/server/changepw.rs:64-90`; the all-zero rejection happens only later inside `build_auth_message_cp` (`crates/shamir-connect/src/common/changepw.rs:59-64`)
- **Severity:** nit
- **Issue:** A client submitting a zero nonce gets a pending challenge stored and a `challenge_cp` issued, then deterministically fails at verify — asymmetric with `ServerHandshake::new`, which rejects all-zero nonces at issuance. No replay impact (both nonces are server-stored and single-use).
- **Suggested fix:** Validate `client_nonce_cp` non-zero in `start_change_password_challenge` for symmetry and fail-fast.

### 7. `encode_details_canonical` is a dead placeholder with a broken signature
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:355-361`
- **Severity:** nit
- **Issue:** Takes `&BTreeMap<String, rmp_serde::config::DefaultConfig>` (a serializer config type, not msgpack values) and unconditionally returns `Vec::new()`; zero callers workspace-wide. If anyone "completes" the call site, audit entries would silently carry empty `details_canonical_msgpack` while appearing canonical.
- **Suggested fix:** Delete it, or implement against `rmpv::Value` with a test that the output round-trips.

### 8. `canonical_bytes` length prefixes truncate silently at 255/65535 bytes
- **File:line:** `crates/shamir-connect/src/server/audit_chain.rs:102-113` (`as u8` / `as u16` casts)
- **Severity:** nit
- **Issue:** `transport`/`user`/`ip_subnet`/`result` longer than 255 bytes (or `event` > 65535) corrupt the canonical form's length prefix. The raw bytes still follow, so the HMAC remains collision-safe, but cross-language canonical re-derivation breaks and the `debug_assert_eq!` fires in debug builds.
- **Suggested fix:** Reject over-long fields with an error instead of casting.

</details>
