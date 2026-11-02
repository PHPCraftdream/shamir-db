## Prerequisite items

- TypeScript SCRAM validator and SCRAM/protocol tests are present. Execution status is not established here.
- Genuine Argon2 invocation counter exists in `crates/shamir-connect/src/common/primitives/crypto.rs:212-228`; actual counter test is `crates/shamir-connect/src/client/tests/kdf_rejection_tests.rs`. The counter is connect-only; do not infer TCP handshake instrumentation from it. IPC rejection coverage exists separately; no `kdf_tests.rs` exists under `crates/shamir-client`, and no old stray file in that area was found.
- Mapped IPv4 normalization is a separate prerequisite: regression test `subnet_of_normalizes_mapped_ipv4_and_preserves_ipv6_families`.

## Revision 1 report

All DONE statuses below describe implementation/source state only; they do not mean tests, build, or CI were run.

- Item 1 — DONE (source only; runtime CI pending). Stale inactive pairs are free for base, parent-quota, and admission accounting; their effective count is zero, while active entries are protected. Quotas remain 64/512. Tests: `stale_high_count_pair_is_replaced_at_full_base_and_parent_quotas`, `expired_pair_lock_releases_quota_without_gc`, `stale_pair_slot_is_reused_when_global_table_has_room`.
- Item 2 — DONE (source only; runtime CI pending). Fixed-table index stored in reserved slot bytes; lookups bounded to <=14 search steps at 8,192 capacity, under a single mutex. Quota and eviction scans occur only for new-pair admission. Index tests: `lookups_are_bounded_and_preserve_physical_slots_at_all_fill_levels`, `removals_clears_and_replacements_keep_index_reachable`, `shared_prefix_keys_are_collision_independent_and_comparator_is_total`. Collision test: `hash_collisions_keep_pair_index_lookups_bounded_through_remove_and_reinsert`, using deterministic pigeonhole 32 same-Fx-home keys. Store integration test: `indexed_lookup_survives_reset_unlock_gc_and_slot_reuse`.
- Item 3 — DONE (source only; runtime CI pending). Snapshot tests: `snapshot_caps_base_parent_and_global_with_active_first_and_highest_count`, `quota_eviction_uses_lowest_count_before_oldest_and_oldest_ties`, `lock_only_pair_is_retained_and_stale_failures_gc_without_expiring_hour_lock`. Snapshot shape unchanged.
- Item 4 — DONE: this report corrected to current source reality.

Progress: Item 1 completed (source only, runtime CI pending); expanded coverage includes `expired_pair_lock_releases_parent_quota_without_gc_for_both_families`.
Progress: Item 2 completed (source only, runtime CI pending); expanded coverage includes `handshake_and_existing_pair_lookups_use_bounded_index_when_full`.
Progress: Item 3 strengthened with `gc_removes_stale_payload_from_index_at_reset_boundary` (source addition; demonstrates `occupied == false` and empty index after GC, not merely failure count zero; execution not established).
Progress: Item 4 completed (report corrected).

Documentation-only nested path preserved and unreachable: `crates/shamir-connect/worktrees/p34-auth-pre/crates/shamir-connect/src/server/limits/lockout_storage/pair_slot.rs`.

## Verification and outstanding work

Observed at 10:01 CET: `cargo fmt -p shamir-connect -- --check`, tracked-file layout check, and git diff whitespace check all passed. The final lint-aware source read-through was completed; no concrete remaining mechanism bug was found. No compilation, `cargo check`, clippy, build, tests, `rustc`, npm, or behavioral tests were run. Runtime behavior and the 128-byte slot assertion remain pending CI; the assertion exists in source but was not compiled. Runtime CI remains pending; do not treat implementation as runtime-verified.

Manual inventory: `lockout_storage` has 4 source/test files (`pair_slot`, `pair_index`, `pair_index_tests`, `pair_collision_tests`); security has 6 entries; crate root has 7 entries, including accidental `worktrees/`. Meaningful changed files: `lockout.rs`, `pair_slot.rs`, `pair_index.rs`, `pair_index_tests.rs`, `pair_collision_tests.rs`, `lockout_storage_tests.rs`, and this report. Each source file is under 1,000 lines. The nested documentation placeholder remains and was not removed; it is untracked, unreachable, and cleanup requires operator authorization. All Cargo manifests and lockfiles are unchanged. No files were reverted, moved, deleted, or emptied; no commits or pushes were made. Overall status: PARTIAL cleanup, runtime CI pending.

Index tradeoff: lookup takes at most 14 iterations and is allocation-free within the 8,192-entry table under one mutex. Insert/remove shift bounded rank metadata (O(8,192)); GC/admin operations may repeat those shifts on a cold path.

## Final message

**PARTIAL — implementation complete; CI verification and cleanup remain.**
- Items 1–4 are implemented: stale/expired pairs release quotas without GC; exact lookup uses an allocation-free index bounded to 14 search steps; regression oracles and the report are corrected.
- Changed: `lockout.rs`, `lockout_storage/{pair_slot,pair_index,pair_index_tests,pair_collision_tests}.rs`, `lockout_storage_tests.rs`, and this report. Final lint-aware read-through completed; formatting, tracked-file layout, and whitespace checks pass.
- No compilation, clippy, or behavioral tests ran. Runtime behavior and the 128-byte slot assertion still require CI.
- An accidental one-line placeholder remains at `crates/shamir-connect/worktrees/p34-auth-pre/crates/shamir-connect/src/server/limits/lockout_storage/pair_slot.rs`. It was preserved under the no-delete rule; operator-authorized cleanup is outstanding. No files were reverted, moved, deleted, or emptied; no commits or pushes were made.
