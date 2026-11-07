//! Curl-based network gateway for HTTP egress (slice 8c).
//!
//! Wraps the system `curl` binary via `tokio::process::Command`. All
//! secrets (tokens, headers) go into a curl config file (`-K <tmpfile>`)
//! so they never appear in `/proc` argv — argv is world-readable.
//!
//! Temp files holding secrets are cleaned up on every path (success,
//! error, panic). The allowlist + SSRF guard runs BEFORE any curl
//! invocation.

use async_trait::async_trait;
use shamir_engine::function::{
    check_url_allowed_resolved, HttpRequest, HttpResponse, NetGateway, NetGatewayError, ResolvedPin,
};
use std::num::NonZeroUsize;
use std::path::Path;
use tokio::io::AsyncReadExt;

/// Reject any guest-controlled curl-config string containing CR, LF, other
/// C0 controls or DEL — these would start new directives in the config file
/// (RV-DB-C9444758, CRLF/config-directive injection).
fn reject_control_chars(field: &str, label: &str) -> Result<(), String> {
    for &b in field.as_bytes() {
        let bad = b == 0x7F || (b < 0x20 && b != b'\t');
        if bad {
            return Err(format!(
                "egress: invalid control character (0x{b:02X}) in {label}"
            ));
        }
    }
    Ok(())
}

