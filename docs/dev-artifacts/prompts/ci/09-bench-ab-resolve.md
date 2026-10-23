# hot-path A/B bench workflow — resolve bench sources from cargo metadata and take a free baseline

## Context

`.github/workflows/hot-path-ab.yml` (manual `workflow_dispatch` only, never part of the merge
gate) builds a baseline revision A and the candidate B on the same GitHub-hosted runner, runs
alternating A/B pairs of the benches listed in `scripts/bench/bench_ab.toml` through
`scripts/bench/bench_ab.py`, and writes a comparison summary and artifact. It has never been
dispatched. Two facts make it useless on current master (owner-approved ruling, DEC-3 follow-up,
bench policy by @oh):

1. `bench_ab.py` locates a bench by the flat path `crates/<crate>/benches/<name>.rs`. The
   layout sweep moved most bench sources into subdirectories (for example
   `crates/shamir-engine/benches/backend/wasm_invoke.rs`), so on master only `wal_append` is
   found.
2. The baseline revision is fixed (`e1106477`). It must be selectable.

This is a small CI-tooling PR. The owner forbids local benchmark runs; CI is the acceptance, but
the python tooling has unit tests that CI runs.

## Scope (exact)

1. `scripts/bench/bench_ab.py`: resolve every bench target through
   `cargo metadata --no-deps --format-version 1` (the `targets[]` entries with `kind` containing
   `"bench"`: name and `src_path`) instead of the flat path. The manifest keys stay
   `crate` + `bench` names; the resolved `src_path` is what the tool reads/compares. The
   byte-identical comparison between baseline and candidate ("supported / unsupported by
   fixture drift") must keep working: resolve the path in the candidate tree and in the
   baseline tree separately (the baseline tree is a checked-out worktree of the baseline SHA; a
   bench that does not exist or differs there is reported "unsupported" as today).
2. `.github/workflows/hot-path-ab.yml`: add a `baseline` workflow_dispatch input (full 40-hex
   SHA, default = the current default `e1106477...` full SHA already used by the workflow).
   Validate in the first step that it matches `^[0-9a-f]{40}$` and is an ancestor of the
   dispatched ref (`git merge-base --is-ancestor`); fail clearly otherwise. Use the validated
   value everywhere the old constant was used. Keep action pins by full SHA, the 120-minute cap,
   the artifact name/retention (90 days) and the advisory thresholds exactly as they are.
3. Tests: extend `scripts/bench/tests/` (look for `test_bench_ab.py`) for: nested bench paths are
   resolved, a missing bench is reported unsupported, the baseline-input validation helper (if
   you put the SHA validation into python, test it; if it stays in YAML shell, keep it a few
   lines and covered by a workflow-lint style test only if such a test already exists in the
   repo). Python tests run in CI; you cannot run them locally.
4. Docs: update the header comment / README of the bench tooling (where the old flat-path
   assumption and the fixed baseline are described) and note in it that the workflow is manual,
   advisory, and not part of the merge gate.

## Do not

- Do not add new benches, change bench sources, `bench-iters.txt`, thresholds or the merge gate
  (`ci.yml`, `merge_gate_check.py`). Do not dispatch anything.
- Do not touch `perf-gate.yml` (dormant, self-hosted runner) or any Rust code.
- Layout limits: at most 7 entries per folder, at most 1000 lines per file; imports at the top;
  no machine-specific paths.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index (including `git add` / `git commit`). Only edit
  files; the orchestrator commits.
- Compile / build / test policy: see the policy text at the top of this run (verbatim). Allowed
  locally for this task: `python -I scripts/ci/check_layout.py`, `python -I -m py_compile` on the
  changed scripts, `cargo metadata --no-deps --format-version 1` (read-only, to see real
  `src_path` values), running the python unit tests only if the repo documents a way to run them
  without a test runner (otherwise they are CI-only; say so). No `cargo build/test/bench`.
- NEVER stop processes by image name or pattern. Never use the shell builtin `xargs`. Capture
  exit codes without pipes. Logs under `.scratch/`.
- Before finishing, start ONE read-only verifier child (policy pasted verbatim) that reads every
  changed file and checks scope items 1-4 against the diff. At most one child at a time.

## Final message

1. Per scope item 1-4: done / not done with file:line.
2. The real `src_path` list `cargo metadata --no-deps` returned for the benches named in
   `bench_ab.toml` (so the orchestrator can see which resolve).
3. Exit codes of the allowed gates; what is CI-only.
