# shamir-engine sweep: adjudicated plan (input, pre-manifest)

Status: reviewer verdict that fixed the stage order, gates and risks. It predates the post-#189 re-inventory; every count and path in it is superseded by the manifest `05b-engine-manifest.md`. Kept verbatim below.

---

**VERDICT 1 — Reject A–F as an executable plan; retain its ownership direction, but install the safety net first, narrow A, split B/D/E, and move tests with their owners.**
**VERDICT 2 — Require parser-based structural checks plus compiled test-discovery conservation; regex inventories and a PR-title trigger are not adequate merge gates.**
**VERDICT 3 — Mechanical conservation cannot establish concurrency or durability equivalence; require an explicit lifetime, cancellation, and persistence-order review.**
**VERDICT 4 — Merge #189 first and re-inventory its resulting tree; the locally available follower-retirement branch changes substantially more than “~45 tx/repo/meta files.”**
**VERDICT 5 — Approve a revised sequence conditionally; reject fabricated exact move/split totals until there is a complete source-to-destination manifest.**

## 1. Findings that change the tree and execution order

### The proposed `instance/` arrangement does not, by itself, solve privacy

**OBSERVED:** `RepoInstance` is defined in `repo/repo_instance.rs`; fields including `configs`, `tables`, `repo_wal`, `wal_dir`, and `marker_write_mutex` are private. Its adapters are explicitly mounted as **children of that defining module** (`crates/shamir-engine/src/repo/repo_instance.rs:1–46`, `:54–78`, `:99–146`).

**INFERRED:** This proposed arrangement is insufficient:

```text
instance/
  mod.rs
  repo_instance.rs       # defines RepoInstance
  catalog/              # sibling of defining module
  transactions/
  journal/
  lifecycle/
```

A sibling beneath `instance` cannot access fields private to `instance::repo_instance`. Physical colocation does not confer access.

**Recommendation:** Preserve the actual privacy boundary without `#[path]`:

```text
repo/
  mod.rs
  instance/
    mod.rs              # declarations and re-exports only
    config.rs
    repo_instance.rs    # defines RepoInstance; declares its child modules
    repo_instance/
      catalog/
      transactions/
      journal/
      lifecycle/
```

Each child area has a manifest and topical implementation files. Private-field adapters are descendants of the defining module. `instance/mod.rs` re-exports the type; `repo/mod.rs` supplies the public facade.

**Rejected alternative:** Widening private fields to `pub(crate)` so the originally drawn siblings compile. That would trade an organizational refactor for a larger access surface.

Also audit every existing `pub(super)`: its meaning changes when a module moves. In particular, `repo` and `write_domain` currently have that visibility (`repo/repo_instance.rs:57`, `:176`). Preserve necessary access through deliberate APIs, not blanket widening.

### “Every `impl RepoInstance`” requires more relocations than the sketch identifies

**OBSERVED:** Implementations also exist in:

- `repo/visibility_observer.rs:12`, `:156`;
- `repo/seed_metadata.rs:6`;
- `repo/seed_install/backing.rs:10`;
- `repo/changelog_store/follower_durability.rs:9`;
- `repo/changelog_store/write_domain_membership.rs:9`;
- `tx/follower_durability/receipt/resume/worker.rs:362`.

The last contains worker installation and submission methods, not just receipt algorithms (`…/worker.rs:367–409`).

**Recommendation:** Extract these implementation blocks into the defining module’s descendants. Keep free observation algorithms, seed types, and receipt execution algorithms in their respective ownership areas. “Move entire files” is not always the correct operation.

The final invariant should be an AST assertion: **every inherent implementation whose resolved self type is the engine’s `RepoInstance` belongs to that defining module or its descendants.**

### A must not be a global path-normalization sweep

**OBSERVED:** Logical mounts differ from filesystem layout:

- `repo/repo_instance.rs:36–46`;
- `tx/apply_replicated.rs:99–100`;
- `tx/drainer.rs:100–101`.

There is also a real downstream hook consumer at `crates/shamir-db/src/shamir_db/tests/shutdown_quiesce_tests.rs:9`.

**Recommendation:**

