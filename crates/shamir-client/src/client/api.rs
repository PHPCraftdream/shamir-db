//! Accessors, request helpers and teardown for [`Client`].

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;

thread_local! {
    static REQ_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1024));
}

#[cfg(test)]
pub(crate) fn request_scratch_len_for_test() -> usize {
    REQ_BUF.with(|cell| cell.borrow().len())
}

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

use shamir_connect::common::envelope::RequestEnvelopeRef;
use shamir_connect::common::wire::RotationInProgressPayload;

use shamir_transport_tcp::framing::write_frame;

use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use crate::client::subscription::{SubscriptionHandle, CLIENT_SUB_CHANNEL_CAP};
use crate::client::transport::{await_pending_response, Lifecycle};
use crate::client::Client;
use crate::error::{ClientError, CloseCause};

/// Cancellation-safety guard for a registered pending entry (P0 write-phase):
/// created right after the locked insert; its `Drop` removes the entry IF it
/// is still present — a no-op once the reader resolved/drained it or the
/// request timeout removed it.
struct RegistrationGuard {
    state: Lifecycle,
    rid: u32,
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        self.state.pending.map.remove_sync(&self.rid);
    }
}

/// Cancellation-safety guard for the write phase (P0 C1): armed right after
/// the writer is acquired and the authoritative recheck passed. If the
/// caller's future is DROPPED mid-frame (unexpected cancellation), `Drop`
/// shuts the lifecycle down irreversibly — the connection must never stay
/// reusable after a partially written frame. Every normal exit path defuses
/// the guard first; `Lifecycle::shutdown` is idempotent (first cause wins),
/// so a defused-vs-explicit overlap is harmless.
struct WriteCancelGuard {
    state: Lifecycle,
    cause: std::cell::Cell<Option<CloseCause>>,
}

impl WriteCancelGuard {
    fn new(state: &Lifecycle) -> Self {
        Self {
            state: state.clone(),
            cause: std::cell::Cell::new(Some(CloseCause::Io(Arc::from(
                "write cancelled mid-frame",
            )))),
        }
    }

    /// Normal completion (including the closed_wait branch — the existing
    /// cause wins): prevent the Drop shutdown.
    fn defuse(&self) {
        self.cause.set(None);
    }
}

impl Drop for WriteCancelGuard {
    fn drop(&mut self) {
        if let Some(cause) = self.cause.take() {
            self.state.shutdown(cause);
        }
    }
}

impl Client {
    #[cfg(test)]
    pub(crate) fn request_scratch_len_for_test(&self) -> usize {
        request_scratch_len_for_test()
    }

    /// 32-byte session id assigned by the server.
    pub fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    /// `SHA256(server_ed25519_pub_key)` — persist this for subsequent
    /// connections via `ConnectOptions.trusted_pin`.
    pub fn server_pub_key_pin(&self) -> [u8; 32] {
        self.pinned_hash
    }

    /// Absolute session expiry (unix nanoseconds).
    pub fn expires_at_ns(&self) -> u64 {
        self.expires_at_ns
    }

    /// Resumption ticket bytes (if the server issued one). Persist
    /// per-server to skip Argon2id on the next connection.
    pub fn resumption_ticket(&self) -> Option<&[u8]> {
        self.resumption_ticket.as_deref().map(Vec::as_slice)
    }

    /// Resumption expiry (paired with [`Self::resumption_ticket`]).
    pub fn resumption_expires_at_ns(&self) -> Option<u64> {
        self.resumption_expires_at_ns
    }

    /// Max query-language version advertised by the server during the
    /// handshake (`auth_ok.server_query_version` or
    /// `resume_ok.server_query_version`). Returns `0` when the server
    /// did not send the field (pre-v2 server) — treat as v1.
    ///
    /// Use this to gate v2 id-keyed write/read: emit v2 protocol only when
    /// `server_query_version() >= 2`.
    pub fn server_query_version(&self) -> u8 {
        self.server_query_version.load(Ordering::Relaxed)
    }

    /// `auth_ok.rotation_in_progress` (spec §6.5), when the server sent one.
    /// Surfaced as a hint only; no identity recovery is performed here.
    pub fn rotation_in_progress(&self) -> Option<&RotationInProgressPayload> {
        self.rotation_in_progress.as_ref()
    }

