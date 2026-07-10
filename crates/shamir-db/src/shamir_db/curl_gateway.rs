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
    check_url_allowed_resolved, HttpRequest, HttpResponse, NetGateway, ResolvedPin,
};
use std::path::Path;
use tokio::io::AsyncReadExt;

/// Network gateway that delegates to the system `curl` binary.
///
/// Holds an allowlist of host patterns. Empty allowlist = deny everything.
/// Runs the SSRF guard before building any curl config files.
pub struct CurlNetGateway {
    allowlist: Vec<String>,
}

impl CurlNetGateway {
    pub fn new(allowlist: Vec<String>) -> Self {
        Self { allowlist }
    }
}

#[async_trait]
impl NetGateway for CurlNetGateway {
    async fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, String> {
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
        let tmp_dir =
            tempfile::tempdir().map_err(|e| format!("egress: failed to create temp dir: {e}"))?;

        let config_path = tmp_dir.path().join("curl.cfg");
        let body_in_path = tmp_dir.path().join("body_in");
        let body_out_path = tmp_dir.path().join("body_out");
        let headers_out_path = tmp_dir.path().join("headers_out");

        // 3. Write request body to temp file (if non-empty).
        if !req.body.is_empty() {
            tokio::fs::write(&body_in_path, &req.body)
                .await
                .map_err(|e| format!("egress: failed to write body temp file: {e}"))?;
        }

        // 4. Build curl config file.
        //    On Windows, paths in curl config must use forward slashes.
        let mut cfg = String::new();

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
        cfg.push_str("max-time = \"30\"\n");
        cfg.push_str(&format!(
            "output = \"{}\"\n",
            path_to_forward_slash(&body_out_path)
        ));
        cfg.push_str("write-out = \"%{http_code}\"\n");
        cfg.push_str(&format!(
            "dump-header = \"{}\"\n",
            path_to_forward_slash(&headers_out_path)
        ));

        tokio::fs::write(&config_path, &cfg)
            .await
            .map_err(|e| format!("egress: failed to write curl config: {e}"))?;

        // 5. Invoke curl -K <config>.
        let output = match tokio::process::Command::new("curl")
            .arg("-K")
            .arg(&config_path)
            .output()
            .await
        {
            Ok(o) => o,
            Err(e) => {
                return if e.kind() == std::io::ErrorKind::NotFound {
                    Err("egress unavailable: curl not found".to_string())
                } else {
                    Err(format!("egress: curl spawn error: {e}"))
                }
            }
        }

        // Cleanup happens when tmp_dir drops at the end of this scope.
        ;

        if !output.status.success() && output.stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("egress: curl failed: {stderr}"));
        }

        // 6. Parse status code from stdout (write-out).
        let status_str = String::from_utf8_lossy(&output.stdout);
        let status: u16 = status_str.trim().parse().map_err(|_| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!("egress: could not parse HTTP status from curl output: '{status_str}' (stderr: {stderr})")
        })?;

        // Status 000 means curl couldn't connect / complete the request.
        if status == 0 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "egress: curl request failed (status 000): {stderr}"
            ));
        }

        // 7. Read response body from output file.
        let body = if body_out_path.exists() {
            let mut buf = Vec::new();
            let mut f = tokio::fs::File::open(&body_out_path)
                .await
                .map_err(|e| format!("egress: could not open response body file: {e}"))?;
            f.read_to_end(&mut buf)
                .await
                .map_err(|e| format!("egress: could not read response body: {e}"))?;
            buf
        } else {
            Vec::new()
        };

        // 8. Parse response headers from dump-header file.
        let headers = parse_response_headers(&headers_out_path).await;

        // Temp files cleaned up when tmp_dir drops here.
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

/// Convert a path to forward-slash notation for curl config files.
/// On Windows, curl interprets backslashes in config values as escape chars.
fn path_to_forward_slash(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Build curl config `resolve = "host:port:ip"` lines that pin the connection
/// to the exact IP(s) the SSRF guard validated (finding 2c DNS-rebind fix).
///
/// One `resolve` entry per validated IP — curl accepts the flag repeatedly for
/// the same `host:port` and tries the addresses in order. When `pinned_ips` is
/// empty (the exact-allowlist-match path, where no DNS resolution happened and
/// there is nothing to rebind) this returns an empty string, so no pin is
/// emitted and curl resolves the exact operator-allowed host itself.
pub(crate) fn build_resolve_lines(pin: &ResolvedPin) -> String {
    let mut out = String::new();
    for ip in &pin.pinned_ips {
        out.push_str(&format!(
            "resolve = \"{}:{}:{}\"\n",
            escape_curl_value(&pin.host),
            pin.port,
            ip
        ));
    }
    out
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
async fn parse_response_headers(path: &Path) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Ok(bytes) = tokio::fs::read(path).await {
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            // Skip status line and empty lines
            if line.starts_with("HTTP/") || line.is_empty() {
                continue;
            }
            if let Some(colon) = line.find(':') {
                let name = line[..colon].trim().to_string();
                let value = line[colon + 1..].trim().to_string();
                headers.push((name, value));
            }
        }
    }
    headers
}
