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
//! { "method": Str, "url": Str, "headers": Map<Str,Str>, "body": Bin }
//! ```
//!
//! The host returns an envelope `Value::List([Bool, payload])`:
//! - `[true, { "status": Int, "headers": Map, "body": Bin }]` on success.
//! - `[false, "error message"]` on runtime error.

use crate::error::{Error, Result};
use crate::Value;

/// Decode the host envelope `[ok: Bool, payload]` into a Result.
pub(crate) fn decode_fetch_envelope(raw: &Value) -> Result<HttpResponse> {
    let items = match raw {
        Value::List(items) => items,
        _ => return Err(Error::user("http_fetch: host returned unexpected value")),
    };
    if items.len() != 2 {
        return Err(Error::user("http_fetch: host envelope has wrong shape"));
    }
    let ok = match &items[0] {
        Value::Bool(b) => *b,
        _ => return Err(Error::user("http_fetch: envelope ok-flag is not a bool")),
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

    /// Add a header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
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

    /// Convert to the wire `Value::Map` shape.
    pub fn to_value(&self) -> Value {
        let header_entries: Vec<(String, Value)> = self
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), Value::Str(v.clone())))
            .collect();

        Value::Map(vec![
            ("method".to_string(), Value::Str(self.method.clone())),
            ("url".to_string(), Value::Str(self.url.clone())),
            ("headers".to_string(), Value::Map(header_entries)),
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
            _ => return Err(Error::user("http_fetch: response is not a Map")),
        };

        let status = map
            .iter()
            .find(|(k, _)| k == "status")
            .and_then(|(_, v)| match v {
                Value::Int(n) => Some(*n as u16),
                _ => None,
            })
            .ok_or_else(|| Error::user("http_fetch: missing status field"))?;

        let headers = match map.iter().find(|(k, _)| k == "headers") {
            Some((_, Value::Map(entries))) => entries
                .iter()
                .filter_map(|(k, v)| match v {
                    Value::Str(s) => Some((k.clone(), s.clone())),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };

        let body = match map.iter().find(|(k, _)| k == "body") {
            Some((_, Value::Bin(b))) => b.clone(),
            _ => Vec::new(),
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
