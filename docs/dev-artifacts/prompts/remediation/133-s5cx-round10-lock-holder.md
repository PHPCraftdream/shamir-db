# Round 10 — TEST-ONLY CI failure: durable reopen of the follower's Fjall system store never gets the lock

Worktree: `D:/dev/rust/shamir-db/worktrees/s5cx-scoped-quarantine`, branch `feat/s5cx-scoped-quarantine` @ a1d1d839.

REAL CI EVIDENCE (fast tier, ubuntu): everything passes (7372/7373) except
`shamir-server ...::terminal_retry_tests::terminal_marking_retries_until_success`:
panic at `terminal_retry_tests.rs:239` — `reopen_follower_with_lock_retry` exhausted
10 attempts with `lock never released: Storage("FjallError: Locked")` after 3.0 s of
REAL time (each attempt waits inside fjall's own lock retry, which sleeps 100 ms
×3 on a real thread per failed acquire — see fjall-3.1.6 `src/locked_file.rs:53-81`).
A lazy release is therefore EXCLUDED: something still OWNS the follower's Fjall
system store when the test drops its last external handle. The G1 test passes — do
not touch it, do not touch `reopen_follower_with_lock_retry` (keep it as the
lock-ONLY safety net; it stays as-is).

## Root cause (established by reading; your job is to implement the sanctioned fix)

Trace of the fjall-directory-lock holders in this test's flow (all file:line
verified on this worktree):

- The follower is built by `persistent_follower` (`terminal_retry_tests.rs:198-219`):
  Fjall SYSTEM store + in-memory repos. So the only Fjall keyspace is the system
  store, and the surviving holder must reference IT.
