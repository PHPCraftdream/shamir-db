<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — api-wire-protocol independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The /info prose mismatch and prospective query-version narrowing remain. HMAC re-derivation is refuted by pre-existing caching, and current handshake layouts agree.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 2 | 0 | 0 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `/info` endpoint documented as human-readable but returns raw msgpack

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The module promises pretty-printed curl output, but info_handler returns named MessagePack with application/msgpack. The registered HTTP assertion decodes MessagePack and the operations guide agrees with implementation; correcting that module prose preserves the existing wire contract.

Evidence: [crates/shamir-server/src/observability.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L26); [crates/shamir-server/src/observability.rs:595](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L595); [crates/shamir-server/src/observability.rs:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L598); [crates/shamir-server/tests/observability_http.rs:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/observability_http.rs#L180); [docs/guide-docs/guide/07-operations.md:279](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/guide/07-operations.md#L279).

<a id="review-2"></a>

### Claim 2 — `server_query_version` wire field is `u8` but the version constant is `u32`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both unchecked casts narrow the actual query-types constant. Current value 2 fits, so this is a prospective compatibility risk, not a current decode failure. Any widening must include both server/client DTOs, AtomicU8 storage and the public client accessor.

Evidence: [crates/shamir-server/src/connection/handshake.rs:152](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L152); [crates/shamir-server/src/connection/handshake.rs:640](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L640); [crates/shamir-query-types/src/wire/db_message.rs:21](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/wire/db_message.rs#L21); [crates/shamir-client/src/client.rs:456](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L456); [crates/shamir-client/src/client.rs:987](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L987).

<a id="review-3"></a>

### Claim 3 — `check_destructive_hmacs` re-derives the HMAC key on every destructive-op sweep across `execute` and `tx_execute`

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Session::hmac_key uses OnceLock, warmed when SessionStore stamps the ID. History confirms commit 111df6b331d1171bb60d6b9d6d7abf9f85924067 introduced this on 2026-05-16, before the original report. Calls copy cached bytes; derivation is domain-separated SHA-256, not HKDF.

Evidence: [crates/shamir-connect/src/server/session.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L284); [crates/shamir-connect/src/server/session.rs:446](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L446); [crates/shamir-connect/src/common/crypto.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L96); [crates/shamir-server/src/db_handler/admin.rs:650](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L650).

<a id="review-summary-wire-and-builder"></a>

### Claim Summary/wire-and-builder — Positional wire discipline, separate version axes and no builder-rule violations

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Inspected handshake DTOs preserve positional defaults/order, and protocol/query version axes are separate. JSON sites found in this crate serialize backup/doctor DTOs or test those formats, not construct queries. This supports the inspected paths rather than a transitive universal guarantee.

Evidence: [crates/shamir-server/src/connection/wire.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/wire.rs#L35); [crates/shamir-client/src/wire_frames.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L67); [crates/shamir-server/src/version.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/version.rs#L45); [crates/shamir-server/src/backup.rs:332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L332); [crates/shamir-server/src/doctor.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/doctor.rs#L189).

## Evidence and recipe corrections

- Changing /info to JSON would change an existing documented/tested representation; a prose correction is the minimal safe resolution.
- Clamping or wrapping an overflowing advertised version can misrepresent server capability. Prefer a compile-time ceiling or coordinated protocol/client change.
- The relevant handshake import is the query-types constant, not merely the similarly named server/version constant.
- No new HMAC cache is needed, and the pre-existing cache must not be labeled a later fix.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server -- API & wire-protocol design

## Summary

The wire surface (SCRAM handshake in `connection/wire.rs` + `connection/handshake.rs`,
the `DbRequest`/`DbResponse` bridge in `db_handler/`, the replication pull-API in
`replication/`/`db_handler/repl_handler.rs`, and the subscription push protocol in
`subscriptions/`) is unusually well-documented: every positional-msgpack struct
carries explicit "always present on the wire" / field-order-matters comments, and
version negotiation (`version.rs`) cleanly separates the handshake-protocol axis
(`u8`) from the query-language axis (`u32`). No violations of the builder-only
query-construction rule were found — the one `serde_json` use is in a test
asserting against an already-produced JSON string (backup manifest), which is a
documented exception, and the subscription `EventData`/`KeysData` payload structs
(`subscriptions/payload.rs`) are server-push serialization DTOs, not query
construction. The main findings are a genuine doc/behavior mismatch on the
operator-facing `/info` HTTP endpoint and a latent version-field width mismatch.

## Findings

### 1. `/info` endpoint documented as human-readable but returns raw msgpack

- **File:line**: `crates/shamir-server/src/observability.rs:26`, `:583-602`
- **Severity**: medium
- **Issue**: The module doc comment explicitly promises `/info` is "pretty-printed
  server info for curl-debugging by an operator" ("Optional convenience" for a
  human running curl). The actual `info_handler` implementation serializes
  `InfoBody` via `rmp_serde::to_vec_named` and sets
  `Content-Type: application/msgpack` — i.e. it returns opaque binary, not
  human-readable text. The crate's own integration test
  (`tests/observability_http.rs:172-181`) confirms this by decoding the response
  with `rmp_serde::from_slice` and labels it "msgpack `/info` endpoint" in a
  comment — directly contradicting the doc comment one file over. The same
  "pretty-printed" claim is repeated in `lib.rs`-adjacent config doc
  (`src/config.rs:155` cross-reference) and the crate's `//!` header.
- **Failure scenario**: An operator runs `curl http://127.0.0.1:9090/info` (the
  exact use case the doc comment advertises) expecting readable JSON/text and
  instead gets raw binary msgpack dumped to their terminal — no debugging value,
  and actively confusing since every sibling endpoint (`/healthz`, `/readyz`,
  `/metrics`) returns plain text.
- **Suggested fix**: Either (a) change `info_handler` to actually emit
  human-readable output (JSON via `serde_json` is the documented exception for
  non-query surfaces, or plain `Debug`-formatted text) matching the doc's promise,
  or (b) fix the doc comment in `observability.rs` (and the cross-reference in
  `config.rs`) to say "msgpack-encoded" instead of "pretty-printed... for
  curl-debugging" so it stops promising curl-friendliness it doesn't deliver.

### 2. `server_query_version` wire field is `u8` but the version constant is `u32`

- **File:line**: `crates/shamir-server/src/connection/handshake.rs:151`, `:639`;
  `crates/shamir-server/src/version.rs:57`
- **Severity**: low
- **Issue**: `CURRENT_QUERY_LANG_VERSION` is declared `u32` specifically because,
  per `version.rs`'s own doc comment, "the query-language version is much more
  likely to evolve than the wire-level handshake — easier to bump for a long time
  without overflowing." Both handshake response paths
  (`wire::AuthOk::server_query_version` and `wire::ResumeOkWire::server_query_version`,
  both declared `u8` in `connection/wire.rs`) narrow it via `as u8` when populating
  the handshake reply. The moment `CURRENT_QUERY_LANG_VERSION` crosses 255, this
  cast silently wraps instead of failing to compile or panicking, defeating the
  stated rationale for choosing `u32` in the first place — the request-side
  `query_version: u32` field (`DbRequest::Execute`) has no such ceiling, so the
  advertised max-supported version a client sees over the handshake channel could
  silently disagree with what the request-dispatch version check actually accepts.
- **Failure scenario**: A future bump of `CURRENT_QUERY_LANG_VERSION` past 255
  causes `auth_ok.server_query_version` / `resume_ok.server_query_version` to wrap
  (e.g. 256 -> 0), so clients that gate v2+ behavior on that advertised field
  silently downgrade to v1 behavior (or worse, misinterpret a wrapped low value as
  "server predates negotiation," per the field's own `0` sentinel doc comment on
  `AuthOk::server_query_version` in `connection/wire.rs`), even though the server
  and the request-dispatch path both actually support the newer version.
- **Suggested fix**: Either narrow `SUPPORTED_QUERY_LANG_VERSIONS`/
  `CURRENT_QUERY_LANG_VERSION`'s type to `u8` explicitly (documenting that the
  handshake-advertised ceiling is intentionally `u8`-bounded) or widen
  `server_query_version` on both wire structs to `u16`/`u32` to match the
  request-side field's width, with a fallible conversion (`u8::try_from`, clamped
  with a `tracing::warn!` on overflow) at the population site instead of a silent
  `as u8` truncation.

### 3. `check_destructive_hmacs` re-derives the HMAC key on every destructive-op sweep across `execute` and `tx_execute`

- **File:line**: `crates/shamir-server/src/db_handler/admin.rs:637-653`
- **Severity**: nit
- **Issue**: The lazy-derivation closure (`key(&mut key_opt)`) correctly memoizes
  the session HMAC key within a single `check_destructive_hmacs` call, but both
  `ShamirDbHandler::execute` (`handler.rs:549`) and `tx_execute`
  (`tx_handlers.rs:119`) call this function independently per request, so a
  session issuing repeated small destructive batches re-derives
  `session.hmac_key()` once per request rather than caching it session-wide. Not
  a wire-protocol correctness issue (the derivation is a cheap HKDF, not Argon2id)
  but worth noting since the memoization pattern reads as though it were solving a
  cross-call cost, when it only dedupes within one call.
- **Failure scenario**: None observed — purely a minor efficiency/readability note,
  not a functional defect.
- **Suggested fix**: No action required; documenting only in case a future reviewer
  assumes `key_opt`'s lazy pattern already amortizes cost across requests.

</details>
