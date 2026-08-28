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