/// Validate every guest-controlled string that reaches the curl config file:
/// method and each header name/value. Tab is allowed in header values.
pub(crate) fn validate_curl_request(req: &HttpRequest) -> Result<(), String> {
    reject_control_chars(&req.method, "request method")?;
    for (name, value) in &req.headers {
        reject_control_chars(name, "header name")?;
        reject_control_chars(value, "header value")?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) use super::curl_supervisor::LifecycleEvent;

/// Network gateway that delegates to the system `curl` binary.
///
/// Holds an allowlist of host patterns. Empty allowlist = deny everything.
/// Runs the SSRF guard before building any curl config files.
pub struct CurlNetGateway {
    allowlist: Vec<String>,
    response_limit: NonZeroUsize,
    #[cfg(test)]
    observer: Option<tokio::sync::mpsc::UnboundedSender<LifecycleEvent>>,
}

impl CurlNetGateway {
    /// Creates a gateway with a 16 MiB maximum response body.
    pub fn new(allowlist: Vec<String>) -> Self {
        Self::with_response_limit(allowlist, NonZeroUsize::new(16 * 1024 * 1024).unwrap())
    }

    /// Creates a gateway with an explicit positive response-body limit in bytes.
    pub fn with_response_limit(allowlist: Vec<String>, response_limit: NonZeroUsize) -> Self {
        Self {
            allowlist,
            response_limit,
            #[cfg(test)]
            observer: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_observer(
        mut self,
        observer: tokio::sync::mpsc::UnboundedSender<LifecycleEvent>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }

    #[cfg(test)]
    pub(crate) fn response_limit(&self) -> usize {
        self.response_limit.get()
    }
}

#[async_trait]
impl NetGateway for CurlNetGateway {
    async fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, NetGatewayError> {
        // 0. Config-directive injection guard (RV-DB-C9444758): reject
        //    CR/LF/controls in guest-controlled method/header strings BEFORE
        //    the SSRF guard, so no network/DNS is touched on rejection.
        validate_curl_request(&req).map_err(NetGatewayError::Transport)?;

        // 1. Allowlist + DNS-resolved SSRF guard — reject before any network
        //    I/O. `check_url_allowed_resolved` resolves the host and rejects a
        //    hostname that resolves to a private/loopback IP (finding 2c),
        //    unless it is an exact allowlist entry. curl is invoked WITHOUT
        //    `--location`, so it does not follow redirects — each redirect hop
        //    would require a fresh guarded request from the guest (no
        //    transparent redirect-following bypass exists here).
        //
        //    The guard returns the exact host/port/IP(s) it validated. We pin
        //    curl's connection to those IP(s) below via `--resolve` so curl
        //    does NOT perform its own second DNS lookup at connection time —
        //    closing the DNS-rebind TOCTOU where an attacker's authoritative
        //    DNS could answer a safe IP to the guard and an internal IP to curl
        //    moments later (finding 2c DNS-rebind).
        let pin: ResolvedPin = check_url_allowed_resolved(&req.url, &self.allowlist).await?;

        // 2. Build temp directory for all temp files.
        let tmp_dir = tempfile::tempdir().map_err(|source| NetGatewayError::Io {
            operation: "failed to create temp dir",
            source,
        })?;

        let config_path = tmp_dir.path().join("curl.cfg");
        let body_in_path = tmp_dir.path().join("body_in");
        let headers_out_path = tmp_dir.path().join("headers_out");

        // 3. Write request body to temp file (if non-empty).
        if !req.body.is_empty() {
            tokio::fs::write(&body_in_path, &req.body)
                .await
                .map_err(|source| NetGatewayError::Io {
                    operation: "failed to write body temp file",
                    source,
                })?;
        }

        // 4. Build curl config file.
        //    On Windows, paths in curl config must use forward slashes.
        let mut cfg = String::from("noproxy = \"*\"\n");

        cfg.push_str(&format!("url = \"{}\"\n", escape_curl_value(&req.url)));
        cfg.push_str(&format!(
            "request = \"{}\"\n",
            escape_curl_value(&req.method)
        ));

        // DNS-rebind pin (finding 2c): pin curl's connection to the EXACT IP(s)
        // the SSRF guard validated via `--resolve host:port:ip`, so curl skips
        // its own connection-time DNS lookup. curl still uses `pin.host` for the
        // `Host` header / TLS SNI. See [`build_resolve_lines`].
        cfg.push_str(&build_resolve_lines(&pin));

        for (name, value) in &req.headers {
            cfg.push_str(&format!(
                "header = \"{}: {}\"\n",
                escape_curl_value(name),
                escape_curl_value(value)
            ));
        }

        if !req.body.is_empty() {
            cfg.push_str(&format!(
                "data-binary = \"@{}\"\n",
                path_to_forward_slash(&body_in_path)
            ));
        }

        cfg.push_str("silent\n");
        cfg.push_str("show-error\n");
        // Flush decoded body bytes promptly so the supervisor can enforce its limit during receipt.
        cfg.push_str("no-buffer\n");
        cfg.push_str("max-time = \"30\"\n");
        cfg.push_str(&format!(
            "dump-header = \"{}\"\n",
            path_to_forward_slash(&headers_out_path)
        ));

        tokio::fs::write(&config_path, &cfg)
            .await
            .map_err(|source| NetGatewayError::Io {
                operation: "failed to write curl config",
                source,
            })?;

        super::curl_supervisor::run(
            tmp_dir,
            config_path,
            headers_out_path,
            self.response_limit.get(),
            #[cfg(test)]
            self.observer.clone(),
        )
        .await
    }
}

/// Convert a path to forward-slash notation for curl config files.
/// On Windows, curl interprets backslashes in config values as escape chars.
fn path_to_forward_slash(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Build one curl resolve entry containing all guard-validated addresses.
/// An empty pin set emits no entry for the exact-allowlist path.
pub(crate) fn build_resolve_lines(pin: &ResolvedPin) -> String {
    if pin.pinned_ips.is_empty() {
        return String::new();
    }
    let addresses = pin
        .pinned_ips
        .iter()
        .map(|ip| match ip {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "resolve = \"{}:{}:{}\"\n",
        escape_curl_value(&pin.host),
        pin.port,
        addresses
    )
}

/// Escape a value for curl config file double-quoted strings.
/// Only `\` and `"` need escaping inside double quotes.
pub(crate) fn escape_curl_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

/// Parse response headers from curl's `--dump-header` output.
///
/// The file starts with the status line (HTTP/1.1 200 OK\r\n),
/// then header lines. We skip the status line and parse Key: Value pairs.
pub(in crate::shamir_db) async fn parse_response_headers(
    path: &Path,
) -> Result<(u16, Vec<(String, String)>), NetGatewayError> {
    // The supervisor bounds stdout while receiving; no response-body file is read.
    let mut headers = Vec::new();
    const HEADER_LIMIT: usize = 1024 * 1024;
    let read_error = |error| {
        log::warn!(
            "egress: failed to read response headers at {}: {error}",
            path.display()
        );
        NetGatewayError::Transport(format!("egress: failed to read response headers: {error}"))
    };
    let file = tokio::fs::File::open(path).await.map_err(read_error)?;
    let mut bytes = Vec::new();
    file.take((HEADER_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(read_error)?;
    if bytes.len() > HEADER_LIMIT {
        log::warn!(
            "egress: response headers at {} exceed {HEADER_LIMIT} bytes",
            path.display()
        );
        return Err(NetGatewayError::Transport(format!(
            "egress: response headers exceed {HEADER_LIMIT} bytes"
        )));
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut status = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("HTTP/") {
            status = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok());
        } else if let Some(colon) = line.find(':') {
            headers.push((
                line[..colon].trim().to_string(),
                line[colon + 1..].trim().to_string(),
            ));
        }
    }
    let status = status
        .filter(|s| *s != 0)
        .ok_or_else(|| NetGatewayError::Transport("egress: missing HTTP status".into()))?;
    Ok((status, headers))
}
