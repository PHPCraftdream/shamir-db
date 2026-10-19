# CI-03a — `rust-guard`: source-integrity gates for large structural refactors

## Why

The layout sweep of `shamir-engine` (tx/, repo/, table/, query/, validator/) moves
and splits ~150 files. Every earlier sweep in this repo broke module paths,
privacy, braces or **silently lost tests** (a dropped `mod x_tests;` still
compiles). Builds and tests run only in GitHub CI and executors can run only
scoped `cargo check` / `clippy` / `fmt` locally, so the safety net must be
mechanical, run in CI on draft AND ready PRs, and fail closed. This brief
implements the *static* half (CI-03a). The compiled half — nextest discovery
diff, rustdoc gate, feature matrix, aggregate merge gate — is CI-03b and is NOT
part of this task.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`
  or any git command that mutates the working tree or index. Only edit files;
  the orchestrator commits.
- Compile / build / test policy (CLAUDE.md "🚦 Compile / build / test policy"):
  no `cargo test`/`build`/`run`/`bench`/`doc`, no nextest, no
  `./scripts/test.sh` locally. Allowed locally: `cargo check`, `cargo clippy
  -- -D warnings`, `cargo fmt` (scoped), `python -I scripts/ci/check_layout.py`.
  The tool's own tests run in CI.
- Never use the shell builtin `xargs` (crashes the rush session). Capture exit
  codes without pipes (`cmd > log 2>&1; echo exit=$?`). Logs under `.scratch/`.
- Layout limits apply to the tool itself: <= 7 entries per folder (README and
  tests/ count), <= 1000 lines per file, imports at the top, `mod.rs`
  re-exports only, no `#[allow]` to silence a lint.

## Deliverables

### 1. `tools/rust-guard/` — a Rust tool (workspace-excluded)

A small binary crate outside `crates/*` (add `tools/rust-guard` to the root
`Cargo.toml` `exclude`, keep its own `Cargo.lock`, pin `syn` with
`features = ["full", "visit", "extra-traits"]` or the minimum needed,
`proc-macro2` with `span-locations`, `toml`, `serde`, `clap`; no async, no
network). It parses Rust with `syn` (NOT regex), reads files from git objects
(`git ls-files -z`, `git show <rev>:<path>`), exits non-zero on any finding and
prints findings as `path:line: code: message`. Subcommands:

#### `modules` — module reachability (fail-closed)

