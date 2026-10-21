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
  directory). It rejects any `#[cfg_attr(..., path = "...")]` (including
  nested/multiple path attributes) as `UNSUPPORTED_CFG_ATTR_PATH` before
  traversal. Such conditional paths are not modelled: replace them with an
  unconditional `#[path = "..."]` or a plain `mod`. This gate does not
  implement exception-based bypasses for unsupported syntax; a config entry
  will not suppress this finding, and any exception mechanism needs separate
  review.
  An inline module needs no file of its own and appends its name to the
  resolution directory, so `mod a { mod b; }` resolves `b` at `a/b.rs`
  (and inline-in-inline composes); `#[path]` inside an inline module is
  relative to that inline module's directory context.
  cfg predicates COMPOSE down the tree: inherited and module-own plain
  `#[cfg]` predicates are AND-ed (`all(...)`) into the child's reaching
  predicate, and inline traversal AND-s the parent cfg with the inline
  module's own cfg. Mutual-exclusion duplicate tests run on the COMPOSED
  predicates, so `all(unix, ...)` vs `all(not(unix), ...)` compares correctly.
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
  `PATH_ESCAPES_REPO`, `UNSUPPORTED_CFG_ATTR_PATH` (conditional
  `cfg_attr(path)` mounts are unsupported; not resolved).
- `rust-guard structure [--paths <glob>...]... [--changed-since <rev>]
  [--scope <repo-relative-dir-prefix>...] [--base <rev>] [--delta <path>]
  [--exceptions <path>] [--warn-primary-export]` — AST rules over selected
  Rust files (at least one selector is required; globs match git-tracked
  paths). `--paths` and `--changed-since` selections are unioned; repeatable
  `--scope` directory prefixes intersect that union on path-component
  boundaries. Omitting scopes preserves existing selection behavior.
  Finding codes:
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
  - `OWNED_IMPL` — an `impl` (inherent OR trait) of a configured type
    outside the type's defining module or its descendants. Configured in
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
    an entry with `path`, `item`, `reason`, `owner` (`[[allow_widen]]` /
    `[[allow]]`). Matching is EXACT on `(path, item)` and consumed on
    first use — `item` is REQUIRED (an entry without one fails to
    parse), the old path-only/empty-item wildcard is gone. Leftover
    entries are `UNUSED_ALLOWANCE` findings — including runs with no
    base revision or no selected files, where no finding could ever
    have consumed them.
  **Split-off files.** A file that is new at head and does not pair as a
  whole-file move (the result of splitting a deleted base file) is compared
  with the VACATED base — the base `.rs` files that no longer exist at
  head, plus everything that LEFT a selected base file which still exists
  at head (a facade that shrank: items whose key is gone from its head
  version, `#[allow]` texts with fewer occurrences, per-file findings that
  no longer occur) — instead of an empty base: a `pub(crate)`+ item is a
  `VIS_WIDENING` only if no vacated base file had the same item (kind and
  qualified name, impl members keyed by self type) at a rank at least as
  wide; an `#[allow]` is `ALLOW_ADDED` only if its normalised text occurs
  in no vacated base file; and a `MODRS_LOGIC` / `TESTS_MANIFEST` /
  `INLINE_TEST_MOD` / `LOCAL_USE` finding is excused only while an
  identical finding (same code and message) exists in the vacated base
  files — each base finding excuses one head finding, so the debt can move
  but never grow. Parse failures and the ownership rule are never excused,
  an inline test module is excused by its module name, and an item that
  stays in its source file (a copy) is never indexed.
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
  the module tree (parent + child compose with `&&`). `#[path]` is followed
  as an explicit source edge. File-backed `mod` and literal `include!` edges
  targeting an ancestor file emit `MOUNT_CYCLE` in the `modules` gate (the
  inventory visits each physical file once in its per-revision visited set
  and reports no cycle of its own). `cfg_attr(path)` (including
  nested and multiple path-bearing attributes) is rejected as
  `UNSUPPORTED_CFG_ATTR_PATH` in inventory, not conditionally resolved. There
  is no generic bypass; supporting a specific case requires a deliberate
  extension and explicit reviewed exception in `.rust-guard/`.
  Replace it with unconditional `#[path]` or plain `mod`; this gate has no
  exception bypass for unsupported syntax, and any future mechanism needs
  separate review. An UNRESOLVED child module (missing,
  ambiguous or generated) is an `INVENTORY_ERROR`, never a silent skip — its items
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
  Pairing: exact identity first (a strict form with the physical file and
  the full content runs before it, so unchanged items that share one
  logical path — cfg twins, the `main`/`const` items of every bench or
  example target — keep their own counterpart); leftovers pair by (kind,
  name, enclosing) + signature as moves (content-identical twins with the
  same number of copies on both sides pair one-to-one first, so a moved
  helper that exists in several files is not ambiguous) —
  moves/renames/extractions are only
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

