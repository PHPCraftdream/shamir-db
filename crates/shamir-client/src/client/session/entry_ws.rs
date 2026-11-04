//! WebSocket and plain-TCP entry points: [`Client::connect_ws`],
//! [`Client::resume_ws`] / [`Client::resume_ws_secure`] and
//! [`Client::connect_plain`]. Same three steps as the TCP twins in
//! `entry.rs` — dial, `run_scram` / `resume_wire_exchange`, `assemble`.

use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::aws_lc_rs::default_provider;
use tokio::io::AsyncRead;

use shamir_connect::common::username::NormalizedUsername;

use crate::client::handshake::{read_binding, resume_wire_exchange, run_scram, ScramRequest};
use crate::client::session::client::{Client, SessionMeta};
use crate::client::session::entry::resume_security::ProtectedResumeBuffer;
use crate::client::session::entry::{phase_name, session_meta, PHASE_TCP};
use crate::client::session::options::{ConnectOptions, ResumeOptions, SecureResumeOptions};
use crate::client::session::token::{AuthKind, TokenWsOptions};
use crate::client::transport::token::dial_token_ws;
use crate::client::transport::{
    dial_tcp_plain, dial_ws_native, Dialed, LifecycleHooks, WriteSink, WsByteStream,
};
use crate::error::ClientError;

async fn bound_handshake<T>(
    handshake_timeout: Option<Duration>,
    phase: &AtomicU8,
    fut: impl Future<Output = Result<T, ClientError>>,
) -> Result<T, ClientError> {
    match handshake_timeout {
        None => fut.await,
        Some(budget) => tokio::time::timeout(budget, fut).await.map_err(|_| {
            ClientError::HandshakeTimeout(
                budget,
                Arc::from(phase_name(phase.load(Ordering::Relaxed))),
            )
        })?,
    }
}

impl Client {
    /// Run the full TCP→TLS→WebSocket upgrade→SCRAM handshake against a native
    /// WebSocket listener (`kind: ws`, `profile: tls_exporter`, path
    /// `/shamir/v1`) and return a ready client. Same options and trust model
    /// as [`Client::connect`]; `handshake_timeout` bounds the whole sequence
    /// including the upgrade.
    pub async fn connect_ws(opts: ConnectOptions) -> Result<Self, ClientError> {
        let _ = default_provider().install_default();
        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;
        let phase = Arc::new(AtomicU8::new(PHASE_TCP));
        let handshake_timeout = opts.handshake_timeout;
        let fut = Self::handshake_ws(opts, username, phase.clone());
        bound_handshake(handshake_timeout, &phase, fut).await
    }

    async fn handshake_ws(
        opts: ConnectOptions,
        username: NormalizedUsername,
        phase: Arc<AtomicU8>,
    ) -> Result<Self, ClientError> {
        let dialed = dial_ws_native(
            opts.addr,
            &opts.server_name,
            opts.connect_timeout,
            Some(phase.as_ref()),
        )
        .await?;
        Self::scram_and_assemble(dialed, opts, username, phase.as_ref()).await
    }

    /// Open a session over plain TCP (`kind: tcp`, `profile: plain`, binding
    /// mode 0x00). The server accepts plain TCP on loopback only, so a
    /// non-loopback `opts.addr` is refused here, before any socket is
    /// opened. `opts.server_name` is ignored.
    pub async fn connect_plain(opts: ConnectOptions) -> Result<Self, ClientError> {
        if !opts.addr.ip().to_canonical().is_loopback() {
            return Err(ClientError::handshake("plain TCP is loopback-only"));
        }
        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;
        let phase = Arc::new(AtomicU8::new(PHASE_TCP));
        let handshake_timeout = opts.handshake_timeout;
        let fut = Self::handshake_plain(opts, username, phase.clone());
        bound_handshake(handshake_timeout, &phase, fut).await
    }

    async fn handshake_plain(
        opts: ConnectOptions,
        username: NormalizedUsername,
        phase: Arc<AtomicU8>,
    ) -> Result<Self, ClientError> {
        let dialed = dial_tcp_plain(opts.addr, opts.connect_timeout).await?;
        Self::scram_and_assemble(dialed, opts, username, phase.as_ref()).await
    }

