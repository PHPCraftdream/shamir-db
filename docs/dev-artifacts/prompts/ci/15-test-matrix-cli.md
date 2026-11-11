# Brief: `test_matrix.py` — compute the test matrix for a commit range (CI-MATRIX-CLI, shadow mode)

## Why

Owner requirement: the tests of a crate run only when a change really affects that crate — its own code or
something it depends on. A change travels up the dependency chain to the top crates (`shamir-server`,
`shamir-e2e`), which then run only their own tests; sibling branches of the graph are not triggered. The selection
is computed automatically by ONE CLI command from a commit range. No event selects the whole workspace by itself.

Today's selector (`scripts/ci/affected_crates.py`, read it and its tests first) is crate-level and coarse.
Measured on the 31 changes landed after the crate regroup:

* 9 of 31 fell back to the whole workspace: `Cargo.lock` (6), `.config/nextest.toml` (2), `ci.yml` + scripts (1),
  `bench-iters.txt` (1).
* A test-only change selects every dependent crate (9 and 5 crates where 1 is enough).
* Dev-dependency edges are followed transitively (`shamir-test-diag` selects 11 crates, 7 are really affected;
  `shamir-bench-utils` 10 instead of 4).
* `push`, `workflow_dispatch` and `schedule` always select `--workspace`.
* Edges that are NOT in the cargo graph are missed: `shamir-wasm-host` compiles guest crates against `shamir-sdk`
  by path (`crates/wasm/shamir-wasm-host/src/compile.rs`), so an SDK-only change runs only the SDK's tests; the
  TypeScript parity test reads `crates/query/shamir-query-builder/tests/fixtures/wire_ops_matrix.json`; TypeScript
  tests read `crates/query/shamir-query-types/test-vectors/`; `shamir-server` tests `include_str!` `deploy/*.ktav`.

This stage builds the command and runs it in CI in SHADOW mode: CI keeps using the old selector for what it runs.
The switch is the next stage (`16-test-matrix-switch.md`, not part of this brief).

## What

### 1. The command

`python3 -I scripts/ci/matrix/test_matrix.py` (Python 3 standard library only; `tomllib` is available). Inputs:

* `--range A..B` (two-dot, explicit) or `--base <ref> [--head <ref>]` (merge-base three-dot, the PR case);
  `--event pull_request|push|workflow_dispatch|schedule` with `--before/--after` for push;
* `--metadata <file>` (pre-captured `cargo metadata --format-version 1 --no-deps --locked`), `--changed <file>`
  (newline list, for tests) so the core is pure and testable without git or cargo;
* `--format json|github-output|explain` (default `json`);
* `--backtest <rev-range>`: for every first-parent commit print old selection (via the existing selector's pure
  core) vs new selection, sizes and reasons — one line per commit plus a summary;
* `--check`: the self-checks of items 4 and 6 (exit non-zero with the list).

Output (`json`): `{"version":1,"base":..,"head":..,"scope":"none|bounded|all","all_reasons":[..],
"crates":[{"name":..,"lib":bool,"integration":"all"|[names]|[],"check_only":bool,"reasons":[..]}],
"flags":{"engine":..,"ts":..,"node":..,"wasm32":..,"deny":..,"pipeline":..,"docs_only":..},"args":"-p a -p b"}`.
Every selected crate carries at least one human-readable reason ("changed: <path> (+N)", "depends on X (normal)",
"dev-depends on X", "declared edge: <reason>", "lock: hyper 1.11.0 -> 1.12.0"). `explain` prints a table for humans
(crate, what runs, why). `github-output` prints `matrix=<one-line json>`, `args=`, and the flags, validated like the
existing script. Never print a partial result: any failure is a non-zero exit with a message on stderr.

### 2. File roles (inside a workspace package; nearest manifest wins, as today)

* `tests/<bin>.rs`, `tests/<bin>/**` -> integration binary `<bin>` of that crate only. Map names through the
  `targets` of `cargo metadata` (kind `test`, `src_path`); a directory under `tests/` that is not a target (shared
  helpers, fixtures) -> all integration binaries of the crate.
* `src/**/tests/**`, `src/**/*_tests.rs` -> unit tests: the crate's lib tests only.
* `benches/**`, `examples/**` -> `check_only` for that crate (clippy `--all-targets`), no tests.
* `*.md` inside a package -> nothing, unless the crate's sources reference the file name in `include_str!` /
  `#[doc = include_str!(..)]` (then production).
* Everything else in the package (`src/**`, `Cargo.toml`, `build.rs`, other files) -> production.

### 3. Propagation

* Production change in X: X runs lib + all integration; every crate reachable from X over reverse normal/build
  edges (transitively) runs lib + all integration; every crate that DEV-depends on any crate of that set runs lib
  + all integration, and propagation stops there (dev edges are terminal).
* Unit-test change: that crate's lib tests, nothing else. Integration-binary change: that binary, nothing else.
* Results merge per crate (union; `all` absorbs lists; tests absorb `check_only`).

### 4. Declared hidden edges: `scripts/ci/matrix/edges.toml`

`[[edge]] paths = [globs], crates = [names], propagate = "tests"|"prod", flags = [names], reason = "..."`:
a changed path matching `paths` selects `crates` (`tests`: those crates' tests, terminal; `prod`: as if their
production code changed) and sets `flags`. Seed it with the four edges listed in "Why" (SDK and SDK macros ->
`shamir-wasm-host` with `prod`; the query-builder fixture and the query-types test vectors -> flag `ts`;
`deploy/**` -> `shamir-server` with `tests`) and with `canary = [..]` (see item 5).
`--check` scans `crates/**/*.rs` for `include_str!`, `include_bytes!` and
`concat!(env!("CARGO_MANIFEST_DIR"), "..")` literals that resolve outside the reading package, and the
`shamir-client-ts` tests for relative paths that leave the TypeScript package; each such target must be inside a
package the reader depends on (any edge kind) or be covered by an `[[edge]]` that names the reader (or the `ts`
flag). Anything else fails `--check` with file:line. State the limits of this scan in the module doc.

