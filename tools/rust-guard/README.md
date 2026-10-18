# rust-guard

Static source-integrity gates for this repository (CI-03a). A small
workspace-excluded binary that reads files from git objects (`git
ls-files -z`, `git show <rev>:<path>`) or the working tree, parses Rust
with `syn` (never regex), and **fails closed**: any finding exits
non-zero and prints

```
path:line: CODE: message
```

sorted, to stderr. Builds and self-tests run in GitHub CI only — do not
run them locally (see CLAUDE.md "Compile / build / test policy").

## Global flag: `--repo <path>`

Every subcommand accepts a global `--repo <path>` (default: `.`) that
names the repository root to operate on. The integration self-tests use
it to point the built binary at throwaway fixture repositories under
the OS temp dir. Ancillary config files (`.rust-guard/exceptions.toml`,
`.rust-guard/owned_impl.toml`) remain relative to the process working
directory unless an explicit `--exceptions` path is given.

## Subcommands

- `rust-guard modules [--rev <rev>] [--exceptions <path>]` — module
  reachability. Discovers package/target roots (`cargo metadata
  --no-deps` members plus workspace-excluded packages and `tools/*`;
  `vendor/*` is out of scope), resolves every `mod x;` / inline
  `mod x { }` with Rust's contextual rules (`x.rs` vs `x/mod.rs` vs the
  non-mod-rs parent directory, literal `#[path]`, `#[cfg_attr(pred,
  path = "...")]`; an inline module appends its name to the resolution
  directory, so `mod a { mod b; }` resolves `b` at `a/b.rs`, and
  inline-in-inline composes), and keeps every cfg predicate attached. Findings:
  `UNACCOUNTED_FILE` (a tracked `.rs` under `crates/*/` or `tools/*/`
  reachable from no root — the lost-test case), `MISSING_MOD_FILE`,
  `AMBIGUOUS_MOD`, `UNSATISFIABLE_CFG`, `DUPLICATE_FILE`,
  `STALE_EXCEPTION`, `UNRESOLVED_INCLUDE`, `UNRESOLVED_MACRO`,
  `PARSE_FAILED`.
- `rust-guard structure [--paths <glob>...]... [--changed-since <rev>]
  [--base <rev>] [--delta <path>] [--exceptions <path>]
  [--warn-primary-export]` — AST rules over the selected files (at least
  one of `--paths` / `--changed-since` required; globs match
  git-tracked paths). Finding codes:
  - `MODRS_LOGIC` — a `mod.rs` holds anything beyond attributes/doc
    comments, `mod` declarations and `use`/`pub use` re-exports (no fns,
    types, impls, macros, constants).
  - `TESTS_MANIFEST` — a `tests/mod.rs` is not a pure `mod` manifest
    (not even `use` is allowed).
  - `INLINE_TEST_MOD` — an inline test-only module (`#[cfg(test)] mod
    tests { .. }`, `mod *_tests`, or a `loom`/`*_loom` model module)
    with bodies in a production file. Loom models may be exempted per
    exact path via `[[inline_tests]]` in `exceptions.toml`
    (`path`/`reason`/`owner`).
  - `LOCAL_USE` — a function- or block-local `use`. Documented
    exceptions (do not flag): `use super::*;` inside a `cfg(test)` or
    test-named module; the enclosing item carries a `cfg` attribute
    (macro-generated / cfg-gated body); a line comment on its own line
    directly above the `use` containing the word "collision". These are
    deliberately simple heuristics, kept conservative.
  - `OWNED_IMPL` — an inherent `impl` of a configured type outside the
    type's defining module or its descendants. Configured in
    `.rust-guard/owned_impl.toml` as
    `[[owned_impl]] type = "RepoInstance" module = "crate::repo::instance::repo_instance"`
    (the shipped entry is the template). Logical module paths are
    derived from the physical path under `src/`.
  - `VIS_WIDENING` / `ALLOW_ADDED` (only with `--base`, or with
    `--changed-since`, whose merge base with HEAD is used) — an item's
    visibility grew (private < `pub(super)`/`pub(in)` < `pub(crate)` <
    `pub`), or a new `#[allow]`/`#![allow]` attribute not present in the
    base file. Either is suppressed when the `--delta` TOML file lists
    the path (`[[allow_widen]]` / `[[allow]]` with `path`, optional
    `item`, `reason`, `owner`; matching is on `path` only).
  `--warn-primary-export` prints a WARNING line (never a finding) for
  each non-`mod.rs` file declaring more than one public item.
