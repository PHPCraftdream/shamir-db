# shamir-transport-ws -- Security & crypto boundary

## Summary

The crate's security mechanics are largely sound: zero `unsafe` (grep-verified), Origin is validated inside tungstenite's handshake callback **before** the 101 response, endpoint paths are exact-match, the framing layer enforces `declared == actual` length equality as defense-in-depth, the NEW-1 pre-auth buffering cap is pinned to 16 MiB and live-tested, and the TLS-exporter boundary fails closed on the client side (shamir-client errors on `None`). No timing-sensitive comparisons exist in this crate (Origin matching guards no secret; SCRAM/HMAC and the binding_mode anti-downgrade matrix live in shamir-connect, as the crate docs correctly state). The two substantive issues are at the boundary itself: a phantom second copy of the WS parser in the dependency tree, and zero live-wiring test coverage for the browser endpoint's Origin enforcement -- the crate's primary anti-CSWSH control.

## Findings

### 1. Phantom `tungstenite = "0.29"` dependency -- two WS parsers compiled, the live one is the older
**File:** `crates/shamir-transport-ws/Cargo.toml:20` (evidence: `Cargo.lock:4233-4246`, `4455-4488`, `3769`)
**Severity:** medium

Every code path in the crate imports WS types via the `tokio_tungstenite::tungstenite::*` re-export -- i.e. tungstenite **0.24.0**, the version tokio-tungstenite 0.24.0 pins. The direct `tungstenite = "0.29"` entry is never imported anywhere under `src/` (verified by grep). Cargo.lock consequently carries **both** tungstenite 0.24.0 and 0.29.0, and `shamir-transport-ws` is the sole depender of the 0.29.0 copy.
**Failure scenario:** a CVE fix or hardening release on the tungstenite 0.29 line gives false assurance -- `cargo audit` triage and auditors see 0.29.0 present while the code that actually parses untrusted frames from unauthenticated peers runs 0.24.0 and receives nothing. A second full parser copy (plus its `rand 0.9` / `thiserror 2` closure) is also compiled into the binary for no benefit, inflating supply-chain surface on the security-boundary crate.
**Suggested fix:** delete the `tungstenite = "0.29"` line; if tungstenite types are ever needed directly, use the `tokio_tungstenite::tungstenite` re-export (as the code already does) so exactly one version exists. Treat "move to a tokio-tungstenite release on the 0.29 line" as its own deliberate upgrade task. While there, the direct `rustls` / `tokio-rustls` entries are likewise unused by this crate's code (types arrive via `shamir_transport_tcp::tls::ConnectionExporter`); dropping them keeps the manifest honest about the crypto boundary.

### 2. `accept_browser_ws` Origin enforcement has no live-wiring test coverage
**File:** `crates/shamir-transport-ws/src/server.rs:108-146`; tests: `src/tests/browser_tests.rs` (pure predicate only), `src/tests/server_tests.rs` (native path only)
**Severity:** medium

`validate_origin` is unit-tested as a pure string matcher, but the only consumer that turns it into a security control -- the handshake callback in `accept_browser_ws` (header extraction via `to_str().ok()`, policy moved into the closure, 403-before-101 ordering, browser-path check) -- is exercised by **no test anywhere in the workspace** (grep: `accept_browser_ws` appears only in `server.rs`, `lib.rs`, and the production caller `server_launcher.rs:1266`). The repo's own NEW-1 work sets the standard of live-wiring tests for accept-path security caps (`live_accept_rejects_message_over_cap`); the Origin control never got the equivalent.
**Failure scenario:** a refactor that moves validation after the upgrade response, drops the `move` capture (falling back to an empty default policy), or swaps `to_str().ok()` for a laxer extraction compiles cleanly and passes the entire suite while silently disabling the anti-CSWSH defense that `browser.rs`'s own docs call "the primary defence".
**Suggested fix:** add duplex-pipe tests (reuse the harness from `server_tests.rs`): (a) missing `Origin` -> handshake error before upgrade; (b) disallowed `Origin` -> error, no 101; (c) allowlisted `Origin` -> 101 plus a frame round-trip; (d) wrong path on the browser route -> 404 error.

### 3. Attacker-controlled `Origin` echoed into the HTTP 403 response body
**File:** `crates/shamir-transport-ws/src/server.rs:136` (source: `browser.rs:87`, `browser.rs:103`)
**Severity:** low

`OriginRejected::NotAllowed(origin.to_string())` embeds the raw header value, and the accept path interpolates it into the `ErrorResponse` body sent on the wire ("origin rejected: {rej}"). Exploitation as XSS is effectively blocked in practice (rendering the body requires a top-level navigation, which does not carry an attacker-chosen `Origin`; fetch/WS callers cannot read the cross-origin body), but it is needless reflection of untrusted input in a boundary response, and the same unsanitized string also flows into operator debug logs (`ws browser upgrade failed`, server_launcher.rs:1270).
**Suggested fix:** send a static body ("origin rejected") and keep the offending value in structured `tracing` fields only; `OriginRejected` already preserves it for library callers.

### 4. Unbounded control-frame loop in `ws_recv_into_stream` (ping-flood liveness)
**File:** `crates/shamir-transport-ws/src/framing.rs:176`, `framing.rs:183`
**Severity:** low

