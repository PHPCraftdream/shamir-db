//! Per-transport accept loops for the launched server: TCP+TLS, plain TCP,
//! local IPC, native WSS and browser WSS. Each loop reserves connection
//! slots (global + per-IP) before any expensive handshake work and runs
//! every connection task under `run_until_cancelled`.

use std::sync::Arc;

use shamir_transport_ipc::IpcListener;
use shamir_transport_tcp::tls::extract_tls_exporter;
use shamir_transport_ws::browser::BrowserOriginPolicy;
use shamir_transport_ws::server::{accept_browser_ws, accept_native_ws};
use shamir_transport_ws::tls_exporter::BROWSER_CHANNEL_BINDING;

use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::conn_limiter::{ConnLimiter, PerIpLimiter};
use crate::connection::{handle_connection, ConnectionContext};
use crate::framer::{TcpFramer, WsFramer};

use super::helpers::{run_until_cancelled, IPC_PEER_ADDR};

/// Per-listener WS accept parameters (endpoint path + origin policy),
/// bundled so the accept-loop signatures stay under the clippy
/// argument limit.
pub(super) struct WsAcceptParams {
    pub(super) path: String,
    pub(super) policy: Arc<BrowserOriginPolicy>,
}

/// TCP+TLS accept loop. The accept_loop itself never blocks: every step
/// is async, and per-connection work is `tokio::spawn`'d so multiple
/// in-flight TLS handshakes / Argon2id verifies don't serialise on the
/// listener.
pub(super) async fn accept_loop_tcp(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
    per_ip: PerIpLimiter,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_tcp: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "tcp accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                // Reserve a slot BEFORE the TLS handshake so a saturated
                // server doesn't waste CPU on TLS for connections we're
                // about to close. Drop closes the TCP socket immediately
                // (the `tcp` binding goes out of scope) — kernel sends RST.
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached, refusing",
                        );
                        continue;
                    }
                };
                // Cheapest-check-first: the global cap already rejected
                // above; now enforce the per-source-IP cap before we burn
                // TLS CPU on a host that's already at its allowance. If
                // this rejects, the global `guard` drops here, releasing
                // the global slot (audit §2a / top-5 #4).
                let per_ip_guard = match per_ip.try_acquire(peer_addr.ip()) {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ip = %peer_addr.ip(),
                            active = per_ip.active(peer_addr.ip()),
                            cap = per_ip.cap(),
                            "max_active_connections_per_ip reached, refusing",
                        );
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                let tasks = ctx.tasks.clone();
                tasks.spawn(run_until_cancelled(shutdown.clone(), async move {
                    let _guard = guard;  // keep alive for the lifetime of this task
                    let _per_ip_guard = per_ip_guard;
                    // Bound the TLS handshake — without this a client that
                    // opens a TCP socket and never sends a ClientHello
                    // holds the slot forever (audit §2a / top-5 #4).
                    // Reuses `ctx.auth_init_timeout` (the same window that
                    // bounds the post-handshake `auth_init` read).
                    let tls = match tokio::time::timeout(
                        ctx.auth_init_timeout,
                        acceptor.accept(tcp),
                    ).await {
                        Ok(Ok(t)) => t,
                        Ok(Err(e)) => {
                            tracing::debug!(?peer_addr, ?e, "tls handshake failed");
                            return;
                        }
                        Err(_) => {
                            tracing::debug!(
                                ?peer_addr,
                                timeout = ?ctx.auth_init_timeout,
                                "tls handshake timed out",
                            );
                            return;
                        }
                    };
                    let exporter = match extract_tls_exporter(&tls) {
                        Ok(exporter) => exporter,
                        Err(e) => {
                            tracing::warn!(?peer_addr, ?e, "TLS exporter unavailable; closing connection");
                            return;
                        }
                    };
                    let framer = TcpFramer::new(tls);
                    handle_connection(ctx, peer_addr, framer, exporter).await;
                }));
            }
        }
    }
}