- `rust-guard conservation --base <rev> --head <rev> [--packages p,q]
  [--delta <path>]` — source inventory diff between two immutable git
  revisions (chunk B). The inventory is built from git objects only
  (`git ls-tree -r -z --name-only`, `git show <rev>:<path>`; never the
  working tree) for every `.rs` item reachable from the target roots of
  the packages discovered via `cargo metadata --no-deps` members plus
  workspace-excluded packages (`vendor/*` skipped), optionally narrowed
  with `--packages`. Per item it records: package, kind (`fn`, `struct`,
  `enum`, `union`, `trait`, `impl-member`, `static`, `const`, `type`,
  `mod`, `macro`), name, full logical module path, enclosing type/trait,
  normalised `impl` head, normalised signature tokens, cfg predicate,
  test attributes (`#[test]`, `#[tokio::test]`, `proptest!`, loom model
  modules, `#[ignore]` + reason) and a token-normalised BODY
  FINGERPRINT (whitespace/comments/visibility stripped, identifiers,
  literals and punctuation hashed). Split `impl` blocks merge into one
  logical member per (impl head, member) before comparison.
  Pairing: exact identity first; leftovers pair by (kind, name,
  enclosing) + signature as moves — moves/renames/extractions are only
  legal when declared in the delta; names-only counting is impossible
  (an identity pair with a different fingerprint is `BODY_CHANGED`, a
  same-name leftover with a different body never pairs silently).
  Finding codes:
  - `BLOCKED_COMPARISON` — `--base`/`--head` does not name a commit;
    never treated as an empty baseline.
  - `BASE_MISMATCH` — the delta's `base` does not match the resolved
    base SHA.
  - `MALFORMED_DELTA` — the delta file does not parse or an entry is
    missing `path`/`item`/`reason`/`owner`.
  - `ITEM_LOST` — an item vanished with no pairing and no `[[removed]]`
    /`[[extracted]]` declaration.
  - `ITEM_ADDED` — a genuinely new item without `[[added]]`.
  - `ITEM_MOVED` — an item kept its logical path but changed files
    without a `[[moved]]` declaration.
  - `BODY_CHANGED` — fingerprint drift on a paired item without
    `[[changed]]`.
  - `TEST_LOST` — a test-attributed item (incl. `proptest!` and loom
    models) is gone. NEVER excusable by any delta entry.
  - `TEST_IGNORED` / `TEST_UNIGNORED` — `#[ignore]` status changed.
  - `TEST_ATTR_CHANGED` — test kind (`#[test]` → `#[tokio::test]`),
    cfg predicate or ignore-reason changed.
  - `UNPAIRABLE_DUPLICATE` — two or more head items match one base item
    by kind/name/enclosing with no way to choose.
  - `UNUSED_ALLOWANCE` — a declared delta entry matched nothing.

  ### Delta file (`--delta`, optional; without it every difference is a
  finding)

  ```toml
  base = "abc1234"              # expected base SHA (abbreviated ok)

  [[removed]]                   # item intentionally deleted
  path   = "crates/x/src/old.rs"   # base file
  item   = "shamir_x::old::Plan"   # logical path, `::name` tail or bare name
  reason = "superseded by Planner"
  owner  = "@alice"

  [[added]]                     # genuinely new item
  path   = "crates/x/src/planner.rs"  # head file
  item   = "Plan"
  reason = "new planning stage"
  owner  = "@alice"

  [[changed]]                   # accepted body-fingerprint change
  path   = "crates/x/src/planner.rs"
  item   = "Planner::plan"
  reason = "new spill heuristic"
  owner  = "@bob"

  [[moved]]                     # relocation (file or module move)
  path    = "crates/x/src/old.rs"        # base side
  item    = "Plan"
  to_path = "crates/x/src/planner.rs"    # head side, optional but checked
  to_item = "Plan"                       # head side, optional but checked
  reason  = "moved to planner module"
  owner   = "@alice"

  [[renamed]]                   # everything equal, new name
  path    = "crates/x/src/old.rs"
  item    = "Plan"
  to_item = "Draft"
  reason  = "clearer name"
  owner   = "@alice"

  [[extracted]]                 # one base fn split into several head fns
  path    = "crates/x/src/old.rs"
  item    = "Plan::run"
  to_item = "Plan::prepare"      # first head identity (informational)
  reason  = "split prepare/execute phases"
  owner   = "@carol"
  ```

  All four base fields (`path`, `item`, `reason`, `owner`) are
  REQUIRED on every entry; a missing or empty one is a
  `MALFORMED_DELTA` finding. Each consumed entry disappears; whatever
  remains when the comparison ends is `UNUSED_ALLOWANCE`. Tests are
  outside the allowance system: removing or newly ignoring a test can
  never be declared. Delta files are referenced from the campaign entries
