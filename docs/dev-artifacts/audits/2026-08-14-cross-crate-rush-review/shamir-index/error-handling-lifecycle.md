# shamir-index -- Error handling & resource lifecycle

## Summary

The crate's DDL error paths are unusually disciplined — enriched multi-phase error messages, durable tombstones with rollback-on-persist-failure (`add_to_dropping*`), RAII drain guards that clean up on panic, and a documented fail-closed corruption policy (F-83/#911, P0-4/#960). However, that discipline does not extend uniformly: one open-path store error is swallowed entirely (sorted-index definitions), the index2 `drop_all` sweeps and the vector compaction double-writes discard errors with no logging, and `try_fit_and_rebuild` failures are silently dropped behind comments that falsely claim they are logged. `IndexError` remains stringly-typed, losing the structured `DbError` it wraps, and the crate's own enriched error paths are unit-tested only for the regular-hash family.

## Findings

### 1. `SortedIndexManager::load` swallows ALL store errors, silently loading zero sorted definitions
- **File:** `crates/shamir-index/src/base_index/sorted_index_manager.rs:2706-2709`
- **Severity:** high
- **Issue:** The initial metadata read is `match self.info_store.get(sys_id...) { Ok(b) => b, Err(_) => return Ok(()) }`. The blanket `Err(_)` arm treats *every* store failure — IO errors, backend faults, corruption-class errors — identically to `NotFound`, i.e. "no sorted indexes exist". This directly contradicts the crate's own fail-closed policy: the sibling `IndexManager::new` (index_manager.rs:501-549) matches `DbError::NotFound` explicitly and propagates everything else, with a comment (F-83/#911) that even *claims* to "mirror `sorted_index_manager::load`'s corruption→`DbError::Codec` propagation" — that mirror covers only the *decode* path (which does propagate), not this store-*get* path.
- **Failure scenario:** A transient IO/backend error while opening a table causes the manager to load an empty definition set. The table opens "successfully" with all sorted indexes gone from planning; the next `persist_defs()` (any DDL on the table) writes the empty Vec back, permanently destroying every sorted-index definition while their postings remain as orphans.
- **Suggested fix:** Match `Err(DbError::NotFound(_)) => return Ok(())` and propagate all other errors with `Err(e)`, mirroring `IndexManager::new`. Add a test injecting a failing store `get` (the `FaultyStore` pattern from `p12_ddl_partial_error_tests.rs`) proving the open aborts instead of returning an empty manager.

