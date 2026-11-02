use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use shamir_query_types::wire::DbRequest;
use shamir_transport_http::{serve_with_ws, BodyBudget, ConnAdmission, EndpointSet};

use crate::boot::config::{to_http_server_config, ListenerConfig, ListenerKind};
use crate::conn_limiter::{ConnLimiter, PerIpLimiter};
use crate::http_ws::{PreAuthAdmission, WsUpgradeService};
use crate::server::boot_error::BootError;
use shamir_connect::server::token_session::TokenSocketCaps;
use shamir_tunables::instance_defaults::{WS_PING_INTERVAL, WS_PONG_TIMEOUT};

use super::super::listeners::ListenerEnv;
use super::proxy::ProxyGate;
use super::validate_http_tls;

type StartedHttp = (SocketAddr, JoinHandle<()>, Arc<RestGate>);
type UsernameLookup = Arc<dyn Fn(&[u8; 16]) -> String + Send + Sync>;

struct HttpAdmission {
    global: ConnLimiter,
    per_ip: PerIpLimiter,
}

impl ConnAdmission for HttpAdmission {
    fn admit(&self, peer: &SocketAddr) -> Option<Box<dyn Send + Sync>> {
        let global = self.global.try_acquire()?;
        let per_ip = self.per_ip.try_acquire(peer.ip())?;
        Some(Box::new((global, per_ip)))
    }
}

pub(in crate::engine::server::server_launcher) async fn start_http_listener(
    env: &ListenerEnv<'_>,
    listener_config: &ListenerConfig,
    global: &ConnLimiter,
    per_ip: &PerIpLimiter,
    body_budget: Arc<BodyBudget>,
    shutdown: &CancellationToken,
) -> Result<StartedHttp, BootError> {
    if listener_config.kind != ListenerKind::Http {
        return Err(BootError::Bind(
            "HTTP listener setup used for non-HTTP listener".into(),
        ));
    }
    let address: SocketAddr = listener_config.addr.parse().map_err(|error| {
        BootError::Bind(format!("HTTP address {}: {error}", listener_config.addr))
    })?;
    let listener = TcpListener::bind(address)
        .await
        .map_err(|error| BootError::Bind(format!("HTTP bind {address}: {error}")))?;
    let address = listener
        .local_addr()
        .map_err(|error| BootError::Bind(format!("HTTP local address: {error}")))?;
    let settings = &listener_config.http;
    let tls = if settings.tls == Some(true) {
        let validated = validate_http_tls(&env.config.tls, address, settings.allow_self_signed)?;
        Some(
            shamir_transport_http::server::accept::tls13_config(&validated)
                .map_err(|error| BootError::Bind(format!("HTTP TLS config: {error}")))?,
        )
    } else {
        None
    };
    let gate_clock: Arc<dyn Fn() -> Option<u64> + Send + Sync> = Arc::new(|| {
        shamir_connect::common::time::UnixNanos::try_now()
            .ok()
            .map(|value| value.as_u64())
    });
    let username = Arc::clone(env.user_dir);
    let username_lookup: UsernameLookup = Arc::new(move |id| {
        username
            .state_by_user_id(id)
            .map(|state| state.username)
            .unwrap_or_default()
    });
    let rails = RestGateRails {
        plaintext_remote: settings.tls == Some(false) && !address.ip().is_loopback(),
        allow_plaintext_write: settings.allow_plaintext_write,
    };
    if rails.plaintext_remote {
        tracing::warn!(local_addr = %address, "remote plaintext HTTP exposes bearer tokens");
        metrics::describe_gauge!(
            "shamir_token_listener_plaintext_remote",
            "Whether a remote plaintext token listener is enabled"
        );
        metrics::gauge!("shamir_token_listener_plaintext_remote").set(1.0);
    }
    let (gate, executor) = RestHandler::new(
        Arc::clone(env.token_service),
        Arc::clone(env.handler_concrete),
        gate_clock,
        username_lookup,
        rails,
    );
    let gate = Arc::new(gate);
    let proxy_gate = match ProxyGate::new(Arc::clone(&gate), &settings.trusted_proxies) {
        Ok(proxy_gate) => Arc::new(proxy_gate),
        Err(error) => {
            gate.shutdown().await;
            return Err(error);
        }
    };
    let forbidden_body = Arc::new(|code: &str| {
        let response = shamir_query_types::wire::DbResponse::Error {
            code: code.to_string(),
            message: code.to_string(),
        };
        rmp_serde::to_vec_named(&response)
            .map(bytes::Bytes::from)
            .unwrap_or_else(|_| {
                bytes::Bytes::from_static(
                    b"\x83\xa4kind\xa5error\xa4code\xabserver_busy\xa7message\xabserver_busy",
                )
            })
    });
    let ping = rmp_serde::to_vec_named(&DbRequest::Ping)
        .map(bytes::Bytes::from)
        .map_err(|error| BootError::Bind(format!("HTTP ping encoding: {error}")));
    let config = match ping.and_then(|ping| {
        to_http_server_config(
            settings,
            EndpointSet {
                rest: true,
                ws: true,
            },
            body_budget,
            env.config.security.connection.auth_init_timeout_ms,
            tls,
            forbidden_body,
            ping,
        )
        .map_err(|error| BootError::Bind(error.to_string()))
    }) {
        Ok(config) => config,
        Err(error) => {
            gate.shutdown().await;
            return Err(error);
        }
    };
    let admission = Arc::new(HttpAdmission {
        global: global.clone(),
        per_ip: per_ip.clone(),
    });
    let task_gate = proxy_gate;
    let task_shutdown = shutdown.child_token();
    let ws_service = Arc::new(WsUpgradeService {
        gate: Arc::clone(&gate),
        service: Arc::clone(env.token_service),
        handler: Arc::clone(env.handler_concrete),
        registry: Arc::clone(env.token_ws_cancel_registry),
        pre_auth: Arc::new(PreAuthAdmission::default()),
        tls: settings.tls == Some(true),
        browser_available: settings.tls == Some(true) || address.ip().is_loopback(),
        origins: listener_config.browser_origin_allowlist.clone(),
        host_allowlist: settings.host_authority_allowlist.clone(),
        username: Arc::clone(&username_lookup),
        caps: TokenSocketCaps {
            per_token: env.config.tokens.max_sockets_per_token,
            per_owner: env.config.tokens.max_sockets_per_owner,
        },
        session_max_age_hours: env.config.tokens.session_max_age_hours,
        ping_interval: WS_PING_INTERVAL,
        pong_timeout: WS_PONG_TIMEOUT,
        max_in_flight: 8,
        resolve_peer: Arc::new({
            let trusted = settings
                .trusted_proxies
                .iter()
                .filter_map(|item| super::proxy::parse_cidr(item))
                .collect::<Vec<_>>();
            move |peer| super::proxy::resolve_peer(peer, &trusted)
        }),
    });
    let span = tracing::Span::current();
    let task = tokio::spawn(
        async move {
            if let Err(error) = serve_with_ws(
                listener,
                Arc::new(config),
                task_gate,
                Arc::new(executor),
                admission,
                task_shutdown,
                ws_service,
            )
            .await
            {
                tracing::error!(%error, "HTTP listener stopped");
            }
        }
        .instrument(span),
    );
    tracing::info!(local_addr = %address, kind = "http", "listener bound");
    Ok((address, task, gate))
}
