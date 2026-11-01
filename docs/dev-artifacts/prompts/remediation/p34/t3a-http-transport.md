# TOKEN-T3a - new crate shamir-transport-http: the HTTP/1.1 server transport for the token REST channel

Source: `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md` (normative; sections 1, 4, 9 and 11 are the contract of this stage; section 10 is context for the later WebSocket stage T6a) and the design `docs/dev-artifacts/design/token-api-rest-ws.md` (sections 4, 5, 8, 10, 15). This stage builds only the transport: an HTTP/1.1 server library with TLS, hard framing rules, deadlines, a global request-body budget and a pluggable authentication gate and executor. It has NO dependency on `shamir-db`, `shamir-server`, `shamir-connect` or `shamir-engine`, knows nothing about tokens or the database, and is tested entirely with fake gates. The server-side wiring (listener kind, token verification, `ScopedHandler`, status mapping of application errors) is T3b; the WebSocket upgrade is T6a (design the seams for it now, do not implement it). Read first: `CLAUDE.md` (Code ideology, layout limits, rust-guard gates), `crates/shamir-transport-ws/` and `crates/shamir-transport-tcp/` (sibling transports: crate shape, `Cargo.toml`, TLS helpers in `shamir-transport-tcp::tls`, listener bind policy), `crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs` (how accept loops use the connection and per-IP limiters), and `crates/shamir-server/src/security/observability/` (the existing axum 0.7 metrics server; do NOT reuse axum here).

## Goal

A new workspace crate `crates/shamir-transport-http` (member through the `crates/*` glob; family of `shamir-transport-tcp/ws/ipc`) exposing a small, fully tested server API.

### A. Dependencies

hyper 1.x (`features = ["http1", "server"]`), `hyper-util` (the `tokio` feature, for `TokioIo`), `http`, `http-body-util`, `bytes`, `tokio` (as the siblings), `tokio-rustls` and `rustls` (the versions the siblings use), `shamir-transport-tcp` (TLS material helpers and bind policy), `thiserror`, `tracing`, `tokio-util` (`CancellationToken`). All of these are already in the lockfile (axum and the siblings use them); do not add axum, `hyper` features beyond those listed, or any crate not in `Cargo.lock`. Any other new dependency: stop and report. Dev-dependencies: `tokio` test features, `rcgen` only if the sibling transport tests already use it for self-signed certificates (reuse their helper), nothing else. TLS: TLS 1.3 only, no 0-RTT/early data, ALPN `http/1.1` only.

### B. Public API (generic, no token knowledge)

- `HttpServerConfig { limits: HttpLimits, endpoints: EndpointSet { rest: bool, ws: bool }, tls: Option<Arc<rustls::ServerConfig>>, hsts: bool }` and `HttpLimits { max_head_bytes: 8192, max_headers: 64, max_body_bytes: usize, head_timeout, body_timeout, request_timeout, body_budget: Arc<BodyBudget> }` with documented defaults (5 s, 30 s, 120 s; body limit supplied by the caller).
- `BodyBudget`: global in-flight request-body budget shared by all connections. `try_reserve(n) -> Option<BodyReservation>` with an exact CAS loop on an `AtomicUsize` (never O(N)); the reservation releases on drop. A `Content-Length` request reserves its declared length up front (over budget -> `503` with `Retry-After`); a chunked body reserves incrementally as chunks arrive. Debug oracle `in_use()`.
- `trait TokenGate: Send + Sync + 'static { type Grant: Send + Sync + 'static; async fn authenticate(&self, auth: AuthHeader<'_>, endpoint: Endpoint, peer: PeerInfo) -> GateDecision<Self::Grant>; fn still_valid(&self, grant: &Self::Grant) -> bool; }` where `AuthHeader` is `Missing | Single(&[u8]) | Multiple` (the transport never interprets the value), `GateDecision` is `Allow(Grant) | Unauthorized | Forbidden(code) | RateLimited { retry_after_secs } | Unavailable { retry_after_secs }`. Use `async-trait` only if a sibling already depends on it, otherwise return boxed futures by hand or use native `async fn` in traits where the MSRV allows (check `rust-toolchain`).
- `trait RestExecutor<G>: Send + Sync + 'static { async fn execute(&self, grant: &G, body: Bytes) -> ExecOutcome; }` with `ExecOutcome { status: ExecStatus (Ok 200 | BadRequest 400 | Forbidden 403 | RateLimited 429 | Unavailable 503), body: Bytes, hold: Option<Box<dyn Send + Sync>> }`; `hold` is dropped only after the response body has been fully written or the connection died (implement with a body wrapper that owns it; this is how the server keeps its response-byte-budget guard alive until the bytes are on the wire).
- `trait ConnAdmission: Send + Sync + 'static { fn admit(&self, peer: &SocketAddr) -> Option<Box<dyn Send + Sync>> }` (the permit lives for the connection; `None` closes the socket immediately).
- `serve(listener: tokio::net::TcpListener, cfg: Arc<HttpServerConfig>, gate: Arc<G>, exec: Arc<E>, admission: Arc<A>, shutdown: CancellationToken) -> impl Future<Output = ()>`: accept loop, per-connection task, TLS accept with the head deadline, graceful stop on `shutdown` (stop accepting, let in-flight requests finish within `request_timeout`).
- `PeerInfo { addr: SocketAddr, tls: bool }`. X-Forwarded-For interpretation is NOT here (the server crate owns `trusted_proxies`); the transport hands the gate the TCP peer only and exposes the raw `X-Forwarded-For` header values through `PeerInfo::forwarded_for: Vec<Vec<u8>>`.
- Seams for T6a (declare, do not implement): `Endpoint { Rest, Ws }` and a documented `// T6a` extension point in the router for `GET /shamir/v1/ws`; with `endpoints.ws == false` the path is `404`.

