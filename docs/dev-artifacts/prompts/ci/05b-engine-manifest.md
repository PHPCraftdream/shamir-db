# Engine manifest index

**Status: ROUTING GUIDE (stage-0 deliverable), not an authorization to move code.** It fixes the source -> destination routing of all 699 engine files, the stage assignment and the stage order. The mechanical checker (`.scratch`-only, not committed) passed: 699 sources mapped once, 968 unique artifact paths, 51 baseline entries present, no projected folder above 7 entries or file above 1000 lines by the manifest's own estimates.

**Authority for correctness is NOT this document.** Each stage executor must re-derive visibility, privacy and sizes from the code at the time of the stage; the authorities are the local compiler gates (`cargo check/clippy -p shamir-engine --all-targets`, which reject any private-field or `pub(in ...)` access that a move breaks) and the CI gates (rust-guard conservation, compiled test-discovery diff, rustdoc, feature cells). The privacy/visibility rows below are advisory.

**Known defects of this routing guide (to be corrected by the stage that touches the item, not by a further manifest round):**
- Zero-line output estimates for some relocated artifacts in part 3 (for example `cold_reconcile/algorithm.rs`, `orphan_repair/repair.rs`, and test relocations around lines 412-417): the real size is the sum of the source ranges; treat any zero as unknown.
- Part 3 privacy row near line 475 gives `crate::tx::owned::recovery::orphan_repair::repair` an audience `pub(in crate::tx::owned::drain::drainer)` that is not an ancestor of the destination, and records the old path as `crate::tx::orphan_repair::repair` although the module is mounted under `crate::tx::drainer` (`drainer.rs:100-101`).
- Part 2 privacy rows state "concrete consumers not resolved"; part 4 rows 317-320 are open (a sibling journal reader, call-reference checks, and `wal_segment_bytes`, which is NOT `#[cfg(test)]`, repo_instance.rs:174).
- Part 3 line 418 proposes six `pub(crate) mod` preparation modules without individual justification; line 221 declares `pub mod unique_check;` without a nested artifact.

This index is generated from the current layout baseline and round-2 decisions. The four exhaustive part documents contain detailed mappings, projected artifacts, privacy records, and baseline inventories.

## Observed engine baseline inventory

All 51 engine baseline entries are assigned below; counts are baseline facts, not relocation authorization.