### Suggesting moves (`--suggest-delta <path>`, optional diagnostic)

Both `conservation` and `discovery` accept `--suggest-delta <path>`. It
never changes the findings or the exit code: the gate fails exactly as
it would without the flag. Pairs that are blocked ONLY by a missing
`[[moved]]` declaration (otherwise clean moves, tests included) are
written to `<path>` as a delta file ready to be fed back via `--delta`:

```toml
base = "abc1234..."            # the resolved full base SHA

[[moved]]
path    = "crates/x/src/old.rs"
item    = "shamir_x::old::Plan"
to_path = "crates/x/src/planner.rs"
to_item = "shamir_x::planner::Plan"
reason  = "relocated by the layout sweep"
owner   = "@layout-sweep"
```

Entries are sorted by base path, then base item; the file is written
only when at least one pair exists, and one summary line
(`rust-guard: note: N undeclared move(s) written to <path>`) goes to
stderr. Real losses, genuinely new items, body-changed,
signature-changed and attribute-changed pairs, and ambiguous
(`UNPAIRABLE_DUPLICATE`) pairings are NEVER suggested — they stay
findings. `[[renamed]]`, `[[extracted]]`, `[[removed]]`, `[[added]]`
and `[[changed]]` are never suggested. For `discovery` the `base` value is the resolved
full base SHA passed via the REQUIRED `--base-sha <sha>` argument
(clap enforces that `--suggest-delta` and `--base-sha` are given
together); the loader never checks it against anything (discovery
compares list files, not revisions), but the pin lets a reviewer
trace the run.

## `rust-guard discovery` — compiled test-inventory diff (CI-03b)

`rust-guard discovery --base-list base.json --head-list head.json
[--delta <delta.toml>] [--kind lib|loom|doc]` compares two
`cargo nextest list --message-format json` outputs that must be taken
under identical toolchain, OS, features, profile and target selection.
A test's IDENTITY is the pair (binary id, test name). The parser
accepts ONLY the real nextest shape: ONE JSON document with a
top-level `rust-suites` object whose keys are the binary ids and whose
suites carry a `testcases` object keyed by test name (unknown fields
are ignored). Legacy/JSON-lines shapes, documents without
`rust-suites`, or non-JSON text fail the whole list — no tolerant
fallback parsing exists, by design: a mis-generated list must block
the comparison, never pass vacuously.

Findings (fail closed, `path:line: CODE: message`):

- `BLOCKED_COMPARISON` — a base or head list file is missing,
  unreadable, or is not one JSON document with a `rust-suites` object;
  never an empty baseline.
- `EMPTY_LIST` — a list with zero tests is a failure, not a pass.
- `TEST_LOST` — a base test with no head counterpart and no
  `[[moved]]`/`[[renamed]]` declaration, or whose declaration is
  unusable (missing `to_path`/`to_item`, target identity absent from
  head, or already claimed by another declaration). NEVER excusable by
  any other delta entry kind.
- `TEST_IGNORED` — a base test whose `ignored` is false while its
  DIRECTLY matched head counterpart's `ignored` is true (a newly
  ignored test; compared only on direct matches, never through a
  declared move/rename). The reverse flip (un-ignoring) is allowed.
- `TARGET_LOST` — a binary/target id whose remaining base tests are
  lost and which carries no head tests (a declared move of its last
  test to another binary does not double-report).
- `UNUSED_ALLOWANCE` — a delta entry that matched no pairing.

Duplicates cannot occur through the document parser (JSON object keys
are unique; nextest lists each testcase once), so no duplicate finding
exists.

