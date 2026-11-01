//! Accessors, request helpers and teardown for [`Client`].

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use zeroize::{Zeroize, Zeroizing};

use shamir_connect::common::envelope::RequestEnvelopeRef;
use shamir_connect::common::wire::RotationInProgressPayload;

use shamir_transport_tcp::framing::write_frame_prereserved;

use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use crate::client::subscription::{SubscriptionHandle, CLIENT_SUB_CHANNEL_CAP};
use crate::client::transport::{await_pending_response, Lifecycle};
use crate::client::Client;
use crate::error::{ClientError, CloseCause};

thread_local! {
    /// Card RV-TRANSPORT-TCP-79910BA5: the scratch holds the FRAMED,
    /// secret-bearing request (`[u32 BE len][envelope]`), reused across
    /// calls so the write phase needs no third buffer. `Zeroizing` wipes
    /// the allocation when the buffer is ever dropped; the encode path
    /// additionally zeroizes at the start of every reuse and the
    /// [`ScratchReturn`] guard zeroizes before handing the buffer back.
    static REQ_BUF: RefCell<Zeroizing<Vec<u8>>> =
        RefCell::new(Zeroizing::new(Vec::with_capacity(1024)));
    /// Card RV-TRANSPORT-TCP-79910BA5 (design note): the envelope serializer
    /// borrows the request body (`req: &[u8]`) while writing the envelope
    /// bytes, so the body cannot live in the SAME `Vec` it is being written
    /// into (simultaneous shared+mut borrow of one buffer is unsound).
    /// The body therefore gets its own zeroizing scratch slot; both slots
    /// are taken out (owned) for the duration of the roundtrip and returned
    /// zeroized afterwards — no per-call heap allocation in steady state.
    static REQ_BODY_BUF: RefCell<Zeroizing<Vec<u8>>> =
        RefCell::new(Zeroizing::new(Vec::with_capacity(1024)));
}

/// Card RV-CLIENT-35A189A9: envelope encoding failures keep their typed
/// rmp_serde source (`ClientError::Encode`) instead of a stringly
/// `Protocol` message. Generic over `T: Serialize` so it can be tested
/// with a failing `Serialize` impl; partial output on failure is zeroized
/// by the caller ([`ScratchReturn`]).
pub(crate) fn encode_named_into<T: serde::Serialize>(
    buf: &mut Vec<u8>,
    value: &T,
) -> Result<(), ClientError> {
    rmp_serde::encode::write_named(buf, value).map_err(ClientError::Encode)
}

/// Which thread-local scratch slot a [`ScratchReturn`] guard hands its
/// buffer back to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScratchSlot {
    Framed,
    Body,
}

/// Returns a taken thread-local scratch buffer to its slot (including on
/// early returns), zeroizing it first so no secret bytes ever sit in the
/// slot between requests. Never holds a `RefCell` borrow across `.await`:
/// the guard owns the buffer; the borrow happens only inside `Drop`.
struct ScratchReturn(Option<Zeroizing<Vec<u8>>>, ScratchSlot);

