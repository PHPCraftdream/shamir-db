# T9-2 - Rust client: session over native WebSocket, resume over WebSocket, session over plain loopback TCP

Base: branch `feat/t9-r3-transport` (pre-refactor R1/R2 landed; R3 adds `client/transport/{binding,dial,write_sink,mod}.rs` with `ChannelBinding`, `Dialed<R>`, `dial_tcp_tls`, `dial_ipc`). Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN.md` section 3 "T9-2" and sections 1-2 (read them, then the R1/R2/R3 reports at the end, then CLAUDE.md). Crate: `crates/shamir-client`. Server side exists and does not change.

## Goal

Three new ways to open a `Client`, all through the existing layers (dial -> `run_scram` -> `assemble`), nothing else changes:

1. `Client::connect_ws(opts: ConnectOptions)` - native WebSocket listener (`kind: ws`, `profile: tls_exporter`, path `/shamir/v1`, binding mode 0x01). Reuse `ConnectOptions` as is (no new public options struct).
2. `Client::resume_ws(opts: ResumeOptions)` and `Client::resume_ws_secure(opts: SecureResumeOptions)` - the ticket resume over the same WebSocket listener, mirroring `resume` / `resume_secure`.
3. `Client::connect_plain(opts: ConnectOptions)` - plain TCP (`kind: tcp`, `profile: plain`, binding mode 0x00, zeroed exporter). The server accepts it on loopback only: the client MUST refuse a non-loopback `opts.addr` (fail closed, with an existing `ClientError` variant of your choice, text `plain TCP is loopback-only`) before opening any socket. `server_name` is ignored.

Out of scope (record in your report as not done, do not start): the browser WebSocket profile (needs WebPKI trust), HTTP/token paths, napi, TypeScript.

## Wire facts for the native WebSocket path (verified against the server)

- TLS 1.3, same trust model as the TCP path: `make_client_config_app_authenticated(AppLayerServerAuth::exporter_bound())`, the Ed25519 identity pin is checked inside SCRAM exactly as for TCP.
- The TLS exporter (`EXPORTER-ShamirDB-AUTH-v1`, empty context, 32 bytes) is extracted from the `TlsStream` BEFORE the WebSocket upgrade, with the same `extract_tls_exporter` the TCP path uses; the value and label are identical to TCP, so tickets move between TCP and WS.
- Upgrade: `GET /shamir/v1` over the TLS stream with `Sec-WebSocket-Protocol: shamir-v1` (mandatory; the server answers 400 otherwise). Wrong path is 404. Native profile sends NO `Origin` header. Look at `crates/shamir-transport-ws/src/server.rs` (accept side) and `crates/shamir-server/tests/e2e/core/mvp_ws_e2e.rs` (a working client side over a real server) for the exact request shape, then copy it.
- Framing: every WebSocket BINARY message carries exactly one frame `[u32_be length][payload]` where `length` equals the body length (see `crates/shamir-transport-ws/src/framing.rs`). TEXT messages are an error. Frame limits are the same as TCP (`MAX_PRE_AUTH_FRAME` before auth).
- The server pings and closes with code 4003 when no pong comes back: the reader task must keep polling the socket (tungstenite answers pings automatically while it is read). A WebSocket Close from the server must surface as end-of-stream (EOF) to the reader.
- `auth_message` uses `TransportKind::WebSocket` and `BindingMode::TlsExporter`.

## Design (follow it; deviations only with a written reason in the report)

Keep `Client`, `reader_task` and the write path untouched by adapting the WebSocket to a byte stream:

- New `client/transport/ws_stream.rs`: `WsByteStream<S>` wrapping `tokio_tungstenite::WebSocketStream<S>` and implementing `AsyncRead + AsyncWrite + Unpin + Send`:
  - read side: each received BINARY message is appended to an internal read buffer and served to `poll_read` (message boundaries are irrelevant to the reader because the bytes already contain the `[u32_be len]` prefix); Ping/Pong are handled by tungstenite; Close -> return EOF; TEXT or malformed -> `io::ErrorKind::InvalidData`.
  - write side: buffer bytes handed to `poll_write` until one complete frame is present (parse the leading `u32_be` length, wait until `4 + len` bytes are buffered), then send them as ONE BINARY message; `poll_flush` drives the sink; `poll_shutdown` sends a Close and closes. A caller may hand the prefix and the payload in separate writes or one buffer; both must work. Reject a declared length above the existing frame maximum (`InvalidData`).
  - It is then split with `tokio::io::split`, exactly like the TLS stream, so `WriteSink` gets a new variant `Ws(WriteHalf<WsByteStream<TlsStream<TcpStream>>>)` (plus `Tcp(WriteHalf<TcpStream>)` for the plain path) with the same delegating `AsyncWrite` impl, and the read half goes to `reader_task` unchanged.
- New `client/transport/dial_ws.rs`: `dial_ws_native(addr, server_name, connect_timeout, phase) -> Dialed<ReadHalf<WsByteStream<..>>>` (phases and errors as in `dial_tcp_tls`; the upgrade is a new step, error text `websocket upgrade`; `ChannelBinding::ws_tls_exporter(exporter)` = `(WebSocket, TlsExporter, exporter)` added to `binding.rs`). And `dial_tcp_plain(addr, connect_timeout) -> Dialed<ReadHalf<TcpStream>>` with `ChannelBinding::tcp_plain()` = `(Tcp, None, [0; 32])`. Put `dial_tcp_plain` next to `dial_tcp_tls` in `dial.rs` only if that file stays under 200 lines, otherwise in `dial_ws.rs`' sibling `dial_plain.rs` (one primary export per file).
- New entry points in `client/session/entry_ws.rs` (the existing `entry.rs` stays as is): `connect_ws`, `resume_ws`, `resume_ws_secure`, `connect_plain`, each the same three steps as the TCP twins (dial -> `run_scram` / `resume_wire_exchange` -> `assemble`), same timeout handling (`connect_timeout`, `handshake_timeout`, `request_timeout`) and error mapping. Reuse helpers from `entry.rs` through narrowly widened `pub(in crate::client)` visibility instead of copying code. `session/mod.rs` and `client/mod.rs` keep re-exports only; the new methods are public API of `Client` (the only new public items); update the `lib.rs` docs only if they list entry points.
- `Cargo.toml` of `shamir-client`: add exactly one dependency, `tokio-tungstenite = "0.30"` (same line as in `crates/shamir-server/Cargo.toml`, already in the lockfile; do not add features, do not change any other dependency or version). Do not touch `Cargo.lock` (the orchestrator adds the one dependency edge).

## Tests (CI-only, they must really exercise the new paths)

- Unit, `src/tests/wire/ws_byte_stream_tests.rs` (register in `src/tests/wire/mod.rs`; the folder has 6 entries, 7 after): two `WebSocketStream`s over `tokio::io::duplex` (`WebSocketStream::from_raw_socket` with the client and server roles); cases: prefix and payload written in separate writes arrive as ONE message; a frame arriving split across reads; two frames in one message are not produced by the writer; TEXT message -> `InvalidData`; peer Close -> EOF; oversized declared length -> `InvalidData`.
- Integration, new folder `tests/matrix/` of the crate (`tests/` has 5 entries, 6 after) with `main.rs` (or the layout the existing integration folders use; read `tests/smoke.rs`, `tests/smoke_local.rs` and `tests/hostname/` first), a shared `common.rs` that boots the server like `smoke_local.rs` does (`ServerLauncher`, fast Argon2 `KdfConfig`) with three listeners (`tcp`+`tls_exporter`, `ws`+`tls_exporter` path `/shamir/v1`, `tcp`+`plain` on loopback), and one test file per cell: session over WS (connect, ping, execute a small batch built with the query builder, a second client concurrently), resume over WS (connect over WS, take the ticket, `resume_ws`; also a ticket issued over TCP resumes over WS and vice versa because the exporter is the same), session over plain TCP (connect, ping, execute), and the refusal of a non-loopback address by `connect_plain` (no server needed). Multi-line indented literals; no raw JSON for queries (use the builder).
- Do not edit existing tests.

## Boundary

Existing public API byte for byte (`connect`, `connect_host`, `connect_local`, `resume`, `resume_secure`, option structs, `ClientError`, `CloseCause`, getters); wire bytes of the existing paths unchanged. Layout: at most 7 entries per folder (`client/` stays 7, `transport/` has 4, +`ws_stream.rs`, `dial_ws.rs` = 6; `session/` 5 + `entry_ws.rs` = 6; `tests/wire/` 7, `tests/` of the crate 6), at most 1000 lines per file, one primary export per file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`. Narrowest visibilities; list every widening in the report.