Delta mapping: the conservation delta parser is reused; for
`[[moved]]`/`[[renamed]]` entries `path` + `item` are read as the BASE
(binary id, test name) and `to_path` + `to_item` as the HEAD identity.
Unlike `conservation`, the head side does NOT default to the base
side: a declaration without explicit `to_path`/`to_item` is a finding
(`TEST_LOST` + `UNUSED_ALLOWANCE`), and each head identity may be
claimed by at most one declaration. Identities present in BOTH lists
match directly, no delta needed; head-only identities are additions
and never findings. The delta's `base` SHA is required by the loader
but cannot be verified here — this subcommand sees two list files, no
revision; the CI job pins the base list to the merge base instead.

`--kind` (default `lib`) is a label echoed into the finding paths and
messages so doctest and loom lists can be diffed in separate runs.

`discovery` also accepts `--suggest-delta <path>` (see the conservation
section above): each unexplained lost test paired with the single
unclaimed head identity of the same name is written there; ambiguous
names (several head candidates) are never suggested. `--base-sha
<sha>` is required with it and is written as the suggestion's `base`
pin; the pairing must be mutually unique (single base identity and
single head candidate per test name) with equal `ignored` state,
otherwise nothing is suggested.

## When the gates run

On pull requests, `modules` scans the whole repository. `structure` and
`conservation` run only for packages in active `.rust-guard/stages.toml`
campaigns touched by the diff. With `campaigns = []`, neither diff gate is
invoked and CI emits a notice. Master pushes skip both diff gates. Manual
`structure` runs may use repeatable `--scope` prefixes, which intersect the
union of `--paths` and `--changed-since`; omitting scopes preserves legacy
selection behavior.

## Exceptions: `.rust-guard/exceptions.toml`