impl Drop for ScratchReturn {
    fn drop(&mut self) {
        if let Some(mut buf) = self.0.take() {
            // `Vec::zeroize` wipes the WHOLE allocation capacity and clears.
            (*buf).zeroize();
            match self.1 {
                ScratchSlot::Framed => REQ_BUF.with(|cell| *cell.borrow_mut() = buf),
                ScratchSlot::Body => REQ_BODY_BUF.with(|cell| *cell.borrow_mut() = buf),
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn request_scratch_len_for_test() -> usize {
    REQ_BUF.with(|cell| cell.borrow().len())
}

#[cfg(test)]
pub(crate) fn fill_request_scratch_capacity_for_test() -> usize {
    REQ_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        let len = buf.len();
        let capacity = buf.capacity();
        buf.resize(capacity, 0xA5);
        buf.fill(0xA5);
        buf.truncate(len);
        capacity
    })
}

/// Reads the entire allocation after its bytes have been initialized by the
/// test fill helper.
///
/// # Safety
/// The caller must have initialized every byte up to capacity with
/// `fill_request_scratch_capacity_for_test`, and must ensure no operation has
/// written beyond the original capacity (which could reallocate the buffer).
#[cfg(test)]
pub(crate) unsafe fn request_scratch_capacity_bytes_for_test(expected_capacity: usize) -> Vec<u8> {
    REQ_BUF.with(|cell| {
        let buf = cell.borrow();
        assert_eq!(
            buf.capacity(),
            expected_capacity,
            "scratch capacity changed"
        );
        // SAFETY: upheld by this function's caller: every byte in this allocation
        // was initialized, and no operation wrote beyond its original capacity.
        unsafe { std::slice::from_raw_parts(buf.as_ptr(), buf.capacity()) }.to_vec()
    })
}

#[cfg(test)]
pub(crate) fn request_body_scratch_len_for_test() -> usize {
    REQ_BODY_BUF.with(|cell| cell.borrow().len())
}

#[cfg(test)]
pub(crate) fn request_body_scratch_capacity_for_test() -> usize {
    REQ_BODY_BUF.with(|cell| cell.borrow().capacity())
}

/// Reads the entire BODY-slot allocation (see `REQ_BODY_BUF`) after the
/// [`ScratchReturn`] guard has handed it back: the `zeroize` impl on
/// `Vec` writes every capacity byte (via a full-capacity `set_len`), so the
/// whole allocation is initialized.
///
/// # Safety
/// The caller must have captured the slot's capacity beforehand and must
/// ensure no operation has grown/reallocated the buffer since (the helper
/// asserts the capacity still matches), and that every byte up to capacity
/// is initialized (guaranteed once the slot was written to and returned
/// zeroized by [`ScratchReturn`]).
#[cfg(test)]
pub(crate) unsafe fn request_body_scratch_capacity_bytes_for_test(
    expected_capacity: usize,
) -> Vec<u8> {
    REQ_BODY_BUF.with(|cell| {
        let buf = cell.borrow();
        assert_eq!(
            buf.capacity(),
            expected_capacity,
            "body scratch capacity changed"
        );
        // SAFETY: upheld by this function's caller: every byte in this allocation
        // was initialized, and no operation wrote beyond its original capacity.
        unsafe { std::slice::from_raw_parts(buf.as_ptr(), buf.capacity()) }.to_vec()
    })
}

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

    #[cfg(test)]
    pub(crate) fn fill_request_scratch_capacity_for_test(&self) -> usize {
        fill_request_scratch_capacity_for_test()
    }

    /// Reads request scratch allocation bytes after the test fill helper ran.
    ///
    /// # Safety
    /// The caller must have initialized all capacity bytes with
    /// `fill_request_scratch_capacity_for_test` and prevent writes past the
    /// original capacity until this read.
    #[cfg(test)]
    pub(crate) unsafe fn request_scratch_capacity_bytes_for_test(
        &self,
        expected_capacity: usize,
    ) -> Vec<u8> {
        // SAFETY: forwarded caller obligations are the helper's safety contract.
        unsafe { request_scratch_capacity_bytes_for_test(expected_capacity) }
    }

    #[cfg(test)]
    pub(crate) fn request_body_scratch_len_for_test(&self) -> usize {
        request_body_scratch_len_for_test()
    }

    #[cfg(test)]
    pub(crate) fn request_body_scratch_capacity_for_test(&self) -> usize {
        request_body_scratch_capacity_for_test()
    }

    /// Reads the BODY scratch slot's whole allocation (zeroized on the fixed
    /// code path).
    ///
    /// # Safety
    /// Same contract as [`request_body_scratch_capacity_bytes_for_test`]: the
    /// caller captured the capacity beforehand and every capacity byte is
    /// initialized (see the free function's `# Safety` section).
    #[cfg(test)]
    pub(crate) unsafe fn request_body_scratch_capacity_bytes_for_test(
        &self,
        expected_capacity: usize,
    ) -> Vec<u8> {
        // SAFETY: forwarded caller obligations are the helper's safety contract.
        unsafe { request_body_scratch_capacity_bytes_for_test(expected_capacity) }
    }

    #[cfg(test)]
    pub(crate) fn set_next_request_id_for_test(&self, id: u32) {
        self.next_request_id.store(id, Ordering::Relaxed);
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
    /// Surfaced as a hint when unchanged; a verified changed identity returns
    /// `ClientError::IdentityRotationPending`. The caller must approve out of
    /// band and reconnect with the new pin; no pin is updated automatically.
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
                batch
                    .interner_epochs
                    .entry(repo.clone())
                    .or_insert_with(|| self.interner_cache().get_or_create(db, &repo).epoch());
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
        let mut req = DbRequest::CreateScramUser {
            name: name.to_string(),
            password: password.as_str().to_owned().into(),
            roles,
            hmac: None,
        };
        let tag = {
            let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
            let canonical =
                shamir_query_types::hmac::canonical_db_request(&req).map_err(|error| {
                    ClientError::Protocol(format!("canonical request failed: {error:?}"))
                })?;
            shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
        };
        if let DbRequest::CreateScramUser { hmac, .. } = &mut req {
            *hmac = Some(tag);
        }
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
        let mut req = DbRequest::SetReplicator {
            user: user.to_string(),
            on,
            hmac: None,
        };
        let tag = {
            let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
            let canonical =
                shamir_query_types::hmac::canonical_db_request(&req).map_err(|error| {
                    ClientError::Protocol(format!("canonical request failed: {error:?}"))
                })?;
            shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
        };
        if let DbRequest::SetReplicator { hmac, .. } = &mut req {
            *hmac = Some(tag);
        }
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
    /// (GC'd, never existed, or a pre-RFC op that had no op_id). Unsupported
    /// operations and other server errors are returned as [`ClientError::Db`].
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

        // T-cl-1 + RV-CLIENT-E16A04E5 + RV-CLIENT-35A189A9 +
        // RV-TRANSPORT-TCP-79910BA5: thread-local ZEROIZING scratch slots for
        // request serialisation, reused across calls. Both buffers are taken
        // OUT of their `RefCell` slots (borrow released before any `.await`;
        // owning them across await is fine), the request body encodes into
        // the body slot via the typed-source `encode_named_into` helper, and
        // the borrowed envelope encodes into the framed slot with its
        // 4-byte length prefix patched in place — zero per-call heap
        // allocations and no clone of the request bytes. The
        // `ScratchReturn` guards zeroize and restore the slots on every
        // exit path (including cancellation) after the write phase ends.
        let rid =
            match self
                .next_request_id
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            {
                Ok(id) => id,
                Err(_) => {
                    self.state.shutdown(CloseCause::RequestIdExhausted);
                    return Err(ClientError::ConnectionClosed(
                        self.state.cause().unwrap_or(CloseCause::RequestIdExhausted),
                    ));
                }
            };
        let mut framed_return = ScratchReturn(None, ScratchSlot::Framed);
        let mut body_return = ScratchReturn(None, ScratchSlot::Body);
        let encode_result: Result<(), ClientError> = {
            // Take BOTH scratch buffers out of their slots (the `RefCell`
            // borrow ends immediately — no borrow crosses the `.await`s
            // below; owning the buffers across await is fine). The
            // [`ScratchReturn`] guards zeroize and return them on every exit
            // path, including after the write phase and on cancellation.
            framed_return.0 = Some(REQ_BUF.with(|cell| std::mem::take(&mut *cell.borrow_mut())));
            body_return.0 = Some(REQ_BODY_BUF.with(|cell| std::mem::take(&mut *cell.borrow_mut())));
            let framed = framed_return
                .0
                .as_mut()
                .expect("framed scratch taken from slot");
            let body = body_return
                .0
                .as_mut()
                .expect("body scratch taken from slot");
            let body_vec: &mut Vec<u8> = &mut *body;
            // Zeroize-on-reuse: wipe whatever the previous request left.
            body_vec.zeroize();
            // Card RV-CLIENT-35A189A9: the request body encodes via
            // `encode_named_into`, keeping the typed rmp_serde source on
            // failure. Partial output stays in `body_vec` and is zeroized by
            // the `ScratchReturn` guard before the slot is refilled.
            let body_encoded = encode_named_into(body_vec, req);
            // The `ScratchReturn` guards zeroize the partial output before
            // the slot is refilled (35A189A9 contract).
            body_encoded?;
            let framed_vec: &mut Vec<u8> = &mut *framed;
            framed_vec.zeroize();
            // Reserve the frame-length prefix as placeholders; patched in
            // place below once the envelope size is known (no third buffer,
            // card RV-TRANSPORT-TCP-79910BA5).
            framed_vec.extend_from_slice(&[0u8; 4]);
            let envelope = RequestEnvelopeRef {
                session_id: &self.session_id,
                request_id: Some(rid),
                req: &body_vec[..],
            };
            let envelope_encoded = encode_named_into(framed_vec, &envelope);
            envelope_encoded?;
            let len = ((framed_vec.len() - 4) as u32).to_be_bytes();
            framed_vec[0..4].copy_from_slice(&len);
            Ok(())
        };
        encode_result?;

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
            if reg.map.insert_sync(rid, tx).is_err() {
                self.state
                    .shutdown(CloseCause::Protocol(Arc::from("request id collision")));
                return Err(ClientError::ConnectionClosed(
                    self.state.cause().unwrap_or(CloseCause::ReaderCancelled),
                ));
            }
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
            let framed = framed_return
                .0
                .as_deref()
                .expect("framed scratch in flight during write");
            tokio::select! {
                // Biased: a pending closure beats a write onto a dying
                // connection, so a released hook never emits wire bytes.
                biased;
                cause = self.state.closed_wait() => {
                    cancel_guard.defuse();
                    drop(write);
                    return Err(ClientError::ConnectionClosed(cause));
                }
                r = write_frame_prereserved(&mut *write, framed) => {
                    // On Err the shared shutdown(Io) below runs with the real
                    // transport cause — the guard must not double-close.
                    cancel_guard.defuse();
                    r.map(|_| ())
                }
            }
        };
        // RV-CLIENT-E16A04E5-D: the write phase is over — release BOTH
        // scratch buffers NOW, BEFORE any response await. Their `Drop`
        // zeroizes full capacity and restores the thread-local slots, so a
        // concurrent in-flight request on the same thread can reuse them
        // (zero-alloc multiplexing) and the secret-bearing request body does
        // not stay live (unwiped, outside the slot) through the whole
        // response wait. The early-return arm inside the `select!` above
        // already dropped the guards by returning; do not move this scope
        // earlier — the write needs the framed borrow.
        drop(framed_return);
        drop(body_return);
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
pub(crate) fn should_advertise_interner_epochs(server_version: u8, _has_epochs: bool) -> bool {
    server_version >= 2
}