### 5. Paths outside packages (ordered rules; the first match wins; every rule records its reason)

| Path | Result |
|---|---|
| `Cargo.lock` | lock diff: parse base and head; changed/added/removed third-party packages -> the workspace members that reach them in the lock graph (head graph; base graph for removed ones) are marked as production changes; a member whose own dependency list changed is marked; unparsable -> `all` |
| root `Cargo.toml` | parsed diff: only `workspace.dependencies.<n>` changed -> members that use `<n>.workspace = true` are marked production; only `workspace.members`/`exclude` -> nothing by itself; anything else (`profile`, `workspace.lints`, `patch`, `workspace.package`, resolver) -> `all` |
| `rust-toolchain*`, `.cargo/**`, `scripts/test.sh` | `all` (this is a true answer, not a fallback) |
| `.config/nextest.toml` | if every changed line lies in override blocks whose filter names `package(<crate>)` -> those crates' tests (terminal); otherwise `all` |
| `.github/workflows/ci.yml`, `.github/actions/**` | flag `pipeline`; crates = the `canary` list of `edges.toml` (one small crate per job kind) |
| other `.github/**`, `scripts/**` (except `test.sh`), `.rust-guard/**`, `tools/rust-guard/**`, `bench-iters.txt`, `.gitignore`, `.gitattributes`, `.githooks/**`, `LICENSE*`, `docs/**`, `*.md` | nothing (their own jobs cover them) |
| `deny.toml` | flag `deny` only |
| `clippy.toml` | every crate `check_only` |
| `crates/client/shamir-client-ts/**` | flag `ts` |
| anything unclassified | `all`, reason "unclassified path: <p>" (fail safe; `--check` lists unclassified tracked top-level entries so the table stays complete) |

Renames count both the old and the new path. Flags keep today's meaning (`engine`, `ts`, `node`, `docs_only`);
`wasm32` is true when the guest crates of the current wasm32 step are selected.

### 6. Guard for the test-path rule

`--check` also verifies that every module declaration that points into a unit-test path (`mod tests;`,
`mod *_tests;`, `#[path = "...tests..."]`) is gated by a `cfg` attribute containing `test`; a file that fails this
is treated as production by the selector and reported. Keep it textual and simple; document its limits.

### 7. Shadow mode in CI

In `.github/workflows/ci.yml`: the `affected` job (full tier) and the `fast` job compute the new matrix and append
`--format explain` plus the old selection to `$GITHUB_STEP_SUMMARY`; a failure of the new command is a
`::warning::`, never a job failure; `--check` runs as a real step next to the layout check (it must pass). Nothing
else in what CI runs changes in this stage: every job still consumes the old selector's outputs.

### 8. Layout

`scripts/ci/` and `scripts/ci/tests/` are at 7 entries each. Put the new code in `scripts/ci/matrix/` (the CLI
entry plus small modules, <=7 entries, <=1000 lines each) and its tests in `scripts/ci/tests/matrix/`. To stay
within 7 entries move `affected_crates.py` into `scripts/ci/matrix/` and `test_affected_crates.py` into
`scripts/ci/tests/matrix/`, updating EVERY reference (`ci.yml`, hooks, docs, `CLAUDE.md`, tests; grep for both
names). If another arrangement is smaller, use it and say why. `python3 -I -m unittest discover -s scripts/ci/tests`
must still find every test.

## Tests (unit tests run in CI, not here)

Pure-function tests over hand-built metadata, lock texts and file lists: each role; dev edges terminal; the four
seeded edges; lock diff (bump of a leaf package, added dependency of one member, removed package, unparsable);
root manifest diff (workspace dependency, profile); each row of the path table; renames; merge rules; reasons
present for every selected crate; output formats; `--check` positives and negatives. Property test over random
small graphs: a crate whose production file changed, and every crate with a normal/build path to it, and their
direct dev-dependents, are always selected with tests (the selection is never narrower than this set). The
existing selector tests stay green (moved, not weakened).

## Observable acceptance

`python -I scripts/ci/check_layout.py` passes on the final tree; `--check` passes on the repository;
`--backtest b9d837e49..origin/master` runs and its summary is pasted into the final report (old vs new sizes, count
of `all` results and their reasons); for every commit of that range the new selection contains every crate whose
production code the commit changed. Docs: `CLAUDE.md` "CI tiers" gets one short paragraph (the command, shadow
mode, where the edges file is); `docs/dev-artifacts/prompts/ci/` keeps this brief.

## Constraints

Compilation, builds and tests run on GitHub CI, not on this machine. Banned unless the user explicitly asks:
`cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`,
`npm`/`tsc`/`vitest` builds and test runs, WASM builds, e2e harnesses, any release or full-workspace build, and
the Python unit-test suite (`unittest`) — CI runs it. Allowed locally, only these: `cargo check -p <crate>
--all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` (none of them is
needed here: no Rust changes), `python -I scripts/ci/check_layout.py`, `cargo metadata --locked --no-deps
--format-version 1`, running the new command itself (`--explain`, `--backtest`, `--check`), read-only git. If you
believe you need a build or test run, say so in your final message.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (File moves: create the new file and delete the
old one with ordinary file operations; `git add -A` only for the layout check.)

Threat model: none; this stage changes no access, identity or data path and does not change what CI executes.

Your last message must be a final report: what was built, the layout arrangement, the backtest summary, `--check`
findings (every undeclared edge you had to declare, with the reason), anything unverified. Do not end on a tool call.