Every tracked `.rs` file under `crates/*/` and `tools/*/` must be accounted
for: reachable from a target root, or listed in `exceptions.toml`.
Algorithm:
1. Discover package/target roots from `cargo metadata --no-deps --format-version 1`
   plus the manifests of workspace-excluded packages (shamir-client-node,
   tools/*): lib, bin, test, bench, example targets with explicit paths and
   auto-discovery (`src/lib.rs`, `src/main.rs`, `src/bin/*.rs`,
   `src/bin/*/main.rs`, `tests/*.rs`, `tests/*/main.rs`, `benches/*.rs`,
   `examples/*.rs`, `build.rs`). Do NOT treat every file under `tests/`
   `benches/` `examples/` as a root: `tests/clean_horizon/crash_matrix.rs` is
   mounted by `tests/crash_recovery.rs` and its mere existence must not make it
   reachable.
2. Traverse `mod x;` and inline `mod x { ... }` carrying the physical path, the
   logical module path, the module-directory resolution context and the
   inherited `cfg` predicate. Implement `foo.rs` vs `foo/mod.rs` vs non-mod-rs
   parent (`foo.rs` -> children in `foo/`) resolution, reject ambiguity
   (both `x.rs` and `x/mod.rs`), literal `#[path = "..."]` and
   `#[cfg_attr(<pred>, path = "...")]` with Rust's contextual rules (relative
   to the directory of the file that contains the `mod`, inline modules extend
   the context).
3. Keep every cfg alternative with its predicate, do not erase `#[cfg]`:
   `#[cfg(test)]` modules are reachable in the test configuration; a module
   under an unsatisfiable predicate (`#[cfg(any())]`) is reported; mutually
   exclusive `cfg_attr` path alternatives are not duplicate modules.
4. `include!("literal")` of a tracked file is an explicit source edge;
   generated (`OUT_DIR`) includes and opaque module-producing macros must be
   declared in `exceptions.toml` with producer + owner + reason — otherwise
   fail.
5. Findings: unaccounted tracked `.rs` file (the lost-test case),
   `mod x;` whose file does not exist, ambiguous resolution, unsatisfiable
   module, duplicate physical file reached by two logical paths without a
   declared reason, stale exception.
Exceptions: exact path (no directory wildcards), package, reason, owner; stale
exceptions fail.

#### `conservation` — source inventory diff (base vs head)

`rust-guard conservation --base <rev> --head <rev> [--packages p,q] --delta deltas/<stage>.toml`.
Build an inventory from immutable git objects for both revisions: per item
package/target membership, kind, name, enclosing type/trait identity,
normalised signature, `impl` head, cfg predicate, test attributes
(`#[test]`, `#[tokio::test]`, `proptest!`, `#[cfg(loom)] mod`-style loom models,
`#[ignore]` status), and a token-normalised BODY FINGERPRINT (strip whitespace,
comments, and the module path of the item; keep identifiers/literals/tokens).
Include associated items; treat split `impl` blocks by their member sets (one
block split into three is not new behaviour). Matching ignores module location
for unchanged items; renames and extractions must be declared in the delta file.
Findings: item lost, item added, body fingerprint changed, test lost or newly
`#[ignore]`d, test attribute/cfg changed, duplicate that cannot be paired,
unused allowance. Names-only counting is explicitly rejected (deleting one
`fn new` and adding another must fail). Delta file: TOML, per stage, names the
expected base identity (`base = "<sha>"`), lists exact `[[moved]]`,
`[[renamed]]`, `[[extracted]]`, `[[removed]]`, `[[added]]`, `[[changed]]`
entries with a rationale each. A missing base commit is an explicit
"blocked comparison", never an empty baseline.

#### `structure` — AST rules for changed/moved areas

`rust-guard structure --paths <glob>...` (or `--changed-since <rev>`):
- `mod.rs` contains only attributes, docs, `mod` declarations, `use`/`pub use`
  (re-exports) — no fns/types/impls/macros;
- `tests/mod.rs` is a pure manifest (`mod` declarations, nothing else);
- no inline `#[cfg(test)] mod tests { ... }` implementation in production
  files (loom model modules included unless listed in exceptions);
- no function-local `use` except documented exceptions (`use super::*;` inside
  test modules, a collision comment, cfg-gated bodies);
- every inherent `impl` whose resolved self type is the engine's `RepoInstance`
  lives in the defining module or its descendants (configurable assertion:
  `[[owned_impl]] type = "RepoInstance" module = "crate::repo::instance::repo_instance"`);
- no new `pub(crate)`/`pub` widening or `#[allow(...)]` compared with base in
  changed files unless the delta file lists it.
"One primary export per file" is NOT enforced by counting declarations; offer
it as an opt-in warning only.

### 2. Baseline integrity (Python, `scripts/ci/check_baseline_monotonic.py`)

Compares `scripts/ci/layout_baseline.txt` at HEAD against the base ref
(`origin/<base>` in PRs, `--base <rev>` locally): fail on any new entry and on
any entry whose recorded number grew. Reason: `check_layout.py` checks HEAD
against a manually enlarged HEAD baseline, which is no protection. Unit tests in
`scripts/ci/tests/`.

### 3. CI wiring (`.github/workflows/ci.yml`)

An unconditional job `source-integrity` that runs on draft AND ready PRs
(before expensive work; `needs:` nothing): builds `tools/rust-guard` with the
rust-cache, runs `modules` over the whole repo, `structure --changed-since
<merge-base>`, `conservation` only when the diff touches Rust files of the
packages named in `.rust-guard/stages/*.toml` (configuration file listing
active campaigns: packages + delta file path; empty by default so the job is a
no-op for ordinary PRs), and `check_baseline_monotonic.py`. A second job
`rust-guard self-test` runs `cargo test --manifest-path tools/rust-guard/Cargo.toml`
(CI only) with FIXTURES: deliberately broken trees that the guard must reject —
dropped `mod x_tests;`, impossible cfg, stale `#[path]`, a raw string / comment
containing `mod x;`, duplicate item names, renamed tests, altered `#[ignore]`,
macro-generated tests, a sibling-of-defining-module `impl RepoInstance`, and
compiler-accepted fixtures for the `#[path]` resolution rules. Put the fixtures
under `tools/rust-guard/tests/fixtures/` (keep folder entry limits by grouping
into topical subfolders). Document the tool in `tools/rust-guard/README.md`
(counts as an entry!) and CLAUDE.md "Code quality" (one short subsection:
what the guard enforces, how to add an exception, how to write a delta file).

## Not in scope

nextest discovery diff, rustdoc gate, feature matrix, aggregate required-check
job (CI-03b); any change to engine code; any change to `check_layout.py`'s
rules.

## Final message

Per deliverable: files and line counts (all <= 1000), the fixture list with
which finding each triggers, what fails closed, local gate exit codes
(`cargo check --manifest-path tools/rust-guard/Cargo.toml --all-targets`,
`cargo clippy ... -- -D warnings`, `cargo fmt --manifest-path ...`, the layout
checker, `python -I` unit tests are CI-only), and every limitation you knowingly
left (parser drift, macros, cfg alternatives).
