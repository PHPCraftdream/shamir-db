# TOKEN-T5a - server side of online API-token administration

Status: implemented, read-through reviewed twice as compiler/clippy/layout (no build was run locally by policy). CI is the first compile.

## What exists

- Wire: four inline `DbRequest` variants (`CreateApiToken`, `ListApiTokens`, `RevokeApiToken`, `RevokeUserTokens`) and responses (`ApiTokenCreated`, `ApiTokensListed`, `ApiTokenRevoked`, `ApiUserTokensRevoked`) in `shamir-query-types` (`wire/db_message.rs`); typed `ApiTokenMode`, `ApiTokenChannels`, `ApiTokenView` in `security/auth/token_admin/` (re-exported through `wire`). The secret is carried as `SecretString` and appears once, in the creation response.
- HMAC: canonical encoding for the four operations, op tags 0x20..=0x23 (`security/hmac.rs`); shared vectors extended to 35 (`test-vectors/admin_hmac_v2.json`, Python reference, Rust, TS and e2e consumers).
- Handler: `engine/db_handler/handlers/token_admin/` over the shared `TokenService` injected at launch. Self-service without HMAC; `for_user`, foreign list/revoke/revoke-user need superuser plus HMAC; browser-bound sessions cannot create; token sessions are denied for all four; read-only replicas refuse create/revoke but allow list; post-creation race re-check revokes the fresh token on a stale owner floor.
- Service: `TokenService::lookup`, `now_ns`, `*_with_context` variants and `TokenAuditContext` (actor username, transport label, 8-byte session-id prefix). The offline CLI passes transport `cli` and an empty actor. Audit events keep the names `api_token_created`, `api_token_revoked`, `api_token_user_revoked`; details carry ids and owner only.
- Spec: TOKEN_API.md section 12 (field names, HMAC canonical encoding, wire tags, error codes, audit context).
- Tests: wire round trips and golden bytes, HMAC canonical-encoding tests, handler regressions in `tests/handlers/token_admin/` (self service, delegation, session policy, race, audit).

## Deviations and notes

- Request and response variants are inline; no standalone wire structs. Both enums are internally tagged, so wrapping a variant in a struct later keeps the msgpack encoding. T5b recommendation: wrap the created-token payload in its own struct so the Rust client returns a typed value.
- Folder slots: `wire/token_admin` moved under `security/auth/`; `TokenGenerator` moved into `service/options.rs` and `generator.rs` removed to keep `security/tokens/service/` at 7 entries; the `handlers/token_admin.rs` facade was dropped.
- Hooks for rule 4 (password change, role change, user deletion) are the T2b owner floors; read and verified, not redesigned.

## Needs CI

Compilation, clippy `-D warnings`, rust-guard, the handler and HMAC tests, `ts-unit`, and the e2e HMAC vectors test.
