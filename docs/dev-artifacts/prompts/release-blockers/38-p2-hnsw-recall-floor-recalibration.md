# Brief — #1034: recalibrate `restart_preserves_recall_at_10_against_brute_force`'s recall floor (ubuntu observed 0.638, below the current 0.75 floor)

## Context

S.H.A.M.I.R. Database. `crates/shamir-index/src/vector/tests/crash_recovery_tests.rs:380-491`,
`restart_preserves_recall_at_10_against_brute_force`, currently asserts
`recall >= 0.75`. CI SHA `cccf2d16`, job "cargo test lib (ubuntu-latest)",
failed: `crash_recovery_tests.rs:491` — measured recall@10 = **0.638**,
below the 0.75 floor.

This is NOT a regression from this session's own work (F-3/F-4/#1032
instrumentation never touched vector/HNSW code) — it is a genuine
under-calibration of the existing floor, surfaced by a NEW CI observation
on a runner OS this test's threshold was never calibrated against.

## Read this first — the existing calibration this task must mirror

The 0.75 floor itself was derived by commit `8e2146af` (2026-07-30, F-68
cluster A) — read its full commit message (`git show 8e2146af`) and the
extensive doc comment already in the test file (`crash_recovery_tests.rs`,
the ~40-line comment immediately above the `assert!` at line ~491) before
touching anything. Summary of what it established, load-bearing for this
task:

- **Root cause (confirmed by reading `hnsw_rs` 0.3.4's own source)**:
  `LayerGenerator::new` seeds via `StdRng::from_os_rng()` — genuinely
  unseedable, fresh OS entropy every graph build. `Hnsw::parallel_insert`
  additionally drives insertion via rayon `par_iter`, so insertion order
  is ALSO nondeterministic. **No caller-side lever exists to pin this.**
- **Methodology used to derive 0.75**: instrumented the test to print
  recall on every run, looped it 55 times on a Windows dev box at the
  SAME params (DIM=16, N_E2E=3000) — min 0.970, max 0.998, mean 0.985,
  **zero runs anywhere near the 0.800 macOS-CI-observed value**. The dev
  box could not reproduce the CI-observed value even once in 55 tries.
  This is itself informative, not a failure of the methodology: it
  corroborates that CI runners (fewer vCPUs than a dev box) produce
  measurably worse-connected HNSW graphs under the same unseedable-RNG +
  rayon-scheduled-insertion mechanism — a property of the RUNNER, not
  reproducible from a beefier local machine.
- **Floor derivation logic**: set with real margin BELOW the one real CI
  observation (0.800 → 0.75), while staying well ABOVE "corrupted graph"
  territory — a genuine corruption craters recall far below 0.5 (per the
  adjacent fallback-rebuild tests in the same file, which assert
  `rebuild_count == 1` for actual corruption rather than a recall
  threshold at all).

**The new ubuntu observation (0.638) is a SECOND, WORSE data point from a
DIFFERENT OS than the one 0.75 was calibrated against.** This is not
"the same flake happening again" — it is new information the existing
floor never accounted for.

## What to do — mirror the methodology, don't just move the number

1. **Reproduce the local-instrumentation methodology** (temporary, removed
   after measurement, exactly as `8e2146af` did): instrument this test (or
   a scratch copy) to print/collect recall on every run, loop it a
   comparable number of times (~50+) on whatever machine you're running
   on, at the SAME params (DIM=16, N_E2E=3000, same query set/seed). This
   corroborates (or refutes) the hypothesis that low-vCPU CI runners
   produce worse graphs than dev/agent boxes — expect this run to also
   fail to reproduce anything near 0.638, matching the historical pattern,
   but confirm rather than assume.

2. **Do NOT derive the new floor from local numbers alone** — the prior
   calibration's own lesson is that local reproduction systematically
   undershoots the worst CI values. The floor must have real margin BELOW
   the worst CI observation known (currently 0.638, ubuntu), not below
   whatever your local loop happens to produce.

3. **Decide between the three options the task raised, with evidence, not
   by default**:
   - **(a) Lower the floor further** (e.g., to comfortably below 0.638,
     while staying well above 0.5 corruption territory — the same margin
     logic `8e2146af` used, just against the new worst-known value).
     Trade-off: a lower floor reduces this test's power to catch a genuine
     partial-corruption regression that degrades recall but not below the
     old 0.75. Weigh this against the adjacent corruption tests already
     covering the "recall craters far below 0.5" failure mode via
     `rebuild_count` assertions instead — if that coverage is real and
     adequate, a lower recall floor here is not giving up meaningful
     regression detection.
   - **(b) Retry/average over N attempts** inside the test (e.g., run the
     recall measurement 3-5 times, assert on the mean or best-of-N).
     Trade-off: reduces false-positive rate at the cost of test wall-clock
     (N times the current cost) — quantify this cost (time the test as-is
     vs an N-repeat version) before deciding it's acceptable.
   - **(c) Per-OS floor** — investigate whether ubuntu-latest's github-actions
     runner vCPU count is systematically lower than macOS's (check GitHub's
     published runner specs for both `ubuntu-latest` and `macos-latest` —
     this is knowable, not speculative) as a possible explanation for the
     0.638 vs 0.800 gap, and whether a per-OS floor (`cfg!(target_os)` or
     an env-based CI-runner-OS check) is justified vs added complexity for
     uncertain benefit. Only recommend this if the vCPU-count investigation
     genuinely supports it — don't add OS-branching speculatively.
   You may combine options (e.g., a single lower floor is likely the
   simplest fix if it doesn't meaningfully weaken corruption detection —
   but justify whichever combination you land on with the evidence you
   gathered, not by default preference).

