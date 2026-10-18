# 127 — S5c-X scoped quarantine, round 4: start-time durability barrier + generation lease

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
touched (`-p <crate>`, plus dependents when a public signature changed;
`--workspace` only when asked):

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

TOOLING: never use the shell builtin `xargs` (it crashes the whole rush session).

Standing rules: ⛔ NO git mutations; never `#[allow]`; imports at file tops; `mod.rs` re-exports only; layout ≤7 entries/folder, ≤1000 lines/file (supervisor_tests.rs 998, admin_replication.rs 991 — new tests/logic go in NEW files or within headroom, verify with the layout checker); no sleeps and no yield-budget loops in new tests (Notify seams + `futures::poll!` instead); queries only through the builder.

## Goal

Replace round 3's in-memory `ConfirmedActive` set (`crates/shamir-db/src/repl_binding/confirmed.rs`, `ShamirDb::may_start_follower`) with two simpler mechanisms. Do not keep both.

**MECHANISM 1 — durability proof at START, not in memory.** `start_subscription` (supervisor, shamir-server), under `replication_control_lock`, re-validates the live row (exists, `active`, generation, destination set) and then runs the catalogue durability barrier itself (the same `flush_seed_catalogue` the CREATE/RESUME path uses). Only if that flush succeeds does it start the follower. A flush failure means: do not start (row stays live-active, a later reconcile retries the barrier). There is no membership state at all, so Pause / terminal transitions / failed flushes cannot leave stale permission behind, and CREATE needs no extra publication step. `pending_durable` rows are never started. The typed `pending_creation_barrier` refusal for RESUME of a pending row stays.

**MECHANISM 2 — per-subscription generation lease with a quiesce barrier** (closes BOTH stale windows: inside startup after the lock is released, and between the loop's check and applying a Pull response). In `ShamirDb` keep a registry `name -> Arc<SubscriptionLease { generation, valid: AtomicBool, apply_gate: tokio::sync::RwLock<()> }>`. The supervisor takes the lease for (name, generation) in `start_subscription` while it still holds the control lock, and hands it to the follower loop. The follower, immediately before EVERY apply and after the Pull response has arrived, does `let guard = lease.apply_gate.read().await; if !lease.valid.load(Acquire) { exit cleanly }` and keeps `guard` alive across the whole engine apply + bookmark advance. Every operation that changes the subscription's binding or lifecycle — DROP, same-name recreate, rebind / replacement-seed rebind, PAUSE, terminal marking from another task — runs, UNDER the control lock and BEFORE mutating the row: `lease.valid.store(false, Release); let _q = lease.apply_gate.write().await;` (waits for an in-flight apply to finish and blocks new ones), then mutates, then drops. A terminal marking made by the follower loop itself must run after it dropped its own read guard (document the order: control lock -> lease write gate; follower never takes the control lock while holding the read guard). Generation-aware terminal callbacks stay as a second line of defence.

Lock order everywhere: `replication_control_lock` → `lease.apply_gate.write()`. A DashMap guard must NEVER be held across an `.await` (clone the `Arc<SubscriptionLease>` out, drop the ref, then await).

## Stages
- W1 (shamir-db prod): delete confirmed.rs/may_start_follower; add lease.rs + registry + quiesce; P2 guard params on remove_db_locked/remove_repo_locked; wire quiesce into DROP / PAUSE / terminal marking / installer replacement rebind; pending.rs final-write test seam.
- W2 (shamir-server prod): start_subscription barrier + lease take/hand-off + validity checks; follower_loop lease guard across apply; reconcile drops may_start_follower; cfg(test) seams (start-barrier faults, start race hook).
- W3 (tests): T1 create→reconcile starts; T2 start-barrier failure → not started, retry later starts; T3 failed-Pause-flush leaves live row paused, follower stops, no stale permission (split: db-side semantics + server-side behavior); T4 obsolete follower mid-Pull applies nothing + `futures::poll!` proves the invalidator is Pending while a read guard is held; T5 startup interior window seam → never starts, nothing marked; T6 cascade interlock test rewritten to the real invariant (both winning orders consistent, never partial drops); T7 CREATE future DROPPED (not aborted) at a Notify seam exactly on the final-write boundary → row ends `pending_durable`, reconcile must not start it; T8 fix `subscription_row_opt` (`records.remove(0)` on empty) and make the stale-start control leg prove genuine running+applying (bookmark advance), not registry membership.
- Verifier: re-read every changed file + callers; audit lock order (control lock → lease write gate; follower read guard never held while taking the control lock) for deadlock; grep for remnants.
═══════════════════════════════════════════
