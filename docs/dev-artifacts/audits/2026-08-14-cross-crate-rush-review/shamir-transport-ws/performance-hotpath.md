# shamir-transport-ws — Performance & O(x→0)

Scope: `crates/shamir-transport-ws/` (lib.rs, framing.rs, browser.rs, listener.rs, server.rs, tls_exporter.rs, Cargo.toml), judged against CLAUDE.md pillar 3 (O(x→0): no hidden O(N)/O(N²), no allocations in loops, no unbounded growth/buffering) and pillar 1 (no hot-path locks). Consumer context checked: `shamir-server/src/framer.rs` + `connection/request_loop.rs`, which drive `ws_send_sink` / `ws_recv_into_stream` once per request/response frame on the WSS path.

## Summary

The receive path is genuinely O(x→0)-clean: `ws_recv_into_stream` recycles the caller's scratch buffer (`clear` + `extend_from_slice`), capacity reuse is proven by `tests/framing_round_trip.rs::round_trip_into_buffer_reuses_capacity`, and validation is all constant-time checks — no scans, no locks, no `scc::len()`. The send path is the weak side: every frame pays a fresh heap allocation plus a full-payload memcpy in `ws_send_sink`, which silently defeats the `write_frame_prereserved` zero-copy optimization the request loop performs for TCP. Remaining findings are buffering-boundary issues (uncapped control-frame/pong queue, a stale doc claim about pre-auth enforcement) and a dead duplicate `tungstenite` dependency that is pure build-time weight.

## Findings