## Rules

- NO LOCAL COMPILATION OR TESTS. Allowed commands: `cargo fmt -p shamir-client` (and `-- --check`) and `python -I scripts/ci/check_layout.py` only. No `cargo check/clippy/build/test`, no rustc, no npm. Children (if any) only edit files and must not run commands. Only the one `Cargo.toml` line above; no other dependency, lockfile or workflow edits.
- Without a compiler, finish with a compiler-style read of every touched file: `use` paths, privacy, generic bounds (`WebSocketStream<S>` needs `S: AsyncRead + AsyncWrite + Unpin`; the `Sink`/`Stream` traits from `futures` for `SinkExt/StreamExt` or poll-level calls), `Pin` projections in the `AsyncRead/AsyncWrite` impl, `Send` of futures used in `tokio::spawn`, tungstenite 0.30 API names (`Message::Binary(Bytes)` in 0.30 takes `Bytes`, check the version in `Cargo.lock` and the usage in `crates/shamir-transport-ws/src/framing.rs`, `crates/shamir-server` and `mvp_ws_e2e.rs` before writing any call), `cfg(test)` re-exports, unused imports under `-D warnings`. Copy API usage from the existing code in this repo rather than from memory.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. No machine-local absolute paths in any file.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the read. Never leave a half-edited item: finish it or revert by editing back and record it BLOCKED. Append a "T9-2 report" section to the audit DESIGN.md as the last step (what was added, widenings, the adapter decision, what is not done).