### C. Behaviour (TOKEN_API.md section 9, exactly)

Routes: `POST /shamir/v1/request`, `GET /shamir/v1/ping` (the executor receives the msgpack `Ping` request body supplied by the caller through a config hook `ping_body: Bytes`), everything else `404`, wrong method on a known path `405` with `Allow`. No unauthenticated route exists, every route goes through the gate first.

1. Gate before body: the `Authorization` header is extracted (exactly one header -> `Single`, none -> `Missing`, several -> `Multiple`) and `authenticate` is called BEFORE any body byte is read. `Unauthorized` -> `401` with the fixed msgpack body `81 a5 "error" ac "unauthorized"` and `WWW-Authenticate: Bearer`; `Forbidden(code)` -> `403` with a body the gate does not supply, so the transport asks the executor-supplied `forbidden_body(code)` hook (callback in the config) for the bytes; `RateLimited` -> `429` + `Retry-After`; `Unavailable` -> `503` + `Retry-After`.
2. Any response sent before the body was read (including `401/403/415/429/503/404/405`) carries `Connection: close` and the connection is closed afterwards (an unread body must never be interpreted as the next request).
3. `Content-Type` must be `application/msgpack` (case-insensitive media type, no other parameters required) else `415`, checked after the gate and before the body.
4. `Expect: 100-continue`: hyper's automatic `100 Continue` must be sent only after the gate allowed the request; verify from the hyper source (cargo registry) that polling the body is what triggers it and structure the code so a rejected request never polls the body; document the finding.
5. Body: `Content-Length` over `max_body_bytes` -> `413` immediately without reading (close); chunked bodies are counted as they stream and abort with `413` past the limit; the `BodyBudget` is reserved as above; `body_timeout` covers the whole body, `head_timeout` the head, `request_timeout` the whole request; expiry -> `408` + close. After the body is read and before the executor runs, `gate.still_valid(&grant)` is called; false -> `401`.
6. Response headers on every response: `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Content-Type: application/msgpack` for bodies, `Strict-Transport-Security: max-age=31536000` only when `tls` is on and `hsts` is true. NEVER any `Access-Control-*` header (assert in a test).
7. Framing guard (a byte-level scanner UNDER hyper; hyper alone is too lenient about bare LF): inspect the bytes of each request head before hyper sees them and reject with a canned `400` + close: a bare `\n` not preceded by `\r` inside the head, obsolete line folding (a header line starting with SP or HT), a request line or header containing NUL or other control bytes, more than one `Content-Length` header, `Content-Length` together with `Transfer-Encoding`, a `Transfer-Encoding` value other than exactly `chunked`, a non-numeric or signed `Content-Length`. Head over `max_head_bytes` or more than `max_headers` headers -> `431` + close. Implement the scanner as a small incremental state machine in its own file with its own exhaustive unit tests (feed the bytes in arbitrary split points; the verdict must not depend on how the stream is chunked). It is a validator, not a parser of semantics; hyper still does the real parsing. Pipelined requests are processed strictly sequentially by hyper; each one is authenticated independently (this relaxes the earlier "no pipelining" wording; say so in the design note and report it so the spec is amended).
8. HTTP/1.1 only; HTTP/1.0 and h2 prior-knowledge are rejected. Hyper settings: keep-alive on, `max_buf_size` tied to `max_head_bytes`, header read timeout through hyper-util's timer if hyper's own is used, `title_case_headers` off.
9. No panics on any input; no `unwrap` outside tests; secrets: the transport never logs header values, only header names and lengths, and never logs the body.