1. Before moves, stabilize public facades and move shared hook definitions out of test implementation modules.
2. Resolve relative paths against the **current logical module graph**.
3. Rewrite move-sensitive paths in the PR that moves their owner.
4. Do not normalize every unaffected `super::` chain first.

**Rejected alternative:** A crate-wide “absolute paths everywhere” PR. It creates a large conflict surface, modifies near-limit files, and duplicates path churn without establishing semantic safety.

### Tests must move with their implementation

**OBSERVED:** `tx/tests/mod.rs:1–62` mixes commit, recovery, replication, snapshot, and SSI topics, with feature/debug gating at `:44–48` and `:59–60`.

**Recommendation:** Redistribute each owner’s tests in B/C/D/E, not collectively at E. Otherwise earlier stages either retain backwards test dependencies or silently validate an incomplete destination module.

A cross-protocol `tx/tests/` remains acceptable, but it too needs topical subfolders if its direct entries exceed seven.

### Several additional splits are mandatory

**OBSERVED:** The committed baseline includes:

- `tx/commit.rs` — 1183 lines;
- `tx/pre_commit.rs` — 2818;
- `tx/drainer.rs` — 1730;
- `tx/tests/drainer_tests.rs` — 1344;
- `tx/tests/p1096_tx_aware_unique_check_tests.rs` — 1305;
- `tx/tests/recovery_tests.rs` — 1726;
- engine integration `tests/crash_recovery.rs` — 1651.

See `scripts/ci/layout_baseline.txt:108–114`.

**Recommendation:** Include the oversized **tests**, not just implementation files, in the stage manifest. Moving any of them unchanged creates a new path-keyed violation. Layout measures tracked paths and physical newline counts, not conceptual modules (`scripts/ci/check_layout.py:55–68`, `:157–163`, `:283–295`).

## 2. Required verification machinery

These are **proposed gates**, not checks executed during this review.

### Placement and activation

Keep `check_layout.py` focused on layout. Add a sibling package such as:

```text
scripts/ci/rust_guard/
  check_modules.py
  check_conservation.py
  check_discovery.py
  check_structure.py
  exceptions.toml
  tests/
```

Keep helper subdivision within the seven-entry limit.

Run the static guards in an unconditional `source-integrity` job for **both draft and ready PRs**, before expensive work. Existing fast and full layout entry points are `.github/workflows/ci.yml:55–62` and `:143–148`.

**Do not activate conservation only from a `refactor` title.** A title is mutable, omissible, and this workflow does not currently subscribe to title-edit events (`ci.yml:6–9`).

For this campaign, require conservation whenever the diff touches engine Rust, its manifest, or guard configuration. Longer term, apply it to affected Rust crates generally: intentional changes use exact deltas rather than bypassing the checker.

### 2(a). Module reachability: syntax-aware, configuration-aware, fail-closed

**Important limitation:** An exact general Rust module resolver cannot be implemented by scanning `mod` statements alone. Arbitrary macros and generated `include!` inputs make that claim false.

#### Algorithm

1. Read tracked files using NUL-delimited Git output.
2. Discover package/target roots from Cargo manifests and Cargo metadata **in CI**:
   - explicit target paths;
   - automatic library/binary/integration/bench/example targets;
   - `autobins`, `autotests`, `autobenches`, `autoexamples`;
   - build-script configuration;
   - required features.
3. Include excluded packages such as the Node binding through their own manifests. Do not mistake the TypeScript directory for a Rust package.
4. Parse Rust with a pinned Rust grammar, not regex. Reject parse errors and unsupported relevant constructs.
5. Traverse inline and external modules while carrying:
   - physical source path;
   - logical module path;
   - module-directory resolution context;
   - inherited configuration predicate.
6. Implement ordinary `foo.rs` versus `foo/mod.rs` resolution, including inline modules and non-`mod.rs` parents. Reject ambiguity.
7. Implement literal `#[path]` and nested `cfg_attr(..., path = "...")` using Rust’s contextual resolution rules. Test those rules against compiler-accepted fixtures in CI.
8. Retain all configuration alternatives with predicates. Do not merely erase `#[cfg]`:
   - `#[cfg(test)]` is reachable in a test configuration;
   - a module under `#[cfg(any())]` has no satisfiable configuration;
   - mutually exclusive path alternatives are not simultaneous duplicate modules.