`Message::Ping | Message::Pong` and `Message::Frame` hit `continue` with no cap on consecutive non-BINARY messages. A peer streaming Pings keeps the loop spinning indefinitely (tungstenite auto-queues a Pong per Ping, so 1:1 outbound traffic is also forced). Production callers currently wrap reads in `auth_init_timeout`-style bounds (shamir-server), but the crate API itself offers no progress guarantee -- any future `ws_recv*` caller without an outer timeout can be wedged one task per connection, forever.
**Suggested fix:** cap consecutive non-BINARY messages (4-8) and return a dedicated `WsFrameError::ControlFrameFlood`, matching the crate's fail-closed style.

### 5. `Option`-returning exporter API + public all-zeros constant invites silent zero-substitution on the native path
**File:** `crates/shamir-transport-ws/src/tls_exporter.rs:20-25`
**Severity:** low

`extract_tls_exporter_from_stream -> Option<[u8; 32]>` plus the exported `BROWSER_CHANNEL_BINDING = [0u8; 32]` makes `unwrap_or(<zeros>)` the path of least resistance -- which is exactly what the production callers do today, including on the **native** path where binding_mode = 0x01/TlsExporter (`shamir-server/src/server/server_launcher.rs:1073`, `:1159`). Today this fails closed only by accident of the client also failing closed on `None` (`shamir-client/src/client.rs:391-392`, `:603-604`): zero-vs-real binding bytes break the SCRAM proof. But it masks a broken TLS state as a generic auth failure, and the design is one client-side `unwrap_or` away from a native session genuinely bound with the browser placeholder.
**Suggested fix:** expose one fail-closed helper, e.g. `channel_binding_for(stream, BindingMode) -> Result<[u8; 32], ChannelBindingError>`, which errors when extraction fails for `TlsExporter` and returns the placeholder only for `TlsNoExport`; gate `BROWSER_CHANNEL_BINDING` (or rename it `TLS_NO_EXPORT_PLACEHOLDER`) with a doc warning against `unwrap_or` on native paths.

### 6. Doc misattributes the 4 KiB pre-auth cap to this crate's framing layer
**File:** `crates/shamir-transport-ws/src/server.rs:27-28`, `:37-38`
**Severity:** nit

The comment claims the "4 KiB pre-auth logical check (HIGH-1)" is "enforced in `crate::framing::ws_recv_into`". In this crate, `ws_recv_into` enforces whatever `max_frame_size` its caller passes; the 4 KiB constant (`MAX_PRE_AUTH_FRAME`) lives in shamir-connect and is applied by shamir-server's framer/handshake. A reader of this crate alone could conclude the pre-auth cap is intrinsic and ship a caller that passes `MAX_WS_FRAME_SIZE` pre-auth -- reinstating the 16 MiB unauthenticated buffering the doc says was fixed.
**Suggested fix:** reword to credit the caller ("shamir-server passes `MAX_PRE_AUTH_FRAME` = 4 KiB; see `shamir_connect::common::types::limits`").

### 7. `ws_send_sink` truncates the length prefix for payloads >= 4 GiB
**File:** `crates/shamir-transport-ws/src/framing.rs:118`
**Severity:** nit

`payload.len() as u32` wraps silently; the peer then rejects with `LengthMismatch` (equality is enforced on both sides, so there is no desync or memory-safety issue -- only a confusing protocol error). Practically unreachable given the 16 MiB conventions, but `u32::try_from(payload.len()).map_err(|_| WsFrameError::TooLarge { .. })` costs nothing.

### 8. `BrowserOriginPolicy::allow` accepts malformed patterns that silently never match
**File:** `crates/shamir-transport-ws/src/browser.rs:37-41`, `:56-76`
**Severity:** nit

Wildcard detection is a bare `find("//*.")`; entries like `https://*.*.example.com`, `*.example.com` (no scheme), or a typo'd scheme fall through to exact-match and match nothing. The operator gets a full-reject allowlist -- fail-closed, but a configuration trap surfaced only via client-side 403s.
**Suggested fix:** validate at construction (must contain `://`; at most one `*`, only in the `//\*.` slot) via a `try_allow -> Result`, or `debug_assert!` the shape in `allow`.

## Checked and clean (this theme)

- **unsafe:** none in the crate (grep-verified across `src/` and `tests/`).
- **Timing side-channels:** none applicable here -- Origin matching guards no secret; no secret comparison, HMAC, or SCRAM logic lives in this crate.
- **Untrusted-input handling:** framing validates `declared == actual` before use and rejects non-BINARY payloads without reflecting their content (`TEXT len=N` only); `TooLarge` fires after tungstenite's own 16 MiB cap (documented NEW-1 residual).
- **Injection:** endpoint paths exact-match against constants; Origin comparisons are non-wildcard exact or single-component wildcard with apex/deep-subdomain/port mismatches all rejecting (fail-closed, test-covered in `browser_tests.rs`); non-ASCII Origin headers classify as `Missing` (fail-closed).
- **Ordering:** browser Origin check runs inside the handshake callback, before 101 Switching Protocols -- correct.
- **TLS 1.3-only:** enforced at the rustls config layer in `shamir-transport-tcp` (`make_server_config_from_pem` / `make_client_config_no_ca`, `builder_with_protocol_versions(&[TLS13])`), which this crate correctly delegates to rather than duplicating.
