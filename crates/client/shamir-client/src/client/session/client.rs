//! The [`Client`] type and its shared construction path.

#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicU32, AtomicU8};
use std::sync::Arc;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use shamir_connect::common::wire::RotationInProgressPayload;

use crate::client::session::token::AuthKind;
use crate::client::transport::{reader_task, Lifecycle, LifecycleHooks, ShutdownGuard, WriteSink};
use crate::error::ClientError;
use crate::interner_cache::InternerCacheRegistry;

/// Session metadata shared by every client-construction path; consumed by
/// [`Client::assemble`].
pub(crate) struct SessionMeta {
    pub(crate) session_id: [u8; 32],
    pub(crate) pinned_hash: [u8; 32],
    pub(crate) expires_at_ns: u64,
    pub(crate) resumption_ticket: Option<Zeroizing<Vec<u8>>>,
    pub(crate) resumption_expires_at_ns: Option<u64>,
    pub(crate) server_query_version: u8,
    pub(crate) request_timeout: Option<std::time::Duration>,
    pub(crate) rotation_in_progress: Option<RotationInProgressPayload>,
    pub(crate) kdf_upgrade_required: bool,
    pub(crate) auth: AuthKind,
}

/// Connected, authenticated client.
///
/// Concurrent calls to `execute`/`ping`/`create_scram_user` are fully
/// supported: multiple requests can be in flight simultaneously on the same
/// connection.  The server may send responses in any order (completion order,
/// not send order); a background reader task correlates each response to its
/// caller via the `rid` (request-id) field of the wire envelopes.
///
/// Each public method registers a `oneshot` receiver in the pending map
/// **before** writing to the socket (avoiding a race with a very fast server),
/// then awaits its own channel independently.
pub struct Client {
    pub(in crate::client) write: tokio::sync::Mutex<WriteSink>,
    pub(in crate::client) session_id: [u8; 32],
    pub(in crate::client) pinned_hash: [u8; 32],
    pub(in crate::client) expires_at_ns: u64,
    /// Bearer credential — held in `Zeroizing` so it is wiped when the
    /// client drops rather than lingering in freed heap.
    pub(in crate::client) resumption_ticket: Option<Zeroizing<Vec<u8>>>,
    pub(in crate::client) resumption_expires_at_ns: Option<u64>,
    pub(in crate::client) next_request_id: AtomicU32,
    /// The ONE closeable lifecycle state: pending-request registry,
    /// subscription registry (+ early buffer) and the `closed_hint` fast
    /// path. See `transport::Lifecycle`.
    pub(in crate::client) state: Lifecycle,
    /// §B21: JoinHandle is stored so we can abort on close/drop.
    pub(in crate::client) reader_handle: Option<JoinHandle<()>>,
    /// Per-`(db, repo)` interner field-map cache (Stage 5 minimal). Lock-free
    /// registry; populated lazily by `dump_repo`/`touch_fields`. Ids come ONLY
    /// from server `interner_dump`/`interner_touch` responses.
    pub(crate) interner_cache: Arc<InternerCacheRegistry>,
    /// Max query-language version advertised by the server in `auth_ok` /
    /// `resume_ok`. `0` when the server did not send the field (pre-v2 server).
    /// Set once on connect/resume; never mutated afterward.
    pub(in crate::client) server_query_version: AtomicU8,
    /// Per-request response deadline carried over from
    /// [`crate::client::ConnectOptions::request_timeout`]. `None` = unbounded wait
    /// (prior behaviour). Applied in [`Self::roundtrip`].
    pub(in crate::client) request_timeout: Option<std::time::Duration>,
    /// `auth_ok` orphan-recovery hint (spec §6.5). A changed pin fails closed;
    /// a verified pending rotation is returned as `IdentityRotationPending`
    /// for caller-approved, out-of-band persistence and reconnect.
    pub(in crate::client) rotation_in_progress: Option<RotationInProgressPayload>,
    /// `auth_ok` KDF-upgrade request (spec §13).
    pub(in crate::client) kdf_upgrade_required: bool,
    pub(in crate::client) auth: AuthKind,
    /// Test-only lifecycle hooks (P0 C1). The type is zero-sized in
    /// production; the fields (and this field's readers) are test-only.
    #[cfg(test)]
    pub(in crate::client) hooks: LifecycleHooks,
}

