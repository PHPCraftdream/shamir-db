# S5c-X PR8b-2 W3 — CI evidence matrix (tests)

Master brief: `docs/dev-artifacts/prompts/remediation/120-s5cx-scoped-quarantine.md`, appendix §5 (the required CI evidence table is copied below). Stages W1+W2 are already in the working tree: durable binding on every row, typed open validation (`DbError::SubscriptionCatalogue`, code `subscription_catalogue_invalid`), ArcSwap exact-key quarantine cache (no global sentinel), binding-based terminal coverage, generation-aware supervisor, and guards for rebind/same-name/DROP/RENAME. READ the W1/W2 diffs (`git diff` is read-only and allowed; do NOT mutate git state) and the new `crates/shamir-db/src/repl_binding/` module before writing tests.

## Rules (binding)

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. (`git diff`/`git status`-style READ-ONLY inspection is fine.)
- NO compile/test/lint commands of any kind. Only `cargo fmt -p shamir-db -p shamir-server` as your LAST step. CI is the gate — tests must be written to compile first time: copy helper patterns from the existing quarantine tests (`crates/shamir-db/src/shamir_db/execute/replication_seed/tests/quarantine_reads_tests.rs`) instead of inventing new ones, and resolve every builder/API name by reading its definition.
- Never silence a lint with `#[allow]`. Gate test-only imports with `#[cfg(test)]` where the file is not already test-only.
- Layout ratchet: ≤7 entries per folder; ≤1000 lines per file. `execute/replication_seed/tests/` has 4 entries → you may add up to 3 new files there (wire each into its `tests/mod.rs`, which is a manifest only). `crates/shamir-db/src/shamir_db/tests/` (27 entries) and `crates/shamir-server/src/replication/tests/` (8 entries) are OVER the limit — edit existing files only, never add. `quarantine_reads_tests.rs` is 741 lines → keep it ≤1000.
- Imports at file tops (test files included, except `use super::*`-style module-local imports in test modules). Queries only through `shamir-query-builder` (`Batch`, `Query`, `subscription`, `replication_profile`, `repl_scope`, `alter_subscription`, `drop_subscription`, `insert`, `doc!` — copy the existing helpers).
- No machine-specific absolute paths. Temp dirs for persisted tests come from the repo's existing tempfile conventions (grep how `durable_repo_tests` / the PR8b restart test build `SystemStoreConfig::Fjall` paths).

## Required evidence (verdict §5 table — implement every row)

| # | Required case | Acceptance result |
|---|---|---|
| 1 | Quarantine `A/main`; read `A/side` and populated `B/main` | Only `A/main` refuses; unrelated reads return their actual data |
| 2 | One subscription, multiple destinations across databases | Every bound destination refuses; everything outside the set stays readable |
| 3 | Delete / corrupt / recreate the profile after binding | Original bound destinations remain blocked, including after reopen |
| 4 | Missing new fields; malformed or unsupported binding | Typed open refusal, including active AND paused legacy rows |
| 5 | Profile update, SetProfile, same-name creation, stale terminal callback | No destination escapes the durable binding; no replacement binding is mutated by an old generation |
| 6 | Terminal persistence/flush failure | Affected live reads remain blocked; unrelated destinations stay readable |
| 7 | Two quarantined subscriptions overlap | Clearing one does not clear the other's protection |
| 8 | Quarantined destination DROP/RENAME and subscription DROP | Refused before any live or durable mutation |
| 9 | Valid replacement seed and Resume | Only destinations proven recovered become readable; stale receipt/state flips do not suffice |

Plus retained coverage (already exists from PR8b — VERIFY these still exist and still assert the right thing; extend only where the mechanism changed): nested batches, interactive transactions opened before quarantine, direct table acquisition (the raw-handle limitation of `table_management.rs:367–379` stays an explicit non-goal), journal reads, cursors/live receivers, downstream Hello/Pull, seed export.

## Where each test goes

