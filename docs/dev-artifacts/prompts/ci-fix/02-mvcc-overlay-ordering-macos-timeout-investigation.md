# CI investigation — `overlay_ordering_reader_sees_version_implies_value_history_arm` 600s timeout on macos-latest

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## What happened

The latest `master` CI run (gh run 29585194098, `cargo test lib
(macos-latest)` job) killed a test after its configured slow-timeout
budget:

```
TIMEOUT [ 600.061s] (4805/4805) shamir-tx
  tests::mvcc_store_tests::overlay_ordering_tests
  ::overlay_ordering_reader_sees_version_implies_value_history_arm
error: test run failed
```

This is NOT a new/unknown class of failure — `crates/shamir-tx/src/tests/
mvcc_store_tests/overlay_ordering_tests.rs` and `.config/nextest.toml`
ALREADY document this EXACT test as the repo's own "leading, still-
unconfirmed hypothesis" for a class of CI-only timeout:

`.config/nextest.toml` lines 61-80 (read this whole `[profile.ci]`
comment block first):

> Task #589: nextest's default test-threads = num-cpus already caps how
> many test BINARIES run concurrently... but it has no visibility into
> what happens INSIDE each spawned test process. A large fraction of this
> workspace's tests use `#[tokio::test(flavor = "multi_thread",
> worker_threads = 4)]` (33 files as of this writing), so N concurrently-
> running binaries can each spin up their OWN 4-worker tokio runtime —
> N×4 real OS threads competing for whatever (likely far fewer than this
> repo's 16-core dev boxes) vCPUs a hosted CI runner actually has. This is
> the leading, still-unconfirmed hypothesis for the handful of tests that
> legitimately pass in isolation but TIMEOUT under full-workspace CI
> parallelism (e.g.
> `overlay_ordering_reader_sees_version_implies_value_history_arm`, which
> itself uses `worker_threads = 4`). Capping test-threads explicitly
> bounds the number of concurrently-running binaries... Starting value —
> tune down further (or up, if CI logs show idle cores) once real
> GitHub-runner behavior is observed; this is a hypothesis-driven starting
> point, not a measured-optimal value.
>
> test-threads = 4

`scripts/test.sh` (lines ~184-197) already auto-selects `--profile ci`
whenever `CI=true` (set automatically by GitHub Actions), which sets
`slow-timeout = { period = "60s", terminate-after = 10 }` — exactly the
`600.061s` kill this run hit. So the observed failure is EXACTLY the
scenario this config comment predicted and was waiting to observe on a
real GitHub runner — this is that observation.

**CLAUDE.md is explicit and non-negotiable on this class of finding:**
*"Hangs and test-locks are BUGS — hunt and fix them, never tolerate. ...
Reproduce it (loop the suite under load — these surface under nextest's
parallelism, often not in isolation), find the root (lock-order cycle,
bounded-channel backpressure with no drain, a Barrier a task never
reaches, a guard held across `.await`, runtime starvation), and FIX it.
NEVER raise the timeout to paper over it."*

Your job is exactly this: **determine whether this is (a) genuine runtime/
scheduling starvation from tokio-runtime-fan-out oversubscription (as the
existing hypothesis suggests) — in which case find and apply the right
CONFIGURATION fix (not just "raise the timeout") — or (b) an actual
logical deadlock/lock-order cycle in the `MvccStore` code path this test
exercises — in which case root-cause and FIX the code.** Do not simply
bump `terminate-after`/`slow-timeout` without first ruling out (b), and do
not declare it "just scheduling" without concrete evidence.

## Test under investigation

Read `crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs`
in full (it's short, ~260 lines). Both arms
(`overlay_ordering_reader_sees_version_implies_value_overlay_arm` and
the failing `..._history_arm`) spawn `READERS=4` reader tasks in a tight
`while !stop.load(...)` loop plus one writer task doing `ROUNDS ×
WRITES_PER_ROUND` commits (12×40=480 for the history arm), all inside a
`#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` runtime.
The history arm additionally calls `mvcc.write_committed_to_history(...)`
and (further down in the file, read past line 260) `gc_overlay_to` per
write — more work per iteration than the overlay arm, which is presumably
why only the history arm (not the overlay arm) hit the 600s ceiling in
this run.

## Investigation steps

1. **Try to reproduce genuine contention/starvation locally** (this dev
   machine has 16 logical cores per the nextest.toml comment, so a single
   isolated run won't reproduce CI's tighter-core scenario) — artificially
   oversubscribe: run this ONE test binary concurrently with several
   CPU-bound busy-loop processes (or several OTHER heavy test binaries in
   parallel) to simulate N-binaries × 4-worker-threads contention on a
   constrained core count. Windows tools: you can use PowerShell/bash to
   launch several `cargo test`-adjacent busy loops, or use `cmd /c start`
   /background `Bash` calls running a CPU-spin (`yes > nul` equivalent,
   or a tight Rust/Python loop) pinned/left unpinned to simulate load, and
   observe whether THIS test's wall-clock time balloons dramatically
   under that induced contention vs. running alone. Report concrete
   numbers (isolated time vs. contended time).
