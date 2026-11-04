use super::body::response_body;
use super::response::{canned, response, response_held};
use crate::config::{Endpoint, HttpServerConfig};
use crate::gate::{AuthHeader, GateDecision, PeerInfo, RestExecutor, TokenGate};
use crate::server::routes::ws::{WsRoute, WsUpgrade};
use crate::server::routes::LoginRoutes;
use bytes::Bytes;
use http::{header, HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt};
use hyper::body::Incoming;
use std::sync::Arc;

type UpgradeSlot<S> =
    tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<(S, hyper::upgrade::OnUpgrade)>>>;

pub(crate) struct Context<G, E, W = super::accept::NoWs, L = super::routes::NoLogin>
where
    G: TokenGate,
    W: WsUpgrade<G::Grant>,
    L: LoginRoutes,
{
    pub cfg: Arc<HttpServerConfig>,
    pub gate: Arc<G>,
    pub exec: Arc<E>,
    pub peer: PeerInfo,
    pub holds: tokio::sync::mpsc::Sender<Box<dyn Send + Sync>>,
    pub ws: Arc<W>,
    pub upgrade: UpgradeSlot<W::Session>,
    pub login: Arc<L>,
    pub write_timeout_override: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) async fn handle<G, E, W, L>(
    request: Request<Incoming>,
    context: Arc<Context<G, E, W, L>>,
) -> Result<Response<BoxBody<Bytes, std::convert::Infallible>>, std::convert::Infallible>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    W: WsUpgrade<G::Grant>,
    L: LoginRoutes,
{
    if request.version() != http::Version::HTTP_11 {
        return Ok(canned(
            StatusCode::BAD_REQUEST,
            context.peer.tls,
            context.cfg.hsts,
        ));
    }
    let started = tokio::time::Instant::now();
    let deadline = started + context.cfg.limits.request_timeout;
    let path = request.uri().path().to_owned();
    if path == "/shamir/v1/auth/begin" || path == "/shamir/v1/auth/finish" {
        let request_peer = peer(context.peer.addr, context.peer.tls, request.headers());
        let mut response = super::routes::handle_login(
            request,
            context.login.clone(),
            request_peer,
            context.cfg.hsts,
            started,
            context.cfg.limits.request_timeout,
            context.write_timeout_override.clone(),
        )
        .await;
        if let Some(hold) = response.extensions_mut().remove::<Arc<dyn Send + Sync>>() {
            // Closed means the connection is gone; Full cannot happen (one in-flight request,
            // consumed on the first response write). Either way a failed hold is dropped here.
            let _ = context
                .holds
                .try_send(Box::new(hold) as Box<dyn Send + Sync>);
        }
        return Ok(response);
    }
    let ws_route = match path.as_str() {
        "/shamir/v1/ws" if context.cfg.endpoints.ws => Some(WsRoute::Native),
        "/shamir/v1/ws/browser" if context.cfg.endpoints.ws => Some(WsRoute::Browser),
        _ => None,
    };
    if let Some(route) = ws_route {
        return super::routes::ws::handler::handle_ws(request, context, route).await;
    }
    let endpoint = if path == "/shamir/v1/ws" {
        Endpoint::Ws
    } else {
        Endpoint::Rest
    };
    let known = match path.as_str() {
        "/shamir/v1/request" | "/shamir/v1/ping" => context.cfg.endpoints.rest,
        "/shamir/v1/ws" => context.cfg.endpoints.ws,
        _ => false,
    };
    let auth = auth_header(request.headers());
    let peer = peer(context.peer.addr, context.peer.tls, request.headers());
    let decision =
        tokio::time::timeout_at(deadline, context.gate.authenticate(auth, endpoint, peer)).await;
    let grant = match decision {
        Err(_) => {
            return Ok(canned(
                StatusCode::REQUEST_TIMEOUT,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        Ok(GateDecision::Allow(g)) => g,
        Ok(GateDecision::Unauthorized) => {
            let mut r = response(
                StatusCode::UNAUTHORIZED,
                unauthorized_body(),
                context.peer.tls,
                context.cfg.hsts,
                true,
            );
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                http::HeaderValue::from_static("Bearer"),
            );
            return Ok(r);
        }
        Ok(GateDecision::Forbidden(code)) => {
            return Ok(response(
                StatusCode::FORBIDDEN,
                (context.cfg.forbidden_body)(&code),
                context.peer.tls,
                context.cfg.hsts,
                true,
            ));
        }
        Ok(GateDecision::RateLimited { retry_after_secs }) => {
            return Ok(retry(
                StatusCode::TOO_MANY_REQUESTS,
                retry_after_secs,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        Ok(GateDecision::Unavailable { retry_after_secs }) => {
            return Ok(retry(
                StatusCode::SERVICE_UNAVAILABLE,
                retry_after_secs,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
    };
    if !known {
        return Ok(canned(
            StatusCode::NOT_FOUND,
            context.peer.tls,
            context.cfg.hsts,
        ));
    }
    let method = match path.as_str() {
        "/shamir/v1/request" => Method::POST,
        "/shamir/v1/ping" | "/shamir/v1/ws" => Method::GET,
        _ => Method::GET,
    };
    if request.method() != method {
        let mut r = canned(
            StatusCode::METHOD_NOT_ALLOWED,
            context.peer.tls,
            context.cfg.hsts,
        );
        r.headers_mut().insert(
            header::ALLOW,
            http::HeaderValue::from_static(if method == Method::GET { "GET" } else { "POST" }),
        );
        return Ok(r);
    }
    if path.ends_with("/request")
        && !request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/msgpack"))
            })
    {
        return Ok(canned(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            context.peer.tls,
            context.cfg.hsts,
        ));
    }
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let mut reservation = match declared {
        Some(n) => {
            if n > context.cfg.limits.max_body_bytes {
                return Ok(canned(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    context.peer.tls,
                    context.cfg.hsts,
                ));
            }
            match context.cfg.limits.body_budget.try_reserve(n) {
                Some(r) => Some(r),
                None => {
                    return Ok(retry(
                        StatusCode::SERVICE_UNAVAILABLE,
                        1,
                        context.peer.tls,
                        context.cfg.hsts,
                    ));
                }
            }
        }
        None => None,
    };
    let mut body = request.into_body();
    let mut collected = Vec::new();
    let body_deadline = (started + context.cfg.limits.body_timeout).min(deadline);
    let read = tokio::time::timeout_at(body_deadline, async {
        while let Some(frame) = body.frame().await {
            let frame = frame?;
            if let Ok(chunk) = frame.into_data() {
                let next = collected
                    .len()
                    .checked_add(chunk.len())
                    .ok_or(BodyError::TooLarge)?;
                if next > context.cfg.limits.max_body_bytes {
                    return Err(BodyError::TooLarge);
                }
                if let Some(declared_bytes) = declared {
                    if next > declared_bytes {
                        return Err(BodyError::TooLarge);
                    }
                } else if let Some(res) = reservation.as_mut() {
                    if !res.grow(chunk.len()) {
                        return Err(BodyError::Budget);
                    }
                } else {
                    reservation = Some(
                        context
                            .cfg
                            .limits
                            .body_budget
                            .try_reserve(chunk.len())
                            .ok_or(BodyError::Budget)?,
                    );
                }
                collected.extend_from_slice(&chunk);
            }
        }
        Ok::<(), BodyError>(())
    })
    .await;
    match read {
        Err(_) => {
            return Ok(canned(
                StatusCode::REQUEST_TIMEOUT,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        Ok(Err(BodyError::TooLarge)) => {
            return Ok(canned(
                StatusCode::PAYLOAD_TOO_LARGE,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        Ok(Err(BodyError::Budget)) => {
            return Ok(retry(
                StatusCode::SERVICE_UNAVAILABLE,
                1,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        Ok(Err(BodyError::Hyper)) => {
            return Ok(canned(
                StatusCode::BAD_REQUEST,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        Ok(Ok(())) => {}
    }
    let reservation = reservation.take();
    match context.gate.revalidate(&grant) {
        GateDecision::Allow(()) => {}
        GateDecision::Unauthorized => {
            let mut r = response(
                StatusCode::UNAUTHORIZED,
                unauthorized_body(),
                context.peer.tls,
                context.cfg.hsts,
                true,
            );
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                http::HeaderValue::from_static("Bearer"),
            );
            return Ok(r);
        }
        GateDecision::Unavailable { retry_after_secs } => {
            return Ok(retry(
                StatusCode::SERVICE_UNAVAILABLE,
                retry_after_secs.max(1),
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        GateDecision::RateLimited { retry_after_secs } => {
            return Ok(retry(
                StatusCode::TOO_MANY_REQUESTS,
                retry_after_secs.max(1),
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
        GateDecision::Forbidden(code) => {
            return Ok(response(
                StatusCode::FORBIDDEN,
                (context.cfg.forbidden_body)(&code),
                context.peer.tls,
                context.cfg.hsts,
                true,
            ));
        }
    }
    let payload = if path.ends_with("/ping") {
        context.cfg.ping_body.clone()
    } else {
        Bytes::from(collected)
    };
    let outcome =
        match tokio::time::timeout_at(deadline, context.exec.execute(&grant, payload)).await {
            Ok(v) => v,
            Err(_) => {
                return Ok(canned(
                    StatusCode::REQUEST_TIMEOUT,
                    context.peer.tls,
                    context.cfg.hsts,
                ));
            }
        };
    drop(reservation);
    let body = match tokio::time::timeout_at(
        deadline,
        response_body(outcome.body, outcome.hold, context.holds.clone()),
    )
    .await
    {
        Ok(body) => body,
        Err(_) => {
            return Ok(canned(
                StatusCode::REQUEST_TIMEOUT,
                context.peer.tls,
                context.cfg.hsts,
            ));
        }
    };
    let mut r = response_held(
        exec_status(outcome.status),
        body,
        context.peer.tls,
        context.cfg.hsts,
        false,
    );
    if matches!(outcome.status, crate::gate::ExecStatus::Unauthorized) {
        r = response(
            StatusCode::UNAUTHORIZED,
            unauthorized_body(),
            context.peer.tls,
            context.cfg.hsts,
            true,
        );
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            http::HeaderValue::from_static("Bearer"),
        );
    }
    if matches!(outcome.status, crate::gate::ExecStatus::Unavailable) {
        r.headers_mut()
            .insert(header::CONNECTION, http::HeaderValue::from_static("close"));
    }
    if matches!(
        outcome.status,
        crate::gate::ExecStatus::RateLimited | crate::gate::ExecStatus::Unavailable
    ) {
        let delay = outcome.retry_after_secs.unwrap_or(1);
        if let Ok(value) = http::HeaderValue::from_str(&delay.to_string()) {
            r.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    Ok(r)
}

#[derive(Debug)]
enum BodyError {
    TooLarge,
    Budget,
    Hyper,
}
impl From<hyper::Error> for BodyError {
    fn from(_: hyper::Error) -> Self {
        Self::Hyper
    }
}
pub(crate) fn exec_status(s: crate::gate::ExecStatus) -> StatusCode {
    match s {
        crate::gate::ExecStatus::Ok => StatusCode::OK,
        crate::gate::ExecStatus::BadRequest => StatusCode::BAD_REQUEST,
        crate::gate::ExecStatus::Unauthorized => StatusCode::UNAUTHORIZED,
        crate::gate::ExecStatus::Forbidden => StatusCode::FORBIDDEN,
        crate::gate::ExecStatus::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        crate::gate::ExecStatus::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
pub(crate) fn unauthorized_body() -> Bytes {
    Bytes::from_static(b"\x81\xa5error\xacunauthorized")
}
pub(crate) fn auth_header(headers: &HeaderMap) -> AuthHeader<'_> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    match (values.next(), values.next()) {
        (None, _) => AuthHeader::Missing,
        (Some(v), None) => AuthHeader::Single(v.as_bytes()),
        _ => AuthHeader::Multiple,
    }
}
pub(crate) fn retry(
    status: StatusCode,
    seconds: u64,
    tls: bool,
    hsts: bool,
) -> Response<BoxBody<Bytes, std::convert::Infallible>> {
    let mut r = canned(status, tls, hsts);
    if let Ok(v) = http::HeaderValue::from_str(&seconds.to_string()) {
        r.headers_mut().insert(header::RETRY_AFTER, v);
    }
    r
}
pub(crate) fn peer(addr: std::net::SocketAddr, tls: bool, headers: &HeaderMap) -> PeerInfo {
    PeerInfo {
        addr,
        tls,
        forwarded_for: headers
            .get_all("x-forwarded-for")
            .iter()
            .map(|h| h.as_bytes().to_vec())
            .collect(),
    }
}
