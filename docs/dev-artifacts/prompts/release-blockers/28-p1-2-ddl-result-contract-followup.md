# Follow-up brief — P1-2 (#1015): make sub-slice A actually compile, wire it end-to-end, and gate it green

## Context

Your previous pass on session `t1015-ddl-result-contract` landed real
foundational pieces (wire types, op-status log module, SDK method
stubs, backward-compat test) but **left the workspace not compiling** —
`cargo check --workspace --all-targets` currently fails with **37
`E0063` "missing field" errors** across `shamir-engine`,
`shamir-query-builder`, and `shamir-query-types` test files (struct
literals constructing `QResult`/`QueryResult`/`HashRenameTombstone`
directly, without the new `op_id`/`ddl_status`/`op_id` fields or a
`..Default::default()` spread) — your own report already flagged this
("No Clippy/Fmt/Test Gates Run... cargo check shows errors") but
under-scoped it as a minor blocker; it is not, it means nothing you
built has actually been verified to build or run.

Per this repo's zero-trust delegation discipline, **non-compiling code is
not an acceptable "natural split point" — a legitimate split means
sub-slice A is complete, compiling, and gate-green even if sub-slice B is
deferred.** This pass exists to get sub-slice A to that bar. Sub-slice B
(the three tombstone-family recovery wirings, RFC §2.2/§3.3) is being
tracked as a **separate follow-up task** — do NOT attempt it in this
pass; if you find yourself touching `recover_hash_renames`,
`recover_index2_drops`, or `recover_in_progress_drops`'s actual recovery
LOGIC (as opposed to just making an existing struct-literal compile),
stop, that's out of scope here.

## What "sub-slice A, done" means for this pass

### 1. Fix every compile error

Run `cargo check --workspace --all-targets` yourself first and get the
FULL list — don't assume the 37 count above is exhaustive; more may
surface in `shamir-db`/`shamir-server`/`shamir-client` once the earlier
crates compile (the workspace check short-circuits on first-failing
crates, so downstream crates haven't even been checked yet). For every
`QResult`/`QueryResult` struct-literal construction site that's missing
the new fields: prefer adding `..Default::default()` to the literal
(matches this repo's convention for additive fields) over manually
listing `op_id: None, ddl_status: None` — but if the type doesn't
`#[derive(Default)]`, check why before forcing it (don't blanket-add
`Default` if there's a reason it's missing; if genuinely safe, adding
`#[derive(Default)]` to `QResult`/`QueryResult` is a reasonable, in-scope
fix here since it's exactly what makes additive-field call sites
resilient going forward). For `HashRenameTombstone`'s new `op_id`
field specifically: existing construction sites should pass `None` (no
recovery wiring calls this yet in this pass) unless you find it's
trivial to actually mint a real value at the one or two call sites that
already exist — use your judgement, but do not implement new recovery
logic to populate it.

### 2. Resolve the `info_store()` access blocker

Your report says `ShamirDb::get_ddl_op_status()` calls
`RepoInstance::info_store()` which doesn't exist. Investigate how the
EXISTING tombstone read/write call sites (the ones the RFC references,
e.g. `add_to_renaming`/`load_renaming_list`/`clear_from_renaming` in
`crates/shamir-index/src/base_index/index_manager.rs`, or wherever the
`info_store` this repo already uses for tombstones is actually reached
from) get their `info_store` handle — mirror that exact access path for
`ddl_op_log::write_op_status`/`read_op_status` instead of inventing a new
`RepoInstance::info_store()` public method (unless that turns out to
genuinely be the right layer — investigate before assuming either way).

### 3. Wire `admin_result_with_op_id` into at least one real call site

You added this helper but nothing calls it yet, which makes it dead code
(will trip `-D warnings` under clippy `dead_code`). Per the original
brief's item 4/6, wire it into the DROP INDEX and RENAME INDEX handlers
in `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs` (regular
+ unique + index2 — the three families already in scope for sub-slice A's
wire-contract side) so a real DDL call actually returns a populated
`op_id`/`ddl_status: Succeeded` in its `QueryResult`. Minting the `op_id`
itself at dispatch time (RFC §2.2 step 1) is IN SCOPE here — this is
just "assign an id and stamp it into the synchronous reply", not the
recovery-log `InProgress`-before-first-mutation write (that requires the
tombstone integration, which is sub-slice B). A simple
`RecordId::system()`-minted id passed straight into
`admin_result_with_op_id` for the synchronous-success case is sufficient
for this pass.

### 4. Tests + gates, run for real

- Extend/confirm the backward-compat test suite you already wrote
  compiles and passes.
- Add (or confirm you already have) a test that a real DROP/RENAME INDEX
  call (regular + unique + index2) returns a `QueryResult` with `op_id`
  set and `ddl_status: Succeeded` — now that item 3 above wires it for
  real.
- Add a test that `GetDdlOpStatus` polling an `op_id` that was just
  returned finds the log entry (now that item 2's access path works),
  and that polling an unknown id returns `Unknown`/`None` per the
  contract.
- Run and report REAL output for: `cargo fmt --all -- --check` (scope
  down to touched crates if workspace-wide is too slow, but say which),
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-query-types -p shamir-engine -p
  shamir-db -p shamir-client --full`. Use the wrapper — never raw
  `cargo test`/`cargo nextest run` directly, and never `--lib` when you
  need integration tests too (a prior delegation this session got this
  exact thing wrong).

## Explicitly OUT of scope for this pass (do not touch)

- Any actual tombstone-recovery LOGIC changes in
  `recover_hash_renames` / `recover_index2_drops` /
  `recover_in_progress_drops` — only make existing struct literals
  compile, don't add `SucceededViaCrashRecovery` writes.
- Sorted-family index tombstones/recovery.
- `CREATE INDEX` status wiring.
- Non-index DDL (db/repo/table/schema/function/validator/user/access).
- Any `CURRENT_QUERY_LANG_VERSION` bump.

## Constraints

Same as the original brief: `CLAUDE.md` conventions (tests in `tests/`
dirs, imports at top of file, one-file-one-export), no stray files at
the repo root, no destructive git commands.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Definition of done

- [ ] `cargo check --workspace --all-targets` clean, zero errors.
- [ ] `info_store()` access resolved — `get_ddl_op_status` actually reads
      a real record, proven by a test.
- [ ] `admin_result_with_op_id` called from at least DROP/RENAME INDEX
      (regular + unique + index2) handlers — proven by a test that a real
      DDL call's `QueryResult` carries a populated `op_id`.
- [ ] fmt/clippy/test gates actually run, real pass/fail summary reported
      (not paraphrased, not "should be fine").
- [ ] Report clearly confirms sub-slice B (tombstone recovery wiring) is
      still deferred and untouched — don't accidentally half-implement it
      while fixing compile errors.
