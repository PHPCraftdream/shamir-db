use super::{LoginHead, LoginResponse, LoginRoutes};
use crate::gate::PeerInfo;
use bytes::Bytes;
use http::{header, HeaderValue, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

pub(crate) async fn handle_login<L: LoginRoutes>(
    request: Request<Incoming>,
    routes: Arc<L>,
    peer: PeerInfo,
    hsts: bool,
    request_started: tokio::time::Instant,
    request_limit: Duration,
    write_timeout_override: Arc<AtomicU64>,
) -> Response<BoxBody<Bytes, Infallible>> {
    let tls = peer.tls;
    if request.version() != http::Version::HTTP_11 {
        return finish_reply(
            error_response(StatusCode::BAD_REQUEST, "invalid_request"),
            tls,
            hsts,
            &write_timeout_override,
        );
    }
    let route = Bytes::copy_from_slice(request.uri().path().as_bytes());
    let request_deadline = request_started + request_limit.min(Duration::from_secs(120));
    let total_deadline = if routes.enabled() {
        request_deadline
    } else {
        request_started + request_limit
    };
    if !routes.enabled() {
        return finish_reply(
            LoginResponse::new(
                StatusCode::NOT_FOUND,
                Bytes::from_static(b"\x81\xa5error\xa9not_found"),
            ),
            tls,
            hsts,
            &write_timeout_override,
        );
    }
    if request.method() != http::Method::POST {
        return finish_reply(
            error_response(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
            tls,
            hsts,
            &write_timeout_override,
        );
    }
    let content_types: Vec<_> = request
        .headers()
        .get_all(header::CONTENT_TYPE)
        .iter()
        .collect();
    let encoding_values: Vec<_> = request
        .headers()
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .collect();
    if content_types.len() != 1
        || content_types[0].as_bytes() != b"application/msgpack"
        || !encoding_values.is_empty()
    {
        return finish_reply(
            error_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type"),
            tls,
            hsts,
            &write_timeout_override,
        );
    }
    let declared_values: Vec<_> = request
        .headers()
        .get_all(header::CONTENT_LENGTH)
        .iter()
        .collect();
    if declared_values.len() > 1
        || (request.headers().contains_key(header::TRANSFER_ENCODING)
            && !declared_values.is_empty())
    {
        return finish_reply(
            error_response(StatusCode::BAD_REQUEST, "invalid_request"),
            tls,
            hsts,
            &write_timeout_override,
        );
    }
    let transfer_values: Vec<_> = request
        .headers()
        .get_all(header::TRANSFER_ENCODING)
        .iter()
        .collect();
    if transfer_values.len() > 1
        || transfer_values
            .iter()
            .any(|value| value.as_bytes() != b"chunked")
    {
        return finish_reply(
            error_response(StatusCode::BAD_REQUEST, "invalid_request"),
            tls,
            hsts,
            &write_timeout_override,
        );
    }
    let protocol_limit = if route.ends_with(b"/begin") {
        4096
    } else {
        256
    };
    let limit = routes.body_limit(route.as_ref()).min(protocol_limit);
    let declared = match declared_values.first() {
        Some(value) => match value.to_str().ok().and_then(|s| s.parse::<usize>().ok()) {
            Some(length) if length <= limit => Some(length),
            Some(_) => {
                return finish_reply(
                    error_response(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"),
                    tls,
                    hsts,
                    &write_timeout_override,
                )
            }
            None => {
                return finish_reply(
                    error_response(StatusCode::BAD_REQUEST, "invalid_request"),
                    tls,
                    hsts,
                    &write_timeout_override,
                )
            }
        },
        None => None,
    };
    let head = LoginHead::new(
        request.method().as_str().as_bytes(),
        route.as_ref(),
        request.headers(),
    );
    let admission = tokio::time::timeout_at(
        total_deadline,
        routes.admit_head(route.as_ref(), head, peer),
    )
    .await;
    let permit = match admission {
        Err(_) => {
            return finish_reply(
                error_response(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
                tls,
                hsts,
                &write_timeout_override,
            )
        }
        Ok(Err(reply)) => return finish_reply(reply, tls, hsts, &write_timeout_override),
        Ok(Ok(permit)) => permit,
    };
    let admitted = tokio::time::Instant::now();
    let deadline = (admitted + Duration::from_secs(3)).min(total_deadline);
    let mut body = request.into_body();
    let mut collected = Vec::with_capacity(declared.unwrap_or(0).min(limit));
    let read = tokio::time::timeout_at(deadline, async {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| BodyReadFailure::Invalid)?;
            if let Ok(chunk) = frame.into_data() {
                let next = collected
                    .len()
                    .checked_add(chunk.len())
                    .ok_or(BodyReadFailure::TooLarge)?;
                if next > limit || declared.is_some_and(|length| next > length) {
                    return Err(BodyReadFailure::TooLarge);
                }
                collected.extend_from_slice(&chunk);
            }
        }
        Ok::<(), BodyReadFailure>(())
    })
    .await;
    match read {
        Err(_) => {
            return finish_reply(
                error_response(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
                tls,
                hsts,
                &write_timeout_override,
            )
        }
        Ok(Err(BodyReadFailure::TooLarge)) => {
            return finish_reply(
                error_response(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"),
                tls,
                hsts,
                &write_timeout_override,
            )
        }
        Ok(Err(BodyReadFailure::Invalid)) => {
            return finish_reply(
                error_response(StatusCode::BAD_REQUEST, "invalid_request"),
                tls,
                hsts,
                &write_timeout_override,
            )
        }
        Ok(Ok(())) => {}
    }
    match tokio::time::timeout_at(
        total_deadline,
        routes.handle(route, permit, Bytes::from(collected)),
    )
    .await
    {
        Ok(reply) => finish_reply(reply, tls, hsts, &write_timeout_override),
        Err(_) => finish_reply(
            error_response(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
            tls,
            hsts,
            &write_timeout_override,
        ),
    }
}

fn finish_reply(
    reply: LoginResponse,
    tls: bool,
    hsts: bool,
    write_timeout_override: &AtomicU64,
) -> Response<BoxBody<Bytes, Infallible>> {
    write_timeout_override.store(5000, Ordering::Release);
    login_response(reply, tls, hsts)
}

#[derive(Debug)]
enum BodyReadFailure {
    Invalid,
    TooLarge,
}

pub(super) fn login_response(
    reply: LoginResponse,
    tls: bool,
    hsts: bool,
) -> Response<BoxBody<Bytes, Infallible>> {
    let mut output = Response::new(
        Full::new(reply.body().clone())
            .map_err(|never| match never {})
            .boxed(),
    );
    *output.status_mut() = reply.status();
    let headers = output.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/msgpack"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    if tls && hsts {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    if let Some(seconds) = reply.retry_after() {
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            headers.insert(header::RETRY_AFTER, value);
        }
    }
    if reply.status() == StatusCode::METHOD_NOT_ALLOWED {
        headers.insert(header::ALLOW, HeaderValue::from_static("POST"));
    }
    output
}

fn error_response(status: StatusCode, code: &'static str) -> LoginResponse {
    let mut body = Vec::with_capacity(40);
    body.push(0x81);
    body.push(0xa5);
    body.extend_from_slice(b"error");
    body.push(0xa0 + u8::try_from(code.len()).unwrap_or(0));
    body.extend_from_slice(code.as_bytes());
    LoginResponse::new(status, Bytes::from(body))
}
