//! Accessors, request helpers and teardown for [`Client`].

use std::cell::RefCell;
use std::sync::atomic::Ordering;

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use shamir_connect::common::envelope::RequestEnvelope;

use shamir_transport_tcp::framing::write_frame;

use shamir_query_types::batch::{BatchRequest, BatchResponse};
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

use crate::client::subscription::{SubscriptionHandle, CLIENT_SUB_CHANNEL_CAP};
use crate::client::transport::await_pending_response;
use crate::client::Client;
use crate::error::ClientError;

impl Client {
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

    /// Register a subscription and get a handle to receive push frames.
    ///
    /// The caller must have already sent a subscribe request to the server
    /// and obtained the `sub_id`. Push frames arriving for this `sub_id`
    /// will be routed to the returned handle.
    pub fn subscribe_push(&self, sub_id: u64) -> SubscriptionHandle {
        let (tx, rx) = tokio::sync::mpsc::channel(CLIENT_SUB_CHANNEL_CAP);
        {
            let mut map = self.subscriptions.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(sub_id, tx.clone());
        }
        // Flush any early-buffered pushes that arrived before registration.
        {
            let mut buf = self.early_buffer.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(buffered) = buf.remove(&sub_id) {
                for envelope in buffered {
                    match tx.try_send(envelope) {
                        Ok(()) => {}
                        Err(tokio::sync::mpsc::error::TrySendError::Full(env)) => {
                            tracing::warn!(
                                "client mpsc full for sub={} while flushing early buffer; dropping push",
                                env.sub
                            );
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            // Receiver gone before we finished flushing — bail.
                            break;
                        }
                    }
                }
            }
        }
        SubscriptionHandle::new(sub_id, rx, self.subscriptions.clone())
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
        // BEFORE send: populate interner_epochs from the per-(db,repo) cache.
        // `distinct_repos` walks the batch's data ops' table-refs; admin ops
        // (no table_ref) are skipped. For each repo with a FieldMap, advertise
        // its current epoch so the server can attach a delta.
        if batch.interner_epochs.is_empty() {
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
        if self.closed.load(Ordering::Acquire) {
            return Err(ClientError::ConnectionClosed);
        }

        // T-cl-1: thread-local scratch buffer for request serialisation.
        // The buffer's capacity grows to the high-water mark and is reused
        // across calls, avoiding repeated grow-from-0 allocations.
        // The borrow is released before any `.await` — only a sized copy of
        // the bytes is passed forward, keeping the thread_local exclusive.
        thread_local! {
            static REQ_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1024));
        }
        let req_bytes: Vec<u8> = REQ_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            rmp_serde::encode::write_named(&mut *buf, req)?;
            Ok::<Vec<u8>, rmp_serde::encode::Error>(buf.clone())
        })?;

        let rid = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let envelope = RequestEnvelope::new(self.session_id, Some(rid), req_bytes);
        let envelope_bytes = envelope
            .to_msgpack()
            .map_err(|e| ClientError::Protocol(format!("envelope encode: {e}")))?;

        // Register BEFORE writing — avoid a race where the server sends the
        // response before we reach the oneshot registration.
        let (tx, rx) = oneshot::channel();
        {
            // §B2 audit: std::sync::Mutex, no .await while held — fine.
            let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(rid, tx);
        }

        // Send the request.  If write fails, remove the pending entry and
        // propagate the error so the caller is not left waiting on a dead rx.
        {
            let mut write = self.write.lock().await;
            if let Err(e) = write_frame(&mut *write, &envelope_bytes).await {
                // Clean up pending entry.
                let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
                map.remove(&rid);
                return Err(ClientError::Transport(format!("send: {e}")));
            }
        }

        // Await our oneshot.  The reader task delivers Ok(payload) or Err.
        // When `request_timeout` is set, bound the wait: a server that
        // accepts the connection but never answers this rid must not hang
        // the caller forever. `None` preserves the original unbounded await.
        let payload = await_pending_response(rx, self.request_timeout, &self.pending, rid).await?;

        let response: DbResponse = rmp_serde::from_slice(&payload)?;
        crate::visibility::map_response(response)
    }

    /// Close the TLS write half cleanly and abort the reader task.
    ///
    /// After this call the session is dead on the server side too
    /// (TCP close → session evicted).
    pub async fn close(mut self) {
        // Abort reader task first — then shut down write half.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        let mut w = self.write.lock().await;
        let _ = AsyncWriteExt::shutdown(&mut *w).await;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // §B21: abort the reader task when the Client drops without an
        // explicit `close()` call so the task does not outlive the Client.
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
    }
}
