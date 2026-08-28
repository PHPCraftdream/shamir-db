# shamir-client -- Error handling & resource lifecycle

## Summary

The crate's `Result`/`thiserror` discipline is broadly sound: a single `ClientError` enum, `?` propagation throughout, poison-tolerant `unwrap_or_else(|p| p.into_inner())` locking everywhere, `Zeroizing` on the password paths (including every exit of the `spawn_blocking` closure), and deliberate error-path resource cleanup that is actually tested (pending-map cleanup on write failure and request timeout, reader-drain on EOF, best-effort `CancelCursor` on mid-pagination error). The weakest spots are (1) a check-then-act race between `roundtrip`'s `closed` flag read and the reader task's shutdown drain that can hang a caller forever under the default unbounded `request_timeout`, (2) push-subscription consumers that also hang forever after connection loss because the reader never releases the subscription senders, (3) `dump_repo` returning `Ok(())` even when the dump failed, and (4) `resume()` silently discarding all timeout protection. Several error-shaping paths are dead or untestable as written (`get_ddl_op_status`'s `not_supported` arm, `RequestIdMismatch`), and `Client::resume` has no error-path test coverage in this crate at all.

## Findings

### 1. Race between `roundtrip`'s closed-check and the reader shutdown drain hangs the caller forever under default options
- **File:line:** `crates/shamir-client/src/client.rs:966` (closed check), `:993-998` (pending insert), `:318-326` (reader store+drain)
- **Severity:** high
- **Issue:** `roundtrip` checks `self.closed` at entry, then encodes, allocates a rid, and inserts the oneshot into `pending`. `reader_task`, on EOF/I/O error, stores `closed = true` and then drains `pending`. These are not atomic: if the caller's insert lands **after** the reader's drain, the entry is orphaned — no reader will ever send on the oneshot or drop the sender. With `request_timeout: None` (the documented default, preserving "unbounded-wait behaviour"), `await_pending_response`'s `rx.await` then never resolves and the request task hangs until the `Client` is dropped.
- **Failure scenario:** server restarts / connection drops while a caller issues a request; the caller's insert loses the race with the drain; `client.execute(...).await` (or `ping`, `stream_cursor`, …) hangs permanently with default options. Server restarts are routine; the timeout knobs added in task #520 exist precisely for this class of failure but are `None` by default.
- **Suggested fix:** after inserting into `pending`, re-check `closed` and — to close the residual window — do the insert + re-check while holding the `pending` mutex, and have `reader_task` store `closed` while holding that same mutex before draining. Alternatively, bound the await by a non-`None` default deadline.

### 2. Push-subscription consumers hang forever after connection loss; `subscribe_push` ignores the closed flag
- **File:line:** `crates/shamir-client/src/client.rs:318-326` (reader exit does not touch subscriptions), `:722-750` (`subscribe_push` has no `closed` check); `crates/shamir-client/src/subscription.rs:54-56`
- **Severity:** medium
- **Issue:** on reader exit, `pending` is drained but the `subscriptions` registry is not: the `mpsc::Sender` clones stay alive (held by the registry `TFxMap` and the `Client`), so a consumer blocked in `SubscriptionHandle::next()` never observes closure — `rx.recv()` pends forever. Likewise `subscribe_push` on a dead connection happily registers a new channel nobody will ever feed. `roundtrip` guards with `closed`; the subscription path is the one data-plane surface without an equivalent.
- **Failure scenario:** connection drops mid-session; a live subscription consumer's `next().await` hangs permanently (no `None`, no error), and any subsequently created handle hangs identically.
- **Suggested fix:** on reader exit, drain/clear the `subscriptions` map (dropping the senders so `recv()` yields `None`), and make `subscribe_push` return an already-closed handle (or error) when `closed` is set.

