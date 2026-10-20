# CI-02 — layout sweep: repository scripts and client-ts root

## Context

`scripts/ci/check_layout.py` (merged in #187) enforces at most 7 direct
entries per folder and at most 1000 physical lines per `.rs`/`.ts`/`.py`/`.sh`
file; current violations are grandfathered in `scripts/ci/layout_baseline.txt`.
The owner decided (2026-10-16) to sweep them until the baseline is empty.
Earlier sweeps (query crates #190, shamir-index #193, client-ts #192) are
merged; this PR sweeps exactly the scope below. It is a PURE STRUCTURAL
REFACTOR: behaviour, public API paths, error behaviour, test semantics and
test names stay identical.

## Scope

Two baseline entries only (the third, `crates/shamir-client-node`, is deliberately NOT in scope: it needs an owner decision about the napi build flags):

- `dir scripts` (15 direct entries, limit 7).
- `dir crates/shamir-client-ts` (8 direct entries, limit 7).

Remove exactly those two entries from the baseline with `python -I scripts/ci/check_layout.py --update-baseline` (it only lowers or removes entries) after `git add -A` (the checker sees only tracked files), and the checker must exit 0 with `crates/shamir-client-node` as the only remaining non-engine entry.

### scripts/

Current entries: README.md bench-vector.sh bench_ab.py bench_ab.toml bench_gate.sh ci-qemu-numa-test.sh ci test-all.ps1 test-all.sh test.sh tests ts wsl-flame-bench.sh wsl-flame.sh wsl-perf-report.sh. `scripts/test.sh` stays where it is (the central test entry point, referenced by `.cargo/config.toml` aliases, CI, CLAUDE.md and the perimeter guard); `scripts/ci/` and `scripts/ts/` stay. Suggested grouping (adjust if the references say otherwise): `scripts/bench/` (bench-vector.sh, bench_ab.py, bench_ab.toml, bench_gate.sh and `scripts/tests` content that tests them), `scripts/wsl/` (wsl-*.sh), `scripts/runners/` (test-all.ps1, test-all.sh, ci-qemu-numa-test.sh). Result must have at most 7 direct entries in `scripts/` and in every folder you create.

Update EVERY reference to a moved file: `.github/workflows/*.yml` (path filters such as `scripts/bench_ab*` and `scripts/tests/test_bench_ab.py` in hot-path-ab.yml, the `control/scripts/bench_ab.py` invocations, numa.yml's `scripts/ci-qemu-numa-test.sh`), the python/shell files themselves (relative paths, `bench_ab.toml` lookups, `sys.path`, `source`/`bash` of siblings, `unittest discover -s` directories), `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md`, `docs/**`, `.cargo/config.toml`, `.config/nextest.toml`, `.githooks/`. Find them with `git grep -n` for every moved basename and every moved path BEFORE and AFTER the move; no stale reference may remain outside historical docs under `docs/dev-artifacts/` (leave those untouched). Do not edit the content of the moved scripts beyond path references. Keep executable bits (`git update-index` is forbidden for you: the orchestrator fixes modes; list the moved shell scripts in your final message).

### crates/shamir-client-ts

Current entries: .gitignore configs docs package-lock.json package.json src tsconfig.json vitest.config.ts. Reduce to at most 7 by moving `vitest.config.ts` into `configs/` (update the npm scripts in package.json and every CI/workflow/doc reference to the vitest config path, and fix relative imports/paths inside the moved config file) or by another reference-safe move of one entry; `tsconfig.json`, `package.json` and `package-lock.json` stay at the root. The `ts unit (shamir-client-ts)` CI job runs the package's npm scripts, so the scripts must keep working unchanged from the caller's point of view.

## How to split

- Folders: group entries into topical subfolders (for tests:
  `tests/<topic>/…`), each subfolder with its own `mod.rs` manifest
  (re-exports only). Keep every public Rust path stable with `pub use`
  re-exports from the original module so no caller in any crate changes
  (prefer this over editing callers).
- Files: split along their seams (separate `impl` blocks, helper groups,
  test topics), each new file keeping "one file = one primary export";
  `impl` blocks of one type may live in several files of the same module.
  Do not change signatures or visibility beyond what the move needs
  (`pub(super)` -> `pub(crate)` only if a split genuinely requires it).
- A crate root folder (`crates/<crate>/` itself, holding Cargo.toml, build
  scripts, configs) may not be reducible below 8 without breaking tooling;
  if so leave that single entry in the baseline and explain why in the final
  message. Everything else must reach the limit.
- Tests: moved test modules keep their test function names; nextest filters
  by module path change, which is acceptable. Never lose a test: every moved
  test module stays wired in through its `tests/mod.rs` and the parent
  `#[cfg(test)] mod tests;`.

## Lessons from the earlier sweeps (CI found 20+ errors of these classes — check each yourself with `cargo check` / `clippy`)

- a new module file must be declared with `mod x;` exactly once in the right parent;
- `super::` / `crate::` paths change when a file moves one level deeper;
- an `impl` moved into a sibling/child module loses access to private fields and methods of its type: keep it in a submodule of the type's own module, or widen visibility only as far as needed;
- imports left unused after code moved out must be removed; test-only imports need `#[cfg(test)]`;
- a module and a re-exported item with the same name collide; a private item must not shadow a glob re-export;
- `#[path = "..."]` mounts and `include!` use paths relative to the file that contains them — re-check every one after a move;
- a closing brace lost at the end of a split file (check balance of every file you cut).

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator
  commits. Moving a file = create the new file with your file tool and
  delete the old one with your file tool (never `git mv`).
- Compile / build / test policy: no `cargo test` / `build` / `nextest` /
  `./scripts/test.sh` / `npm` / `tsc`, by you or any child agent. Allowed
  locally, scoped: `cargo check -p <crate> --all-targets`,
  `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo fmt -p <crate>`, `python -I scripts/ci/check_layout.py`
  (also for every dependent crate if a public path could change). CI is the
  gate for tests.
- Never use the shell builtin `xargs` (crashes the rush session). Capture exit
  codes without pipes. Logs under `.scratch/`. One cargo process at a time,
  `CARGO_BUILD_JOBS=4`.
- Never silence a lint with `allow`. Imports at the top of each file;
  `mod.rs` files only re-export. No machine-specific absolute paths.
- Before finishing, one verifier child agent (the whole policy pasted
  verbatim) reads in full every new/changed file and checks: every moved item
  is still reachable at its old public path, every `mod` declaration matches a
  file, no item was dropped or duplicated (compare item lists before/after
  with read-only `git show origin/master:<path>`), imports resolve, test
  modules are all wired into their `tests/mod.rs`, braces balance.

## Final message

The before/after entry count of every folder and line count of every file in
scope, the checker's final output, the real exit codes of check/clippy/fmt,
and any baseline entry intentionally left (with the reason).
