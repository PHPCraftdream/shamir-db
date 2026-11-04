# T9-3a - Rust client: token over REST (`TokenHttpClient`, `ApiToken`, TLS trust, error mapping)

Base: branch `feat/t9-2-ws` (contains R1-R3 and T9-2). Design: `docs/dev-artifacts/audits/2026-11-04-t9-client-matrix/DESIGN-T9-3.md` sections 0, 2, 3, 4 and 7 (read them first, then `DESIGN.md` for the matrix and the R1-R3/T9-2 reports at its end, then CLAUDE.md). Crate: `crates/shamir-client`. Server and other crates do not change. Token WebSocket (T9-3b) and password login (T9-4) are NOT part of this task.

## Goal

A second, token-authenticated client over the server's HTTP listener (`kind: http`, REST endpoints `POST /shamir/v1/request`, `GET /shamir/v1/ping`), independent of the SCRAM `Client`:

- `ApiToken` (secret type, validation, redacted `Debug`), `TokenEndpoint` (`https(addr, server_name, trust)` / `plain_loopback(addr)`), `TokenTrust` (`WebPkiRoots` / custom roots from PEM or DER), `TokenHttpOptions` (timeouts: connect, request), `TokenHttpClient` (`connect`, `ping`, `request`, `execute`, `wait_for_visibility`, `get_ddl_op_status`, `close`).
- New `ClientError` variants `Http { status, code, retry_after }` and `InvalidToken`, exactly as described in the design (section 2).
- All new public types are `#[non_exhaustive]` with constructors. Re-export the public ones from `lib.rs` (`ApiToken`, `TokenEndpoint`, `TokenTrust`, `TokenHttpOptions`, `TokenHttpClient`).

## Hard rules for the implementation

- Use `hyper` 1.x low-level client (`hyper::client::conn::http1`) over a `TokioIo` TLS stream. Do NOT use tungstenite and do NOT use `make_client_config_app_authenticated` / `AppLayerServerAuth` anywhere on this path: the token path verifies the chain and the host name with the standard WebPKI verifier (design section 2, "TLS").
- The token never appears in `Display`, `Debug`, error values, `tracing` output or panics; the `Authorization` header value is built once as `HeaderValue` with `set_sensitive(true)` over a zeroizing owner (design section 4). Token parsing/validation goes through `shamir_connect::server::token::parse_token` (check its exact public path in `crates/shamir-connect/src/server/mod.rs`; the `auth` module itself is private).
- Request filter in `request()` is deny-by-default and local (design section 2): only `Ping`, `Execute`, `WaitForVisibility`, `GetDdlOpStatus` go out; transactions and cursors fail locally with `ClientError::Db { code: "not_supported_over_rest", .. }`; everything else with `token_request_denied`. Read how the session `Client` maps `DbResponse` to results (`client/visibility/*`, `client/api.rs`) and reuse those mappers instead of duplicating them; read `crates/shamir-query-types/src/wire/` for `DbRequest`/`DbResponse`.
- Connection reuse: a small lock-free pool (`scc::Bag` plus an `AtomicUsize` bound of 4) as in the design; never retry a request that may have been written.
- `Cargo.toml` of `shamir-client`: add EXACTLY the dependency lines of design section 3 (`hyper`, `hyper-util`, `http-body-util`, `bytes`, `webpki-roots`); no other dependency, version or feature change anywhere. Do not touch `Cargo.lock` (the orchestrator regenerates it).

## Layout (limit 7 entries per folder, 1000 lines per file, one primary export per file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`)

```
src/client/transport/token/{mod,endpoint,tls,http_conn,status}.rs  + tests/ (unit tests live next to the module as tests/ per CLAUDE.md)
src/client/session/token/{mod,api_token,options,http_client}.rs     + tests/
tests/matrix/token/{mod,common,http}.rs    and `mod token;` added to tests/matrix/main.rs
```