| Kind | Baseline path | Count | Stage |
|---|---|---:|---|
| dir | `crates/shamir-engine/benches` | 39 | Q |
| dir | `crates/shamir-engine/src` | 12 | F |
| dir | `crates/shamir-engine/src/query` | 8 | Q |
| dir | `crates/shamir-engine/src/query/batch` | 19 | Q |
| dir | `crates/shamir-engine/src/query/batch/tests` | 33 | Q |
| dir | `crates/shamir-engine/src/query/batch/tests/executor_tests` | 12 | Q |
| dir | `crates/shamir-engine/src/query/filter` | 16 | Q |
| dir | `crates/shamir-engine/src/query/filter/tests` | 8 | Q |
| dir | `crates/shamir-engine/src/query/filter/tests/eval_tests` | 19 | Q |
| dir | `crates/shamir-engine/src/query/read` | 8 | Q |
| dir | `crates/shamir-engine/src/query/read/tests` | 11 | Q |
| dir | `crates/shamir-engine/src/repo` | 19 | D1 |
| dir | `crates/shamir-engine/src/repo/tests` | 21 | D3 |
| dir | `crates/shamir-engine/src/table` | 38 | T |
| dir | `crates/shamir-engine/src/table/tests` | 114 | T |
| dir | `crates/shamir-engine/src/tx` | 21 | E2 |
| dir | `crates/shamir-engine/src/tx/tests` | 60 | E2 |
| dir | `crates/shamir-engine/src/tx/tests/ssi_phantom_tests` | 9 | B1 |
| dir | `crates/shamir-engine/src/validator` | 18 | Q |
| dir | `crates/shamir-engine/src/validator/schema` | 11 | Q |
| dir | `crates/shamir-engine/src/validator/tests` | 13 | Q |
| file | `crates/shamir-engine/benches/tx_concurrent.rs` | 1013 | Q |
| file | `crates/shamir-engine/src/query/batch/fk_actions.rs` | 1438 | Q |
| file | `crates/shamir-engine/src/query/batch/fk_on_update.rs` | 1353 | Q |
| file | `crates/shamir-engine/src/query/batch/query_runner.rs` | 2006 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_actions_tests.rs` | 1688 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_on_update_tests.rs` | 2481 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs` | 1147 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/fk_ri_barrier_tests.rs` | 1341 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` | 2151 | Q |
| file | `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` | 1509 | Q |
| file | `crates/shamir-engine/src/query/filter/filter_node.rs` | 1099 | Q |
| file | `crates/shamir-engine/src/query/filter/tests/eval_tests/collection_tests.rs` | 1318 | Q |
| file | `crates/shamir-engine/src/query/read/aggregate.rs` | 1779 | Q |
| file | `crates/shamir-engine/src/query/read/tests/exec_tests.rs` | 1222 | Q |
| file | `crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs` | 1354 | Q |
| file | `crates/shamir-engine/src/repo/repo_instance.rs` | 1967 | D1 |
| file | `crates/shamir-engine/src/table/read_exec.rs` | 3092 | T |
| file | `crates/shamir-engine/src/table/table_manager.rs` | 1536 | T |
| file | `crates/shamir-engine/src/table/table_manager_tx_ops.rs` | 1552 | T |
| file | `crates/shamir-engine/src/table/tests/filtered_ann_tests.rs` | 1107 | T |
| file | `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs` | 1155 | T |
| file | `crates/shamir-engine/src/table/tests/write_exec_tests.rs` | 1009 | T |
| file | `crates/shamir-engine/src/table/write_exec.rs` | 1452 | T |
| file | `crates/shamir-engine/src/tx/commit.rs` | 1183 | B2 |
| file | `crates/shamir-engine/src/tx/drainer.rs` | 1730 | C1 |
| file | `crates/shamir-engine/src/tx/pre_commit.rs` | 2818 | B1 |
| file | `crates/shamir-engine/src/tx/tests/drainer_tests.rs` | 1344 | C1 |
| file | `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check_tests.rs` | 1305 | B1 |
| file | `crates/shamir-engine/src/tx/tests/recovery_tests.rs` | 1725 | C2 |
| file | `crates/shamir-engine/tests/crash_recovery.rs` | 1651 | Q |

## Manifest parts

| Part | Scope |
|---|---|
| [`05b-query.md`](05b-1-query.md) | Q decisions, full source-to-destination tables, artifacts and privacy evidence |
| [`05b-table.md`](05b-2-table.md) | T decisions, full source-to-destination tables, artifacts and privacy evidence |
| [`05b-transactions.md`](05b-3-transactions.md) | B/C/E decisions, full source-to-destination tables, artifacts and privacy evidence |
| [`05b-repository.md`](05b-4-repository.md) | D decisions, full source-to-destination tables, artifacts and privacy evidence |

## Campaign stages

Common gate G: static module/conservation/structure/layout checks; no baseline enlargement; same-configuration discovery conservation; affected-crate clippy; engine doctests and rustdoc; feature checks; full existing three-OS lib/integration CI before merge. These execute in GitHub CI only.

| Stage | Scope | Depends on |
|---|---|---|
| A1 | Ownership facades, hooks and downstream consumers | stage 0 |
| A2 | Unused transaction adapters; exact removal allowances; retain/retarget tests | A1 |
| B1 | Pre-commit extraction and corresponding tests | A2 |
| B2 | Commit pipeline, driver, phases, acknowledgements and owner tests | B1 |
| C1 | Drain implementation and drain tests | B2 |
| C2 | Recovery, truncation and recovery tests | C1 |
| D1 | RepoInstance defining module and privacy descendants | C2 |
| D2 | Repository backend and durability organization | D1 |
| D3 | Repository admission, seed, metadata and owner tests | D2 |
| E1 | Receipt/replication algorithms and tests | D3 |
| E2 | Remaining cross-protocol transaction tests | E1 |
| Q | Query, validator, benches and engine integration tests | A1 |
| T | Table implementation and tests | A1 |
| F | Root ownership, migration/index facade atomic conversion and documentation | E2, Q, T |

## Campaign registry and delta files

Per-stage campaign names and package arrays (A1 and F additionally gate the db and server crates; every other stage is engine-only). Delta files live under `.rust-guard/stages/`.

| Stage | Campaign | Packages | Delta |
|---|---|---|---|
| A1 | engine-sweep-a1 | shamir-engine, shamir-db, shamir-server | `.rust-guard/stages/engine-sweep-a1.toml` |
| A2 | engine-sweep-a2 | shamir-engine | `.rust-guard/stages/engine-sweep-a2.toml` |
| B1 | engine-sweep-b1 | shamir-engine | `.rust-guard/stages/engine-sweep-b1.toml` |
| B2 | engine-sweep-b2 | shamir-engine | `.rust-guard/stages/engine-sweep-b2.toml` |
| C1 | engine-sweep-c1 | shamir-engine | `.rust-guard/stages/engine-sweep-c1.toml` |
| C2 | engine-sweep-c2 | shamir-engine | `.rust-guard/stages/engine-sweep-c2.toml` |
| D1 | engine-sweep-d1 | shamir-engine | `.rust-guard/stages/engine-sweep-d1.toml` |
| D2 | engine-sweep-d2 | shamir-engine | `.rust-guard/stages/engine-sweep-d2.toml` |
| D3 | engine-sweep-d3 | shamir-engine | `.rust-guard/stages/engine-sweep-d3.toml` |
| E1 | engine-sweep-e1 | shamir-engine | `.rust-guard/stages/engine-sweep-e1.toml` |
| E2 | engine-sweep-e2 | shamir-engine | `.rust-guard/stages/engine-sweep-e2.toml` |
| Q | engine-sweep-q | shamir-engine | `.rust-guard/stages/engine-sweep-q.toml` |
| T | engine-sweep-t | shamir-engine | `.rust-guard/stages/engine-sweep-t.toml` |
| F | engine-sweep-f | shamir-engine, shamir-db, shamir-server | `.rust-guard/stages/engine-sweep-f.toml` |

Q and T may proceed in parallel with one another and the spine only on disjoint body files; shared manifests, hooks, public facades, Cargo metadata, workflow files and baseline updates require one serialized integrator.

## Campaign and delta examples

Campaign names, package arrays and per-stage delta files are fixed by the table above (delta files under `.rust-guard/stages/`).

Source conservation delta:

```toml
base = "<base-sha>"

