# TOKEN-T2b server revision 2 notes

## Prior-stage context

Items A, D and E were completed in the preceding stage: persisted zeroizing token key, optional cached user ticket floor, and validated `TokensConfig`. Revision 2 addresses the server token store/service gaps. Local compilation, tests and Clippy are prohibited; CI must verify them.

## Revision 2 decisions and evidence

### Item 1 — audit path and redaction

`TokenService` calls `AuditChainWriter::append`, which advances the shared chain and invokes `AuditAppender::append_entry` (`/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-connect/src/server/infra/audit_chain.rs:491-529`). The production launcher constructs the shared writer/appender (`/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/engine/server/server_launcher/launch.rs:312-321`). `FjallAuditAppender` supports batched operation (`/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/security/observability/logging/audit_appender.rs:13-20`). Token events already use the standard path; no extra queue or production-path change was warranted.

`token_audit_events_include_ids_without_secret_or_mac` checks event names, exact payload shapes `([u8; 16], [u8; 16])` / `([u8; 16], Vec<[u8; 16]>)`, both creation payloads and owner/list IDs. It parses both bearer strings and checks both raw secrets, both complete bearer strings and both MACs against every event-details payload. It captures appender entries and compares their sequence, name and details with the chain snapshot. Chain HMAC links are not event details and are excluded from the payload-redaction assertions.

### Item 2 — durable write-failure seam

`TokenStore` owns a clone-shared `AtomicBool`, initialized disabled. A test-only setter arms the next durable mutation; normal write logic checks/consumes it under the existing operation lock. `TokenStoreError::InjectedWriteFailure` propagates through record writes/deletes, user-revocation batches and GC batches before commit/publication. `durable_write_failures_preserve_create_revoke_and_batch_state` checks failures, unchanged mirror/counters/canceller and label/slot reuse with `max_per_user = 1`. `injected_write_failure_is_consumed_by_gc_batch_before_publication` checks the GC error and retained durable row.

### Item 3 — service regressions

Added `revoke_user_skips_expired_and_already_revoked_records`, `retained_non_live_record_is_revocable`, `garbage_collection_removes_only_records_beyond_exact-retention-boundary` [test function uses `garbage_collection_removes_only_records_beyond_exact_retention_boundary`], and `injected_id_collision_retries_and_mirror_conflict_rolls_back_put`. The collision retry checks the preseeded row remains equal and the candidate sequence advances twice; the mirror-conflict case verifies the conflict remains in the mirror while the just-written durable row is removed.

**PARTIAL:** retained absent-mirror revoke tests cover injected durable failure, success, durable timestamp, audit event, counter and idempotence. Deterministic proof that retained-row lookup acquires the owner gate is not included; the draft scheduling assertion was removed as inconclusive.

### Item 4 — launcher shutdown and read-only behavior

`launcher_shutdown_stops_token_gc_task` starts a real `ServerLauncher`, observes GC termination during `ServerHandle::shutdown`, drops its additional service `Arc` after GC termination, and bounds shutdown. `read_only_node_rejects_create_and_revoke_mutations` creates a durable bearer on a writable service, reopens the same store read-only, verifies the existing bearer and checks create/revoke/revoke_user rejection. Tests are unexecuted pending CI.

### Item 5 — source split (performed by the orchestrator after revision 2)

The executor made no moves; the orchestrator then performed the split below with `git mv` plus extraction. `tokens/` keeps six top-level entries: `mod.rs`, `owners.rs`, `store.rs`, `tests/`, and the `model/` and `service/` directories.

Orchestrator extracts the model items (each subdirectory's `mod.rs` is a manifest only):
- `model/token_view.rs`: `TokenView` and `From<&TokenRecord>`.
- `model/created_token.rs`: `CreatedToken` and redacted `Debug`.
- `model/token_create_error.rs`: `TokenCreateError`.
- `model/token_counters.rs`: `TokenCounters` and implementation.

Orchestrator extracts service items; `service/` has exactly seven entries including its manifest:
- `service/session_canceller.rs`: `SessionCanceller` and `NoopCanceller`.
- `service/new_token.rs`: `NewToken`.
- `service/options.rs`: `TokenServiceOptions`.
- `service/token_service.rs`: `TokenService`, implementation and `Drop`.
- `service/gc.rs`: `GcService`.
- `service/generator.rs`: `TokenGenerator`.
- `service/mod.rs`: manifest/re-exports.

Update imports/re-exports during extraction; moving the current files as-is is insufficient.

### Item 6 — lint-aware read-through

Read end-to-end after edits:
- `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/security/tokens/service/token_service.rs`
- `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/security/tokens/store.rs`
- `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/security/tokens/tests/service_tests.rs`
- `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/security/tokens/tests/store_tests.rs`
- `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/tests/boot/server_tests.rs`
- `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/docs/dev-artifacts/audits/2026-11-01-token-t2b-server/DESIGN.md`

Reviewed imports/privacy/cfg(test), missing_docs on newly visible items, `Send`/`'static` captures, lock scope across awaits, exhaustive matches, moved values and `?` conversions, struct literals, unused items, clippy patterns including bool/assert comparisons and field-reassign-with-default, std HashMap/HashSet/RandomState bans, absolute path depth, no `#[allow]`/inline cfg(test)/local use, and folder/file layout. This source read-through is not compiler verification.

## Final report
Revision 2 items 1–6 status:
- Item 1 DONE — standard audit path; event details/schema/redaction and appender delivery covered.
- Item 2 DONE — typed failure seam covers record writes/deletes, revoke_user and GC batches.
- Item 3 PARTIAL — functional regressions added; retained-row owner-gate acquisition remains unverified.
- Item 4 PARTIAL — launcher GC shutdown and read-only reopen/verify/rejection regressions added; behavior awaits CI.
- Item 5 — split targets above; the moves and extraction were done by the orchestrator, not the executor.
- Item 6 DONE — six changed files read end-to-end and source classes reviewed above.
- Scoped `cargo fmt -p shamir-server`, `cargo fmt -p shamir-server -- --check`, `python -I scripts/ci/check_layout.py`, and `git diff --check` passed. Tests, compilation and Clippy were not run; CI verification remains required.
