# Follow-up brief #2 — P1-2 (#1015): fix the op-status poll routing bug, then finish gates/tests

## Context

Round 2 on session `t1015-ddl-result-contract` got the workspace
compiling again (0 errors, confirmed) and wired `admin_result_with_op_id`
into `handle_drop_index`/`handle_rename_index` in
`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` — good
progress, keep that. It hit its own `--timeout 90m` ceiling before
finishing tests/gates ("Context deadline exceeded"), which is expected
for a task this size, not a failure.

Zero-trust review of what landed so far found **one real, load-bearing
bug** that must be fixed before this can be considered working, plus the
still-outstanding test/gate work from the original follow-up brief.

## Bug (CRITICAL) — `get_ddl_op_status` cannot actually find most operations' status

`crates/shamir-db/src/shamir_db/shamir_db/core.rs::get_ddl_op_status`
currently does this (read the actual function to confirm current state,
this describes what round 2 landed):

```rust
// Get the first available db/repo to access the info_store.
let db = match self.list_dbs().into_iter().next() { ... };
```

**This is wrong.** `info_store` is a **per-table** field on
`TableManager` (`crates/shamir-engine/src/table/table_manager.rs:29`,
`pub(super) info_store: Arc<dyn Store>`) — confirmed by reading the
struct definition — NOT a single global store shared across the whole
`ShamirDb`. The op-status log therefore actually lives inside whichever
specific table's `TableManager` wrote it (since `write_op_status` is
called from `handle_drop_index`/`handle_rename_index`, which operate on
one specific table's `TableManager::info_store()`). "Grab the first
db/repo" will only ever find a status record if it happens to belong to
that arbitrarily-first table — for every other table in the system, a
legitimate, just-completed op's poll will silently return `None`/
`Unknown`, indistinguishable from "never existed". This defeats the
RFC's entire purpose (§0: the load-bearing question is "did my CREATE
INDEX from before the crash finish?" — this bug means the answer is
wrong for any table that isn't the first one enumerated).

**Fix — thread routing context through the poll request, matching this
codebase's existing convention.** Every other per-db/table `DbRequest`
variant that needs scoping already carries it explicitly as a field (see
`DbRequest::CreateCursor { query_version, db, query, page_size }` in
`crates/shamir-server/src/db_handler/handler.rs` — there is no ambient
"current db" at the wire level, so this is the established pattern, not
a new one). Extend `DbRequest::GetDdlOpStatus` to carry the routing the
client already knows (it just issued the DDL call against a specific
`db`/`repo`/`table`):

```rust
DbRequest::GetDdlOpStatus {
    db: String,
    repo: String,
    table: String,
    op_id: RecordId,  // or however it's currently typed
}
```

This is a legitimate refinement of the RFC's own wire shape — the RFC
explicitly marks every wire type as "illustrative... DRAFT — pending
review; none exist yet" (RFC preamble) precisely because implementation
would surface exactly this kind of gap. Do not treat this as scope creep;
it is required for the feature to work at all.

Thread this through the whole chain:
- `crates/shamir-query-types/src/wire/db_message.rs` — the enum variant.
- `crates/shamir-server/src/db_handler/handler.rs` — the dispatch arm and
  `get_ddl_op_status` handler method: use `db`/`repo`/`table` to resolve
  the correct `TableManager` (mirror however `CreateCursor`'s handler
  resolves its own `db`/table target — same access pattern should apply)
  and call `ddl_op_log::read_op_status` against THAT table's
  `info_store()`, not an arbitrarily-chosen one.
- `crates/shamir-db/src/shamir_db/shamir_db/core.rs::get_ddl_op_status` —
  accept `db`/`repo`/`table` params and route to the right `RepoInstance`
  → `TableManager` → `info_store()`, replacing the "first available"
  hack entirely.
- `crates/shamir-client/src/client.rs::get_ddl_op_status` — accept
  `db`/`repo`/`table` params (or infer `db` from whatever the client's
  existing `execute(db, batch)` call already threads — check if `Client`
  has an ambient notion of "current db" at the connection/session level
  that other methods reuse; if so, reuse that convention instead of
  making every caller repeat it).
- `crates/shamir-client-ts/src/core/client.ts::getDdlOpStatus` — same.

**Secondary, smaller finding while you're in `ddl.rs`:** `DdlOpKind`'s
`DropHashIndex`/`DropUniqueHashIndex`/`RenameHashIndex`/
`RenameUniqueHashIndex`/`DropIndex2` variants don't carry `table_name`
(only the `Create*` variants do) — this is an inconsistency, not
strictly required to fix the routing bug above (routing now comes from
the request params, not from inspecting the stored `DdlOpKind`), but
worth adding `table_name` to those variants too for a coherent status
record an operator can actually read without cross-referencing anything
else. Use your judgement on whether this is cheap to include in this
pass or worth a one-line note in your final report as a follow-up.

## Then: finish the original follow-up's remaining items

From the prior brief (`28-p1-2-ddl-result-contract-followup.md`), still
outstanding:

1. **Tests, for real, now that routing is fixed:**
   - A real DROP/RENAME INDEX call (regular + unique + index2) returns a
     `QueryResult` with `op_id` set and `ddl_status: Succeeded`.
   - `GetDdlOpStatus` polling that same `op_id` (with correct
     `db`/`repo`/`table`) finds the log entry.
   - Polling an unknown `op_id` (or the right table but wrong op_id)
     returns `Unknown`/`None`.
   - Polling the RIGHT `op_id` but WRONG `db`/`repo`/`table` — confirm
     this correctly returns "not found" rather than silently returning
     the wrong table's record or panicking (this is the exact case the
     bug above would have gotten wrong).
2. **Gates, run for real and reported with actual output:**
   `cargo fmt --all -- --check` (scope down if needed, say which crates),
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `./scripts/test.sh -p shamir-query-types -p shamir-engine -p
   shamir-db -p shamir-client --full`. Use the wrapper, never raw
   `cargo test`/`cargo nextest run`, never `--lib` for integration tests.

## Explicitly OUT of scope (still)

- Tombstone recovery LOGIC (`recover_hash_renames` /
  `recover_index2_drops` / `recover_in_progress_drops` actual
  `SucceededViaCrashRecovery` writes) — tracked separately as task #1048,
  blocked on this task closing. Don't touch recovery logic.
- Sorted-family, CREATE INDEX status, non-index DDL, version bump — same
  as before.

## Constraints

Same as prior briefs: `CLAUDE.md` conventions, no stray files at repo
root (clean up any scratch `.log`/`.txt` files you create — round 2 left
several at the repo root that the orchestrator had to clean up), no
destructive git commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Definition of done

- [ ] `GetDdlOpStatus` carries explicit `db`/`repo`/`table` routing
      (or reuses an existing ambient-db convention if one exists on
      `Client`) — no more "grab the first db/repo" hack anywhere.
- [ ] A poll for the right op_id + right table finds the record; a poll
      for the right op_id + WRONG table does not (proven by a test).
- [ ] Tests per the list above, all passing.
- [ ] fmt/clippy/test gates green, real output reported.
- [ ] Confirm sub-slice B (tombstone recovery) remains untouched.
