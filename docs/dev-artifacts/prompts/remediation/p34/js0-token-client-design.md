# TOKEN-JS0 - design of the token client in shamir-client-ts (browser and Node.js, http and ws)

Documentation-only stage. No code, no builds, no tests. Deliverable: one design note plus a list of proposed spec amendments; the orchestrator applies the amendments to the normative spec after the owner decision.

## Sources to read first

- `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md` (normative: tokens, REST `POST /shamir/v1/request` and `GET /shamir/v1/ping`, WebSocket `GET /shamir/v1/ws`, allowlist, close codes 4010-4012, error model).
- `docs/guide-docs/client-server-protocol-spec/TRANSPORT_WS.md`, `CLIENT_BROWSER.md`, `SECURITY_MODEL.md`, `ADMIN_UI_HOSTING.md`.
- The client: `crates/shamir-client-ts/src/` - entry points `index.ts` (Node) and `browser.ts`; platform layer `platform/browser.ts`, `platform/node.ts`; core `core/client/` (`client.ts`, `client-internals.ts`, `subscription-router.ts`, `cursor-iterator.ts`), `core/protocol/` (`framing.ts`, `scram.ts`, `protocol.ts`), `core/types/`. Read how `ShamirClient.connect(platform, opts)` and the `Platform` abstraction work today (SCRAM over WebSocket) and what a transport must provide.
- Existing TS tests layout under `src/__tests__/` and `src/platform/__tests__/`.

## What the note must decide (each point: the decision, the rejected alternatives, the consequences)

1. **Public API.** How a caller passes a token and picks a channel: a variant of `ConnectOptions` (auth by token vs the existing login/password), channel `http | ws` derived from the URL scheme (`http`, `https`, `ws`, `wss`) or explicit. Exact TypeScript signatures for `connect` in `index.ts` (Node) and `browser.ts`, new option/error types, what `ShamirClient` exposes over each channel. The HTTP channel offers only the allowlist (Ping, Execute, WaitForVisibility, GetDdlOpStatus): what is the client behaviour for an operation outside it (typed error before sending, never a silent downgrade). Transactions and cursors over WS only.
2. **Transport seam.** How an HTTP transport and a token WebSocket transport plug into the existing core without breaking the SCRAM path: which interface in `core/` the transports implement, what stays in `platform/{browser,node}.ts`, how `subscription-router` is reused for WS, how request ids and the `{sid, rid, req}` / `{rid, res}` / `{rid, error}` envelopes map onto the existing protocol layer.
3. **The browser WebSocket problem (needs an owner decision).** A browser `WebSocket` cannot set an `Authorization` header, while TOKEN_API.md requires `Authorization: Bearer` on `/shamir/v1/ws`. Evaluate at least: token in a second `Sec-WebSocket-Protocol` value; token in a first frame after the upgrade; token in the query string (record why it is bad: logs, history, referrer); HTTP-only for browsers. For each: security (log exposure, replay, CSWSH/Origin interplay), server impact (listener change in stage T6, spec text), library impact, behaviour behind proxies. Give a recommendation and the exact spec amendment it implies.
4. **Node.js specifics.** Which HTTP client (global `fetch`/undici versus `node:https`) and which WebSocket (`ws` package versus the built-in `WebSocket` of Node 22+), supported Node versions (read `package.json` `engines`), TLS options (custom CA, `rejectUnauthorized` only as an explicit opt-in with a loud name), keep-alive and agent shutdown, `AbortController` timeouts. Dependency policy: prefer no new runtime dependency; if `ws` is needed, justify it.
5. **Browser specifics.** `fetch` + `AbortController`, no Node-only modules in the browser bundle (check how the two entry points are built in `package.json`/`configs/`), CORS reality (TOKEN_API.md sends no CORS headers: what this means for a browser client on another origin, and what is expected of the deployer; do not invent server behaviour, list it as an open item for the owner), where a token may be kept (memory only by default; storage is the caller's decision).
6. **Lifecycle and errors.** Token expiry and revocation: close codes 4010 `token_sid_mismatch`, 4011 `token_revoked`, 4012 `token_expired` mapped to typed errors; behaviour of pending requests and subscriptions on close (reject with the typed error, never retry with the same token); no automatic reconnect with a dead token; HTTP status mapping (401 uniform, 403, 413, 415, 429/503 with `Retry-After`, 4xx application errors) onto `ShamirDbError`/`ShamirTimeoutError`/`isRetryableCode`.
7. **Secret hygiene.** The token never appears in `toString`, `JSON.stringify`, `util.inspect`, error messages, thrown stack text, URLs, logs or test snapshots; how the type enforces it (a class with redacting methods, symbol-keyed field, or an opaque handle).
8. **Login flow hook.** A later stage (AUTH2) adds `login({ username, password })` over HTTP that returns a token (see the AUTH0 spec stage). Reserve the API shape so that stage fits without a breaking change.
9. **Test strategy.** Unit tests against a fake server on `node:http` and `ws`; what can be tested in the browser build without a browser; the e2e plan against a real server in CI (stage JS4): headless browser tool already used by the repo's e2e or a proposal; Windows and Linux.
10. **Stage breakdown.** Confirm or correct the task split JS1 (HTTP transport), JS2 (WS transport), JS3 (public API), JS4 (e2e), JS5 (docs): file-level layout for each (folders under `src/`, at most 7 entries per folder, at most 1000 lines per file, tests under `__tests__/`), and the order.

## Output

- `docs/dev-artifacts/design/token-client-js.md` (the note; headings per point above; an "Owner decisions needed" section at the top with each question and your recommendation).
- A section "Proposed spec amendments" in the same note: exact replacement text for TOKEN_API.md / TRANSPORT_WS.md / CLIENT_BROWSER.md where the decision changes them. Do NOT edit the normative spec files themselves.

## Rules

- Documentation only. Touch only the design note. Do not run any build, test, npm or cargo command; reading files is fine.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` or any git command that mutates the tree or index. You cannot move or delete files.
- Use forward slashes in all paths. Examples of queries use the typed builder of `shamir-client-ts`, never hand-assembled JSON.
- After each numbered point is done, append one line to a `## Progress` list at the end of the note so a crash does not lose the work. The final message is mandatory and is also written into the note under `## Final report`: per point DONE / PARTIAL with the file evidence you relied on, open questions, and everything you could not establish by reading.
