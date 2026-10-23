# hot-path-ab: `ratio` operation (warm/cold disk-cache decision) + manifest entries for the compile cells

## Context

The wasm compile cells `compile_cold/{fn64,fn256}` and `compile_warm/{fn64,fn256}`
(`crates/shamir-engine/benches/backend/wasm_invoke.rs`) and `boot_k_functions/{cold,warm}/{16,64}`
(`crates/shamir-db/benches/engine_perf.rs`) are merged. Owner ruling (bench policy by @oh): the wasm
disk-cache tier is kept only if it pays: the decision statistic is the ratio of the median WARM time to
the median COLD time **within one revision** (not an A/B across revisions). Rule: the larger of the
`fn64`/`fn256` median warm/cold ratios > 0.8 means the disk tier does not pay and a later PR removes it;
a ratio within 0.7..0.9 is "borderline" and gets one confirmation with 10 pairs instead of 5.

`scripts/bench/bench_ab.py` (read it entirely, together with `scripts/bench/bench_ab.toml`,
`scripts/bench/test_bench_ab.py` — find the real test location with `git ls-files scripts/bench`,
`.github/workflows/hot-path-ab.yml`, `bench-iters.txt` header, and the bench-scale-tool README in the
cargo registry for the `--calibrate` / `--scale` / output format) has the operations `compare` and
`calibrate`. It runs only on the manual, advisory workflow; never in the merge gate. Benches run only in
CI: you cannot run them, and you must not.

## Scope (exact)

1. `bench_ab.toml`: register the four compile cells (`class = "cpu"`, crate `shamir-engine`, bench
   `wasm_invoke`, cell ids exactly `compile_cold/fn64`, `compile_cold/fn256`, `compile_warm/fn64`,
   `compile_warm/fn256`) and the four boot cells (crate `shamir-db`, bench `engine_perf`, class `io`,
   `boot_k_functions/cold/16`, `.../cold/64`, `.../warm/16`, `.../warm/64`) with the same rank/optional
   conventions as neighbours (read why `optional` is used there). Remember the harness filters by
   SUBSTRING: make sure a requested key does not accidentally match a sibling cell (for example
   `compile_cold/fn64` vs a hypothetical `fn640`) and that `preflight_cell`'s expectations still hold.
2. `bench_ab.py`: new `--operation ratio` (choices become `compare`, `calibrate`, `ratio`):
   - candidate revision only (no baseline build; the baseline input is ignored and says so in the plan);
   - for each ratio pair declared in the manifest (`[[ratios]]` table with `cold`/`warm` cell keys and a
     `threshold = 0.8`, `borderline = [0.7, 0.9]`), run the cold and warm cells of the same built binary
     in alternating order for N pairs (5 by default, 10 when `--pairs 10` is given; keep the first warm-up
     pair discard convention of `compare`), take medians (use the same `paired`/median helpers where they
     fit), and compute `ratio = median(warm) / median(cold)` per pair of cells;
   - the verdict: `pays` (ratio < 0.7 for the larger of the two), `borderline`, `does-not-pay`
     (> 0.9 or > threshold per the stated rule: implement exactly "larger ratio > 0.8 -> does-not-pay;
     within [0.7, 0.9] and not > 0.8 -> borderline needing a 10-pair confirmation; otherwise pays" and
     document the exact precedence in the docstring);
   - `results.json` / `report.md` get a ratio table (cell pair, cold median ns, warm median ns, ratio,
     verdict); `verdict()` returns 0 for ratio runs (advisory), the decision is read from the report;
   - the workflow gets `operation: ratio` as a dispatch choice and an optional `pairs` input (5/10).
3. Unit tests next to the existing bench_ab tests: ratio statistics (known medians -> known ratio),
   verdict precedence including the boundary values 0.7, 0.8, 0.9, the manifest parse of `[[ratios]]`,
   the plan for `ratio` ignores baseline, and the substring-collision guard. Python tests run in CI.
4. Docs: the tool header comment and the bench README (if one exists under `scripts/bench`) describe
   `ratio`, the decision rule and the order of operations: (a) dispatch `calibrate` to produce
   iteration counts for the new cells, (b) a human commits `bench-iters.txt`, (c) dispatch `ratio`.

## Do not

- Do not edit `bench-iters.txt` (calibration is a later, human-committed step), thresholds of existing
  classes, the merge gate, `perf-gate.yml`, or any Rust code. No new dependencies.
- Layout limits: at most 7 entries per folder, 1000 lines per file (check `bench_ab.py` size: if it would
  exceed 1000 lines, put the ratio logic into a sibling module under `scripts/bench/` and import it).
  Imports at the top; no machine-specific paths; terse comments.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that
  mutates the working tree or index. Only edit files; the orchestrator commits.
- Compile / build / test policy: see the policy text at the top of this run (verbatim). Allowed locally:
  `python -I scripts/ci/check_layout.py`, `python -I -m py_compile` on changed python files,
  `cargo metadata --no-deps --format-version 1` (read-only). Python unit tests: CI-only unless the repo
  documents a no-runner way (then say so). No cargo build/test/bench.
- NEVER stop processes by image name or pattern. Never use the shell builtin `xargs`. Capture exit codes
  without pipes. Logs under `.scratch/`.
- Before finishing, start ONE read-only verifier child (the whole policy pasted verbatim) that reads the
  diff and checks scope items 1-4; one child at a time.

## Final message

1. Per scope item: done / not done with file:line.
2. The exact verdict precedence implemented and the boundary-value test table.
3. Exit codes of the allowed gates; what is CI-only.
