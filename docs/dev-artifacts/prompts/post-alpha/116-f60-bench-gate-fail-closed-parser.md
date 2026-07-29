# Brief for F-60 (#886, P0-R1) — perf-gate fail-closed bench parser hardening

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-R1) found that `scripts/bench_gate.sh` (landed as F-53d, commit
`2fa8bba9`) has a fail-OPEN parser: a bench-output format drift, a missing
cell, or a renamed/new workload can silently stop gating a cell without
failing CI — the opposite of what a release-blocking gate must do.

**Explicitly OUT of scope for this task** (do not attempt): actually
capturing `bench-baseline.json` on a real self-hosted machine. No such
baseline file exists in this repo yet, and none can be captured from this
environment — that remains a manual operator step per
`docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`, unchanged. This task is
PARSER-HARDENING ONLY: making the script fail closed on malformed/missing/
unexpected data, whether or not a real baseline exists yet.

### The three fail-open gaps (read `scripts/bench_gate.sh` in full before starting)

1. **Missing cell.** `WORKLOADS` (lines 89-99) names 9 expected
   `<bench>::<workload_id>` keys. The `awk` parser (lines 127-131) only
   emits a JSON line for output it successfully matches — if a bench
   binary's stdout format ever drifts (e.g. a `bench-scale-tool` version
   bump changes its print shape) the pattern silently matches ZERO lines
   for that workload, and NOTHING downstream ever notices: the gate-mode
   comparison loop (lines 182-207) only iterates over lines that exist IN
   `$JSON_LINES_FILE` — a cell simply absent from that file is never
   compared, never flagged, and the script still exits 0 if every OTHER
   cell is fine.
2. **New/renamed cell with no baseline entry.** Gate mode's per-line loop
   (lines 193-196): if a fresh cell's key isn't found in
   `bench-baseline.json`, the script prints `(new) ... not gated` and
   `continue`s — this is silent, unconditional, and never fails the gate,
   even though the whole point of a release-blocking gate is that EVERY
   cell it's supposed to cover is actually checked.
3. **Duplicate cell.** Nothing detects if the same `<bench>::<workload_id>`
   key appears more than once in `$JSON_LINES_FILE` (e.g. a bench binary
   printing an extra data line, or a copy-paste duplicate in `WORKLOADS`)
   — the comparison loop would just process both occurrences independently
   with no warning that something is structurally wrong.

## What to do

1. **Build the expected key set** from `WORKLOADS` at script start: for
   each `crate::bench::workload_id` entry, the expected key format is
   `"${bench}::${workload_id}"` (matching exactly what the `awk` parser
   and the baseline JSON already use as their key — verify this by
   reading the `--capture-baseline` writer at lines 147-167 and the
   gate-mode lookup at line 191 to confirm the exact key shape you must
   match).

