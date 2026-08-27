# shamir-tx -- Error handling & resource lifecycle

## Summary

Error-path discipline in this crate is genuinely strong in places — the `VersionGuard`/`CellReservationGuard`/`SnapshotGuard` RAII design makes abort-marking hold "by construction", and fault-injection tests exist for the single-key non-tx write paths. However, the bump-first write paths (`set_versioned`/`set_versioned_many`/`delete_versioned`) advance the in-memory `RecordCell` *before* the durable `history.transact` and perform **no compensating rollback when that transact fails**, which masks the key's prior version on every subsequent point read. Beyond that, `thiserror` is declared in `Cargo.toml` but never used (six APIs return `Result<_, String>`, including the public `ChangelogStore` trait), several GC/recovery paths silently swallow storage errors in ways inconsistent with their siblings, the changefeed's gap-detection invariant has an undetected hole on journal persist failures, and error-path test coverage stops at two functions on fresh keys. Note: `RepoTxGate::pending_commits.lock().unwrap()` (repo_tx_gate.rs:753/761) is **not** flagged — CLAUDE.md explicitly sanctions it as dead scaffolding with zero live callers.

## Findings

### 1. Failed `history.transact` leaves `RecordCell` advanced with no rollback — prior version permanently masked on point reads
**File:** `crates/shamir-tx/src/mvcc_store/mod.rs:766-832` (`set_versioned`: `publish_cell` at :785, `?` at :799); same shape at `:853-938` (`set_versioned_many`, publish loop :885, `?` :898) and `:1035-1086` (`delete_versioned`, publish :1051, `?` :1063)
**Severity:** high

**Issue:** All three non-tx write paths execute `publish_cell(key, new_v)` *before* the single durable `history.transact(...)` (deliberate — the MVCC-2 snapshot invariant requires publish-before-log). On transact failure the `?` propagates immediately; the `VersionGuard` drop correctly marks the version `Aborted` and advances the watermark, but the cell map is left at `new_v` with **no compensating restore** (verified: no rollback/revert/compensate exists anywhere in `mvcc_store/`). `publish_cell`'s A2 max-monotonic guard means nothing short of a successful rewrite of the key ever corrects it, and `prune_version_cache` only evicts cells with `version < min_alive` — a cell at the aborted (now watermark-past) version is never evicted.

**Failure scenario:**
- *Failed SET:* key `k` has committed version 5. `set_versioned(k, v6)` hits a disk-full/IO error in `transact`. Caller gets `Err`. Every later `get_current`/`get_current_bytes` on `k`: `cur_v = 6`, floor ≥ 6, so no `get_at` fallback; overlay miss; `history.get(k::6)` → `NotFound` → **`Ok(None)` — the record reads as deleted** even though version 5 is intact in the log. Meanwhile `current_stream` (which group-bys the *log*, not the cell) still emits it — point read and stream read disagree indefinitely.
- *Failed DELETE:* `delete_versioned(k)` bumps the cell to the tombstone version but the tombstone never lands. Point reads return `None` — **the delete appears to have succeeded** in-process, then the record resurrects after restart (recovery reads the log). A durability illusion.

The doc comments say "cancel-safe: NO … caller must retry or WAL-replay to converge", but this is the *error* path, not cancellation: non-tx writes have no WAL entry to replay, and a caller that surfaces the `Err` (the normal engine behavior) never retries — the divergence is permanent until the same key is rewritten.

**Suggested fix:** On `transact` failure, restore the cell before propagating: capture `old_v` per key (already captured for vacuum on `set_versioned`/`delete_versioned`), then on the error branch run an explicit `restore_cell(key, old_v)` that unconditionally sets `cell.version = old_v` (a plain `entry_sync` write — *not* `publish_cell`, whose max-monotonic guard would refuse the regression). For `set_versioned_many`, roll back all batch keys to their captured `old_versions`. Add a test with a pre-existing key (see Finding 5). Alternatively, defer the `publish_cell` loop to after transact success on the batch paths (the publish is already synchronous there), keeping the pre-log publish only where the MVCC-2 invariant demonstrably requires it.

### 2. `vacuum_key` scan path silently swallows prefix-scan stream errors — inconsistent with its siblings
**File:** `crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:173-181` (`batch.unwrap_or_default()` at :174)
**Severity:** medium

