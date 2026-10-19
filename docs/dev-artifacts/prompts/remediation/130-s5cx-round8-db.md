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

IMPLEMENT db Round8 O1/O3/O5. Root D:/dev/rust/shamir-db/worktrees/s5cx-scoped-quarantine. NO cargo/tests/sleeps/nested children; central gates by parent. Only edit shamir-db and brief below. ⛔ NEVER run git reset / checkout / clean / stash / restore / rm, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. User forbids commits too. NO git staging/commit/merge/push. Use edit/write, never shell for file mutation. First save brief docs/dev-artifacts/prompts/remediation/130-s5cx-round8-db.md with literal .scratch/POLICY_VERBATIM.md prefix and task body from THIS prompt. New brief file need not already exist: write creates it. Read-before-edit only applies existing files. Do NOT block saying task body lacks source file; it is your received prompt. If actual edit/write tools absent report that exact limitation. Read instructions and rust-intel skill at C:/Users/Computer/.agents/skills/rust-intel/SKILL.md with relevant modules, Cargo.lock pins. Layout <=1000 source lines <=7 entries ratchet; no allow/deps/incidental refactor. Preserve all accepted Round7 changes. Finish within20min.
O1: admin_replication.rs replace_subscription_row useless error.into -> error; repl_binding/tests/pending_protection_tests.rs header doc lint no allow.
O3: separate SetProfile ALONE test in replication_seed/tests/binding_scope_tests.rs. Capture COMPLETE initial row, expected row changes only profile, perform SetProfile, fresh equality, drop and reopen Fjall system store, COMPLETE row equality. No intervening Pause, retain Pause test.
O5: trace admin_replication mark publishes claim name:G:token before SET/interner/flush confirms only afterward; replacement replication_seed/catalogue.rs bumps G2 preserving quarantined; Resume gathers only G2 claims; quarantine_cache.rs install preserves unmatched. Verify statically, then resolve same subscription stale-generation claims only after authoritative durability/recovery success at rebind/Resume. Never clear B owner, never ambiguous starts_with matching, preserve exact mark tokens, preserve pending on failed/cancelled persistence. Prefer minimal private API. Regression uses actual failed-flush mark A at G, real image/rebind G2, Resume succeeds clears A while B own pending claims remain. Must isolate A release too: resolve B afterward and prove destination serves; B cannot hide A leak. Can reuse quarantine_reads_tests::recovery real install/rebind/probe, single-stream scopes. Fault seam in shamir_db::shamir_db available. B may be cache-level injected exact claim if real terminal B would make competing owner invalid; explain. Each new test counterfactual names missing fix and failed assertion. Report files:lines, cancellation/error reasoning, test names. No local behavioral execution.