/// Plain (no TLS) TCP accept loop — `binding_mode = None`, channel binding
/// is always `[0u8; 32]` (spec TRANSPORT_TCP §4: `tcp+plain` always carries
/// zeroed `tls_exporter_or_zeros`). `bind_tcp`'s `ListenerProfile::Plain`
/// policy (loopback-only) already gated the bind itself before this loop
/// starts; `Config::validate` gates it a second time at config-load time.
/// No TLS handshake step, so no handshake-timeout wrapper is needed here —
/// `handle_connection` already bounds the first-frame (`auth_init`) read by
/// `ctx.auth_init_timeout`.
pub(super) async fn accept_loop_plain(
    listener: TcpListener,
    ctx: Arc<ConnectionContext>,
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
    per_ip: PerIpLimiter,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_plain: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "tcp accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached, refusing",
                        );
                        continue;
                    }
                };
                let per_ip_guard = match per_ip.try_acquire(peer_addr.ip()) {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ip = %peer_addr.ip(),
                            active = per_ip.active(peer_addr.ip()),
                            cap = per_ip.cap(),
                            "max_active_connections_per_ip reached, refusing",
                        );
                        continue;
                    }
                };
                let ctx = ctx.clone();
                let tasks = ctx.tasks.clone();
                tasks.spawn(run_until_cancelled(shutdown.clone(), async move {
                    let _guard = guard; // keep alive for the lifetime of this task
                    let _per_ip_guard = per_ip_guard;
                    let framer = TcpFramer::new(tcp);
                    handle_connection(ctx, peer_addr, framer, [0u8; 32]).await;
                }));
            }
        }
    }
}

/// Local-IPC (Unix domain socket / Windows Named Pipe) accept loop.
/// `binding_mode = None`, channel binding always `[0u8; 32]` — same
/// wire-level semantics as [`accept_loop_plain`], just over
/// `shamir_transport_ipc::IpcStream` instead of a bare `TcpStream`. No TLS
/// handshake, so (as with `accept_loop_plain`) no extra handshake-timeout
/// wrapper is needed — `handle_connection` already bounds the first-frame
/// read by `ctx.auth_init_timeout`.
pub(super) async fn accept_loop_ipc(
    mut listener: IpcListener,
    ctx: Arc<ConnectionContext>,
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
    per_ip: PerIpLimiter,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_ipc: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let stream = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ipc accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached, refusing (ipc)",
                        );
                        continue;
                    }
                };
                let per_ip_guard = match per_ip.try_acquire(IPC_PEER_ADDR.ip()) {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            active = per_ip.active(IPC_PEER_ADDR.ip()),
                            cap = per_ip.cap(),
                            "max_active_connections_per_ip reached, refusing (ipc)",
                        );
                        continue;
                    }
                };
                let ctx = ctx.clone();
                let tasks = ctx.tasks.clone();
                tasks.spawn(run_until_cancelled(shutdown.clone(), async move {
                    let _guard = guard; // keep alive for the lifetime of this task
                    let _per_ip_guard = per_ip_guard;
                    let framer = TcpFramer::new(stream);
                    handle_connection(ctx, IPC_PEER_ADDR, framer, [0u8; 32]).await;
                }));
            }
        }
    }
}

/// Native WSS accept loop — `tcp -> tls -> ws upgrade -> handle_connection`.
/// binding_mode = TlsExporter. The exporter is extracted BEFORE the WS
/// upgrade by choice: it keeps the extraction failure a cheap pre-upgrade
/// reject. (TLS remains reachable post-upgrade via `ws.get_ref()`; the
/// earlier ordering is a choice, not an access restriction.)
pub(super) async fn accept_loop_ws_native(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
    per_ip: PerIpLimiter,
    params: WsAcceptParams,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_ws_native: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ws accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached (ws native), refusing",
                        );
                        continue;
                    }
                };
                let per_ip_guard = match per_ip.try_acquire(peer_addr.ip()) {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ip = %peer_addr.ip(),
                            active = per_ip.active(peer_addr.ip()),
                            cap = per_ip.cap(),
                            "max_active_connections_per_ip reached (ws native), refusing",
                        );
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                let params = WsAcceptParams {
                    path: params.path.clone(),
                    policy: params.policy.clone(),
                };
                let tasks = ctx.tasks.clone();
                tasks.spawn(run_until_cancelled(shutdown.clone(), async move {
                    let _guard = guard;
                    let _per_ip_guard = per_ip_guard;
                    let tls = match tokio::time::timeout(
                        ctx.auth_init_timeout,
                        acceptor.accept(tcp),
                    ).await {
                        Ok(Ok(t)) => t,
                        Ok(Err(e)) => {
                            tracing::debug!(?peer_addr, ?e, "tls handshake failed (ws native)");
                            return;
                        }
                        Err(_) => {
                            tracing::debug!(
                                ?peer_addr,
                                timeout = ?ctx.auth_init_timeout,
                                "tls handshake timed out (ws native)",
                            );
                            return;
                        }
                    };
                    // Extract BEFORE the WS upgrade by choice (cheap
                    // pre-upgrade reject on failure); TLS stays reachable
                    // after the upgrade via `ws.get_ref()`.
                    let exporter = match extract_tls_exporter(&tls) {
                        Ok(exporter) => exporter,
                        Err(e) => {
                            tracing::warn!(?peer_addr, ?e, "TLS exporter unavailable; closing connection");
                            return;
                        }
                    };
                    let ws = match tokio::time::timeout(
                        ctx.auth_init_timeout,
                        accept_native_ws(tls, &params.path, &params.policy),
                    ).await {
                        Ok(Ok(ws)) => ws,
                        Ok(Err(e)) => {
                            tracing::debug!(?peer_addr, ?e, "ws native upgrade failed");
                            return;
                        }
                        Err(_) => {
                            tracing::debug!(
                                ?peer_addr,
                                timeout = ?ctx.auth_init_timeout,
                                "ws native upgrade timed out",
                            );
                            return;
                        }
                    };
                    let framer = WsFramer::new(ws);
                    handle_connection(ctx, peer_addr, framer, exporter).await;
                }));
            }
        }
    }
}