### D. Layout of the crate

`src/lib.rs` (re-exports), `config.rs`, `budget.rs` (`BodyBudget`), `gate.rs` (traits and decision enums), `framing/` (`mod.rs`, `scanner.rs`, `canned.rs`), `server/` (`mod.rs`, `accept.rs`, `connection.rs`, `service.rs`, `response.rs`, `body.rs`), `tests/` (topic folders). At most 7 entries per folder, at most 1000 lines per file, `mod.rs` re-exports only, tests under `src/tests/` in topic files (`framing_tests`, `gate_order_tests`, `body_limit_tests`, `budget_tests`, `tls_tests`, `routing_tests`, `shutdown_tests`).

## Tests (red first in CI; no sleeps: use `tokio::time::pause` where timers matter, hooks and `Notify`/oneshot seams otherwise)

All through real loopback sockets (`127.0.0.1:0`) with a raw-bytes client written in the tests, a fake gate that records every call, and a fake executor:

- order: a request whose body never arrives and whose gate answer is `Unauthorized` gets `401` while the client has sent only headers (proves the body is not read first); `Expect: 100-continue` with a rejected token never sees `100 Continue`; with an allowed token it does.
- framing: each anomaly of C7 as raw bytes -> canned `400` and the socket closed; the same anomalies split at every byte boundary give the same verdict; `431` for oversized head and for 65 headers; bare `\r` inside a header value.
- auth header: none / one / two -> `Missing` / `Single` / `Multiple` reach the gate; gate never sees the body.
- body: over-limit `Content-Length` -> `413` without reading; chunked over limit -> `413`; exact limit passes; budget exhaustion -> `503` + `Retry-After`; reservations return to zero after every outcome (success, early reject, client abort, timeout); `still_valid == false` after the body -> `401` and the executor is not called.
- timeouts: slow head -> `408`; slow body -> `408`; idle keep-alive connection is closed (use paused time).
- response headers: `Cache-Control`, `nosniff`, no `Access-Control-*` ever, HSTS only on TLS with the flag; `405` has `Allow`; `404` for unknown path and for `/shamir/v1/ws` when ws is disabled.
- `hold`: the executor's `hold` object is dropped only after the full response body was written (assert with a drop flag and a client that reads slowly), and also when the client disconnects mid-response.
- TLS: a TLS 1.3 client works; a TLS 1.2-only client fails the handshake; ALPN `http/1.1`; a plaintext client against a TLS listener is dropped without reaching the gate.
- keep-alive: two sequential valid requests on one connection both reach the gate; a rejected request closes the connection.
- shutdown: cancelling the token stops accepting and lets an in-flight request finish.
- `BodyBudget` unit tests: exact accounting under concurrent reservations (barrier-started threads), release on drop, `in_use` never exceeds the cap.

## Deliverables

A design note `docs/dev-artifacts/audits/2026-11-01-token-t3a-http-transport/DESIGN.md` written FIRST: the layered picture (accept -> admission -> TLS -> scanner -> hyper -> service), the hyper behaviours you verified in its source for C4 and C7 (cite the file and line in the cargo registry copy), what the scanner covers that hyper does not, the body-streaming and budget protocol, how `hold` survives until the last byte, and the extension seam for the WebSocket upgrade (T6a), including how `hyper::upgrade` will take over the connection after the gate allowed the request.

## Scope

New crate `crates/shamir-transport-http` and its design note. Other files only where adding a workspace crate requires it: grep `scripts/`, `.github/`, `deploy/` and `CLAUDE.md` for lists that enumerate `shamir-transport-ws` (for example `scripts/test.sh` scopes, `scripts/ci/affected_crates.py` tables and its tests) and extend those lists minimally; report each such edit. Do NOT edit `CLAUDE.md` (its crate list changes in the documentation stage T8). `Cargo.lock` will change (a new workspace member); say so. Nothing in `shamir-server`, `shamir-connect` or any other crate changes.

## Coordination with other tasks

T2a edits `shamir-connect`; P34-03b edits engine/db/storage; nothing else touches the new crate. T3b (server wiring) starts after this stage and after T2b and will consume the traits defined here, so keep names and signatures exactly as written above unless a written reason is given in the report.

## Decisions

Binding: hyper (no hand-written HTTP parser; the scanner is only a validator under it); TLS 1.3 only, no 0-RTT; the transport knows nothing about tokens; pipelined requests are served sequentially and each is authenticated; every early response closes the connection; no CORS headers ever.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-transport-http (new crate); `cargo check -p shamir-server --all-targets` is not needed because nothing outside the new crate and the CI lists changes. Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
