<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-engine — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Pre-read swallowing remains open. Counter initialization and commit-entry cleanup are fixed; FK corruption handling and attach retry lifecycle remain incomplete.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 1 | 9 | 2 | 1 | 2 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Write-path pre-reads swallow all read errors as record does not exist

Status: `confirmed-open`. Current risk: `high`.

delete/set/update_tx still use .ok(); existing rows can be mistaken for absent after storage or codec failure.

Evidence: [crates/shamir-engine/src/table/table_manager_crud.rs:457](../../../../../crates/shamir-engine/src/table/table_manager_crud.rs#L457); [crates/shamir-engine/src/table/table_manager_crud.rs:537](../../../../../crates/shamir-engine/src/table/table_manager_crud.rs#L537); [crates/shamir-engine/src/table/table_manager_tx_ops.rs:1106](../../../../../crates/shamir-engine/src/table/table_manager_tx_ops.rs#L1106).

<a id="review-2"></a>

### Claim 2 — Commit-entry ? on tx_gate/repo_wal bypasses pessimistic-lock release

Status: `fixed`. Current risk: —.

Both resolutions have explicit error arms releasing locks. The registered WAL-init-failure test checks a subsequent transaction can acquire the same key.

Evidence: [crates/shamir-engine/src/tx/commit.rs:598](../../../../../crates/shamir-engine/src/tx/commit.rs#L598); [crates/shamir-engine/src/tx/commit.rs:605](../../../../../crates/shamir-engine/src/tx/commit.rs#L605); [crates/shamir-engine/src/tx/tests/commit_wal_init_failure_lock_release_tests.rs:90](../../../../../crates/shamir-engine/src/tx/tests/commit_wal_init_failure_lock_release_tests.rs#L90).

<a id="review-3"></a>

### Claim 3 — RecordCounter lazy init swallows read/decode errors and silently zeroes durable count

Status: `fixed`. Current risk: —.

get_or_try_init maps only NotFound to zero; storage/decode errors propagate without initializing. Registered tests inspect unchanged backing bytes.

Evidence: [crates/shamir-engine/src/table/record_counter.rs:198](../../../../../crates/shamir-engine/src/table/record_counter.rs#L198); [crates/shamir-engine/src/table/record_counter.rs:208](../../../../../crates/shamir-engine/src/table/record_counter.rs#L208); [crates/shamir-engine/src/table/tests/record_counter_tests.rs:279](../../../../../crates/shamir-engine/src/table/tests/record_counter_tests.rs#L279).

<a id="review-4"></a>

### Claim 4 — FK parent-value scans silently skip rows that fail to decode

Status: `partially-fixed`. Current risk: `medium`.

Extraction now errors when both decoders reject. Header-valid malformed maps still become absent scalars, and filter_matches can discard corrupt rows before extraction.

Evidence: [crates/shamir-engine/src/query/batch/fk_restrict.rs:307](../../../../../crates/shamir-engine/src/query/batch/fk_restrict.rs#L307); [crates/shamir-engine/src/query/batch/fk_restrict.rs:525](../../../../../crates/shamir-engine/src/query/batch/fk_restrict.rs#L525); [crates/shamir-engine/src/validator/validator_db.rs:109](../../../../../crates/shamir-engine/src/validator/validator_db.rs#L109); [crates/shamir-types/src/record_view/lens.rs:814](../../../../../crates/shamir-types/src/record_view/lens.rs#L814).

<a id="review-5"></a>

### Claim 5 — per_table_mvcc attach discards the split-brain error signal

Status: `fixed`. Current risk: —.

Occupied-token insertion now logs and returns Internal instead of silently attaching another store. Cleanup after later initialization failure remains a separate regression.

Evidence: [crates/shamir-engine/src/repo/repo_instance.rs:411](../../../../../crates/shamir-engine/src/repo/repo_instance.rs#L411); [crates/shamir-engine/src/repo/tests/per_table_mvcc_attach_collision_tests.rs:74](../../../../../crates/shamir-engine/src/repo/tests/per_table_mvcc_attach_collision_tests.rs#L74).

<a id="review-6"></a>

### Claim 6 — Background verify never clears its single-flight latch if the task panics

Status: `fixed`. Current risk: —.

verify is caught and the latch cleared after error handling. The accompanying panic regression file is present but unregistered.

Evidence: [crates/shamir-engine/src/table/table_manager.rs:1017](../../../../../crates/shamir-engine/src/table/table_manager.rs#L1017); [crates/shamir-engine/src/table/table_manager.rs:1045](../../../../../crates/shamir-engine/src/table/table_manager.rs#L1045); [crates/shamir-engine/src/table/tests/mod.rs:1](../../../../../crates/shamir-engine/src/table/tests/mod.rs#L1).

<a id="review-7"></a>

### Claim 7 — Silent best-effort operations drop errors without logging

Status: `fixed`. Current risk: —.

Assigned metadata-save, replay-attach and recovery-marker acquisition failures now emit warnings explaining accepted consequences.

Evidence: [crates/shamir-engine/src/table/table_manager.rs:751](../../../../../crates/shamir-engine/src/table/table_manager.rs#L751); [crates/shamir-engine/src/table/doctor.rs:760](../../../../../crates/shamir-engine/src/table/doctor.rs#L760); [crates/shamir-engine/src/tx/recovery.rs:198](../../../../../crates/shamir-engine/src/tx/recovery.rs#L198); [crates/shamir-engine/src/tx/commit_phases.rs:382](../../../../../crates/shamir-engine/src/tx/commit_phases.rs#L382).

<a id="review-8"></a>

### Claim 8 — apply_replicated conflates attach failure with unattached table

Status: `fixed`. Current risk: —.

Only NotFound may continue; other attachment errors return Err before applying the event or advancing the caller's bookmark.

Evidence: [crates/shamir-engine/src/tx/apply_replicated.rs:289](../../../../../crates/shamir-engine/src/tx/apply_replicated.rs#L289); [crates/shamir-engine/src/tx/tests/apply_replicated_tests.rs:572](../../../../../crates/shamir-engine/src/tx/tests/apply_replicated_tests.rs#L572).

<a id="review-9"></a>

### Claim 9 — Missing error-path tests for the specific gaps above

Status: `partially-fixed`. Current risk: `low`.

Commit-entry, counter and one FK-corruption regression are registered. Pre-read tests are unregistered; remaining corruption variants are not covered by the added malformed-header fixture.

Evidence: [crates/shamir-engine/src/tx/tests/mod.rs:10](../../../../../crates/shamir-engine/src/tx/tests/mod.rs#L10); [crates/shamir-engine/src/table/tests/mod.rs:74](../../../../../crates/shamir-engine/src/table/tests/mod.rs#L74); [crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs:633](../../../../../crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs#L633); [crates/shamir-engine/src/table/tests/mod.rs:1](../../../../../crates/shamir-engine/src/table/tests/mod.rs#L1).

<a id="review-10-1"></a>

### Claim 10.1 — SystemTime::now().duration_since(UNIX_EPOCH).unwrap repeated ten times

Status: `fixed`. Current risk: —.

Assigned call sites use shared unix_millis, whose pre-epoch case returns zero.

Evidence: [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:1136](../../../../../crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L1136); [crates/shamir-types/src/time.rs:14](../../../../../crates/shamir-types/src/time.rs#L14).

<a id="review-10-2"></a>

### Claim 10.2 — ids.lock().unwrap poisoning cascade

Status: `fixed`. Current risk: —.

Production guard operations recover the poisoned mutex's structurally valid map.

Evidence: [crates/shamir-engine/src/table/in_flight_create_guard.rs:110](../../../../../crates/shamir-engine/src/table/in_flight_create_guard.rs#L110); [crates/shamir-engine/src/table/in_flight_create_guard.rs:130](../../../../../crates/shamir-engine/src/table/in_flight_create_guard.rs#L130); [crates/shamir-engine/src/table/in_flight_create_guard.rs:166](../../../../../crates/shamir-engine/src/table/in_flight_create_guard.rs#L166).

<a id="review-10-3"></a>

### Claim 10.3 — QueryParseError/WriteValueError lack thiserror

Status: `fixed`. Current risk: —.

QueryParseError disappeared with the legacy parser; WriteValueError now derives thiserror::Error.

Evidence: [crates/shamir-engine/src/query/mod.rs:1](../../../../../crates/shamir-engine/src/query/mod.rs#L1); [crates/shamir-engine/src/query/batch/param_subst.rs:145](../../../../../crates/shamir-engine/src/query/batch/param_subst.rs#L145).

<a id="review-verified-non-issues-unwrap-invariants"></a>

### Claim Verified non-issues/unwrap invariants — Every remaining production unwrap/expect is structurally guarded

Status: `unverified`. Current risk: —.

Several cited guards remain visible, but the universal claim exceeds this finding-scoped review and no compiler/test verification was allowed.

Evidence: [crates/shamir-engine/src/query/auth/session.rs:184](../../../../../crates/shamir-engine/src/query/auth/session.rs#L184).

<a id="review-verified-non-issues-scc-ignored-results"></a>

### Claim Verified non-issues/scc ignored results — Ignored scc results are idempotent or benign by contract

Status: `unverified`. Current risk: —.

Specific idempotent sites do not establish this blanket property; the attach collision previously cited as benign required explicit handling.

Evidence: [crates/shamir-engine/src/repo/repo_instance.rs:411](../../../../../crates/shamir-engine/src/repo/repo_instance.rs#L411); [crates/shamir-engine/src/validator/registry.rs:90](../../../../../crates/shamir-engine/src/validator/registry.rs#L90).

<a id="review-verified-non-issues-resource-lifecycles"></a>

### Claim Verified non-issues/resource lifecycles — Resource lifecycles are sound and documented

Status: `refuted`. Current risk: —.

Successful MVCC registration precedes fallible table/interner initialization without rollback; later retries fail on the orphan registration.

Evidence: [crates/shamir-engine/src/repo/repo_instance.rs:413](../../../../../crates/shamir-engine/src/repo/repo_instance.rs#L413); [crates/shamir-engine/src/repo/repo_instance.rs:442](../../../../../crates/shamir-engine/src/repo/repo_instance.rs#L442); [crates/shamir-engine/src/repo/repo_instance.rs:447](../../../../../crates/shamir-engine/src/repo/repo_instance.rs#L447).

<a id="review-verified-non-issues-test-only-hooks"></a>

### Claim Verified non-issues/test-only hooks — Failure-injection synchronous locks are test-only

Status: `not-applicable`. Current risk: —.

The inspected failure-injection lock types and their hooks remain test-gated.

Evidence: [crates/shamir-engine/src/tx/pre_commit.rs:1037](../../../../../crates/shamir-engine/src/tx/pre_commit.rs#L1037); [crates/shamir-engine/src/table/table_manager_streaming.rs:77](../../../../../crates/shamir-engine/src/table/table_manager_streaming.rs#L77).

## Corrections and qualified non-findings

- Presence of a new test file is not registration: write_path_preread_fail_closed_tests.rs and verify_panic_safety_tests.rs are absent from the table test manifest.
- The pre-read file's get-hook claim is false: TableManager::get does not consult the injector; read_one_tx does.
- The FK added fixture uses a non-map/truncated header and does not detect malformed map bodies accepted by RecordView::new.
- Attach-collision detection is repaired, but the comment promising a clean automatic retry is false after later initialization errors.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-engine -- Error handling & resource lifecycle

## Summary

Error-handling discipline in this crate is generally excellent: zero `panic!`/`todo!`/`unimplemented!` and no `anyhow` in production code; `thiserror` on the principal error enums (`TxError`, `ValidatorRegistryError`, `ValidatorDecodeError`); a pervasive RAII-guard culture (`VersionGuard`, `CellReservationGuard`, `WriterDrainGuard`, `WriteBarrierGuard`, `InFlightCreateGuard`, `OpGuard`, `CascadePathGuard`) with documented release-on-every-exit semantics; and a canonical NotFound-vs-real-error discipline (`meta/recovery_marker.rs::load_u64`, `pre_commit.rs::read_pre_tx_bytes` F-73) that past audits (#435, #881, #891, #900, #1013) progressively enforced. The residual defects cluster in older code that predates that discipline: three write-path pre-reads collapse *every* read error into "record absent" (silent no-op deletes / create-path updates with stale unique postings), a handful of swallowed error signals on lifecycle paths (counter init, MVCC attach, WAL-init-at-commit), and FK parent-scan loops that still silently skip decode-corrupt rows — the exact defect class F-65/#891 fixed for read errors but left open for decodes. Error-path test coverage is strong overall (extensive `#[cfg(test)]` fault-injection seams, crash-injection harness, `test-util` feature) but misses the specific gaps below.

## Findings

### 1. Write-path pre-reads swallow all read errors as "record does not exist"
- **File:** `src/table/table_manager_crud.rs:431` (`delete_returning_version`), `src/table/table_manager_crud.rs:511` (`set_returning_version`), `src/table/table_manager_tx_ops.rs:1075` (`update_tx`)
- **Severity:** high
- **Issue:** `let old_value = self.get(id).await.ok();` (and `self.read_one_tx(id, Some(&*tx)).await.ok()`) conflates `Err(DbError::NotFound)` — the expected "row absent" signal — with `Storage` I/O errors and `Codec` decode errors (`get()` maps a corrupt stored record to `Err(Codec)` via `InnerValue::from_bytes`, `table_manager_crud.rs:591-601`). Non-NotFound errors must propagate; only NotFound means "absent".
- **Failure scenario:** A corrupt or unreadable stored record turns `delete()` into a silent successful no-op returning `(false, 0)` "record did not exist" — the record survives, no error is surfaced, index cleanup and history archiving are skipped. `set()`/`update_tx()` instead take the create path: `validate_unique_for_create` runs against a row that still exists (spurious unique violations, or wrong exclusion), old postings are never removed (`plan_insert_ops` instead of `plan_update_ops` — the OLD unique key stays claimed forever, permanently blocking future inserts of that value), and the record counter double-counts (+1 for an existing row). This is the exact fail-open defect class the crate itself fixed elsewhere (F-73 `read_pre_tx_bytes` doc: "a transient error here used to let the tx commit successfully while silently skipping the row's posting"; `meta/recovery_marker.rs::load_u64`'s `Err(NotFound) => Ok(None), Err(e) => Err(e)` is the canonical pattern).
- **Suggested fix:** Replace `.ok()` with a match that maps `NotFound` to `None` and propagates every other error via `?` (mirror `read_pre_tx_bytes`). Add a corrupt-record/se injected-read regression test per site (the `TEST_READ_ONE_TX_BYTES_FAILURE` seam already exists for the tx path).

### 2. Commit-entry `?` on `tx_gate()`/`repo_wal()` bypasses pessimistic-lock release
- **File:** `src/tx/commit.rs:589-590` (`commit_tx_inner`)
- **Severity:** medium
- **Issue:** Every other early-exit in `commit_tx_inner`/`commit_tx_lockfree`/`commit_tx_inner_legacy_async` (lines 574-576, 580-582, 603-608, 686-689, 929-932, 952-965) explicitly calls `release_pessimistic_locks(&tx, repo).await` before returning `Err`. The two `?`s at function entry — `let gate = repo.tx_gate().await?; let wal = repo.repo_wal().await?;` — do not. Per the crate's own documentation (`batch_execute.rs:66-68`: "TxContext has no Drop impl for Level-3 locks"), a `Pessimistic` tx aborted through this path leaks its `locked_keys` permanently; younger waiters park in `lock_key` with no timeout (only an *older* waiter can wound the holder, and the holder never runs again).
- **Failure scenario:** `repo_wal()` lazily performs real I/O on first touch (`create_dir_all`, `SegmentSet::open`, `repo_instance.rs:741-820`); an interactive Pessimistic tx created via the engine API (`RepoInstance::begin_tx(Pessimistic)`) whose commit is the repo's first WAL touch fails on a permissions/disk error → `?` propagates → locks leak → subsequent lockers of those keys hang. (Currently mitigated by the wire layer only exposing Snapshot/Serializable for interactive txs, `shamir-db/.../db_tx.rs:83-84` — direct embedders of the engine crate are exposed.)
- **Suggested fix:** Wrap both resolutions so the `Err` arm calls `release_pessimistic_locks(&tx, repo).await` before returning, matching the sibling paths. Add a fail-injection test (force `repo_wal()` init failure on a Pessimistic tx; assert the key is re-lockable).

### 3. `RecordCounter` lazy init swallows read and decode errors → durable counter silently zeroed
- **File:** `src/table/record_counter.rs:175-178` (`ensure_cache`)
- **Severity:** medium
- **Issue:** `Err(_) => 0` treats *any* info_store read error as "no persisted count", and `bincode::from_bytes(&bytes).unwrap_or(0)` treats a corrupt count blob as 0 — both silently. `last_persisted` is seeded to 0 alongside, so the first `persist()` after any increment sees `cur != last` and `write_through(cur)` **overwrites** the previously durable count (e.g. 10 000 → 1). This contradicts the same crate's interner policy (`interner_manager.rs:143-151`, audit §2.6: corruption is fatal, not skippable, precisely because silently-truncated state is worse than a failed open).
- **Failure scenario:** Transient I/O error on the counter key at first `count()` after open → in-memory count = 0 → next write increments and persists 1 → durable count destroyed until an operator runs `doctor.repair()` (which does reconcile via full scan — the mitigation that keeps this medium, not high).
- **Suggested fix:** Distinguish `NotFound` (→ 0) from other errors (→ propagate `Err`); on decode failure either fail or at least `log::warn!` and skip the write-back (never persist a value derived from a defaulted init). Add tests for both error classes.

### 4. FK parent-value scans silently skip rows that fail to decode
- **File:** `src/query/batch/fk_actions.rs:661`, `src/query/batch/fk_actions.rs:1142` (`collect_parent_values`), `src/query/batch/fk_on_update.rs:845`, `src/query/batch/fk_restrict.rs:270`; same class in `src/validator/validator_db.rs:98-108` (`record_field_matches_by_id` returns `false` on decode failure)
- **Severity:** medium
- **Issue:** `if let Ok(view) = RecordView::new(&bytes) { ... }` drops the ref-field values of any matched parent/grandchild row whose bytes fail to decode — no error, no log, no corrupt-record counter. F-65 (#891) fixed the *read-error* half of exactly this defect class in these same files (its module doc names "storage error, and decode error" as the outcomes that must abort), but the *decode-error* half remains swallowed at the value-collection layer.
- **Failure scenario:** One corrupt parent row in a CASCADE / SET NULL / ON UPDATE fan-out: its ref-field values silently vanish from the collected set → the corresponding children are never visited → a silently-shrunk RI action set with a success result — the precise "RI violation with no error surfaced" F-65's doc describes. (The read path reports corrupt rows via `CorruptRecordRef` (F-10); these FK scans have no equivalent.)
- **Suggested fix:** On `RecordView::new` failure, fall back to `InnerValue::from_bytes` (as `record_field_matches_by_id` already tries) and only then return `Err(DbError::Codec(...))` — fail closed, mirroring `read_pre_tx_bytes`. Extend `fk_indexed_action_read_error_tests.rs`-style injection to cover a decode-corrupt matched row.

### 5. `per_table_mvcc` attach discards the error signal that indicates split-brain
- **File:** `src/repo/repo_instance.rs:399` (`create_table_context`)
- **Severity:** medium
- **Issue:** `let _ = self.per_table_mvcc.insert_sync(token, Arc::clone(&mvcc));` — `scc::HashMap::insert` returns `Err` when the token is already present, and `remove_table`'s own A13 comment (lines 496-510) states that a stale entry at attach time means "a split-brain where committed transactions silently vanish" (the commit pipeline resolves the MvccStore *by token through this map*, while the new `TableManager` reads through its own store). Discarding the `Err` hides exactly that condition instead of surfacing it.
- **Failure scenario:** `remove_table(X)` racing a `get_table(X)` init (the shared `OnceCell` is removed by `remove_table`, so a second init can run): the second `insert_sync` fails silently, the map keeps the *old* store, and all subsequent commits for the re-created table write into the detached store.
- **Suggested fix:** On `Err((_old, _new))`, `log::error!` and/or return `DbError::Internal` (fail the open) — the presence of the error is itself the invariant-violation signal A13 documents.

### 6. Background `verify` never clears its single-flight latch if the task panics
- **File:** `src/table/table_manager.rs:946-969` (`bump_write_counter`)
- **Severity:** low
- **Issue:** The spawned task clears `verify_running` only on the normal path (`self_clone.verify_running.store(false, ...)` at line 968). A panic inside `verify()` is swallowed by the un-awaited `JoinHandle`, so the CAS latch stays `true` forever — background consistency verification is permanently disabled for that table with zero signal.
- **Failure scenario:** Any panic in `verify()` (e.g. an unwrap on unexpected state during a scan) silently kills all future background verifies; inconsistencies the gauge exists to catch go unreported.
- **Suggested fix:** Wrap the body so the flag is reset in a `Drop` guard (or use a `scopeguard`-style local), or `std::panic::AssertUnwindSafe(...).catch_unwind()` with a `log::error!` on the JoinError path.

### 7. Silent best-effort operations: errors dropped without even a log line
- **File:** `src/table/table_manager.rs:703` and `src/table/doctor.rs:716` (`let _ = save_index2_metadata(...)`), `src/tx/recovery.rs:180` and `src/tx/recovery.rs:231` (`if let Ok(tbl) = repo.get_table(&name).await` in broadcast replay), `src/tx/commit_phases.rs:345-370` (`tx_gate()` failure → `ok = false` with no log)
- **Severity:** low
- **Issue:** These are legitimate best-effort choices (documented as such in comments), but unlike every sibling best-effort site in the crate (e.g. `flush_buffers`' first-error pattern, `replay_v2_op`'s warn-on-skip, the DDL op-status writers' loud `log::error!` at `table_manager_index_mgmt.rs:1129-1140`), these drop the error with no observability at all. In `recovery.rs`, a table whose `get_table` fails during broadcast `IndexPut`/`IndexDel` replay is skipped silently while the neighboring "token not found" branches warn — an operator debugging a missing posting replay gets no trace.
- **Suggested fix:** Add a `log::warn!`/`log::error!` at each site (the messages can state the accepted consequence, as the DDL sites already do).

### 8. `apply_replicated` conflates attach failure with "unattached table"
- **File:** `src/tx/apply_replicated.rs:208`
- **Severity:** low
- **Issue:** `let _ = repo.get_table(&table_name).await;` intentionally ignores NotFound ("table not configured on this follower"), but it also ignores every other error (store I/O, open-time index recovery failure). A non-NotFound failure falls through to `mvcc_found == None` → the direct `base.transact` branch — precisely the non-MVCC write path the attach exists to prevent (the R1-d divergence: replication writes invisible to subsequent MVCC reads, worked around once before with "a throwaway SELECT").
- **Failure scenario:** Follower whose `TableManager::create` fails on one index's recovery during replication apply: event data lands in `__data__` only, MVCC reads never see it, and the bookmark advances so re-delivery won't fix it.
- **Suggested fix:** Match on the error: `NotFound` → proceed to the unattached branch; anything else → propagate (the caller must not advance the watermark — the function's own idempotency contract supports retry).

### 9. Missing error-path tests for the specific gaps above
- **File:** `src/query/batch/tests/executor_tests/error_handling_tests.rs` (covers only planner-level errors: circular dependency, unknown table, id echo); no tests at the sites in findings 1-5
- **Severity:** low
- **Issue:** The crate has a strong fault-injection culture (`SHAMIR_TEST_CRASH_AFTER` seams, `TEST_READ_ONE_TX_BYTES_FAILURE`, `TEST_REDERIVE_PRE_TX_READ_FAILURE`, `FAIL_HISTORY_SEED_TX_ID`, the `test-util` feature, `p967`/`r0d`/`f73` fail-closed suites) — but none of it reaches: (a) the `delete`/`set`/`update_tx` pre-read `.ok()` conflation, (b) commit-entry lock release on `tx_gate`/`repo_wal` init failure, (c) `RecordCounter` init read/decode errors, (d) decode-corrupt rows inside FK parent-value collection. Each of those paths would fail silently today even though deterministic injection seams for adjacent sites already exist.
- **Suggested fix:** One regression test per fixed finding, reusing the existing seam conventions (a corrupt-blob fixture for the counter; the `read_one_tx_bytes` injector for the tx-path pre-read; a `get_table`-failing store double via `install_table_for_test` where needed).

### 10. Nits
- **`SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` repeated 10×** — `src/table/table_manager_index_mgmt.rs:1135, 1215, 1263, 1367, 1570, 1765, 1858, 2274, 2435, 2636`. Panics on a pre-1970 system clock; `migration/coordinator.rs:62` already uses the safe `.unwrap_or(0)` form for the identical computation. Extract one `unix_millis()` helper using the safe form.
- **`ids.lock().unwrap()` poisoning cascade** — `src/table/in_flight_create_guard.rs:100, 113, 143`. The `std::sync::Mutex` itself is the documented sanctioned DDL exception (inline justification present), and the critical sections are panic-free BTreeMap ops, so poisoning is unreachable in practice — but a panic anywhere in a future edit of these sections would cascade into `degraded_index_count()` panics. `.lock().unwrap_or_else(|p| p.into_inner())` (state is always structurally valid) or a brief comment noting the invariant would harden it.
- **`QueryParseError` / `WriteValueError` lack `thiserror`** — `src/query/common/parser.rs:17-18` (has hand-written `Display` + `impl std::error::Error`, so functionally compliant) and `src/query/batch/param_subst.rs:145-146` (`pub(super)`, stringified at the boundary). Cosmetic deviation from CLAUDE.md's "`thiserror` for library error enums"; no behavioral impact.

## Verified non-issues (checked, no finding)

- All `unwrap()`/`expect()` sites in production code outside the above are structurally guarded invariants verified at their guard site: `session.rs:196,238-254,626-628` (`table_ref()` is `Some` for exactly the matched variants, `shamir-query-types/batch_op.rs:561-573`; `len()==1` guards), `read_exec.rs:1056-1058` / `read_temporal.rs:184-186` (`use_topk` requires `order_by.is_some() && take_resolved.is_some()`), `order.rs:95,295` (permutation validity / `with_ids` alignment), `query_runner.rs:512` (loop-runs-at-least-once), `table_manager_streaming.rs:725,785` (`recording ⇒ tx.is_some()`), `tx_scan_overlay.rs:213,231` (peek-then-next), `fk_*: get_mut(field).unwrap()` (map pre-seeded with all ref fields), `interner_manager.rs:154,224` / `record_counter.rs:183` (post-`get_or_try_init`), `format.rs` regexes (constant patterns), `writer_drain_barrier.rs:444` (test code).
- `let _ =` on `scc` map ops (`drainer.rs:592,710`, `validator/registry.rs`, `repo_instance.rs:493,510,583`) are idempotent or benign-by-contract with inline justification.
- Resource lifecycles are sound and documented: drainer/group-commit/`spawn_gc_task`/watchdog-thread exits; `WriteBarrierGuard` drop-order (bit cleared before admission mutex released); `InFlightCreateGuard` refcounting; `BackgroundCommitHandle` panic → `Deferred`; `flush_buffers`/`flush_all_history` first-error pattern; crash seams use `process::abort` (no unwind) with `#[cfg(debug_assertions)]` gating.
- The remaining `#[cfg(test)]`-gated `lock().unwrap()` hooks (`pre_commit.rs:1014,1020`, `table_manager_streaming.rs:63,69`) and the `#[cfg(test)]` `futures::executor::block_on` (`table_manager.rs:869-870`) are test-only.

</details>