`client/transport/` has 6 entries today (7 with `token/`), `client/session/` 6 (7 with `token/`), `tests/matrix/` 6 (7 with `token/`). Wire the new modules from `transport/mod.rs`, `session/mod.rs` and `client/mod.rs` with re-exports only; the new public API is the only widening of visibility - keep everything else as narrow as possible and list every widening in your report.

## Tests (CI-only; they must really exercise the code)

Unit (against a local fake `tokio::net::TcpListener`, plaintext loopback endpoint, no TLS needed): exact request head (one `Authorization`, no `Cookie`/`Origin`/`Expect`/`Connection`/`Transfer-Encoding`, `Content-Type: application/msgpack`, path `/shamir/v1/request`, `GET /shamir/v1/ping`), keep-alive reuse (one accept for two requests) and a fresh connection after `Connection: close`, a chunked response is accepted, a 307 is not followed (`Http { status: 307 }`), body larger than the limit fails, the status table (200 with the wrong content type, 400/403 with a `DbResponse::Error` body, 401, 404, 429/503 with `Retry-After`: delta seconds and an HTTP-date), `plain_loopback` refuses a non-loopback address before opening a socket, the local filter (`Tx*`, cursor requests -> `not_supported_over_rest`, nothing is sent). `ApiToken`: valid form, wrong length/prefix/non-canonical base64 -> `InvalidToken` without the input in the message, `Debug` never prints the secret, `id()` is the non-secret part.

Integration, `tests/matrix/token/` against a live server (design section 7, "Test server"): own `common.rs` (do not widen the private `make_config` of the T9-2 `matrix/common.rs`); an HTTP listener with `tls: Some(true)` (self-signed on loopback is allowed) and one plaintext loopback listener, owner seeded like `crates/shamir-server/tests/e2e/http_rest/common.rs::seed_owner`, token issued through `handle.token_service.create(NewToken{..})` the way `crates/shamir-server/tests/e2e/http_ws/session_loop.rs` does it (read both files first; copy the working calls, do not invent signatures), trust anchor = the server's generated `cert.pem` (SAN `localhost`, `server_name = "localhost"`; see `crates/shamir-server/tests/e2e/token_matrix/support.rs` for the working rustls setup). Cases: ping and a small `execute` batch built with the query builder over TLS; a bad token -> `Http { status: 401, code: Some("unauthorized") }`; a read-only token's write -> `Db { code: "token_read_only" }` (check the real code in `docs/guide-docs/guide/token-api.md`); a transaction request -> `not_supported_over_rest`; the plaintext loopback listener works too. Multi-line indented literals; no raw JSON queries.

Do not edit existing tests or the server.

## Rules

- NO LOCAL COMPILATION OR TESTS. Allowed commands: `cargo fmt -p shamir-client` (and `-- --check`) and `python -I scripts/ci/check_layout.py` only. No `cargo check/clippy/build/test/metadata`, no rustc, no npm. Children (if any) only edit files and must not run commands.
- Without a compiler, finish with a compiler-style read of every touched file: `use` paths, privacy, hyper 1.x API (`conn::http1::handshake`, `SendRequest::{ready, send_request, try_send_request}`, `Request::builder`, `Full<Bytes>`, `BodyExt::collect`, `Limited`), read the real signatures in the cargo registry sources (`D:/system_artefact/cargo/registry/src/*/hyper-1.*`, `hyper-util-*`, `http-body-util-*`, `rustls-*`, `tokio-rustls-*`, `webpki-roots-*`) instead of writing from memory; `Send` bounds of spawned connection tasks, `Pin`/`Unpin`, `cfg(test)` re-exports, unused imports and `dead_code` under `-D warnings`, `clippy::absolute_paths` (max 5 segments).
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show/log/diff` is fine). Only edit files; the orchestrator commits. No machine-local absolute paths in any file.
- Hard session limit 150 minutes: stop editing 40 minutes before the time in the run file and spend that time on the compiler-style read. Never leave a half-edited item: finish it or revert by editing back and record it BLOCKED. Append a "T9-3a report" section to `DESIGN.md` as the last step (what was added, widenings, decisions, what is not done).
