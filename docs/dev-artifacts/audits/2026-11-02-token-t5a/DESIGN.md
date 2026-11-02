# TOKEN-T5a audit

## Final report — PARTIAL (review in progress, through 16:30)

1. **PARTIAL — wire/API.** Corrected the four inline wire variants using typed `ApiTokenMode`, `ApiTokenChannels`, `ApiTokenView`, and `SecretString` response handling; canonical HMAC tags are `0x20..=0x23`; §12 is corrected. Standalone request/response structs were not introduced. Cross-workspace references were source-inspected only; no compiler verification. New public surface includes `TokenService::lookup` (`crates/shamir-server/src/security/tokens/service/token_service.rs:475`), `ShamirDbHandler::with_token_service` (`crates/shamir-server/src/engine/db_handler/handler.rs:275`), wire enums/views, and new `DbRequest`/`DbResponse` variants.
2. **PARTIAL — handler/service.** Source review found shared-`Arc` service boot/handler injection, self/foreign-token HMAC authorization, browser/token/read-only dispatch, and rollback/finalization paths. Audit and counters reuse existing batched service events; no channel/subnet context was added. Source inspection only; no compiler/runtime validation.
3. **DONE (source review only) — hooks.** Existing integration hooks were read and verified; no redesign made. This is not a test-pass claim.
4. **PARTIAL — tests.** Wire golden roundtrips, manual HMAC vectors, negative controls, and handler test files exist, including `self_service.rs`, `delegation.rs`, `session_policy.rs`, `race.rs`, and `audit.rs` under `crates/shamir-server/src/engine/db_handler/tests/handlers/token_admin/`. They remain unrun and under review. Do not claim compilation or test success.

## Restored source-evidence notes (appended 16:23)

- Acceptance 3 — user-directory and restore hooks. `FjallUserDirectory::remove` removes the user, user-id and principal indexes and synchronously persists; it also evicts the user's tickets-cache entry (`crates/shamir-server/src/security/user_directory/directory.rs:449-475`, cache eviction at `:462-469`). `set_superuser` and `set_replicator` update the ticket invalidation floor on privilege changes (`directory.rs:515-519`, `:563-567`); `grant_role`/`revoke_role` likewise bump the floor on a real change (`directory.rs:705-734`). Restore invalidates tickets in the staged copy before swap (`crates/shamir-server/src/sync/durability/restore.rs:210-227`) by opening its users directory and calling `invalidate_all_tickets` (`restore.rs:394-399`; directory implementation `directory.rs:754-785`). Source evidence only.
- Acceptance 1 — wire/security. Source inspection previously captured `db_message.rs:244`, new response variants around `:400`, and canonical HMAC handling around `:680`; this is not compiler verification. These are carried-forward source observations, not re-read in this pass.
- Acceptance 2 — handler/service. Handler has an optional shared `TokenService` field (`crates/shamir-server/src/engine/db_handler/handler.rs:215-217`) and builder injection (`:275-282`). Launch-time service construction/injection was previously captured at `server_launcher/launch.rs:440` and nearby; not re-read in this pass. Handler create logic is in `handlers/token_admin/logic.rs:62-141`; post-issuance recheck/finalization and rollback are at `:143-200`. Service audit events/counter updates reuse existing create/revoke/revoke-user paths in `security/tokens/service/token_service.rs`; source locations previously noted around `:450` and elsewhere, not re-read in this pass.
- Exact handler test-file names confirmed by current file listing: `self_service.rs`, `delegation.rs`, `session_policy.rs`, `race.rs`, `audit.rs` (under `crates/shamir-server/src/engine/db_handler/tests/handlers/token_admin/`). They are source files only; no tests were run. Test function names were not captured in this pass.
- Review remains **PARTIAL**. No claim that the 16:30 review is complete; no test/build commands were run.

