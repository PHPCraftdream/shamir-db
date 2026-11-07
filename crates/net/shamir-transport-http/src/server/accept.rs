use super::routes::ws::{
    HeadOk, UpgradeHead, UpgradedIo, WsAdmission, WsReject, WsRoute, WsUpgrade,
};
use super::service::{handle, peer, Context};
use crate::server::routes::{LoginRoutes, NoLogin, RouteServices};
use crate::{
    config::HttpServerConfig,
    framing::guarded_io::GuardedIo,
    gate::{ConnAdmission, RestExecutor, TokenGate},
};
use http::StatusCode;
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{io, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> IoStream for T {}
type BoxedIo = Box<dyn IoStream>;

/// Inbound bytes discarded after a connection ends before the socket is closed.
const LINGER_DISCARD_LIMIT: usize = 64 * 1024;
/// Longest time a finished connection lingers waiting for the peer to close.
const LINGER_DEADLINE: Duration = Duration::from_secs(5);

/// Failure to initialize or serve the HTTP listener.
#[derive(Debug, Error)]
pub enum ServeError {
    /// The configured TLS settings cannot provide TLS 1.3.
    #[error("TLS 1.3 configuration is unavailable")]
    TlsConfiguration(#[source] rustls::Error),
}

/// Builds the listener TLS configuration restricted to TLS 1.3, HTTP/1.1 ALPN,
/// and disabled 0-RTT, preserving the supplied certificate resolver.
pub fn tls13_config(
    config: &rustls::ServerConfig,
) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    let builder = rustls::ServerConfig::builder_with_provider(config.crypto_provider().clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?;
    let mut tls = builder
        .with_no_client_auth()
        .with_cert_resolver(config.cert_resolver.clone());
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    tls.max_early_data_size = 0;
    Ok(Arc::new(tls))
}

/// Serves HTTP/1.1 connections with disabled login and WebSocket hooks.
pub async fn serve<G, E, A>(
    listener: TcpListener,
    cfg: Arc<HttpServerConfig>,
    gate: Arc<G>,
    exec: Arc<E>,
    admission: Arc<A>,
    shutdown: CancellationToken,
) -> Result<(), ServeError>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    A: ConnAdmission,
{
    serve_inner(
        listener,
        cfg,
        gate,
        exec,
        admission,
        shutdown,
        RouteServices {
            login: Arc::new(NoLogin),
            ws: Arc::new(NoWs),
        },
    )
    .await
}

/// Serves HTTP/1.1 connections with an application-provided login hook.
pub async fn serve_with_login<G, E, A, L>(
    listener: TcpListener,
    cfg: Arc<HttpServerConfig>,
    gate: Arc<G>,
    exec: Arc<E>,
    admission: Arc<A>,
    login: Arc<L>,
    shutdown: CancellationToken,
) -> Result<(), ServeError>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    A: ConnAdmission,
    L: LoginRoutes,
{
    serve_inner(
        listener,
        cfg,
        gate,
        exec,
        admission,
        shutdown,
        RouteServices {
            login,
            ws: Arc::new(NoWs),
        },
    )
    .await
}

/// Serves HTTP/1.1 connections and tracked WebSocket sessions until shutdown.
pub async fn serve_with_ws<G, E, A, W>(
    listener: TcpListener,
    cfg: Arc<HttpServerConfig>,
    gate: Arc<G>,
    exec: Arc<E>,
    admission: Arc<A>,
    shutdown: CancellationToken,
    ws: Arc<W>,
) -> Result<(), ServeError>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    A: ConnAdmission,
    W: WsUpgrade<G::Grant>,
{
    serve_inner(
        listener,
        cfg,
        gate,
        exec,
        admission,
        shutdown,
        RouteServices {
            login: Arc::new(NoLogin),
            ws,
        },
    )
    .await
}

/// Serves HTTP/1.1 connections with login and WebSocket routes together.
pub async fn serve_with_routes<G, E, A, L, W>(
    listener: TcpListener,
    cfg: Arc<HttpServerConfig>,
    gate: Arc<G>,
    exec: Arc<E>,
    admission: Arc<A>,
    shutdown: CancellationToken,
    routes: RouteServices<L, W>,
) -> Result<(), ServeError>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    A: ConnAdmission,
    L: LoginRoutes,
    W: WsUpgrade<G::Grant>,
{
    serve_inner(listener, cfg, gate, exec, admission, shutdown, routes).await
}

pub(crate) struct NoWs;

impl<G: Send + 'static> WsUpgrade<G> for NoWs {
    type HeadState = ();
    type Session = ();

    fn check_head(
        &self,
        _route: WsRoute,
        _head: UpgradeHead<'_>,
        _peer: crate::gate::PeerInfo,
    ) -> Result<HeadOk<Self::HeadState>, WsReject> {
        Err(WsReject {
            status: StatusCode::NOT_FOUND,
            retry_after: None,
            close: false,
        })
    }

    fn admit(
        &self,
        _route: WsRoute,
        _head: HeadOk<Self::HeadState>,
        _grant: Option<G>,
        _peer: crate::gate::PeerInfo,
    ) -> Result<WsAdmission<Self::Session>, WsReject> {
        Err(WsReject {
            status: StatusCode::NOT_FOUND,
            retry_after: None,
            close: false,
        })
    }

    async fn run(&self, _session: Self::Session, _io: UpgradedIo, _shutdown: CancellationToken) {}
}

async fn serve_inner<G, E, A, L, W>(
    listener: TcpListener,
    cfg: Arc<HttpServerConfig>,
    gate: Arc<G>,
    exec: Arc<E>,
    admission: Arc<A>,
    shutdown: CancellationToken,
    routes: RouteServices<L, W>,
) -> Result<(), ServeError>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    A: ConnAdmission,
    L: LoginRoutes,
    W: WsUpgrade<G::Grant>,
{
    let tls_config = cfg
        .tls
        .as_deref()
        .map(tls13_config)
        .transpose()
        .map_err(ServeError::TlsConfiguration)?;
    let mut tasks = JoinSet::new();
    let mut accept_backoff = Duration::from_millis(10);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "HTTP connection task failed");
                }
            }
            result = listener.accept() => {
                let (stream, addr) = match result {
                    Ok(value) => {
                        accept_backoff = Duration::from_millis(10);
                        value
                    }
                    Err(error) => {
                        if !accept_retry(&error, &mut accept_backoff, &shutdown).await.is_continue() {
                            break;
                        }
                        continue;
                    }
                };
                let Some(permit) = admission.admit(&addr) else {
                    drop(stream);
                    continue;
                };
                let cfg = cfg.clone();
                let gate = gate.clone();
                let exec = exec.clone();
                let ws = routes.ws.clone();
                let login = routes.login.clone();
                let token = shutdown.clone();
                let tls_config = tls_config.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if token.is_cancelled() {
                        return;
                    }
                    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel::<Box<dyn Send + Sync>>(1);
                    let tls = tls_config.is_some();
                    let peer_info = peer(addr, tls, &http::HeaderMap::new());
                    let stream: BoxedIo = if let Some(tls_cfg) = tls_config {
                        let accepted = tokio::time::timeout(
                            cfg.limits.head_timeout,
                            tokio_rustls::TlsAcceptor::from(tls_cfg).accept(stream),
                        )
                        .await;
                        let Ok(Ok(stream)) = accepted else { return };
                        Box::new(stream)
                    } else {
                        Box::new(stream)
                    };
                    let write_timeout_override = Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let login_refusal = login.clone();
                    let guarded = GuardedIo::new(
                        stream,
                        cfg.limits.max_head_bytes,
                        cfg.limits.max_headers,
                        tls,
                        cfg.hsts,
                        hold_rx,
                    )
                    .with_login_errors(login.enabled())
                    .with_refusal_hook(Arc::new(move |route, status| {
                        if login_refusal.enabled()
                            && (route == b"/shamir/v1/auth/begin"
                                || route == b"/shamir/v1/auth/finish")
                        {
                            if let Ok(status) = StatusCode::from_u16(status) {
                                login_refusal.refused(route, status);
                            }
                        }
                    }))
                    .with_write_timeout_override(write_timeout_override.clone())
                    .with_timeouts(
                        cfg.limits.head_timeout,
                        cfg.limits.request_timeout,
                        cfg.limits.write_timeout,
                    );
                    let io = TokioIo::new(guarded);
                    let (upgrade_tx, mut upgrade_rx) = tokio::sync::oneshot::channel();
                    let ws_run = ws.clone();
                    let service_context = Arc::new(Context {
                        cfg: cfg.clone(),
                        gate,
                        exec,
                        peer: peer_info,
                        holds: hold_tx,
                        ws,
                        upgrade: tokio::sync::Mutex::new(Some(upgrade_tx)),
                        login,
                        write_timeout_override,
                    });
                    let service = service_fn(move |request| handle(request, service_context.clone()));
                    let mut builder = http1::Builder::new();
                    builder
                        .keep_alive(true)
                        .max_buf_size(cfg.limits.max_head_bytes.max(8192))
                        .max_headers(cfg.limits.max_headers)
                        .timer(TokioTimer::new())
                        .header_read_timeout(cfg.limits.head_timeout);
                    let mut connection = Box::pin(builder.serve_connection(io, service).with_upgrades());
                    let connection_result = tokio::select! {
                        _ = token.cancelled() => {
                            connection.as_mut().graceful_shutdown();
                            tokio::time::timeout(cfg.limits.request_timeout, &mut connection).await.ok()
                        }
                        result = &mut connection => Some(result),
                    };
                    if let Ok((session, on_upgrade)) = upgrade_rx.try_recv() {
                        match on_upgrade.await {
                            Ok(upgraded) => {
                                let child = token.child_token();
                                ws_run.run(session, UpgradedIo::new(upgraded), child).await;
                                return;
                            }
                            Err(error) => tracing::debug!(%error, "HTTP upgrade handoff failed"),
                        }
                    }
                    if let Some(outcome) = connection_result {
                        let connection = *std::pin::Pin::into_inner(connection);
                        if let Some(parts) = connection.into_parts() {
                            let mut guarded = parts.io.into_inner();
                            // A dead connection must not hold the login slot for up to the
                            // linger deadline; drop any pending or active hold now.
                            guarded.release_holds();
                            if let (Err(failure), false) = (&outcome, token.is_cancelled()) {
                                let timed_out = failure.is_timeout();
                                let drain = tokio::time::timeout(
                                    cfg.limits.write_timeout,
                                    guarded.finish_rejection(timed_out),
                                )
                                .await;
                                if let Err(error) = drain.unwrap_or_else(|_| {
                                    Err(io::Error::new(
                                        io::ErrorKind::TimedOut,
                                        "canned response drain deadline expired",
                                    ))
                                }) {
                                    tracing::debug!(%error, "HTTP rejection response drain stopped");
                                }
                            }
                            let _ = tokio::time::timeout(
                                cfg.limits.write_timeout,
                                tokio::io::AsyncWriteExt::shutdown(&mut guarded),
                            )
                            .await;
                            tokio::select! {
                                _ = token.cancelled() => {}
                                _ = tokio::time::timeout(LINGER_DEADLINE, guarded.discard_inbound(LINGER_DISCARD_LIMIT)) => {}
                            }
                        }
                    }
                });
            }
        }
    }
    let drain_timeout = cfg
        .limits
        .connection_drain_timeout
        .unwrap_or(cfg.limits.request_timeout);
    let deadline = tokio::time::Instant::now() + drain_timeout;
    let mut timed_out = false;
    while !tasks.is_empty() {
        if tokio::time::timeout_at(deadline, tasks.join_next())
            .await
            .is_err()
        {
            let stragglers = tasks.len();
            tracing::warn!(
                stragglers,
                ?drain_timeout,
                "HTTP connection drain timed out; aborting stragglers"
            );
            timed_out = true;
            break;
        }
    }
    if timed_out {
        tasks.abort_all();
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            if !error.is_cancelled() {
                tracing::warn!(%error, "HTTP connection task failed during drain");
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcceptRetry {
    Continue,
    Stop,
}

impl AcceptRetry {
    fn is_continue(self) -> bool {
        matches!(self, Self::Continue)
    }
}

async fn accept_retry(
    error: &io::Error,
    backoff: &mut Duration,
    shutdown: &CancellationToken,
) -> AcceptRetry {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
    ) {
        return AcceptRetry::Continue;
    }

    if is_resource_exhaustion(error) {
        tracing::warn!(%error, "HTTP accept resource exhausted");
    } else {
        tracing::warn!(%error, "HTTP accept failed");
    }
    tokio::select! {
        _ = shutdown.cancelled() => AcceptRetry::Stop,
        _ = tokio::time::sleep(*backoff) => {
            *backoff = (*backoff * 2).min(Duration::from_secs(1));
            AcceptRetry::Continue
        }
    }
}

fn is_resource_exhaustion(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::OutOfMemory | io::ErrorKind::WouldBlock
    ) || error
        .raw_os_error()
        .is_some_and(|code| matches!(code, 23 | 24 | 10024 | 105 | 10055))
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
