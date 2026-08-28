# shamir-transport-ws -- API & wire-protocol design

## Summary

The crate's framing, origin policy, and listener-profile APIs are clean, well-documented, and this crate constructs no queries or raw JSON anywhere, so the builder-only rule is satisfied by construction. The wire-protocol problems are at the handshake and cross-transport seams: the spec-mandated `Sec-WebSocket-Protocol: shamir-v1` subprotocol negotiation (TRANSPORT_WS §2.1) is entirely unimplemented, and the versioned endpoint paths are hardcoded string literals with no shared constants while the server config treats `path` as an operator knob. Secondary issues: send-side framing lacks the size cap its TCP sibling enforces (with `u32` truncation at the extreme), an unused version-mismatched `tungstenite` dependency pins the public API to a different version than the manifest advertises, and the zero-length-frame semantics silently diverge from the TCP framing this crate claims to share.

## Findings

### 1. Spec-mandated WebSocket subprotocol negotiation is unimplemented
- **File:line:** `crates/shamir-transport-ws/src/server.rs:85-99` and `:120-143` (both accept callbacks); spec: `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md:18` (§2.1)
- **Severity:** high
- **Issue:** Spec TRANSPORT_WS §2.1 is normative for this crate (every module cites the spec by section): the client sends `Sec-WebSocket-Protocol: shamir-v1`, the server "confirm[s] same. Mismatch → 400." Neither `accept_native_ws` nor `accept_browser_ws` reads or echoes the `Sec-WebSocket-Protocol` header — the handshake callbacks check only the URI path and (browser) `Origin`. There is no transport-layer protocol-version gate at all.
- **Failure scenario:** A spec-conformant browser client that requests `shamir-v1` gets a 101 response with no echoed `Sec-WebSocket-Protocol`; per RFC 6455 the browser then *fails the connection itself* ("Incorrect 'Sec-WebSocket-Protocol' header") — a conformant client cannot connect at all. Conversely, a client offering a different/future subprotocol (`shamir-v2`) is silently accepted, so the spec's mismatch→400 downgrade defense never fires. First-party clients dodge both today only because they also ignore the spec (`shamir-client-ts/src/platform/browser.ts:118` calls `new WebSocket(url)` with no subprotocol) — implementation and spec have drifted in opposite directions.
- **Suggested fix:** In both handshake callbacks, read `Sec-WebSocket-Protocol`: reject with 400 unless it contains exactly `shamir-v1`, and echo `shamir-v1` back via the callback's `Response` headers (`resp.headers().append(...)`). Update the TS client to request it, or amend the spec to make the subprotocol optional and record why.

