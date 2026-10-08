<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-connect — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Security-policy asymmetry and residual-memory hygiene gaps are confirmed with scoped reachability. The crypto wrappers make sound narrow primitive choices, not blanket timing or canonicality guarantees. Canonical audit prefix truncation genuinely permits encoding ambiguity.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 7 | 0 | 0 | 1 | 0 | 3 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Public dispatch_request silently skips the post-auth rate limit

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A valid stored sid sent through owned dispatch reaches the handler regardless of a depleted bucket. The shipped server uses view dispatch, so present production requests do not take this bypass.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/dispatch.rs#L100); [crates/shamir-connect/src/server/dispatch.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/dispatch.rs#L153); [crates/shamir-server/src/connection/request_loop.rs:340](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L340).

<a id="review-2"></a>

### Claim 2 — Long-lived server secrets are plain arrays outside the zeroization policy

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

ServerSecrets and ResumeConfig store freely copied raw arrays with no wiping Drop. Debug redaction exists. A memory-disclosure capability is needed; wiping on drop cannot protect live secrets or every compiler-created copy.

Evidence: [crates/shamir-connect/src/server/config.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/config.rs#L26); [crates/shamir-connect/src/server/config.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/config.rs#L29); [crates/shamir-connect/src/server/resume.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L130).

<a id="review-3"></a>

### Claim 3 — Client password buffers are not zeroized on early-error paths

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Validation/derivation returns precede wipes, leaving caller-owned bytes intact. This is hygiene divergence, not remotely readable password storage by itself.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/client/bootstrap.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/bootstrap.rs#L93); [crates/shamir-connect/src/client/changepw.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/changepw.rs#L60).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — ServerIdentityState::rotate / try_finalize are non-atomic check-then-act

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Concurrent snapshot stores can lose identity changes; stale finalization can separate the installed key from its version mirror. Tickets matching the mirror may still pass, so universal resume rejection is false.

Evidence: [crates/shamir-connect/src/server/rotation.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/rotation.rs#L169); [crates/shamir-connect/src/server/rotation.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/rotation.rs#L194); [crates/shamir-connect/src/server/rotation.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/rotation.rs#L100); [crates/shamir-connect/src/server/resume.rs:290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L290).

Grouping/duplicate: [concurrency-lockfree.md#7](concurrency-lockfree.md#review-7). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Known-user challenge exposes per-user KDF params — accepted residual enumeration

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Different stored/current tuples are directly distinguishable, but AUTH §13.5 explicitly accepts this distinction. Client Argon2 timing and server response padding are separate mechanisms.

Evidence: [crates/shamir-connect/src/server/handshake.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/handshake.rs#L155); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:877](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L877).

<a id="review-6"></a>

### Claim 6 — start_change_password_challenge accepts an all-zero client_nonce_cp

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Issuance stores zero; canonical proof construction later rejects it and consumes the challenge. This wastes a challenge but establishes no replay/authentication bypass.

Evidence: [crates/shamir-connect/src/server/changepw.rs:74](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/changepw.rs#L74); [crates/shamir-connect/src/common/changepw.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/changepw.rs#L59).

<a id="review-7"></a>

### Claim 7 — encode_details_canonical is a dead placeholder with a broken signature

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The public helper ignores its map and emits no bytes; no live caller exercises it.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:355](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L355).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — canonical_bytes length prefixes truncate silently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Public arbitrary fields are narrowed to u8/u16/u32 without rejection. Oversized adjacent fields can produce the same canonical input; normal production field producers do not establish a reachable oversized-network exploit.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L102); [crates/shamir-connect/src/server/audit_chain.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L104); [crates/shamir-connect/src/server/audit_chain.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L113).

<a id="review-8-guarantee"></a>

### Claim 8.guarantee — Raw bytes preserve HMAC collision-safety and debug_assert catches oversized fields

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Transport containing 256 NUL bytes with empty user, and empty transport with user containing those bytes, encode identical adjacent segments. The assertion compares total sizes computed from full lengths and succeeds. This is non-injective encoding, not a broken HMAC primitive.

Evidence: [crates/shamir-connect/src/server/audit_chain.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L91); [crates/shamir-connect/src/server/audit_chain.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L104); [crates/shamir-connect/src/server/audit_chain.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L106); [crates/shamir-connect/src/server/audit_chain.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/audit_chain.rs#L116).

<a id="review-summary-crypto-core"></a>

### Claim Summary.crypto-core — Constant-time comparisons, unconditional fake derivation, strict verification, and no unsafe

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Scoped source checks support these choices. Exact subtle 2.6.1 fixed-width comparison and ed25519-dalek 2.2.0 strict verification were inspected; neither proves identical complete-flow timing. Published sources: https://docs.rs/crate/subtle/2.6.1/source/src/lib.rs and https://docs.rs/crate/ed25519-dalek/2.2.0/source/src/verifying.rs.

Evidence: [crates/shamir-connect/src/common/crypto.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L131); [crates/shamir-connect/src/common/crypto.rs:254](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L254); [crates/shamir-connect/src/server/handshake.rs:146](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/handshake.rs#L146); [Cargo.lock:1199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1199); [Cargo.lock:4000](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4000).

<a id="review-summary-injection"></a>

### Claim Summary.injection — Opaque request bodies and exact-match directory lookups

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Connect forwards opaque bytes and its reference directory uses keyed lookup. This scoped observation does not audit arbitrary application handlers.

Evidence: [crates/shamir-connect/src/server/dispatch.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/dispatch.rs#L163); [crates/shamir-connect/src/server/admin.rs:317](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/admin.rs#L317).

## Evidence and recipe corrections

- No active server identity rotate caller was found; do not infer an exposed unauthenticated rotation attack.
- The real/fake path uses an ordinary branch and identical selected primitive operations; unconditional fake derivation does not prove whole-path constant-time execution.
- The wrapper's public-key-canonicality wording exceeds the established guarantee. ed25519-dalek 2.2.0 from_bytes decompresses the supplied encoding and verify_strict rejects small-order points; no explicit public-key re-encoding equality check is present in the wrapper.
- Audit fields require release-enforced representability checks; a debug assertion is neither a size validator nor a production remedy.

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
