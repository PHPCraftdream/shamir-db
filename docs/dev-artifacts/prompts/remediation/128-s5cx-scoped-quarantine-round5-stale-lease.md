# 128 — S5c-X scoped quarantine, round 5: stale-lease lifecycle + guard errors

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

TOOLING: never use the shell builtin `xargs` (it crashes the whole rush session); all scratch logs under `.scratch/` (git-ignored), never the repo root or `target-scratch/`; ONE cargo process at a time; `CARGO_BUILD_JOBS=4` — never 1.

## Round-5 items (compact; full specs live in the orchestrator briefs)

**A2 — single live lease per name** (`repl_binding/lease.rs`): `take_subscription_lease` stops unconditionally replacing the map entry. It refuses (`None`) while a VALID lease exists, and invalidates + quiesces (apply-gate write drain) an INVALID displaced lease before installing the fresh one. Rationale: every legitimate re-issue path invalidates its own lease first, so a valid lease always means a live follower — refusing keeps exactly one live follower. New `subscription_lease(name)` accessor and `invalidate_subscription_lease(name, lease, control)` (removes the entry when `Arc::ptr_eq` matches; no apply-gate write — the caller is the lease's own finished loop or the failed barrier).

**A3 — `ReplicationControlGuard` newtype**: the raw `tokio::sync::MutexGuard<()>` leaves the public API. A single acquisition function `ShamirDb::lock_replication_control()` returns the guard (constructor `pub(crate)`; outside code cannot fabricate it — compiler-enforced caller-holds-lock as far as crate boundaries go, no compile-fail evidence in CI). The `replication_control_lock` field and its accessor go private; every acquisition site converts; `remove_db_locked`/`remove_repo_locked` become `pub(crate)` and take `&ReplicationControlGuard`.

**B1 — DROP DATABASE CASCADE table bug**: `handle_drop_db`'s cascade loop dropped tables of the SESSION database (`self.db_name`) instead of `op.drop_db` — master data loss. Fixed to `&op.drop_db`, with a red-on-master regression test proving the session db's same-named table survives.

**B2 — guards treat pending/prepublished cache keys as bound/quarantined**: `ensure_destinations_unbound` and `ensure_drop_allowed` also consult `resync_quarantine.reason_for` — a destination whose quarantine key was prepublished but not yet durably confirmed is refused exactly like a durably bound/quarantined one (covers the terminal-path publish-before-persist window that survives a failed flush until restart).

**B4 — typed, coded guard errors**: two new `shamir-storage` `DbError` variants with dedicated wire codes — `ReplicationDestinationBound` (`replication_destination_bound`) and `ReplicationDestinationQuarantined` (`replication_destination_quarantined`) — built by truthful-message builders in `repl_binding/validate.rs`, converted at the admin-dispatch boundary via `repl_guard_error_to_batch` (in `execute/helpers.rs`), with the three stale remedy texts corrected (quarantined subscriptions cannot be dropped or abandoned in place in this version).

**Step 5 — test-util feature gate**: the flush-fault seam (`SeedCatalogueFlushFaultForTest`, `flush_fault_armed`/`pass_first`/`fail_first`) is gated `#[cfg(any(test, feature = "test-util"))]` so shamir-server's tests can inject real-seam faults (T2/T3/B3); `arm_fail_first(n)` added; production default features untouched.

## NOT in this round (branches stay intact)

Abandonment exit path, runbook rewrite, and the t13 replacement-rebind branch: untouched. `crates/shamir-server` is W2/W3 scope — the ONLY permitted touch is the dev-dependency `features = ["test-util"]` line; the supervisor's compile breakage from the new lease/guard signatures is EXPECTED and reported, not fixed.