`exceptions.toml` is the repository's reviewed bypass ledger for the
`modules` gate: it declares, with an owner and a reason, the tracked
files and module-producing constructs that legitimately cannot satisfy
the reachability rules. Record kinds, documented inline in the file
itself:

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
| `modules_ok` | none (exit 0): `#[path]`, plain inherited/own cfg propagation, shared-target helpers, `src/bin/x.rs` bin target, non-mod-rs parent, inline `mod` directory-context extension (`mod nested { mod leaf; }` → `nested/leaf.rs`, no wrapper file), `include!`, raw-string/comment `mod x;` negative |
| `modules_ok/producer_declared` | none (exit 0): documented `[[producer]]` spelling accepted, package-scoped, plus `[[exception]]` for the generated file |
| `modules_ok/generated_include_producer` | none (exit 0): `include!(concat!(env!("OUT_DIR"), ...))` with a scoped `include` `[[producer]]` |
| `modules_bad/dropped_mod` | `UNACCOUNTED_FILE` |
| `modules_bad/unaccounted_extra` | `UNACCOUNTED_FILE` (clean when a `scan.toml` excludes `extra.rs`) |
| `modules_bad/unsat_cfg` | `UNSATISFIABLE_CFG` |
| `modules_bad/missing_path` | `MISSING_MOD_FILE` |
| `modules_bad/ambiguous` | `AMBIGUOUS_MOD` |
| `modules_bad/duplicate` | `DUPLICATE_FILE` (mounted twice in ONE target — cross-target sharing is clean, see `modules_ok`) |
| `modules_bad/path_escapes` | `PATH_ESCAPES_REPO` (`#[path = "../../../../outside.rs"]` from `crates/fix/src`: four parents, one more than the three that only reach the repo root) |
| `modules_bad/generated_include` | `UNRESOLVED_INCLUDE` (non-literal include, no producer) |
| `modules_bad/mount_cycle` | `MOUNT_CYCLE` (file-backed module targets an ancestor file) |
| `modules_bad/macro_mods` | `UNRESOLVED_MACRO` (module-level `macro_rules!` mentioning `mod` inside nested braces, no `[[producer]]`) |
| `modules_bad/cfg_attr_neither` | `UNSUPPORTED_CFG_ATTR_PATH` |
| `modules_bad/cfg_attr_neither_two` | `UNSUPPORTED_CFG_ATTR_PATH` (multiple attributes) |
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
| `structure/owned_impl_trait` | `OWNED_IMPL` (trait impl; logical module `crate::other` is outside the owner module) |
| `structure/modrs_inline` | `MODRS_LOGIC` (logic in an inline module of a mod.rs) |
| `structure/widen_super_crate` | `VIS_WIDENING` (`pub(super)` → `pub(crate)` distinct tiers) |
| `structure/widen_field` | `VIS_WIDENING` (struct field, keyed `S::a`) |
| `structure/new_file_widen` | `VIS_WIDENING` (brand-new file with a `pub(crate)` item); clean with `[[allow_widen]]` |
| `structure/allow_added` | `ALLOW_ADDED` (file gains `#[allow(dead_code)]`); clean with `[[allow]]` |
| `structure/moved_plain` | none (exit 0) — a TRUE move (old.rs deleted at head) with byte-identical content pairs and stays clean |
| `structure/copy_of_base_file` | `VIS_WIDENING` — a COPY (base file still present at head) never pairs as a move; the added file is new-file-checked |
| `structure/split_clean` | none (exit 0) — `old.rs` split into two files that carry its `pub(crate)` item, `#[allow]`, inline test module and function-local `use` |
| `structure/split_partial_clean` | clean — items, `#[allow]`, local `use` and test module left a surviving facade file |
| `structure/split_partial_copy` | `VIS_WIDENING` — `a` is copied while it stays in the surviving `old.rs`; the moved `b` is excused |
| `structure/split_widened` | `VIS_WIDENING` — the split-off item is `pub` where the vacated base item was `pub(crate)` |
| `structure/split_new_allow` | `ALLOW_ADDED` — only the new `#[allow(unused)]`, not the one the vacated base file had |
| `structure/split_copy_not_vacated` | `INLINE_TEST_MOD` for the new file — a base file still present at head vacates nothing |
| `conservation/test_lost` | `TEST_LOST` |
| `conservation/test_ignored` | `TEST_IGNORED` |
| `conservation/test_cfg_changed` | `TEST_ATTR_CHANGED` (INHERITED module cfg drift: the inline test module's `#[cfg]` changes v1 → v2) |
| `conservation/assoc_type_changed` | `SIGNATURE_CHANGED` (an impl-type member's VALUE `type A = u8;` → `type A = u16;`) |
| `conservation/renamed_ok` | none (exit 0) with a `[[renamed]]` delta; without one ITEM_LOST + ITEM_ADDED |
| `conservation/renamed_test` | none (exit 0) with a `[[renamed]]` delta on a `#[test]` fn; without one TEST_LOST only (never ITEM_ADDED) |
| `conservation/body_changed` | `BODY_CHANGED` |
| `conservation/signature_changed` | `SIGNATURE_CHANGED` (param/return type change, identical body) |
| `conservation/gone_pkg` | `ITEM_LOST` for every item of a base-only package the head deletes (per-revision manifest discovery) |
| `conservation/target_deleted` | `TEST_LOST` for a test TARGET (`tests/x.rs`) deleted at head (per-revision auto-discovery + deletion-aware overlay) |
| `conservation/path_test_deleted` | `TEST_LOST` for a `#[path]`-mounted test module deleted at head (shared attribute-path extraction) |
| `conservation/fallback_changed` | `UNSUPPORTED_CFG_ATTR_PATH` during inventory |
| `conservation/broken_mod` | `INVENTORY_ERROR` (`mod ghost;` resolves at neither revision — fail closed) |
| `conservation/unused_allowance` | `UNUSED_ALLOWANCE` (identical trees — exercises the `--allow-empty` head commit) |
| `conservation/base_mismatch` | `BASE_MISMATCH` (head carries a real comment-only diff) |
| `conservation/dup_unchanged_targets` | clean — identically named items of two integration-test targets stay paired |
| `conservation/dup_cfg_twins` | clean — cfg variants under one logical path stay paired |
| `conservation/twin_moves` | two identical twins move to two other files: both suggested, the suggestion feeds back to clean |
| `conservation/unpairable_duplicate` | `UNPAIRABLE_DUPLICATE` (one base item, two same-key head candidates) |
| `conservation/uncond_path_default_ignored` | none (exit 0) — an UNCONDITIONAL `#[path]` mod never inventories the phantom default file |
| (no fixture — bad rev argument) | `BLOCKED_COMPARISON` |
| (no fixture — delta without `base`) | `MALFORMED_DELTA` |
