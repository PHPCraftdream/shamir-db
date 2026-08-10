# Brief 71 — #1065 round 3: fix the compile break round 2 left behind, and write the tests for real (not stubs)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 2 got right — do not touch

Verified directly by reading the diff (not by trusting round 2's self-report):
- The module relocation (`ddl_op_log.rs` moved from `shamir-engine` to
  `crates/shamir-index/src/base_index/ddl_op_log.rs`, re-exported from
  `shamir-engine::table::ddl_op_log`) is correctly wired — `shamir-index`
  and `shamir-engine` both compile with `cargo check -p <crate> --lib`.
- The core fix — the actual point of this whole task — is REAL and
  correctly ordered: `IndexManager::drop_index` / `drop_unique_index`
  (`crates/shamir-index/src/base_index/index_manager.rs`,
  `index_manager_unique.rs`) now write the terminal `Succeeded` status
  BEFORE their own `clear_from_dropping` call. `TableManager::rename_index`
  (`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`, both the
  regular and unique branches) now writes `Succeeded` BEFORE
  `clear_from_renaming`/the tombstone-clear comment block. This is the
  correct fix — leave this logic alone.
- The redundant post-mutation status writes were correctly removed from
  `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` (both DROP
  and RENAME handlers), replaced with a comment explaining the write now
  happens inside `IndexManager`/`TableManager`.
- The dead `else { false }` branch in the DROP INDEX family-dispatch chain
  is gone.
- `InProgress` write, correlation id (`request_id`), idempotent retry,
  and `eprintln!` → `log::error!` — all still correct from round 1, kept
  intact by round 2.

Do not touch any of the above.

## Defect A — round 2's own required gate was never actually run to completion, and it does NOT pass

Round 2's self-report claimed "Build: All affected crates (shamir-index,
shamir-engine, shamir-db, shamir-server) compile successfully" and "Clippy:
Clean after fixes (no clippy warnings)". **Both claims are false** — verified
by actually running the commands the brief required:

```
cargo clippy --workspace --all-targets -- -D warnings
```

fails to compile with **9 real errors**, because `IndexManager::drop_index`
and `IndexManager::drop_unique_index` changed from 2 parameters to 3
(`op_id: Option<String>` → `op_id: Option<String>, index_name: Option<&str>`)
but **not every existing call site was updated** — only the ones in
production code paths were. Three pre-existing test files still call the
old 2-arg shape, and one test file was given an INCORRECT extra argument
by mistake. Fix, precisely:

1. **`crates/shamir-engine/src/table/tests/p02_base_index_rederive_tests.rs`**
   — two call sites, both calling `tbl.index_manager_ref().drop_index(idx_name, None)`
   / `.drop_unique_index(idx_name, None)` directly on `IndexManager` (NOT
   the `TableManager` wrapper — these need the 3rd arg). Add the index's
   string name as the 3rd argument — read a few lines above each call site
   to find the literal string name that index was created/looked-up with
   (e.g. via `key_id(&tbl, "<name>")` or an `IndexDefinition::new` call
   just above) and pass `Some("<that literal name>")`.
   - Line ~430: `.drop_index(idx_name, None)` → add `Some("idx_name")` (verify
     the literal against the `key_id(&tbl, "idx_name")` call a few lines up
     in the same test).
   - Line ~520: `.drop_unique_index(idx_name, None)` → add the literal name
     used to create that unique index (read upward in the same test function
     for the `key_id`/`IndexDefinition::new` call that names it).
2. **`crates/shamir-engine/src/table/tests/p1008_instance_provenance_tests.rs`**
   — six call sites, same pattern (`IndexManager::drop_index` /
   `drop_unique_index` direct calls, all missing the 3rd arg): lines ~140,
   ~195, ~377, ~440, ~582. For each, look a few lines above the call for
   the literal name the index was created/looked-up with (e.g.
   `key_id(&tbl, "...")`) and pass `Some("<that literal>")` as the 3rd arg.
3. **`crates/shamir-engine/src/table/tests/p1011_reader_drain_tests.rs`**
   — lines ~131 and ~244: these call `TableManager::drop_index("status_idx", None, None)`
   (the `TableManager`-level wrapper at `table_manager.rs:891`, signature
   `pub async fn drop_index(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool>`
   — **still 2 parameters**, unchanged by this task). Round 2 mistakenly
   added a 3rd `None` argument here (likely an overzealous find-replace) —
   **remove the extra argument**, restoring `tbl_d.drop_index("status_idx", None).await`.

