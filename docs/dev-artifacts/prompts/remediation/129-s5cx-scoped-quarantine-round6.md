# 129 — S5c-X scoped quarantine, round 6: real marker removal, retry/lifecycle defects

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

TOOLING: never use the shell builtin `xargs` (it crashes the whole rush session); all scratch logs under `.scratch/` (git-ignored), never the repo root or `target-scratch/`; ONE cargo process at a time; `CARGO_BUILD_JOBS=4` — never 1. No `git add`/`git commit`/`git reset`/`checkout`/`clean`/`stash`/`restore`/`rm`; no `cargo clean`; the orchestrator commits and runs all gates.

## Root cause (round-5 review verdict)

Resume removes the durable quarantine marker with `shift_remove` on
`quarantined`/`status_message`/`resync_from_receipt` and persists through
`SetOp` — but `SetOp` MERGES (`execute_set_tx` merge branch,
`shamir-engine/src/table/write_exec.rs:1254`; omitted keys' old bytes are
copied verbatim, `shamir-types/src/codecs/interned/messagepack.rs:647-653`).
The marker survives and `repl_binding/coverage.rs:15-17` keeps the
destination quarantined. This, not the pending-key merge-back removal, is
the root cause of the four "still ResyncRequired after rebind + Resume" CI
failures.

## Round-6 items

1. (P1) Replace-record semantics for Resume and the test helper (remove the
   three fields durably; durable-row test after reopen).
2. (P1) Terminal retry must guard on generation currency, not `lease.valid`
   (the mark itself invalidates the lease: `admin_replication.rs:945`,
   `repl_binding/lease.rs:134-138`); B3 test must inject failures at the
   TERMINAL persistence, not the start barriers.
3. (P1) Startup lease must be RAII — every early exit / cancellation after
   issuance cleans the lease up.
4. (P1) Multi-target exit: reference-count live targets; only the LAST exit
   invalidates/removes the shared lease + handle.
5. (P1) Quarantine cache: a pending key is removed ONLY by an explicit
   confirmation after the owning mark's SET + interner persist + flush all
   succeeded (or a definitive documented rollback) — never by an unrelated
   refresh.
6. (P1) Overlap tests become COVERAGE-AGGREGATION tests; rule stated in the
   `check_owners` comment; real-writer exclusion kept.
7. (P1) Test defects: unbounded follower joins, wrong T7 oracle, manual
   handle removal in T3/A1, readiness not persistence, stale lifecycle
   expectations (`binding_lifecycle_tests.rs:372,392-404`).
8. (P2) `ensure_drop_allowed` checks ALL destinations; refusal names the
   ACTUAL operation; blocker flags include cache-pending state.
