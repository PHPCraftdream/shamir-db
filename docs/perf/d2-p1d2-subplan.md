# D2 / P1d-2 — Sub-plan: history-write off the ack-path (the cutover)

HIGH risk. Changes crash/durability mechanics. Builds on committed
`609a3ae` (foundation) + `223329c` (P1d-1 durable_watermark).

Approach chosen: **run_leader → background batcher** (task #1 framing).
Concentrate the change in `materialize` + a new repo-level drainer, so all
four commit call-sites (commit_tx_lockfree, legacy_async tail, run_leader,
run_single_tx) inherit deferral automatically.

---

## 1. Invariant (the crash contract)

WAL entry is the source of truth. After cutover:

```
Phase 4: wal.begin_grouped(entry)         [DURABLE — ack point, unchanged]
ack-path: overlay.insert + publish_cell + counter + index(5c)  [visible]
          → guard.commit() (visibility)   [reader sees it]
          → enqueue(version, table data ops) into Drainer
[background Drainer]: history.transact(batch) + record_ts        [durable data]
                      → gate.mark_durable(version)
                      → wal.commit(txn_id)  [truncate inflight marker]
```

**Hard rule:** `wal.commit(txn_id)` (inflight-marker removal / truncation
eligibility) NEVER runs before the version's data is durable in `history`.
Crash anywhere before the drain finishes ⇒ inflight WAL entry survives ⇒
`recover_inflight_v2` replays it into history (idempotent, already exists).

`durable_watermark ≤ visibility_watermark` always (P1d-1 holds it). The gap
= versions enqueued-but-not-drained = the overlay window.

---

## 2. What moves where

### Stays on the ack-path (cheap, in-memory / already-inline)
- overlay.insert + publish_cell (P1c) — visibility value.
- Phase 5b counter (in-memory).
- Phase 5c index postings (info_store) — **kept inline for the first cut**;
  the overlay holds DATA only. Deferring index needs an index-overlay (out of
  scope; note as follow-up). uwl still covers index, but the expensive
  *data* `history.transact` leaves the uwl-held section — that is the D0a win.
- guard.commit() (visibility mark + last_committed advance).
- Phase 6.5 persist_markers (last_committed/next_tx_id) — visibility metadata,
  cheap, stays. Consistent on crash: recovery re-derives the same floor.

### Moves to the background Drainer
- Phase 5a `history.transact` (the version-log data write) + record_ts.
- `gate.mark_durable(version)` (was P1d-1 inline-on-Complete).
- Phase 7 `wal.commit(txn_id)` + its A5 interner-hwm gate.

---

## 3. New component — `Drainer` (repo-level, single owner)

`crates/shamir-engine/src/tx/drainer.rs` (or in shamir-tx if it needs no
engine types; likely engine because it touches per-table history + wal).

- **Queue:** lock-free `crossbeam::SegQueue<DrainJob>` (or `scc::Queue`).
  `DrainJob { commit_version, txn_id, per_table: Vec<(table_token, Vec<KvOp>)>, interner_max_ids }`.
- **Single drain task per repo:** spawned once (lazy, OnceCell) at first
  commit. Owns the drain loop. Lock-free leader is unnecessary — exactly ONE
  owner task (no contention on a File like WAL had); the queue is the only
  shared point (lock-free).
- **Drain loop:**
  1. Pop a batch of jobs (drain queue until empty or batch cap).
  2. Group ops by table_token; for each table, one `history.transact(ops)`
     (+ record_ts per version). One write-tx per table per batch (fsync
     amortized — backends batch).
  3. On success: `gate.mark_durable(version)` for each version; then
     `wal.commit(txn_id)` for each (gated by A5 interner-hwm as today).
  4. On history write error: leave the job's WAL marker inflight (do NOT
     mark_durable, do NOT wal.commit) — recovery converges. Log + retry
     (bounded) or re-enqueue.
- **Lifecycle:** weak-ref task (exits when repo dropped), mirrors
  MemBufferStore flusher / WAL background fsync.
- **Shutdown/flush:** a `drain_all().await` (graceful close) flushes the queue
  so a clean shutdown leaves nothing un-drained (and the marker can truncate).

### Source of the data ops
`materialize` already has `collect_data_batches(tx)` = `(table_id, base, ops)`.
The ack-path keeps overlay.insert (already done in apply_committed_ops via
P1c); the drain needs ONLY the `history.transact` half. So:
- Split `MvccStore::apply_committed_ops` into:
  - `apply_committed_visible(ops, version)` — overlay.insert + publish_cell
    (NO history write). Called inline (Phase 5a).
  - `drain_committed_to_history(ops, version)` — history.transact + record_ts
    (NO overlay, NO publish_cell). Called by the Drainer.
- `materialize` Phase 5a calls `apply_committed_visible` and enqueues a
  `DrainJob` carrying the ops (cloned — they're already cloned into the WAL
  entry, so memory cost is bounded by the overlay window).

---

## 4. Edits (concrete)

1. **`mvcc_store/mvcc_history.rs`**: split `apply_committed_ops` → visible-half
   + drain-half (as §3). `apply_data_batch` (commit_phases) routes the visible
   half on the ack-path.
2. **`tx/materialize.rs`**: Phase 5a calls the visible-half; build + enqueue
   `DrainJob`. Remove inline `history.transact`. Remove the inline
   `mark_durable`-on-Complete (moves to drainer). Phase 7 (wal.commit) removed
   from `post_publish_cleanup` → drainer.
