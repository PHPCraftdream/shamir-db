# Worker contract: checked calls — host/admission

## Assignment
Implement the host-side checked-call seam and runtime wiring for the full requirements in `docs/dev-artifacts/prompts/crates/20-call-checked.md`. First read that full brief, `docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md` sections named there, `docs/dev-artifacts/design/shamir-lang-design.md` call/import material, and existing stage-2a host/resolver/active-call seams.

Own `shamir-wasm-host` (including `wasm/admission/gate/lang_compile.rs`, host binding/linker, shared resolver/dispatcher seam, and all host-side `create_shamir_source` tuple destructuring) and `shamir-wasm-admission` (allowlist/signature admission). Update every `create_shamir_source` destructuring outside `shamir-db`, including tests. Do not edit `shamir-db` or `shamir-lang`; coordinate contracts define their owners.

## Fixed shared decisions
- Add default async `StoredCallResolver::resolve_checked(&self,name:&str,parent:&FnCtx,expected_signature:&[u8])->FnResult<ResolvedStoredCall>`; its default fails closed. Leave legacy `resolve` unchanged.
- Add `FnCtx::call_stored_checked(name,expected_signature,batch,params)`. Extend the shared dispatcher with optional expected signature bytes; checked calls must never fall back to registry resolution. Reuse the existing dispatch/depth/fuel/active-call path; do not fork it.
- `create_shamir_source` returns exactly `(WasmFunction,Vec<u8>,ModuleReport,Option<Vec<u8>>)` and uses the already-existing `options.callable_catalog`; `check_shamir_source` stays `ModuleReport`. Preserve reviewed signature/options fields; do not redo them.
- Register/admit `call_checked` together with exact ABI `(i32,i32,i32,i32,i32,i32) -> i64` (six i32 parameters, one i64 result). Checked binding passes guest signature bytes through the shared checked resolver path.
- No new dependencies, version changes, or fuel/lease behavior changes. No tests/builds locally. Do not broaden visibility beyond the seam; no `allow`, local `use`, or logic in `mod.rs`.
- Keep each file under 1000 lines and folders at no more than 7 entries; report any unavoidable existing-layout conflict. No commits are authorized.

## Gate and coordination
The orchestrator owns gates. Do not run tests or builds. If the orchestrator explicitly delegates a local gate, only fmt/check/clippy are permitted, under `wnice -p idle -c 50 -t 8 -m 50 --` and `-j 2` for check/clippy. Do not run other commands. Report touched crates and anything unverified.

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
