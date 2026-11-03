# TOKEN-T5b-TS design and progress

## Design

The client surface is `client.apiTokens` with create/list/revoke/revokeUser operations and four signed builders: `createApiToken`, `listApiTokens`, `revokeApiToken`, and `revokeUserTokens`. Canonical tags are 0x20–0x23; absent/null optional values encode None, while empty database lists encode Some([]). Builders omit absent request options, sign operations, copy database lists, and preserve raw usernames. Browser-bound creation is server-refused. Public token-admin types, exactly: `ApiTokenMode`, `ApiTokenChannels`, `CreateApiTokenSpec`, `CreateApiTokenOp`, `ListApiTokensOp`, `RevokeApiTokenOp`, `RevokeUserTokensOp`, `CreatedApiToken`, `ApiTokenInfo`, `ApiTokenAdmin`. Error exports: `TokenAdminErrorCode`, `TOKEN_ADMIN_ERROR_CODES`, `isTokenAdminError`. The AUTH2 reference `docs/dev-artifacts/prompts/remediation/p34/auth2-login-ts.md` is missing on this branch; its relevant traps are recorded in the remediation brief.

## Progress

- DONE 1 — canonical encoder supports token operations; shared fixture coverage represents all 35 vectors.
- DONE 2 — four token-admin builders, fixture-backed signing, omission, raw-username, database-copy and validation coverage.
- DONE 3 — public `client.apiTokens` API and FakeSocket request/response tests.
- DONE 4 — token-admin error code set, predicate, and runtime/type exports.
- DONE 5 — API-token result redaction and secret-free error coverage; server messages remain handled by existing `ShamirDbError`, with secret-free fixture expectations relying on the normative server no-secret contract, not a new sanitizer.
- DONE 6 — malformed creation response validation and fixed secret-free error coverage.
- DONE 7 — static review complete: `client.ts` 993 lines; `client-internals.ts` 737; no entry/platform/login or dependency changes. The orchestrator observed layout check OK (0 baselined, 0 hints), and inspected the untracked-entry counts: admin 5, admin tests 2, access types 2, client tests 7. `getDdlOpStatus` behavior was preserved through extraction to internals.

## Final report

All eight acceptance items are DONE for implementation and static/test coverage only; CI execution is unverified.

| # | Status | Evidence |
|---|---|---|
| 1. Encoder / 35 vectors | DONE | `crates/shamir-client-ts/src/core/protocol/hmac.ts:66`; `crates/shamir-client-ts/src/core/protocol/__tests__/hmac.test.ts:83,110` — `rv_query_types_c7aec31f_matches_known_answer_vectors_for_every_tag`; absent-vs-empty and unknown-operation tests. |
| 2. Builders | DONE | `crates/shamir-client-ts/src/core/builders/admin/token-admin.ts:34`; `crates/shamir-client-ts/src/core/builders/admin/__tests__/token-admin.test.ts:56,88,129,148` — vector signing, omissions, raw username, empty databases and validation-before-signing. |
| 3. Client FakeSocket API | DONE | `crates/shamir-client-ts/src/core/client/client.ts:970`; `crates/shamir-client-ts/src/core/client/client-internals.ts:32`; `crates/shamir-client-ts/src/core/client/__tests__/api-token-admin.test.ts:133,195,248,268` — create/list/revoke/revokeUser, signed wire requests, normalized typed results. |
| 4. Errors | DONE | `crates/shamir-client-ts/src/core/support/errors.ts:16,34,54`; `crates/shamir-client-ts/src/core/client/__tests__/api-token-admin.test.ts:345` — error-code set, predicate, and server-error mapping. |
| 5. Redaction | DONE | `crates/shamir-client-ts/src/core/client/__tests__/api-token-admin.test.ts:189,345,381,414` — token serialization/inspection and secret-free error assertions. |
| 6. Malformed creation replies | DONE | `crates/shamir-client-ts/src/core/client/__tests__/api-token-admin.test.ts:414`; `crates/shamir-client-ts/src/core/client/client-internals.ts:130` — malformed field cases and fixed validation errors. |
| 7. Line limit, DDL, untouched scope | DONE | `crates/shamir-client-ts/src/core/client/client.ts:993`; `crates/shamir-client-ts/src/core/client/client-internals.ts:151,737`; client test `:618`; static review confirms 993-line client, preserved DDL behavior, and no entry/platform/login or dependency modifications. |
| 8. Entries and e2e | DONE | `crates/shamir-client-ts/src/index.ts:78`; `crates/shamir-client-ts/src/browser.ts:60`; `crates/shamir-client-ts/src/core/types/index.ts:111`; `crates/shamir-client-ts/src/core/builders/admin/index.ts:1`; `tests/e2e/tests/20-hmac-vectors.test.js:40` — exports, reviewed entry counts, and vector assertion. |

Client FakeSocket test file is 645 lines; builder test file is 169 lines; `hmac.ts` is 77 lines and `hmac.test.ts` is 189 lines. The orchestrator ran the permitted layout check and read-only Git inspection; child agents used no command tools. No local builds or tests were run. CI must verify strict TypeScript NodeNext/declaration compilation, Vitest 1.6 (including all 35 vectors), and e2e execution.

## User-facing report

Implemented the token-admin client surface, builders, canonical HMAC support, error helpers and regression tests. Changed TS protocol/builders/types/client/errors/barrels, the e2e vector test, and DESIGN.md. Validation: layout check passed; all touched files statically reviewed; client.ts is 993 lines. No local builds or tests run; strict TypeScript, Vitest and e2e execution remain for CI. No dependency, token-client, platform or login changes; no git mutations.