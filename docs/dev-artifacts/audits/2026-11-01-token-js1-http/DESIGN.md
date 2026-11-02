# TOKEN-JS1 token HTTP transport audit — 2026-11-01

## Scope and decisions

This revision covers the HTTP-only token transport and its Node/browser adapters. WebSocket, login, end-to-end work, dependency changes, and public connect/package-export changes are out of scope; public connect overloads and package exports remain unchanged for JS3.

`normalizeTokenDdlState` is the sole new export in this revision. The shared normalizer accepts Rust externally tagged unit strings and one-key maps, along with compatible legacy `kind` objects. It returns the existing `DdlOpState` tagged shape. Timestamp validation accepts nonnegative safe integers (including bigint only when exactly representable); malformed states and unsafe timestamps become sanitized protocol errors. Both batch DDL states and `getDdlOpStatus` use this normalization.

Structured visibility timeouts retain the existing `VisibilityError`, with sanitized static detail, preserved code, and bigint positions. Malformed visibility metadata is handled as a protocol error. Transaction and receipt optional `reason` and `error` fields are validated as strings when present. The legacy binary/bigint wire-value cast remains for compatibility and has an explicit short comment; there is no `LegacyBatchResponse` type alias.

Item 4 assertions cover absent Retry-After as `undefined`, nested wire-batch interner-field exclusion and builder comparison, and an exact normalized `10n` wait. Visibility wait input is not mutated. These assertions are in `crates/shamir-client-ts/src/core/protocol/token/__tests__/http.test.ts:117`, `crates/shamir-client-ts/src/platform/token/__tests__/http-node.test.ts:69`, and `crates/shamir-client-ts/src/entry/__tests__/token-client.test.ts:44`.

A JS0 deviation is documented: peer-provided error text is sanitized, arbitrary error codes use the existing identifier guard, legacy typed DDL timestamps are normalized safely, and the browser connection budget measures fetch headers rather than TCP connection establishment.

## Revision 1 status

Items 1–4 are DONE based on source inspection and authored tests; no test execution is claimed. Item 5 is DONE. Item 6 is DONE based on lint-aware source read-through of all 20 token source/test files and both modified type files (22 files total). This is source inspection, not compiler or runtime verification. The deadline is a bound only; no claim is made that the last 15 minutes elapsed.

The token source/test read-through covered these files:

- `crates/shamir-client-ts/src/core/protocol/token/auth.ts`
- `crates/shamir-client-ts/src/core/protocol/token/errors.ts`
- `crates/shamir-client-ts/src/core/protocol/token/http.ts`
- `crates/shamir-client-ts/src/core/protocol/token/options.ts`
- `crates/shamir-client-ts/src/core/protocol/token/transport.ts`
- `crates/shamir-client-ts/src/core/protocol/token/__tests__/http.test.ts`
- `crates/shamir-client-ts/src/entry/api-token.ts`
- `crates/shamir-client-ts/src/entry/token-client.ts`
- `crates/shamir-client-ts/src/entry/token-db.ts`
- `crates/shamir-client-ts/src/entry/token-ddl-state.ts`
- `crates/shamir-client-ts/src/entry/token-response.ts`
- `crates/shamir-client-ts/src/entry/__tests__/api-token.test.ts`
- `crates/shamir-client-ts/src/entry/__tests__/token-client.test.ts`
- `crates/shamir-client-ts/src/entry/__tests__/token-response.test.ts`
- `crates/shamir-client-ts/src/platform/token/types.ts`
- `crates/shamir-client-ts/src/platform/token/http-browser.ts`
- `crates/shamir-client-ts/src/platform/token/http-node.ts`
- `crates/shamir-client-ts/src/platform/token/__tests__/http-browser.test.ts`
- `crates/shamir-client-ts/src/platform/token/__tests__/http-node.test.ts`
- `crates/shamir-client-ts/src/platform/token/__tests__/http-node-tls.test.ts`

The modified type files read through were `crates/shamir-client-ts/src/core/types/connection.ts` and `crates/shamir-client-ts/src/core/types/write/batch.ts`. No local builds, typechecks, or tests were run. CI verification is pending: strict TypeScript typecheck and Vitest across all seven test files are required. Both permitted layout-check runs returned OK; the checker ignores the untracked helper: `python -I scripts/ci/check_layout.py`. Inspected folder/file budgets were protocol 6, entry 6, platform 4, and test directories 1/3/3; all are within the limit of 7.

## Final report

Revision 1 items 1–6 are DONE by source inspection; CI verification remains pending.
1. DONE — structured visibility timeouts preserve sanitized `VisibilityError` code and bigint positions (`crates/shamir-client-ts/src/core/protocol/token/http.ts:17`).
2. DONE — batch DDL states and `getDdlOpStatus` share `normalizeTokenDdlState` (`crates/shamir-client-ts/src/entry/token-ddl-state.ts:18`).
3. DONE — transaction/receipt text fields are validated; the legacy binary/bigint cast is explicitly documented (`crates/shamir-client-ts/src/entry/token-response.ts:21`).
4. DONE — Retry-After absence, nested interner-field exclusion, and exact normalized waits are asserted in the protocol, Node, and client tests.
5. DONE — this audit note is consistent and contains this final report.
6. DONE — all 20 token source/test files and both modified type files received a lint-aware read-through; folder/file budgets were inspected.
Added tests: “surfaces sanitized typed visibility timeouts from execute and REST visibility waits losslessly”; “rejects malformed visibility timeout metadata as a protocol error”; “normalizes Rust externally tagged DDL states in batch results”; “rejects malformed externally tagged DDL variants”; “rejects non-string transaction reason and error fields on transactions and receipts”; “preserves valid transaction reason and error strings and legacy nested wire values”; “preserves structured visibility errors through execute and visibility waits”.
Validation: `python -I scripts/ci/check_layout.py` passed. No local builds, typechecks, or tests were run. CI must typecheck and run Vitest; public connect/package exports remain for JS3.