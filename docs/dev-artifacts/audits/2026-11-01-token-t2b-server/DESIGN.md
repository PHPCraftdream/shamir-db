# T2b server chunk: A, D, E

## Decisions

- A stores a separate raw 32-byte token key in server metadata; its wrapper is zeroizing and Debug-redacted. Legacy metadata gets a durable lazy upgrade and malformed key sizes fail closed.
- D exposes the cached optional ticket floor without changing legacy zero-on-miss semantics.
- E adds nested `TokensConfig` defaults and validation. Deploy examples were intentionally untouched.
- B/C/F remain partial and unverified; no local build/test/Clippy execution is authorized. CI must establish compilation and behavior.

## Public API

- `server_meta::{TokenKey, ServerMetaStore::token_key}`
- `FjallUserDirectory::floor_if_present`
- `config::TokensConfig`
- `security::tokens::{TokenServiceOptions, TokenService, NewToken, SessionCanceller, TokenStore, TokenStoreError, TokenView, CreatedToken, TokenCreateError, TokenCounters, DirectoryOwnerFloors}`
- Service methods in `crates/shamir-server/src/security/tokens/service.rs`; store methods in `.../store.rs`.

## Current implementation/status

- A, D, E: implemented with regression/config tests; tests not run.
- B, C: partial implementation includes durable fjall store, named MessagePack, owner-stripe mutations, token service CRUD/verify/counters/audit, owner-floor adapter, retention GC, and tests. Known incomplete requirements: injected write-failure guarantee, cancellation-specific regression, audit/canceller ordering assertions, real-directory adapter-to-service test, full role/database/channel/mode policy matrix. Audit append failures currently warn and continue; cancellation is an ID hook alongside direct TokenSessionStore removal.
- Explicitly unresolved: `TokenStore::open` only opens; `TokenService::open` scans/loads/GCs; `active_count_hint` counts mirror entries including revoked/expired until GC; revoke of a retained record absent from mirror returns false; token-ID collision handling is not robust after durable put; no injected write-failure tests; no boot/shutdown tests; audit appends run synchronously on `spawn_blocking` (not a dedicated queue) and failures are warnings; full tests and end-to-end reread incomplete.
- F: partial boot wiring opens service from metadata key, connects the directory floor adapter, passes `NodeMode` from `ServerLauncher` (default ReadWrite), guards GC on read-only mode, starts/stops retained GC, and exposes service on `ServerHandle`. No boot/shutdown regression test. `ListenerEnv` does not expose service; consumers must use `ServerHandle`.
- No per-owner global mutation lock: fixed 64 owner stripes are used; GC locks all stripes in stable order. Store’s `std::sync::Mutex` serializes cold DB operations on blocking workers only.

## Tests added (not executed)

- Metadata: `token_key_first_open_and_reopen_preserve_exact_bytes`, `token_key_accessor_rejects_non_32_byte_storage`, `malformed_token_key_length_fails_open_closed`, `legacy_meta_lazy_upgrade_preserves_other_key_bytes`.
- Directory: `floor_if_present_tracks_ticket_invalidation_and_unknown_users`.
- Config: tests in `tokens_config_tests.rs` for defaults, parsing, validation, and invalid values.
- Store: `token_store_round_trip_and_live_reload`, `token_store_skips_and_counts_corrupt_records`, `garbage_collection_removes_only_records_beyond_exact_retention_boundary`.
- Service: `create_verify_and_revoke_publishes_after_durable_record`, `labels_and_limits_count_only_active_unexpired_records`, `ttl_bounds_zero_over_max_and_default_are_enforced`, `database_allowlist_shape_is_rejected_before_persistence`, `concurrent_owner_creates_never_exceed_active_limit`, `owner_floor_rejects_unknown_and_equal_floor_timestamp`, `read_only_node_rejects_create_and_revoke_mutations`.