### 3. `dump_repo` swallows every failure and returns `Ok(())`
- **File:line:** `crates/shamir-client/src/interner_cache_ops.rs:191-223`
- **Severity:** medium
- **Issue:** the public `dump_repo(&self, ...) -> Result<(), ClientError>` maps both the roundtrip failure and the payload-parse failure to a `tracing::warn!` and always resolves `Ok(())`. The signature promises what it cannot deliver: a caller doing `client.dump_repo(db, repo).await?` cannot detect that the dump failed. The swallow is deliberate (keeps the `OnceCell` uninitialized so a later call retries — see the inline comment), but the `Result` return then lies.
- **Failure scenario:** server briefly unreachable at first use; `dump_repo` returns `Ok(())`; the cache stays empty; the failure surfaces much later as `encode_record_idmsgpack`'s misleading `"field 'x' not in FieldMap — touch_fields must be called first"` even though the caller did warm the cache — or as silent `resolve_field() == None`.
- **Suggested fix:** keep the retry semantics without the lie: use `tokio::sync::OnceCell::get_or_try_init` (stays uninitialized on `Err`, propagates the error to the first caller while concurrent waiters still share the attempt), or store a `Result`/last-error in the cell and return it. Add the missing error-path test (failed dump leaves `is_populated() == false` and a retry succeeds).

