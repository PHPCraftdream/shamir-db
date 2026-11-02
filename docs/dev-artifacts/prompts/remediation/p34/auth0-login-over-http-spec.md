# TOKEN-AUTH0 - specification of login/password sign-in over HTTP that issues an API token

Documentation-only stage. No code, no builds, no tests. Deliverable: a normative specification file plus a list of amendments to neighbouring spec files; an independent reviewer attacks it afterwards; the orchestrator sends the owner-decision cards.

## Owner's decision (already taken)

Support sign-in by login/password over HTTP; the result is an API token (the same token as in TOKEN_API.md), not a separate HTTP session and not HTTP Basic. Rejected: HTTP Basic (Argon2 on every request, password on every request, bypasses the failure limiters designed for tokens).

## Sources to read first

- `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md` (normative: token format and verification, `tickets_invalid_before_ns` reuse, listener model, REST channel, issuance rules, "browser sessions cannot create tokens", limits).
- The SCRAM specification files in the same folder (`HANDSHAKE.md`, `SECURITY_MODEL.md`, `TRANSPORT_TCP.md`, `TRANSPORT_WS.md`, `CLIENT_BROWSER.md`; list the folder and read what matters) and the code that implements the SCRAM server: `crates/shamir-connect/src/server/` (state machine, `auth/`, lockout, rate limit, Argon2 semaphore) and `crates/shamir-server/src/sync/connection/session/scram/`.
- The client SCRAM implementation `crates/shamir-client-ts/src/core/protocol/scram.ts` (what a browser client can do: WebCrypto, argon2-browser).

## What the specification must define (file: `docs/guide-docs/client-server-protocol-spec/TOKEN_LOGIN_HTTP.md`)

1. **Endpoints** on the http listener (names are a proposal, justify them): a two-step SCRAM exchange `POST /shamir/v1/auth/begin` and `POST /shamir/v1/auth/finish`, MessagePack bodies, exact field lists and byte sizes, content types, status codes. The password never crosses the network.
2. **Handshake state.** Server-side pending state under an opaque handshake id: size, entropy, TTL, hard cap on the number of pending handshakes (global and per source), eviction, single use, what happens on a second `finish`, on a `finish` from another source, on expiry. Memory-exhaustion argument.
3. **Channel binding.** The two HTTP requests may use different TCP/TLS connections, so a TLS exporter cannot bind them; decide the binding mode (the browser mode `0x02`, `tls_no_export`, or an explicit "http-login" mode), what the proof covers, and what that means for downgrade and relay attacks. Plain HTTP only on loopback or with the explicit opt-in used by the token listener; state what a passive attacker gets and what an active one gets in every case. This point needs an owner decision: give a recommendation.
4. **Issued token.** Owner = the authenticated user; default `mode = read_only` with an explicit request for `read_write` (never above the user's own rights); requested `channels` and `databases` scope; short TTL from a new setting `tokens.login_ttl` (capped by `tokens.max_ttl_days`); returned once in the `finish` response; tokens are never administrative, a superuser or replicator who signs in gets a non-admin token (state this explicitly); how the label is set. This point needs an owner decision.
5. **The existing rule "browser sessions cannot create tokens".** Define the explicit, narrow exception for this path and why it is safe, or reject the exception and propose the alternative. This point needs an owner decision.
6. **Abuse resistance.** Same lockout and failure accounting as SCRAM (per user and per source), per-subnet failure limiter (with `trusted_proxies` rules from TOKEN_API.md), Argon2 only through the bounded semaphore with a queue limit, constant-shape responses (unknown user versus wrong password indistinguishable in status, body and timing class), request size and time limits, protection against handshake fixation and parallel `finish`, audit events (success, failure with reason class, never the password or proof), metrics.
7. **Browser concerns.** No cookies; `Origin` handling (TOKEN_API.md sends no CORS headers: state what the endpoints do for cross-origin browser calls, and list it as an open item for the owner if a decision is needed), CSRF argument, `Cache-Control: no-store`.
8. **Interaction with revocation.** Password or role change bumps `tickets_invalid_before_ns` and so kills login-issued tokens; `RevokeUserTokens` revokes them; the token is a normal token afterwards.
9. **Errors.** One uniform error for credential failures; the full status table (400, 401, 403, 408, 413, 415, 429, 503) with when each is used.
10. **Test checklist** for the later implementation stage (AUTH1) and e2e stage (AUTH3), including negative tests (replay, wrong source, expired handshake, cap overflow, timing class, plaintext refusal).

## Amendments

In the same stage, list the exact text changes needed in `TOKEN_API.md`, `SECURITY_MODEL.md`, `TRANSPORT_WS.md` and `README.md` of the spec folder (index entry for the new file) as a section "Proposed amendments" at the end of the new file. Do NOT edit those neighbouring files; the orchestrator applies them after review.

## Rules

- Documentation only. Touch only the new spec file. Do not run any build, test, npm or cargo command; reading files is fine.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` or any git command that mutates the tree or index. You cannot move or delete files.
- Use forward slashes in all paths. No private machine paths in the text.
- After each numbered point is done, append one line to a `## Progress` list at the end of the file so a crash does not lose the work. The final message is mandatory and is also written into the file under `## Final report`: per point DONE / PARTIAL, the code locations you relied on (file:line), the owner-decision questions with your recommendations, and everything you could not establish by reading.