in `.rust-guard/stages.toml` (written by the CI wiring chunk).

## Exceptions: `.rust-guard/exceptions.toml`

The default exceptions file (override with `--exceptions`) holds two
record kinds, documented inline in the file itself:

- `[[exception]]` — one exact tracked path (`path`, no wildcards) plus
  `package`, `reason`, `owner`; used for files that legitimately cannot
  be reached from a target root or are known duplicates.
- `[[producer]]` — `name` (macro name, or `include` for OUT_DIR
  includes), `packages`, `owner`, `reason`; used for module-producing
  macros and generated includes the parser cannot resolve.

Every exception must still be needed: a path that is no longer tracked,
or a file now reachable without it, is a `STALE_EXCEPTION` finding.

## Scope and layout

Scan scope is `crates/*/` and `tools/*/` (vendored `vendor/*` excluded).
Any tracked `.rs` file under a path segment named `fixtures` is skipped
by the `modules` scope and the `conservation` inventory — the self-test
fixture trees are mounted by the test harness, never the build (same
treatment as `check_layout.py`'s `EXCLUDED_DIR_NAMES`).
The crate follows the repo layout rules: ≤ 7 entries per folder, ≤ 1000
lines per file, `mod.rs` re-exports only, imports at the top.

## Self-tests (CI-only)

`cargo test --manifest-path tools/rust-guard/Cargo.toml` (GitHub CI
only) runs `tests/modules_tests.rs`, `tests/structure_tests.rs` and
`tests/conservation_tests.rs` through the shared `tests/harness.rs`:

each test materialises a fixture tree from `tests/fixtures/` into a
unique throwaway git repository under the OS temp dir (created with
`git init` and committed with a fixed identity), then invokes the built
binary with `--repo <temp-dir>` and asserts the exact finding code in
its output (exit != 0) or a clean zero-finding exit. Conservation
fixtures carry a `base/` and a `head/` tree committed as two commits;
the test compares them with `--base <sha1> --head HEAD`, optionally
with a generated delta file. `modules`/`conservation` fixtures get a
minimal `[workspace]` + `crates/fix` scaffold so `cargo metadata
--no-deps` succeeds inside the temp repo.

Fixture → finding map:

| Fixture | Finding asserted |
|---|---|
| `modules_ok` | none (exit 0): `#[path]`, `cfg_attr` alternatives (one excepted), non-mod-rs parent, inline `mod` directory-context extension (`mod nested { mod leaf; }` → `nested/leaf.rs`), `include!`, raw-string/comment `mod x;` negative |
| `modules_bad/dropped_mod` | `UNACCOUNTED_FILE` |
| `modules_bad/unsat_cfg` | `UNSATISFIABLE_CFG` |
| `modules_bad/missing_path` | `MISSING_MOD_FILE` |
| `modules_bad/ambiguous` | `AMBIGUOUS_MOD` |
| `modules_bad/duplicate` | `DUPLICATE_FILE` |
| `modules_bad/stale_exception` | `STALE_EXCEPTION` |
| `modules_bad/macro_mods` | `UNRESOLVED_MACRO` (module-level `macro_rules!` mentioning `mod`, no `[[producer]]`) |
| `structure/modrs_logic` | `MODRS_LOGIC` |
| `structure/tests_manifest` | `TESTS_MANIFEST` |
| `structure/inline_test_mod` | `INLINE_TEST_MOD` |
| `structure/local_use` | `LOCAL_USE` |
| `structure/owned_impl` | `OWNED_IMPL` (fixture-local `.rust-guard/owned_impl.toml`) |
| `conservation/test_lost` | `TEST_LOST` |
| `conservation/test_ignored` | `TEST_IGNORED` |
| `conservation/renamed_ok` | none (exit 0) with a `[[renamed]]` delta |
| `conservation/body_changed` | `BODY_CHANGED` |
| `conservation/unused_allowance` | `UNUSED_ALLOWANCE` |
| `conservation/base_mismatch` | `BASE_MISMATCH` |
| `conservation/unpairable_duplicate` | `UNPAIRABLE_DUPLICATE` (one base item, two same-key head candidates) |
| (no fixture — bad rev argument) | `BLOCKED_COMPARISON` |
