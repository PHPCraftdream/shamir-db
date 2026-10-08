<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-storage — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Normal Result failures are propagated at several seams, but worker death, unacknowledged shutdown and partial-copy policy remain open. Narrow historical fixes are supported without upgrading them into universal durability guarantees.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 13 | 8 | 3 | 0 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — CachedStore::flush can hang forever if the async write-worker task dies before draining

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Inner set/remove unwinds before pending decrement/notify; discarded JoinHandle leaves outstanding jobs and waiters stranded. Subsequent closed sends only undo their own increment. Existing FailingStore returns Err rather than panicking.

Evidence: [crates/shamir-storage/src/storage_cached.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L85); [crates/shamir-storage/src/storage_cached.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L106); [crates/shamir-storage/src/storage_cached.rs:243](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L243); [crates/shamir-storage/src/tests/storage_cached_tests.rs:984](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_cached_tests.rs#L984).

<a id="review-2"></a>

### Claim 2 — Blocking SyncSender::send executed directly on tokio executor threads

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

SyncSender::send is directly executed before async reply waiting; queue saturation blocks runtime workers, not merely submitting tasks.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L199).

Grouping/duplicate: [concurrency-lockfree.md#4](concurrency-lockfree.md#review-4). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — MemBufferStore::Drop silently discards a non-empty dirty buffer — zero observability

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Drop sets shutdown/notifies without acknowledged drain or warning. A flusher already waiting can execute one batch because shutdown is checked before select; larger dirty sets can remain unflushed.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L339); [crates/shamir-storage/src/storage_membuffer.rs:353](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L353); [crates/shamir-storage/src/storage_membuffer.rs:621](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L621).

<a id="review-4"></a>

### Claim 4 — Missing error-path tests; audit-§2.2 telemetry is written but never read

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

flush_errors is only initialized/incremented. Tests do not force MemBuffer backing failures, Cached closed sends, Fjall submit failures or partial copy; ordinary Cached background Err tests are distinct existing coverage.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:192](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L192); [crates/shamir-storage/src/storage_membuffer.rs:355](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L355); [crates/shamir-storage/src/tests/storage_cached_tests.rs:1031](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_cached_tests.rs#L1031).

<a id="review-5"></a>

### Claim 5 — Cache eviction/deletion committed before the fallible backing op is acknowledged

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Delay backing remove, read the evicted key and refill old V, then complete remove: cache still serves V after successful deletion/flush. A failure is not required. Failed jobs are logged and surfaced by flush.

Evidence: [crates/shamir-storage/src/storage_cached.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L487); [crates/shamir-storage/src/storage_cached.rs:476](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L476); [crates/shamir-storage/src/storage_cached.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L92).

<a id="review-6"></a>

### Claim 6 — Repo::copy_store default impl leaves a partially-populated destination on failure

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Errors after previous set_many batches preserve partial destination state; retry overwrites matching keys but does not remove extras. Caller-owned pre-existing destinations preclude blanket deletion cleanup.

Evidence: [crates/shamir-storage/src/types.rs:495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L495); [crates/shamir-storage/src/types.rs:500](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L500); [crates/shamir-engine/src/repo/repo_instance.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L593).

<a id="review-7"></a>

### Claim 7 — FjallRepo::store_get returns a fresh FjallStore per call — fragile per-instance worker lifecycle

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Per-handle lazy creation/join cost exists conditionally. Exact fjall 3.1.6 sets is_deleted and rejects subsequent point insert/remove with KeyspaceDeleted; read/batch lifecycle should not be generalized from that narrower result.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L240); [crates/shamir-storage/src/storage_fjall.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L134); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Error-source chains flattened; thread-spawn failure panics instead of DbResult

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Codec/backend errors lose source chains; spawn expect can panic on environmental resource failure. Flattening is diagnostic preference, whereas fallible thread startup violates the stated Result discipline.

Evidence: [crates/shamir-storage/src/error.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L94); [crates/shamir-storage/src/storage_fjall.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L98).

<a id="review-nf-panic-surface"></a>

### Claim NF-panic-surface — Every production unwrap/expect is genuinely unreachable with inline justification

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Thread creation is fallible. Several historical citations are ignored inserts, not unwraps, and insertion-after-remove is not guaranteed under concurrent writers. Backend invalid-name assertions add a captured panic path.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L98); [crates/shamir-storage/src/storage_cached.rs:409](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L409); [crates/shamir-storage/src/storage_fjall.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L234).

<a id="review-nf-dirty-cleanup"></a>

### Claim NF-dirty-cleanup — drain_once retains dirty on error and guarded cleanup preserves differing concurrent writes

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Backing ? propagation precedes removal, and value-matching remove_if preserves differing concurrent dirty values. The injected regression catches reverting that removal guard; it does not prove outstanding drain-write ordering.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:527](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L527); [crates/shamir-storage/src/storage_membuffer.rs:554](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L554); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:799](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L799).

