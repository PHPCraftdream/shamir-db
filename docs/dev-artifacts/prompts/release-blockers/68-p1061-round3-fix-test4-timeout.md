# Brief 68 — #1061 round 3: fix test 4's timeout (too slow, not a deadlock)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 2 got right — do not touch

Round 2 correctly strengthened `p1061_equivalence_with_old_path_per_value_lookup_sets`
(renamed from the old byte-comparison attempt) to compare per-distinct-value
`lookup_by_index` result sets between the old and new paths, plus verifying
each returned record's actual field content — exactly what the brief
asked for, closing the "right count, wrong records" gap. It compiled and
ran correctly (round 2's own run confirmed it produces the right
comparison logic — I independently confirmed compilation is clean).
Tests 1, 2, 3 in this file all still pass (`p1061_convergence_termination_under_sustained_load`,
`p1061_completeness_of_capture_mid_scan_mixed_ops`,
`p1061_bounded_barrier_duration_constant_across_sizes` — the last one is
correctly SLOW at ~135s, that's expected per its own design comparing 500
vs 50,000 row builds). Do not touch tests 1-3, and do not touch the
per-value comparison LOGIC in test 4 — only its fixture SIZE (see below).

## The problem — diagnosed, not guessed

Running the full `p1061_*` test group myself:

```
PASS  [   0.450s] p1061_convergence_termination_under_sustained_load
PASS  [   0.483s] p1061_completeness_of_capture_mid_scan_mixed_ops
PASS  [ 134.872s] p1061_bounded_barrier_duration_constant_across_sizes
TIMEOUT [ 180.308s] p1061_equivalence_with_old_path_per_value_lookup_sets
```

Test 4 hangs to nextest's 180s kill. This is NOT a deadlock — it's simply
too slow. `FIXTURE_SIZE = 3_000` with the fixture's value distribution
(1/5 field-absent, 2/5 sharing "dup_a", 1/5 sharing one of 3 "int_i"
values, 1/5 distinct `v_i` strings) produces roughly **~1,800 distinct
indexed values**. Round 2's per-value loop does, PER distinct value: 2
`lookup_by_index` calls (new path + old path) plus, for every RETURNED
RECORD across both paths, a separate `tbl.get(rid)` call to verify its
field content. For ~1,800 distinct values plus ~2,400 indexed rows across
BOTH tables, that's on the order of **10,000+ sequential awaited async
calls** in one test — each cheap individually, but the sum comfortably
exceeds 180s. (Compare: round 1's ORIGINAL weaker version of this same
test, same 3,000-row fixture, ran in 1.6s — because it only did ONE
`.len()` comparison, no per-value loop.)

## Fix — reduce ONLY this test's fixture size, keep the same distribution shape

Change `FIXTURE_SIZE` for `p1061_equivalence_with_old_path_per_value_lookup_sets`
from `3_000` down to **`300`** (10x smaller) — keep the EXACT SAME `i % 5`
distribution logic (field-absent / two "dup_a" buckets / "int_i" buckets /
distinct `v_i` strings) so the test still exercises collisions,
field-absent rows, and distinct values, just at a scale that produces
~180 distinct values (down from ~1,800) — roughly a 10x reduction in the
per-value loop's iteration count, which should bring total wall time down
from 180s+ to a few seconds (matching the proportional slowdown observed:
3,000→1.6s pre-strengthening, so 300 with the SAME loop structure that
took >180s at 3,000 should land well under nextest's 30s slow-warning
threshold).

**Do not reduce it further than necessary to just "make it pass fast"** —
300 should keep this a meaningful test (still exercises multiple
collision buckets and dozens of distinct single-record values), not a
token 3-row check. If 300 still measures slow in your run, you may reduce
further (try 200, then 100) but report the actual measured time so the
orchestrator can judge whether the test still meaningfully exercises the
comparison logic at whatever size you land on.

**Also keep the existing "both-sides-empty false-pass" guard** — adjust
its threshold proportionally if it references `FIXTURE_SIZE` directly
(e.g. `postings.len() > FIXTURE_SIZE / 2`), it should still hold
automatically since it's a ratio, not an absolute number.

## After the fix — re-run and confirm

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p1061_
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

All 4 `p1061_*` tests must show `PASS`, not `TIMEOUT`. Report the ACTUAL
measured wall time for test 4 after your fix (not just "it passed") —
paste the exact nextest line. If it's still uncomfortably slow (say, over
30s), say so and explain what you tried, rather than silently shipping a
borderline-slow test.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff (should be a small, localized change — just the
fixture size constant for this one test) and the exact nextest output for
all 4 `p1061_*` tests plus the full suite's final summary line.
