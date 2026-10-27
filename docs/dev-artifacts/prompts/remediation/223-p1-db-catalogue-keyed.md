# P1 stage F13b — p1-db-catalogue-keyed

## Goal

Split from F13 (cards C58706F9, 5977A782, 23A03103). Remove the repeated O(N) system-catalogue scans on the batch authorization path, group-name resolution and repository record lookup, without a schema change and without weakening the fail-closed corruption contract. Each card is a `### RV-DB-<ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md`: read the whole card. Line numbers below are from master 62085d0f; re-locate them in this tree.

This worktree is based on branch fix/p1-db-errors-perf (PR #350, P1 F13, not yet merged). Its changes are a given: build on them, do not revert or rework them.

## Binding design (from the design review — implement as written; deviate only with file:line evidence in the final message)


### Facts and decision

- System tables have no indexes (`system_store/store.rs:112-125`). The planner supports index scans, but not when `!has_indexes()` (`read_planner.rs:151`).
- Ids are allocated, not derived from the name.
- Adding indexes would break the fail-closed contract in `tests/access/catalogue_corruption_tests.rs:70-137`: today an unrelated corrupt row makes the lookup fail.
- **Decision: no schema change.**

### C58706F9: per-request ACL memo

- **New file** `shamir_db/shamir_db/access/batch_gate.rs` (the folder goes from 5 to 6 entries):
  - `pub(super) struct AclMemo` with two maps, both `DashMap::with_hasher_and_shard_amount(THasher, 2)`:
    - `metas: DashMap<ResourcePath, ResourceMeta, THasher>`
    - `membership: DashMap<(u64, u64), bool, THasher>`
  - `struct BatchAccessGate<'a> { db: &ShamirDb, memo: AclMemo }`, which implements `AccessGate` by calling `db.authorize_access_in(actor, path, action, Some(&memo))`.
  - `pub(crate) async fn authorize_batch(&self, request, actor, db_name) -> Result<Authorized<'r>, BatchError>`. It builds the gate locally and calls `Box::pin(Authorized::authorize(.., &gate))`.
  - Never hold a DashMap ref across an `.await`: clone the value out first.
- **`authorize.rs`:**
  - `pub authorize_access` keeps its signature and delegates to the new `pub(super) authorize_access_in(.., memo: Option<&AclMemo>)`, which holds the current body from `:38-114`.
  - `resource_meta` calls (`:57`, `:87`) become `meta_memo`: look up the memo, otherwise load, and cache only `Ok`.
  - `resolve_in_group` (`:120-125`) caches only `Ok(bool)`. An error is still `false` and is not cached.
- **Call sites:** `db_execute.rs:72` and `db_tx.rs:272` switch to `self.authorize_batch(...)`. `impl AccessGate for ShamirDb` (`gate.rs`) stays without a memo.
- **Why no stale ACLs:**
  - The memo is dropped before execution starts.
  - In-batch mutation checks call `authorize_access` with `None`.
  - Every request, open-tx call and nested `execute_as` builds a new memo.

### 5977A782: name→id snapshot in SystemStore

- **New file** `system_store/group_names.rs` (the folder goes from 6 to 7 entries, the limit): `GroupNames { by_name: TFxMap<String,u64>, by_id: TFxMap<u64,String> }`.
- **New `SystemStore` fields:**
  - `group_names: Arc<ArcSwapOption<GroupNames>>`
  - `group_names_rebuild: Arc<tokio::sync::Mutex<()>>`. This is a DDL-only lock, sanctioned category 2; add the inline contention comment.
- **Rebuild:** after a durable `save_group` (`identities.rs:124`) or `remove_group` (`:258`):
  1. Take the mutex.
  2. Call `load_groups()`.
  3. Insert in scan order with `entry(name).or_insert(gid)`, which keeps first-match semantics.
  4. Store the result.
  - Skip the rebuild when the name is unchanged for an existing gid.
  - If the scan fails, store `None`.
  - Build once in `init_with_durability`; a failure there stores `None` and does not fail init.
- **Reader** `resolve_group_id` Name arm (`groups.rs:384-392`):
  - snapshot hit → id;
  - snapshot miss → `NotFound`;
  - `None` → the existing scan.
- Verify that nothing else writes the system `groups` table.

### 23A03103

- `load_repository_record` (`catalogue.rs:163-183`) delegates to `load_repository(db, repo)` (`:331-365`), the filtered read.
- Corruption diagnostics are kept, because the collecting scan reports every corrupt row.

### Regressions

- **Counters in `store.rs`**, `#[cfg(test)]`, following the `table_catalogue_reads` pattern at `:74-170`:
  - `catalogue_accesses`, incremented in `SystemStore::table()` (`:205`);
  - `full_catalogue_loads`, incremented in `load_repositories` (`catalogue.rs:141`) and `load_groups` (`identities.rs:153`).
- **New file** `src/shamir_db/tests/system/catalogue_lookup_cost_tests.rs`, registered in that `mod.rs` (the folder goes from 4 to 5 entries):

  | Test | Check | Without the fix |
  |---|---|---|
  | `rv_db_c58706f9_batch_auth_reads_shared_ancestors_once` | 8 reads on `data.t0..t7`, all System-owned 0o755, `Actor::User(7)`: `catalogue_access_count()==11` | 34 |
  | companion | a chmod between two separate calls is denied on the second | — |
  | companion | one denied table in a batch → Err naming it | — |
  | `rv_db_5977a782_group_name_resolution_is_keyed` | `resolve_group_id(Name "ops")`: `catalogue_access_count()==0`; plus rename / drop / direct `save_group` consistency | 1 |
  | `rv_db_23a03103_load_repository_record_is_not_a_full_load` | `full_catalogue_loads==0` | 1 |

- **Append** to `tests/access/catalogue_corruption_tests.rs`: `rv_db_23a03103_corrupt_repository_row_still_fails_closed`. A corrupt `repositories` row makes both `load_repository_record` and `rename_repo` return `Err`.
- Build queries in tests with `shamir_query_builder`.

### Layout

- `shamir_db/shamir_db/core.rs` is at exactly 1000 lines: do NOT touch it.
- `tests/access/` is full at 7 entries: append only.
- `shamir_db/shamir_db/tests/`, `execute/core/`, `execute/repo_admin/` and `shamir_db/db/` have 7 entries each: edit only.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