[[moved]]
path = "<BASE-SOURCE-PATH>"
item = "<FULL-LOGICAL-ITEM>"
to_path = "<NEW-SOURCE-PATH>"
to_item = "<NEW-FULL-LOGICAL-ITEM>"
reason = "<reason>"
owner = "<owner>"
```

Discovery delta (same moved fields; `path`/`item` identify binary and test name):

```toml
base = "<base-sha>"

[[moved]]
path = "<BASE-BINARY-ID>"
item = "<BASE-TEST-NAME>"
to_path = "<NEW-BINARY-ID>"
to_item = "<NEW-TEST-NAME>"
reason = "<reason>"
owner = "<owner>"
```

Discovery ledger integration remains an owner question; this documentation does not claim it is implemented.

## Acceptance, manual review and limits

Stage-specific acceptance is defined in the adjudicated plan: A1 downstream test-util/cfg/visibility audit; A2 exact removal allowances/caller conversion; B1 claim/lock lifetime and SSI/unique/cancellation coverage; B2 detach/panic/ack/visibility and batching; C1 deterministic failure/cancellation/crash/concurrent-drain evidence; C2 replay/reopen/truncation and guard scope; D1 private-field/`pub(super)` review; D2 backend-feature and group-commit cancellation; D3 admission/refusal/seed/reopen; E1 prepared/materialized/checkpointed crash/cancellation/refusal; E2 discovery conservation and no implementation dependency on tests; Q/T disjoint ownership and complete gates; F broken doc/link checks, old-path removal, zero engine `#[path]` mounts and final layout.

Manual review per extraction: lock/guard lifetime, drop order, cancellation/unwind ownership, durability/publication/checkpoint/unlink order, identity preservation, progress/performance. Privacy audiences and caller API semantics require semantic human review.

## Owner questions (recommendations; not human requests)

1. Keep private-field implementation descendants nested under the defining `RepoInstance` module rather than widen visibility.
2. Resolve index compatibility and adapter-test retargeting at A2 before changing its public path shape.
3. Freeze Q/T ownership to disjoint body files and serialize shared module/API edits.
4. Assign one integrator for shared hooks, manifests, Cargo metadata, guard state, and baseline.
5. Require a distinct owner for discovery-ledger integration; do not imply it exists.

The verifier checks declared inventory, syntax/ranges, UTF-8 evidence anchors, baseline values, and permitted file scope. It is not an AST or Rust module reachability proof; manual structural, privacy, caller-API, and lifetime/durability reviews remain required.

Checker (docs/scratch-only): `python -I .scratch/engine_inventory/check_round2.py`.

**Verification: the mechanical checker above passed twice; no Cargo/build/test execution was part of this documentation task. Semantic privacy, lifetime and durability review remain per-stage obligations.**