9. Track `include!` sources separately:
   - literal tracked files are explicit source edges;
   - generated `OUT_DIR` files require an exact producer/configuration declaration;
   - an opaque module-producing macro requires a reviewed adapter or explicit expansion record.
10. Fail for unaccounted tracked Rust files, broken applicable edges, unexplained source inclusion, or unhandled module-producing constructs.

Do **not** treat every file recursively beneath `tests/`, `benches/`, or `examples/` as a root. For example, `crates/shamir-engine/tests/clean_horizon/crash_matrix.rs` is mounted by `tests/crash_recovery.rs:43–44`; its existence alone must not establish reachability.

#### Exceptions

Exact path, package, reason, producer/configuration, and owner; no directory wildcards. Fail stale exceptions. Generated-source exceptions establish accounting, **not proof that the generator emits correct Rust**.

#### Failure modes and cost

- Parser/grammar drift and unhandled macros can cause false failures: fail closed and add a tested adapter.
- Satisfiable source reachability does not prove that CI actually compiles that configuration.
- **Estimated cost:** seconds for a cached source traversal; no project compilation required for the static traversal itself.

### 2(b). Refactor conservation: use two complementary inventories

**Reject names-only conservation as the primary check.** Deleting one `fn new` and adding another can preserve counts. Keeping a function’s name while weakening its assertions also passes.

#### Source inventory

Compare the merge-base and PR-head trees, read from immutable Git objects. Also validate the actual merge candidate used by CI.

Record:

- package and target membership;
- item kind and name;
- enclosing type/trait identity;
- normalized signature and `impl` head;
- configuration predicate and test attributes;
- token-normalized body fingerprint;
- original source location.

Include associated methods/items, not only top-level declarations. Treat split `impl` blocks by their member sets; splitting one block into three must not look like a behavioral addition.

Parse test-generating macro bodies with specific adapters. `loom` is not a universal test attribute: the current model is a `#[cfg(loom)] mod loom_model` (`table/writer_drain_barrier.rs:469–473`).

#### Matching and explicit deltas

- Ignore physical module-location changes when matching unchanged items.
- Normalize only paths justified by the declared module relocation map.
- Use exact old→new mappings for renamed items.
- Pair ambiguous duplicates explicitly; do not silently choose a match.
- Permit helper extraction through explicit split mappings, not blanket allowances.
- Require exact additions/removals/changes, including fingerprint/count and rationale.
- Fail unmatched losses, unexplained duplicates, unused allowances, and changed configuration/test attributes.

A TOML delta file should name the stage and expected base identity. Baseline refresh must not silently absorb unreviewed losses.

#### Compiled discovery inventory

In CI, collect pinned-nextest machine-readable lists for base and head under **identical toolchain, OS, features, profile, and target selection**. Compare test identities through the relocation/rename mapping.

Check:

- lost and unexpectedly duplicated tests;
- ignored/filtered status;
- target membership;
- feature-conditioned discovery;
- loom discovery separately;
- doctests separately.

Use the complete relevant discovery set, not only the subset selected to execute. A discovery command succeeding with zero matching tests is not a pass.

**OBSERVED:** Current CI executes affected lib tests and engine doctests (`ci.yml:102–129`), but has no before/after discovery comparison.

#### Placement and cost

- Static inventory: unconditional `source-integrity`, both tiers.
- Discovery: engine-affected fast runs on Ubuntu; full runs on all supported OSes with default configuration, plus selected feature configurations.
- **Estimated cost:** source comparison seconds; base/head discovery costs two test compilations, with substantial cold-cache cost. Cache base artifacts by SHA and configuration, never across mismatched configurations.
- Missing base history/artifacts or failed base discovery must produce an explicit blocked comparison, not an empty baseline.

### Additional structural guard

The requested four guards still do not enforce all tree rules.

Add AST checks for changed/moved areas:

- `mod.rs`: attributes, documentation, module declarations, imports/re-exports only;
- `tests/mod.rs`: manifest only;
- no inline test implementations, including specially gated loom test modules;
- no function-local imports except exact documented exceptions;
- primary-export ownership and `RepoInstance` implementation placement;
- no new broad visibility or lint-suppression escape hatches without review.

“One primary export” needs a narrow, explicit allowance for closely coupled types; semantic relatedness cannot be reliably inferred by counting public declarations.

Test the guard itself in CI with deliberately broken fixtures: dropped module declaration, impossible cfg, stale path mount, raw string containing `mod`, duplicate item names, renamed tests, altered ignore attributes, and macro-generated tests.

### 2(c). Rustdoc gate

**OBSERVED:** Engine doctests already run; they are not a substitute for building documentation (`ci.yml:113–129`, `:208–214`).

Add an engine-affected Ubuntu fast step and a full documentation job:

```text
cargo doc -p shamir-engine --no-deps --document-private-items --locked
```

Set `RUSTDOCFLAGS` to deny both broken intra-doc links and private intra-doc links. Run documentation for affected downstream crates when their API references change.

Check default and supported reduced-feature documentation configurations.

**Failure modes:** Does not validate ordinary Markdown hyperlinks, prose claims, or every configuration. Markdown relocation also requires a local-link/include-path checker.

**Estimated cost:** minutes when dependencies are cold; less with cached artifacts. Do not invent a stronger runtime guarantee.

### 2(d). Feature matrix

**OBSERVED:** Engine features are default/all-backends→fjall, `test-util`, `fts_indexed_long`, and `loom` (`Cargo.toml:65–98`). Some benches require `test-util`; `drain_throughput` requires fjall (`:130–140`, `:210–239`).

Run isolated engine-package checks—not a workspace invocation that can unify defaults back in:

- no default features;
- no defaults + fjall;
- no defaults + test-util;
- no defaults + fjall,test-util;
- default + test-util,fts_indexed_long;
- loom configuration separately, with actual loom discovery/execution.

Use `--all-targets --locked`. Assert feature resolution in CI so “no defaults” is not merely a command-line label. Required-feature targets skipped in one cell must be exercised in another.

Fast tier: at least isolated no-default and default/test-util checks for engine changes. Full tier: supported feature cells across Ubuntu, Windows, macOS; loom on Ubuntu.

**Rejected alternative:** Only `--all-features`. It can hide missing cfg guards and cannot establish reduced-feature correctness.

### Baseline integrity and required-check integrity

Add a comparison against the base baseline: **no new entries and no increased recorded sizes**. The checker’s update mode is conservative, but checking HEAD against a manually enlarged HEAD baseline is not the same protection (`check_layout.py:61–68`, `:289–295`).

Require an aggregate merge gate that fails when required jobs fail, are cancelled, or are unexpectedly skipped. Draft fast success is not authorization to merge.

## 3. Riskiest stage: drain decomposition, but pre-commit extraction is close

**OBSERVED:** `drain_step` holds `replaying` across the whole pass (`tx/drainer.rs:539–540`). It handles unresolved WAL fates before selecting a replay prefix (`:544–599`), pins orphan table handles (`:680–743`), and performs ordered finalization and cleanup (`:1381–1447`).

Its truncation helper depends on the caller retaining that guard and orders repair draining, ceiling computation, journal checkpoint, history flush, and unlink (`:1463–1471`, `:1505–1566`).

**Recommendation:** Keep the replay mutex guard in the outer function. Extract borrowed phase state; do not hide the guard inside a large state object whose destruction order becomes harder to inspect.

### Required additional CI evidence

1. **Deterministic cancellation/failure matrix** at each extracted awaited boundary:
   - pending/failed/unknown WAL fate;
   - window gap and dropped offer;
   - orphan table lookup/replacement;
   - partial history/index/counter work;
   - unsafe interner delta;
   - repair-pending journal admission;
   - checkpoint/history-flush/truncate failures.
2. **Two competing drain callers** plus normal commit traffic, checking serialization and progress.
3. **Subprocess crash tests** around checkpoint/flush/truncation, with exact seam-arrival evidence and repeated reopen assertions.
4. Assert bytes/history/index/counters and watermark relationships, not merely return values.
5. Require negative controls proving the new tests detect the ordering violation they claim to protect.
6. Preserve batching behavior: extraction must not turn one history batch per table into one per entry.

