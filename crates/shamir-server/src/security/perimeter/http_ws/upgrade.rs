//! Token WebSocket HTTP head validation.

use base64::Engine;

use shamir_transport_http::server::routes::ws::{HeadOk, UpgradeHead, WsReject, WsRoute};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::http::{header, HeaderName, HeaderValue, Method, StatusCode};

use super::browser::policy::{host_allowed, valid_origin};

/// Secret-free data retained after successful HTTP head validation.
pub(crate) struct ValidatedHead {
    pub(crate) accept: HeaderValue,
}

/// Validates all security-sensitive headers before bearer-token verification.
pub(crate) fn validate_head(
    route: WsRoute,
    head: UpgradeHead<'_>,
    tls: bool,
    origins: &[String],
    host_allowlist: &[String],
) -> Result<HeadOk<ValidatedHead>, WsReject> {
    if head.method != &Method::GET {
        return Err(reject(StatusCode::METHOD_NOT_ALLOWED, false));
    }
    let one = |name: &str| -> Option<&[u8]> {
        let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
        let values = head.headers.get_all(name);
        let mut iter = values.iter();
        let first = iter.next()?.as_bytes();
        if iter.next().is_some() {
            None
        } else {
            Some(first)
        }
    };
    if !one("upgrade").is_some_and(|value| value.eq_ignore_ascii_case(b"websocket"))
        || !one("sec-websocket-version").is_some_and(|value| value == b"13")
        || !one("sec-websocket-key").is_some_and(valid_key)
        || head.headers.get_all("sec-websocket-key").iter().count() != 1
        || head.headers.get_all("sec-websocket-version").iter().count() != 1
        || head.headers.get_all("upgrade").iter().count() != 1
        || head
            .headers
            .get_all("sec-websocket-protocol")
            .iter()
            .count()
            != 1
        || !head
            .headers
            .get_all(header::CONNECTION)
            .iter()
            .any(|value| {
                value
                    .as_bytes()
                    .split(|byte| *byte == b',')
                    .any(|part| part.trim_ascii().eq_ignore_ascii_case(b"upgrade"))
            })
        || !head
            .headers
            .get_all("sec-websocket-protocol")
            .iter()
            .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
            .any(|part| part.trim_ascii() == b"shamir-v1")
    {
        return Err(reject(StatusCode::BAD_REQUEST, true));
    }
    if route == WsRoute::Native {
        if head.headers.get_all("origin").iter().count() > 1 {
            return Err(reject(StatusCode::FORBIDDEN, true));
        }
        if let Some(origin) = one("origin") {
            if !origins.iter().any(|allowed| allowed.as_bytes() == origin) {
                return Err(reject(StatusCode::FORBIDDEN, true));
            }
        }
    } else {
        if head.headers.get_all("origin").iter().count() != 1 {
            return Err(reject(StatusCode::BAD_REQUEST, true));
        }
        let origin = one("origin").ok_or_else(|| reject(StatusCode::BAD_REQUEST, true))?;
        let canonical =
            valid_origin(origin).ok_or_else(|| reject(StatusCode::BAD_REQUEST, true))?;
        if !origins.iter().any(|allowed| allowed == &canonical) {
            return Err(reject(StatusCode::FORBIDDEN, true));
        }
        if head.headers.get_all("host").iter().count() != 1 {
            return Err(reject(StatusCode::BAD_REQUEST, true));
        }
        let host = one("host").ok_or_else(|| reject(StatusCode::BAD_REQUEST, true))?;
        if !host_allowed(host, host_allowlist, tls) {
            return Err(reject(StatusCode::BAD_REQUEST, true));
        }
    }
    let key = one("sec-websocket-key").ok_or_else(|| reject(StatusCode::BAD_REQUEST, true))?;
    let accept = derive_accept_key(key);
    let accept =
        HeaderValue::from_str(&accept).map_err(|_| reject(StatusCode::BAD_REQUEST, true))?;
    Ok(HeadOk {
        state: ValidatedHead { accept },
    })
}

/// Validates a syntactically valid 16-byte Sec-WebSocket-Key.
fn valid_key(value: &[u8]) -> bool {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .is_ok_and(|decoded| decoded.len() == 16)
}

/// Converts a failed security check into a close-after-response rejection.
fn reject(status: StatusCode, close: bool) -> WsReject {
    WsReject {
        status,
        retry_after: None,
        close,
    }
}

/// Builds the standard protocol headers, deliberately excluding extensions.
pub(crate) fn response_headers(head: &ValidatedHead) -> Vec<(HeaderName, HeaderValue)> {
    vec![
        (header::UPGRADE, HeaderValue::from_static("websocket")),
        (header::CONNECTION, HeaderValue::from_static("Upgrade")),
        (
            HeaderName::from_static("sec-websocket-accept"),
            head.accept.clone(),
        ),
        (
            HeaderName::from_static("sec-websocket-protocol"),
            HeaderValue::from_static("shamir-v1"),
        ),
    ]
}