### 2. Endpoint paths hardcoded as string literals; no shared constants; incompatible with the server's configurable `path`
- **File:line:** `crates/shamir-transport-ws/src/server.rs:90` (`!= "/shamir/v1"`), `:124` (`!= "/shamir/v1/browser"`)
- **Severity:** high
- **Issue:** `/shamir/v1` and `/shamir/v1/browser` are the protocol's version markers on the wire, but they exist only as `&'static str` literals inside the two accept functions. There is no `pub const` in this crate, and the accept API offers no path parameter. Meanwhile the sole production consumer treats the path as operator configuration: `shamir-server/src/config.rs:767-777` accepts *any* `path` starting with `/`, then `server_launcher.rs:1162/:1266` calls these hardcoded acceptors.
- **Failure scenario:** Operator sets `path: /db-ws` for a ws listener → config boots cleanly → every WS upgrade is answered 404 by the hardcoded check → total, silent connectivity loss on that listener with no boot-time error. Independently, the literals are now duplicated in ≥5 uncoordinated places (shamir-server config/tests, `shamir-client-ts` client.ts, deploy `*.ktav` files, docs), so the version string can drift between client and server without any compile-time check.
- **Suggested fix:** Export `pub const NATIVE_WS_PATH: &str = "/shamir/v1";` and `pub const BROWSER_WS_PATH: &str = "/shamir/v1/browser";` from this crate; either (a) make the accept fns take the expected path (or validate server config against the constants at boot, refusing unknown paths), or (b) document that the paths are protocol-fixed and have the server config validator reject any other value up front instead of accepting it.

### 3. Send path has no frame-size cap and truncates the length prefix at `u32::MAX` — diverges from the TCP sibling
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:118` (`let len = payload.len() as u32;`)
- **Severity:** medium
- **Issue:** `ws_send` / `ws_send_sink` accept any payload length: no `MAX_WS_FRAME_SIZE` check, and `payload.len() as u32` silently wraps for payloads > 4 GiB. The TCP transport this crate mirrors does enforce it on send: `shamir-transport-tcp/src/framing.rs:153-156` and `:192-195` return `FrameError::TooLarge` for `payload.len() > MAX_FRAME_SIZE_DEFAULT`. The asymmetry means the WS send API's contract differs from the wire format it claims to share (framing.rs:1-9).
- **Failure scenario:** A caller serializing a large SELECT result (>16 MiB) gets `Ok(())` from `ws_send_sink`; the frame goes out, and the failure surfaces later and remotely — the receiving peer's framing layer returns `TooLarge` mid-connection (or buffers it if the peer's `WebSocketConfig` is loose). For a >4 GiB payload the prefix wraps, the declared length is corrupt, and the receiver reports a baffling `LengthMismatch` on a frame the sender believed valid.
- **Suggested fix:** Mirror the TCP writer: check `payload.len() > MAX_WS_FRAME_SIZE` (or a caller-supplied cap symmetric with `ws_recv`'s `max_frame_size` param) and return `WsFrameError::TooLarge { actual, max }` before building the buffer. This also removes the silent `as u32` truncation path.

### 4. Unused, version-mismatched direct dependency `tungstenite = "0.29"` while the public API is pinned to tungstenite 0.24
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:19-20`; code uses only `tokio_tungstenite::tungstenite::*` (framing.rs:22-23, server.rs:17-19)
- **Severity:** medium
- **Issue:** The crate declares both `tokio-tungstenite = "0.24"` and a direct `tungstenite = "0.29"`. No source file imports `tungstenite` directly (grep: zero matches outside the `tokio_tungstenite::` re-export path), yet `Cargo.lock` carries two copies — tungstenite 0.24.0 (the one actually compiled into this crate's public types) and 0.29.0 (dead weight pulled only by the unused manifest entry). The public API leaks 0.24 types: `WsFrameError::Io(#[from] tokio_tungstenite::tungstenite::Error)` (framing.rs:33) and all `Message` handling — written against 0.24 semantics (`Message::Binary(Vec<u8>)`, vs `Bytes` in newer tungstenite).
- **Failure scenario:** A maintainer responds to a tungstenite security advisory by bumping `tungstenite = "0.29"` — the manifest update succeeds, the lockfile shows a fresh version, and the actually-linked vulnerable 0.24 is untouched. Or a future tokio-tungstenite upgrade changes `Message::Binary`'s payload type and every `Message::Binary(buf)` / `bytes[4..]` site breaks all at once with no manifest hint of the coupling.
- **Suggested fix:** Delete the direct `tungstenite` dependency (nothing uses it), or align it to `"0.24"` with a comment naming it as the version re-exported by tokio-tungstenite 0.24.

### 5. Zero-length frame means "graceful close" on TCP but is a legal empty frame on WS — undocumented divergence in a claimed-identical wire format
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:1-9` (doc claim), `:146-171` (recv path); contrast `shamir-transport-tcp/src/framing.rs:8,31` (`length == 0` → `FrameError::PeerClose`)
- **Severity:** medium
- **Issue:** framing.rs documents "Same wire format as `shamir-transport-tcp::framing`" — true for the byte layout (`[u32_be length][payload]`, length excludes prefix, 16 MiB cap), but the zero-length semantic differs: TCP defines declared length 0 as a graceful-close indicator and surfaces `FrameError::PeerClose`; over WS a 4-byte `[0,0,0,0]` message passes the mismatch and cap checks and returns `Ok(())` with an empty buffer (close is signaled only by the WS Close frame, framing.rs:173). Nothing in the module doc notes the divergence.
- **Failure scenario:** Cross-transport code reuse — a client port that sends the TCP-style zero-frame "close" while on WSS gets its message decoded as an empty payload, which then fails msgpack deserialization downstream as an opaque protocol error instead of a clean close; or a server handler written against TCP semantics treats the empty frame as EOF and skips cleanup that the WS path requires.
- **Suggested fix:** Either document the divergence explicitly in the framing.rs header ("length 0 is NOT a close indicator here; close is the WS Close frame"), or reject declared==0 frames as a protocol error so the two transports stay behaviorally aligned.

### 6. `accept_browser_ws` — the Origin-enforcing handshake path — has zero integration tests; the framing length-mismatch invariant is also untested
- **File:line:** `crates/shamir-transport-ws/src/tests/` (browser_tests.rs covers only the pure `validate_origin` function; server_tests.rs covers only native accept); `src/framing.rs:148-162` (LengthMismatch), `:177-182` (NonBinaryMessage) — no test exercises either branch
- **Severity:** medium
- **Issue:** The security-critical wiring in `accept_browser_ws` — 404 on wrong path, 403 with `OriginRejected` on disallowed/missing Origin, all *during* the handshake before 101 — is never tested end-to-end against a real upgrade request (unlike `accept_native_ws`, which has live-wiring tests in server_tests.rs:51-118). Likewise, the crate's headline defense-in-depth invariant (inner prefix must equal message body − 4) and the TEXT-message rejection have no test constructing the offending wire input.
- **Failure scenario:** A refactor of the callback (e.g. the subprotocol fix in finding #1, which edits exactly this closure) silently breaks Origin enforcement or the 403 status mapping, and the suite stays green because only the pure policy function is covered.
- **Suggested fix:** Add `#[tokio::test]`s mirroring `live_accept_*`: a browser handshake with a disallowed Origin → 403 handshake error; missing Origin → 403; wrong path → 404; plus framing tests hand-crafting `Message::Binary` with a wrong inner prefix and a `Message::Text` to hit `LengthMismatch` / `NonBinaryMessage`.

### 7. Exporter-extraction ordering: this crate's doc contradicts its only production caller
- **File:line:** `crates/shamir-transport-ws/src/server.rs:5-7` ("extracts the exporter ... AFTER the WS handshake completes") and `:76-77` ("Caller then extracts the TLS exporter"); contrast `shamir-server/src/server/server_launcher.rs:1156-1159` ("CRITICAL: extract exporter BEFORE the WS upgrade consumes `tls`. After upgrade the TLS state is owned by the WebSocketStream and not directly accessible.")
- **Severity:** low
- **Issue:** The public contract in server.rs tells callers to extract the TLS exporter *after* `accept_native_ws` returns; the production caller insists it must happen *before* the WS upgrade and calls the after-path impossible. Both cannot be right (technically `WebSocketStream::get_ref()` may make the after-path workable, but the two crates' documentation actively disagree about the safe order for a security-critical step).
- **Failure scenario:** A new integrator (Go/Python client work, second server embedding) follows this crate's doc, extracts after the upgrade, and either fails to get the exporter or — worse, if it silently returns `None` — falls into a zeros-placeholder channel binding without realizing the ordering is contested.
- **Suggested fix:** Settle the ordering once (server_launcher's "before the upgrade" is the conservative choice and is what production does), and rewrite server.rs's doc for `accept_native_ws` to state it, ideally with the `extract_tls_exporter_from_stream` call shown in the doc example.

### 8. `BROWSER_CHANNEL_BINDING` constant exported but consumers re-hardcode `[0u8; 32]`
- **File:line:** `crates/shamir-transport-ws/src/tls_exporter.rs:25`; contrast `shamir-server/src/server/server_launcher.rs:1159` (`unwrap_or([0u8; 32])`) and `:1263` (`let exporter = [0u8; 32];`)
- **Severity:** low
- **Issue:** The crate defines the named placeholder for exactly this wire value, but the sole consumer writes the literal twice — including using it as a *fallback on the native endpoint* (line 1159) where the spec-conformant value is the real exporter. Besides duplication, the native-endpoint fallback silently substitutes the weaker browser binding instead of failing loudly (worth a look from the security reviewer too).
- **Failure scenario:** If the placeholder or its semantics ever change (or a debug/assert is added in one place), the two literal sites drift from the constant and from each other, corrupting the binding_mode↔channel-binding mapping the spec's anti-downgrade matrix (§6.4) depends on.
- **Suggested fix:** Use `BROWSER_CHANNEL_BINDING` at both server_launcher sites (it is already exported at `shamir_transport_ws::tls_exporter::`), and make the native path return an error (or at least a loud warn + explicit downgrade marker) on exporter extraction failure rather than quietly reusing the browser placeholder.

### 9. `accept_browser_ws` clones the origin policy on every connection
- **File:line:** `crates/shamir-transport-ws/src/server.rs:118` (`let policy = policy.clone();`)
- **Severity:** low
- **Issue:** The handshake callback is `move` solely to own a per-call clone of `BrowserOriginPolicy` (a `Vec<String>` heap allocation) on the browser accept path. The callback does not need ownership: `accept_hdr_async_with_config` has no `'static` bound, so the closure can capture `&'a BrowserOriginPolicy` with an explicit lifetime on the async fn — the future borrows the caller's policy for the duration of the handshake, which the caller (`server_launcher`, which itself clones per connection at :1240 to move into the spawned task) already satisfies.
- **Failure scenario:** No correctness issue; it is a per-connection allocation on the accept hot path that contradicts the O(x→0)/allocation-avoidance pillar, and it invites the same clone to propagate outward as "precedent."
- **Suggested fix:** Change the signature to `pub async fn accept_browser_ws<'a, S>(stream: S, policy: &'a BrowserOriginPolicy)` and let the closure borrow; the per-connection clone in server_launcher then becomes the only copy, or share an `Arc<BrowserOriginPolicy>` across the accept loop.

### 10. Origin matching is case-sensitive; wildcard detection is a whole-pattern substring search
- **File:line:** `crates/shamir-transport-ws/src/browser.rs:56-76`
- **Severity:** nit
- **Issue:** `origin_matches` compares scheme/host byte-for-byte, but per RFC 6454 the scheme and host of an origin are case-insensitive; a client sending `https://App.Example.com` (spec-noncompliant, but seen in the wild) is rejected despite matching the operator's intent. Additionally `pattern.find("//*.")` scans the entire pattern, so wildcard logic triggers on the substring appearing anywhere, and there is no normalization (default port, trailing dot) — exact-match policies are strict literals.
- **Failure scenario:** An allowlisted user cannot connect because their environment emitted a mixed-case Origin; operators "fix" it by adding both casings to the allowlist, accumulating near-duplicate entries.
- **Suggested fix:** ASCII-lowercase both pattern and origin before comparison (origins never carry case-meaningful userinfo/path), and anchor the wildcard check to immediately after `scheme://`.

### 11. `is_loopback` re-implements `IpAddr::is_loopback`
- **File:line:** `crates/shamir-transport-ws/src/listener.rs:47-52`
- **Severity:** nit
- **Issue:** The private helper matches on V4/V6 to call each's `is_loopback`, which is exactly what the stable `std::net::IpAddr::is_loopback` already does. Redundant indirection in a public-surface module.
- **Suggested fix:** Delete the helper and call `addr.ip().is_loopback()`.

### 12. Unused dependencies: direct `tungstenite` (see finding #4) and unused dev-deps `hex`, `serde`, `serde_bytes`, `rmp-serde`
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:32-36`
- **Severity:** nit
- **Issue:** No file in the crate (src or tests) references `hex`, `serde`, `serde_bytes`, or `rmp_serde` — grep confirms zero uses. They are likely leftovers from a test that encoded msgpack payloads by hand; note this also means the crate's tests never exercise a real msgpack payload despite the docs calling this "length-prefix msgpack framing."
- **Suggested fix:** Drop the four dev-dependencies (and the direct `tungstenite` per finding #4). If payload-level round-trips are wanted, a single rmp-serde test through `ws_send`/`ws_recv` would justify keeping it and simultaneously cover the real payload shape.
