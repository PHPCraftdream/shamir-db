COMPILE / BUILD / TEST POLICY (owner rule 2026-10-18) — read first; it overrides anything else in this brief.

You may now run LOCALLY, scoped to the crates you touched (-p <crate>, plus dependents when a public signature changed), and you MUST run them until clean before you report: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>`, and `python -I scripts/ci/check_layout.py`. Run one cargo process at a time and set CARGO_BUILD_JOBS=4 (several sessions share this machine). Tests, builds, benches and everything else stay banned — they run in the PR's CI. Fix every compiler/clippy message these commands print; do not leave them for CI. The policy text verbatim (it also binds every child agent you start — paste it into each child brief):

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
* `cargo build`, `cargo run`, `cargo bench`, `cargo doc`
* `npm` / `tsc` / `vitest` builds and test runs, WASM builds, e2e and
  integration harnesses, loom, any release or full-workspace build
* any other command whose purpose is to build the project or to execute its
  tests or benchmarks

**Allowed locally before a push — only these**, scoped to the crates you
touched (`-p <crate>`, plus their dependents when a public signature
changed; `--workspace` only when asked):

* `cargo check -p <crate> --all-targets` — type-check, runs no tests
* `cargo clippy -p <crate> --all-targets -- -D warnings`
* `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`
* `python -I scripts/ci/check_layout.py` — the instant layout gate

Never push code that fails `check`, `clippy` or `fmt`. Everything else is
CI's job, and `./scripts/test.sh` is executed **in the PR's CI**:

* Push to a branch and open a draft PR: the fast tier (fmt, layout, clippy
  and `./scripts/test.sh` lib tests of the affected crates) runs.
  `gh pr ready` runs the full matrix (`./scripts/test.sh` and the
  integration run on Ubuntu/Windows/macOS), which is the merge gate;
  `gh workflow run ci.yml --ref <branch>` forces the full tier on a draft.
* A failing test, hang or flake is diagnosed from the CI log
  (`gh run view <id> --log-failed`), fixed in code and re-pushed. It is not
  reproduced locally; reading code and logs is the debugging tool. When a
  flake needs repeated runs, the loop runs in CI (a workflow job), not here.
* The fast tier stops at the first failing crate. Before pushing, read every
  changed file for the whole class of error (module paths after a move,
  imports, privacy, `cfg(test)` gating, unused items) instead of letting CI
  reveal them one at a time.

**Delegated work.** Every brief for a sub-agent, rush executor or verifier
child carries this policy verbatim, including the allowed list. A child that
believes it needs a build or test run says so in its final message; the
orchestrator decides whether to ask the user.

The sections below that mention `./scripts/test.sh`, `cargo t` / `cargo tl`
or benches describe HOW they are run — in CI, or locally only on the user's
explicit request. They never grant permission to run them.

=== END OF POLICY ===


---

# Round 9 TEST-ONLY fixes — branch s5cx-scoped-quarantine @ 743fa35b

Only edit `crates/shamir-server/src/replication/seed/tests/**`. No production
changes, no `#[allow]`, no new deps, no git mutation, no nested agents.

## G1 — lease_gate_tests.rs:422 `pause_flush_failure_stops_live_follower_and_never_restarts`

CI (ubuntu fast tier): panic at :295 `wait_until: condition not reached
within the yield budget` from `wait_until_deregistered` (:464). Root cause:
after the failed PAUSE invalidates the lease, the run-forever loop is parked
INSIDE `source.pull` (leader-side long-poll, `with_poll_wait_ms(100)`;
`follower_loop.rs` pull at :478-486 is a bare await, NOT selected against
cancel), then returns empty events, takes the IDLE_BACKOFF_MS=50ms sleep
(:638), and only then hits the top-of-iteration lease check and exits. The
10,000-yield budget in `wait_until` (:288-296) burns through in far less wall
time than that 150ms of timer waits on a current-thread runtime — the
deregistration wait deterministically starves. Production is correct (the
lease IS invalidated; the loop DOES stop); this is test synchronisation only.

FIX: drive the loop to a deterministic park point with the existing test
hook. Build an `ApplyParkHook { repo: Some("main".into()), after_apply: true,
arrived, release, exited: Notify::new() }` and `with_apply_park` it on `sup`
(mirror the A1 tests at :499-509). Sequence: reconcile → await
`arrived.notified()` (parked AFTER apply, holding NO apply guard —
follower_loop.rs:702-708) → `wait_until_bookmark_covers` v1 → arm flush
fault → `pause_op_future(...).await` must be Err → assert row `paused` and
lease `Some(false)` (already present) → `release.notify_one()` (the park
select!s on release/cancel, follower_loop.rs:208-228; the loop resumes, sees
the invalid lease at the top-of-iteration check and exits; the wrapper then
deregisters) → `wait_until_deregistered` completes promptly. Keep the final
fault-drop + reconcile-never-restarts assertions. Update the test doc comment
to state WHY the after-apply park is needed (the pre-round-8 passing budget
depended on scheduler timing inside the leader long-poll; the park gives a
notify-based deterministic handoff).

## G2 — terminal_retry_tests.rs:376 `terminal_marking_retries_until_success`

CI: `reopen persistent follower: Storage("FjallError: Locked")`. The fjall
directory lock is released only lazily after the LAST `Arc<ShamirDb>`/store
clone drops, and some background handles release it asynchronously.

FIX: (a) before the reopen, `sup2.stop_all().await; sup.stop_all().await;`
(joins any live handles; the loops already exited via max_iterations but this
is the documented stop path), noting the `with_start_race_hook` closure's
`hook_follower` clone drops with `sup2` (after `stop_all`); (b) replace the
single `ShamirDb::init(...).expect("reopen persistent follower")` with a
bounded retry helper `reopen_follower_with_lock_retry`: up to 10 attempts of
`ShamirDb::init(SystemStoreConfig::Fjall(meta))`, retry on any `Err(_)`
(keep last error), `tokio::time::sleep(Duration::from_millis(50)).await`
between attempts (mirrors `crates/shamir-engine/src/tx/tests/
recovery_tests.rs:19-40 reopen_disk_repo` and `crates/shamir-engine/src/tx/
clean_horizon/tests/repair_pending_tests.rs:142-153 open_retry`, whose
comments document the lazy lock release after `drop`); panic with the last
error after the budget. Comment: the lock is held until the last store clone
drops and is released lazily; `stop_all` + drops above release the clones,
the retry absorbs the lazy release.

## Audit (no edits unless an ACTUAL defect is found)

Hand-trace every other new Unix-gated test on this branch for the same
classes (state not set up before reconcile/join; leaked Arc clones before a
reopen; raw yield budgets where a notify exists):
`seed/tests/loop_tests.rs` (#![cfg(unix)]) and the three Unix-gated tests in
`binding_scope_tests.rs` (`resume_releases_stale_same_name_claim_after_recovery`,
`set_profile_alone_preserves_complete_row_through_reopen`,
`pause_and_set_profile_persist_exact_durable_row`) plus
`quarantine_reads_tests.rs` t13/t15 + recovery helpers. Verify
`persistent_follower` callers: the portable test must use `build_db` (it
does) and the durable test keeps `create_profile_and_subscription` at :273.

## Layout ratchet

`lease_gate_tests.rs` is 995 lines — must stay <=1000 AFTER `cargo fmt`.
Trim only NEW comments if needed; never delete unrelated assertions.

## Gates (only these; ONE cargo at a time, CARGO_BUILD_JOBS=4)

`cargo check -p shamir-server --all-targets`, `cargo clippy -p shamir-server
--all-targets -- -D warnings`, `cargo fmt -p shamir-server`,
`python -I scripts/ci/check_layout.py` — output to .scratch/r9-*.log, report
real inner exit codes. NO tests, NO other builds.