**Issue:** The retention-aware vacuum's phase-1 scan treats every stream error as an empty batch (`for (phys_key, _val) in batch.unwrap_or_default()`). The two sibling GC paths — `gc_below` (:312) and `purge_below_ts` (:408) — both propagate with `batch?`. Analysis shows the truncation errs toward over-retention rather than data loss (deletions only target *collected* entries, and a shorter list lowers each entry's reclaim rank), so this is not a correctness hole — but under a persistent read error, vacuum silently stops reclaiming anything with **zero log output**, and the history log grows unboundedly while `gc_below` on the same store correctly reports errors. The deletions themselves being best-effort (`let _ = remove_no_flag`, documented) is fine; the *decision-input* scan being silently lossy is not.

**Failure scenario:** A store backend returns intermittent `DbError::Storage` from `scan_prefix_stream` (e.g. corrupted batch, transient IO). Every write still succeeds, but the vacuum scan collects a partial/empty entry list each time and quietly deletes nothing. No warning is logged; disk fills; the only symptom is history growth. An operator correlating with `gc_below` errors would see inconsistent behavior between two GC entry points.

**Suggested fix:** Either propagate (return early, making `vacuum_key` return `DbResult<()>` — its only callers are write paths that already return `DbResult`), or at minimum `log::warn!` on each errored batch and skip the reclaim pass for that key (a partial entry list must not drive deletions at all). Match `gc_below`/`purge_below_ts`.

### 3. Changefeed journal gap detection has an undetected hole on persist failures
**File:** `crates/shamir-tx/src/changefeed.rs:632-655` (`persist_one`); related: `journal_send` `Closed` branch :336-339
**Severity:** medium

**Issue:** CF-1's `first_gap_version` is updated **only** on `TrySendError::Full` (channel overflow, :310-334). When the background writer's `store.put()` fails (`persist_one`, :645-651), the event is dropped with a `log::warn!` and *no* gap marker — and the next successful persist advances `last_persisted_version` via `fetch_max` **past the hole**, so the CF-2 watermark also cannot expose it. `read_from` (:414-421) then returns `gap_at: None` over a journal that is silently missing a version — directly violating the module's own stated contract at :415: "Conservative over-signal is acceptable; silent omission is not." The `TrySendError::Closed` branch is even quieter: no counter, no gap marker, no log (a panicked writer task closes the channel and every subsequent commit's journal event vanishes with zero observability; CF-2 only detects this if a consumer is actively comparing watermarks).

**Failure scenario:** A replication consumer resumes from `read_from(v)` after a transient store failure dropped version `v`'s journal write. `gap_at` is `None`, events on both sides of the hole are returned, and the consumer trusts an unbroken history — missing exactly one committed transaction with no signal to trigger the documented full-snapshot resync.

**Suggested fix:** In `persist_one`'s error branch, run the same min-CAS loop on `first_gap_version` that `journal_send` uses for `Full`. For `Closed`, bump `journal_dropped` (or a dedicated counter) so a dead writer is at least countable.

### 4. `thiserror` declared but never used — six APIs return `Result<_, String>`, including a public trait
**File:** `crates/shamir-tx/Cargo.toml:23` (dependency); `changefeed.rs:154` & `:157` (`ChangelogStore::put`/`range_from`), `changefeed.rs:542` (`serialize_event`), `staging_store.rs:249` (`rewrite_set_bytes`), `tx_context.rs:913-916` (`apply_id_remap`), `mvcc_store/mod.rs:500` (`set_retention`), `mvcc_store/retention.rs:60` (`Retention::validate`)
**Severity:** medium

**Issue:** CLAUDE.md's error-handling rule is explicit: "`thiserror` for library error enums (with `#[from]` where natural)". `thiserror` is a declared dependency of this crate but a repo-wide grep finds zero uses — the crate defines no error enum of its own and instead threads `String` through six APIs. `ChangelogStore` is the worst offender: a **public trait** whose `Result<(), String>` / `Result<Vec<Bytes>, String>` shape forces stringly-typed errors onto every implementor (engine-side production store included), making error-kind matching impossible and pushing `format!`-based error construction into callers (`serialize_event`, `apply_id_remap`'s `.map_err(|e| format!("remap: {e}"))`). The others are validation/config seams where a small `thiserror` enum (`RetentionError`, `RemapError`) would let callers distinguish decode failures from policy violations.