**Do not claim the existing loom job proves this decomposition safe.** It targets `WriterDrainBarrier` (`ci.yml:394–410`). Its own source carefully limits its claim and explains why it does not red/green the weak-memory ordering fix (`table/writer_drain_barrier.rs:425–466`).

Add a bounded drain-state model only for a precisely stated transition invariant, with an explicit correspondence to production behavior. Loom is supplementary; it does not model the filesystem’s persistence semantics.

Pre-commit extraction needs equally deliberate guard review: cell claims must immediately enter a releasing guard, and partial failures must release earlier claims (`tx/pre_commit.rs:121–133`, `:178–200`). Unique-write and writer-drain guards deliberately survive into materialization (`:204–214`).

## 4. What remains manual

Every extraction PR needs an old→new review table containing:

| Obligation | Required review evidence |
|---|---|
| Lock/guard lifetime | Acquisition, every await while held, explicit release, error/unwind/cancellation destruction |
| Drop order | Local declaration order, struct field order, destructuring and moved ownership |
| Cancellation | Caller future versus owned worker/driver; who continues and who settles obligations |
| Durability | Durable write, flush, publication, watermark, checkpoint and unlink order |
| Identity | Same table registration, interner and write-domain capability throughout |
| Progress/performance | No new re-entry, task spawn, unbounded scan, allocation loop, or per-entry I/O |

**OBSERVED:** `CommitDriver::drop` either transfers the exact suspended future to a task or handles unwind cleanup (`tx/commit_driver.rs:74–106`). Helper extraction can compile while changing what that suspended future owns.

Require:

- one reviewer for structural/module conservation;
- a separate reviewer for lifecycle/durability;
- explicit review of all changed visibility and cfg attributes;
- a body-by-body moved-code comparison, with extraction changes reviewed separately;
- successful CI on the final rebased merge candidate.

`git diff -M90%` and color-moved output are useful presentation aids, not evidence that nothing changed or disappeared.

## 5. Actual #189 conflict surface

**Evidence boundary:** I inspected the local remote-tracking follower-retirement branch at `54397e8a`, treating it as the #189 branch identified by the brief. I did not query GitHub; subsequent pushes or the final merge can differ.

The available diff accounts for **53 logical file changes** in engine `tx/repo/meta`, treating the fence-record rename as one change. Counting both rename endpoints gives 54 distinct paths. This is not a stable execution inventory.

Paths below are relative to `crates/shamir-engine/src/`.

### Meta — 14 logical changes

```text
meta/namespace.rs
meta/repl/bookmark.rs
meta/repl/bookmark_v3.rs
meta/repl/mod.rs
meta/repl/gate_provenance.rs                         added
meta/tests/follower_bookmark/fence_record.rs
  -> meta/repl/fence_record.rs                      renamed
meta/repl/tests/bookmark_refusal_tests.rs            added
meta/repl/tests/bookmark_v3_tests.rs
meta/repl/tests/mod.rs
meta/tests/follower_bookmark/mod.rs                  deleted
meta/tests/follower_bookmark/v2.rs                   deleted
meta/tests/follower_bookmark/v2_tests.rs             deleted
meta/tests/follower_durability_tests.rs
meta/tests/repl_bookmark_tests.rs
```

**Collisions:** facade cleanup A; metadata relocation D; root module removal F.

Notably, production bookmark code currently lives under a tests directory via a path mount (`meta/repl/bookmark.rs:7–14`). #189 removes that arrangement. Do not plan to relocate that old implementation.

The new `gate_provenance` constructs a runtime transaction gate; classify its destination by responsibility, not its current `meta/` placement.

### Repo — seven changes

```text
repo/changelog_store/follower_durability.rs
repo/durability_policy/durability_policy/repo_impl.rs
repo/mod.rs
repo/repo_instance.rs
repo/seed_install/tests/image_tests.rs
repo/seed_install/writer.rs
repo/tests/repl_bookmark_tests.rs
```

