# TOKEN-JS5 implementation progress

## Deliverable 1 — token client guide: PARTIAL (initial), DONE (remediation)

The first-pass 135-line overview was incomplete and had an overbroad claim that all Node SCRAM token creation was denied. Remediation replaced it with a detailed reference including the real Node `connectLocal` issuance path, exact CLI forms, HTTP/WS capability and error references, browser and Node token/login examples, platform-specific behavior, and security/operator caveats. No test/build/runtime claim is made.

### Source matrix (checked)

| Subject | Source evidence | Documentation consequence |
|---|---|---|
| Public token connect and exports | `crates/shamir-client-ts/src/index.ts:22-35,56-64`; `src/browser.ts:22-35,37-44` | Node and browser accept explicit token auth and export token types. |
| Connection fields/platform constraints | `crates/shamir-client-ts/src/core/types/connection.ts:4-26` | URL, token, optional channel/timeouts/signal; no SCRAM credential mix; browser has no Node TLS/origin knobs. |
| Opaque/redacting credential | `crates/shamir-client-ts/src/entry/api-token.ts:21-44` | `ApiToken` redacts and exposes no public reveal method. |
| HTTP supported operations and policy errors | `crates/shamir-client-ts/src/entry/token-client.ts:14-30,82-108` | HTTP Execute/visibility/status/ping; interactive Tx and cursor return unsupported; server policy remains authoritative. |
| API token metadata and methods | `crates/shamir-client-ts/src/core/types/access/token-admin.ts:6-68`; `src/core/client/client.ts:970` | Admin methods are typed; created token is one-shot result; timestamps bigint and snake_case. |
| Login options/results | `crates/shamir-client-ts/src/login/types.ts:8-57` | expectedKdf shape, listenerId bytes, Node CA, forbidden ttl, independent result.connect signal, camelCase metadata. |
| Login error vocabulary | `crates/shamir-client-ts/src/login/errors.ts:1-11` | Includes signature mismatch, unsupported server, retry timing, and issuance errors. |
| Existing usage evidence | `crates/shamir-client-ts/src/entry/__tests__/`, `src/entry/ws/__tests__/`, `src/login/__tests__/`, `src/core/client/__tests__/api-token-admin.test.ts` | These are existing source usage/test locations. No execution was performed. |
| Requested e2e location | `crates/shamir-client-ts/src/__tests__/connection/token/` absent in current tree | Not cited as usage evidence; guide examples are checked against present API declarations and source tests instead. |
| Operator guide | `docs/guide-docs/guide/07-operations.md` | Existing operational guide is the target for short listener guidance. |

### Verified caveats and unresolved code/spec drift

- Transport/client evidence: `crates/shamir-client-ts/src/index.ts:22-45,56-64`; `src/browser.ts:22-44`; `src/core/types/connection.ts:4-26`; `src/platform/connect/node-connect.ts:18-54`; `src/platform/connect/browser-connect.ts:11-45`; `src/entry/api-token.ts:21-44`.
- Token admin evidence: `src/core/client/client.ts:969-972`; `src/core/client/client-internals.ts:32-74`; `src/core/types/access/token-admin.ts:6-68`; `src/core/support/errors.ts:16-58`; local IPC handshake mode `src/core/client/client.ts:207-249`.
- HTTP/WS behavior: `src/entry/token-client.ts:14-30,82-108`; `src/entry/token-db.ts:37-61`; `src/entry/ws/token-client.ts:8-19`; `src/entry/ws/token-lifecycle.ts:34-61,123-128`; `src/core/protocol/token/options.ts:6-7,26-40,60-72`; `src/core/protocol/token/errors.ts:1-52`; `src/core/protocol/token/http.ts:17-34,58-67`; `src/core/protocol/token/ws.ts:11-25`.
- Login evidence: `src/login/types.ts:8-57`; `src/login/errors.ts:1-11`; `src/login/core.ts:68-118,200-235,256-280,325-340`; `src/login/protocol.ts:9-10,76-100,128-168`; Node CA verification `src/login/node/node-http.ts:75-84`; browser same-origin fetch/no credentials `src/login/browser/browser-http.ts:85-95`.
- Code constants and example update: `crates/shamir-client-ts/src/core/protocol/scram.ts:22-33`; `src/core/client/client.ts:207-249`; `src/core/builders/read/filter.ts:25-35`; `src/core/builders/write/batch.ts:85-109,328-362,375-382`; `deploy/server.example.ktav:42-50`.
- CLI exact args/output: `crates/shamir-server/src/security/tokens/cli/args.rs:4-44`; TTL/channel/default and stopped-server lock `cli/create.rs:25-63,116-136,175-180`; list/revoke `cli/list.rs:6-14`, `cli/run.rs:4-18`; secret output `cli/secret_output.rs:11-46`.
- Server listener evidence: `crates/shamir-server/src/boot/config/http_listener.rs:15-54`; `deploy/server.example.ktav:101-134`; token HTTP server defaults/body cap in `crates/shamir-server/src/boot/config/tests/http_listener_tests.rs:30-75`; protocol policy and close mapping also in `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md`.
- Existing usage suites: `crates/shamir-client-ts/src/entry/__tests__/`, `src/entry/ws/__tests__/`, `src/login/__tests__/`, `src/core/client/__tests__/api-token-admin.test.ts`, `src/__tests__/connection/e2e-connect-local.test.ts`. The brief-listed `src/__tests__/connection/token/` does not exist.