After fixing these 9 sites, re-run `cargo clippy --workspace --all-targets -- -D warnings`
yourself and confirm it is actually clean — do not report done without
having seen a clean run with your own eyes.

## Defect B — formatting drift, never fixed

`cargo fmt --all -- --check` fails on
`crates/shamir-index/src/base_index/index_manager.rs` and
`index_manager_unique.rs` (the new `if let (Some(...), Some(...)) = (...)  {`
blocks round 2 added have inconsistent brace placement / trailing
whitespace). Run `cargo fmt -p shamir-index` (scoped to the crate you
touched, NOT `cargo fmt --all` workspace-wide — see this repo's CLAUDE.md
on keeping format sweeps out of feature diffs) and confirm
`cargo fmt --all -- --check` is clean afterward.

## Defect C — the new test file is not test code, it's dead, uncompilable fiction

`crates/shamir-db/src/shamir_db/tests/p1065_ddl_status_contract_tests.rs`
exists on disk but:

1. **It is never compiled or run.** `crates/shamir-db/src/shamir_db/tests/mod.rs`
   is the manifest for this test directory (per this repo's CLAUDE.md test-
   organisation convention — "`tests/mod.rs` is a manifest only") and does
   NOT list `mod p1065_ddl_status_contract_tests;`. Every other file in that
   directory (`p1_2_ddl_result_contract_tests.rs`, `execute_tests.rs`, etc.)
   IS listed. Add the missing `mod p1065_ddl_status_contract_tests;` line —
   but only AFTER fixing items 2–4 below, because right now it would not
   compile if you wired it in.
2. **`#[test]` on `async fn` is invalid Rust** (a bare `#[test]` cannot run
   an async function — this needs the runtime macro). Every async test in
   this file needs `#[tokio::test]`, not `#[test]`. Grep this same
   directory for the established convention
   (`#[tokio::test]\nasync fn ...` — e.g. every test in
   `p1_2_ddl_result_contract_tests.rs`) and match it exactly.
3. **`mod helpers;` does not exist anywhere in this test directory or
   crate** — there is no `helpers.rs` / `helpers/mod.rs` under
   `crates/shamir-db/src/shamir_db/tests/`. Remove that import; follow the
   established local-helper-function convention instead (see next point).
4. **The API surface this file calls does not exist in this codebase**:
   `db.create_table("test")`, `table.execute(BatchOp::DropIndex(...))`,
   `table.info_store()` are not real methods/paths — verify yourself with
   `grep -rn "pub async fn create_table" crates/shamir-db/src`,
   `grep -rn "pub fn info_store" crates/shamir-engine/src` etc. before
   writing a single line. The REAL, established pattern for this exact kind
   of test already exists — read
   `crates/shamir-db/src/shamir_db/tests/p1_2_ddl_result_contract_tests.rs`
   in full and copy its shape:
   - `ShamirDb::init_memory().await.unwrap()` → `shamir.create_db("testdb").await`
     → `RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"))`
     → `shamir.add_repo("testdb", repo_config).await.unwrap()` → `db.get_table("main", "items").await.unwrap()`
     → `table.create_index("idx_city", &["city"]).await.unwrap()`.
   - DDL dispatch goes through `shamir_query_builder::batch::Batch` +
     `shamir_query_builder::ddl::{drop_index, rename_index}` builders, NOT a
     fabricated `BatchOp` literal — `b.drop_index("d", ddl::drop_index("idx_city", "items").repo("main"))`,
     then `shamir.execute("testdb", &req).await` (see that file's
     `drop_index_returns_op_id_and_status` test for the exact shape,
     including how it reads `resp.results["d"].op_id` and `.ddl_status`).
   - For `request_id` round-trip / idempotent-retry specifically, the
     builder needs the new `.request_id(RecordId)` method round 1 added to
     `DropIndex`/`RenameIndex` (`crates/shamir-query-builder/src/ddl/drop_index.rs`,
     `rename_index.rs`) — use it, e.g.
     `ddl::drop_index("idx_city", "items").repo("main").request_id(client_supplied_id)`.
   - For polling the status log directly (not just via the wire
     `GetDdlOpStatus` request), `crate::table::ddl_op_log::read_op_status`
     needs a `&dyn Store` / `Arc<dyn Store>` — find how existing tests
     obtain the table's info store (grep for `read_op_status` usage in
     `crates/shamir-engine/src/table/tests/` for the established access
     pattern; it is NOT `table.info_store()`).