**Collisions:** A paths/hooks; D instance/open/backend/seed/durability decomposition; E receipt integration.

The open-path implementation is especially important: current construction and recovery reside in the adapter being moved, not just in `repo_instance.rs`.

### Tx — 32 changes

```text
tx/apply_replicated.rs
tx/mod.rs

tx/cold_reconcile/algorithm.rs
tx/cold_reconcile/receipt_recovery.rs
tx/cold_reconcile/tests/follower_cold_tests.rs
tx/cold_reconcile/tests/helpers.rs
tx/cold_reconcile/tests/receipt_recovery_tests.rs

tx/follower_durability/barrier.rs                    deleted
tx/follower_durability/batch.rs                      deleted
tx/follower_durability/mod.rs
tx/follower_durability/test_seam.rs

tx/follower_durability/receipt/apply.rs
tx/follower_durability/receipt/checkpoint.rs
tx/follower_durability/receipt/stage.rs
tx/follower_durability/receipt/resume/activation.rs
tx/follower_durability/receipt/resume/worker.rs

tx/follower_durability/receipt/tests/activation_routing_tests.rs
tx/follower_durability/receipt/tests/cancellation_tests.rs
tx/follower_durability/receipt/tests/crash_subprocess_tests.rs
tx/follower_durability/receipt/tests/ddl_interlock_tests.rs
tx/follower_durability/receipt/tests/receipt_apply_tests.rs

tx/follower_durability/tests/apply_crash_tests.rs      deleted
tx/follower_durability/tests/apply_failure_tests.rs    deleted
tx/follower_durability/tests/crash_image_tests.rs
tx/follower_durability/tests/gap_tests.rs
tx/follower_durability/tests/mod.rs

tx/tests/apply_replicated_counter_tests.rs
tx/tests/apply_replicated_journal_order_tests.rs
tx/tests/apply_replicated_preflight_tests.rs
tx/tests/apply_replicated_tests.rs
tx/tests/materialized_means_visible_tests.rs
tx/tests/mod.rs
tx/tests/replication_interner_persistence_tests.rs
tx/tests/replication_interner_tests.rs
```

The last listing includes 34 paths when counted independently; therefore **do not use the aggregate 53 as an execution input**: the path listing, not the approximate summary, is the useful conflict record. The complete executable inventory must be machine-produced after merge.

**Collisions:** C recovery, D worker adapters/lifecycle, E replication, and A shared paths/test hooks. B conflicts through shared manifests and test helpers even where its implementation files are unchanged.

### Re-inventory after merge

Recompute:

- module graph and all `#[path]` mounts;
- every inherent `RepoInstance` implementation;
- field visibility and all accesses;
- external facade and test-util consumers;
- feature/debug/loom test discovery;
- file lengths and directory entries;
- intentional test/protocol removals already performed by #189;
- crash seams and their actual reachable scenarios.

Do not conserve the pre-#189 legacy tests against the later layout campaign. Establish the post-merge baseline.

### Parallel sweeps

**Conditional yes**, not unrestricted parallelism.

- Table/query/validator implementation subtrees can be assigned separately after facade decisions.
- Root manifests, shared hooks, public facade edits, `Cargo.toml`, workflow files, and baseline updates need one integration owner.
- Bench and integration-test sweeps are **not independent** of path/feature changes. `tests/crash_recovery.rs:58–61` imports both `index2` and `repo::repo_types`.
- #188 may avoid engine implementation edits while still colliding with downstream caller cleanup.
- Freeze shared APIs during parallel body moves, then serialize integration and rebase verification.

## 6. Final stage list and merge gates

**Common gate G:** static module/conservation/structure/layout checks; no baseline enlargement; same-configuration discovery conservation; affected-crate clippy; engine doctests and rustdoc; feature checks; full existing three-OS lib/integration CI before merge. All execution occurs in GitHub CI.

**Sizing rule:** One semantic extraction per PR. Move-only PRs may be larger, but must have a complete mapping and no unexplained body changes.