    async fn scram_and_assemble<R>(
        mut dialed: Dialed<R>,
        opts: ConnectOptions,
        username: NormalizedUsername,
        phase: &AtomicU8,
    ) -> Result<Self, ClientError>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let outcome = run_scram(
            &mut dialed.read,
            &mut dialed.write,
            ScramRequest {
                username,
                binding: dialed.binding,
                trusted_pin: opts.trusted_pin,
                accept_new_host: opts.accept_new_host,
                password: &opts.password,
                phase: Some(phase),
            },
        )
        .await?;

        Ok(Self::assemble(
            dialed.read,
            dialed.write,
            session_meta(outcome, opts.request_timeout),
            LifecycleHooks::default(),
        ))
    }

    /// [`Client::resume`] over the native WebSocket listener: the ticket moves
    /// freely between TCP and WebSocket because the TLS exporter label and
    /// value are identical on both.
    pub async fn resume_ws(opts: ResumeOptions) -> Result<Self, ClientError> {
        Self::resume_ws_secure(opts.into()).await
    }

    /// [`Client::resume_secure`] over the native WebSocket listener.
    pub async fn resume_ws_secure(opts: SecureResumeOptions) -> Result<Self, ClientError> {
        let ticket = ProtectedResumeBuffer(opts.ticket);
        let _ = default_provider().install_default();
        let mut dialed = dial_ws_native(opts.addr, &opts.server_name, None, None).await?;

        let resumed = resume_wire_exchange(
            &mut dialed.read,
            &mut dialed.write,
            ticket,
            opts.pinned_hash,
            dialed.binding.exporter,
        )
        .await?;

        Ok(Self::assemble(
            dialed.read,
            dialed.write,
            SessionMeta {
                session_id: resumed.session_id,
                pinned_hash: opts.pinned_hash,
                expires_at_ns: resumed.expires_at_ns,
                resumption_ticket: resumed.resumption_ticket,
                resumption_expires_at_ns: resumed.resumption_expires_at_ns,
                server_query_version: resumed.server_query_version,
                request_timeout: None,
                rotation_in_progress: None,
                kdf_upgrade_required: false,
                auth: AuthKind::Scram,
            },
            LifecycleHooks::default(),
        ))
    }

    /// Open a session with an API token over the native token WebSocket
    /// (`GET /shamir/v1/ws` on a `kind: http` listener, binding mode none).
    /// TLS uses WebPKI trust of the endpoint (no Ed25519 pin). The data plane
    /// is identical to [`Client::connect`]; admin operations
    /// (`create_api_token`, ..., `repl`) are refused locally with
    /// `Db { code: "token_request_denied" }`. Server closes arrive as
    /// `ClientError::ConnectionClosed(CloseCause::PeerClosed { code, .. })`
    /// (4011 revoked, 4012 expired, ...).
    pub async fn connect_token_ws(opts: TokenWsOptions) -> Result<Self, ClientError> {
        let _ = default_provider().install_default();
        let TokenWsOptions {
            endpoint,
            token,
            connect_timeout,
            handshake_timeout,
            request_timeout,
        } = opts;
        let authorization = token.authorization_header();
        drop(token);
        let authorization = authorization?;
        let phase = Arc::new(AtomicU8::new(PHASE_TCP));
        let fut = async {
            let mut ws = dial_token_ws(
                &endpoint,
                authorization,
                connect_timeout,
                Some(phase.as_ref()),
            )
            .await?;
            let binding = read_binding(&mut ws).await?;
            Ok::<_, ClientError>((binding, tokio::io::split(WsByteStream::raw(ws))))
        };
        let (binding, (read, write)) = bound_handshake(handshake_timeout, &phase, fut).await?;

        Ok(Self::assemble(
            read,
            WriteSink::TokenWs(write),
            SessionMeta {
                session_id: binding.sid,
                pinned_hash: [0u8; 32],
                expires_at_ns: binding.expires_at_ns,
                resumption_ticket: None,
                resumption_expires_at_ns: None,
                server_query_version: 0,
                request_timeout,
                rotation_in_progress: None,
                kdf_upgrade_required: false,
                auth: AuthKind::Token,
            },
            LifecycleHooks::default(),
        ))
    }
}
