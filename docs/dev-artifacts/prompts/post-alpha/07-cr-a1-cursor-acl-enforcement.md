# Brief: CR-A1 — cursor ACL enforcement (#760)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — SECURITY, verified against the current tree 2026-07-23

`crates/shamir-server/src/db_handler/cursor_handlers.rs::create_cursor`
resolves the target repo/table directly (`resolve_repo`, ~line 208) and
calls `table.read_with_encoding(...)` with **zero** access-control checks.
Compare the normal batch path,
`crates/shamir-db/src/shamir_db/execute/db_execute.rs::execute_as`
(~lines 35-65), which:

1. Calls `self.authorize_access(&actor, &ResourcePath::Database{db: db_name.to_string()}, Action::Read)` once, up front.
2. For every query in the batch, walks `collect_required_access(&request.queries, db_name)` and calls `self.authorize_access(&actor, &path, action)` for each `(Action::Read, ResourcePath::Table{db,store,table})` pair the query tree actually touches.

**`authorize_access`'s own ancestor-walk already covers the whole chain in
ONE call** (`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`,
`authorize_access` ~line 825): calling it with a `ResourcePath::Table{..}`
target internally iterates `path.ancestors()` (Database, then Store),
requiring `Action::Execute` on each, THEN checks the requested `action`
(here `Action::Read`) on the Table itself. `Actor::System`/`Actor::Admin`
short-circuit to `Ok(())` (admin bypass, same as everywhere else). So the
fix needs exactly the SAME TWO calls `execute_as` makes — no more, no
less:

```rust
self.db
    .authorize_access(&actor, &shamir_db::access::ResourcePath::Database { db: db_name.to_string() }, shamir_db::access::Action::Read)
    .await
    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

self.db
    .authorize_access(
        &actor,
        &shamir_db::access::ResourcePath::Table {
            db: db_name.to_string(),
            store: repo_name.clone(),
            table: table_name.clone(),
        },
        shamir_db::access::Action::Read,
    )
    .await
    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
```

(Check the exact import paths — `shamir_db::access::{ResourcePath, Action}`
vs however `cursor_handlers.rs` already imports `shamir_db::access::Actor`
— follow the existing import style in that file, don't introduce a new
one.) **Note the field is named `store`, not `repo`** — `ResourcePath::Table`'s
third field is called `store` (`crates/shamir-types/src/access.rs:377-385`);
`cursor_handlers.rs`'s own local variable is `repo_name` — map it into
`store: repo_name.clone()` correctly.

`authorize_access` is `pub async fn` on `ShamirDb` — already reachable via
`self.db` (the `Arc<ShamirDb>` `ShamirDbHandler` already holds), no
visibility widening needed anywhere.

## Where exactly to add the checks

- **`create_cursor`** (`cursor_handlers.rs`): add BOTH checks (Database
  Read, then Table Read) BEFORE `repo.tx_gate().await`/`gate.open_snapshot()`
  — authorize before touching MVCC state, mirroring `execute_as`'s
  ordering (authorize before `get_db`/execute).
- **`fetch_next`**: the `Cursor` object already stores `db()`/`repo()`;
  `cursor.state()`'s `query.from.table` gives the table name. Decide and
  document: does `fetch_next` need to RE-authorize on every page (in case
  permissions were revoked mid-cursor-lifetime), or is authorizing once at
  `create_cursor` time sufficient (the cursor pins a snapshot anyway, so a
  later permission change wouldn't affect what data it CAN see, only
  whether it SHOULD still be allowed to see it)? Recommendation: re-check
  on every `fetch_next` too — it's cheap (no I/O beyond the existing
  `resource_meta` catalog reads already used by every other authorize call
  in this codebase) and closes the "permission revoked between create and
  fetch" window, which the review didn't explicitly call out but is the
  same class of gap. Add it if the extra check is a small, clean addition;
  otherwise document why once-at-create is the deliberate choice.
- **`cancel_cursor`**: already gated by `CursorRegistry::get_owned`'s
  session-ownership check (a cross-session cancel silently no-ops) — no
  ACL check needed here (you can only cancel your OWN cursor, and you were
  already authorized when you created it).

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`
(the existing FG-5b test file) or a new file if you prefer — follow that
file's existing fixture style (`build_handler_with_rows`, `alice_session`/
`other_session`, `send`/`create_cursor_req` helpers already there):

- **Negative e2e**: create a table owned by Alice with `Action::Read`
  denied to other users (mirror however the codebase's existing ACL tests
  set up a restricted-permission table/user — check
  `crates/shamir-db/src/**/tests/` or `crates/shamir-server/**/tests/`
  for an existing "Bob can't read Alice's table" fixture pattern rather
  than inventing DAC setup from scratch). Bob calls `CreateCursor` against
  that table → the response must be `DbResponse::Error{code: "access_denied", ..}`,
  NOT a `CursorPage`. No cursor must be registered (verify via
  `handler.cursor_registry().len() == 0` or equivalent after the attempt).
- **Positive control**: the same setup, Alice (the owner) creates a cursor
  on her own table → succeeds normally (`DbResponse::CursorPage`).
- If you added re-authorization on `fetch_next`: a test where the actor's
  permission is revoked between `CreateCursor` and `FetchNext` (however
  this codebase's tests simulate a permission change — check for an
  existing "chmod mid-session" test pattern) → the next `FetchNext` also
  gets `access_denied`.

## Gate

```
cargo fmt -p shamir-server -p shamir-db -- --check
cargo clippy -p shamir-server -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-db --full
```

All must pass before returning. Stay inside `shamir-server`'s
`db_handler/cursor_handlers.rs` (+ its tests) — you should NOT need to
touch `shamir-db` at all (`authorize_access`/`ResourcePath`/`Action` are
already public), but if you discover you genuinely do, keep that change
minimal and explain why in your final report.
