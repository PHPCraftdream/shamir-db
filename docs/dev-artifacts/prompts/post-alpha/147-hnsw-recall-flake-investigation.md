# Task #923 -- hnsw_rs_contract_tests recall flake on windows-latest lib tests

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Discovered 2026-08-02 as a side-effect of investigating a separate CI hang
(task #922), via a real CI run (`ci.yml`, run `30757334929`, `cargo test
lib (windows-latest)`):

```
shamir_index::vector::tests::hnsw_rs_contract_tests::parallel_insert_surfaces_all_ids_and_matches_bruteforce_top1
  panicked at crates\shamir-index\src\vector\tests\hnsw_rs_contract_tests.rs:293:5:
  parallel_insert recall vs brute-force too low: 16/20
```

This is a probabilistic recall-threshold assertion (HNSW approximate
nearest-neighbor recall vs. an exact brute-force baseline) that dipped
below its pass threshold under CI timing/scheduling variance. Same CLASS
of issue as the already-documented, already-accepted
`truncation_ceiling_blocks_segment_removal_when_a5_gate_unsafe` flake
(`crates/shamir-engine/src/tx/tests/truncation_tests.rs`) -- but this one
has NOT yet been investigated or confirmed as a known/accepted flake vs.
a genuine correctness bug in the parallel-insert path. Do not assume it's
"just a flake" without checking.

Note: this repo has a separate, larger "vector production campaign" in
flight (`docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md`, tasks #393-415,
hnsw_rs contract spike work) -- check whether this exact test/threshold
is already flagged or being addressed there before starting a fresh
investigation from scratch. If it is, coordinate/defer rather than
duplicate.

## What to do

1. Read `crates/shamir-index/src/vector/tests/hnsw_rs_contract_tests.rs`
   in full, focusing on `parallel_insert_surfaces_all_ids_and_matches_bruteforce_top1`
   (around line 230-293) -- understand the test's dataset size, `k`
   (top-k), the parallel-insert concurrency shape, and the exact recall
   threshold being asserted (`16/20` failed -- what's the pass bar? 18/20?
   19/20? 20/20?).
2. Reproduce locally: `./scripts/test.sh -p shamir-index -- parallel_insert_surfaces_all_ids_and_matches_bruteforce_top1`,
   run it repeatedly (a shell loop, 20-50 iterations) to establish an
   actual failure rate. Also try under artificial load/contention (e.g.
   running it concurrently with other CPU-heavy work) since CI-only
   reproduction suggests a scheduling-sensitivity issue.
3. Read the parallel-insert code path itself
   (`crates/shamir-index/src/vector/` -- find wherever concurrent HNSW
   inserts are implemented) to understand whether there's a plausible
   correctness bug that would SYSTEMATICALLY lower recall under
   concurrent load (e.g. a race in graph-edge linking that could drop a
   valid edge, not just approximate-algorithm noise) vs. genuine,
   expected HNSW approximate-recall variance (HNSW is inherently
   probabilistic; recall dipping under certain seed/ordering combinations
   is normal for the algorithm, not a bug).
4. Determine the verdict:
   - **If it's a genuine, expected flake** (HNSW's own approximate-recall
     variance, no correctness bug): raise the threshold with a clear
     justification (cite the observed failure rate, explain WHY 16/20 is
     within expected variance for this k/dataset-size/concurrency
     combination), matching this repo's own precedent style for the
     truncation-tests flake's documentation.
   - **If it's a genuine bug** (e.g. a race that can drop a valid
     edge/insert under concurrent access): fix the root cause. Do NOT
     just raise the threshold to hide a real correctness regression.

## What NOT to do

- Do not just bump the threshold without establishing WHY the current
  one is wrong (a bare "make CI pass" threshold change is exactly the
  kind of change this repo's own conventions warn against).
- Do not touch unrelated tests/code in this file or elsewhere.
- If the "vector production campaign" tasks (#393-415) are already
  actively addressing this area, stop and report that rather than
  duplicating effort.

## Definition of done

- A clear verdict: genuine flake (threshold adjusted with justification)
  or genuine bug (fixed at the root).
- If threshold-adjusted: the new threshold's justification documented
  inline (a comment) mirroring the existing truncation-tests flake's
  documentation style.
- If bug-fixed: `cargo fmt`/`clippy -p shamir-index --all-targets -- -D
  warnings` clean; `./scripts/test.sh -p shamir-index` green; the
  specific test passes reliably across at least 20 consecutive local
  runs (report the actual count).

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
