# Task 121 round 7 — backend-capability briefs (S5c-X PR6b-3, ready PR #189)

CI run 39196052188 on b7b96035 (round 6 merged): the 3 seed_recovery_e2e
scenarios STILL stall, but the round-6 stall visibility did its job — the
subscription row now shows `status_message = "follower stalled: 10
consecutive failures without progress; last error: Repository-wide atomic
batch is not supported by this backend: follower repo main requires the
receipt apply protocol but its backend does not support repository-wide
atomic batches"` — i.e. `DbError::AtomicBatchUnsupported` from
`receipt/resume/activation.rs:55-80`, retried forever. ROOT CAUSE:
`BoxRepo::supports_atomic_batch` (`repo/repo_types.rs:304-317`) is true
only for InMemory and raw Fjall; `BoxRepoFactory::fjall(path)` is
MemBuffer-wrapped by default (`repo_types.rs:480-501`); the seed staging
uses RAW Fjall (`replication_seed/install/staging.rs:60,117`), but
`register_seed_image` saves the repository kind `"fjall"`
(`shamir-db/src/shamir_db/shamir_db/seed/catalogue.rs:188-197`) and boot
maps `"fjall"` to the WRAPPED factory
(`shamir-db/src/shamir_db/shamir_db/core.rs:756-763`). A seeded follower
reopened from disk therefore lands on a backend that can never run the
receipt protocol. The round-6 JCLN theory was refuted (the reviewer showed
`StoreChangelog::range_from` already filters metadata,
`changelog_store.rs:126-131`) and that guard was reverted — do NOT
reintroduce it.

CHOSEN FIX (orchestrator decision): option (a) — a distinct catalogue kind
`"fjall_raw"` that boot maps to `BoxRepoFactory::fjall_raw`. Why (a): raw
Fjall is exactly what the staged image already runs on, so reopening on the
same backend is provably correct by reading; (c) (making MemBuffer/Cached
batch-capable) is a large storage change whose cache/buffer invariants
cannot be proven in this window; (b) (a replica flag) is redundant once a
kind string exists.

## Policy + tooling (pasted VERBATIM into every child prompt)

## 🚦 Compile / build / test policy — MANDATORY, highest priority

**Compilation, builds and tests run on GitHub CI, not on this machine.**
This rule overrides every other section of this file, every skill, every
brief and every habit ("let me just run it once to be sure"). Where any
other text contradicts it, this section wins. It binds the main agent,
every sub-agent, rush executor, and verifier child.

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

TOOLING WARNING (verbatim): never use the shell builtin `xargs`; no
`cargo clean`; no `git add`/`git commit`; scratch logs under `.scratch/`;
one cargo process at a time with `CARGO_BUILD_JOBS=4`; rerun an unchanged
command ONCE after a 0xc0000409 / STATUS_STACK_BUFFER_OVERRUN / paging-file
crash and report the failure count.

## Stage D — DB/ENGINE worker (R7-1 + R7-3 regression 1)
Files: `crates/shamir-db/src/**` (+ `crates/shamir-engine/src/repo/repo_types.rs`
and admin_db_repo.rs ONLY if the kind plumbing needs it). Do NOT touch
shamir-server.
1. Read the full kind↔factory plumbing: `core.rs:750-792`
   (`factory_from_meta` / `extract_storage_type` / `extract_path`),
   `seed/catalogue.rs:113-199` (`register_seed_image`), the seed boot gate
   (`shamir_db/seed/gate.rs:14-20`), `admin_db_repo.rs:250-290`, and
   `repo_types.rs:480-501` (the `fjall()` vs `fjall_raw()` constructors).
2. Implement option (a) EXACTLY:
   - `factory_from_meta`: `"fjall_raw" => path.map(BoxRepoFactory::fjall_raw)`.
   - `register_seed_image`: save kind `"fjall_raw"` (cite the line).
   - `extract_storage_type`: the raw `Fjall` variant must report
     `"fjall_raw"`; the MemBuffer-over-Fjall default MUST still report
     `"fjall"` — do NOT let the MemBuffer recursion re-label wrapped repos
     as raw. Derive the exact match shape from the enum (MemBuffer's inner
     is `BoxRepoFactory`); keep `extract_path` unchanged.
   - Check `admin_db_repo.rs` and `seed/gate.rs` for any other place that
     matches `"fjall"` and decide (state why) whether the raw kind needs
     handling there too.