- Observed: scoped `cargo fmt` for query-types/server completed successfully at 16:17.
- Observed: layout checker tracked-file checks pass. The count of eight entries in `service` versus limit seven was determined by manual `ls`; the checker itself ignored untracked entries and reported OK. `token_lookup` is among the entries. It cannot be moved/deleted under current user instruction. Orchestrator relocation/removal permission is needed. No deletion was performed.
- No `cargo check`, clippy, build, test, rustc, or npm command was run by the top-level agent. A child-reported cargo-check failure is unverified and is not treated as a receipt.
- A test worker remains outstanding; its unavailable child ID prevents stopping it via the available tool interface. Its result is not counted as verification.

**Disposition: PARTIAL, not fully complete.** Review is still in progress; compile/test and layout-gate closure remain outstanding.

## Final report — PARTIAL, NOT CI-ready (16:40 review-only)

- Production source review: four operations, HMAC, shared service, and floor rollback are present. Item 3 hooks are **DONE (source review only)**.
- Observed final source at 16:38: `manifest` tests in `tests/handlers/token_admin/mod.rs` lack `mod race;`, leaving three race tests unreachable. `delegation.rs:144-147` still uses `assert_eq!` on `DbResponse`, which lacks `PartialEq`—a source compile defect. Do not claim tests fixed or passing.
- The original test worker continues to overwrite fixes and cannot be stopped because its child ID parameter is unavailable. Full read-through is incomplete due to this changing worker.
- Layout: `service` has eight entries versus the seven-entry limit. Operator relocation/removal permission is required; no file was moved or deleted.
- Standalone request/response wire structs requirement remains unmet; variants are inline. No audit transport/subnet context was added; service event reuse is present.
- All tests are **UNRUN** under the explicit user compilation ban. Top-level permitted checks were only scoped `cargo fmt` for query-types/server, Python layout check, and read-only git. `cargo fmt` completed at 16:38; tracked-file layout check reported OK but ignores untracked files. No build, check, clippy, or test verification.
- Source-copy test names observed:
  - Wire: `api_token_request_wire_roundtrips_and_missing_options_are_none`, `api_token_response_secret_is_once_and_list_is_secret_free`, `api_token_admin_wire_requests_and_responses_roundtrip`.
  - HMAC: `token_admin_canonical_encoding_binds_optional_values_and_ignores_hmac`.
  - Handler: `self_create_defaults_once_lists_no_secret_and_revoke_invalidates_bearer`, `self_create_enforces_labels_ttl_default_maximum_and_per_user_limit`, `disabled_self_service_only_blocks_creation`, `browser_binding_blocks_creation_but_not_read_or_revoke`, `non_superuser_cannot_create_or_list_for_another_user`, `superuser_delegated_create_requires_valid_confirmation_hmac`, `superuser_delegated_list_requires_confirmation_hmac`, `foreign_revoke_hides_existence_from_non_superuser_and_requires_admin_hmac`, `foreign_revoke_user_requires_superuser_and_hmac`, `token_origin_is_denied_for_all_four_administration_requests`, `foreign_token_revoke_by_non_superuser_is_token_not_found`, `self_revoke_user_with_explicit_and_omitted_owner_needs_no_hmac`, `read_only_node_rejects_mutations_but_allows_list`, `token_mutations_emit_secret_free_audit_events_and_correct_counters`.
  - Race tests exist in source but are currently unmounted: `target_floor_advanced_after_creation_invalidates_and_revokes`, `caller_floor_advanced_invalidates_delegated_creation`, `changed_target_roles_invalidate_delegated_creation`.
- Crypto source observation: direct `verify_tag_hex` call reuses `session.hmac_key`; `hmac` 0.12.1 `Mac::verify` provides constant-time verification. No custom crypto or unsafe code observed.
- Final source groups: query-types security/wire; server handler, boot, token lookup, and tests; `TOKEN_API` §12; this audit note.
- No commit, push, git mutation, Cargo manifest edit, file move, or deletion was performed.
- Next minimal step: stop the outstanding worker; repair the source compile and test-discovery defects and resolve layout; then run CI compile, clippy, and tests when authorized.
