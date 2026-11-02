# TOKEN-T5a revision 1 audit note

## Final report — PARTIAL, not CI-ready (source-only; read-through complete)

This is a final source/audit report, not a build or test result. The orchestrator re-read every changed file. CI compilation, clippy, and tests remain unverified and were not run. The outstanding service layout deviation prevents calling this CI-ready.

### Final status

Path shorthand: service/* = crates/shamir-server/src/security/tokens/service/*; CLI *.rs = crates/shamir-server/src/security/tokens/cli/*.rs; handler token_admin/* = crates/shamir-server/src/engine/db_handler/handlers/token_admin/*; handler test filenames = crates/shamir-server/src/engine/db_handler/tests/handlers/token_admin/*; TOKEN_API.md = docs/guide-docs/client-server-protocol-spec/TOKEN_API.md; wire/db_message.rs = crates/shamir-query-types/src/wire/db_message.rs; reference.py = crates/shamir-query-types/test-vectors/admin_hmac_v2_reference.py; security/tests/admin_hmac_v2_vectors.rs = crates/shamir-query-types/src/security/tests/admin_hmac_v2_vectors.rs.

Overall PARTIAL: implementation and source read-through complete; layout remains unresolved and CI compilation/clippy/tests are unrun.

1. PARTIAL — audit actor context implemented in crates/shamir-server/src/security/tokens/service/audit_context.rs:3, service/token_service.rs:160/305/436, handler token_admin/audit_context.rs:5 and CLI create.rs:46/revoke.rs:51; event names and secret-free details preserved. Service folder has8entries; new audit_context.rs preserved under no-delete/no-move rule. Requires operator permission to relocate/remove an entry.
2. DONE (source) — TokenService::now_ns at service/token_service.rs:120; handler service-clock revoke/rollback, self_service.rs:55 and race.rs:40 timestamp assertions.
3. DONE (source) —35vectors including0020..0023 in crates/shamir-query-types/test-vectors/admin_hmac_v2.json:253, reference.py:75, security/tests/admin_hmac_v2_vectors.rs:305; existing31unchanged by gitdiff. TS/e2e consumers adapted; T5b encoder remains intentionally outofscope.
4. DONE (source) — TOKEN_API.md:236–243 and wire/db_message.rs:445; only spec section12changed.
5. DONE (source) — delegation.rs:29/48/73/97/108/184, audit.rs:106, CLI log audit checks include revoke-user/revoke-all; test names listed below plus service_audit_context_defaults_to_token_transport. All tests UNRUN.
6. DONE — this short accurate final note replaces stale reports; inline wire variants/no standalone structs retained, T5b recommendation wrap created payload; no race session-store kicks.

### Checkpoints

1. **PARTIAL (layout).** Context-aware token auditing is implemented: public `TokenAuditContext` carries actor username, transport, and an eight-byte session prefix. Existing `create`/`revoke`/`revoke_user` wrappers retain token-default behavior; new `*_with_context` methods are used by handler and CLI. Offline CLI uses an empty actor (no authenticated operator), transport `cli`, and zero session ID. Handler TCP/WS/Unix contexts use the first eight session-ID bytes; subnet is empty per spec §12. Service has eight entries, one above the seven-entry limit due to `service/audit_context.rs`. Preserve the file: resolving requires operator relocation/removal permission.
2. **DONE (source).** Shared `now_ns` accessor is used by handler revoke, revoke-user, and race rollback paths; fixed-`NOW` assertions cover timestamp behavior.
3. **DONE (source).** The shared fixture/reference and Rust coverage provide 35 contiguous vectors, tags 1–35, including UTF-8 and u64 values above 2^53. Four additional vectors were independently derived with Python stdlib and their hex manually reassembled. TypeScript/e2e fixtures are compatibility-only; T5b encoder/builders are not implemented.
4. **DONE (source).** API/wire documentation and canonical-encoding changes are recorded in the source checkpoint history below; tests remain unrun.
5. **DONE (source).** New tests: `superuser_self_revoke_needs_no_hmac_and_invalidates_bearer`; `superuser_self_user_revoke_needs_no_hmac_and_invalidates_bearer`; `superuser_delegated_create_for_unknown_user_returns_not_found`; `superuser_delegated_list_for_unknown_user_returns_not_found`; `revoked_label_can_be_reused_with_new_id_and_bearer`; `audit_records_delegated_actor_transport_and_eight_byte_session_prefix`. Existing tests were strengthened for the context-default contract, explicit service audit, actual CLI log output, HMAC rejection with persisted count zero, and self-service/race `revoked_at = NOW >= created`. The default context is also emitted and audited by `retained_non_live_record_is_revocable` in crates/shamir-server/src/security/tokens/tests/service_tests.rs:660. No session-store kicks were added to race rollback. All tests are unrun.
6. **DONE (note).** This final note replaces the stale audit note; source read-through is complete.

### Wire deviation

Four request/response variants exist inline; standalone wire structs were not added. Recommendation for T5b: wrap the created-token payload in a struct. This is a known deviation, not an implementation claim.

### Verification and limits

Checks OBSERVED: `cargo fmt -p shamir-server -p shamir-query-types -- --check` passed. Final gate at 17:47 passed fmt+fmtcheck+trackedlayout+diffcheck. `python -I scripts/ci/check_layout.py` passed for tracked files only; manual service folder count of 8 violates the 7-entry limit. Do not interpret the layout checker result as clearing that deviation. Manual service8 remains failure. No compilation, clippy, tests, rustc, or npm were run; CI compilation/clippy/tests are unverified. No Cargo.toml/lock edits, git mutations, file moves/deletions. All edit/review children awaited before final response.

## Preserved revision checkpoint history

Item 3 DONE (source only): shared fixture/reference/Rust test cover35 contiguous tags1..35 including0x20..23; four new vectors independently derived with Python stdlib and manually assembled byte hex; TS/e2e fixture consumers adjusted without implementing T5b; tests UNRUN.

Item 4 DONE (source only): TOKEN_API.md section12 Russian canonical encoding, wire op/kind/error/audit documentation and restored rule2; db_message error-code docs expanded; source reread, tests UNRUN.

## Prior baseline checkpoints (superseded by revision 1)

The prior note's initial and 16:40 reports documented earlier incomplete review, test-source defects/unmounted race tests, and missing context. Those are historical observations, not current revision 1 status. No earlier test/build claim is carried forward as a passing result.
