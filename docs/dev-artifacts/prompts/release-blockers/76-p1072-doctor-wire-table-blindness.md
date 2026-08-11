# Brief 76 — #1072 (CRITICAL): `doctor` doesn't see wire-created tables, exits 0, tests are tautological

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## The defect (F-3, F-4, F-5 — verified directly against the code)

`crates/shamir-server/src/doctor.rs::run` (~line 179) opens the data
directory via `ShamirDb::init(SystemStoreConfig::Fjall(meta_path))` and
immediately starts scanning `shamir.list_dbs()` → `db.list_repos()` →
`repo.list_table_names()` (~line 236). It NEVER replays
`TablesRegistry` — the small MessagePack file at
`<data_dir>/wire_tables.mpack` that records every table a wire client
created via `BatchOp::CreateTable`
(`crates/shamir-server/src/tables_registry.rs:3-10` explains why this
file exists: `shamir-db`'s system store tracks databases/repos but NOT
per-table config; `RepoInstance::add_table` is in-memory-only).

The REAL boot path does this replay explicitly —
`crates/shamir-server/src/server/server_launcher.rs:414-437`:
```rust
let tables_registry = Arc::new(TablesRegistry::open(&config.data_dir)?);
{
    let snap = tables_registry.snapshot();
    for (db_name, repo_name, table_name) in snap.iter_entries() {
        if let Some(db) = shamir.get_db(db_name) {
            if !db.has_table(repo_name, table_name) {
                if let Err(e) = db.create_table(repo_name, table_name) {
                    tracing::warn!(db = db_name, repo = repo_name, table = table_name, ?e,
                        "tables_registry replay: create_table failed");
                }
            }
        }
    }
}
```
`doctor::run` skips this entirely and `crates/shamir-server/src/main.rs`
(~line 323, `shamir_server::doctor::run(&config, &args).await?`) calls it
directly — no boot path in between.