- The system repo's changefeed journal writer task owns the `__changelog__`
  partition store until it exits: `crates/shamir-tx/src/changefeed.rs:268-272`
  ("The writer ... owns the receiver and the `Arc<dyn ChangelogStore>`; it exits
  when the `journal_tx` sender is dropped (channel closed) or `shutdown` is set +
  `notify` fired"), spawn at `changefeed.rs:326-342`, final drain on channel close
  at `changefeed.rs:906-919`.
- The writer's exit is normally gated on the channel closing, which requires the
  last `Arc<RepoChangefeed>` (inside the repo's `changefeed` OnceCell,
  `crates/shamir-engine/src/repo/repo_instance.rs:1393-1451`) to drop — which is
  shared with the drainer's BACKGROUND repo clone (`repo_instance.rs:100-115`),
  which exits only after a scheduler tick observes liveness death
  (`crates/shamir-engine/src/tx/drainer.rs:1666-1687`). Under
  `#[tokio::test(start_paused = true)]` that multi-task unwind is not guaranteed
  to complete before the reopen attempts, and CI proves it does not.
- The module contract ALREADY requires the explicit path:
  `changefeed.rs:279-280` — "Exactly one live feed may own a given store's marker
  writer at a time; stop/drain the old writer before re-opening the same store."
- The existing graceful API: `RepoInstance::stop_journal_writer_for_test()`
  (`repo_instance.rs:1636-1641`) → `RepoChangefeed::shutdown_and_wait()`
  (`changefeed.rs:528-540`): sets `shutdown`, then awaits the writer's terminal
  drain via `writer_done`. Precedents for calling it before a drop/reopen:
  `crates/shamir-engine/src/tx/cold_reconcile/tests/follower_cold_tests.rs:78-83`,
  `crates/shamir-engine/src/tx/cold_reconcile/tests/reconcile_tests.rs:65`,
  `crates/shamir-db/src/shamir_db/shamir_db/seed/tests/catalogue_tests.rs:144`.
- The follower's SYSTEM repo is not reachable from shamir-server:
  `SystemStore::system_repo()` is `pub(crate)` (`crates/shamir-db/src/shamir_db/system_store.rs:151`).

Per the round instruction, the sanctioned fix is the explicit graceful shutdown
BEFORE the drop, using the existing API — the retry helper must NOT be what makes
the test pass.

## What to implement

1. FIRST write the brief file `docs/dev-artifacts/prompts/remediation/133-s5cx-round10-lock-holder.md` containing this entire brief (policy block included), so the brief survives in the repo.
2. Product seam (shamir-db, minimal, mirrors the existing `#[doc(hidden)]` test-seam precedent): add to `ShamirDb` (in `crates/shamir-db/src/shamir_db/shamir_db/core.rs`, next to the other public API):
   ```rust
   /// Test-only: stop and drain the SYSTEM repo's changefeed journal writer so a
   /// Fjall-backed system store can be dropped and reopened deterministically
   /// (the writer owns the `__changelog__` partition store until its channel
   /// closes; the explicit shutdown removes that scheduler-tick dependency).
   /// Mirrors `RepoInstance::stop_journal_writer_for_test`.
   #[doc(hidden)]
   pub async fn stop_system_journal_writer_for_test(&self) -> DbResult<()> {
       self.system.system_repo()?.stop_journal_writer_for_test().await
   }
   ```
   Adapt the field path to the actual `SystemStore` field name in `core.rs`. Keep `DbError`/`DbResult` imports consistent with the file. NO other product changes.
3. Test change (`crates/shamir-server/src/replication/seed/tests/terminal_retry_tests.rs`, durable test `terminal_marking_retries_until_success` ONLY): immediately BEFORE `drop(follower);`, add:
   ```rust
   // Deterministic writer stop BEFORE the drop: the system repo's changefeed
   // journal writer owns the `__changelog__` Fjall partition store until its
   // channel closes, and that close is gated on background ticks that do not
   // reliably run before the reopen under `start_paused`. The module contract
   // requires stop/drain before re-opening the same store (changefeed.rs),
   // so use the graceful API instead of waiting. The lock-only retry helper
   // below stays as a safety net; it must not be what makes this pass.
   follower
       .stop_system_journal_writer_for_test()
       .await
       .expect("stop follower system journal writer");
   ```
   Do NOT touch the portable test, G1, the helper, or any other test.
4. Gates, in this order, one cargo at a time, `CARGO_BUILD_JOBS=4`, output to `.scratch/r10-*.log`:
   - `cargo check -p shamir-db -p shamir-server --all-targets`
   - `cargo clippy -p shamir-db -p shamir-server --all-targets -- -D warnings`
   - `cargo fmt -p shamir-db -p shamir-server` then `-- --check`
   - `python -I scripts/ci/check_layout.py`

## 🚦 Compile / build / test policy — MANDATORY, highest priority

**Compilation, builds and tests run on GitHub CI, not on this machine.**
This rule overrides every other section of this file, every skill, every
brief and every habit ("let me just run it once to be sure"). Where any
other text contradicts it, this section wins. It binds the main agent,
every sub-agent and rush executor, and every verifier child.

**Banned unless the user explicitly asks for it in the current
conversation** (a request for one run does not extend to later runs, and an
approval from an earlier session does not carry over):

* `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo t` / `cargo tl`
* `cargo build`, `cargo run`, cargo bench`, `cargo doc`
* `npm` / `tsc` / `vitest` builds and test runs, WASM builds, e2e and
  integration harnesses, loom, any release or full-workspace build
* any other command whose purpose is to build the project or to execute its
  tests or benchmarks

**Allowed locally before a push — only these**, scoped to the crates you
touched (`-p <crate>`, plus dependents when a public signature changed;
`--workspace` only when asked):

* `cargo check -p <crate> --all-targets` — type-check, runs no tests
* `cargo clippy -p <crate> --all-targets -- -D warnings`
* `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`
* `python -I scripts/ci/check_layout.py` — the instant layout gate

Never push code that fails `check`, `clippy` or `fmt`. Everything else is
CI's job, and `./scripts/test.sh` is executed **in the PR's CI**.

**Delegated work.** Every brief for a sub-agent, rush executor or verifier
child carries this policy verbatim, including the allowed list. A child that
believes it needs a build or test run says so in its final message; the
orchestrator decides whether to ask the user.

The sections below that mention `./scripts/test.sh`, `cargo t` / `cargo tl`
or benches describe HOW they are run — in CI, or locally only on the user's
explicit request. They never grant permission to run them.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, `git add`, `git commit`, `git merge`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. Never use the shell builtin `xargs`. Scratch only under `.scratch/`. No new `#[allow]`. One cargo process at a time, `CARGO_BUILD_JOBS=4`. If a cargo command dies with 0xc0000409, rerun the unchanged command ONCE and disclose it.