    /// `auth_ok.kdf_upgrade_required` (spec §13): server asks for a password
    /// change to upgrade the KDF.
    pub fn kdf_upgrade_required(&self) -> bool {
        self.kdf_upgrade_required
    }

    /// Register a subscription and get a handle to receive push frames.
    ///
    /// The caller must have already sent a subscribe request to the server
    /// and obtained the `sub_id`. Push frames arriving for this `sub_id`
    /// will be routed to the returned handle.
    ///
    /// On a closed client, returns an already-ended handle (empty stream,
    /// [`SubscriptionHandle::termination`] carrying the close cause).
    pub fn subscribe_push(&self, sub_id: u64) -> SubscriptionHandle {
        let (tx, rx) = tokio::sync::mpsc::channel(CLIENT_SUB_CHANNEL_CAP);
        let (term_tx, term_rx) = tokio::sync::oneshot::channel();
        {
            let mut subs = self.state.subs.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(cause) = subs.closed.clone() {
                let _ = term_tx.send(cause);
            } else {
                subs.live.insert(sub_id, tx.clone());
                subs.terminal.insert(sub_id, term_tx);
                if let Some(buffered) = subs.early.remove(&sub_id) {
                    subs.early_total -= buffered.len();
                    for envelope in buffered {
                        let _ = tx.try_send(envelope);
                    }
                }
                debug_assert_eq!(
                    subs.early_total,
                    subs.early.values().map(Vec::len).sum::<usize>()
                );
            }
        }
        SubscriptionHandle::new(sub_id, rx, tx.downgrade(), self.state.subs.clone(), term_rx)
    }