2. **Read the exact code paths this test exercises** for anything that
   could be a genuine deadlock independent of scheduling: `MvccStore::
   apply_committed_visible`, `write_committed_to_history`, `gc_overlay_to`,
   `get_at`, `version_of` (find them in `crates/shamir-tx/src/mvcc_store.rs`
   or wherever `MvccStore` lives). Look specifically for: a lock held
   across an `.await` point that could form a cycle with another lock
   acquired by a concurrent reader; an unbounded-wait primitive (channel,
   barrier, semaphore) with no guaranteed drain under this test's exact
   interleaving; any O(n²)-or-worse behavior in `gc_overlay_to`/history
   writes that could make 480 iterations pathologically slow (not
   deadlocked, just slow) if e.g. it rescans a growing structure per call.
3. **Check test history**: `git log --follow -p -- crates/shamir-tx/src/
   tests/mvcc_store_tests/overlay_ordering_tests.rs` and `git log --all
   --oneline --grep overlay_ordering` — has this test's timing/timeout
   been tuned before? Any prior incident reports (check
   `docs/dev-artifacts/` for anything mentioning "overlay_ordering" or
   "D2 ack-path") that already ruled something in/out.
4. **Form a conclusion with evidence**, then act:
   - If you find genuine evidence of scheduling/oversubscription (e.g.
     wall-clock time scales dramatically with induced contention, but the
     LOGICAL behavior — number of iterations completed, no stuck task —
     is fine even if slow): the right fix is a CONFIGURATION change, not
     raising the global timeout blindly. Options, in order of preference:
     a. Add this test (or the whole `overlay_ordering_tests` module/
        binary) to a dedicated `[test-groups]` entry (mirroring the
        existing `wasm-heavy` group) with a `max-threads` cap, so it
        doesn't compound with the rest of the workspace's parallel
        binaries on constrained CI runners — this is the SAME pattern
        already used for `functions_lifecycle`.
     b. A per-test `[[profile.ci.overrides]]` (note: `ci`, not `default`
        — the override at nextest.toml lines 83-89 is only under
        `[profile.default.overrides]` today; check whether overrides need
        to be duplicated per-profile or if there's a cleaner shared
        mechanism — read nextest's own docs/schema comments if unclear)
        raising ONLY this test's slow-timeout, with a comment explaining
        the actual measured legitimate duration under contention (mirror
        the existing `wasm_function_inserts_and_queries` override's
        style: "~99s legit, kill at 240s" — you need the equivalent real
        number for this test, not a guess).
     c. Reducing `worker_threads` for just this test file (e.g. from 4 to
        2) if you determine 4 worker threads specifically is what's
        driving the oversubscription math, without weakening the actual
        race-window coverage the test needs (READERS=4 could still run
        as 4 tokio TASKS on fewer OS threads — tasks and worker_threads
        are not the same axis; changing this must not silently reduce
        the test's ability to catch the real D2 race it's designed to
        catch).
   - If you find genuine evidence of a real deadlock/lock-order issue
     (not just slowness): root-cause it precisely (name the exact lock/
     primitive and the interleaving) and fix the PRODUCTION code (not
     the test) — this would be a serious, separate finding; stop and
     report it in detail rather than silently patching around it if the
     fix is non-trivial or you're not fully confident, per this session's
     standing zero-trust/no-shortcuts discipline.

## What NOT to do

- Do NOT simply raise `[profile.ci]`'s global `slow-timeout`/
  `terminate-after` — that mutes the signal for every OTHER test in the
  workspace, hiding a REAL future deadlock behind a longer wait. Any
  timeout change must be SCOPED to this specific test (via a test-group
  or per-test override), never the global CI profile.
- Do NOT weaken the test's actual race-coverage (e.g. drastically
  cutting `ROUNDS`/`WRITES_PER_ROUND`/`READERS`) just to make it finish
  faster — that reduces the probability of catching the real D2 ordering
  bug it exists to catch. If you determine the iteration counts
  themselves are the issue (not thread oversubscription), justify any
  reduction with reasoning about what coverage is preserved.
- Do NOT touch anything else in this session's #661-670 wave or the
  other CI fixes already landed today (the sq8 tolerance fix, the
  `$fn`+`$ref` write-value fix).

## Verification (MANDATORY before you report done)

- Run the specific test in isolation AND under your best local
  reproduction of contention, multiple times, and report the actual
  wall-clock numbers you observed (don't just assert "it's fine now" —
  show the before/after timing evidence).
- `./scripts/test.sh -p shamir-tx --full` green (or `-p shamir-tx` lib
  scope — this test lives under `--lib`, check which invocation
  actually runs it, matching the CI job's own `./scripts/test.sh
  --locked` which is the DEFAULT (non---full) scope — confirm which
  scope this test module is wired into and use the matching command).
- `cargo fmt --all -- --check` clean (or scoped, report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean if you
  touched any `.rs` file; if your fix is `.config/nextest.toml`-only, this
  still won't hurt to confirm nothing else broke.
- Report literal command output.
- State your conclusion explicitly: "genuine oversubscription, config
  fix applied because X" or "genuine deadlock, root cause is Y, fixed by
  Z" or "inconclusive — here is what I tried and what I'd need to
  confirm further" (it is OK to report inconclusive rather than guess,
  but explain exactly what evidence you gathered and what remains
  uncertain).
