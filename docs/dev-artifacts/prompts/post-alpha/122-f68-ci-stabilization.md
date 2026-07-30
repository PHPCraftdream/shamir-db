# F-68 (#895) — stabilize all CI workflows, root cause only

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Hard rule — no workarounds, no exceptions

Every failure below must be fixed at its **root cause**. Forbidden, full
stop: loosening an assertion threshold with no derivation, adding a
retry/rerun wrapper around a flaky test, `#[ignore]`, raising a nextest
timeout to make a hang "pass", pinning an RNG seed purely to dodge a
failure without understanding it, or dropping a platform from a matrix.

If a test is flaky, find the actual nondeterminism (unseeded RNG,
wall-clock dependence, iteration-order dependence, a resource leaked by an
earlier test, a real race) and eliminate it. If — after genuinely finding
the root cause — the correct fix is a threshold change (e.g. a floor was
mathematically wrong for the achievable recall), that is acceptable ONLY
with the derivation written in the commit message and in a code comment
next to the assertion. Anything else, go find the real bug.

This file bundles five investigations. Treat each as its own root-cause
exercise; do not let the size of the bundle tempt you into a shortcut on
any one of them. Commit after each cluster is genuinely fixed and verified
(five to six commits expected out of this brief, not one).

## Cluster A — vector/HNSW self-query and restart recall flakiness (REPRODUCED LOCALLY)

Two tests, same failure class:

1. `crates/shamir-index/src/vector/tests/quantization_snapshot_tests.rs`
   — `spike_file_dump_u8_works`. Was "fixed" once already by commit
   `f6016bce` (grew the self-query fixture from N=300 to N=1500) — **that
   fix did not work**. Reproduced FAILING again on this Windows dev box,
   personally, this session: looping `./scripts/test.sh -p shamir-index --full`
   3× back-to-back, run 1 failed with:
   ```
   thread '...spike_file_dump_u8_works' panicked at
   crates\shamir-index\src\vector\tests\quantization_snapshot_tests.rs:217:5:
   pre-dump: self-id 0 not in top-10 (got [928, 672, 440, 728, 72, 112, 688, 1272, 784, 1224])
   ```
   Runs 2 and 3 of the same loop passed. This is genuine, reproducible,
   parallel-load-dependent flakiness — it did NOT reproduce across 25
   consecutive isolated single-test runs, only under the full-suite
   nextest concurrency load. Per CLAUDE.md's own guidance, that is exactly
   the shape "these surface under nextest's parallelism, often not in
   isolation" — reproduce it the same way (loop `./scripts/test.sh -p
   shamir-index --full` several times; do not rely on isolated single-test
   runs, they will look green).

2. `crates/shamir-index/src/vector/tests/crash_recovery_tests.rs` —
   `restart_preserves_recall_at_10_against_brute_force`. CI-only failure
   (macOS `cargo test lib`, run 30515165549): "recall@10 after restart
   (0.800) below 0.90 floor". Did NOT reproduce in 25 consecutive isolated
   local runs on this Windows box — try the same full-suite-loop technique
   as (1), and separately consider that macOS CI runners have far fewer
   cores (macos-latest ≈ 3-4 vCPU) than a dev box, which may matter (see
   below).

**Documented root cause, already in the codebase**
(`hnsw_adapter.rs:46-58`): `hnsw_rs` 0.3.x assigns graph node layers from
an internal, **unseedable** RNG. The comment there already treats this as
inherent for tiny graphs (hence `BRUTE_FORCE_MAX = 256`), but both failing
tests operate ABOVE that threshold (1500 and 3000 vectors respectively),
where the graph path is active and the comment's mitigation does not
apply.

**What is NOT yet known, and must be determined before picking a fix:**
Does the run-to-run variance come from (a) the RNG itself being
non-deterministic regardless of insertion order, or (b) `parallel_insert`
(rayon-based concurrent insertion, used by the batch/quantized build
paths — see `hnsw_adapter.rs` around `parallel_insert` call sites) making
the graph's build order (and hence neighbor selection) vary between runs
even with an otherwise-fixed RNG. Distinguish these empirically: build the
same fixture via a strictly serial `insert` loop, repeat many times, and
see whether the self-query / recall result is now stable. If serial
insertion is stable and only `parallel_insert` reintroduces variance, that
tells you the real lever (and its cost — measure it, don't guess).

**Fix, once the mechanism is understood — pick the one the evidence supports:**
- If ordering-dependence (b) is the driver and a stable build is cheap
  enough, use a deterministic/serial insertion path for these two tests'
  fixtures (or for correctness-sensitive small builds generally) instead
  of relying on a statistical floor.
- If the RNG is genuinely nondeterministic irrespective of order (a),
  the tests are asserting a false determinism guarantee against a
  library that provides none. In that case the legitimate fix is a
  **derived** floor: run the specific test 100-200× locally (a tight
  bash loop calling `./scripts/test.sh -p shamir-index -- <test name>`,
  or better, temporarily instrument the test to print the actual recall/
  hit-count on every run so you get a real distribution, then remove the
  instrumentation), compute the empirical failure rate at the CURRENT
  floor, and set a new floor with the derivation written in the commit
  message and as a comment beside the assertion (e.g. "observed recall
  distribution over N=200 runs: min X, p1 Y — floor set at Z with margin
  M"). A floor picked without that derivation is a banned workaround.
- Either way, if `f6016bce`'s "grow N" approach turns out to reduce but
  not eliminate the failure rate, say so explicitly in the commit message
  — don't silently repeat a fix that's already known to be insufficient.

## Cluster B — `ts-e2e-nightly` / "node napi e2e" job: NOT a flaky test, a CI config bug

`.github/workflows/ts-e2e-nightly.yml:132-133` sets `cache: npm` +
`cache-dependency-path: tests/e2e/package-lock.json` on the `setup-node`
step. But `.gitignore:30` explicitly excludes `tests/e2e/package-lock.json`
— it has never been committed (confirmed: `git ls-files
tests/e2e/package-lock.json` returns nothing; `git ls-files
crates/shamir-client-ts/package-lock.json` DOES return a match — that
sibling lockfile IS tracked). In a clean CI checkout the referenced file
does not exist, and the setup-node step fails outright:
```
##[error]Some specified paths were not resolved, unable to cache dependencies.
```
This is why the job has been red for 2+ consecutive nightly runs — it is
deterministic, not flaky, and nobody looked at it because the annotation
scrolled past under a wall of otherwise-green test output.

The `.gitignore` line was added in `0b9bced2` ("feat: shamir-client-node
napi-rs binding + replace tests/e2e") — read that commit to understand
whether excluding the e2e lockfile was deliberate policy or copy-paste
from the (correctly-ignored) `crates/shamir-client-node/package-lock.json`
line right above it. Then decide the real fix:
- If `tests/e2e`'s dependencies should be pinned/reproducible like the
  `shamir-client-ts` job's (which DOES commit its lockfile and DOES cache
  successfully), the fix is to un-ignore and commit
  `tests/e2e/package-lock.json` — the asymmetry between the two
  `cache-dependency-path` configs in the SAME workflow file (one pointing
  at a tracked file, one at an untracked one) is itself evidence this was
  an oversight, not an intentional design.
- If there's a real reason the e2e lockfile must stay untracked (check
  whether `tests/e2e/package.json` uses floating version ranges that make
  a committed lockfile misleading), then the correct fix is to drop the
  `cache: npm` / `cache-dependency-path` config for that specific
  `setup-node` step instead — do not leave a cache config pointing at a
  file that will never exist.

Whichever direction the investigation supports, verify the fix actually
lets the job run past the `setup-node` step (a green `ts-e2e-nightly` run,
or at minimum: manually confirm the `actions/setup-node` step no longer
errors, since the workflow itself is `schedule`-triggered and you likely
cannot force a nightly run on demand — check if it also has a
`workflow_dispatch` trigger you can use to confirm on a branch).

## Cluster C — `ts-e2e-nightly` / "ts client e2e" job: genuine test failure

```
FAIL src/__tests__/e2e-permissions.test.ts > e2e permissions (requires release binary)
  > A11/G4d-group: group membership + chgrp + group bits grant read; removal re-denies
Error: connection closed
  ❯ src/core/framing.ts:104:24
