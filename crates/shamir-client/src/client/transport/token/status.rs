use std::time::Duration;

use shamir_query_types::wire::DbResponse;

use super::http_conn::TokenReply;
use crate::error::ClientError;

const MIN_RETRY_AFTER_SECS: u64 = 1;
const MAX_RETRY_AFTER_SECS: u64 = 3600;
const MAX_CODE_LEN: usize = 64;

/// Delta-seconds `Retry-After` only (HTTP-dates and anything else give `None`),
/// clamped to 1..=3600 seconds.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secs = value.parse::<u64>().unwrap_or(u64::MAX);
    Some(Duration::from_secs(
        secs.clamp(MIN_RETRY_AFTER_SECS, MAX_RETRY_AFTER_SECS),
    ))
}

/// Maps a reply of `POST /shamir/v1/request`. A 200 body is returned
/// unmapped; the caller applies the visibility mapping.
pub(crate) fn map_reply(reply: TokenReply) -> Result<DbResponse, ClientError> {
    if reply.status == 200 {
        if !reply.content_type_msgpack || reply.has_content_encoding {
            return Err(ClientError::Protocol(
                "unexpected response content type".into(),
            ));
        }
        return Ok(rmp_serde::from_slice::<DbResponse>(&reply.body)?);
    }
    Err(map_failure(&reply))
}

/// Maps a reply of `GET /shamir/v1/ping`; only the status matters.
pub(crate) fn map_ping_reply(reply: TokenReply) -> Result<(), ClientError> {
    if reply.status == 200 {
        return Ok(());
    }
    Err(map_failure(&reply))
}

/// Maps a non-101 answer of the `GET /shamir/v1/ws` upgrade.
pub(super) fn map_upgrade_reply(reply: TokenReply) -> ClientError {
    if reply.status == 200 {
        return ClientError::Protocol("websocket upgrade not performed".into());
    }
    map_failure(&reply)
}

fn map_failure(reply: &TokenReply) -> ClientError {
    match reply.status {
        400 | 403 => match decode_db_error(reply) {
            Some((code, message)) => ClientError::Db { code, message },
            None => http_error(reply.status, None, None),
        },
        401 => http_error(401, Some("unauthorized"), None),
        429 | 503 => http_error(reply.status, None, reply.retry_after),
        status => http_error(status, None, None),
    }
}

fn decode_db_error(reply: &TokenReply) -> Option<(String, String)> {
    match rmp_serde::from_slice::<DbResponse>(&reply.body) {
        Ok(DbResponse::Error { code, message }) => Some((code, message)),
        _ => None,
    }
}

fn http_error(status: u16, code: Option<&str>, retry_after: Option<Duration>) -> ClientError {
    ClientError::Http {
        status,
        code: code.filter(|c| valid_code(c)).map(str::to_owned),
        retry_after,
    }
}

/// `[a-z][a-z0-9_]{0,63}`
fn valid_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    bytes.len() <= MAX_CODE_LEN
        && first.is_ascii_lowercase()
        && rest
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}