3. **`tx/drainer.rs`** (new): the Drainer + DrainJob + drain loop + drain_all.
4. **`repo/repo_instance.rs`**: hold `Arc<Drainer>` (OnceCell), spawn task,
   expose `drainer()` + `drain_all()` for shutdown/tests.
5. **`tx/recovery.rs`**: after replay, also `gate.mark_durable(commit_version)`
   (replayed data is durable). Overlay empty on open → reads hit history.
6. **`tx/commit.rs` / `group_commit.rs`**: remove the inline
   `gate.mark_durable(..)` added in P1d-1 (now the drainer marks). Call-sites
   otherwise unchanged (materialize/post_publish_cleanup do the enqueue).
7. **Phase-7 truncation gate**: the drainer's `wal.commit` keeps the A5
   interner-hwm check (move that logic from post_publish_cleanup).

---

## 5. Sub-steps (each gated; STOP on doubt)

- **P1d-2a** — Drainer skeleton + DrainJob + single drain task + drain_all,
  WIRED but run_leader/materialize still write history inline (drain is a
  redundant no-op: drain-half is idempotent over already-written history).
  Proves the drainer + queue + lifecycle. Gate @oracle @engine.
- **P1d-2b** — Cutover: materialize stops writing history inline (visible-half
  only) + enqueues; drainer becomes the sole history writer; mark_durable +
  wal.commit move to drainer; recovery marks durable. Phase 7 removed from
  ack-path. Gate @oracle @engine @e2e.
- **P1d-2c** — Crash-injection seam (e): crash with WAL durable + data NOT
  drained → recovery reconstructs history from WAL; convergence + watermark
  tests. `drain_all()` on graceful shutdown. Gate crash + @e2e.

---

## 6. Risks / open questions

- **Index still inline under uwl.** First cut defers DATA only. If index
  materialize is a co-bottleneck, a later index-overlay defers it too. Note,
  measure with D0b bench after P1d-2b.
- **Unbounded drain queue.** P1d-2 uses an unbounded queue; backpressure is
  P1e (soft cap on visibility−durable gap → async-yield producers). Under a
  slow disk + heavy writes the overlay+queue grow until P1e lands — acceptable
  for the gated milestone, `log()` the depth.
- **record_ts ordering / age-retention.** ts now written by the drain (lags).
  Age-vacuum already treats unknown-ts as "keep" (conservative) — overlay-
  window versions are kept until drained. Verify retention tests.
- **overlay.remove in vacuum/gc/purge (P1c)** must not outrun the drain: a
  not-yet-drained overlay version must NOT be removed (it is the only data
  copy). Gate those `overlay.remove` calls on `version <= durable_watermark`
  in P1d-2b (the P1c agent flagged this).
- **Drain error / poison.** A persistent history write failure leaves markers
  inflight forever (no truncation) and the overlay grows. Bounded retry +
  circuit-breaker + metric; recovery still correct (WAL intact).

---

## 7. Verification

- Existing @oracle/@engine/@e2e green at each sub-step.
- New: crash seam (e) recovery test (data only in WAL+overlay, crash, reopen,
  data present from history after replay).
- durable_watermark now genuinely LAGS visibility under concurrent load
  (assert the gap is non-zero mid-flight, converges to zero after drain_all).
- Bench D0b: same-table concurrent-commit regress closed (data write off uwl).

---

## 8. Refinement (post-contemplation) — drain = generalized recovery

Созерцание уточнило §3/§5: **убрать `SegQueue<DrainJob{ops}>` (третья копия).**
Дренаж — это `recover_inflight_v2` (`recovery.rs:245`), прокрученный в фоновом
цикле, а не только при open:

- Источник дренажа = **inflight-хвост WAL** (`wal.recover()` → `Vec<WalEntryV2>`,
  каждый несёт `commit_version` + ops). Хвост мал (truncation держит).
  `replay_v2_entry(entry, repo)` уже маршрутизирует ops по таблицам в history.
- Дренаж-шаг: для entries с `durable_wm < commit_version ≤ visibility_wm`,
  по возрастанию версии → `replay_v2_entry` → `gate.mark_durable(V)` →
  `wal.commit(txn_id)` (A5 interner-hwm gate). Это тело `recover_inflight_v2`.
- Overlay НЕ источник дренажа — он чистый read-cache (RYOW), GC'ится отдельно
  на продвижении `durable_watermark` (`version ≤ durable_wm`). Значения
  идентичны (P1c), так что warm-drain (overlay) и cold-drain (WAL) дали бы
  одно; берём WAL как простейший корректный (= recovery), overlay-source —
  потенциальная оптимизация перечтения.
- **Materialize↔recovery дубликат схлопывается:** общее ядро «replay V →
  history + mark_durable + truncate» зовут и recovery (cold), и drainer (warm).

### Под-срезы (уточнённые)
- **P1d-2a** — `Drainer` компонент (single-owner task + `drain_step` =
  обобщённый recover_inflight_v2 + `drain_all`), юнит-тесты на seeded inflight
  WAL. НЕ подключён к live commit-пути (как P1a-scaffold). Gate @oracle @engine.
- **P1d-2b** — cutover: materialize → visible-half (overlay+publish_cell, без
  history); Phase-7 `wal.commit` + `mark_durable` уходят с ack в drainer;
  drainer спавнится на repo; recovery тоже `mark_durable`. Gate @oracle @engine @e2e.
- **P1d-2c** — crash-seam (e) + `drain_all` на shutdown + тесты. Gate crash + @e2e.
