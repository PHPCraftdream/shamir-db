# Brief 13 — fix flaky `filtered_ann_low_selectivity_finds_rare` (RI-13 gate blocker)

## ⛔ Mandatory constraints

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND.

Tests are run ONLY through `./scripts/test.sh` (never raw `cargo test`,
which is blocked outright by a perimeter guard). Use
`./scripts/test.sh -p shamir-engine -- <name>` to run a single test by
name.

## Context

We are running the RI-13 frozen-commit gate for the v0.1.0-alpha.1
release — the whole workspace test suite must be reliably green. A full
`./scripts/test.sh --full` run surfaced a real (not flaky-by-coincidence)
FAIL in:

`crates/shamir-engine/src/table/tests/filtered_ann_tests.rs`,
test `filtered_ann_low_selectivity_finds_rare` (~line 286-350):

```
assertion `left == right` failed: 1% selectivity: must find 3 of 5 rare records; got 1
  left: 1
 right: 3
```

It failed ONLY under the full-workspace parallel run (heavy CPU
contention across ~20 concurrently-running test binaries). It passed
13/13 in isolated re-runs and in a `shamir-engine`-crate-only full run.
This is NOT the already-known `vr5_cofilter_sees_staged_and_filters_residual`
TIMEOUT flake (that one is separate, already fixed this session via a
per-test `slow-timeout` override in `.config/nextest.toml` — do not touch
that override or that test).

## Root cause (already diagged — do not re-investigate from scratch)

The codebase's OWN documentation already explains why this class of test
is unreliable. Read `crates/shamir-index/src/vector/hnsw_adapter.rs`
lines 46-58 verbatim — it states:

- `hnsw_rs` 0.3.x assigns graph node layers from an internal
  **unseedable** RNG — a freshly-built HNSW graph over a small dataset is
  genuinely nondeterministic run-to-run; recall can drop below 100% and
  the same query can return different neighbours across builds.
- `BRUTE_FORCE_MAX: usize = 256` — datasets with ≤256 live elements use
  an EXACT brute-force scan (deterministic, exact top-k, safe for
  `assert_eq!`-style tests). Only above 256 elements does the code use
  the approximate HNSW graph.
- The comment's own stated test-design guidance: "256 keeps small
  indexes (and the exact-assertion tests) deterministic while leaving
  the recall-tolerance tests (**≥1k vectors**) on the HNSW path."

`filtered_ann_low_selectivity_finds_rare` inserts 500 "common" + 5
"rare" vectors = **505 total** — above the 256 brute-force-exact
threshold (so it DOES exercise the approximate HNSW path, which is
almost certainly the test's actual intent, given the file name
`filtered_ann_tests.rs` and the "1% selectivity" framing testing the
post-filter oversample-retry code path in
`crates/shamir-engine/src/table/read_exec.rs` lines ~1628-1715) — but
well BELOW the ≥1k-vector size the codebase's own comment says is needed
for the HNSW path's recall to be reliable. On top of that, the test uses
an EXACT assertion (`assert_eq!(result.records.len(), k as usize)` —
literally requiring finding exactly 3 of 5 "rare"-tagged vectors) instead
of a recall-tolerance assertion.

This test is therefore straddling the WRONG side of both safe zones
established by the codebase's own design: too big for the
brute-force-exact guarantee, too small for HNSW's reliable-recall zone,
AND asserting an exact count instead of tolerating legitimate small-N
approximate-recall variance. Under heavy CPU load (contention on the
`spawn_blocking`/rayon pool that runs the HNSW search — see
`crates/shamir-index/src/vector/hnsw_adapter.rs` around line 2900-2930,
`hnsw.search(&query, overscan, ef)`), this pre-existing nondeterminism is
more likely to surface as a real recall miss.

## The fix (two parts — do both)

**Part A — make the test land in a genuinely reliable zone.** Increase
the dataset size well past the "recall-tolerance" threshold the
codebase's own comment recommends (≥1k vectors) so the HNSW graph is
well-connected enough for reliable (if still approximate) recall. A
good target: keep the "common" cluster large enough that total count
comfortably clears 1k — e.g. 2000 "common" + 5 "rare" = 2005 total
(≈0.25% selectivity — an even lower, still valid "low selectivity"
scenario for the test's stated purpose). Do NOT reduce below 256 total
(that would flip the code path to brute-force and stop exercising the
post-filter oversample-retry logic the test exists to cover).

**Part B — replace the exact-count assertion with a recall-tolerance
assertion**, consistent with the codebase's own stated philosophy
("recall-tolerance tests" for the HNSW path). Concretely: change

```rust
assert_eq!(
    result.records.len(),
    k as usize,
    "1% selectivity: must find 3 of 5 rare records; got {}",
    result.records.len()
);
```

to a tolerance check that still meaningfully validates the post-filter
path works (finds a majority of the rare records, not a literal exact
count) — e.g. assert `result.records.len() >= (k as usize).saturating_sub(1)`
(i.e. "found at least k-1 of the requested k", so ≥2 of the 3 requested
here) with a comment explaining the tolerance is for legitimate small-N
approximate-recall variance in the unseeded HNSW RNG (cite
`hnsw_adapter.rs:46-58`), NOT a correctness compromise. Keep the
`tags.iter().all(|t| t == "rare")` assertion right after it unchanged —
every record actually returned must still be correctly tag-filtered,
that part of the test's contract is unaffected by ANN approximation and
must stay an exact assertion.

Do not touch anything else in this file or in `read_exec.rs` /
`hnsw_adapter.rs` — this is a test-only fix. Do not touch
`.config/nextest.toml` (the `vr5_cofilter` override already there is
unrelated and already correct).

## Verification (do this yourself before reporting done)

1. Run the specific test in isolation at least 15 times back-to-back:
   `./scripts/test.sh -p shamir-engine -- filtered_ann_low_selectivity_finds_rare`
   — every run must pass.
2. Run the whole `filtered_ann_tests` module:
   `./scripts/test.sh -p shamir-engine -- filtered_ann`
   — must all pass, including `vr5_cofilter_sees_staged_and_filters_residual`
   (which should still pass fine standalone; its own override only
   matters under full-workspace load).
3. Run `cargo fmt --all -- --check` and
   `cargo clippy --workspace --all-targets -- -D warnings` — both must be
   clean (this is a tiny test-only diff, should not touch either).
4. Report the before/after diff and all four command outputs in your
   final summary.