### 4. `Client::resume` silently drops all timeout protection
- **File:line:** `crates/shamir-client/src/client.rs:95-105` (`ResumeOptions` has no timeout fields), `:596-598` (connect unbounded), `:675` (`request_timeout: None` hardcoded)
- **Severity:** medium
- **Issue:** `ConnectOptions` grew `connect_timeout`/`request_timeout` (task #520), but `ResumeOptions` carries neither and `resume()` hardcodes unbounded waits for both the TLS connect and every subsequent request. The comment ("resumption preserves the prior unbounded-wait connect behaviour") documents the choice but the asymmetry is a trap: a caller who carefully bounded its initial connection gets an unbounded client after ticket resumption, with no compile-time or run-time signal.
- **Failure scenario:** TOFU connect with `request_timeout = Some(5s)`; server later degrades to accept-but-never-answer; `Client::resume` + `ping()` hangs forever where the original path would have returned `RequestTimeout`.
- **Suggested fix:** add `connect_timeout`/`request_timeout` to `ResumeOptions` (defaulting to `None` for compatibility), mirroring `ConnectOptions`.

### 5. Stringly-typed `Handshake`/`Tls`/`Transport`/`Protocol` variants erase typed sources
- **File:line:** `crates/shamir-client/src/error.rs:9-21`; conversion sites `client.rs:388, 403, 417, 422, 455, 466, 471, 537`
- **Severity:** medium
- **Issue:** `shamir-connect` produces a typed error enum (`Error::ServerAuthFailed`, `Error::ServerIdentityChanged`, `Error::ServerSignatureInvalid` — see `crates/shamir-connect/src/client/handshake.rs:261-293`), but the SDK flattens it via `.map_err(|e| ClientError::Handshake(e.to_string()))`. Per CLAUDE.md's thiserror discipline ("`#[from]` where natural"), these should preserve the source: programmatic handling (e.g. surfacing "server identity changed — possible MITM" differently from "bad password") currently requires matching on formatted English strings.
- **Failure scenario:** an SDK consumer writing `matches!(err, ClientError::Handshake(_))` + `contains("ServerIdentityChanged")` breaks on any wording change; no `source()` chain for diagnostics.
- **Suggested fix:** add `#[error("handshake: {0}")] Handshake(#[from] shamir_connect::client::Error)` (and analogous typed variants or `#[source]` fields for TLS/transport), keeping the `String` forms only for genuinely unstructured text.

### 6. Dead `DbResponse::Error` arm in `get_ddl_op_status`; the `not_supported` special case is unreachable
- **File:line:** `crates/shamir-client/src/client.rs:935-948` (dead arm), vs `:1018-1024` (`roundtrip` already converts `DbResponse::Error` → `ClientError::Db`)
- **Severity:** low
- **Issue:** `roundtrip` returns `Err(ClientError::Db { .. })` for any in-band `DbResponse::Error` and never `Ok(DbResponse::Error)`, so `get_ddl_op_status`'s `DbResponse::Error { code, message } =>` arm can never match. The intended shaping — old servers' `not_supported` becoming a distinguishable `ClientError::Protocol` — never fires (callers see `ClientError::Db { code: "not_supported" }` instead), and the comment ("Treat `not_supported` as 'feature unavailable' rather than a hard error") misleads since both branches return `Err` anyway.
- **Failure scenario:** none beyond misshaped error taxonomy; but any future editor "fixing" behaviour against the dead arm wastes effort, and clippy cannot flag a semantically-unreachable match arm.
- **Suggested fix:** match on `Err(ClientError::Db { code, message })` from `roundtrip` for the `not_supported` reshaping (or delete the arm and fix the doc comment).

### 7. Push-subscription `next()` and reader exit: no error-path tests; `subscribe_push` on a closed client untested
- **File:line:** `crates/shamir-client/src/tests/` (absent), finding 2's paths
- **Severity:** low
- **Issue:** the demux tests are otherwise exemplary (EOF drain, garbage frames, error envelopes, late responses, bounded channel, handle-drop registry cleanup), but there is no test for the post-connection-loss subscription lifecycle (currently a hang — untestable without first fixing finding 2), nor one asserting `roundtrip` returns `ConnectionClosed` when `closed` is already set (`client.rs:966-968`).
- **Failure scenario:** regressions in findings 1/2 land silently; the closed-flag guard at `roundtrip` entry could be removed without any test failing.
- **Suggested fix:** after fixing finding 2, add: (a) reader-exit → `next()` yields `None`; (b) `roundtrip` on a closed client → `Err(ConnectionClosed)`; (c) `subscribe_push` on a closed client → closed handle.

### 8. Cancellation path leaks the pending entry until response or connection death
- **File:line:** `crates/shamir-client/src/client.rs:993-1016`
- **Severity:** low
- **Issue:** the write-failure path (`:1004-1009`) and the `RequestTimeout` path (`await_pending_response`, `:188-195`) both remove the pending entry, but there is no cleanup for caller-side cancellation (dropping the future via an outer `tokio::time::timeout`/`select!`/task abort after registration). The oneshot sender then sits in `pending` until the server's response arrives (reader removes it) or the connection dies (drain). On a long-lived connection repeatedly cancelling against a stalled rid, entries accumulate for the connection's lifetime.
- **Failure scenario:** unbounded (per-connection) memory growth of dead senders; each entry also consumes a monotonic rid — harmless, but the map drift masks genuine in-flight state when debugging.
- **Suggested fix:** scope-guard the insert (remove-on-drop if still present), mirroring the write-failure cleanup.

### 9. `.expect()` in library code rests on an unenforced cross-crate invariant
- **File:line:** `crates/shamir-client/src/client.rs:539-542`; contract in `crates/shamir-connect/src/client/handshake.rs:149-151, 266-276`
- **Severity:** low
- **Issue:** `pin_capture.lock().expect("either trusted_pin pre-set or TOFU callback fired")` is currently unreachable: `build()` rejects `pinned_hash == None && !accept_new_host`, and `process_auth_ok` fires the callback exactly when `pinned_hash == None`, before any later `Err`. But the invariant lives in another crate's private control flow and is not statically enforced; a future handshake mode returning `Ok` without invoking the callback would turn user input into a panic in this library. CLAUDE.md reserves panics for genuine programmer-bug invariants.
- **Suggested fix:** `.ok_or_else(|| ClientError::Handshake("pin capture missing after auth_ok".into()))?` — same semantics, no panic path, robust to upstream drift.

### 10. `touch_fields` silently omits names the server failed to map
- **File:line:** `crates/shamir-client/src/interner_cache_ops.rs:260-264, 280-283`
- **Severity:** low
- **Issue:** both return paths use `filter_map(|n| fm.id_of(n) ...)`: if the server's response omits a mapping for a name the client explicitly touched (a protocol violation), the missing name is dropped from the result with no error, and the failure resurfaces later as finding-3's misleading encode error. The early-return path (`unknown.is_empty()`) is fine by construction; the post-roundtrip path is not.
- **Suggested fix:** after merging, verify every input name resolves; return `ClientError::Protocol("interner_touch: server returned no mapping for '<name>'")` otherwise. Add an error-path test.

### 11. Undecodable response frames are dropped at `debug` level; default-unbounded waiters hang
- **File:line:** `crates/shamir-client/src/client.rs:121-135` (`decode_frame` → `None`), `:236-278` (log-and-`continue`)
- **Severity:** low
- **Issue:** a frame that decodes as neither `ResponseEnvelope` nor `ErrorEnvelope` nor `PushEnvelope` is dropped with a `tracing::debug!` (demux_tests proves alignment survives). Defensible for frame alignment — but if that frame was the response to an outstanding rid, its waiter hangs forever under the default `request_timeout: None`, and the only trace is a debug-level line.
- **Suggested fix:** at minimum raise the log to `warn` with the frame length; consider a counter that escalates to a fatal protocol error (mark closed + drain with `ClientError::Protocol`) after repeated decode failures on a live connection.

### 12. `ClientError::RequestIdMismatch` is never constructed
- **File:line:** `crates/shamir-client/src/error.rs:27-29`
- **Severity:** nit
- **Issue:** the variant is dead — the reader routes strictly by rid and drops rid-less frames (`client.rs:291-298`), so a "mismatch" can never be reported. Dead taxonomy misleads API consumers into handling a case that cannot occur (or, worse, assuming rid-correlation errors surface at all — they are silently dropped per `demux_late_response_for_unknown_rid_is_dropped`).
- **Suggested fix:** remove the variant, or wire it up if response-side rid validation is ever added.

### 13. `ResumeOptions::ticket` and its wire copy are not `Zeroizing`
- **File:line:** `crates/shamir-client/src/client.rs:100-101, 616-620`; contrast `:347, 564` (`resumption_ticket: Option<Zeroizing<Vec<u8>>>`)
- **Severity:** nit
- **Issue:** the crate treats the resumption ticket as a secret everywhere except the `resume()` input path: `ResumeOptions::ticket: Vec<u8>` is moved into `WireResumeInit` and dropped un-wiped (the serialized copy in the request buffer likewise), while the Client's own copy is `Zeroizing`. Same-lifecycle inconsistency for the same credential. (`create_scram_user`'s reliance on `SecretString`'s conditional `crypto`-feature wipe at `client.rs:834-842` is similarly feature-gated but documented.)
- **Suggested fix:** `pub ticket: Zeroizing<Vec<u8>>` in `ResumeOptions` (napi/TS wrappers already copy bytes, so the breaking change is contained).

## Error-path test coverage notes (for the record)

Covered well: connect/request timeout fire + pending cleanup (`src/tests/timeout_tests.rs`); reader EOF drain, error-envelope routing, garbage/rid-less/late frames, bounded push channel, handle-drop registry cleanup (`src/tests/demux_tests.rs`); cursor close-mid-stream, mid-pagination best-effort cancel, idle-timeout eviction, as-of rejection as `ClientError::Db` (also exercises `roundtrip`'s `DbResponse::Error` conversion indirectly) (`src/tests/cursor_stream_tests.rs`, `tests/cursor_lifecycle_e2e.rs`, `tests/cursor_stream.rs`); iteration-error rollback (`tests/batch_for_each_e2e.rs:475`); circular-dependency error (`tests/batch_sequencing_e2e.rs:331`).

Gaps: `Client::resume` has zero coverage in this crate (`src/tests/resume_wire_tests.rs` is serde-only; the single happy-path resume lives in `shamir-server/tests/duplex_e2e.rs`) and no rejected/rotated-ticket test anywhere; `dump_repo`/`refresh_repo` failure propagation untested (finding 3); post-loss subscription lifecycle untested (finding 7); handshake `Protocol` size-validation paths (`client.rs:424-493`) untested directly; `get_ddl_op_status`'s `not_supported` path is untestable as written (dead arm, finding 6).
