# Engine manifest index

**Status: FINAL PARTIAL — 699 Markdown source rows covered once; all 51 baseline entries present. Acceptance FAIL: primary/split sizing conflicts, duplicate planned outputs, source-range conservation, and visibility graph are not certified. This is not execution-ready.**

- [Part 1: Query](05b-1-query.md)
- [Part 2: Table](05b-2-table.md)
- [Part 3: Transactions](05b-3-transactions.md)
- [Part 4: Repository](05b-4-repository.md)



Generated from `.scratch/engine_inventory/inventory.json`; these 51 baseline violations are route assignments, not move authorization.

| Kind | Baseline path | Count | Stage |
|---|---|---:|---|
| dir | `crates/shamir-engine/benches` | 39 | A1 |
| dir | `crates/shamir-engine/src` | 12 | A1 |
| dir | `crates/shamir-engine/src/query` | 8 | F |
| dir | `crates/shamir-engine/src/query/batch` | 19 | Q |
| dir | `crates/shamir-engine/src/query/batch/tests` | 33 | Q |
| dir | `crates/shamir-engine/src/query/batch/tests/executor_tests` | 12 | Q |
| dir | `crates/shamir-engine/src/query/filter` | 16 | Q |
| dir | `crates/shamir-engine/src/query/filter/tests` | 8 | Q |
| dir | `crates/shamir-engine/src/query/filter/tests/eval_tests` | 19 | Q |
| dir | `crates/shamir-engine/src/query/read` | 8 | Q |
| dir | `crates/shamir-engine/src/query/read/tests` | 11 | Q |
| dir | `crates/shamir-engine/src/repo` | 19 | F |
| dir | `crates/shamir-engine/src/repo/tests` | 21 | D3 |
| dir | `crates/shamir-engine/src/table` | 38 | F |
| dir | `crates/shamir-engine/src/table/tests` | 114 | T |
| dir | `crates/shamir-engine/src/tx` | 21 | F |
| dir | `crates/shamir-engine/src/tx/tests` | 60 | E2 |
| dir | `crates/shamir-engine/src/tx/tests/ssi_phantom_tests` | 9 | E2 |
| dir | `crates/shamir-engine/src/validator` | 18 | F |
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
| file | `crates/shamir-engine/src/tx/tests/drainer_tests.rs` | 1344 | E2 |
| file | `crates/shamir-engine/src/tx/tests/p1096_tx_aware_unique_check_tests.rs` | 1305 | B1 |
| file | `crates/shamir-engine/src/tx/tests/recovery_tests.rs` | 1725 | C2 |
| file | `crates/shamir-engine/tests/crash_recovery.rs` | 1651 | Q |

## Fourteen campaign stages

Q and T run in parallel with one another and the spine, limited to disjoint body files. Shared manifest, guards, Cargo metadata, hooks and other shared files are serialized by one integrator.

| Stage | Campaign | Scope | Depends on | Packages | Delta |
|---|---|---|---|---|---|
| A1 | `engine-sweep-a1` | Shared baseline/guards/hooks; owner decisions | stage0 | shamir-engine, shamir-db, shamir-server | `.rustguard/stages/engine-sweep-a1.toml` |
| A2 | `engine-sweep-a2` | Privacy descendants inline; index compatibility and semver decision | A1 | shamir-engine | `.rustguard/stages/engine-sweep-a2.toml` |
| B1 | `engine-sweep-b1` | Precommit and p1096 | A2 | shamir-engine | `.rustguard/stages/engine-sweep-b1.toml` |
| B2 | `engine-sweep-b2` | Commit path | B1 | shamir-engine | `.rustguard/stages/engine-sweep-b2.toml` |
| C1 | `engine-sweep-c1` | Drainer implementation and tests | B2 | shamir-engine | `.rustguard/stages/engine-sweep-c1.toml` |
| C2 | `engine-sweep-c2` | Recovery tests | C1 | shamir-engine | `.rustguard/stages/engine-sweep-c2.toml` |
| D1 | `engine-sweep-d1` | RepoInstance and metadata roots | C2 | shamir-engine | `.rustguard/stages/engine-sweep-d1.toml` |
| D2 | `engine-sweep-d2` | Repository body files; backend durability | D1 | shamir-engine | `.rustguard/stages/engine-sweep-d2.toml` |
| D3 | `engine-sweep-d3` | Repository tests | D2 | shamir-engine | `.rustguard/stages/engine-sweep-d3.toml` |
| E1 | `engine-sweep-e1` | Transaction receipt | D3 | shamir-engine | `.rustguard/stages/engine-sweep-e1.toml` |
| E2 | `engine-sweep-e2` | Cross-protocol transaction ownership and tests | E1 | shamir-engine | `.rustguard/stages/engine-sweep-e2.toml` |
| Q | `engine-sweep-q` | Query/validator/benches plus tx-concurrent and crash-recovery tests | A1 | shamir-engine | `.rustguard/stages/engine-sweep-q.toml` |
| T | `engine-sweep-t` | Stable table implementation/tests | A1 | shamir-engine | `.rustguard/stages/engine-sweep-t.toml` |
| F | `engine-sweep-f` | Engine src-root integration | E2 + Q + T | shamir-engine, shamir-db, shamir-server | `.rustguard/stages/engine-sweep-f.toml` |

## Owner questions — all blocking; no answers recorded

1. Should index privacy descendants be inlined? Recommendation: inline rather than widen visibility.
2. Is changing the index public surface semver-compatible? Recommendation: require explicit semver approval before breakage.
3. What happens to index adapter tests? Recommendation: retain and retarget to canonical implementation.
4. How are Q/T files frozen? Recommendation: restrict to disjoint body files.
5. Who edits shared manifest/guard/Cargo/hook state? Recommendation: one serialized integrator with shared ledger.
6. Can Q/T overlap spine work? Recommendation: yes, only on disjoint body files, with A1 dependency.
7. What gates A2? Recommendation: owner resolves compatibility and retargeting before merge.
8. What is common acceptance? Recommendation: independent source and compiled-discovery ledgers both pass.
9. Who signs off durability-sensitive behavior? Recommendation: stage owner plus independent reviewer.

## TOML skeletons (separate files)

Campaign registry:

```toml
[[campaigns]]
name = "engine-sweep-<lower>"
packages = ["shamir-engine"]
delta = ".rustguard/stages/engine-sweep-<lower>.toml"
```

Source delta:

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

Discovery delta (same moved fields; path/item represent binary ID/test name):

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

No `[[removed]]` binary/test shape is supported.

## Acceptance and manual review

Common acceptance G: exact source conservation; separate compiled discovery conservation with identical collection conditions; no test loss/ignore changes; clean structure/layout scopes; reviewed package/delta declarations. Each stage manually reviews lifetime/drop order, cancellation/unwind, lock-across-await and durability/publication ordering. D2: backend durability. E1: receipt. E2: cross-protocol.
Recommendation: privacy descendants inline; settle index compatibility and retarget tests at A2; freeze Q/T to disjoint body files; one shared-ledger integrator gates shared edits.
OBSERVED: 699 tracked engine files; 51 baseline entries (21 overfull directories and 30 oversized source files); 32 actual path attributes; 13 inherent RepoInstance impl sites.
Verification status: pending orchestrator updates.
