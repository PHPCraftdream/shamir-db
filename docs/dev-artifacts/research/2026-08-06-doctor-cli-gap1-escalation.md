# Gap 1 — Missing test scenarios (b) and (c): Escalation

## What the original brief required

The original P1-1 (#1014) Definition of Done required at least 3 integration tests:
- (a) a healthy data dir → exit 0, clean report — **DONE** (`doctor_healthy_data_dir_exit_zero`)
- (b) a data dir with a deliberately Building/stuck index → non-zero exit, report correctly flags it
- (c) `--apply` actually heals it and a SECOND `doctor` run reports healthy

## Why (b) and (c) are NOT constructible from `shamir-server` tests

To construct a durably-stuck Building index, we need to:

1. Access a `TableManager` instance
2. Install a `BackfillPauseHook` via `table_mgr.index_manager_ref().set_create_index_backfill_hook(...)`
3. Spawn a `create_index` operation that parks mid-backfill
4. Shutdown the server WITHOUT completing the backfill, leaving the index durably at `Building`
5. Invoke the `doctor` CLI subprocess to detect the stuck index

However, from `shamir-server/tests` we have **no access** to:

- `TableManager` directly — it's not re-exported through `shamir_db`
- `BackfillPauseHook` — it's in `shamir-index`, but we can't get to the `IndexManager` to install it
- Any API to install pause hooks from the CLI/test side

The only way to interact with the database from `shamir-server/tests` is through the DDL wire protocol (`shamir_client::Client`), which:
1. Doesn't expose pause hooks
2. Doesn't provide a way to park operations mid-execution
3. Completes CREATE INDEX synchronously (or spawns background tasks we can't control)

## Dependency direction prevents a fix

The dependency chain is:
```
shamir-server → shamir-db → shamir-engine → shamir-index
```

To add test scaffolding in `shamir-server` that can install pause hooks, we'd need:
- Either re-export `TableManager` and pause hook APIs through `shamir_db`
- Or add a special "test mode" DDL op that can park mid-backfill

Both options would be **disproportionate engine-side scaffolding** beyond the scope of this task.

## What would be needed to close this gap

To properly test scenarios (b) and (c), one of these approaches would be needed:

### Option 1: Re-export engine types through `shamir_db`
Add to `shamir_db/src/lib.rs`:
```rust
pub use shamir_engine::table::TableManager;
pub use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
```
And expose methods like:
```rust
impl TableManager {
    pub fn index_manager_ref(&self) -> &crate::index::IndexManager { ... }
}

impl IndexManager {
    pub fn set_create_index_backfill_hook(&self, hook: Option<Arc<BackfillPauseHook>>) { ... }
}
```

This would allow `shamir-server/tests` to:
1. Open a server and create a table
2. Get the `TableManager` via a new API
3. Install a `BackfillPauseHook`
4. Spawn `create_index` and wait for it to park
5. Shutdown WITHOUT completing the backfill
6. Run `doctor` CLI subprocess to detect the stuck index

### Option 2: Add test-mode DDL operation
Add a special DDL operation like `admin::test_create_stuck_index` that:
1. Creates an index and immediately marks it as `Building` without backfilling
2. Persists this state durably

This avoids needing to expose internal engine types but requires a new DDL path in both `shamir-server` and `shamir-engine`.

## Recommendation

Do NOT add this scaffolding in this pass. The original brief anticipated this exact situation ("if a clean durably-stuck-Building state genuinely cannot be constructed from `shamir-server`'s test binary without disproportionate new engine-side test scaffolding, STOP and report exactly why").

The underlying `doctor::verify()` and `doctor::repair()` methods ARE already well-tested in `shamir-engine/src/table/tests/doctor_tests.rs`:
- `verify_detects_building_regular_index`
- `verify_detects_building_unique_index`
- `verify_detects_building_sorted_index`
- `repair_heals_orphan_index_entry`
- etc.

Those tests verify the core logic works correctly. The CLI integration layer is also tested by the existing tests that exercise:
- Healthy tables → exit 0
- Non-existent data_dir → error
- Filter options work
- JSON output is valid

The gap is ONLY in end-to-end CLI testing of stuck indexes, not in the core functionality.