- Explicitly unresolved: `TokenStore::open` only opens; `TokenService::open` scans/loads/GCs; `active_count_hint` counts mirror entries including revoked/expired until GC; revoke of a retained record absent from mirror returns false; token-ID collision handling is not robust after durable put; no injected write-failure tests; no boot/shutdown tests; audit appends run synchronously on `spawn_blocking` (not a dedicated queue) and failures are warnings; full tests and end-to-end reread incomplete.


- `cargo fmt -p shamir-server -- --check` and `python -I scripts/ci/check_layout.py` pass. `cargo fmt -p shamir-server` was used to apply formatting.
- Builds, tests, Clippy were not run; compile correctness remains unknown. Require CI compile, tests, Clippy/warnings-as-errors, and review of lifecycle, durable failure, replica behavior, and audit semantics.
- Manual source review covered token service/store/model/owners/tests, Config migration, launch/handle/runtime, NodeMode handler path, metadata and directory work, and listed Config literal sites. Full end-to-end reread and test matrix were not completed before deadline.
- B/C/F are explicitly partial; no completion checkpoint is claimed.

- Compatibility follow-up: exhaustive `ServerLauncher` literals in `shamir-server` source/tests/benches now set `node_mode: Default::default()`; struct-update runtime construction remains unchanged. `rg 'ServerLauncher {'` found only these actual literal sites plus definition/destructuring/docs.


- A/D/E: implemented; named regression/config tests added, not executed.
- B/C: partial and compile status unknown; no write-failure injection, lifecycle boot tests, or complete matrix. Owner serialization, persistence/service API and GC draft exist but remain CI-review candidates.
- F: partial launcher/handle wiring; no boot/shutdown test, production role-selection validation, or full lifecycle proof.
- Explicit unresolved risks: `TokenStore::open` does not scan/GC/load mirror (service open does); active count is approximate and includes stale mirror entries; revoke of retained non-mirrored row returns false; ID collision path is not robust; audit synchronous on blocking worker, not queued; full end-to-end reread incomplete.
- Tests were not run; no build/Clippy. Formatting and layout checks are the only validation. CI must compile, lint, test and review all paths.

## Revision 1 checkpoints

- Item 1 PARTIAL: `TokenService::open` (`crates/shamir-server/src/security/tokens/service.rs:73-114`) owns full scan, shape checks in `TokenStore::scan_blocking`, live mirror load, and initial GC; test `service_open_loads_preseeded_live_records_and_skips_non_live_corrupt` seeds durable rows before service open.
- Item 2 PARTIAL: `put_if_absent_blocking` (`store.rs:149-164`) and 8 retries (`service.rs:218-250`) prevent overwrite; strengthened `store_put_if_absent_preserves_existing_id` collision candidate changes label/MAC under same ID; no injected ID source or cross-owner-stripe service collision test.
- Item 4 PARTIAL: retained absent-mirror rows get durable revoke and missing IDs return typed `TokenCreateError::NotFound` (`service.rs:290-310`, `model.rs:78-80`); store write uses sibling-safe `with_locked_records` and `write_locked`. Hook ordering is durable write, mirror publish then canceller (`service.rs:329-333`); observer test confirms durable/mirror at hook time. Missing write-failure seam/test; retained timestamp idempotence and lookup/revoke race are untested.
- Item 3 DONE via explicitly named `mirror_entry_count_hint` and new `active_count(now_ns)` (`service.rs:140-153`); per-owner cap scans durable ACTIVE records (`service.rs:201-217`) and does not rely on hint.
- Item 4 PARTIAL: retained absent-mirror rows durably revoked; missing id returns `TokenCreateError::NotFound` (`service.rs:290-310`, `model.rs:78-80`). Durable write uses sibling-safe store operation, then mirror publish/canceller (`service.rs:329-333`); ordering observer confirms durable/mirror at hook time. Missing write-failure seam/test; retained timestamp idempotence and fetch/revoke race untested.
- Item 5 PARTIAL: three event names emitted by `AuditChainWriter::append`; identifiers-only payload. `AuditChainWriter` is synchronous and has no queue API; no queue/change to connect, emission tests not added.
- Item 6 PARTIAL: service owns hourly GC, injectable clock and cancellation/join (`service.rs:461-508`); added `gc_task_exits_after_cancellation_without_sleeping`; no launcher shutdown test.
- Item 7 PARTIAL: service construction moved before listener spawning (`server_launcher/launch.rs:569-624`); NodeMode originates from `ServerLauncher::node_mode` (`launch.rs:49-67,80-85`), defaults ReadWrite, and is passed to token options. Server handle owns service and joins GC in shutdown (`server_handle.rs:52,186-188`). No launcher regression; errors after spawning listeners can still leave work/tasks unless lifecycle cancellation is added.