/// Browser WSS accept loop — origin-allowlist enforced inside
/// `accept_browser_ws`. binding_mode = TlsNoExport (exporter = zeros per
/// spec §6.4 because the JS environment can't access it).
pub(super) async fn accept_loop_ws_browser(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<ConnectionContext>,
    shutdown: tokio_util::sync::CancellationToken,
    limiter: ConnLimiter,
    per_ip: PerIpLimiter,
    params: WsAcceptParams,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                tracing::debug!("accept_loop_ws_browser: shutdown cancelled, exiting");
                break;
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, "ws-browser accept failed; sleeping briefly");
                        tokio::time::sleep(shamir_tunables::instance_defaults::SERVER_POLL_INTERVAL).await;
                        continue;
                    }
                };
                let guard = match limiter.try_acquire() {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ?peer_addr,
                            active = limiter.active(),
                            cap = limiter.cap(),
                            "max_active_connections reached (ws browser), refusing",
                        );
                        continue;
                    }
                };
                let per_ip_guard = match per_ip.try_acquire(peer_addr.ip()) {
                    Some(g) => g,
                    None => {
                        tracing::debug!(
                            ip = %peer_addr.ip(),
                            active = per_ip.active(peer_addr.ip()),
                            cap = per_ip.cap(),
                            "max_active_connections_per_ip reached (ws browser), refusing",
                        );
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let ctx = ctx.clone();
                let params = WsAcceptParams {
                    path: params.path.clone(),
                    policy: params.policy.clone(),
                };
                let tasks = ctx.tasks.clone();
                tasks.spawn(run_until_cancelled(shutdown.clone(), async move {
                    let _guard = guard;
                    let _per_ip_guard = per_ip_guard;
                    let tls = match tokio::time::timeout(
                        ctx.auth_init_timeout,
                        acceptor.accept(tcp),
                    ).await {
                        Ok(Ok(t)) => t,
                        Ok(Err(e)) => {
                            tracing::debug!(?peer_addr, ?e, "tls handshake failed (ws browser)");
                            return;
                        }
                        Err(_) => {
                            tracing::debug!(
                                ?peer_addr,
                                timeout = ?ctx.auth_init_timeout,
                                "tls handshake timed out (ws browser)",
                            );
                            return;
                        }
                    };
                    // Browser binding mode: exporter MUST be zeros per spec.
                    let exporter = BROWSER_CHANNEL_BINDING;
                    let ws = match tokio::time::timeout(
                        ctx.auth_init_timeout,
                        accept_browser_ws(tls, &params.path, &params.policy),
                    ).await {
                        Ok(Ok(ws)) => ws,
                        Ok(Err(e)) => {
                            tracing::debug!(?peer_addr, ?e, "ws browser upgrade failed");
                            return;
                        }
                        Err(_) => {
                            tracing::debug!(
                                ?peer_addr,
                                timeout = ?ctx.auth_init_timeout,
                                "ws browser upgrade timed out",
                            );
                            return;
                        }
                    };
                    let framer = WsFramer::new(ws);
                    handle_connection(ctx, peer_addr, framer, exporter).await;
                }));
            }
        }
    }
}
