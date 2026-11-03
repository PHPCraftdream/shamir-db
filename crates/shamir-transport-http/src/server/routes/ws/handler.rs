use super::hook::{UpgradeHead, WsReject, WsRoute, WsUpgrade};
use crate::config::Endpoint;
use crate::gate::{GateDecision, RestExecutor, TokenGate};
use crate::server::response::{canned, response};
use crate::server::service::{auth_header, peer, retry, unauthorized_body, Context};
use bytes::Bytes;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt};
use hyper::body::Incoming;
use std::sync::Arc;

/// Handles a WebSocket route before the ordinary REST bearer path.
pub(crate) async fn handle_ws<G, E, W>(
    mut request: Request<Incoming>,
    context: Arc<Context<G, E, W>>,
    route: WsRoute,
) -> Result<Response<BoxBody<Bytes, std::convert::Infallible>>, std::convert::Infallible>
where
    G: TokenGate,
    E: RestExecutor<G::Grant>,
    W: WsUpgrade<G::Grant>,
{
    if request.method() != Method::GET {
        let mut response = canned(
            StatusCode::METHOD_NOT_ALLOWED,
            context.peer.tls,
            context.cfg.hsts,
        );
        response
            .headers_mut()
            .insert(header::ALLOW, http::HeaderValue::from_static("GET"));
        return Ok(response);
    }
    let peer = peer(context.peer.addr, context.peer.tls, request.headers());
    let head = UpgradeHead {
        path: request.uri().path(),
        method: request.method(),
        headers: request.headers(),
    };
    let validated = match context.ws.check_head(route, head, peer.clone()) {
        Ok(head) => head,
        Err(reject) => return Ok(ws_rejection(reject, context.peer.tls, context.cfg.hsts)),
    };
    let grant = if route == WsRoute::Native {
        let auth = auth_header(request.headers());
        match tokio::time::timeout_at(
            tokio::time::Instant::now() + context.cfg.limits.request_timeout,
            context.gate.authenticate(auth, Endpoint::Ws, peer.clone()),
        )
        .await
        {
            Ok(GateDecision::Allow(grant)) => Some(grant),
            Ok(GateDecision::Unauthorized) => {
                let mut response = response(
                    StatusCode::UNAUTHORIZED,
                    unauthorized_body(),
                    context.peer.tls,
                    context.cfg.hsts,
                    true,
                );
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    http::HeaderValue::from_static("Bearer"),
                );
                return Ok(response);
            }
            Err(_) => {
                return Ok(canned(
                    StatusCode::REQUEST_TIMEOUT,
                    context.peer.tls,
                    context.cfg.hsts,
                ));
            }
            Ok(GateDecision::Forbidden(code)) => {
                return Ok(response(
                    StatusCode::FORBIDDEN,
                    (context.cfg.forbidden_body)(&code),
                    context.peer.tls,
                    context.cfg.hsts,
                    true,
                ))
            }
            Ok(GateDecision::RateLimited { retry_after_secs }) => {
                return Ok(retry(
                    StatusCode::TOO_MANY_REQUESTS,
                    retry_after_secs,
                    context.peer.tls,
                    context.cfg.hsts,
                ))
            }
            Ok(GateDecision::Unavailable { retry_after_secs }) => {
                return Ok(retry(
                    StatusCode::SERVICE_UNAVAILABLE,
                    retry_after_secs,
                    context.peer.tls,
                    context.cfg.hsts,
                ))
            }
        }
    } else {
        None
    };
    let (session, headers) = match context.ws.admit(route, validated, grant, peer) {
        Ok(admitted) => admitted,
        Err(reject) => return Ok(ws_rejection(reject, context.peer.tls, context.cfg.hsts)),
    };
    let body = http_body_util::Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert(header::UPGRADE, http::HeaderValue::from_static("websocket"));
    response.headers_mut().insert(
        header::CONNECTION,
        http::HeaderValue::from_static("Upgrade"),
    );
    for (name, value) in headers {
        if name != header::UPGRADE
            && name != header::CONNECTION
            && name.as_str() != "sec-websocket-extensions"
        {
            response.headers_mut().insert(name, value);
        }
    }
    let Some(on_upgrade) = request
        .extensions_mut()
        .remove::<hyper::upgrade::OnUpgrade>()
    else {
        return Ok(canned(
            StatusCode::BAD_REQUEST,
            context.peer.tls,
            context.cfg.hsts,
        ));
    };
    let mut slot = context.upgrade.lock().await;
    let Some(sender) = slot.take() else {
        return Ok(canned(
            StatusCode::BAD_REQUEST,
            context.peer.tls,
            context.cfg.hsts,
        ));
    };
    let _ = sender.send((session, on_upgrade));
    drop(slot);
    Ok(response)
}

fn ws_rejection(
    rejection: WsReject,
    tls: bool,
    hsts: bool,
) -> Response<BoxBody<Bytes, std::convert::Infallible>> {
    if rejection.status == StatusCode::UNAUTHORIZED {
        let mut response = response(
            StatusCode::UNAUTHORIZED,
            unauthorized_body(),
            tls,
            hsts,
            true,
        );
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            http::HeaderValue::from_static("Bearer"),
        );
        return response;
    }
    let mut response = canned(rejection.status, tls, hsts);
    if rejection.close {
        response
            .headers_mut()
            .insert(header::CONNECTION, http::HeaderValue::from_static("close"));
    }
    if let Some(seconds) = rejection.retry_after {
        if let Ok(value) = http::HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    response
}
