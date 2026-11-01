use super::service::{handle, peer, Context};
use crate::{
    config::HttpServerConfig,
    framing::guarded_io::GuardedIo,
    gate::{ConnAdmission, RestExecutor, TokenGate},
};
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

/// Failure to initialize or serve the HTTP listener.
#[derive(Debug, Error)]
pub enum ServeError {
    /// The configured TLS settings cannot provide TLS 1.3.
    #[error("TLS 1.3 configuration is unavailable")]
    TlsConfiguration(#[source] rustls::Error),
}

fn tls13_config(config: &rustls::ServerConfig) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    let builder = rustls::ServerConfig::builder_with_provider(config.crypto_provider().clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?;
    let mut tls = builder
        .with_no_client_auth()
        .with_cert_resolver(config.cert_resolver.clone());
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    tls.max_early_data_size = 0;
    Ok(Arc::new(tls))
}

/// Serves HTTP/1.1 connections and gracefully drains tracked tasks on cancellation.
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
                let token = shutdown.clone();
                let tls_config = tls_config.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if token.is_cancelled() {
                        return;
                    }
                    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel::<Box<dyn Send + Sync>>(1);
                    let tls = tls_config.is_some();
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
                    let guarded = GuardedIo::new(
                        stream,
                        cfg.limits.max_head_bytes,
                        cfg.limits.max_headers,
                        tls,
                        cfg.hsts,
                        hold_rx,
                    )
                    .with_timeouts(
                        cfg.limits.head_timeout,
                        cfg.limits.request_timeout,
                        cfg.limits.write_timeout,
                    );
                    let io = TokioIo::new(guarded);
                    let context = Arc::new(Context {
                        cfg: cfg.clone(),
                        gate,
                        exec,
                        peer: peer(addr, tls, &http::HeaderMap::new()),
                        holds: hold_tx,
                    });
                    let service = service_fn(move |request| handle(request, context.clone()));
                    let mut builder = http1::Builder::new();
                    builder
                        .keep_alive(true)
                        .max_buf_size(cfg.limits.max_head_bytes.max(8192))
                        .max_headers(cfg.limits.max_headers)
                        .timer(TokioTimer::new())
                        .header_read_timeout(cfg.limits.head_timeout);
                    let mut connection = Box::pin(builder.serve_connection(io, service));
                    let result = tokio::select! {
                        _ = token.cancelled() => {
                            connection.as_mut().graceful_shutdown();
                            tokio::time::timeout(cfg.limits.request_timeout, &mut connection).await.ok()
                        }
                        result = &mut connection => Some(result),
                    };
                    if let Some(Err(_error)) = result {
                        let connection = *std::pin::Pin::into_inner(connection);
                        let mut guarded = connection.into_parts().io.into_inner();
                        if !token.is_cancelled() {
                            let timed_out = _error.is_timeout();
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
                    }
                });
            }
        }
    }
    let deadline = tokio::time::Instant::now() + cfg.limits.request_timeout;
    while !tasks.is_empty() {
        if tokio::time::timeout_at(deadline, tasks.join_next())
            .await
            .is_err()
        {
            break;
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
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
