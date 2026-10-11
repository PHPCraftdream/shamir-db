# Brief — #1216 ValidatePayload config

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

- `crates/shamir-server/src/replication/follower_loop.rs`
- `crates/shamir-server/src/replication/supervisor.rs` and `crates/shamir-server/src/replication/mod.rs` (only if the config must be threaded through them)
- `crates/shamir-server/src/config.rs` — ONLY a new replication opt-in field (and its default + doc comment) in the replication-related config section; another agent adds pagination ceilings in a different section of this file concurrently, do not touch those
- the replication guide that documents `ApplyPolicy` (Grep for `ApplyPolicy::Trusted` / `4.4` under `docs/guide-docs/`), and `deploy/server.example.ktav` (one commented example line)
- tests: a new topic file in `crates/shamir-server/src/replication/tests/` (or the existing tests location for `follower_loop`; Grep for `follower_loop` tests) plus its one-line `pub mod`

## Task

#1216 — wire `ApplyPolicy::ValidatePayload` through the server config.

Observed on master: the production follower hard-codes `ApplyPolicy::Trusted` (`crates/shamir-server/src/replication/follower_loop.rs` ~:337-344, see the `#1199` comment there). `ApplyPolicy::ValidatePayload` (decode-validation of every Put payload BEFORE a version is allocated; defined in the engine's `apply_replicated` path, Grep `enum ApplyPolicy`) exists and is unit-tested at the `apply_replicated` level only; neither `FollowerLoopConfig` nor the server config can select it.

Required result:
1. A real opt-in config flag in the server config's replication section (default = current behaviour, `Trusted`; name it for what it does, e.g. `validate_replicated_payload`, and document the cost/benefit in its doc comment), threaded through the replication supervisor to `FollowerLoopConfig` and used by `run_follower_loop` to choose the policy passed to `apply_replicated`.
2. A test that proves, on the follower-loop path (not only in the `apply_replicated` unit test), that with the flag ON an undecodable Put payload from the leader is rejected without allocating a version and the loop's existing failure handling runs, while with the flag OFF behaviour is unchanged. Reuse the in-process leader/follower fixtures that the existing follower-loop tests use.
3. Update the replication guide's `Trusted` paragraph and add one commented example line to `deploy/server.example.ktav`.
4. No change to the wire protocol or to the engine's `apply_replicated`.