**Suggested fix:** Introduce `#[derive(thiserror::Error)]` enums for the crate's own failure kinds and convert the six sites, starting with the public `ChangelogStore` trait (its `put`/`range_from` failures are already only ever logged — a small `ChangelogStoreError` with `#[from]` for the underlying storage error is a drop-in).

### 5. Error-path tests stop at two functions on fresh keys — the documented batch-abort and drain error claims are untested
**File:** `crates/shamir-tx/src/tests/mvcc_store_tests/error_tests.rs` (entire file); fault double at `test_stores.rs:10-108`
**Severity:** medium

**Issue:** The crate has an excellent fault-injection double (`FailingStore` with `fail_get`/`fail_remove`/`fail_set`) but it is exercised by exactly three tests, all on a **fresh key** against `set_versioned`/`delete_versioned`. Untested error paths, each carrying a documented behavioral claim that only a test can pin:
- `set_versioned_many` / `set_versioned_many_append_only` transact failure — the guard-vector comment (mod.rs:869-873) claims "every guard drops un-committed and marks its version Aborted, so the contiguous watermark advances past the whole failed batch instead of wedging at the first version". No test asserts the watermark advances or that overlay stays empty.
- No pre-existing-key failure test — which is precisely why Finding 1 (prior-version masking) is invisible to the suite: `set_versioned_propagates_archive_read_error` asserts `get_current == None` on a key that *never existed*, where `None` is the correct answer for the wrong reason.
- `write_committed_to_history` / `write_committed_batch_to_history` / `drain_to_history` `?` propagation, and the `drain_exclusive` backoff-returns-`Ok` contract (#1032) — only the deferral is tested (`write_committed_batch_tests.rs:363`), not the error path.
- Batched reads: `get_at_many`/`get_current_many` propagate `get_many` errors with `?` mid-assembly — untested (note: `FailingStore` doesn't override `get_many`, so injection through the default per-key `get` works but is never exercised).
- `vacuum_key` scan-error behavior (Finding 2) and journal persist-failure gap behavior (Finding 3) — untested.

**Suggested fix:** Extend `error_tests.rs`: (1) failed `set_versioned` on a key with a committed prior version, asserting the prior value is still readable (will currently fail — Finding 1); (2) failed `set_versioned_many`, asserting `gate.last_committed()` advances past the batch, overlay stays empty, and `durable_watermark() <= last_committed()`; (3) failed `drain_to_history` mid-version, asserting the error propagates and `drain_exclusive` is released; (4) `get_at_many` with `fail_get` armed, asserting propagation.

### 6. `ts_index_rebuild` swallows all stream errors and unconditionally marks the index ready
**File:** `crates/shamir-tx/src/mvcc_store/mod.rs:398-429` (`Err(_) => continue` at :408; `ts_index_ready.store(true, ...)` at :428)
**Severity:** low

**Issue:** The lazy rebuild treats every errored batch as skippable, then sets `ts_index_ready = true` regardless of how much was dropped. A partial rebuild is never retried (the flag gates future rebuilds forever), and the documented fallback ("falls back to the full history scan only if the index is empty after rebuild", mvcc_history.rs:80-81) does not fire for a *partially* populated index — `version_at_or_before_ts` then silently resolves as-of-ts queries to stale versions with no log line. Best-effort is acceptable for this index; permanently caching a known-bad state is not.

**Suggested fix:** Count dropped batches; if any errored, leave `ts_index_ready = false` (retry on next query) and `log::warn!` once. Alternatively track a `ts_index_degraded: AtomicBool` surfaced next to `ts_index_len()`.

### 7. `LayeredInterner::touch_sync` panics on a fallible signature that `commit_interner_overlay` propagates
**File:** `crates/shamir-tx/src/layered_interner.rs:82-86` vs `:259-263`
**Severity:** low

**Issue:** `Interner::touch_ind` returns `Result<TouchInd, &'static str>` (shamir-types interner.rs:138). The `Direct`-mode branch handles it with `.expect("Interner::touch_ind is infallible for valid input")` — on the **non-tx hot path** — while the same operation ten lines below in `commit_interner_overlay` is treated as a real, mappable error (`.map_err(|e| DbError::Codec(e.to_string()))?`). Today `touch_ind` happens to never return `Err` (all its body paths are `Ok`), so the expect holds; but the two call sites disagree about the contract, and any future `touch_ind` change that returns an error (its signature explicitly reserves that) converts the non-tx write path into a panic while the commit path degrades gracefully. CLAUDE.md permits `expect` only for genuine invariant violations; "I checked the other call site and it disagrees" means this isn't one.

**Suggested fix:** Make `touch_sync` return the `Result` (or map the error to a sentinel id / propagate as `DbError::Codec`) so both paths share one contract; if the infallibility claim is real, that belongs in a test pinning `touch_ind`'s Ok-for-all-inputs property, not an `expect` on the hot path.

### 8. `StagedRow::as_inner` panics on msgpack decode failure though nothing validates at construction
**File:** `crates/shamir-tx/src/staging_store.rs:46-49`
**Severity:** low

**Issue:** `StagedRow`'s invariant ("always holds valid msgpack") is a caller contract — `StagingStore::set`/`set_many` accept arbitrary `Bytes` with no validation, so a caller staging malformed bytes (raw put path, corrupt upstream payload) defers the failure to `as_inner`, which panics. The inconsistency: the commit-time remap of the *same bytes* (`remap_inner_value_bytes`, id_remap.rs:77-78) treats decode failure as a recoverable `Err` that aborts the tx. A malformed staged row should be an abortable error at read-your-own-write time too, not a process-killing panic (this is a server library; a panic takes down the runtime shared by every other session).

**Suggested fix:** Change `as_inner` to `Result<Cow<'_, InnerValue>, rmp_serde...Error>` (or add `try_as_inner`), or validate on `set` and return an error at staging time. Callers are few (read-your-own-write lens and commit remap).

### 9. `apply_committed_ops` doc contradicts the code's error-path ordering
**File:** `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:413-431`
**Severity:** low

**Issue:** The function doc states "Ordering: history FIRST (durable landing), then visible (overlay + cell) — matching the pre-split contract where a failed history `transact` (`?`) left no reader-visible state." The code does the **opposite**: `apply_committed_visible(&ops, commit_version)` runs first (:427), then `write_committed_to_history(...).await?` (:428). The inner comment (:417-426) correctly explains the intentional swap (pending_ts must be stamped before the drain half consumes it) and admits "a history error propagates via `?` (the cell/overlay are then ahead …)" — so on this path a failed history write *does* leave reader-visible state. The stale outer paragraph is exactly what a future maintainer auditing error-path state will read first, and it describes the opposite guarantee.

**Suggested fix:** Rewrite the doc's ordering paragraph to match the code: visible-first (ts stamp) → history `?` → on error the cell/overlay are intentionally ahead, mirroring the production ack-path until the drainer catches up.

### 10. `lookup_ts` swallows all `history.get` errors as "unknown age" with no log
**File:** `crates/shamir-tx/src/mvcc_store/mod.rs:1624-1636`
**Severity:** nit

**Issue:** `Err(_) => None` is the conservative direction (unknown-ts versions are KEPT by vacuum/purge), so it is safe — but a persistent storage error silently disables the entire age-retention axis and, in `history_of`, silently degrades every `ts_millis` to `None`. A single `log::debug!`/`warn!` (rate-limited) on the error arm would make "age retention quietly stopped working" diagnosable.

### 11. `RepoChangefeed::new` panics outside a tokio runtime; writer task is detached
**File:** `crates/shamir-tx/src/changefeed.rs:249-255`
**Severity:** nit

**Issue:** `tokio::spawn(journal_writer_loop(...))` panics if called off-runtime (e.g. from a `new()` during engine construction before the runtime exists), and the returned `JoinHandle` is discarded so a writer panic is observable only via the CF-2 watermark stall. Engine-side call sites are runtime-hosted today; a `#[track_caller]`-documented contract or a builder that takes the runtime's handle would remove the foot-gun.