### Remediation findings and caveats

- `TOKEN_API.md` and `TOKEN_LOGIN_HTTP.md` still contain stale “not implemented” status text although corresponding client/server code exists. The guide follows current implementation.
- Node IPC `connectLocal` is verified `TRANSPORT_KIND_UNIX` (0x03), `BINDING_MODE_NONE` (0x00); WS uses `TRANSPORT_KIND_WS` (0x02), browser binding `BINDING_MODE_TLS_NO_EXPORT` (0x02). Local path omitted in examples; OS permissions/DACL are transport boundary.
- CLI options confirmed in `cli/args.rs`: HTTP channel is `http`, default read-only, `--read-write`, repeat `--db`, TTL suffixes, `--out-file`; CLI requires stopped server lock even for list, and Windows ACLs are not tightened.
- HTTP Execute batches can be transactional; interactive transactions are WS-only. Browser token WS `fail()` preserves local input/abort/timeout codes but wraps other pre-binding failures as `authentication_failed` (`crates/shamir-client-ts/src/entry/ws/token-lifecycle.ts:34-50`); close mapping is in `:123-128`. After binding, `TokenWsClientImpl.expiresAtNs` comes from server binding (`src/entry/ws/token-ws-client.ts:65-68`).
- Login URL/security, scope constraints, defaults, timeout distinctions, partial PRECIS behavior, signature-verification boundary, and declared-vs-emitted error mapping are documented from source.
- Detailed review references now also include CLI, code constants, login core/protocol, token WS lifecycle/close mapping, local IPC tests, and server config. The source matrix above is representative rather than an exhaustive line-by-line checklist.
## Deliverable 2 — operator pointer: PARTIAL (initial), DONE (remediation)

Updated the existing operator guide section with concrete origin-root example assumptions, TLS/direct-listener configuration, HTTP cap/timeouts, login identity policy, and relevant token/login defaults. T8's full operator manual remains intentionally deferred.

## Deliverable 3 — source-backed audit progress: DONE

This is an evidence/progress record, not the deliverable itself. Deliverable 3 is builder-only, source-checked client usage examples in the token guide. Source coverage and the genuinely absent requested e2e directory are recorded above. No runtime tests/builds/CI were run.

## Validation and remaining work

No commands, builds, tests, or local validation were run. The initial partial corrections were reviewed against current sources; no runtime proof is claimed. The specified `src/__tests__/connection/token/` directory is absent; existing usage/test locations were used instead. No claims of complete validation or CI completion are made.

Progress: Source-review corrections completed: listener config nesting, IPC issuance distinction, read-only commit policy, close-code table, login CA/deadline/error mapping; no runtime checks by worker. Deliverables 1–3 source-reviewed; layout gate passed by orchestrator; runtime validation intentionally not performed.

## Final report

Implemented TOKEN-JS5 documentation:
- DONE — `crates/shamir-client-ts/docs/TOKENS.md`: token CLI/online administration, Node/browser HTTP and WebSocket connections, scope/capabilities, password login, error/close-code references, and security guidance.
- DONE — `crates/shamir-client-ts/docs/README.md`: linked the token guide.
- DONE — `docs/guide-docs/guide/07-operations.md`: added concrete listener settings and client-guide cross-reference.
- DONE — `docs/dev-artifacts/audits/2026-11-02-token-js5/DESIGN.md`: recorded progress, source evidence, caveats, and this report.
Validation: `python -I scripts/ci/check_layout.py` passed. No compilation, tests, or CI runs were performed; examples were source-reviewed, not executed.
Verification limits: the brief's token e2e directory is absent, so existing client unit/usage suites and implementation were used instead. Runtime interoperability, deployment TLS/proxy/CSP behavior, and sample execution remain unverified. Stale protocol-spec implementation banners remain unchanged; the full T8 operator manual is outside this task.
No source, manifests, workflows, or lockfiles changed. No commits, pushes, or destructive git commands were run.

