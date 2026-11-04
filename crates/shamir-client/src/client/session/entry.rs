//! Handshake entry points: [`Client::connect`] (TCP+TLS+SCRAM),
//! [`Client::connect_local`] (IPC+SCRAM) and [`Client::resume`]
//! (ticket-based resumption).

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::ServerName;
use tokio::io::split;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::common::username::NormalizedUsername;

use shamir_transport_tcp::tls::{
    extract_tls_exporter, make_client_config_app_authenticated, AppLayerServerAuth,
};

use crate::client::handshake::{
    resume_wire_exchange, run_scram, ScramOutcome, ScramRequest, PHASE_KDF, PHASE_SCRAM,
};
use crate::client::session::client::{Client, SessionMeta};
use crate::client::session::options::{
    ConnectLocalOptions, ConnectOptions, ResumeOptions, SecureResumeOptions,
};
use crate::client::transport::{connect_ipc, connect_tcp, LifecycleHooks, WriteSink};
use crate::error::ClientError;

#[path = "../../interner_cache/resume/security.rs"]
pub(in crate::client) mod resume_security;
#[cfg(test)]
pub(crate) use resume_security::take_resume_wipes;
use resume_security::ProtectedResumeBuffer;

/// Handshake phases for [`ClientError::HandshakeTimeout`] diagnostics —
/// stored in a shared atomic and updated before each stage.
const PHASE_DNS: u8 = 0;
const PHASE_TCP: u8 = 1;
const PHASE_TLS: u8 = 2;

fn phase_name(phase: u8) -> &'static str {
    match phase {
        PHASE_DNS => "dns",
        PHASE_TCP => "tcp",
        PHASE_TLS => "tls",
        PHASE_SCRAM => "scram",
        PHASE_KDF => "kdf",
        _ => "unknown",
    }
}

fn session_meta(outcome: ScramOutcome, request_timeout: Option<Duration>) -> SessionMeta {
    let verified = outcome.verified;
    SessionMeta {
        session_id: verified.success.session_id,
        pinned_hash: verified.pinned_hash,
        expires_at_ns: verified.success.expires_at_ns,
        resumption_ticket: verified.resumption_ticket.map(Zeroizing::new),
        resumption_expires_at_ns: verified.resumption_expires_at_ns,
        server_query_version: outcome.server_query_version,
        request_timeout,
        rotation_in_progress: verified.auth_ok.rotation_in_progress,
        kdf_upgrade_required: outcome.kdf_upgrade_required,
    }
}

impl Client {
    /// Run the full TCP→TLS→SCRAM handshake and return a ready client.
    pub async fn connect(opts: ConnectOptions) -> Result<Self, ClientError> {
        let budget = opts.handshake_timeout.unwrap_or_default();
        let deadline = opts
            .handshake_timeout
            .map(|d| tokio::time::Instant::now() + d);
        Self::connect_until(opts, budget, deadline).await
    }

    /// [`Self::connect`] against an ABSOLUTE deadline shared by the caller, so
    /// a multi-step connect (DNS + several address attempts) spends ONE
    /// `handshake_timeout` budget in total instead of re-arming per step.
    async fn connect_until(
        opts: ConnectOptions,
        hs_budget: std::time::Duration,
        hs_deadline: Option<tokio::time::Instant>,
    ) -> Result<Self, ClientError> {
        // Install rustls crypto provider once. `install_default` is a
        // no-op on second call.
        let _ = default_provider().install_default();

        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;

        // ONE terminal deadline bounds the WHOLE handshake — the TCP connect,
        // TLS negotiation, every SCRAM frame read/write and the blocking KDF
        // derivation await (RV-CLIENT-NODE-1CFB3796). `connect_timeout`
        // additionally bounds just the TCP-connect step inside it.
        // Phase tracking: the timeout error reports the stage the handshake
        // was ACTUALLY in when the deadline elapsed (never a guessed phase).
        let phase = Arc::new(AtomicU8::new(PHASE_TCP));
        let fut = Self::handshake(opts, username, phase.clone());
        match hs_deadline {
            None => fut.await,
            Some(dl) => tokio::time::timeout_at(dl, fut).await.map_err(|_| {
                ClientError::HandshakeTimeout(
                    hs_budget,
                    Arc::from(phase_name(phase.load(Ordering::Relaxed))),
                )
            })?,
        }
    }