<a id="review-nf-mirror-first"></a>

### Claim NF-mirror-first — MirroredStore mirror-first ordering delivers honest error atomicity

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

All fallible mirror calls precede primary changes; injected Err tests observe seeded primary values or both absent subsets. Mocks fail before applying anything, so they cannot prove the actual mirror's rollback/error detection.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:351](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L351); [crates/shamir-storage/src/storage_mirrored.rs:595](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L595); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:608](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L608).

<a id="review-nf-cached-flush"></a>

### Claim NF-cached-flush — Cached flush always attempts inner.flush and consumes background errors once

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

After pending settles, inner.flush is attempted for either background result, and swap(None) consumes the latest error. Repeat-flush and marker assertions discriminate the prior early-return bug. Cancellation or worker death is outside this guarantee.

Evidence: [crates/shamir-storage/src/storage_cached.rs:346](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L346); [crates/shamir-storage/src/storage_cached.rs:397](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L397); [crates/shamir-storage/src/tests/storage_cached_tests.rs:1170](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_cached_tests.rs#L1170).

<a id="review-nf-notify"></a>

### Claim NF-notify — Notify future created before the pending check is race-free with notify_waiters

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Exact Tokio 1.49.0 documentation promises notify_waiters delivery from Notified creation, even unpolled; the source uses that operation rather than notify_one. Worker survival is separate.

Evidence: [crates/shamir-storage/src/storage_cached.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L109); [crates/shamir-storage/src/storage_cached.rs:385](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L385); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

## Evidence and recipe corrections

- Ordinary allocation failure is not established as an unwinding worker panic; a custom inner panic supplies the supported liveness witness.
- A per-job Drop guard and periodic counter recheck do not settle a dead worker's remaining queue. Maintain an explicit terminal signal and preserve queued-write failure accounting.
- Native fjall 3.1.6 WriteBatch discards write_batch errors; its default later Buffer persist detects persistent failures but need not detect an earlier transient partial-batch failure. Mirror-first cannot repair an erroneously successful mirror result.
- The historical fixed mechanisms predate the original August report. No new implementation fix occurred during this review.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage -- Error handling & resource lifecycle

## Summary

The crate's error discipline is broadly strong and clearly battle-tested: every fallible op returns `DbResult`, backend errors are mapped into `DbError` variants rather than unwrapped, panics in production code are confined to commented invariant violations, and the test suite covers several genuine error paths (injected mirror-write failures, background-flush error surfacing via `#1082`, the audit 2.3 lost-concurrent-write race). The remaining weaknesses are concentrated in resource lifecycle rather than Result plumbing: one liveness hazard where `CachedStore::flush()` can park forever if the spawned async write-worker dies before draining its queue, blocking `SyncSender::send` calls executed directly on tokio executor threads in `FjallStore`, a silently-discarded dirty buffer on `MemBufferStore` drop that contradicts the crate's own audit-§2.2 observability stance, and a set of error branches (background drain failure telemetry, worker-channel-closed fallbacks, `copy_store` partial failure) that have no test coverage — some are also unreadable dead code.

## Findings

### 1. `CachedStore::flush()` can hang forever if the async write-worker task dies before draining
- **File:line:** `crates/shamir-storage/src/storage_cached.rs:68-113` (worker loop), `:243-249` (`tokio::spawn`, handle discarded), `:383-399` (`wait_for_async_writes`)
- **Severity:** high
- **Issue:** In `WriteMode::Async`, `CachedStore` increments `pending_writes` per enqueued job and relies exclusively on the worker task to (a) `fetch_sub` after each job completes and (b) call `notify.notify_waiters()`. The task is launched with a discarded `JoinHandle`; if any `inner.set/remove` (an `Arc<dyn Store>` this crate does not control — it can be any wrapper, a foreign impl in tests/tooling, or scc/moka hitting an allocation panic) panics inside the worker task, or the runtime drops the task at shutdown mid-queue, the decrement + notify for every queued job never happens. `wait_for_async_writes` then loops: `pending_writes != 0` forever, and since `notify_waiters()` will never fire again, every subsequent `flush()` parks indefinitely. A durability-path deadlock of unbounded length; under house rules ("hangs are bugs") this is a defect even though the trigger requires an inner panic/cancellation.
- **Failure scenario:** one failing/panicking inner write during a bulk load → all later `flush()` calls (the graceful-shutdown flush included) stall permanently instead of returning `Err`.
- **Suggested fix:** make the decrement+notify panic-safe — wrap each job iteration so decrement/notification run on unwind (a Drop guard over `pending_writes`), and/or await the worker's `JoinHandle` alongside (abort-on-death → surface `Err(DbError::Internal("async write worker died"))` from `flush()`), optionally add a bounded recheck with a plain atomic loop as a backstop.

### 2. Blocking `SyncSender::send` executed directly on tokio executor threads
- **File:line:** `crates/shamir-storage/src/storage_fjall.rs:92-93` (`sync_channel(1024)`), `:194-208` (`submit`), call sites `:330-334`, `:496-500`
- **Severity:** medium
- **Issue:** `FjallStore::insert` / `FjallStore::transact` call `submit()`, which performs `tx.send(job)` on a bounded `std::sync::mpsc` channel *inside* the `async fn`. When the queue is full (>1024 in-flight inserts/transacts against a slow disk), the sending task blocks its tokio worker thread synchronously. This violates pillar 2 ("CPU-bound/blocking work crosses to `spawn_blocking`"); the in-code comment acknowledges the parking as intended backpressure but not the executor-thread cost.
- **Failure scenario:** a large batch fan-out (e.g. `insert_many` storm through the commit path while the worker thread is I/O-bound) parks N concurrent submitters on N runtime worker threads; multi-thread runtimes with few workers starve unrelated ready tasks → throughput collapse and SLOW/TIMEOUT-class symptoms under load.
- **Suggested fix:** route the send through `tokio::task::spawn_blocking`, use `try_send` + yield/park loop, or replace the std channel with a `tokio::sync::mpsc::channel(1024)` whose async `send` parks only the logical task.

### 3. `MemBufferStore::Drop` silently discards a non-empty dirty buffer — zero observability
- **File:line:** `crates/shamir-storage/src/storage_membuffer.rs:621-626` (`Drop`), dirty-buffer contract `:49-52` (module doc)
- **Severity:** medium
- **Issue:** Dropping the store sets `shutdown` and wakes the flusher, which exits *before* draining; whatever is still in `dirty` (values not yet applied to `inner`) is dropped without any log, count check, or accessor. The crate itself established in audit §2.2 (`:348-360`) that buffered writes dying silently is unacceptable ("dirty grows unboundedly with zero signal") and added a counter + log for the flusher case — but the drop path loses the same data class with *less* signal than the bug §2.2 fixed. A `Drop` cannot `.await`, but observing the loss costs nothing.
- **Failure scenario:** a consumer recreates/replaces a MemBuffer-wrapped store outside `apply_config`'s drain-first path (the only documented safe path); all ACKed-but-unflushed writes vanish while `inner` keeps stale values — undiagnosable afterwards.
- **Suggested fix:** in `Drop`, when `dirty_count > 0`, emit a `log::warn!` naming the store and entry count (and/or expose `dirty_count()` for callers/tests to assert orderly shutdown); document explicitly that drop-with-dirty = data loss by contract.

### 4. Missing error-path tests; audit-§2.2 telemetry is written but never read
- **File:line:** `storage_membuffer.rs:192,355` (`flush_errors` — no reader anywhere, not even a `#[cfg(test)]` accessor); `storage_cached.rs:446-462,499-510` (worker-channel-closed fallbacks); `storage_fjall.rs:199-207` (both `DbError::Internal` mappings in `submit`); `types.rs:488-503` (`Repo::copy_store` default partial-failure)
- **Severity:** medium
- **Issue:** No test constructs any of these states:
  - MemBufferStore background-drain failure: the §2.2 behavior (counter increment, error log, dirty retained + retried next tick) is completely uncovered, and `flush_errors` has no accessor, so the counter cannot ever be observed — dead telemetry, unverifiable claim.
  - CachedStore `set`/`remove` send-failure branch ("worker gone, write dropped": pending-count undo + loud log).
  - FjallStore `submit` error shapes (`Internal("write worker channel closed"/"dropped reply")`) mapped to match the old `spawn_blocking` semantics.
  - `Repo::copy_store`: nothing tests what state remains when `src.iter_stream` or `dst.set_many` fails mid-copy (relevant to RENAME TABLE — see finding 6).
- **Failure scenario:** regressions in exactly these branches (e.g. removing the pending-count undo, changing the retry discipline, breaking retained-dirty-on-error) land silently green.
- **Suggested fix:** failing-inner wrappers (already idiomatic in this suite: `FailingStore`, `FailingTransactMirror`) cover the first three cheaply; add a `#[cfg(test)] dirty_error_count()` accessor (and a failing-backend test asserting `flush_errors` bumps and dirty survives a failed drain).

### 5. Cache eviction/deletion committed before the fallible backing op is acknowledged
- **File:line:** `storage_cached.rs:487-515` (`remove` evicts cache before `inner.remove` resolves — both modes), `:427-467` (`set` Async branch populates cache before enqueue result known)
- **Severity:** low
- **Issue:** On `Err` from the backing store the cache mutation is already durably applied locally. Sync mode self-heals (next `get()` read-through re-caches what `inner` still holds), but Async remove is worse: after the one-shot flush error (#1082 semantics), the key cache-misses into `inner`, which still holds the old value — the deleted key silently resurrects on later reads with no further signal, and reload/hydration makes it permanent.
- **Failure scenario:** backing store outage during Async-mode deletes → caller sees one `Err` from `flush()`, then reads resurrect every tombstoned key with no diagnostic.
- **Suggested fix:** hold a sticky negative marker (or re-tombstone on read-through hit of a failed-remove key) until the removal is confirmed, or at minimum log on the resurrection path; document the divergence window in the module doc.

### 6. `Repo::copy_store` default impl leaves a partially-populated destination on failure
- **File:line:** `types.rs:488-503`
- **Severity:** low
- **Issue:** Copy-then-orphan rename streams batches into `dst.set_many` with no compensating cleanup: a mid-stream error returns `Err` leaving a half-copied destination store that persists on disk and appears in `stores_list` forever. Retry convergence relies on overwrite-by-key idempotency, which breaks if source rows were removed between attempts (stale extras survive in dst). None of this is documented on the method.
- **Failure scenario:** RENAME TABLE fails partway → phantom `__data__<t>`-shaped store accumulates; a successful later copy over different src content merges stale keys.
- **Suggested fix:** either best-effort `store_delete(to)` on the error path (documented, orphan disposition matches DROP TABLE) or spell out the convergence/idempotency contract callers must honor.

### 7. `FjallRepo::store_get` returns a fresh `FjallStore` per call — fragile per-instance worker lifecycle
- **File:line:** `storage_fjall.rs:229-245` (new instance each call), `:289-309,312-320` (`OnceLock<WriteWorker>` lazy spawn)
- **Severity:** low
- **Issue:** Unlike `InMemoryRepo` (which caches `Arc<dyn Store>` per name in a `TDashMap`), every `store_get` builds a new `FjallStore` with its own fresh `OnceLock`. The design leans entirely on the documented convention that short-lived instances (e.g. the `__tx__` marker store fetched per commit) must never issue `insert`/`transact` — otherwise each call spawns an OS-thread write worker just to abandon it (spawn+join churn per transaction). Also, outstanding `FjallStore` handles keep operating on a keyspace after a concurrent `store_delete` removes it (backend-dependent errors). Correct today, but guarded only by prose.
- **Failure scenario:** a future commit path submits one `insert` through the per-commit marker store → silent OS-thread create/join storm on the hot path, invisible until bench/flamegraph.
- **Suggested fix:** name-keyed `Arc` cache like `InMemoryRepo` (or a debug_assert/log if `WriteWorker::spawn` fires more than once per (db,name) pair within a window).

### 8. Error-source chains flattened; thread-spawn failure panics instead of `DbResult`
- **File:line:** `error.rs:92-96` (`From<CodecError>` → `err.to_string()`); most variants carry `String` rather than a typed source; `storage_fjall.rs:95-98` (`.expect("spawn fjall write worker thread")`)
- **Severity:** nit
- **Issue:** CLAUDE.md asks for `thiserror` with `#[from]` where natural; `std::io::Error` gets it, but `CodecError` (and fjall/DB errors generally) degrade to display strings, losing `source()` chains for diagnostics. Thread-spawn failure in `WriteWorker::spawn` panics the calling async context via `OnceLock::get_or_init` (can't propagate an error through it); defensible as near-fatal, but inconsistent with the crate's otherwise strict no-panic surface.
- **Suggested fix:** consider `#[source]`/typed variants where ergonomic (esp. `Codec`); make `WriteWorker` spawn return `Option<WriteWorker>` handled as `DbError::Storage`/`Internal` if a hard dependency on graceful degradation matters.

## Verified non-findings (checked, clean)

- Panic surface: every production `.unwrap()`/`.expect()` site (`storage_in_memory.rs:130,227`, `storage_fjall.rs:98,112,123,126,145`, `storage_membuffer.rs:688,704`, `storage_cached.rs:160,409,422,449,502`) is a genuinely unreachable state with an inline justification — consistent with the house rule.
- `MemBufferStore::drain_once` retains dirty entries on error (retryable), and the §2.3 `remove_if` guard correctly protects concurrent writes across `transact`/drain windows — covered by deterministic regression tests.
- `MirroredStore` mirror-first ordering delivers honest error atomicity (primary untouched on mirror failure), and it is thoroughly tested including injected-failure paths and log assertions.
- `CachedStore::flush` runs `inner.flush()` unconditionally even when background writes failed, and surfaces background failures exactly once — both regression-guarded (#1082 / @oh review tests).
- The `Notify` before-check pattern in `wait_for_async_writes` follows tokio's documented race-free shape (given the worker stays alive — see finding 1).

</details>