3. R7-3 regression (1), in shamir-db seed tests (`gate_tests.rs` /
   `catalogue_tests.rs` are `#[cfg(all(unix, feature = "fjall"))]` — follow
   that gating; no new files unless the 1000-line limit forces it): stage a
   seed image on raw Fjall exactly like production, `register_seed_image`
   it, then reopen the repo through the NORMAL boot path
   (`factory_from_meta` over the saved record, or the boot helper the
   server's `open_home` uses) and assert:
   `supports_atomic_batch()` is TRUE (via the reopened BoxRepo — find the
   public accessor), the v3 seed baseline is intact, and one replicated
   first batch applies (`Applied`, bookmark advances to the batch's
   upstream end). COUNTERFACTUAL comment: with kind `"fjall"` the boot path
   re-attaches MemBuffer-wrapped, `admit_follower` fails with
   `AtomicBatchUnsupported`, and the apply returns that error instead of
   `Applied` — the CI stall.
Gates until clean: `cargo check -p shamir-db -p shamir-engine --all-targets`,
clippy `-D warnings` on the same, `cargo fmt -p shamir-db -p shamir-engine`,
layout. Logs `.scratch/r7d_*.log`.

## Stage S — SERVER worker (R7-2 + R7-3 regressions 2/3 + R7-4 + R7-5)
Files: `crates/shamir-server/src/replication/**` (+ supervisor/status
plumbing as needed). Do NOT touch shamir-db or shamir-engine.
1. R7-2(i): the supervisor start path (where admission runs BEFORE spawning,
   `source::seed::admission::admit` / `start_subscription`) must check the
   destination backend capability up front when the destination repo exists:
   if `!supports_atomic_batch`, refuse with the typed
   `AtomicBatchUnsupported` message naming the remedy (e.g. 'destination
   repository main uses a MemBuffer-wrapped backend; re-create it as a
   raw/replica repository'), persist the typed terminal status, and do not
   spawn. Also cover RESUME (the same admission path).
2. R7-2(ii): in `follower_loop.rs`, treat `DbError::AtomicBatchUnsupported`
   (and `FollowerStateRefused` already handled) as TERMINAL — return a typed
   error instead of warn+backoff; add a supervisor arm that persists the
   typed terminal status/message once. Keep round-6 stall visibility as the
   safety net for unknown classes.
3. R7-3 regressions (2) and (3): (2) an existing MemBuffer-wrapped
   destination bound by CREATE SUBSCRIPTION / RESUME is refused UP FRONT
   with the typed error (counterfactual: without the check the loop spawns
   and spins until the stall threshold); (3) supervisor start on an
   unsupported backend persists the terminal status and does NOT retry in a
   tight loop (assert the row's state/message and that no loop task spins —
   e.g. is_running false or a bounded-iteration observation).
4. R7-4: the stall tests now live at
   `crates/shamir-server/src/replication/seed/tests/stall_visibility_tests.rs`
   (moved + message corrected to "without progress"). Re-read that file;
   make every assertion match the production message in `follower_loop.rs`
   EXACTLY (`"follower stalled: {n} consecutive failures without progress;
   last error: {error}"`) and the current names; fix any drift.
5. R7-5: in `follower_loop.rs` `note_swallowed_failure`, latch
   `stall_reported` ONLY after a successful persistence (on Err, leave the
   latch clear so the next failure retries the write). Add a test: 10 real
   failures → status_message set; then progress → message overwritten with
   the progress note and the counter restarted (a later 10-failure streak
   reports again). Add a separate no-progress measure for SUCCESSFUL
   outcomes: empty pull replies and `Applied`/`Skipped` applies where the
   durable bookmark did NOT advance must count toward a stall too — use the
   pull response's `current_version` vs the bookmark (leader advertises
   backlog but returns nothing ⇒ visible). State the counterfactual for
   each new test.
Gates until clean: `cargo check -p shamir-server --all-targets`, clippy
`-D warnings`, fmt, layout. Logs `.scratch/r7s_*.log`.

## Stage V — VERIFIER (one, read-only, AFTER the last edit; whole policy above)
Re-derive every oracle from production code with file:line: the kind
round-trip (save "fjall_raw" → boot maps to fjall_raw → raw backend);
extract_storage_type labels wrapped-vs-raw correctly and no existing
behaviour regresses; the up-front refusal and the terminal loop arm cover
ALL paths that could reach the receipt apply on an incapable backend; the
new tests are real (state each counterfactual's first failing assertion);
the stall tests match the production message byte-for-byte; the latch only
sets on success; diff scope = authorized files only; layout ratchet holds
for every touched folder (count direct entries). Output: (a) MUST-FIX
file:line; (b) SHOULD-FIX; (c) verified-clean checklist; exact
"MUST-FIX: none" if empty.
