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

# Round 8 — shamir-server O2/O4 only

Work only in `crates/shamir-server/**` and this brief. Do not run cargo commands, tests, builds, or benchmarks. No sleep/polling, nested agents, dependency changes, `#[allow]`, git mutations, commits, or edits outside scope. The orchestrator runs gates centrally. Preserve accepted R7-6 apply park and startup semantics; do not alter false Notify or startup flush allegations.

## O2 — original supervisor stale-handle sweep lifecycle tests

In `replication/seed/tests/lease_gate_tests.rs`, make both A1 lifecycle tests deterministically require the ORIGINAL supervisor stale-handle sweep (`supervisor.rs`: qualify active/same profile/generation, remove stale handle, cancel/join, then skip start for registered name).

Test 1 must perform real Pause and Resume between reconcile ticks while the original handle remains registered. Never wait for deregistration before Resume, manually remove registry state, use a new supervisor, sleep, or use yield budgets.

Test 2: explicitly park the old loop after observing actual follower apply bookmark/notify and outside the apply guard, with no control lock held. Keep it registered until the stale sweep. Read actual production follower pull cancellation and teardown. Do not substitute parking a wrapper's second pull unless cancellation releases it during sweeping. On current-thread runtime, omission of the sweep must leave the name registered/start skipped and the new bookmark unchanged. Assert the new lease/applied notification as appropriate; use bounded, non-hanging progress signals and RAII cleanup if needed. No fake registry state or mocked sweep.

Read `SubHandle::stale()`, `stop()`, cancellation/top apply guard, and teardown behavior before changing tests.

## O4 — portable terminal retry test

In `terminal_retry_tests.rs`, remove module-wide `#![cfg(unix)]` coverage loss. Add a portable test observing exactly 3 attempts, 2 failures, and 1 completed real flush success via the existing production supervisor marking seam, reading before disarm. Keep the durable Fjall reopen leg Unix-only and document why. Prefer a shared async exercise helper taking follower/setup if practical; minimize duplication. Whole-row state alone is not retry proof. Do not alter accepted counters, startup lease path, or fixture single-stream admission. All platforms' imports must remain cfg-clean. Counterfactual single-attempt behavior must record 1/1/0 and fail the assertion.

## Final report

Include exact test names and absolute file paths/line numbers, plus source-derived counterfactuals. No local behavioral tests. Only report what was inspected/changed.