```

Read `src/__tests__/e2e-permissions.test.ts`'s "A11/G4d-group" case and the
server-side permission-check path it exercises (chgrp + group-bit
grant/revoke). Find why the client's connection is closed mid-test.
Candidate root causes to check, don't assume one: (a) a genuine race
between the OS-level `chgrp`/`chmod` syscall the test issues and the
server's own permission cache/re-check timing — the test may need to wait
for a specific server-side signal instead of assuming the change is
visible immediately; (b) the server intentionally drops ALL open
connections when permissions change (not just ones that lose access) and
the test doesn't expect/handle a legitimate reconnect; (c) a real
over-broad revocation bug where a group-permission change disconnects
more sessions than it should. Fix the actual defect (test-side race,
server-side over-broad disconnect, or missing reconnect handling) — do
not just retry the assertion.

## Cluster D — pre-push intermittent failures: two independent 600s hangs (REAL BUGS, not flakes)

Two separate pre-push CI runs on 2026-07-29 each had exactly one job time
out at the nextest ceiling (600s), and in BOTH cases the timed-out test
was the LAST test position in its binary's run:

- ubuntu-latest, run 30447563126, `cargo test integration`:
  `shamir-db::rename_table_durability::rename_populated_survives_cold_restart`
  — `TIMEOUT [600.016s] (881/881)`.
- macos-latest, run 30447562583, `cargo test integration`:
  `shamir-server::observability_http::metrics_exposes_unbounded_sentinel_when_no_byte_budget`
  — `TIMEOUT [600.059s] (879/879)`.

Two DIFFERENT tests, on DIFFERENT platforms, both being the literal last
test to finish in their binary's run is a strong signal this is NOT
independent per-test flakiness — it smells of a resource an EARLIER test
leaks (an unclosed listener/socket, a global runtime/registry not reset,
a file lock never released, a `Barrier`/channel one party never reaches)
that only manifests once nothing else is left running to mask it. This is
exactly the hang class CLAUDE.md already names: "a real deadlock in one
e2e test (tokio::accept that never returns, broadcast channel reader,
file-lock race)".

You are on Windows and cannot literally reproduce an ubuntu/macos-only
hang — do NOT waste time trying to force it blind. Investigate via code
reading first:
1. Read `rename_table_durability::rename_populated_survives_cold_restart`
   and `observability_http::metrics_exposes_unbounded_sentinel_when_no_byte_budget`
   — what do they share? (Do they both spawn a real server process/socket?
   Do they both touch a global/shared registry — e.g. metrics — that a
   PRECEDING test in the same binary might leave in a bad state?)
2. Grep for other tests in the same binaries (`shamir-db`'s integration
   tests, `shamir-server`'s) using the same server-spawn or
   metrics-registry helper, to see whether most already tear down
   cleanly and these two are the odd ones out, or whether ALL of them
   share a latent leak that just happens to only bite the last one to run.
3. If code reading finds a plausible leak (e.g. a listener/port not
   dropped, a `tokio::sync::Barrier` sized for N parties where one path
   can exit early without reaching it, a metrics registry that's global
   `static` and never reset between tests), fix it and validate the fix
   locally as best you can on Windows (run the FULL relevant integration
   suite, `./scripts/test.sh -p shamir-db --full` / `-p shamir-server
   --full`, in a loop — willing to accept a genuinely long wait since a
   600s-class hang, if present, will show up as a `SLOW`/`TIMEOUT` marker
   per CLAUDE.md's own detection guidance).
4. Also check the `windows-latest` `cargo test lib` job from the SAME
   pre-push run (30447562583) — it was reported failing alongside the
   macOS job; determine whether it shares this cluster's root cause or is
   independent, and root-cause it either way.

Do NOT raise the nextest timeout to make this "pass" — if the true fix
takes longer than 600s legitimately (it shouldn't for these tests), that
is a signal the test itself needs restructuring, not a timeout bump.

## Cluster E — Node 20 deprecation (mechanical)

All five `actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020  # v4.4.0`
pins (`.github/workflows/ts-e2e-nightly.yml:76,129`,
`.github/workflows/release.yml:243,286`, `.github/workflows/ci.yml:178`)
target an action version whose runtime is Node 20, which GitHub Actions
now force-runs on Node 24 with a deprecation warning. Bump all five to the
commit SHA for `actions/setup-node@v5` (the node24-runtime major) —
resolve the real SHA via `gh api repos/actions/setup-node/git/refs/tags/v5.0.0`
(or the current latest v5.x tag) rather than guessing one, and keep F-63's
established pin format: `@<sha>  # v5.x.y`. Confirm via `git log -p` on
the file that no other setup-node behavior (node-version input, cache
config) needs to change for the major bump.

## Definition of done

- All 5 workflows (`CI`, `numa`, `supply-chain`, `ts-e2e-nightly`,
  `stress-nightly`) green on `master`.
- For EACH cluster (A-E), a commit whose message states the actual root
  cause found — not "fixed flaky test", but the mechanism.
- No assertion weakened without a written derivation.
- For cluster A specifically: a repeat-run demonstration (loop the
  previously-flaky test/suite several times) showing the flake rate is
  actually reduced/eliminated, not just quiet on one run.
- `cargo fmt -p <touched crates> -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-index -p shamir-db -p shamir-server --full`
  (and any other touched crate) green, run more than once for any crate
  whose fix targets a flake.

Commit each cluster's fix separately as it's verified, with `Co-Authored-By:
Claude Sonnet 5 <noreply@anthropic.com>` per repo convention. Do not batch
all five clusters into one commit — that would make it impossible to
`git bisect` a regression back to a single root cause later.
