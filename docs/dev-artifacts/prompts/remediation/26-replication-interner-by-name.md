# Brief — S3 replication interner translation by name

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.

## FILE SCOPE (only these paths may be edited)

- NEW `crates/shamir-engine/src/tx/replicated_event.rs` (ReplicatedEvent wire type, one primary export) + its re-export in `crates/shamir-engine/src/tx/mod.rs`
- `crates/shamir-engine/src/tx/apply_replicated.rs` (preflight of ids, name-based translation, re-projection with the rewritten bytes)
- `crates/shamir-engine/src/repo/repo_instance.rs` — ONLY a new `read_changelog_for_replication` (the leader side that attaches field bindings); do NOT touch the changefeed init region (another PR edits it)
- `crates/shamir-db/src/**/changelog.rs` (the db-level Pull/changelog helper — Grep to locate), `crates/shamir-server/src/db_handler/repl_handler.rs`, `crates/shamir-server/src/replication/in_process.rs`, `crates/shamir-server/src/replication/follower_loop.rs` (decode `Vec<ReplicatedEvent>`)
- `crates/shamir-tx/src/id_remap.rs` ONLY if `collect_referenced_ids` / `remap_value` do not walk Set values (verify first)
- tests: `crates/shamir-server/src/replication/tests/read_after_replication_tests.rs` (un-ignore, see below) and NEW topic files next to it / in `crates/shamir-engine/src/tx/tests/` (+ one-line `pub mod`)
- NO `changefeed.rs` / `changelog_store.rs` (another PR owns them), NO post-WAL files (`commit.rs`, `drainer.rs`, `materialize.rs`, `version_guard.rs`)

## Task

S3 — replicated records must be readable on a follower (task #1211; replication plan by the design review of 2026-10-11). Your worktree already contains the read-after-replication matrix (`crates/shamir-server/src/replication/tests/read_after_replication_tests.rs`): 28 rows, each marked `#[ignore = "#NNNN"]` with the task that fixes it. CI proved that the rows ignored with `#1211` fail today with "Interned key not found" or read values under foreign field names.

Broken on master: record field keys are u64 ids of the LEADER's repo interner; `ChangelogEvent` carries raw record bytes with no id->name bindings (`crates/shamir-tx/src/changefeed.rs` ~:57-101) and the follower writes them as they are (`apply_replicated.rs` ~:241-256; the doc ~:93-95 says the interner is not replicated). The follower interns names itself (CREATE INDEX `table_manager_index_mgmt.rs` ~:178,191; even reads, `query/read/exec.rs` ~:100-101), so replicated map records fail to decode (`shamir-types` `codecs/interned/codec.rs` ~:155-163) or read under foreign names.

DESIGN (decided; do NOT build snapshot/delta sync and do NOT trust leader ids — `touch_with_id` would collide with locally interned names, the issue #53 class): translate BY NAME, per event.
1. Leader side, on Pull: for each Put op run `shamir_tx::collect_referenced_ids` (`id_remap.rs` ~:99) over the record bytes and resolve each id with `get_str` on the leader's interner -> `field_bindings: Vec<(u64, String)>`. An id with no name is an ERROR response, never silence. Both Pull paths (`repl_handler.rs` and `in_process.rs`, which duplicates `handle_pull` at ~:84-150) must use ONE shared function (`RepoInstance::read_changelog_for_replication`).
2. Wire: `ReplicatedEvent { #[serde(flatten)] event: ChangelogEvent, #[serde(default)] field_bindings: Vec<(u64, String)> }` in the new `replicated_event.rs`. The on-disk journal format is UNCHANGED. Mixed versions are unsupported (alpha); an old leader makes the follower fail closed.
3. Follower: `apply_replicated` takes the bindings. PREFLIGHT before any version allocation: every id referenced by a Put must be present in the bindings, else `Rejected` (fail closed). Then `touch_ind(name)` -> local id for each binding, `repo_interner().persist()` UNCONDITIONALLY (a retry after a failed persist converges because `InternerManager::persist` compares length with `last_persisted_len`, `interner_manager.rs` ~:306-371 — issue #49), then `remap_inner_value_bytes` (`id_remap.rs` ~:73) on every Put, and apply the REWRITTEN bytes. `reproject_for_downstream` (`apply_replicated.rs` ~:441, :455-468) must use the rewritten bytes (so a follower's own re-emitted events carry follower ids).
4. Verify `remap_value` / `collect_referenced_ids` also walk Set values (today: Map keys + List); fix in `id_remap.rs` with a test if not.

Required tests:
(a) the matrix rows ignored with `#1211` are UN-IGNORED and must pass (leader inserts {id, v}, SELECT on the follower matches; today a decode error). Also un-ignore the rows ignored with `#1213`: that fix already merged to master (PR #80); if one of them still fails, report it precisely in your final message instead of re-ignoring it silently. Leave rows ignored with `#1212` untouched.
(b) issue #53: the follower pre-creates an index on a field named `zzz`, taking low ids; today the replicated row reads under `zzz`. Expected: reads under the right name.
(c) issue #49: on-disk follower, a one-shot failure of `set` in the `__interner__` store (pattern `FailingSetStore` in `interner_manager_tests.rs` ~:315): the first apply returns Err WITHOUT consuming a version, the retry is Ok, and after reopening the follower every row decodes.
(d) re-emit from the follower carries follower ids (a downstream consumer of the follower's changefeed decodes the row).
(e) unit tests of the preflight: a Put referencing an id absent from the bindings is Rejected and nothing is applied.
Report the exact list of un-ignored rows and any row that still fails.