**Failure scenario**: `repo.list_table_names()` returns EMPTY for every
table a client ever created over the wire — i.e. practically every
production table. Hits the `table_reports.is_empty()` branch
(`doctor.rs:297-304`): prints "No tables found in the database.",
`return Ok(())` — **exit code 0**. An operator running
`shamir-server doctor --apply` after an incident (or wiring it into CI/a
health check, exactly as the module doc promises: *"Exits with non-zero
if any table is unhealthy"*) gets a clean bill of health while corrupted
indexes sit untouched. This is fail-open in the tool that exists
specifically to be the last integrity check.

**Why the tests didn't catch it (F-4)**:
`crates/shamir-server/tests/doctor_e2e.rs` — all 4 tests assert exactly
`result.is_ok()`. `doctor::run` returns `Ok(())` on the "no tables
found" branch too, so `doctor_with_table_succeeds` passes PRECISELY
BECAUSE the bug exists: `setup_data_with_table` creates a table over the
wire, doctor can't see it, `Ok(())` comes back anyway. Further proof
nobody read the actual output: `doctor_filter_options_work`
(`doctor_e2e.rs:139-157`) filters by `repo: Some("default")`, but the
real production repo is named `"main"`
(`server_launcher.rs:405`, `RepoConfig::new("main", ...)`) — this filter
cannot match ANYTHING, ever, and the test still passes.
`doctor_json_output_works` never captures stdout at all. The suite
cannot distinguish a working doctor from a stub
`async fn run(..) -> anyhow::Result<()> { Ok(()) }`.

**F-5, same file, adjacent fix**: `doctor.rs:349-354` calls
`std::process::exit(1)` from the middle of an async function on the
unhealthy branch. `process::exit` does NOT run destructors — open
Fjall/redb handles and their background flush tasks never drop. On the
`--apply` path, `table_mgr.repair()` (`:259-261`) just rebuilt indexes
right before this forced exit — meaning unflushed buffers and, on
Windows, a leftover lock file that traps the NEXT `doctor` invocation.

## The fix

1. **Replay `TablesRegistry` before scanning**, in `doctor::run`, mirroring
   `server_launcher.rs:414-437` EXACTLY (same `TablesRegistry::open`,
   `snapshot()`, `iter_entries()`, `has_table`/`create_table` guard, same
   `tracing::warn!` on a failed replay — don't invent a different shape).
   Do this AFTER `shamir` is opened, BEFORE the `for db_name in
   shamir.list_dbs()` scan loop.
2. **Filter-no-match must be non-zero.** The `table_reports.is_empty()`
   branch (`doctor.rs:297-304`) currently returns `Ok(())` in BOTH
   sub-cases (genuinely empty database, and an explicit `--db`/`--repo`/
   `--table` filter that matched nothing). Split them: a genuinely empty
   database (no filter args set) can stay `Ok(())` — an empty db isn't
   unhealthy. An explicit filter (`args.db.is_some() ||
   args.repo.is_some() || args.table.is_some()`) matching zero tables
   MUST return a non-zero exit / `Err` — the operator asked for a specific
   table and got silence, which is exactly the kind of "looks clean but
   isn't" failure this whole task is about.
3. **Replace `std::process::exit(1)` with a proper `Result`-based exit.**
   `main.rs`'s `fn main() -> anyhow::Result<()>` already gets a non-zero
   process exit for free when it returns `Err` — Rust's `Termination`
   trait handles this AFTER normal unwinding, so destructors (open
   Fjall/redb handles, background flush tasks) run cleanly before the
   process actually exits, unlike `process::exit` which skips them
   entirely. Change `doctor::run`'s return type/signature so the unhealthy
   case propagates as an `Err` (or use `std::process::ExitCode` if this
   codebase's CLI convention elsewhere prefers that — check
   `crates/shamir-server/src/main.rs`'s other subcommands for the
   established pattern before picking) all the way up through
   `main.rs:323`'s `?`, and do NOT call `std::process::exit` anywhere in
   `doctor.rs` — including for the filter-no-match case in fix #2 above,
   which needs the SAME clean-exit mechanism, not a second
   `process::exit` call.

## Tests — `doctor_e2e.rs` needs a full rewrite, not a patch

Per this task's own requirement: **every new/changed test must FAIL on
the current HEAD (before your fix)** — verify this yourself by
temporarily reverting your `doctor.rs` changes locally, confirming the
test goes red, then restoring the fix. Do this for at least the two
tests marked ⚠️ below and report the outcome.

Rewrite `crates/shamir-server/tests/doctor_e2e.rs`:

1. ⚠️ **`doctor_with_table_succeeds` (or a renamed equivalent)** — after
   `setup_data_with_table()`, assert the report actually SAW the table:
   parse/inspect enough of the result to assert `total_tables > 0`
   (whatever field/structure the report exposes — check `DoctorReport`'s
   fields in `doctor.rs`, you may need `doctor::run` to expose more than
   `anyhow::Result<()>` for tests to inspect, OR capture+parse the printed
   JSON output via `args.json = true` and assert on the parsed structure —
   pick whichever is less invasive to the production code path, but the
   test must observe a NONZERO table count, not just `is_ok()`). This is
   the test that currently passes for exactly the wrong reason — it must
   fail on unfixed HEAD (doctor sees 0 tables) and pass once the
   `TablesRegistry` replay is wired in.
2. **Fix `doctor_filter_options_work`'s repo name** — change
   `repo: Some("default")` to `repo: Some("main")` (or read whatever
   `setup_data_with_table` actually creates — check `RepoConfig::new(...)`
   in that helper) so the filter can genuinely match. Assert the filtered
   report actually contains `test_table`.
3. **`doctor_json_output_works`** — capture the printed JSON (redirect
   stdout, or refactor `doctor::run` to optionally return the JSON string
   for testability — check whether other CLI-subcommand tests in this
   codebase already have an established stdout-capture pattern before
   inventing one) and parse+validate its structure: table count, at least
   one index entry with a resolved name, `healthy` field present.
4. ⚠️ **New test: explicit filter matching nothing → non-zero exit /
   `Err`.** Run doctor with `table: Some("this_table_does_not_exist")`
   against a data dir that HAS tables (via `setup_data_with_table`), and
   assert the result is `Err` (or whatever non-zero signal fix #2 above
   produces) — NOT `Ok(())`. This must fail on unfixed HEAD.
5. **New test: a corrupted/unhealthy table produces a non-zero exit
   without `--apply`.** Check whether an existing corruption-injection
   helper already exists in the engine crate's own doctor/verify tests
   (search `crates/shamir-engine/src/table/tests/` for anything that
   deliberately breaks an index's entry count or state before calling
   `verify()`/`repair()`) and reuse that pattern rather than inventing a
   new corruption mechanism — mirror how you'd corrupt a table through
   the SAME `TableManager` surface those tests already use, then run
   `doctor::run` against that data dir and assert non-zero.
6. Keep `doctor_empty_data_dir_succeeds` as-is (genuinely empty, no
   filter — should still be `Ok(())` per fix #2's carve-out) but double
   check its assertion still makes sense after your `Result`-shape change.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

(`--full` is required here — `doctor_e2e.rs` lives under
`crates/shamir-server/tests/`, which only runs under `--full`/integration
mode, not the default lib-only `./scripts/test.sh -p shamir-server`.)

Paste the actual final summary line (pass/fail counts) — literal output,
not a paraphrase. List every test you touched/added by name with
individual pass/fail status, and the outcome of the mandatory
revert-and-check for the two ⚠️ tests. If anything fails, fix it before
reporting done. This is a release-blocking CRITICAL defect — the standard
is that everything you report is something you personally watched pass,
with the command's actual output as evidence, not an assumption.
