# Brief — P1-1 (#1014): offline `shamir-server doctor` CLI subcommand

## Context

S.H.A.M.I.R. Database, `crates/shamir-server` (CLI binary) + `crates/shamir-engine`
(`TableManager::verify()`/`repair()`, `crates/shamir-engine/src/table/doctor.rs`).
An adversarial review (2026-08-05 §P1-1) found that `verify()`/`repair()` exist
and are recommended by log/metric messages ("run doctor repair"), but there is
NO operator-facing way to invoke them — confirmed by grep: zero references to
`verify`/`repair`/`doctor` anywhere under `crates/shamir-server/src`. Tracked
as task #1014.

## What already exists — mirror this pattern, don't invent a new one

`crates/shamir-server/src/main.rs`'s `Subcmd` enum (`main.rs:76-161`) already
has three OFFLINE subcommands that open a `data_dir` directly (no running
server, no network):
- `Backup { to: PathBuf }` → `shamir_server::backup::backup` (main.rs:214-223)
- `Restore { from: PathBuf, force: bool }` → `shamir_server::restore::restore`
- `AccessTree { depth, db, pretty, connect, ... }` → `shamir_server::access_tree::run`
  (main.rs:241-261) — this one is the closest sibling: it walks a whole data
  dir's DB/repo tree offline and pretty-prints a report, exactly the shape
  `doctor` needs. Read `access_tree.rs` in full before writing anything —
  copy its offline-boot pattern (`ShamirDb::init(SystemStoreConfig)`,
  `crates/shamir-db/src/shamir_db/shamir_db/core.rs:140`) and its
  `--pretty`/plain-JSON output toggle.

`TableManager::verify()` (doctor.rs:171, read-only) and `::repair()`
(doctor.rs:509, mutating self-heal) are the two methods to invoke. Their
report shapes (`VerifyReport`/`RepairReport`/`IndexHealth`/`Index2Health`,
doctor.rs:36-166) are already `Serialize`/`Deserialize` — JSON output is a
direct `serde_json::to_string_pretty` away.

No multi-table enumeration helper exists yet for an offline tool — you'll
write the traversal: DB → repo → `RepoInstance::list_table_names()`
(repo_instance.rs:428) → `RepoInstance::get_table(name)` (repo_instance.rs:310)
→ `.verify()` (and `.repair()` if `--apply` is passed) per table. Look at how
`access_tree.rs` enumerates DBs/repos today (it walks the SAME data dir tree
for a different purpose — access control, not table health) — reuse whatever
top-level DB/repo listing it already does rather than re-deriving it from
`system_store.rs` yourself if `access_tree.rs` already has it.

`IndexHealth.name_interned` is a raw `u64`, not a resolved string — resolve it
via the table's interner for human-readable output, mirroring how
`access_tree.rs`'s pretty-printer resolves names via the interner (find that
exact pattern and copy it).

## The new subcommand

```
shamir-server doctor --config <ktav> [--db <name>] [--repo <name>] [--table <name>] [--apply] [--pretty|--json]
```

- Default (no `--apply`): read-only `verify()` across every table matched by
  the `--db`/`--repo`/`--table` filters (all three optional — omitted means
  "every DB" / "every repo" / "every table" respectively, mirroring
  `AccessTree`'s existing `--db` filter pattern).
- `--apply`: after reporting, additionally run `repair()` on every table
  whose `verify()` reported `!is_healthy()`. Print a before/after summary
  (the `RepairReport` fields already carry `counter_before`/`counter_after`
  and per-family rebuilt counts — surface them). **Do NOT auto-repair a
  HEALTHY table** — `repair()` is a full drop+recreate of every index, not a
  no-op on an already-healthy one; gate it on `!is_healthy()` explicitly.
- Output must show, per table: `Building`/`Failed` indexes (via `IndexHealth`/
  `Index2Health`'s `state`), the `counter_consistent` flag,
  `index2_registry_consistency` and `cross_family_name_collisions` diagnostic
  strings (already plain human-readable text, per doctor.rs — just print
  them), and any recovery-tombstone state if `verify()` surfaces it (check —
  if it doesn't currently, note that as a gap in your final report rather
  than inventing new doctor.rs API to expose it; this brief is about wiring
  the EXISTING surface, not extending `doctor.rs` itself).
- Exit code: `0` if every table is healthy (or became healthy after
  `--apply`), non-zero otherwise — this needs to be scriptable for
  operator/CI health checks, not just human-readable.

## Constraints

- Follow `CLAUDE.md`: no inline `#[cfg(test)] mod tests {}`, tests in
  `tests/` directories.
- This is CLI/binary code (`crates/shamir-server`) — `anyhow`/`Box<dyn Error>`
  at the boundary is fine per this repo's error-handling conventions (library
  code must not leak `anyhow`, but `shamir-server`'s `main.rs`/CLI layer
  already uses it — match the existing style in `main.rs`/`backup.rs`).
- Do NOT add a network/authenticated admin API endpoint in this pass — the
  task's own wording says "at least one" interface; the offline CLI route
  is lower-risk (no new wire-protocol/auth surface) and matches three
  existing precedents. If you think the admin-API route is clearly better,
  say so in your final report instead of silently doing both or switching
  approaches — that's an escalation, not a unilateral scope change.
- Gate: `cargo fmt -p shamir-server -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-server --full`. Add at least one integration
  test proving: (a) a healthy data dir → exit 0, clean report; (b) a data
  dir with a deliberately Building/stuck index (use the same test-hook
  pattern `doctor_tests.rs`/`f76_drop_visibility_tests.rs` already use if
  reachable from `shamir-server`'s test binary, otherwise construct the
  unhealthy state directly via `shamir-engine` test helpers) → non-zero
  exit, report correctly flags it; (c) `--apply` actually heals it and a
  SECOND `doctor` run reports healthy.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Definition of done

- [ ] `shamir-server doctor` subcommand added, following the `AccessTree`
      offline-boot + `--pretty`/JSON pattern.
- [ ] Traverses DB → repo → table (with optional `--db`/`--repo`/`--table`
      filters), calling `verify()` on each.
- [ ] `--apply` flag triggers `repair()` only on unhealthy tables, reports
      before/after.
- [ ] Report shows Building/Failed indexes, counter consistency, index2
      registry consistency, cross-family collisions.
- [ ] Exit code reflects overall health (scriptable).
- [ ] At least 3 integration tests per the scenarios above.
- [ ] fmt/clippy/tests green, exact commands and results reported.
- [ ] If you concluded the admin-API route is actually necessary too/instead,
      say so explicitly rather than silently expanding scope.