## Final report

- Item 1 PARTIAL — service-owned startup scan/load/GC implementation; preseeded-record test added (not run).
- Item 2 PARTIAL — atomic durable put-if-absent plus bounded retries, store collision test; no injected generator or service retry test.
- Item 3 DONE — mirror cardinality is explicitly named `mirror_entry_count_hint`; added real `active_count(now_ns)` (`service.rs:140-153`) counting only unexpired, unrevoked durable rows. Owner limits independently evaluate ACTIVE rows during serialized create.
- Item 4 PARTIAL — retained record revocation/typed NotFound; write uses `with_locked_records`/`write_locked`, then mirror and canceller. Ordering observer in `create_verify_and_revoke_publishes_after_durable_record` asserts durable and mirror state at hook time. Write-failure injection and retained idempotent timestamp tests missing.
- Item 5 PARTIAL — no audit queue exists on observed `AuditChainWriter`; synchronous append remains, payload includes ids only. No event assertions added.
- Item 6 PARTIAL — service GC uses injectable clock and cancellation, task joined (`service.rs:461-508`); cancellation regression added. No deterministic proof task starts/completes before cancellation; CI should review.
- Item 7 PARTIAL — service constructs before listeners, handle owns service and shutdown joins; role is launcher `NodeMode` builder argument (default ReadWrite). No launcher/shutdown test; listener/spawn error cleanup remains.
- Item 8 PARTIAL — focused lint-aware read-through caught and fixed compilation issues in touched tests/service/storage code (see corrections below), but the complete inherited dirty tree was not reread. No compile/tests/Clippy run; CI required.
- Physical layout: `src/tests/boot` has 8 immediate physical entries. `#[path]` does not change this. Operator must move `tokens_config_tests.rs` into an existing test directory and update the module path; no file was moved/deleted.
- Item 8 remaining review/test concerns: retained-record fetch-then-write race; service-level collision retries across owner stripes; full audit/GC/launcher assertions. Local compile/tests/Clippy are prohibited and remain CI tasks.
- Tests added: `store_put_if_absent_preserves_existing_id`, `service_open_loads_preseeded_live_records_and_skips_non_live_corrupt`, `retained_non_live_record_is_revocable`, `gc_task_exits_after_cancellation_without_sleeping`, `revoke_user_only_revokes_owners_tokens_and_leaves_floor_untouched`. Existing `create_verify_and_revoke_publishes_after_durable_record` strengthened to assert persisted/mirror state in canceller hook.
- Files read end-to-end for final lint review: see the exact list in the Item 8 checkpoint below; the complete preexisting dirty worktree was not claimed as reread.
- Files for orchestrator move/delete: move `/D:/dev/rust/shamir-db/worktrees/p34-t2b-token-server/crates/shamir-server/src/tests/boot/tokens_config_tests.rs` to a folder with room and update `boot/mod.rs`; no moves/deletes made here.

- Item 8 concrete corrections applied in final pass: replaced nonexistent `VerifyFailure::Malformed` with `VerifyFailure::Invalid` (`service_tests.rs:120`); replaced inaccessible private `put_blocking` with `with_locked_records`/`write_locked` (`service.rs:301-308`); added `#[cfg(test)]` to test-only getters (`service.rs:123-131`); fixed preseed live expiry and collision candidate (`store_tests.rs:57-65,110-111`).
- Item 8 limitation: full inherited dirty tree was not reread; remaining compile/lint issues possible. Local build/test/Clippy prohibited; CI must check.
