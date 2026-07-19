# Deadlock hazard — `per_table_mvcc` and `token_names` mix scc `_async`/`_sync` lock acquisition (DDL-under-load window)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## Background — read the two prior commits in this same sweep first

`git show 7a4abf62` (the original `#589` `cells`-map fix) and the two
already-landed commits in this exact task series (search `git log --oneline
--grep "H1+H2"` and `--grep "H3"`) establish the mechanism and the doc-
comment style to mirror. Short version: `scc::HashMap`'s `_async` lock-wait
(`entry_async`, `read_async`, `insert_async`, ...) is a lock-HANDOFF — on
release, the bucket lock is granted directly to a suspended waiter TASK,
which then holds it while sitting in tokio's run queue until re-polled.
Synchronous accessors on the SAME map (`entry_sync`, `read_sync`,
`insert_sync`, `remove_sync`, `iter_sync`, `get_sync`, ...) instead PARK the
calling OS thread while the bucket is locked. Mixing the two on one map
risks a whole-runtime deadlock if enough workers park on the same bucket
during a handoff window.

**This task's hazard is a DIFFERENT shape than the prior two fixes in this
series** — H1/H2/H3's `_async` operations were all EXCLUSIVE acquisitions
(read-modify-write or insert/remove), so any suspended waiter directly
blocked the map. Here, the `_async` operations on `per_table_mvcc` and
`token_names` are all SHARED (read-only) acquisitions — so the deadlock
needs an EXCLUSIVE writer in the mix (a DDL op: table attach or drop table)
concurrent with sustained commit/drain traffic. This is reported at MEDIUM
confidence (not HIGH like H1/H2), since it requires two DDL ops overlapping
sustained commit load, plus (unverified against the vendored `scc` version)
writer-fairness behavior where new shared acquirers queue behind a pending
exclusive writer. **Fix it anyway — the fix is trivial, mechanical, and
strictly convention-aligning regardless of how easy the hazard is to
trigger; do not skip the fix because the confidence is MEDIUM rather than
HIGH.**

## Site 1 — `RepoInstance::per_table_mvcc` (`crates/shamir-engine/src/repo/repo_instance.rs` + call sites in `crates/shamir-engine/src/tx/`)

Map: `per_table_mvcc: Arc<scc::HashMap<u64, Arc<MvccStore>, THasher>>` — per
repo, shared, declared at `repo_instance.rs:34`, exposed via `pub fn
per_table_mvcc(&self) -> &Arc<scc::HashMap<...>>` at `repo_instance.rs:275`.

**Async shared-acquisition call sites (convert these):**
- `crates/shamir-engine/src/tx/pre_commit.rs` — grep for
  `.per_table_mvcc()` in this file; at least one site is a plain `let
  mvcc_map = repo.per_table_mvcc();` followed by per-table lookups inside
  the pre-commit loop (every tx pre-commit runs through here) — confirm
  whether it uses `read_async` at this call site or a sibling one and
  convert whichever `_async` accessor is used.
- `crates/shamir-engine/src/tx/commit_phases.rs` — `repo.per_table_mvcc()
  .read_async(&table_id, |_, mvcc| std::sync::Arc::clone(mvcc)).await`
  (~line 510-512).
- `crates/shamir-engine/src/tx/drainer.rs` — `repo.per_table_mvcc()
  .read_async(table_id, |_, m| std::sync::Arc::clone(m)).await` (~line
  488-490, every drain step) AND a separate `iter_async` if one exists on
  this map in `run_gc` — grep `drainer.rs` and `repo_instance.rs` for
  `per_table_mvcc().iter_async`/`.iter_sync` to find both; `repo_instance.rs`
  already has `self.per_table_mvcc.iter_sync(...)` at one site (~line 1347,
  `flush_all_history`) and possibly `iter_async` at another (~line 1439,
  `run_gc`) — if `run_gc` uses `iter_async`, convert it to `iter_sync` to
  match the ALREADY-sync convention at line 1347 on the very same map.
- `crates/shamir-engine/src/tx/recovery.rs` — `repo.per_table_mvcc()
  .read_async(&table_id, |_, m| std::sync::Arc::clone(m)).await` (~line
  585-587).
- `crates/shamir-engine/src/tx/apply_replicated.rs` — `repo.per_table_mvcc()
  .read_async(&token, |_, mvcc| std::sync::Arc::clone(mvcc)).await` (~line
  215-217).

**Sync accessors already on this map (the reason the above must be sync too):**
- `crates/shamir-engine/src/repo/version_provider.rs:13-14` —
  `self.per_table_mvcc.read_sync(&table_id, |_, mvcc| mvcc.version_of(...))`
  — this is the **SSI `validate_read_set` commit hot path**, i.e. runs on
  every serializable-isolation commit.
- `crates/shamir-engine/src/tx/commit.rs` — `mvcc_map.get_sync(&token)` (in
  the pessimistic-lock-release path, ~line 824-826).
- `crates/shamir-engine/src/repo/repo_instance.rs:491` —
  `self.per_table_mvcc.get_sync(&from_token)` (rename-table path).
- `crates/shamir-engine/src/repo/repo_instance.rs:1347` —
  `self.per_table_mvcc.iter_sync(...)` (`flush_all_history`, drainer
  truncation gate).
- `crates/shamir-engine/src/repo/repo_instance.rs:376` —
  `self.per_table_mvcc.insert_sync(token, ...)` (table attach — the
  EXCLUSIVE writer that makes the hazard reachable).
- `crates/shamir-engine/src/repo/repo_instance.rs:456` —
  `self.per_table_mvcc.remove_sync(&token)` (drop table — the OTHER
  exclusive writer).

