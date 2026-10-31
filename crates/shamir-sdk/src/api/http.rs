//! HTTP egress types for the guest SDK (slice 8c).
//!
//! Usage:
//! ```ignore
//! let resp = ctx.http_fetch(HttpRequest::get("https://api.example.com/data"))?;
//! let body = resp.body();
//! ```
//!
//! # Wire shape
//!
//! `HttpRequest` serialises as a msgpack `Value::Map`:
//! ```text
//! { "method": Str, "url": Str, "headers": List<[Str, Str]>, "body": Bin }
//! ```
//!
//! The host returns an envelope `Value::List([Bool, payload])`:
//! - `[true, { "status": Int, "headers": List<[Str,Str]>, "body": Bin }]` on success.
//! - `[false, "error message"]` on runtime error.

use crate::error::{Error, Result};
use crate::Value;

/// Decode the host envelope `[ok: Bool, payload]` into a Result.
pub(crate) fn decode_fetch_envelope(raw: &Value) -> Result<HttpResponse> {
    let items = match raw {
        Value::List(items) => items,
        _ => return Err(Error::codec("http_fetch: host returned unexpected value")),
    };
    if items.len() != 2 {
        return Err(Error::codec("http_fetch: host envelope has wrong shape"));
    }
    let ok = match &items[0] {
        Value::Bool(b) => *b,
        _ => return Err(Error::codec("http_fetch: envelope ok-flag is not a bool")),
    };
    if !ok {
        let msg = match &items[1] {
            Value::Str(s) => s.clone(),
            other => format!("{other:?}"),
        };
        return Err(Error::user(msg));
    }
    HttpResponse::from_value(&items[1])
}

/// Validate an HTTP token according to RFC 9110 §5.6.2.
fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        ..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
}

fn validate_method(method: &str) -> Result<()> {
    if is_token(method) {
        Ok(())
    } else {
        Err(Error::invalid_argument("http_fetch: invalid method token"))
    }
}

/// Validate an HTTP URL as an absolute HTTP(S) URL without userinfo.
// Hostnames are dot-separated non-empty labels; the absolute FQDN form with a
// trailing dot (`example.com.`) is rejected.
fn validate_url(url: &str) -> Result<()> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(Error::invalid_argument(
            "http_fetch: URL must be absolute HTTP(S)",
        ));
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || rest.is_empty()
        || url.bytes().any(|b| b <= 0x20 || b == 0x7f)
    {
        return Err(Error::invalid_argument("http_fetch: invalid URL"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let valid_port = |port: &str| {
        !port.is_empty()
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok()
    };
    let valid_host = |host: &str| {
        !host.is_empty()
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && label
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && label
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
            })
    };
    let valid_authority = if let Some(bracketed) = authority.strip_prefix('[') {
        match bracketed.split_once(']') {
            Some((address, suffix)) => {
                address.parse::<std::net::Ipv6Addr>().is_ok()
                    && (suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port))
            }
            None => false,
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => valid_host(host) && valid_port(port),
            None => valid_host(authority),
        }
    };
    if !valid_authority || authority.contains('@') {
        return Err(Error::invalid_argument("http_fetch: invalid URL authority"));
    }
    Ok(())
}

fn validate_header(name: &str, value: &str) -> Result<()> {
    if !is_token(name) || value.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f) {
        return Err(Error::invalid_argument(
            "http_fetch: invalid header name or value",
        ));
    }
    Ok(())
}

/// An HTTP request for the egress gateway.
///
/// Construct with [`HttpRequest::get`] or [`HttpRequest::post`], or build
/// manually and pass to [`crate::Ctx::http_fetch`].
#[derive(Debug, Clone)]
pub struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    /// Build a GET request.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".to_string(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Build a POST request with a body.
    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "POST".to_string(),
            url: url.into(),
            headers: Vec::new(),
            body,
        }
    }

    /// Add a header. Invalid names or values are rejected by `try_header`
    /// or by `try_to_value` (used by `Ctx::http_fetch`).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Add a validated HTTP header.
    pub fn try_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let value = value.into();
        validate_header(&name, &value)?;
        self.headers.push((name, value));
        Ok(self)
    }

    /// Validate and convert to the wire `Value::Map` shape.
    pub fn try_to_value(&self) -> Result<Value> {
        validate_method(&self.method)?;
        validate_url(&self.url)?;
        for (name, value) in &self.headers {
            validate_header(name, value)?;
        }
        Ok(self.to_value())
    }

    /// Set the HTTP method.
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// Set the body.
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// Convert to the wire `Value::Map` shape without validation; use
    /// [`HttpRequest::try_to_value`] to reject invalid fields.
    pub fn to_value(&self) -> Value {
        // Pair-list form so duplicate header names (Set-Cookie) survive.
        let header_pairs: Vec<Value> = self
            .headers
            .iter()
            .map(|(k, v)| Value::List(vec![Value::Str(k.clone()), Value::Str(v.clone())]))
            .collect();

        Value::Map(vec![
            ("method".to_string(), Value::Str(self.method.clone())),
            ("url".to_string(), Value::Str(self.url.clone())),
            ("headers".to_string(), Value::List(header_pairs)),
            ("body".to_string(), Value::Bin(self.body.clone())),
        ])
    }
}

/// An HTTP response from the egress gateway.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    /// Parse from the wire `Value::Map` shape.
    pub fn from_value(val: &Value) -> Result<Self> {
        let map = match val {
            Value::Map(entries) => entries,
            _ => return Err(Error::codec("http_fetch: response is not a Map")),
        };

        let count = |name: &str| map.iter().filter(|(key, _)| key == name).count();
        for field in ["status", "headers", "body"] {
            if count(field) > 1 {
                return Err(Error::codec(format!(
                    "http_fetch: duplicate response {field} field"
                )));
            }
        }

        let status = map
            .iter()
            .find(|(k, _)| k == "status")
            .and_then(|(_, v)| match v {
                Value::Int(n) if (100..=599).contains(n) => u16::try_from(*n).ok(),
                _ => None,
            })
            .ok_or_else(|| Error::codec("http_fetch: invalid or missing status field"))?;

        // List-of-pairs is canonical; map form accepted for legacy hosts.
        let headers = match map.iter().find(|(k, _)| k == "headers") {
            Some((_, Value::List(items))) => items
                .iter()
                .map(|item| match item {
                    Value::List(pair) if pair.len() == 2 => match (&pair[0], &pair[1]) {
                        (Value::Str(k), Value::Str(v)) => Ok((k.clone(), v.clone())),
                        _ => Err(Error::codec("http_fetch: malformed response header pair")),
                    },
                    _ => Err(Error::codec("http_fetch: malformed response header pair")),
                })
                .collect::<Result<Vec<_>>>()?,
            Some((_, Value::Map(entries))) => entries
                .iter()
                .map(|(k, v)| match v {
                    Value::Str(s) => Ok((k.clone(), s.clone())),
                    _ => Err(Error::codec("http_fetch: malformed response header value")),
                })
                .collect::<Result<Vec<_>>>()?,
            Some(_) => return Err(Error::codec("http_fetch: malformed response headers")),
            None => Vec::new(),
        };

        let body = match map.iter().find(|(k, _)| k == "body") {
            Some((_, Value::Bin(b))) => b.clone(),
            Some(_) => return Err(Error::codec("http_fetch: malformed response body")),
            None => Vec::new(),
        };

        Ok(Self {
            status,
            headers,
            body,
        })
    }

    /// HTTP status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Response headers.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Response body bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Consume into the body bytes.
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }

    /// Response body as UTF-8 string (lossy).
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}