    /// Resolve `host:port` and connect — trying EVERY resolved address in
    /// order (RV-CLIENT-NODE-8FB7D9BF). This is the SINGLE production
    /// hostname path: the Node binding calls it directly and performs no
    /// address parsing of its own. IP literals connect directly; a hostname
    /// is DNS-resolved under the `handshake_timeout` budget (a DNS stall
    /// reports the `dns` phase), and each candidate address receives the
    /// SAME absolute deadline: DNS and every address attempt together spend
    /// ONE `handshake_timeout` (it never re-arms per attempt).
    pub async fn connect_host(
        host: &str,
        port: u16,
        mut opts: ConnectOptions,
    ) -> Result<Self, ClientError> {
        let budget = opts.handshake_timeout.unwrap_or_default();
        let deadline = opts
            .handshake_timeout
            .map(|d| tokio::time::Instant::now() + d);

        // IP literal: no resolver involved.
        if let Ok(ip) = host.parse::<IpAddr>() {
            opts.addr = SocketAddr::new(ip, port);
            return Self::connect_until(opts, budget, deadline).await;
        }

        let candidate = format!("{host}:{port}");
        let lookup = tokio::net::lookup_host(&candidate);
        let addrs: Vec<SocketAddr> = match deadline {
            None => lookup.await?.collect(),
            Some(dl) => tokio::time::timeout_at(dl, lookup)
                .await
                .map_err(|_| {
                    ClientError::HandshakeTimeout(budget, Arc::from(phase_name(PHASE_DNS)))
                })?
                .map_err(ClientError::from)?
                .collect(),
        };
        if addrs.is_empty() {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("host resolved to no addresses: {candidate}"),
            )));
        }

        let mut last: Option<ClientError> = None;
        for addr in addrs {
            opts.addr = addr;
            match Self::connect_until(opts.clone(), budget, deadline).await {
                Ok(client) => return Ok(client),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("at least one resolved address attempted"))
    }

    /// The handshake proper: TCP connect → TLS → SCRAM → [`Self::assemble`].
    /// Bounded as a whole by `ConnectOptions::handshake_timeout` (see
    /// [`Self::connect`]); `connect_timeout` bounds the TCP step inside.
    async fn handshake(
        opts: ConnectOptions,
        username: NormalizedUsername,
        phase: Arc<AtomicU8>,
    ) -> Result<Self, ClientError> {
        // ---- TLS ----
        phase.store(PHASE_TCP, Ordering::Relaxed);
        let client_cfg = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())
            .map_err(|e| ClientError::tls("client config", e))?;
        let connector = TlsConnector::from(client_cfg);
        let server_name = ServerName::try_from(opts.server_name.clone())
            .map_err(|e| ClientError::tls("server name", e))?;
        let tcp = connect_tcp(opts.addr, opts.connect_timeout).await?;
        phase.store(PHASE_TLS, Ordering::Relaxed);
        let tls = connector.connect(server_name, tcp).await?;
        let exporter = extract_tls_exporter(&tls)
            .map_err(|e| ClientError::tls("TLS exporter unavailable", e))?;

        let (mut r, mut w) = split(tls);

        let outcome = run_scram(
            &mut r,
            &mut w,
            ScramRequest {
                username,
                transport_kind: TransportKind::Tcp,
                binding_mode: BindingMode::TlsExporter,
                tls_exporter: exporter,
                trusted_pin: opts.trusted_pin,
                accept_new_host: opts.accept_new_host,
                password: &opts.password,
                phase: Some(phase.as_ref()),
            },
        )
        .await?;

        Ok(Self::assemble(
            r,
            WriteSink::Tls(w),
            session_meta(outcome, opts.request_timeout),
            LifecycleHooks::default(),
        ))
    }

    /// Run the full IPC→SCRAM handshake (no TLS) over a local Unix domain
    /// socket / Windows Named Pipe and return a ready client. Shares
    /// `run_scram` with [`Client::connect`] — same SCRAM flow, same
    /// `auth_ok`/resumption-ticket/TOFU-pin handling — only the transport
    /// setup and the `transport_kind`/`binding_mode` fed to
    /// `HandshakeBuilder` differ (spec TRANSPORT_UNIX.md: `binding_mode =
    /// 0x00`, zeroed channel binding — passed as an all-zero exporter).
    pub async fn connect_local(opts: ConnectLocalOptions) -> Result<Self, ClientError> {
        let username = NormalizedUsername::from_raw(&opts.username)
            .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;

        // ---- IPC connect (no TLS) ----
        let stream = connect_ipc(&opts.addr, opts.connect_timeout).await?;

        let (mut r, mut w) = split(stream);

        let outcome = run_scram(
            &mut r,
            &mut w,
            ScramRequest {
                username,
                transport_kind: TransportKind::Unix,
                binding_mode: BindingMode::None,
                tls_exporter: [0u8; 32],
                trusted_pin: opts.trusted_pin,
                accept_new_host: opts.accept_new_host,
                password: &opts.password,
                phase: None,
            },
        )
        .await?;

        Ok(Self::assemble(
            r,
            WriteSink::Ipc(w),
            session_meta(outcome, opts.request_timeout),
            LifecycleHooks::default(),
        ))
    }

    /// Resume a session using a previously-issued resumption ticket, bypassing
    /// the full Argon2id SCRAM handshake.
    ///
    /// # Flow
    /// 1. Open a new TLS connection (same as `connect`).
    /// 2. Verify the server's pinned identity proof over the TLS exporter and a
    ///    fresh nonce before disclosing the ticket.
    /// 3. Send `WireResumeInit` with the ticket and channel-binding mode.
    /// 4. Read `WireResumeOk` with the new session and optional rotated ticket.
    /// 5. Spawn the background reader task and return the ready `Client`.
    ///
    /// The legacy options contain a raw ticket until this future is polled.
    /// Prefer [`Self::resume_secure`] for retained or cancellable futures.
    pub async fn resume(opts: ResumeOptions) -> Result<Self, ClientError> {
        Self::resume_secure(opts.into()).await
    }

    /// Resume with a ticket protected even if the future is never polled.
    pub async fn resume_secure(opts: SecureResumeOptions) -> Result<Self, ClientError> {
        let ticket = ProtectedResumeBuffer(opts.ticket);
        let _ = default_provider().install_default();
        // ---- TLS ----
        let client_cfg = make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())
            .map_err(|e| ClientError::tls("client config", e))?;
        let connector = TlsConnector::from(client_cfg);
        let server_name = ServerName::try_from(opts.server_name.clone())
            .map_err(|e| ClientError::tls("server name", e))?;
        // ResumeOptions carries no timeout knobs — resumption preserves the
        // prior unbounded-wait connect behaviour (None).
        let tcp = connect_tcp(opts.addr, None).await?;
        let tls = connector.connect(server_name, tcp).await?;
        // Verify TLS exporter is available (required for channel-binding). The
        // exporter value will be used by the server to verify binding; the
        // client sends it implicitly via binding_mode in WireResumeInit.
        let tls_exporter = extract_tls_exporter(&tls)
            .map_err(|e| ClientError::tls("TLS exporter unavailable", e))?;

        let (mut r, mut w) = split(tls);

        let resumed =
            resume_wire_exchange(&mut r, &mut w, ticket, opts.pinned_hash, tls_exporter).await?;

        Ok(Self::assemble(
            r,
            WriteSink::Tls(w),
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
            },
            LifecycleHooks::default(),
        ))
    }
}