### 2. Compaction double-write errors silently discarded; an incomplete graph is then swapped in as live
- **File:** `crates/shamir-index/src/vector/vector_backend.rs:296, 317, 354, 383, 512, 548` (swallows); `vector_backend.rs:1101-1104` (unconditional swap)
- **Severity:** high
- **Issue:** Every write mirrored to the compaction target discards its result: `let _ = target.adapter.upsert(rid, &v).await;` (and the `delete` / `apply_committed_vectors` twins). `run_background_compaction` never learns a double-write failed: Step 5 unconditionally `adapter_swap.store(...)`s the target in as the primary graph and Step 7 snapshots it. A failed target write means the post-compaction live graph is missing vectors/deletes — silently wrong ANN results — with **zero** log, error, or counter anywhere (this crate's own DDL paths follow a "do not swallow — log loudly" rule, e.g. index_manager.rs:2359-2372).
- **Failure scenario:** `target.adapter.upsert` returns `VectorError::Internal` (e.g. "f32 graph absent" class error, or a `spawn_blocking` join error) during the compaction window; the flip proceeds; the record's vector vanishes from similarity results until the next full rebuild — no signal at any layer.
- **Suggested fix:** On a double-write error, set a shared `AtomicBool double_write_failed`; log loudly; have `run_background_compaction` check it before Step 5 and abort the flip (compaction flag already clears via `CompactionFlightGuard`, so a retry happens on the next threshold crossing). At minimum, `log::warn!` every discarded error.

### 3. index2 `drop_all` sweeps swallow per-key errors, and `VectorBackend::drop_all` leaks the entire `__vec_snap__` keyspace
- **File:** `crates/shamir-index/src/fts_backend.rs:248`, `fts_ranked_backend.rs:409`, `functional_backend.rs:298`, `vector/vector_backend.rs:739-743`; stale note at `persistence.rs:527-529`
- **Severity:** medium
- **Issue:** All three storage-backed index2 backends sweep postings with `let _ = self.store.remove(key_bytes).await;` inside `drop_all`, then return `Ok(())`. Per lifecycle.rs and the R0-D contract, `drop_all` errors are supposed to be meaningful (caller leaves the tombstone / marks the backend `Failed`); the swallow makes those error paths dead code for these backends — a fully-failed sweep is reported as success, the DROP tombstone is cleared, and orphan postings remain with no signal. Separately, `VectorBackend::drop_all` is a documented no-op "the HNSW graph lives in memory and dies with the process" — but since V2.1/V2.3 the graph IS persisted: snapshot chunks, sidecars, manifests and delta chunks under `__vec_snap__<id>` (vector_backend.rs:37). Nothing ever removes that keyspace on DROP (`sweep_index2_postings_by_id` scans the 4-byte LE id prefix, which cannot match the ASCII keyspace; grep-verified no other sweep exists), so every dropped vector index permanently leaks its full snapshot + delta log; the persistence.rs comment justifying the no-op predates the snapshot feature and is stale.
- **Suggested fix:** Replace the per-key `remove` loops with collect + `remove_many(...).await?` (exactly like `IndexManager::sweep_index_postings`, index_manager.rs:1243-1264) so failures propagate; implement `VectorBackend::drop_all` to scan-and-remove the `__vec_snap__<id>.` prefix; update the stale comment.

### 4. `try_fit_and_rebuild` failures silently dropped behind comments that falsely claim logging
- **File:** `crates/shamir-index/src/vector/hnsw_adapter.rs:943-947, 2470-2475, 2709-2713`
- **Severity:** medium
- **Issue:** All three trigger sites do `let _ = self.try_fit_and_rebuild().await;`. The inline comments state "Best-effort: a fit failure is logged inside `try_fit_and_rebuild` and does not fail the upsert" — but `try_fit_and_rebuild` contains **no logging at all** (grep for `log::` in hnsw_adapter.rs returns nothing); it returns `Err(VectorError::Internal(...))` which is then discarded. A failed fit (e.g. `spawn_blocking` join error, graph-build failure) leaves the adapter permanently on the f32 path — 4x memory and slower search — with no log, counter, or surfaced error, and the comments document observability that does not exist.
- **Failure scenario:** A one-off `spawn_blocking` panic during fit at a production deployment: SQ8 never activates for that adapter, memory stays 4x plan, and nothing in the logs explains why.
- **Suggested fix:** `if let Err(e) = self.try_fit_and_rebuild().await { log::warn!("SQ8 fit failed, staying on f32 path: {e}"); }` at each site (or log inside the function and keep the `let _`), and correct the comments.

### 5. `build_index2_backend` panics via `unreachable!` on a persisted (disk-driven) descriptor kind
- **File:** `crates/shamir-index/src/build_backend.rs:66-68`
- **Severity:** medium
- **Issue:** `IndexKind::Btree { .. } => unreachable!("Btree indexes are handled by the base_index index manager")`. The `desc` here comes from `load_index2_metadata` — i.e. from the on-disk `__meta__/indexes` blob — and nothing on the load path validates the kind. The function returns `Arc<dyn IndexBackend>` (not `Result`), so there is no way to fail closed. CLAUDE.md restricts panics to invariant violations meaning a *programmer* bug; a corrupted or future-versioned metadata blob containing a Btree kind is *data*, and the panic fires on the table-open path (crash-at-boot / table permanently unopenable), contradicting the fail-closed corruption policy applied everywhere else (F-83, P0-4).
- **Failure scenario:** Bit-flip or version-skew in the persisted descriptor blob yields a Btree kind → `TableManager::create` → panic at open, repeatedly, until manual repair.
- **Suggested fix:** Return `Result<Arc<dyn IndexBackend>, IndexError>` and map the arm to `IndexError::TypeMismatch` (fail this backend closed like `restore_on_open` errors), or filter/validate kinds in `decode_persisted_indexes`.

### 6. `IndexError` is stringly-typed; structured `DbError`s are flattened at every boundary
- **File:** `crates/shamir-index/src/backend.rs:56-66`; conversion sites at `write_ops.rs:93-97, 153-158`, `fts_backend.rs:102, 246`, `fts_ranked_backend.rs:121, 386, 407`, `functional_backend.rs:124`, `vector/vector_backend.rs:572`
- **Severity:** medium
- **Issue:** `IndexError::Storage(String)` (and `Backend(String)`) receive `e.to_string()` of an underlying `DbError` everywhere, destroying the variant information. CLAUDE.md's error-handling rule says "thiserror for library error enums (with `#[from]` where natural)" — there is no `#[from] DbError` and no way for a caller to distinguish NotFound / Io / `IndexDrainInProgress` / Corruption through the `IndexBackend` trait, all of which the same crate distinguishes carefully on its `DbResult`-returning APIs. Relatedly, `save_index2_metadata_with_pending` (persistence.rs:102-105) re-wraps the already-typed `DbError` from `Store::set` into `DbError::Internal(String)` — losing structure the sibling `save_legacy_index_version` and every tombstone writer propagate untouched (`?`).
- **Suggested fix:** `Storage(#[from] shamir_storage::error::DbError)` (or a `Storage { source: DbError, ctx: String }` variant) and use `?`/`#[from]` at the conversion sites; drop the `map_err` re-wrap in `save_index2_metadata_with_pending`.

### 7. Enriched error paths are unit-tested only for the regular-hash family
- **File:** `crates/shamir-index/src/base_index/tests/p12_ddl_partial_error_tests.rs` (only file with fault-injected error-path tests); untested paths in `index_manager_unique.rs:744-758, 839-875, 921-936`, `sorted_index_manager.rs:1003-1110, 1509-1605`, and the index2 backends
- **Severity:** medium
- **Issue:** The P1-2/#967 fault-injection suite (`FaultyStore`) covers exactly three scenarios: regular-hash CREATE Phase-2 failure, Phase-3 failure, and DROP sweep failure. The same enriched messages for UNIQUE CREATE persist failure, UNIQUE DROP, SORTED DROP sweep/persist/tombstone-clear failures, SORTED RENAME definition-swap/rekey failures, and the index2 `drop_all` sweep are asserted nowhere in this crate (grep-verified: their distinctive strings appear in no test file; the engine crate doesn't test them either). `lifecycle.rs` (lines 249-261) openly defers the full crash/cancellation matrix, but that deferral covers crash-window *state* tests — the cheaper "error text + partial state correct on injected failure" tests for the non-hash families are simply missing. Finding 1's swallow path (store-`get` error at sorted open) is likewise untested.
- **Suggested fix:** Extend `FaultyStore`-style injection to `create_unique_index_from_records`, `drop_unique_index`, `SortedIndexManager::drop_index`/`rename_index_sorted`, and one index2 backend `drop_all` (after fixing finding 3 so there is an error to observe); each asserting the enriched message and the post-failure durable state.

### 8. `.unwrap()` on `SystemTime::duration_since(UNIX_EPOCH)` on four DDL success paths
- **File:** `index_manager.rs:2353-2355`, `index_manager_unique.rs:891-893`, `sorted_index_manager.rs:1070-1072, 1565-1567`
- **Severity:** low
- **Issue:** `completed_at` computation panics if the system clock is before the Unix epoch (CMOS reset, VM clock anomaly). The crate's own sibling code (descriptor.rs:48-51, meta_envelope.rs:38-41) uses `.unwrap_or(0)` for the exact same call — the four DDL sites are inconsistent with it.
- **Suggested fix:** `.map(|d| d.as_millis() as u64).unwrap_or(0)` at all four sites.

### 9. Read-hot-path `.expect` panics for the "quantized_active but unset" invariant, while sibling sites return errors
- **File:** `vector/hnsw_adapter.rs:1791, 1848, 1851, 1920, 1923, 2034, 2558, 2594`
- **Severity:** low
- **Issue:** The quantized search cores use `.expect("quantized_active but quantizer unset")` / `"...u8 graph unset"`, taking down the whole process on an invariant break. The mirror-image f32-path sites handle the same class of invariant break by returning `VectorError::Internal` ("NOT panic — upsert must propagate failure cleanly", hnsw_adapter.rs:2374-2378) or a defensive `Ok(vec![])` (search, 2913-2917). The Acquire-load memory-ordering argument for why the expects can't fire is documented, but if that argument is ever invalidated by a refactor, the failure mode on the query path is a server-killing panic rather than an `IndexError`.
- **Suggested fix:** Convert the expects to `ok_or_else(|| VectorError::Internal(...))` for uniform failure behavior (they are not measurably hot relative to the graph traversal that follows).

### 10. Silent degradation fallbacks with no logging
- **File:** `fts_ranked_backend.rs:125-130` (corrupt posting value → default `{tf:1, doc_len:1}`), `fts_ranked_backend.rs:388-391` (undecodable record skipped in `rebuild`), `sorted_index_manager.rs:2861-2868` (`rmp_serde` failure → empty covering projection), `bm25.rs:87-92` (`fetch_sub` underflow wraps `doc_count` to u64::MAX)
- **Severity:** low
- **Issue:** These four paths choose a safe-but-degraded fallback with no observability: corrupt FTS posting values silently score as tf=1/doc_len=1; records that fail `InnerValue::from_bytes` are skipped during BM25 stats rebuild (stats drift from postings); a msgpack encode failure silently empties a covering projection (per-row full-fetch fallback); and `FtsStats::on_delete` wraps on underflow, turning `avg_doc_len` into garbage (a double-applied `BumpFtsStats{sign:-1}` — the exact accident provenance tracking was built to prevent — corrupts all subsequent BM25 scores with no signal). Given the project's "checksums everywhere / log loudly" stance, these should at least log; the underflow should saturate or count.
- **Suggested fix:** `log::warn!` (rate-limited if hot) at each fallback; use `fetch_update` with `saturating_sub` semantics or a poison flag for `FtsStats`.

### 11. `QuantMeta::to_quantizer` panics on a checksum-less, corruptible sidecar at table open
- **File:** `vector/quant_meta.rs:67-74`
- **Severity:** low
- **Issue:** `assert_eq!(self.mins.len(), self.dim)` etc. fire during `load_snapshot` if the sidecar's quantization blob is internally inconsistent. The sidecar is MetaEnvelope-wrapped but carries **no crc of its own** (snapshot.rs:694-697 says so explicitly), so a bit-flip that survives envelope magic/version checks and bincode decode reaches these asserts — a data-driven panic on the open path, contradicting the fail-closed policy that makes every other snapshot corruption surface as `SnapshotError::Corrupt` (crash_recovery_tests prove the no-panic contract for chunks/manifests).
- **Suggested fix:** Return `Result<Sq8Quantizer, SnapshotError>` (or validate lengths in `load_snapshot` before calling) mapping inconsistency to `SnapshotError::Corrupt`, preserving the warn + full-rebuild fallback.

### 12. `IndexRegistry::insert` can still return `Err` leaving `by_id` populated (the exact partial publish #1009 closed via pre-check)
- **File:** `crates/shamir-index/src/registry.rs:288-307`
- **Severity:** nit
- **Issue:** The #1009 fix pre-checks `by_name.contains_async` so the reachable name-collision path never touches `by_id` (tested). But the later `by_name.insert_async(...).map_err(...)?` arm still returns `Err` without rolling back the already-published `by_id` entry — the doc's own description of the pre-#1009 bug. Under the documented `ddl_admission` serialization this is unreachable (no concurrent same-name insert exists), but the error path itself is not self-cleaning.
- **Suggested fix:** On that `Err`, `let _ = self.by_id.remove_async(&id).await;` before returning, or a `debug_assert` documenting the admission precondition.

### 13. Actor `shutdown` discards the join result (panic payload), and `BruteForceAdapter::shutdown` adds a lock-poisoning expect
- **File:** `actor.rs:100-105`; `vector/brute_force.rs:129-135`
- **Severity:** nit
- **Issue:** `let _ = join.await;` drops the `JoinError`, so an applier task that panicked mid-op is indistinguishable from a clean drain — a silently-dead actor is only detectable later via `submit`'s `SendError`. `BruteForceAdapter::shutdown` additionally uses `.expect("brute-force join lock")` on a `std::sync::Mutex`, converting poisoning into a panic.
- **Suggested fix:** Log on `JoinError::is_panic` in both shutdowns; use `lock().unwrap_or_else(|p| p.into_inner())` for the join-handle mutex (the Option inside is still valid under poisoning).
