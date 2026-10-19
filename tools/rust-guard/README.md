# rust-guard

Static source-integrity gates for this repository (CI-03a). A small
workspace-excluded binary that reads files from git objects (`git
ls-files -z`, `git show <rev>:<path>`) or the working tree, parses Rust
with `syn` (never regex), and **fails closed**: any finding exits
non-zero and prints

```
path:line: CODE: message
```

sorted, to stderr. Lines of the form `rust-guard: error: ...` are INFRA
errors (git/cargo/manifest-loading failures) and exit 1; they never carry
a finding code. Finding codes are exactly the ones listed per subcommand
below (`...:line: CODE: message`), plus `BAD_MANIFEST` under `modules`.
Delta-file problems (`MALFORMED_DELTA`, `BASE_MISMATCH`) are reported as
FINDINGS of the `conservation` gate, not infra errors, and never collide
with `modules` codes. Data structures are `std::collections::BTreeMap`
and `BTreeSet` only — no hash-map configuration is shipped, and the
root workspace's map-type bans are satisfied by construction (the
crate's `clippy.toml` was removed). Builds and self-tests run in GitHub
CI only — do not run them locally (see CLAUDE.md "Compile / build /
test policy").

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
  --no-deps` workspace members — matched by `packages[].id` against
  `workspace_members` — plus workspace-excluded packages and `tools/*`;
  `vendor/*` is out of scope). Manifest paths are honoured and joined to
  the package directory (`[lib].path`, `[[bin]]/[[test]]/[[bench]]/
  [[example]].path`, `[package].build`); a target with non-empty
  `required-features` is a root only when every named feature is declared
  in `[features]`, otherwise it is a `BAD_MANIFEST` finding (fail-closed
  choice: cargo would never build it, so nothing guarantees it exists).
  Auto-discovery covers files directly in the conventional directory plus
  the immediate-child form `<dir>/<name>/main.rs` ONLY — any deeper
  nested `main.rs` (helper data such as `tests/case/crash_matrix.rs`
  mounted by `tests/crash_recovery.rs`) is not a root.
  Per-revision discovery: the file listing that drives auto-discovery
  comes from `git ls-tree -r --name-only <rev>` when `--rev` is pinned
  (falling back to `git ls-files` only without a revision) — a target
  deleted at the pinned revision is never resurrected from the current
  index.
  A failing `cargo metadata` (one broken member manifest fails the whole
  command) is a `CARGO_METADATA_FAILED` finding and the scan CONTINUES
  on hand-parsed manifests (root `[workspace]` members, glob-expanded
  against the pinned listing, plus `tools/*`), so hand-scanned manifests
  still yield their `BAD_MANIFEST` findings instead of the run dying as
  an infra error before them.
  An unreadable or unparseable manifest is a `BAD_MANIFEST` finding, not
  a silent skip.
  Resolution of every `mod x;` / inline `mod x { }` follows Rust's
  contextual rules (`x.rs` vs `x/mod.rs` vs the non-mod-rs parent
  directory, literal `#[path]`, `#[cfg_attr(pred, path = "...")]`);
  an inline module needs no file of its own and appends its name to the
  resolution directory, so `mod a { mod b; }` resolves `b` at `a/b.rs`
  (and inline-in-inline composes); `#[path]` inside an inline module is
  relative to that inline module's directory context. Every resolved
  path is LEXICALLY NORMALISED (`.` and `..` collapsed against the repo
  root); a path that escapes the repository root is a
  `PATH_ESCAPES_REPO` finding and is never followed — nor does its
  un-normalised spelling leak into the visited bookkeeping.
  ALL
  `#[cfg_attr]` path alternatives are kept: mutually exclusive branches
  are not duplicate modules, and a file reached only via two provably
  exclusive predicates (`not(X)` vs `X`) is not `DUPLICATE_FILE`. A
  lone `#[cfg_attr(pred, path = "P")]` mod models BOTH branches:
  `pred → P` and `not pred →` the default `x.rs`/`x/mod.rs` candidates —
  each branch is resolved and accounted when it exists; a branch
  resolving to nothing is not a finding on its own (only when NEITHER
  branch resolves).
  cfg predicates COMPOSE down the tree: the inherited predicate, the
  module's own `#[cfg]` and the alternative's `cfg_attr` predicate are
  AND-ed (`all(...)`) into the child's reaching predicate, and inline
  traversal AND-s the parent cfg with the inline module's own cfg.
  Mutual-exclusion duplicate tests run on the COMPOSED predicates, so
  `all(unix, ...)` vs `all(not(unix), ...)` compares correctly.
  cfg predicates are evaluated on the parsed expression for the statically
  provable cases (`any()`, `not(all())`, unsatisfiable branches inside
  `all(...)`/`any(...)`); anything else is treated as satisfiable.
  `DUPLICATE_FILE` fires only when one file is mounted under conflicting
  logical paths WITHIN ONE target's module tree (without mutually
  exclusive cfg) — reaching a file from SEVERAL targets (a shared bench
  helper auto-discovered as a bench root AND `mod`-included by another
  bench, `tests/harness.rs` auto-discovered AND `mod harness;` from each
  test binary) is normal cargo behaviour and never a duplicate;
  auto-discovered `tests/*.rs`/`benches/*.rs` that are also
  mod-included are accounted by being reachable.
  Findings:
  `UNACCOUNTED_FILE` (a tracked `.rs` under `crates/*/` or `tools/*/`
  reachable from no root — the lost-test case), `MISSING_MOD_FILE`,
  `AMBIGUOUS_MOD`, `UNSATISFIABLE_CFG`, `DUPLICATE_FILE`,
  `STALE_EXCEPTION`, `UNRESOLVED_INCLUDE`, `UNRESOLVED_MACRO`,
  `PARSE_FAILED`, `BAD_MANIFEST`, `CARGO_METADATA_FAILED`,
  `PATH_ESCAPES_REPO`.
- `rust-guard structure [--paths <glob>...]... [--changed-since <rev>]
  [--base <rev>] [--delta <path>] [--exceptions <path>]
  [--warn-primary-export]` — AST rules over the selected files (at least
  one of `--paths` / `--changed-since` required; globs match
  git-tracked paths). Finding codes:
  - `UNREADABLE_FILE` — a selected file exists but could not be read
    (I/O error); reported instead of silently skipped. Missing files are
    not reported here (the `modules` gate owns them).
  - `MODRS_LOGIC` — a `mod.rs` holds anything beyond attributes/doc
    comments, `mod` declarations and `use`/`pub use` re-exports (no fns,
    types, impls, macros, constants). The check recurses into INLINE
    modules inside the mod.rs, so logic hidden in `mod x { fn f() {} }`
    is found too.
  - `TESTS_MANIFEST` — a `tests/mod.rs` is not a pure `mod` manifest
    (not even `use` is allowed).
  - `INLINE_TEST_MOD` — an inline test-only module (`#[cfg(test)] mod
    tests { .. }`, `mod *_tests`, or a `loom`/`*_loom` model module)
    with bodies in a production file. Loom models may be exempted per
    exact path via `[[inline_tests]]` in `exceptions.toml`
    (`path`/`reason`/`owner`).
  - `LOCAL_USE` — a function- or block-local `use`. The check is a
    full-file AST walk, so a local `use` is found anywhere: inside impl
    method bodies, closures, loops or conditionals, at any depth.
    Documented exceptions (do not flag): `use super::*;` inside a `cfg(test)` or
    test-named module; the enclosing item carries a `cfg` attribute
    (macro-generated / cfg-gated body); a line comment on its own line
    directly above the `use` containing the word "collision". These are
    deliberately simple heuristics, kept conservative.
  - `OWNED_IMPL` — an inherent `impl` of a configured type outside the
    type's defining module or its descendants. Configured in
    `.rust-guard/owned_impl.toml` as
    `[[owned_impl]] type = "RepoInstance" module = "crate::repo::instance::repo_instance"`
    (the shipped entry is the template). Logical module paths are
    derived from the physical path under `src/`. The configured type
    matches the LAST segment of a qualified self type
    (`impl crate::repo::instance::RepoInstance` is governed by
    `type = "RepoInstance"`); `<T as Trait>::Assoc` self types are out
    of scope on purpose.
  - `VIS_WIDENING` / `ALLOW_ADDED` (only with `--base`, or with
    `--changed-since`, whose merge base with HEAD is used) — an item's
    visibility grew (private < `pub(super)`/`pub(in)` < `pub(crate)` <
    `pub`), including struct fields (keyed `Struct::field`) and items
    inside inline modules (keyed by their module path); a brand-new file
    introducing `pub(crate)`+ items is also a widening. `ALLOW_ADDED`
    fires on a new `#[allow]`/`#![allow]` attribute not present in the
    base file. Either is suppressed when the `--delta` TOML file lists
    the path (`[[allow_widen]]` / `[[allow]]` with `path`, optional
    `item`, `reason`, `owner`; matching is on `path` only).
  `--warn-primary-export` prints a WARNING line (never a finding) for
  each non-`mod.rs` file declaring more than one public item.
- `rust-guard conservation --base <rev> --head <rev> [--packages p,q]
  [--delta <path>]` — source inventory diff between two immutable git
  revisions (chunk B). The inventory is built from git objects only
  (`git ls-tree -r -z --name-only`, `git show <rev>:<path>`; never the
  working tree, and no cargo is invoked) for every `.rs` item reachable
  from the target roots of the packages discovered by parsing the
  revision's OWN ROOT `Cargo.toml` (`[workspace]` members/exclude, with
  `*`-globs expanded segment-wise against the pinned tree listing) plus
  every member manifest ON THE SAME PINNED VIEW (`vendor/*` skipped),
  optionally narrowed with `--packages`. Discovery is per-revision —
  BOTH the manifests AND the auto-discovered target listing
  (`tests/*.rs`, `benches/*.rs`, ...) come from the pinned revision, so
  a target that exists only at base (a deleted package or a deleted
  test file) still yields its items, which then surface as
  `ITEM_LOST`/`TEST_LOST` losses instead of silence.
  The walk follows `mod x;` declarations — including ones INSIDE inline
  `mod x { }` blocks, resolved under the extended directory context —
  and literal `include!("file.rs")` edges; cfg predicates inherit down
  the module tree (parent + child compose with `&&`). `#[path]` (and
  `#[cfg_attr]`) attributes are read with the SAME shared extraction as
  the `modules` gate (`modules_cmd::resolution::path_alternatives`) —
  the inventory's former local `parse_args::<LitStr>()` could not read a
  name-value attribute, so path-mounted modules silently fell back to
  default resolution. An UNRESOLVED child module (missing, ambiguous or
  generated) is an `INVENTORY_ERROR`, never a silent skip — its items
  would silently "vanish" between revisions otherwise. Every resolved
  path is lexically normalised; a path escaping the repo root is an
  `INVENTORY_ERROR`. Per item it
  records: package, kind (`fn`, `struct`, `enum`, `union`, `trait`,
  `impl-member`, `static`, `const`, `type`, `mod`, `macro`), name, full
  logical module path, enclosing type/trait, normalised `impl` head,
  normalised signature tokens, cfg predicate, test attributes
  (`#[test]`, `#[tokio::test]`, `proptest!`, loom model modules,
  `#[ignore]` + reason) and a token-normalised BODY FINGERPRINT
  (whitespace/comments/visibility stripped, identifiers, literals and
  punctuation hashed). Associated-type VALUES (impl and trait members)
  fingerprint their type expression, so a changed `type X = ...` is a
  finding even with untouched method bodies. Split `impl` blocks merge
  into one logical member per (impl head, member) before comparison.
  Pairing: exact identity first; leftovers pair by (kind, name,
  enclosing) + signature as moves — moves/renames/extractions are only
  legal when declared in the delta; renames compare signatures with the
  item's OWN name normalised out, so `helper` → `helper2` can pair;
  names-only counting is impossible (an identity pair with a different
  fingerprint is `BODY_CHANGED`, a different signature is
  `SIGNATURE_CHANGED`, and a same-name leftover with a different body
  never pairs silently).
  Finding codes:
  - `BLOCKED_COMPARISON` — `--base`/`--head` does not name a commit;
    never treated as an empty baseline.
  - `BASE_MISMATCH` — the delta's `base` does not match the resolved
    base SHA.
  - `INVENTORY_ERROR` — a source or manifest could not be read or
    parsed at one of the two revisions, a child `mod` did not resolve,
    or a resolved path escapes the repository root (fail closed: the
    items it would contribute are UNKNOWN, never silently skipped).
  - `MALFORMED_DELTA` — the delta file does not parse, lacks the
    required `base` SHA, or an entry is missing/empty
    `path`/`item`/`reason`/`owner`.
  - `ITEM_LOST` — an item vanished with no pairing and no `[[removed]]`
    /`[[extracted]]` declaration.
  - `ITEM_ADDED` — a genuinely new item without `[[added]]`.
  - `ITEM_MOVED` — an item kept its logical path but changed files
    without a `[[moved]]` declaration.
  - `BODY_CHANGED` — fingerprint drift on a paired item without
    `[[changed]]`.
  - `SIGNATURE_CHANGED` — the normalised signature of a paired item
    changed (parameter/return type, bounds) without `[[changed]]`; a
    signature change with an identical body IS a finding, never silence.
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
  base = "abc1234"              # REQUIRED: expected base SHA (abbreviated
                                # ok); a delta without it is MALFORMED_DELTA

  [[removed]]                   # item intentionally deleted
  path   = "crates/x/src/old.rs"   # base file
  item   = "shamir_x::old::Plan"   # full logical path, EXACT match only
                                   # (no `::name` tails, no bare names:
                                   # loose matching pairs the wrong item)
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

  All four entry fields (`path`, `item`, `reason`, `owner`) are
  REQUIRED and non-empty on every entry; a missing `base`, or a missing
  or empty entry field, is a `MALFORMED_DELTA` finding. `item` fields
  match full logical paths exactly. A single `[[changed]]` entry excuses
  both body-fingerprint and signature drift on one item (consumed once).
  Each consumed entry disappears; whatever remains when the comparison
  ends is `UNUSED_ALLOWANCE`. Tests are outside the allowance system:
  removing or newly ignoring a test can never be declared. Delta files
  are referenced from the campaign entries
in `.rust-guard/stages.toml` (written by the CI wiring chunk).

## Exceptions: `.rust-guard/exceptions.toml`

The default exceptions file (override with `--exceptions`) holds two
record kinds, documented inline in the file itself:

- `[[exception]]` — one exact tracked path (`path`, no wildcards) plus
  `package`, `reason`, `owner`; used for files that legitimately cannot
  be reached from a target root or are known duplicates.
- `[[producer]]` — `name` (macro name — for `macro_rules!` definitions
  the macro is `macro_rules` itself — or `include` for includes the
  parser cannot resolve), `packages` (REQUIRED, non-empty; the producer
  only applies to modules/includes reached from those packages —
  membership is enforced during lookup), `owner`, `reason`; used for
  module-producing macros and generated includes. A NON-literal
  `include!` (`concat!(env!("OUT_DIR"), ...)`) must be covered by an
  `include` `[[producer]]` for the declaring package, or it is an
  `UNRESOLVED_INCLUDE` finding.

Every record of every kind MUST carry a non-empty `reason` and `owner`
(an unattributed exception is an unaccountable one), and every
`[[producer]]` MUST name a non-empty `packages` list; violations make
the file fail to load.

Every exception must still be needed: a path that is no longer tracked,
or a file now reachable without it, is a `STALE_EXCEPTION` finding.

## Scope and layout

Scan scope is `crates/*/` and `tools/*/` (vendored `vendor/*` excluded).
`.rust-guard/scan.toml` (repo root) is the EXPLICIT scan configuration:

```toml
exclude_paths = ["tools/rust-guard/tests/fixtures"]  # exact repo-relative prefixes
```

Files under any `exclude_paths` prefix are skipped by the target-root
discovery, the module walk and the conservation inventory (the tool's
own self-test fixture trees are excluded this way on the real repo —
an explicit repo setting, never a hard-coded path). Self-tests still
scan fixture trees because they are materialised into throwaway temp
repos that carry no `scan.toml`. Any tracked `.rs` file under a path
segment named `fixtures` is additionally skipped by the `modules` scope
and the `conservation` inventory — the self-test
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
with a generated delta file. `modules` fixtures get a minimal
`[workspace]` + `crates/fix` scaffold (including `crates/fix/src/lib.rs`,
so `cargo metadata --no-deps` succeeds inside the temp repo even for
fixtures whose own manifests are deliberately broken; a fixture that
ships its own root, package manifest or sources keeps it). Conservation
needs no cargo at all — its
discovery parses manifests from git objects at each revision.
Conservation fixtures' head overlay DELETES every base-fixture file the
head fixture does not contain (a deleted test target must really be
absent at head).
Head commits use `--allow-empty`, so fixture pairs with identical
trees still yield a two-commit history.

Fixture → finding map:

| Fixture | Finding asserted |
|---|---|
| `modules_ok` | none (exit 0): `#[path]`, genuinely exclusive `cfg_attr` alternatives (`unix`/`not(unix)`, both reachable, no exception), BOTH branches of a lone `#[cfg_attr]` (attr target + default fallback), an inherited `#[cfg(unix)]` gating an inline child, a file shared across two TARGETS (`benches/common.rs` root AND mod-included — never a duplicate), `src/bin/x.rs` bin target, non-mod-rs parent, inline `mod` directory-context extension (`mod nested { mod leaf; }` → `nested/leaf.rs`, no wrapper file), `include!`, raw-string/comment `mod x;` negative |
| `modules_ok/producer_declared` | none (exit 0): documented `[[producer]]` spelling accepted, package-scoped, plus `[[exception]]` for the generated file |
| `modules_ok/generated_include_producer` | none (exit 0): `include!(concat!(env!("OUT_DIR"), ...))` with a scoped `include` `[[producer]]` |
| `modules_bad/dropped_mod` | `UNACCOUNTED_FILE` |
| `modules_bad/unaccounted_extra` | `UNACCOUNTED_FILE` (clean when a `scan.toml` excludes `extra.rs`) |
| `modules_bad/unsat_cfg` | `UNSATISFIABLE_CFG` |
| `modules_bad/missing_path` | `MISSING_MOD_FILE` |
| `modules_bad/ambiguous` | `AMBIGUOUS_MOD` |
| `modules_bad/duplicate` | `DUPLICATE_FILE` (mounted twice in ONE target — cross-target sharing is clean, see `modules_ok`) |
| `modules_bad/path_escapes` | `PATH_ESCAPES_REPO` (`#[path = "../../../outside.rs"]`) |
| `modules_bad/generated_include` | `UNRESOLVED_INCLUDE` (non-literal include, no producer) |
| `modules_bad/stale_exception` | `STALE_EXCEPTION` |
| `modules_bad/macro_mods` | `UNRESOLVED_MACRO` (module-level `macro_rules!` mentioning `mod` inside nested braces, no `[[producer]]`) |
| `modules_bad/producer_wrong_package` | `UNRESOLVED_MACRO` (producer scoped to another package must not apply) |
| `modules_bad/bad_manifest` | `BAD_MANIFEST` (unparseable hand-scanned manifest fails closed) |
| (no fixture — broken root member) | `CARGO_METADATA_FAILED` (finding + scan continues on hand-parsed manifests) |
| `structure/modrs_logic` | `MODRS_LOGIC` |
| `structure/tests_manifest` | `TESTS_MANIFEST` |
| `structure/inline_test_mod` | `INLINE_TEST_MOD` |
| `structure/local_use` | `LOCAL_USE` |
| `structure/local_use_in_impl` | `LOCAL_USE` (inside an impl method body) |
| `structure/owned_impl` | `OWNED_IMPL` (fixture-local `.rust-guard/owned_impl.toml`) |
| `structure/owned_impl_qualified` | `OWNED_IMPL` (qualified self type; the LAST segment is matched) |
| `structure/modrs_inline` | `MODRS_LOGIC` (logic in an inline module of a mod.rs) |
| `structure/widen_super_crate` | `VIS_WIDENING` (`pub(super)` → `pub(crate)` distinct tiers) |
| `structure/widen_field` | `VIS_WIDENING` (struct field, keyed `S::a`) |
| `structure/new_file_widen` | `VIS_WIDENING` (brand-new file with a `pub(crate)` item); clean with `[[allow_widen]]` |
| `structure/allow_added` | `ALLOW_ADDED` (file gains `#[allow(dead_code)]`); clean with `[[allow]]` |
| `conservation/test_lost` | `TEST_LOST` |
| `conservation/test_ignored` | `TEST_IGNORED` |
| `conservation/test_cfg_changed` | `TEST_ATTR_CHANGED` (a `#[test]` fn gains a `#[cfg(...)]` predicate) |
| `conservation/renamed_ok` | none (exit 0) with a `[[renamed]]` delta; without one ITEM_LOST + ITEM_ADDED |
| `conservation/renamed_test` | none (exit 0) with a `[[renamed]]` delta on a `#[test]` fn; without one TEST_LOST only (never ITEM_ADDED) |
| `conservation/body_changed` | `BODY_CHANGED` |
| `conservation/signature_changed` | `SIGNATURE_CHANGED` (param/return type change, identical body) |
| `conservation/gone_pkg` | `ITEM_LOST` for every item of a base-only package the head deletes (per-revision manifest discovery) |
| `conservation/target_deleted` | `TEST_LOST` for a test TARGET (`tests/x.rs`) deleted at head (per-revision auto-discovery + deletion-aware overlay) |
| `conservation/path_test_deleted` | `TEST_LOST` for a `#[path]`-mounted test module deleted at head (shared attribute-path extraction) |
| `conservation/broken_mod` | `INVENTORY_ERROR` (`mod ghost;` resolves at neither revision — fail closed) |
| `conservation/unused_allowance` | `UNUSED_ALLOWANCE` (identical trees — exercises the `--allow-empty` head commit) |
| `conservation/base_mismatch` | `BASE_MISMATCH` (head carries a real comment-only diff) |
| `conservation/unpairable_duplicate` | `UNPAIRABLE_DUPLICATE` (one base item, two same-key head candidates) |
| (no fixture — bad rev argument) | `BLOCKED_COMPARISON` |
| (no fixture — delta without `base`) | `MALFORMED_DELTA` |
