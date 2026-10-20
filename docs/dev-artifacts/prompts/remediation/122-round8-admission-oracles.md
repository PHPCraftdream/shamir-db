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
* any other command whose purpose is to build the project or to execute
  its tests or benchmarks

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

=== END OF POLICY ===

You are sole implementation worker for ROUND 8 task121 PR#189 in worktrees/s5cx-retire-legacy-follower. Current HEAD cb1337b8 clean. Do NOT delegate further. Deadline: stop editing within 75 minutes and report partial if needed. User bans xargs, cargo clean, git add/commit/merge, #[allow]. No git mutation, no push. Scratch .scratch/. One cargo process CARGO_BUILD_JOBS=4; retry identical command once after 0xc0000409/memory crash, report inner exits not shell echo exits.
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
User forbids commits this round; do NOT follow repository prompt-commit rule. First persist this complete brief in docs/dev-artifacts/prompts/remediation/122-round8-admission-oracles.md (verify parent exists); do not commit. Read AGENTS/CLAUDE instructions and rust-intel SKILL + testing/semantics modules before Rust edits. Verify Cargo.lock versions, references before shared API changes. Layout <=7 direct entries, <=1000 lines, never grow over-baseline files. No unrelated refactors or comments. Do NOT touch CI-proven raw-kind plumbing core.rs/seed/catalogue.rs/seed/gate.rs or rename e2e. CI user evidence run39204614179: all integration OS passed; only four R7 tests fail exactly as below.

IMPLEMENT one cohesive bounded fix:
1 shared repository-wide typed capability validator next to BoxRepo::supports_atomic_batch in crates/shamir-engine/src/repo/repo_types.rs. Return DbError::AtomicBatchUnsupported naming destination/remedy. Expose deliberately via RepoInstance forwarding (existing changelog_store/follower_durability.rs is appropriate, not huge repo_instance.rs). Use actual BoxRepo capability, NEVER Store capability. Reuse validator engine admission too if appropriate to avoid inconsistent remedy.
2 Wire DB CREATE SUBSCRIPTION + RESUME BEFORE mutations/probes, seed bind/rebind/resume control.rs and installation catalogue.rs. Paths:
crates/shamir-db/src/shamir_db/execute/admin_replication.rs CREATE ~302–361, Resume ~494–560;
execute/replication_seed/control.rs bind_seed_creation ~182–215 validate_seed_resume ~218–300;
execute/replication_seed/catalogue.rs install_seed_subscription ~138–193.
Keep existing seed checks/locks/quarantine semantics. Typed batch API code atomic_batch_unsupported (derive established DbError mapping convention rather than strings). SeedRefusal may need typed variant/code and conversion: inspect carefully, don't flatten unsupported to IncompleteInstall or generic error. Avoid duplicate scope loops if shared seed control can service admin routes, but cover ALL paths user named including ordinary unseeded CREATE/RESUME, directions only Pull, wildcard repo scopes if supported. Type error DbError::AtomicBatchUnsupported real remedy.
3 Supervisor ~376–407 uses SAME repo-wide validator before source/spawn. Preserve typed ReplError terminal status resync_required; generic message must not lie Hybrid is MemBuffer. Hybrid __tx__ is raw in-memory true but BoxRepo false. Tests must show refusal even no event/source constructed.
4 gate_tests.rs Unix test seed reboot: event74 caller bookmark must be durable73, expected Applied local2 (seed local baseline1). Keep Repo import cb1337b8. Assert actual record data if accessible to prove apply, bookmark74 and baseline intact. Trace real project_event prepare_event watermark skip, seed writer/gate allocator paths in final.
5 stall_visibility_tests.rs transport expected strings include replication transport error: for leader unreachable/streak two. EmptyBacklogSource events must use real production encoder rmp_serde::to_vec_named(&Vec::<ReplicatedEvent>::new()) (read in_process.rs:280 first), no handcrafted bytes. Preserve success-empty/backlog oracle exact.
6 follower_loop.rs successful no-progress must compare actual durable post-apply bookmark vs prior upstream bookmark, NOT local_version vs upstream. Handle read errors consistently generic visibility/typed terminal rules, avoid claiming unchanged without read. Ensure progress resets counters/latch correctly immediately or loop next read, no double count. Add/adjust regression proving local allocation and upstream domains differ if feasible existing file.
7 streak test must observe full threshold10 initial persisted stall THEN bookmark advancement AND intermediate progress restored message BEFORE fresh10 failure streak. Deterministic synchronization/state inspection in source pull (inline before later pull succeeds) or channels, no sleeps to race. Ensure leader journal event readiness deterministic, virtual time can cause journal writer races: use production durability checkpoint or existing bounded helper as appropriate; never create indefinite handshake. Test fails if overwrite removed even reset remains; assert fresh count10 after.
8 Add true API CREATE MemBuffer typed refusal + no subscription row mutation, true ALTER RESUME MemBuffer typed refusal + row unchanged (fixture may need seed paused catalogue row deliberately through SystemStore persistence since CREATE now refuses), Hybrid supervisor terminal/no source/no-spin. Existing server atomic_batch_gate_tests.rs helper currently EXPECTS CREATE success on incapable repo! Rewrite fixture so supervisor tests deliberately inject a pre-existing legacy row without bypassing function under test, explicit fixture separation. Use query builder for all queries. Maintain types/codes tests not just message. Counterfactual in tests (user explicitly requests comments). Cover seed binding validator if needed existing DB replication_seed tests, don't gratuitously new files beyond layout.

Gates on final implementation, sequential with full logs + real inner exits:
CARGO_BUILD_JOBS=4 cargo fmt -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server
same fmt -- --check
CARGO_BUILD_JOBS=4 cargo check -p shamir-storage -p shamir-engine -p shamir-db -p shamir-server --all-targets
same clippy --all-targets -- -D warnings
python -I scripts/ci/check_layout.py
Use .scratch/r8_worker_*.log. Fix compiler/clippy until clean without scope creep. Do NOT run tests/builds. Parent will independently inspect and rerun gates, then separate full verifier AFTER last edit. Final: changed files+lines, detailed four-test hand traces expected outcomes, new regression counterfactuals, every gate inner exit incl failed attempts, any limitations. Do not claim behavioral execution. No final verifier claim yourself.