    /// Health check.
    pub async fn ping(&self) -> Result<(), ClientError> {
        match self.roundtrip(&DbRequest::Ping).await? {
            DbResponse::Pong => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected Pong, got {other:?}"
            ))),
        }
    }

    /// Execute a [`BatchRequest`] against the named database.
    ///
    /// Ambient interner epoch-delta sync (Stage 5-wire Part A): before sending,
    /// the client advertises its per-repo interner epoch for every distinct
    /// repo the batch targets (so the server can attach a delta); after the
    /// response, any `interner_delta` entries are merged into the cache
    /// (`insert_entry` + `set_epoch` CAS-max). This is transparent — a
    /// backward-compatible server leaves `interner_delta` empty → no-op.
    pub async fn execute(
        &self,
        db: &str,
        mut batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        // Synthesize absent completeness cursors only for v2 peers.
        if should_advertise_interner_epochs(
            self.server_query_version(),
            !batch.interner_epochs.is_empty(),
        ) {
            for repo in shamir_query_types::batch::distinct_repos(&batch.queries) {
                let epoch = self.interner_cache().get_or_create(db, &repo).epoch();
                batch.interner_epochs.insert(repo, epoch);
            }
        }

        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.to_string(),
            batch,
        };
        let response = match self.roundtrip(&req).await? {
            DbResponse::Batch { response } => response,
            other => {
                return Err(ClientError::Protocol(format!(
                    "expected Batch, got {other:?}"
                )))
            }
        };

        // AFTER receive: merge interner_delta into the cache. Ids come ONLY
        // from the server (§9.4). The merge mirrors dump/refresh's apply:
        // insert_entry (idempotent) + set_epoch (CAS-max).
        if !response.interner_delta.is_empty() {
            self.merge_interner_delta(db, &response);
        }

        Ok(response)
    }

    /// Create a SCRAM-authenticatable user. Requires the current
    /// session to be a superuser (server enforces). Returns the
    /// stable 16-byte `user_id`.
    ///
    /// The HMAC confirmation tag required by the server (task #604) is
    /// computed automatically from `self.session_id` — callers don't need
    /// to know about the HMAC plumbing.
    pub async fn create_scram_user(
        &self,
        name: &str,
        password: Zeroizing<String>,
        roles: Vec<String>,
    ) -> Result<Vec<u8>, ClientError> {
        let tag = {
            let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
            let canonical = shamir_query_types::hmac::canonical_create_scram_user(name, &roles);
            shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
        };
        let req = DbRequest::CreateScramUser {
            name: name.to_string(),
            password: password.as_str().to_owned().into(),
            roles,
            hmac: Some(tag),
        };
        let result = self.roundtrip(&req).await;
        // Wipe the cleartext password copy placed into the request
        // immediately after the roundtrip, rather than waiting for `req`'s
        // natural end-of-function drop. (The caller's `password` is
        // `Zeroizing` and wipes on its own drop.) `SecretString` already
        // zeroizes its buffer on `Drop` under the `crypto` feature, so
        // dropping `req` here achieves the same "wipe ASAP" intent that the
        // old manual `password.zeroize()` field-reach used to.
        drop(req);
        match result? {
            DbResponse::UserCreated { user_id, .. } => Ok(user_id),
            other => Err(ClientError::Protocol(format!(
                "expected UserCreated, got {other:?}"
            ))),
        }
    }

    /// Grant or revoke the `replicator` role on an existing SCRAM user
    /// (task #921). The `replicator` pseudo-role is reserved — it cannot be
    /// attached through [`create_scram_user`]'s generic `roles` array; the
    /// server explicitly rejects it there and requires this dedicated
    /// [`DbRequest::SetReplicator`] wire op instead (mirrors
    /// `SetSuperuser`'s shape and unconditional-HMAC gate exactly).
    ///
    /// Requires the current session to belong to a superuser (server enforces,
    /// checked before the HMAC gate). The confirmation tag is computed from
    /// `self.session_id` — callers don't need to know about the HMAC plumbing.
    pub async fn set_replicator(&self, user: &str, on: bool) -> Result<(), ClientError> {
        let tag = {
            let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
            let canonical = shamir_query_types::hmac::canonical_set_replicator(user, on);
            shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
        };
        let req = DbRequest::SetReplicator {
            user: user.to_string(),
            on,
            hmac: Some(tag),
        };
        match self.roundtrip(&req).await? {
            DbResponse::ReplicatorSet { .. } => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected ReplicatorSet, got {other:?}"
            ))),
        }
    }

    /// Replication pull-API (REPLICATION §5.1/§5.2/§5.4). Sends a
    /// [`DbRequest::Repl`] carrying a [`ReplRequest`] (`Hello` or `Pull`) and
    /// returns the matched [`ReplResponse`].
    ///
    /// This is the thin wire entry point used by the wire-side
    /// `ReplSource` implementation (R1-c) and by external CDC consumers that
    /// speak the privileged replication protocol. The caller must hold a
    /// session with the `replicator` role (or be a superuser) — the server
    /// enforces this inside `handle_repl` and returns
    /// `ReplResponse::Error { code: "bad_role" }` otherwise.
    pub async fn repl(&self, req: ReplRequest) -> Result<ReplResponse, ClientError> {
        match self.roundtrip(&DbRequest::Repl(req)).await? {
            DbResponse::Repl(r) => Ok(r),
            other => Err(ClientError::Protocol(format!(
                "expected Repl, got {other:?}"
            ))),
        }
    }

    /// Open a server-side cursor (FG-5a/FG-5b) over `query` and return an
    /// idiomatic [`futures::Stream`] of records — see
    /// [`crate::cursor_stream::CursorStream`] for the full contract
    /// (pagination, error propagation, and the cleanup tradeoff around
    /// dropping the stream without calling `close()`).
    pub fn stream_cursor<'a>(
        &'a self,
        db: &str,
        query: shamir_query_types::read::ReadQuery,
        page_size: u32,
    ) -> crate::cursor_stream::CursorStream<'a> {
        crate::cursor_stream::CursorStream::new(self, db, query, page_size)
    }

    /// Poll a DDL operation's status by its operation ID.
    ///
    /// Returns the full `DdlOpStatus` if found, `None` if the operation is unknown
    /// (GC'd, never existed, or a pre-RFC op that had no op_id).
    ///
    /// This is the mechanism that allows a client to learn whether a crashed-and-recovered
    /// DDL op actually completed after a server restart.
    pub async fn get_ddl_op_status(
        &self,
        db: &str,
        repo: &str,
        table: &str,
        op_id: &str,
    ) -> Result<Option<shamir_query_types::read::DdlOpStatus>, ClientError> {
        let req = DbRequest::GetDdlOpStatus {
            db: db.to_string(),
            repo: repo.to_string(),
            table: table.to_string(),
            op_id: op_id.to_string(),
        };
        match self.roundtrip(&req).await? {
            DbResponse::DdlOpStatus { status } => Ok(status),
            DbResponse::Error { code, message } => {
                // Treat `not_supported` as "feature unavailable" rather than a hard error.
                // Old servers don't understand `GetDdlOpStatus` and return this code.
                if code == "not_supported" {
                    return Err(ClientError::Protocol(format!(
                        "GetDdlOpStatus not supported by server: {}",
                        message
                    )));
                }
                Err(ClientError::Db {
                    code: code.clone(),
                    message: message.clone(),
                })
            }
            other => Err(ClientError::Protocol(format!(
                "expected DdlOpStatus, got {other:?}"
            ))),
        }
    }

    /// Test-only seam (P0 C1 write-phase): take and HOLD the write mutex so
    /// a spawned request parks on writer acquisition. The test drops the
    /// returned guard to release the writer.
    #[cfg(test)]
    pub(crate) async fn hold_writer_for_test(
        &self,
    ) -> tokio::sync::MutexGuard<'_, crate::client::transport::WriteSink> {
        self.write.lock().await
    }

    /// Send a request and route the response via the rid-demux pending map.
    ///
    /// 1. Allocate rid and register the oneshot **before** writing (no race
    ///    with a fast server response).
    /// 2. Take the write mutex only for the duration of `write_frame`.
    /// 3. Await the oneshot; the reader task delivers the result.
    ///
    /// `pub(crate)` (rather than private) so sibling modules — e.g.
    /// `cursor_stream`'s `CursorStream` — can drive their own
    /// request/response cycles without duplicating this primitive.
    pub(crate) async fn roundtrip(&self, req: &DbRequest) -> Result<DbResponse, ClientError> {
        // FAST PATH ONLY: `closed_hint` is not authoritative; the locked
        // recheck below is the authoritative closed check.
        if self.state.closed_hint.load(Ordering::Acquire) {
            return Err(ClientError::ConnectionClosed(
                self.state.cause().unwrap_or(CloseCause::ReaderCancelled),
            ));
        }
        #[cfg(test)]
        if let Some(hook) = &self.hooks.after_fast_check {
            hook.fire().await;
        }

        // T-cl-1 + RV-CLIENT-E16A04E5: thread-local scratch buffer for
        // request serialisation, reused across calls. The buffer's capacity
        // grows to the high-water mark; the request bytes stay IN the
        // thread-local (no per-call clone) and the borrowed
        // `RequestEnvelopeRef` encodes the envelope directly against them —
        // zero copies of `session_id` or the request body. The whole encode
        // happens inside the `with` closure so the `RefCell` borrow is
        // released before any `.await`.
        let rid = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let envelope_bytes: Zeroizing<Vec<u8>> =
            REQ_BUF.with(|cell| -> Result<Zeroizing<Vec<u8>>, ClientError> {
                let mut buf = cell.borrow_mut();
                buf.zeroize();
                let serialized = rmp_serde::encode::write_named(&mut *buf, req);
                if let Err(error) = serialized {
                    buf.zeroize();
                    return Err(ClientError::Encode(error));
                }
                let envelope = RequestEnvelopeRef {
                    session_id: &self.session_id,
                    request_id: Some(rid),
                    req: &buf,
                };
                let encoded = envelope
                    .to_msgpack()
                    .map_err(|e| ClientError::Protocol(format!("envelope encode: {e}")));
                buf.zeroize();
                Ok(Zeroizing::new(encoded?))
            })?;

        // Register BEFORE writing — avoid a race where the server sends the
        // response before we reach the oneshot registration. Close-vs-register
        // without a client-side mutex (RV-CLIENT-B80C0D09): shutdown publishes the
        // cause BEFORE draining, so an insert that landed before the drain
        // is resolved by it; an insert that lands after the drain re-reads
        // the cause and unwinds its own entry (P0 RV-CLIENT-B42B8576).
        let (tx, rx) = oneshot::channel();
        let registration_guard = {
            let reg = &self.state.pending;
            if let Some(cause) = reg.closed.cause() {
                return Err(ClientError::ConnectionClosed(cause));
            }
            let _ = reg.map.insert_sync(rid, tx);
            // Close-vs-register recheck: an insert that landed after
            // shutdown's drain must not strand a waiter — remove our entry
            // and fail with the published cause.
            if let Some(cause) = reg.closed.cause() {
                reg.map.remove_sync(&rid);
                return Err(ClientError::ConnectionClosed(cause));
            }
            RegistrationGuard {
                state: self.state.clone(),
                rid,
            }
        };
        #[cfg(test)]
        if let Some(hook) = &self.hooks.after_register {
            hook.fire().await;
        }

        // Send the request. Writer acquisition and the write itself are BOTH
        // raced against closure (watch channel), so a shutdown while we are
        // blocked on either unblocks us instead of hanging (P0 write-phase).
        let write_result = {
            let mut write = tokio::select! {
                w = self.write.lock() => w,
                cause = self.state.closed_wait() => {
                    return Err(ClientError::ConnectionClosed(cause));
                }
            };
            // Authoritative recheck AFTER acquiring the writer: the fast
            // path above is only a hint.
            if let Some(cause) = self.state.pending.closed.cause() {
                return Err(ClientError::ConnectionClosed(cause));
            }
            #[cfg(test)]
            if let Some(hook) = &self.hooks.after_writer_acquired {
                hook.fire().await;
            }
            // Mid-frame cancellation safety: only an UNEXPECTED drop of this
            // future leaves the guard armed — every resolved arm defuses it.
            let cancel_guard = WriteCancelGuard::new(&self.state);
            tokio::select! {
                // Biased: a pending closure beats a write onto a dying
                // connection, so a released hook never emits wire bytes.
                biased;
                cause = self.state.closed_wait() => {
                    cancel_guard.defuse();
                    drop(write);
                    return Err(ClientError::ConnectionClosed(cause));
                }
                r = write_frame(&mut *write, &envelope_bytes) => {
                    // On Err the shared shutdown(Io) below runs with the real
                    // transport cause — the guard must not double-close.
                    cancel_guard.defuse();
                    r.map(|_| ())
                }
            }
        };
        if let Err(e) = write_result {
            // Real transport failure: run the shared shutdown FIRST (so the
            // lifecycle records the Io cause and drains all waiters), then
            // remove OUR entry. If it is already gone, the drain resolved it
            // — surface the drained result; otherwise the transport error.
            let cause: Arc<str> = Arc::from(format!("send: {e}"));
            self.state.shutdown(CloseCause::Io(cause));
            let removed = self.state.pending.map.remove_sync(&rid).map(|(_, tx)| tx);
            if removed.is_none() {
                let payload = await_pending_response(rx, None, &self.state, rid).await?;
                let response: DbResponse = rmp_serde::from_slice(&payload)?;
                return crate::visibility::map_response(response);
            }
            return Err(ClientError::transport("send", e));
        }

        // Await our oneshot.  The reader task delivers Ok(payload) or Err.
        // When `request_timeout` is set, bound the wait: a server that
        // accepts the connection but never answers this rid must not hang
        // the caller forever. `None` preserves the original unbounded await.
        let payload = await_pending_response(rx, self.request_timeout, &self.state, rid).await?;

        drop(registration_guard);
        let response: DbResponse = rmp_serde::from_slice(&payload)?;
        crate::visibility::map_response(response)
    }

    /// Close the TLS write half cleanly and abort the reader task.
    ///
    /// After this call the session is dead on the server side too
    /// (TCP close → session evicted).
    pub async fn close(mut self) {
        // Terminal transition FIRST: every pending request fails with
        // `LocalClose` and every subscription handle ends BEFORE the reader
        // is torn down, so no consumer can outlive its cause.
        self.state.shutdown(CloseCause::LocalClose);
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        let mut w = self.write.lock().await;
        let _ = AsyncWriteExt::shutdown(&mut *w).await;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Terminal transition BEFORE aborting the reader (P0 C1): Drop must
        // close subscriptions and fail pending requests with ClientDropped.
        self.state.shutdown(CloseCause::ClientDropped);
        // §B21: abort the reader task when the Client drops without an
        // explicit `close()` call so the task does not outlive the Client.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
    }
}

/// Version gate for ambient interner epoch work; smart pre-touch remains unconditional.
pub(crate) fn should_advertise_interner_epochs(server_version: u8, has_epochs: bool) -> bool {
    server_version >= 2 && !has_epochs
}
