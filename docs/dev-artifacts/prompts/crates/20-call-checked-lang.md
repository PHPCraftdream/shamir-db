# Worker contract: checked calls — language compiler

## Assignment
Implement compiler signature metadata, `call` checking, effects/diagnostics, lowering, emission, and compiler tests for the full requirements in `docs/dev-artifacts/prompts/crates/20-call-checked.md`. First read that full brief and the exact named sections of `docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md` and `docs/dev-artifacts/design/shamir-lang-design.md`, plus existing compiler/options/catalog seams. Own only `crates/wasm/shamir-lang`. Do not edit host, admission, database, or engine files.

## Fixed shared decisions
- Do not redo the already-reviewed signature/options fields. Consume the existing callable catalog input; keep compilation synchronous and I/O-free.
- Compile canonical signature descriptor bytes per the brief for callable procedure entries; scalar and validator entries have no callable signature. Enforce literal target, exact named args and declared types/nullability, exact return type, capability/effect propagation and cycle/depth/boundary diagnostics using existing codes (T004/T007/T008/T009), adding none unless unavoidable.
- Emit `call_checked` with exact ABI `(i32 name_ptr,i32 name_len,i32 signature_ptr,i32 signature_len,i32 args_ptr,i32 args_len)->i64`; embed name/signature, use shared encoder/codec, project result as specified. Document/report compiler import ceiling. Preserve main-design import-count limit.
- No dependencies/version/fuel changes. Tests/builds run in CI, not here. Keep existing pinned production dependencies and mutation/purity guards. Do not delete/weaken tests; no `allow`, local `use`, or logic in `mod.rs`. Keep <=7 entries per folder and <=1000 lines per file. No commits are authorized.

## Gate and coordination
The orchestrator owns gates. If explicitly delegated a local gate, only fmt/check/clippy are permitted, under `wnice -p idle -c 50 -t 8 -m 50 --` and `-j 2` for check/clippy. Report chosen compile-result metadata shape, import ceiling, changed existing tests and why, touched crates, and anything unverified.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

## Compile / build / test policy — MANDATORY, highest priority

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
  `gh workflow run ci.yml --ref <branch>` forces the full tier on a draft,
  and so does the explicit marker `[ci full]` alone on a line of the head
  commit message (see "CI tiers"). Nothing else triggers a full run on a
  draft: the full matrix is kept for the end of a sprint.
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

---