| Stage / PR title | Scope | Additional gates | Move/split count |
|---|---|---|---|
| **0 — `ci(refactor): enforce Rust source and test conservation`** | Guard scripts, fixtures, workflow wiring | Prove guards reject deliberately broken fixtures; establish post-#189 baseline | **0 engine files moved/split** |
| **A1 — `refactor(engine): establish ownership facades and test seams`** | Shared hooks, downstream facade consumers, no global path normalization | Compile downstream test-util consumers; cfg/visibility audit | **0 filesystem moves required**; extraction count from hook inventory |
| **A2 — `refactor(engine): remove unused transaction adapters`** | Dead predicate helper/tests, exact redundant forwarders and aliases | Exact removal allowances; all callers converted | Predicate implementation + test: **2 deletions**, not moves; other deletions item-level |
| **B1 — `refactor(engine): decompose commit preparation`** | `pre_commit.rs`, guard/type ownership, corresponding tests | Claim/lock lifetime matrix; SSI/unique/cancellation tests | **1 known oversized implementation source split**; destination count manifest-required |
| **B2 — `refactor(engine): organize commit pipeline and acknowledgement`** | Commit, driver, phases, materialization, ack; owner tests | Detach/panic/ack/visibility coverage; batching review | **At least commit.rs split**; all phase destinations enumerated before execution |
| **C1 — `refactor(engine): decompose drain pass without changing ordering`** | Drain extraction at current owner, oversized drain tests | Deterministic failure/cancellation matrix; crash and concurrent-drain evidence | **2 known sources split:** drainer + drainer tests |
| **C2 — `refactor(engine): organize recovery and truncation`** | Recovery/drain/clean horizon/cold algorithms and owner tests | Replay/reopen/truncation matrix; exact guard-scope review | **At least recovery_tests.rs split**; moves from manifest |
| **D1 — `refactor(engine): establish repository instance privacy boundary`** | Type-defining module and child adapters; preserve behavior | Every private-field access and `pub(super)` reviewed | **repo_instance.rs split** plus enumerated adapter moves/extractions |
| **D2 — `refactor(engine): organize repository backend and durability`** | Backend types/composites/factory, durability/group commit | Reduced-feature/backend checks; group-commit cancellation | Backend source decomposition and adapter split counts manifest-required |
| **D3 — `refactor(engine): organize repository admission seed and metadata`** | Remaining admission/seed/meta ownership and tests | Admission/refusal/seed/reopen matrix | Moves/extractions from post-#189 manifest |
| **E1 — `refactor(engine): organize receipt replication`** | Replication algorithms/event/receipt modules; tests together | Prepared/materialized/checkpointed crash/cancellation/refusal evidence | Moves from manifest; split receipt files/tests only where required |
| **E2 — `refactor(engine): classify cross-protocol transaction tests`** | Remaining cross-protocol tests only | No lost discovery; no implementation depends on tests | Includes **p1096 test source split** if not completed in B1 |
| **F — `refactor(engine): finalize root ownership and documentation`** | Root metadata removal, migration under db_instance, index facade, docs | Broken doc/link checks; no old paths; engine-scope `#[path]` zero; final layout | Current migration subtree: **9 Rust files relocated**; delete index shim; docs separately counted |

**Move the index facade conversion as one atomic operation.** Do not delete `src/index/` in A and wait until F to replace it. Current `index` and `index2` expose different path shapes (`src/index/mod.rs:1–11`, `src/lib.rs:13–14`).

Exact total moved/split counts cannot honestly be derived from the reconciled schematic: destination filenames, topic partitions, and several ownership splits are unspecified, and #189 changes the inputs. **A committed, exhaustive mapping is a prerequisite to authorizing each stage, not documentation filled in afterward.**

## What would change my mind

I would approve a smaller sequence if a post-#189 manifest demonstrates that adjacent stages are genuinely pure relocation, preserves the defining-module privacy boundary, and produces unchanged compiled test discovery across the required configurations.

I would accept a different `instance` topology if it preserves private-field ownership without `#[path]` or visibility inflation.

I would relax a gate only with evidence that another required gate detects its specific failure class—including dropped tests—not merely that the project compiles.

**Files changed: none. Tests/builds/cargo commands run: none. Review only; CI outcomes were not verified.**