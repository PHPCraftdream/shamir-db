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