### Fix

Convert every `read_async`/`iter_async` call listed above (pre_commit.rs,
commit_phases.rs, drainer.rs, recovery.rs, apply_replicated.rs — and
`repo_instance.rs`'s `run_gc` if it uses `iter_async`) to the corresponding
sync accessor (`read_sync`/`iter_sync`). All the closures involved are
`Arc::clone` (or, for `version_provider.rs`, already sync) — bounded, no
suspension — so this is mechanical.

## Site 2 — `RepoInstance::token_names`

Map: `token_names: Arc<scc::HashMap<u64, String, THasher>>`, declared at
`repo_instance.rs:42`.

**Async shared-acquisition call sites (convert these):**
- `table_by_token` (`repo_instance.rs:~1175-1186`) — `self.token_names
  .read_async(&token, |_, name| name.clone()).await` — used by the commit
  pipeline (Phases 1/2.6/5b-5d under `commit_lock`) and V2 WAL recovery.
- `table_by_token_if_live` (`repo_instance.rs:~1212-1220`) — `self
  .token_names.read_async(&token, |_, name| name.clone()).await` — called
  by `pre_commit_prelock`'s Phase 2.5 barrier check, specifically chosen
  (per its own doc comment) to be non-instantiating — confirm your fix
  does not change that non-instantiating property, only the lock
  mechanism.

**Sync accessors already on this map:**
- `register_token` helper (~`repo_instance.rs:1507-1509`) —
  `token_names.insert_sync(token, name.to_string())` +
  `token_names.read_sync(&token, |_, n| n.clone())` (collision-check path,
  DDL — table create/rename).
- `repo_instance.rs:441` — `.remove_if_sync(&token, |existing| existing
  .as_str() == table_name)` (drop table).

### Fix

Convert `table_by_token`'s and `table_by_token_if_live`'s `read_async` calls
to `read_sync`. Same mechanical shape — the closure is a `String` clone.

## Concrete failure mechanism (both sites, same shape)

1. A DDL op (`insert_sync`/`remove_sync`/`remove_if_sync` — table attach or
   drop) owns a bucket exclusively for a moment.
2. Concurrent commit-pipeline/drainer `read_async`/`iter_async` waiters
   suspend on that bucket.
3. On release, saa hands SHARED grants to the suspended reader tasks — they
   hold read locks while sitting unpolled in the run queue.
4. A SECOND DDL op (or repo-close teardown) parks a worker in
   `remove_sync`/`insert_sync` waiting for those unpolled readers to
   release. IF saa applies writer-fairness (new shared acquirers queue
   behind a pending writer — the standard anti-starvation behavior; not
   independently verified against the vendored `scc` version here, treat
   as the working hypothesis), every subsequent commit-path `read_sync`/
   `get_sync` parks BEHIND the pending writer too → workers drain into
   parks → the originally-granted-but-unpolled reader tasks are never
   polled → deadlock.

Confidence MEDIUM (requires two DDL ops, or DDL + writer-fair reader queue,
overlapping sustained commit traffic) — but the fix is trivial and strictly
convention-aligning, so apply it regardless of how hard the window is to
trigger deterministically in a test.

## Tests

1. A test exercising DDL (table attach via `add_table`/`get_table`, and
   drop table) CONCURRENT with sustained commit/drain traffic on the SAME
   repo, under a constrained worker-thread count (`worker_threads = 1` or
   `2`), wrapped in a NAMED bounded `tokio::time::timeout` (mirror the
   established style from the H1+H2/H3 test files in this series) —
   confirming no hang.
2. Given H4/H5 require an exclusive writer in the mix (harder to trigger
   deterministically than H1-H3's pure hot-key pattern), it is OK if a
   deterministic repro is not reliably achievable — if so, say so
   explicitly in your summary and explain WHY the sync fix removes the
   hazard (the reasoning above) rather than forcing a flaky stress test to
   "prove" something inherently racy. Still include at least one test that
   exercises the interleaving in good faith (per the established pattern
   in this series), even if it can't be guaranteed to reproduce the exact
   pre-fix hang on every CI run.
3. Existing tests exercising table attach/drop, commit, drain, and
   recovery must continue to pass unchanged.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @engine @oracle --full` green (check `scripts/test.sh`'s
  `scope_args` for the exact scope names if these don't match; report
  which scope you actually used).
- `cargo fmt --all -- --check` clean (or scoped to `shamir-engine`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) EVERY `read_async`/`iter_async` call site on both
  maps you found via grep is now converted — enumerate them, (b) no
  existing observable behavior of `table_by_token`/`table_by_token_if_live`/
  the commit pipeline/drainer/recovery/replication-apply paths changed
  beyond the lock-acquisition mechanism (in particular, confirm
  `table_by_token_if_live`'s non-instantiating property — its whole
  documented reason for existing — is unaffected).

## Out of scope

- Do NOT touch `MvccStore::cells` (#589), `RepoTxGate::active_snapshots`/
  `MvccStore::locks` (H1+H2), the 5 vector-index maps or `rid_map` (H3 and
  its follow-up), or `layered_interner.rs` (a separate, already-tracked,
  lowest-priority follow-up task, H6).
- Do NOT change any DDL/commit/drain/recovery ALGORITHM — this task is
  entirely about lock-acquisition mechanism on `per_table_mvcc` and
  `token_names`, nothing else.
- Do NOT raise any test timeout to paper over a hang — if you observe a
  real hang during testing, root-cause it (per this session's standing
  "hunt and fix hangs, never tolerate" discipline) rather than loosening a
  timeout.