2. **After `$JSON_LINES_FILE` is fully built** (right after the per-workload
   loop at lines 106-132, before the existing `FAILED_BUILDS` check),
   add a validation pass that runs in EVERY mode (gate / capture /
   json-only — a malformed parse is a bug regardless of mode):
   - Count how many times each expected key appears in
     `$JSON_LINES_FILE`.
   - Any expected key appearing **zero times** → hard error: print which
     key(s) are missing and why that's fatal (parser produced no line for
     an expected workload — either the bench crashed silently or its
     output format no longer matches the parser), then `exit 1`.
   - Any expected key appearing **more than once** → hard error: print
     which key(s) are duplicated, `exit 1`.
   - Any line in `$JSON_LINES_FILE` whose key is **not** in the expected
     set → hard error (this should be structurally impossible given the
     parser only runs once per `WORKLOADS` entry, but validate it anyway
     as a defensive check — if you find a legitimate reason multiple
     lines could appear per workload, e.g. `--scale 1` printing more than
     one row, investigate and handle correctly rather than assuming it
     can't happen).

3. **Gate mode: a fresh key with no baseline entry is now a hard error by
   default.** Change the `(new) ... not gated` branch (lines 193-196):
   by default, treat this as a gate FAILURE (increment a counter, print a
   clear diagnostic naming the unbacked key, and ensure the script exits
   non-zero) — a release gate that silently skips checking cells it
   doesn't recognize is not actually gating anything. Add ONE explicit,
   clearly-named opt-in flag (e.g. `--allow-new-cells`) that restores the
   old "(new), not gated, don't fail" behavior for the deliberate case
   where an operator is intentionally introducing a new workload before
   its baseline is captured. Document this flag in the script's own usage
   header (near the existing `--capture-baseline`/`--json-only` docs) and
   in `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md` if that doc references the
   gate's failure modes.

4. **Write tests for the new validation logic that do NOT require running
   real `cargo bench`** (those are slow and this validation logic is pure
   text processing). Two viable approaches — pick whichever fits better
   after reading the script's structure:
   - Refactor the validation logic (steps 2-3 above) into one or more
     small, separately-invocable shell functions within
     `scripts/bench_gate.sh`, then write a lightweight test script (e.g.
     `scripts/tests/bench_gate_parser_test.sh` or wherever this repo's
     convention would put it — check if `scripts/` already has any
     test-like scripts to mirror) that sources the file, feeds synthetic
     `$JSON_LINES_FILE` fixtures (missing key / duplicate key / unbacked
     new key / all-correct) and a synthetic `bench-baseline.json`
     fixture, and asserts the correct exit code and diagnostic message for
     each case.
   - If refactoring into sourceable functions is too invasive, test the
     whole script end-to-end against FAKE bench output: the orchestrator
     used this exact technique during F-53d's own zero-trust verification
     — copy `bench_gate.sh` to a scratch path INSIDE the repo (so
     `SCRIPT_DIR`/`REPO_ROOT` self-location still resolves; copying
     outside the repo breaks `cd "$SCRIPT_DIR/.." && pwd` finding
     `Cargo.toml`), stub out the `cargo bench` calls to print
     hand-crafted fixture stdout instead of actually building/running,
     and assert exit codes. Delete any scratch files before finishing.

5. **Verify the `WORKLOADS` count and any hardcoded "9" assumption stays
   consistent** — if you add/remove nothing from `WORKLOADS` itself (you
   shouldn't need to), just make sure your validation logic derives the
   expected count from `WORKLOADS` dynamically, not a hardcoded literal
   that would silently drift out of sync if `WORKLOADS` ever changes.

## What NOT to do

- Do NOT attempt to capture a real `bench-baseline.json` — no self-hosted
  runner exists in this environment; that step remains manual, per
  `docs/guide-docs/CI_PERF_GATE_RUNBOOK.md`.
- Do NOT add raw-bench-output artifact storage to `.github/workflows/perf-gate.yml`
  — the review mentions this as a nice-to-have, but it's out of scope for
  this parser-hardening task (a workflow/CI change, not a script
  correctness fix).
- Do NOT touch F-55/F-56/F-57/F-58/F-59/F-61 (other tasks from the same
  review).
- Do NOT change the `WORKLOADS` list itself (the 9 workload cells) or the
  25% `THRESHOLD_PCT` default — this task is about making the EXISTING
  gate fail closed, not about changing what it gates or how strict it is.

## Constraints

- This is a bash script, not Rust — there is no `cargo fmt`/`clippy` gate
  for it. Keep the style consistent with the rest of the file (the
  existing `set -u`, quoting conventions, `awk`/`grep` usage patterns).
- Run `bash -n scripts/bench_gate.sh` (syntax check) before finishing.
- If you add a new test script, make sure it's actually runnable
  standalone and document how to invoke it in a comment at its top.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

- `bash -n scripts/bench_gate.sh` (syntax check).
- Whatever test script/fixture approach you build (step 4) — the
  orchestrator will re-run it and personally construct at least one
  additional synthetic "missing cell" fixture to independently confirm
  the hard-fail path fires correctly.

When done, give your final summary as plain text: the exact diff, the
new `--allow-new-cells` (or equivalent) flag's behavior, the test
approach chosen and how to run it, and confirmation the syntax check and
your new tests pass.