impl Client {
    /// Shared finalisation for every construction path (TCP, IPC, resume,
    /// tests): builds the lifecycle state, spawns the reader task with its
    /// shutdown guard, and assembles the [`Client`].
    pub(crate) fn assemble<R>(
        reader: R,
        write_sink: WriteSink,
        meta: SessionMeta,
        hooks: LifecycleHooks,
    ) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let state = Lifecycle::new();
        // §B21: store JoinHandle — never drop silently. The shutdown guard
        // is constructed BEFORE the task can be spawned (or dropped
        // unpolled): every exit path of `reader_task` — including
        // cancellation before the first poll — closes the lifecycle.
        let guard = ShutdownGuard::new(state.clone());
        let reader_handle = tokio::spawn(reader_task(reader, state.clone(), guard, hooks.clone()));

        Self {
            write: tokio::sync::Mutex::new(write_sink),
            session_id: meta.session_id,
            pinned_hash: meta.pinned_hash,
            expires_at_ns: meta.expires_at_ns,
            resumption_ticket: meta.resumption_ticket,
            resumption_expires_at_ns: meta.resumption_expires_at_ns,
            next_request_id: AtomicU32::new(1),
            state,
            reader_handle: Some(reader_handle),
            interner_cache: Arc::new(InternerCacheRegistry::new()),
            server_query_version: AtomicU8::new(meta.server_query_version),
            request_timeout: meta.request_timeout,
            rotation_in_progress: meta.rotation_in_progress,
            kdf_upgrade_required: meta.kdf_upgrade_required,
            auth: meta.auth,
            #[cfg(test)]
            hooks,
        }
    }

    /// Refuses admin operations that need a SCRAM session (API-token
    /// sessions are rejected before any tag or body is built).
    pub(crate) fn require_scram(&self) -> Result<(), ClientError> {
        if self.auth == AuthKind::Token {
            return Err(ClientError::Db {
                code: "token_request_denied".to_string(),
                message: "requires a SCRAM session".to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
impl Client {
    /// Test-only constructor (P0 C1): build a client over injected
    /// simplex/duplex pipe halves, going through the SAME
    /// [`Self::assemble`] path (real lifecycle state + reader task).
    /// `reader` is the server→client half; `writer` the client→server half.
    pub(crate) fn for_test<R, W>(reader: R, writer: W, hooks: LifecycleHooks) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        Self::for_test_with_auth(reader, writer, hooks, AuthKind::Scram)
    }

    /// [`Self::for_test`] with an explicit [`AuthKind`].
    pub(crate) fn for_test_with_auth<R, W>(
        reader: R,
        writer: W,
        hooks: LifecycleHooks,
        auth: AuthKind,
    ) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        Self::assemble(
            reader,
            WriteSink::Test(Box::new(writer)),
            SessionMeta {
                session_id: [0u8; 32],
                pinned_hash: [0u8; 32],
                expires_at_ns: 0,
                resumption_ticket: None,
                resumption_expires_at_ns: None,
                server_query_version: 0,
                request_timeout: None,
                rotation_in_progress: None,
                kdf_upgrade_required: false,
                auth,
            },
            hooks,
        )
    }

    /// Test accessor: take the reader task's join handle so tests can await
    /// the reader's completion deterministically (no sleeps).
    pub(crate) fn take_reader_handle(&mut self) -> Option<JoinHandle<()>> {
        self.reader_handle.take()
    }

    /// Test accessor: the client's lifecycle state (`pub(in crate::client)` fields are
    /// not visible from the crate-root tests module).
    pub(crate) fn lifecycle_for_test(&self) -> &Lifecycle {
        &self.state
    }

    /// Test-only: force the negotiated server query version (e.g. to run the
    /// v2 id-keyed write/read path without a real handshake).
    pub(crate) fn set_server_query_version_for_test(&self, v: u8) {
        self.server_query_version.store(v, Ordering::SeqCst);
    }

    pub(crate) fn server_query_version_for_test(&self) -> u8 {
        self.server_query_version()
    }
}