- NEW `crates/shamir-db/src/shamir_db/execute/replication_seed/tests/binding_scope_tests.rs` — rows 1, 2, 7. Reuse the `setup()`/`seed_row()`/`read_users()`/`quarantine()` helper patterns (copy them in; make `quarantine()` take destinations so one subscription can bind `testdb/main` + a second db). Assert refusal via `read_users`' `BatchError.code() == Some("resync_required")` and changelog `DbError::ResyncRequired`; assert unrelated reads return the seeded row's actual data (parse the response value, not just `is_ok`).
- NEW `.../binding_lifecycle_tests.rs` — rows 3 (in-process leg: `DROP REPLICATION PROFILE` after binding → bound destination still refused, refresh rebuilt), 5 (SetProfile to an equal-destination profile SUCCEEDS and keeps the gate; SetProfile to a different-destination profile is refused with `binding_destination_mismatch`; same-name `subscription(...)` creation refused with `already_exists`; profile replacement to a different destination set refused while referenced), 8 (DROP of a quarantined subscription refused with `resync_required` and the row still present afterwards; `ShamirDb::rename_repo_as` / `rename_db_as` / `remove_repo` / `remove_db` typed refusal while a binding references the destination — call the facade APIs directly; `remove_db` now returns `DbResult<bool>`).
- NEW `.../binding_open_refusal_tests.rs` — row 4. Build a PERSISTED system store (`SystemStoreConfig::Fjall` under a `tempfile` dir — copy the pattern from the existing PR8b restart/reopen test; if that test is Unix-gated, check whether the reopen leg itself needs `#[cfg(unix)]` and say so in the test doc), write subscription rows DIRECTLY into `system/subscriptions` via `set_via_implicit_tx` (legacy row: no binding fields; malformed row: bogus `binding_destinations`; unsupported version row; one `active`, one `paused`), reopen with `ShamirDb::init` and assert the typed error (`code() == Some("subscription_catalogue_invalid")`) naming the subscription. Also the negative control: a fully valid new-format row reopens fine.
- Row 6 — terminal persistence failure. Add a `#[cfg(test)]` fault seam to `crates/shamir-db/src/shamir_db/shamir_db/seed_catalogue.rs::flush_seed_catalogue` (a test-only `AtomicBool` + two `#[cfg(test)] pub(crate)` arm/disarm helpers, ~10 lines, file is 73 lines — well within limits). Test in `binding_lifecycle_tests.rs`: arm the seam → `mark_subscription_resync_required` → expect `Err`; assert the bound destination is STILL refused (prepublished keys survived the failed persistence) and an unrelated destination is still readable; disarm, re-mark successfully, assert the gate now reflects the persisted row.
- Row 3 reopen leg + row 9 — extend `quarantine_reads_tests.rs` IN PLACE (it already holds the unix-gated `recovery` module with the real installer + Hello probe + the full reseed lifecycle). Add: (a) after quarantine, `DROP REPLICATION PROFILE` then reopen (persisted config) → bound destination still refused; (b) the stale-flip legs if not already asserted: `Resume` without a replacement seed is refused; Pause/SetProfile on a quarantined row refused; only the real reseed + `Resume` lifts the gate for exactly the bound destinations. Do not duplicate what the existing recovery tests already prove — read them first.
- Row 5 server leg (stale terminal callback) — `crates/shamir-server/src/replication/tests/supervisor_tests.rs` (edit in place; folder over limit): call `mark_subscription_resync_required_generation(name, gap, from, WRONG_GENERATION)` directly on a quarantined-capable subscription → assert the row's `state` did NOT change and the cache did not grow; then the correct generation → state flips. This is the honest in-process proof of the generation check (no timing races).
- `crates/shamir-db/src/shamir_db/tests/replication_ddl_tests.rs` (over-limit folder, edit in place; file is 824 lines → ≤1000): W1 already replaced the global-sentinel test — verify the replacement asserts BOTH refusal of the bound destination AND real data returned for unrelated repos; extend it with the recreate-profile leg (recreate `profile_a` with DIFFERENT scopes → row keeps its original binding; the ORIGINAL destinations stay blocked and the new scopes are NOT quarantined).

## Conventions

- One topic per file; every new file wired into `crates/shamir-db/src/shamir_db/execute/replication_seed/tests/mod.rs` (manifest-only re-export line per file).
- In tests, JSON literals multi-line and indented; use the document macro (`doc!`) as the existing tests do.
- Every test name should say what it proves (`binding_scope_...`, `binding_open_refusal_...`, `stale_generation_callback_does_not_mark_replacement`, ...). Print a final mapping table: evidence-table row → test name(s) → file.
- `wc -l` for every touched file at the end (all ≤1000; `replication_seed/tests/` folder at 7 entries exactly after your additions — count before adding).
- Final step: `cargo fmt -p shamir-db -p shamir-server`.
