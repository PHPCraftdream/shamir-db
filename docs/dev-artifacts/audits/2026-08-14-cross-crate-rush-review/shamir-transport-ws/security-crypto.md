<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-transport-ws — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Origin rejection remains before upgrade, framing validation remains fail-closed, and the TLS configuration remains TLS-1.3-only. Several security consequences require narrowing: positive browser tests exist, queue growth is unverified, and zero exporter fallback does not change binding_mode.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 7 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Source inspection resolves Pong handling: single pending Pong and read-time flushing coexist with an uncapped encoded write buffer under sustained WouldBlock.

<a id="review-1"></a>

### Claim 1 — Phantom `tungstenite = "0.29"` dependency -- two WS parsers compiled, the live one is the older

Status: `confirmed-open`. Current risk: `low`.

Unused 0.29 remains resolved beside effective 0.24. Direct rustls/tokio-rustls dependencies are also unused by this crate's source. A specific vulnerability or second parser retained in the final executable is not proven.

Evidence: [crates/shamir-transport-ws/Cargo.toml:19](../../../../../crates/shamir-transport-ws/Cargo.toml#L19); [crates/shamir-transport-ws/Cargo.toml:23](../../../../../crates/shamir-transport-ws/Cargo.toml#L23); [Cargo.lock:3781](../../../../../Cargo.lock#L3781); [Cargo.lock:4256](../../../../../Cargo.lock#L4256).

<a id="review-2"></a>

### Claim 2 — `accept_browser_ws` Origin enforcement has no live-wiring test coverage

Status: `refuted`. Current risk: —.

Registered TS live tests connect to the browser endpoint with an allowlisted Origin and perform authentication/requests. They do not cover removing the Origin check or its negative/status branches.

Evidence: [crates/shamir-client-ts/src/__tests__/connect.test.ts:203](../../../../../crates/shamir-client-ts/src/__tests__/connect.test.ts#L203); [crates/shamir-client-ts/src/__tests__/e2e.test.ts:44](../../../../../crates/shamir-client-ts/src/__tests__/e2e.test.ts#L44); [crates/shamir-client-ts/src/core/client.ts:190](../../../../../crates/shamir-client-ts/src/core/client.ts#L190); [.github/workflows/ts-e2e-nightly.yml:85](../../../../../.github/workflows/ts-e2e-nightly.yml#L85).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Attacker-controlled `Origin` echoed into the HTTP 403 response body

Status: `confirmed-open`. Current risk: `low`.

NotAllowed stores the header string and the callback interpolates it into the rejection body. Arbitrary-header clients can reach reflection; browser XSS or effective log injection is not established.

Evidence: [crates/shamir-transport-ws/src/browser.rs:103](../../../../../crates/shamir-transport-ws/src/browser.rs#L103); [crates/shamir-transport-ws/src/server.rs:136](../../../../../crates/shamir-transport-ws/src/server.rs#L136); [crates/shamir-server/src/server/server_launcher.rs:1502](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1502).

<a id="review-4"></a>

### Claim 4 — Unbounded control-frame loop in `ws_recv_into_stream` (ping-flood liveness)

Status: `confirmed-open`. Current risk: `medium`.

The adapter has no control-frame budget. Parent pinned-source inspection corrects the mechanism: tungstenite 0.24 tries flushing during read and has a single replaceable additional_send Pong, so 'only the writer flushes' is false. However FrameCodec appends a Pong into its Vec before attempting the write; read ignores WouldBlock, and the default max_write_buffer_size is usize::MAX. A peer that keeps supplying Pings while refusing replies can grow that outgoing byte buffer. The SCRAM client_proof read has no surrounding timeout after the bounded AuthInit read. These are source-visible conditional backpressure/liveness risks; no CPU/RSS measurement or reproduction was performed.

Evidence: [crates/shamir-transport-ws/src/framing.rs:176](../../../../../crates/shamir-transport-ws/src/framing.rs#L176); [crates/shamir-transport-ws/src/framing.rs:183](../../../../../crates/shamir-transport-ws/src/framing.rs#L183); [crates/shamir-server/src/connection/handshake.rs:291](../../../../../crates/shamir-server/src/connection/handshake.rs#L291); [crates/shamir-server/src/connection/request_loop.rs:297](../../../../../crates/shamir-server/src/connection/request_loop.rs#L297).

Pinned dependency evidence: [tungstenite 0.24.0, src/protocol/mod.rs:387](https://docs.rs/crate/tungstenite/0.24.0/source/src/protocol/mod.rs); [tungstenite 0.24.0, src/protocol/mod.rs:729](https://docs.rs/crate/tungstenite/0.24.0/source/src/protocol/mod.rs); [tungstenite 0.24.0, src/protocol/frame/mod.rs:1](https://docs.rs/crate/tungstenite/0.24.0/source/src/protocol/frame/mod.rs).

<a id="review-5"></a>

### Claim 5 — `Option`-returning exporter API + public all-zeros constant invites silent zero-substitution on the native path

Status: `confirmed-open`. Current risk: `low`.

Extraction discards its error and native WSS still substitutes zeros. This is a fail-closed/diagnostic API defect, not a demonstrated remote downgrade: listener binding_mode stays TlsExporter and participates in the proof transcript.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:20](../../../../../crates/shamir-transport-ws/src/tls_exporter.rs#L20); [crates/shamir-transport-tcp/src/tls.rs:81](../../../../../crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:1391](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-connect/src/server/handshake.rs:135](../../../../../crates/shamir-connect/src/server/handshake.rs#L135); [crates/shamir-connect/src/server/handshake.rs:265](../../../../../crates/shamir-connect/src/server/handshake.rs#L265).

<a id="review-6"></a>

### Claim 6 — Doc misattributes the 4 KiB pre-auth cap to this crate's framing layer

Status: `confirmed-open`. Current risk: `nit`.

The transport uses its caller's max_frame_size; the 4 KiB argument is supplied by server handshake code.

Evidence: [crates/shamir-transport-ws/src/server.rs:27](../../../../../crates/shamir-transport-ws/src/server.rs#L27); [crates/shamir-transport-ws/src/framing.rs:163](../../../../../crates/shamir-transport-ws/src/framing.rs#L163); [crates/shamir-server/src/connection/handshake.rs:717](../../../../../crates/shamir-server/src/connection/handshake.rs#L717).

Grouping/duplicate: `performance-hotpath.md#4`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — `ws_send_sink` truncates the length prefix for payloads >= 4 GiB

Status: `confirmed-open`. Current risk: `nit`.

Unchecked usize-to-u32 conversion remains. The extreme requires a huge caller-owned buffer; the ordinary missing outbound size guard is the actionable defect.

Evidence: [crates/shamir-transport-ws/src/framing.rs:118](../../../../../crates/shamir-transport-ws/src/framing.rs#L118); [crates/shamir-transport-ws/src/framing.rs:157](../../../../../crates/shamir-transport-ws/src/framing.rs#L157).

Grouping/duplicate: `api-wire-protocol.md#3`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — `BrowserOriginPolicy::allow` accepts malformed patterns that silently never match

Status: `confirmed-open`. Current risk: `nit`.

Construction still validates no scheme or wildcard syntax, and production rejects only an empty allowlist. Malformed entries can silently exclude intended valid browser Origins.

Evidence: [crates/shamir-transport-ws/src/browser.rs:37](../../../../../crates/shamir-transport-ws/src/browser.rs#L37); [crates/shamir-transport-ws/src/browser.rs:59](../../../../../crates/shamir-transport-ws/src/browser.rs#L59); [crates/shamir-server/src/server/server_launcher.rs:1014](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1014).

Grouping/duplicate: `correctness-tdd.md#5`. This row is not another independent defect.

## Corrections and qualified non-findings

- The effective parser is 0.24.0, but age/version skew is not proof of a vulnerability. Lockfile membership does not establish executable bloat or an additional reachable parser.
- Origin matching is not secret comparison. No unsafe or cryptographic implementation exists in this crate; actual TLS/exporter work delegates to the TCP transport.
- Source confirms path checks, length equality, TEXT-content nonreflection, Origin validation before acceptance, and TLS-1.3-only configuration. The named endpoints are string literals, not shared constants.
- The 16 MiB setting is a WS message/frame ceiling, not a 4 KiB pre-auth allocation ceiling. Logical framing runs after message assembly; the inner four-byte prefix also occupies that message ceiling.
- Zero fallback does not switch mode 0x01 to 0x02. The listener policy rejects a different binding_mode and includes mode/exporter in authentication. The cited Rust-client fail-closed paths are TCP paths, not universal WSS-client proof.
- The broad claim that production reads are timeout-protected is false for the directly awaited client_proof read.
- Malformed Origin examples require a non-browser header sender. No exploit through normal browser-serialized Origins or ambient-cookie authentication was established.
- Multi-star patterns containing //*. enter wildcard logic rather than necessarily falling back to exact comparison; malformed exact entries can match identical malformed raw headers.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
