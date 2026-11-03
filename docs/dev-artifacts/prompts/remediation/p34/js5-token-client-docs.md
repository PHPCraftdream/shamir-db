# TOKEN-JS5 - documentation of the JS token client (tokens, http/ws, login by password, browser and Node)

Base: the whole token stack is landed on master: JS1-JS3 (HTTP and WS token transports, public `connect`), T3b (REST), T5 (token administration), T6 (WebSocket channel: native bearer and browser first-frame), AUTH1 (server login endpoints), AUTH2 and its follow-ups (login client). This task is documentation only; it changes no code, tests, manifests or workflows.

## Sources to read first (the docs must match them, not the other way round)

- Specs: `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`, `TOKEN_LOGIN_HTTP.md`, `TRANSPORT_WS.md`, `CLIENT_BROWSER.md`, `SECURITY_MODEL.md`, `ADMIN_UI_HOSTING.md`.
- Client code: `crates/shamir-client-ts/src/index.ts`, `src/browser.ts` (public exports), `src/entry/` (token connect, `ApiToken`), `src/core/protocol/token/` (http and ws transports, error codes), `src/core/client/client.ts` and `client-internals.ts` (`client.apiTokens`), `src/login/` (login API, `LoginOptions`, `LoginError` codes, CA option), `src/core/support/errors.ts` (`TOKEN_ADMIN_ERROR_CODES`).
- Server behavior: `crates/shamir-server/src/security/perimeter/http_rest/` (statuses, scope classifier `scope.rs`), `http_ws/`, `sync/connection/token_ws/` (close codes 4010-4013 and their reasons), `boot/config/http_listener.rs` and `login_listener.rs` (listener keys), `deploy/server.example.ktav`.
- The e2e suites `crates/shamir-client-ts/src/__tests__/connection/token/` are working usage examples (they are the source of truth for call shapes).
- Existing docs to extend, not duplicate: `crates/shamir-client-ts/docs/README.md`, `docs/guide-docs/guide/`.

## Deliverables

1. A guide section in `crates/shamir-client-ts/docs/` (a new file `TOKENS.md` linked from `docs/README.md`; keep each file under 1000 lines) covering:
   - getting a token: the operator CLI (`shamir-server token create/list/revoke/revoke-user`) and online administration through `client.apiTokens` (create/list/revoke/revokeUser, self-service vs superuser-on-behalf, the secret is returned once and never listed);
   - connecting by token from Node.js and from the browser over http/https and ws/wss: `connect({ auth: 'token', ... })` options, channels and mode (`read_only`/`read_write`), database scope, what each channel supports (transactions, cursors, subscriptions per the spec table);
   - the HTTP channel limits (request allowlist, body cap, no push) and the WebSocket specifics (native `Authorization: Bearer` vs browser first-frame token, subprotocol, Origin/Host allow-lists);
   - login by username and password over HTTP: `login(...)` for Node and browser, options (scope, `expectedKdf`, `listenerId`, CA bundle for development TLS, abort signal, `timeoutMs`), the result and the token lifetime, the error codes (including `server_signature_mismatch`) and `Retry-After` handling;
   - error and close-code reference: REST statuses, `TokenTransportError` codes, WS close codes 4010-4013 with their meaning and what the application should do (re-login, reconnect, surface to the user), behavior on revoke and expiry;
   - security guidance: where to keep tokens (not `localStorage` unless required; prefer memory/`sessionStorage` with a short TTL), CSP, CORS and Origin policy are configured on the server listener (not in the client), never log tokens (`ApiToken` redacts), rotation and revocation practice, rate limits (429 and failure limiter).
2. Operator-facing pointers: a short section in the server operator guide location named by `docs/guide-docs/` (find the existing operator/deploy guide; if none exists, add a short `docs/guide-docs/guide/token-operations.md`) that links to the client guide and lists the listener keys (`host_authority_allowlist`, `browser_origin_allowlist`, caps, `login` block) with the values the client examples assume. The final T8 task will write the full operator manual; this is only the cross-reference.
3. Every code sample builds queries only through the typed builder of `shamir-client-ts` (`Batch`, `Query`, `write`, `ddl`, `filter`), never raw JSON; every call shape in the samples is copied from or checked against the source/e2e suites above. Examples must not contain machine-local paths or real secrets (use obvious placeholders).

## Rules

- Documentation only: no code, no test, no `package.json`/lockfile/workflow changes. No local builds or tests of any kind (no cargo, npm, tsc, vitest); only `python -I scripts/ci/check_layout.py` is allowed.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index; only edit files; the orchestrator commits. No machine-local absolute paths in any file you write.
- Children only edit files, are awaited; a child that runs cargo/npm/tsc/vitest violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt. Never finish with a child outstanding.
- Time: run `date` first; stop at the time in the run file and report per deliverable DONE / PARTIAL and the list of statements you could not verify against the code.
