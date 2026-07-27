# Brief for F-33 Step 5 (#839, P1) — full end-to-end proof: config is genuinely LIVE post-restart

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-33's hybrid repo backend is fully wired at this point:
- Step 1 (#835, `606eb6f3`): `MirroredStore` + the `is_durable_table_config`
  classifier.
- Step 2 (#836, `c32c4e3d`): `BoxRepo::Hybrid`/`BoxRepoFactory::Hybrid`
  routing in `shamir-engine`.
- Step 3 (#837, `af4053da`): proved `TableManager::create`'s open path
  tolerates a hybrid-repo restart at the low level (direct
  `BoxRepoFactory::hybrid(...)` construction, no DDL) — index defs survive
  but postings don't, validator bindings survive, the interner resolves
  the SAME field name to the SAME id post-restart (raw id equality check),
  record counter reads 0, no spurious `repair()`.
- Step 4 (#838, `606a8b48`): wired `ENGINE 'hybrid'` into the real `CREATE
  REPO` DDL surface. `hybrid_repo_ddl_tests.rs`'s
  `hybrid_repo_config_survives_restart_but_data_is_ephemeral` proved (via
  `DescribeTable`) that an index definition and a schema validator survive
  a full `ShamirDb` restart while an inserted row does not.

**What's still missing, and what this step closes**: every proof so far
checks that surviving config is *readable* post-restart (an index
definition appears in `iter_indexes()`/`DescribeTable`; a validator
binding's `len()` is non-zero). None of them prove the config is
*genuinely live* — that a schema validator actually still REJECTS a bad
write post-restart, and that a surviving index is actually still USABLE
for a brand-new write post-restart (which is also the strongest possible
functional proof of interner coherence: if the repo-level interner
resolved `"name"` to a different id than the one baked into the surviving
index definition, a post-restart insert-then-query-by-index would
silently miss the row, not just fail to error).

## What to build

Extend `crates/shamir-db/src/shamir_db/tests/hybrid_repo_ddl_tests.rs`
(the file Step 4 created) with new tests, reusing its established
`reinit_with_retry` restart helper and query-builder-only DDL style (never
hand-built JSON, per this repo's rule):

1. **Validator genuinely enforces post-restart.** Session 1: `CREATE REPO
   ... ENGINE 'hybrid'`, set a schema requiring a field (mirroring the
   existing test's `ddl::field(["name"]).string().required().build()`).
   Session 2 (after restart): attempt an insert that VIOLATES the schema
   (omit the required field, or wrong type) and assert the write is
   REJECTED with a validation error — not merely that `DescribeTable`
   still lists the rule. Also insert a row that SATISFIES the schema and
   confirm it succeeds — proving the validator is actively running, not
   just present as inert metadata.
2. **Surviving index is genuinely usable for new writes (functional
   interner coherence).** Session 1: `CREATE REPO ... ENGINE 'hybrid'`,
   create an index on a field, insert a row, confirm a query filtered by
   that field/index finds it (pre-restart baseline). Session 2 (after
   restart): insert a DIFFERENT row with a NEW value on the same indexed
   field, then query filtered by that field/value — the query MUST find
   the new row. This is the critical proof: if the post-restart interner
   assigned a different id to the field name than the one the surviving
   index definition was built against, this query would silently return
   zero results (or hit the wrong index) instead of erroring — the
   strongest possible black-box proof that interner coherence holds all
   the way through the real query path, not just via a raw id-equality
   assertion (which is all Step 3 checked, at a lower level).
3. **No stale postings, DDL-level.** Session 2: query for the value that
   was inserted in Session 1 (pre-restart) via the SAME index — confirm 0
   hits (the row is gone, data is ephemeral). This closes the loop at the
   DDL level to match Step 3's lower-level version of the same assertion,
   through the real query surface this time.
4. **Two full restart cycles.** Extend (or add a variant of) one of the
   above so the repo survives TWO restarts in a row (session 1 → restart →
   session 2 writes new data + reads old config → restart → session 3),
   confirming the config doesn't degrade/drift on repeated reattach (e.g.
   accidentally re-persisting something that shouldn't survive, or losing
   something that should, on the SECOND reattach specifically — a
   plausible gap a single-restart test wouldn't catch, e.g. if some
   initialization path only behaves correctly on a "cold" open and not on
   a "warm-then-cold-again" one).

## If any of this reveals a genuine gap

Fix it in whichever production file the bug actually lives in — most
likely `crates/shamir-storage/src/storage_mirrored.rs` (a classifier tag
gap), `crates/shamir-engine/src/repo/repo_types.rs` (a `HybridRepoComposite`
routing/memoization bug), or `crates/shamir-engine/src/table/table_manager.rs`
(an open-sequence ordering issue only observable when combined with the
DDL surface's own state, e.g. how `handle_create_repo`/schema DDL persist
things differently than the hand-built Step 3 harness did). Document the
exact gap and fix precisely — this campaign has repeatedly found real bugs
during exactly this kind of "does it actually work end-to-end" pass, so
do not assume all 4 scenarios above will pass cleanly; investigate for
real rather than writing tests you expect to pass.

If everything holds: no production file changes needed — only the new
tests in `hybrid_repo_ddl_tests.rs`.

## Constraints

- Query builder only for all DDL/writes/queries in the new tests — never
  hand-assembled JSON (`serde_json::json!`/raw `Value`).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -- --check` and
  `cargo clippy -p shamir-db --all-targets -- -D warnings` must be clean.
- Do not touch Steps 1-4's already-landed code unless a genuine gap is
  found and precisely documented.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- hybrid
./scripts/test.sh -p shamir-db --full
```