### 1. WSS send hot path: fresh heap alloc + full-payload copy per frame; TCP's prereserved zero-copy path is silently defeated
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:114-124` (`ws_send_sink`); consumer context `shamir-server/src/framer.rs:110-127, 348-365, 405-415`, `shamir-server/src/connection/request_loop.rs:198-213`
- **Severity:** medium
- **Issue:** `ws_send_sink` takes `&[u8]` and, on every call, allocates `Vec::with_capacity(4 + payload.len())` and memcpys the entire payload into it (framing.rs:119-121) before handing it to tungstenite (which copies it again into its internal write buffer). The server's request loop builds every response as an already length-prefixed buffer precisely to avoid a memcpy — `write_frame_prereserved` — and TCP overrides it with `tcp_write_frame_prereserved` (zero copy). The WS `FrameWriter` does not override it, so the trait's default strips the 4-byte prefix and routes through `ws_send_sink`: the caller's prereserved copy is wasted and a second full-payload copy into a brand-new heap `Vec` is paid on **every WSS response frame**. The `Framer::write_frame_into` `scratch` parameter, designed for exactly this zero-alloc reuse, is explicitly ignored by the WS impl (`_scratch`, framer.rs:355/408 — "WS already builds its own send buffer (one allocation per message)"), i.e. one malloc+free per frame on the hottest WSS loop, plus a redundant O(payload) memcpy; at the 16 MiB frame ceiling that is two extra 16 MiB traversals per frame.
- **Failure scenario:** throughput/latency tax on all WSS traffic (browser endpoint included) proportional to payload size; allocator churn under high RPS. Not asymptotically worse than the transport itself, so medium rather than high.
- **Suggested fix:** add an ownership-taking variant in this crate, e.g. `ws_send_sink_vec(sink, Vec<u8>)` (tungstenite's `Message::Binary` consumes the `Vec`, so a caller that yields its prereserved buffer moves it with **zero** copy), and/or a scratch-reusing variant matching the `Framer` scratch contract; then override `write_frame_prereserved` in shamir-server's `WsFrameWriter` to use it. Document the copy semantics on `ws_send_sink`.

### 2. Receive loop consumes unbounded consecutive control frames; auto-pong write queue is uncapped
- **File:line:** `crates/shamir-transport-ws/src/framing.rs:140-186` (Ping/Pong/Frame `continue` at :176/:183), `crates/shamir-transport-ws/src/server.rs:42-51` (`server_ws_config` deliberately leaves `max_send_queue` at default)
- **Severity:** low
- **Issue:** `ws_recv_into_stream` loops over PING/PONG/Frame messages with no per-connection budget. Tungstenite auto-queues a Pong for every PING read; in the split-half layout shamir-server actually deploys (`WsFrameReader`/`WsFrameWriter` over `StreamExt::split`), those pongs sit in the shared write queue that only the writer task drains. A hostile peer that streams PINGs without reading drives the connection's outgoing buffer to grow without a crate-level cap (`max_send_queue` is intentionally left at tungstenite's default per the deprecation comment at server.rs:43-45). Same theme as NEW-1's pre-auth buffering concern, but on the outbound side, and unaffected by the 16 MiB message caps set in `server_ws_config`.
- **Failure scenario:** unauth'd (or auth'd) peer pins server memory per connection via ping-flood + no-read (slow-reader); multipled by many peers this is a memory-pressure DoS vector the NEW-1 hardening does not cover.
- **Suggested fix:** cheap in-crate hardening: count consecutive control frames in `ws_recv_into_stream` and return a `WsFrameError` past a small budget (e.g. 64), and/or document/enforce a write-buffer cap for the pinned tokio-tungstenite 0.24 (verify the 0.24 default; 0.26+ replaced `max_send_queue` with bounded `write_buffer_size` options — a version bump also fixes it structurally). Pair with an idle/dead-peer timeout at the session layer.

### 3. Dead direct dependency `tungstenite = "0.29"` compiles a second, unused copy of tungstenite
- **File:line:** `crates/shamir-transport-ws/Cargo.toml:20`; `Cargo.lock:4455-4488` (tungstenite 0.24.0 **and** 0.29.0 both in graph), `Cargo.lock:3769` (shamir-transport-ws depends on `tungstenite 0.29.0`)
- **Severity:** low (build time / binary bloat, not runtime hot path)
- **Issue:** `tokio-tungstenite 0.24` bundles tungstenite 0.24 (semver-incompatible, so no unification); the direct `tungstenite = "0.29"` entry is never imported — every use in the crate goes through `tokio_tungstenite::tungstenite::*` (framing.rs:22-23, server.rs:17-19, tests). Net effect: a full second copy of tungstenite (plus `rand 0.9`) is compiled into the workspace for nothing, and it is a version-skew trap (a future `use tungstenite::...` would produce types incompatible with `tokio_tungstenite::WebSocketStream`).
- **Failure scenario:** none at runtime; wasted CI/dev build time and a latent type-mismatch hazard.
- **Suggested fix:** delete the `tungstenite = "0.29"` line (or pin it to `=0.24` only if a direct import is genuinely needed).

### 4. Doc drift: `server_ws_config` claims a "4 KiB pre-auth logical check … enforced in `crate::framing::ws_recv_into`" — no such enforcement exists in this crate
- **File:line:** `crates/shamir-transport-ws/src/server.rs:26-28` (and mirrored in `src/tests/server_tests.rs:11-13`)
- **Severity:** low (documentation accuracy on a buffering-boundary claim)
- **Issue:** framing.rs has no pre-auth constant; the ceiling is entirely the caller-supplied `max_frame_size` parameter, and the actual 4 KiB pre-auth enforcement lives in shamir-server (`connection/handshake.rs` passing `MAX_PRE_AUTH_FRAME`). The doc's cross-reference implies the transport crate self-enforces the 4 KiB pre-auth budget; it does not — tungstenite will have buffered up to the full 16 MiB before any logical check runs (the doc's own "Residual" paragraph at server.rs:33-41 says exactly this, contradicting the earlier sentence). Misdocuments where the guard lives for future perf/security work.
- **Failure scenario:** a future refactor trusts the comment, drops/changes the caller-supplied pre-auth cap, and silently re-widens unauthenticated buffering to 16 MiB/peer.
- **Suggested fix:** reword to "enforced by the caller (shamir-server passes `MAX_PRE_AUTH_FRAME`); this crate only bounds tungstenite's buffering via `server_ws_config`".

### 5. `accept_browser_ws` deep-clones the origin policy per connection accept
- **File:line:** `crates/shamir-transport-ws/src/server.rs:118` (`let policy = policy.clone();`)
- **Severity:** nit
- **Issue:** every browser upgrade allocates and copies the whole allowlist `Vec<String>` only because the handshake callback needs an owned value. Cost is O(allowlist) per *connection* (constant w.r.t. traffic), so minor — but trivially avoidable.
- **Failure scenario:** none; connection-churn allocator noise on browser-heavy deployments.
- **Suggested fix:** take `Arc<BrowserOriginPolicy>` (or `impl AsRef<BrowserOriginPolicy>` + clone the `Arc`) at the API boundary; clone the `Arc` per accept instead of the contents.

### Not findings (checked and clean)
- **Recv path allocation:** `ws_recv_into_stream` writes into the caller buffer with `clear` + `extend_from_slice`; capacity reuse is asserted by `tests/framing_round_trip.rs::round_trip_into_buffer_reuses_capacity`. The allocating `ws_recv` is the documented convenience variant and is not used by the server request loop (it uses `read_frame_into` with a reused `frame_buf`).
- **Hidden O(N) scans:** `BrowserOriginPolicy::allows` is a linear scan but over an operator-config allowlist (constant, per-connection) — O(1) w.r.t. traffic. No `scc::*::len()`, no `Mutex`/`RwLock` anywhere in the crate (pillars 1/3/5 clean).
- **Error-path allocations only:** `format!`/`to_string()` in `OriginRejected`/`WsFrameError` variants fire on rejection paths, not steady state.