4. **Update the test's own doc comment** (the ~40-line block above the
   `assert!`) to append this recalibration's reasoning in the same style
   as the existing block — don't replace the `8e2146af` history, extend it
   (mirrors this repo's "F-NN cluster" incremental-derivation convention
   already visible in that comment).

## Constraints

- Follow `CLAUDE.md`: TDD discipline doesn't quite apply here (this is a
  calibration task, not a red-then-green bug fix) — but the
  "no half-finished implementations" and "don't touch code unrelated to
  the task" rules do. Scope is `crash_recovery_tests.rs`'s one test and
  its doc comment; do not touch the sibling `hnsw_adapter_tests::
  recall_at_10_on_1k_vectors` test mentioned in `8e2146af`'s own commit
  message as "same root-cause class, but out of this brief's named scope".
- **Repeat-run demonstration (mirrors `8e2146af`'s own Definition of
  Done)**: after landing the fix, loop `./scripts/test.sh -p shamir-index
  --full` at least 3x and report full pass/fail for each loop — not just
  a single green run.
- Remove any temporary instrumentation (print statements, scratch loop
  harnesses) before finishing — the committed diff should contain only
  the calibration fix and updated doc comment, not debug scaffolding.
- Gate: `cargo fmt -p shamir-index -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `./scripts/test.sh -p shamir-index
  --full` (looped 3x per above). Use the wrapper, never raw `cargo
  test`/`cargo nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.
⛔ Do not touch CI workflow files or trigger any CI run — this is a
local-code-only calibration task.

## Definition of done

- [ ] Local instrumented-loop data collected (comparable methodology to
      `8e2146af`, ~50+ runs) and reported, even if (as expected) it fails
      to reproduce anything near 0.638 — that outcome itself is evidence,
      report it as such.
- [ ] A decision among (a)/(b)/(c) (or a justified combination), backed by
      the evidence gathered, not a default choice.
- [ ] New floor (or retry logic, or per-OS branching) has real margin
      below the worst KNOWN CI observation (0.638), not below local
      numbers.
- [ ] Test's doc comment extended (not replaced) with this recalibration's
      reasoning, mirroring `8e2146af`'s style.
- [ ] `./scripts/test.sh -p shamir-index --full` looped 3x, full results
      reported.
- [ ] No leftover debug/instrumentation scaffolding in the final diff.
- [ ] fmt/clippy/test gates green, real output reported.