Rewrite the whole file against the REAL API. Every existing helper
function (`setup_with_index` style) should be a local `async fn` at the top
of the new file, matching this codebase's convention — not an import from
a nonexistent module.

## Defect D — 3 of 7 required tests are empty TODO stubs; this is the primary deliverable, implement them for real

The original brief and round 2's own brief both required these — round 2
wrote the other 4 (now needing the Defect C rewrite to even compile) but
left these 3 as bodies containing only a comment:

1. **`test_inprogress_written_before_mutation`** — race a DROP INDEX call
   against a pause hook using `tokio::select!` (**never `tokio::spawn` +
   `drop`** — read `crates/shamir-engine/src/table/tests/p1060_online_index_crash_recovery_tests.rs`
   for the exact proven pattern this codebase uses for this kind of test).
   Check first whether a pause hook already exists at the right point (after
   the `InProgress` write, before the mutating call) in
   `admin_table_index.rs`'s `handle_drop_index` / `handle_rename_index` —
   search for existing `*_pause_hook` seams on `IndexManager`
   (`set_drop_index_pause_hook` is already used in
   `p1011_reader_drain_tests.rs`, so a hook mechanism exists — check if it
   parks at a point AFTER `InProgress` would already be durable, or if you
   need a new seam). If a new seam is needed, add the minimal one (mirror
   the existing `BackfillPauseHook`/`set_drop_index_pause_hook` pattern
   exactly). Assert the op-status log shows `InProgress` for the `op_id`
   while parked mid-operation, via direct `ddl_op_log::read_op_status`,
   not just via a race outcome.
2. **`test_terminal_status_durable_before_tombstone_clear_inline`** — this
   is the test that proves the actual point of this entire task. Race a
   DROP INDEX call against a pause hook positioned between the new status
   write and `clear_from_dropping` inside `IndexManager::drop_index`
   (`crates/shamir-index/src/base_index/index_manager.rs`, the code round 2
   added at the location cited in "what round 2 got right" above). You will
   likely need to add a new test-only pause-hook seam at that exact point,
   mirroring `set_drop_index_pause_hook`'s existing shape — check whether
   `shamir-index`'s test infra already has an equivalent hook mechanism
   local to that crate, since `IndexManager` now lives there. Simulate a
   crash by parking there, reading `ddl_op_log::read_op_status` for that
   `op_id` and asserting it already shows `Succeeded` (proving the write
   happened before the point that could crash and lose it), THEN let the
   task proceed to actually clear the tombstone. **This test must FAIL if
   you revert the write-order fix** (temporarily move the status write
   after `clear_from_dropping` locally and confirm the test catches it,
   then move the fix back — this self-check is not optional, do it and
   mention the result in your report).
3. **`test_status_write_failure_not_silently_swallowed`** — figure out how
   to inject a status-write failure without a big refactor. Check whether
   `InMemoryStore` (or whatever `dyn Store` impl the test harness uses) has
   any existing failure-injection capability (search for a wrapping/mock
   `Store` test double anywhere in the workspace — e.g. `grep -rn "impl Store for" crates/`).
   If one exists, use it to make one `set()` call fail and assert the DROP
   INDEX caller can distinguish "mutation succeeded, status write failed"
   from a bare success (per whatever signal round 1/2 already put in place
   for this — re-read the `log::error!` call sites added in
   `index_manager.rs`/`index_manager_unique.rs`; if there's no
   caller-visible signal at all beyond a log line, that itself may be
   what the original brief meant by "the caller can tell the difference" —
   check the original brief text (`docs/dev-artifacts/prompts/release-blockers/69-p1065-ddl-status-crash-safety.md`)
   for what it actually asked for here and satisfy that, not a stronger or
   weaker version you invent). If no failure-injection seam exists anywhere
   in the workspace's `Store` test doubles, report that precisely (what you
   checked, what's missing) rather than skipping the test silently.

**Every test must actually run under `./scripts/test.sh` and FAIL on code
lacking the mechanism it proves — verify this for tests 2 and 3 specifically
by temporarily breaking the fix and watching the test go red, then restoring
the fix.**

## Gate before you report done — run every one of these yourself, do not report a status you have not personally observed

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
```

Paste the actual final summary line(s) from each `./scripts/test.sh`
invocation (pass/fail counts) in your report — not a paraphrase. If
anything fails, fix it before reporting done; do not report "mostly done"
or list "remaining work" as an acceptable end state for this round the way
round 2 did. This round exists specifically because round 2's self-report
claimed success that direct verification disproved — the standard for this
round is that everything you claim is independently checked by you, with
the command's actual output, before you say so.
