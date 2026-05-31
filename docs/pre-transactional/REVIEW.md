בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Pre-Transactional Review

State-of-the-world snapshot. Captures what landed, what remains, what
compromises were taken, and what open questions exist.

**As of:** 2026-05-31. This is the **Phase-A** audit record; since it,
**Phase B** (interactive multi-call transactions) and **Phase C** (predicate/
range locks → phantom protection, true serializability) have shipped — see
[`../PROJECT_STATE.md`](../PROJECT_STATE.md) for the current project snapshot
and `../roadmap/{PHASE_B_INTERACTIVE_TX,PHASE_C_SERIALIZABLE}.md`. The §11
follow-up list below is updated to reflect what has since closed.

(Original Phase-A line: post-audit hardening — two review waves landed on
top of Phase A; see §11 for the audit closures and the honest list of
follow-ups still open).

---

## 0. Конечная цель проекта

`ShamirDB` — production-grade, self-contained, decentralized database
written in Rust. Single binary < 50 MB, no external runtime dependencies.

**Transactional layer goal:** support `transactional: true` in
`BatchRequest` with two isolation levels:

- **Snapshot Isolation (default)** — consistent snapshot reads,
  last-writer-wins on conflicts.
- **Serializable Snapshot Isolation (`"isolation": "serializable"`)**
  — read-set validated at commit, concurrent overlap → abort with
  `tx_conflict`.

**Cross-repo guard** — tx batch must target a single repository.
2PC across repos is intentionally out of scope.

**Cross-table internal** — tx within one repo may touch N tables;
all-or-nothing visibility.

**Crash safety** — durable WAL marker before any data writes;
recovery on next open replays inflight entries.

---

## 1. Высокоуровневая карта стадий

Восемь подготовительных этапов в `docs/pre-transactional/00-overview.md`.
Каждый — самодостаточный, landable отдельно, не ломает существующего
поведения на non-tx путях.

```
Stage 0 — Foundations               ✅ COMPLETE
Stage 1 — Write isolation           ✅ COMPLETE
Stage 2 — Per-repo coordinator      ✅ COMPLETE
Stage 3 — MvccStore + read pipeline ✅ COMPLETE
Stage 4 — Executor + SI/SSI         ✅ COMPLETE (transactions работают E2E)
            ├── 4.A-D.6              ← core execution
            ├── 4.G                  ← closed 5 design compromises
            ├── 4.H                  ← defensive provider + bench coverage
            └── 4.F                  ← acceptance test suite
Stage 5 — Reconciliation            🔶 5.1+5.2 COMPLETE (SSI + index attribution)
            ├── 5.1                  ✅ Phase 5 → MvccStore (SSI prod-ready)
            ├── 5.2                  ✅ IndexWriteOp per-table token
            ├── 5.3                  ✅ LayeredInterner wired into tx write path
            └── 5.rest               ✅ All 5 limitations closed
Stage 6 — GC + telemetry            🔶 6.1-6.4 COMPLETE (GC + tx lifetime)
            ├── 6.1                  ✅ MvccStore::gc_below
            ├── 6.2                  ✅ RepoInstance::run_gc + tests
            ├── 6.3                  ✅ max-tx-lifetime (5 min default)
            ├── 6.4                  ✅ periodic GC background task
            └── 6.rest               ⏳ Prometheus/OTel exporter (operational)
Stage 7 — Tests + landing           🔶 7.1 COMPLETE (crash recovery)
            ├── 7.1                  ✅ V2 WAL recovery
            ├── 7.rest               ⏳ multi-conn harness
            └── 4.E SDK              ✅ already wired (isolation field in BatchRequest)
```

---

## 2. Что landed — детально

### Stage 0 — Foundations (9 sub-stages)

`MetaKey` enum (16 variants) центрально owns system-key namespace.
Typed WAL/shadow keys, recovery markers (`LastCommittedVersion`,
`NextTxId`), `Store::transact(Vec<KvOp>)` with native overrides on
5 disk backends. `Store::raw_backend()` unwrap helper. `WalEntryV2`
with inline body envelope `[WAL2][version][bincode]`. Dual-version
`list_inflight` (V1 + V2).

**Crates extracted:** `shamir-wal`, `shamir-tx`.

### Stage 1 — Write isolation (10 sub-stages)

`IndexWriteOp` planner API: all 6 index backends (FTS, FtsRanked,
Functional, Vector, Btree, MockBackend) gained `plan_insert/update/delete`
returning `Vec<IndexWriteOp>` instead of writing directly. `apply_index_ops`
helper applies the planned ops via `Store::transact` or per-op set/remove.
HNSW staged buffer with `commit_staged / rollback_staged / search merge`.
`StagingStore` per-transaction write buffer with read-through semantics.

### Stage 2 — Per-repo coordinator (4 sub-stages)

`RepoTxGate` (commit_mutex + version_counter + active_snapshots +
SnapshotGuard RAII). `LayeredInterner` (Direct/Layered enum,
`OVERLAY_ID_BASE = 1<<48`, `commit_interner_overlay` with id-remap).
`RepoWalManager` for V2 entries.

**`TxContext` — current field set** (after the HNSW-relocation refactor,
commit `6492c32`, and the unique-under-lock work, `fbb4aac`):

- `tx_id`, `repo_id`, `snapshot_version`, `isolation`, `started_at`.
- `write_set: HashMap<u64, StagingStore>` — per-table write staging.
- `index_write_set: Vec<(u64, IndexWriteOp)>` — index ops with per-op
  `table_token` attribution.
- `staged_vectors: HashMap<u64, Vec<(RecordId, Vec<f32>)>>` — HNSW
  vectors awaiting promotion at commit. **This is the home for vector
  staging.** Earlier the staging lived in an adapter-held
  `scc::HashMap<TxId, …>` *and* the tx tracked a
  `tables_with_hnsw_staging` set so abort could broadcast a rollback;
  both are gone. Staged vectors now live entirely inside the
  `TxContext`, so RAII drop on abort discards them with zero I/O and no
  broadcast (HIGH-6).
- `interner_overlay: scc::HashMap<String, u64>` + `next_overlay_id`.
- `counter_deltas: HashMap<u64, i64>` — per-table row-count deltas.
- `read_set: scc::HashMap<(u64, Bytes), u64>` — SSI read tracking. Now
  an `scc::HashMap` (not `std::HashMap`) so `record_read_shared` can take
  `&self`; this is load-bearing for HIGH-C (see §11) — the engine's
  `read_one_tx` holds the tx by shared reference, so interior mutability
  is what lets the read path populate the set in production.
- `table_tokens: HashMap<u64, String>` — token → name for WAL emission.
- `version_provider: Option<Arc<dyn VersionProvider>>`.
- `unique_guards: Vec<UniqueGuard>` — deterministic unique-index keys a
  tx intends to own, re-validated under `commit_lock` (closes the
  tx-concurrent unique-violation hole, `fbb4aac`).

### Stage 3 — MvccStore + read pipeline (7 sub-stages)

`MvccStore` over main+history stores. Zero-overhead non-tx path
(`active_snapshots_empty()` → skip history). `version_cache` for
fast-path reads. `get_at(snapshot)` slow-path range scan in history.
Tx-aware surface across the full read pipeline: `TableManager::read_one_tx`,
`*_tx` streaming hooks, `IndexBackend::lookup_tx` default trait method,
`VectorBackend::lookup_tx` override (HNSW staging in-tx search),
`apply_index_ops_tx`, `SortedIndexManager::*_tx` wrappers.

### Stage 4 — Executor (28 sub-stages — split below)

**4.A — Wire format**
`BatchRequest.isolation: Option<String>`. `TransactionInfo { tx_id,
status, reason, snapshot_version, commit_version }` with
`committed()` / `aborted()` helpers.

**4.B — apply_id_remap**
`StagingStore::rewrite_set_bytes(f)` + `TxContext::apply_id_remap(remap)`.
Walks `InnerValue` trees in staged record bytes, rewrites `InternerKey`
IDs per remap. Hook for LayeredInterner commit merge (Stage 5).

**4.C — Cross-repo guard**
`distinct_repos(queries)` helper + `BatchError::CrossRepoNotSupported`.
Guard fires before plan for `transactional: true` batches.

**4.D — Commit pipeline (5 sub-stages + 5 micro-stages in 4.D.6)**
- 4.D.1: `RepoInstance::tx_gate / repo_wal` lazy accessors.
- 4.D.2: `commit_tx` 7-phase scaffold (Phase 3/4/6/7 wired, 1/2/5
  stubbed).
- 4.D.3: `TableResolver::resolve_repo` + `RepoInstance::begin_tx /
  commit_tx` facade.
- 4.D.4: Phase 5 actual physical writes via `base.transact(drain)`.
- 4.D.5: SSI skeleton — `MvccStore::version_of`,
  `TxContext::validate_read_set`, stub provider.
- 4.D.6.a: `insert_tx` + `table_token` + `ensure_table_staging`.
- 4.D.6.b: `update_tx / delete_tx / set_tx` + `StagedMutation` extract.
- 4.D.6.c.1: `execute_insert_tx`.
- 4.D.6.c.2: `execute_update_tx / delete_tx / set_tx`.
- 4.D.6.c.3: `QueryRunner` struct refactor.
- 4.D.6.d: `execute_batch` tx mode wiring + SI happy-path E2E test.
- 4.D.6.e: `VersionProvider` trait + injection hook.

**4.F — Acceptance**
6 integration tests (happy path, abort, read-after-write,
monotonic versions, cross-table, SSI unknown-table conflict).

**4.G — Compromises closure (7 sub-stages)**
- 4.G.1: `repo_token(name)` deterministic ID.
- 4.G.2: **WAL data ops emission** — crash safety closed.
- 4.G.3: Phase 1 safe wire.
- 4.G.4: `RepoVersionProvider` production auto-attach.
- 4.G.5: `idx_id` invariant docs.
- 4.G.6: `tx_pipeline.rs` benchmark suite.
- 4.G.7: Known limitations docs.

**4.H — Defensive semantics + bench coverage (3 sub-stages)**
- 4.H.1: `VersionProvider -> Option<u64>` defensive missing-table.
- 4.H.2: `commit_tx` phase breakdown bench.
- 4.H.3: Provider overhead pairwise bench.

### Stage 7.1 — V2 WAL recovery (5 sub-stages)

- 7.1.a: Recovery skeleton (replay_v2_entry/op + RepoInstance::recover_v2_inflight).
- 7.1.b: `WalOpV2::Put/Delete` `table_id_interned` schema extension.
- 7.1.c: Real apply Put/Delete/CounterDelta.
- 7.1.d: `IndexPut/IndexDel` schema + apply (broadcast for `table_id=0`).
- 7.1.e: End-to-end crash simulation test.

### Stage 5.1+5.2 — SSI production wiring (3+1 sub-stages)

- 5.1.a: `MvccStore::apply_committed_ops(ops, commit_version)`.
- 5.1.b: Phase 5 rewired through MvccStore (closes SSI blind spot).
- 5.1.c: SSI conflict detection E2E test. **Limitation #1 CLOSED.**
- 5.2: `index_write_set` carries `(table_token, IndexWriteOp)`.
  WAL emission uses real table_token. **Limitation #4 CLOSED.**

### Stage 6 — GC + tx lifetime + metrics (5 sub-stages)

- 6.1: `MvccStore::gc_below(min_version)` — history cleanup core.
- 6.2: `RepoInstance::run_gc()` + integration tests (GC respects active
  snapshots).
- 6.3: `TxContext::started_at` + `TxError::Expired` (5 min default).
- 6.4: `RepoInstance::spawn_gc_task(interval)` — periodic background GC.
- 6.5: `TxMetrics` atomic counters (txs started/committed/aborted, GC
  runs/entries deleted). Zero external dependencies.

### Stage 7.2+7.3 — Concurrency scenarios + Rust unit tests

- 12 acceptance tests covering all 12 scenarios from §7.2 (11 as Rust
  tests, #12 migration deferred).
- Concurrent `assign_next_version` / `fresh_tx_id` no-duplicates tests.
- Busy-history MvccStore `get_at` with 5 versions.

---

## 3. Архитектурные решения — quality assessment

Девять решений в `architectural-decisions.md` + `05-executor-isolation.md`.

| ID | Decision | Quality | Notes |
|---|---|---|---|
| **D1** | HNSW stage-on-insert / apply-on-commit | ✅ clean (relocated `6492c32`) | Staging now lives in `TxContext::staged_vectors: HashMap<u64, Vec<(RecordId, Vec<f32>)>>` keyed by table token — **not** an adapter-held `scc::HashMap<TxId, …>` anymore. Commit Phase 5d (`apply_staged_vectors`) promotes exactly the tx's vector footprint; abort discards via RAII drop with no broadcast rollback. Brute-force merge on in-tx vector search reads `staged_vectors_for(token)`. Tests cover commit + rollback. |
| **D2** | IndexWriteOp planner | ✅ clean | Pure data ops, applied separately. All 6 backends migrated. |
| **D3** | MemBuffer / tx staging separation | ⏳ deferred | Stage 5 work. Currently Phase 5 bypasses MemBuffer naturally (direct `base.transact`). |
| **D4** | Repo-scoped WAL | ✅ clean | `RepoWalManager` coexists with per-table V1 WAL. V2 entries carry full ops. |
| **D5** | `Option<&TxContext>` not generic | ✅ clean | Surface decision held. Benches confirm zero-overhead non-tx path. |
| **D6** | `version_codec` 0xFF separator | ✅ clean | 6 round-trip tests. Bench not required (trivial fn). |
| **D7** | `table_token()` accessor | ✅ pragmatic | DefaultHasher placeholder. Stage 5 swaps to real interner. |
| **D8** | `StagedMutation` value object | ✅ clean | 4 mutation types share ~10-line core. Extract was done in 4.D.6.b. |
| **D9** | `QueryRunner` struct | ✅ clean | Existing free `execute_single` is thin wrapper. Refactor, not duplication. |

**Все 9 решений verified by tests + most by benches.** Test+bench coverage
matrix in `architectural-decisions.md` is honored for the realizable
subset (Stage 5/7-blocked decisions documented).

---

## 4. Compromises — status

5 compromises identified at Stage 4 audit, all subsequently closed:

| # | Compromise | Closure | Status |
|---|---|---|---|
| 1 | `tx.repo_id = 0` placeholder | 4.G.1: `repo_token(name)` accessor | ✅ closed |
| 2 | `idx_id: 0` in WAL IndexPut/Del | 4.G.5: doc invariant (recovery decodes from key prefix) | ✅ closed (doc) |
| 3 | `apply_id_remap` Phase 1 TODO | 4.G.3: safe early-skip wire | ✅ closed |
| 4 | WAL incomplete (no data ops) | 4.G.2: `snapshot_ops` + emit Put/Delete | ✅ closed |
| 5 | VersionProvider stub-only | 4.G.4: `RepoVersionProvider` auto-attach | ✅ closed |

**No outstanding Stage 4 compromises.** All silent-fallback patterns
hardened (4.H.1 — `VersionProvider -> Option<u64>` defensive).

---

## 5. Known production limitations (post-Stage 4 + 7.1)

Documented in `docs/pre-transactional/05-executor-isolation.md`
"Known Production Limitations" + `docs/ops/FLAKY_TESTS.md`. Not bugs —
documented stage-cut boundaries.

### 1. ~~SSI conflict detection blind on tx writes~~ — CLOSED (5.1) + hardened (HIGH-C)

Commit Phase 5a routes data writes through
`MvccStore::apply_committed_ops` (5.1.a/b), so `version_cache` is updated
on tx commits and a committer's write is visible to a concurrent tx's
commit-time validation. Verified by
`ssi_conflict_detected_on_concurrent_tx_writes` (5.1.c).

**Honest caveat closed by the audit (HIGH-C, commit `c967317`).** 5.1 made
the *write side* visible, but the *read side* was still inert in
production: `record_read` was reachable only from unit tests, so the
`read_set` was empty at commit and Serializable silently degraded to
Snapshot for real SELECTs. HIGH-C changed `read_set` to an `scc::HashMap`
and added `record_read_shared(&self, …)`, then wired
`TableManager::read_one_tx` to populate it. The final link — the executor
routing `BatchOp::Read` through `read_tx` with a shared `&TxContext` — is
being closed in a parallel task (I.1, see §11); the read-tracking
machinery it depends on is committed. Until I.1 lands, SSI write-skew on
plain SELECTs is detected only where a read path already threads the tx.

### 2. ~~Repo-level interner is placeholder~~ — ACCEPTABLE

`repo_token(name)` and `table_token_for(name)` use `DefaultHasher` —
deterministic, collision-free for production workloads. A "real"
repo-level interner (persistent string→u64 mapping) is unnecessary:
per-table interners handle field names, and repo/table identity tokens
are stable hashes. No action needed.

### 3. ~~`tx.interner_overlay` always empty in production~~ — CLOSED (5.3)

Stage 5.3.b wired `LayeredInterner` into `execute_insert_tx` /
`execute_update_tx` / `execute_set_tx`. New field names during a tx
now populate `tx.interner_overlay`. Phase 1 of `commit_tx` (5.3.c)
merges overlay into each table's base interner with per-table remap.

### 4. ~~Index ops use `table_id_interned: 0` broadcast emission~~ — CLOSED (5.2)

`tx.index_write_set` now carries `(table_token, IndexWriteOp)` tuples.
WAL emission uses the real table_token instead of broadcast `0`.
Recovery routes index ops to the correct table's info_store.

### 5. ~~WAL `InternerOverlayMerge` not replayed~~ — CLOSED (5.3.d)

Recovery now merges overlay entries into every table's base interner
via `touch_ind` (idempotent broadcast). Persists after merge.

---

## 6. Метрики

| Metric | Value |
|---|---|
| Total commits (pre-tx work + 2 audit waves) | 130+ (Phase A ~108, then CRIT/HIGH/NEW/security follow-ups — see §11 for SHAs) |
| Tests workspace `--lib` | 1460+ passing at the pre-audit revision; the audit waves added more (per-crate `--lib` green; full-workspace count not re-tallied for this doc refresh) |
| Acceptance tests (concurrency) | 12 scenarios |
| Benchmarks (tx-related) | 14 functions across 2 crates |
| Clippy `-D warnings` | clean (pre-commit gate: `--workspace --all-targets`) |
| `cargo fmt --all --check` | clean |
| Crates touched (tx + audit) | 8 — Phase A's 6 (shamir-types, shamir-wal, shamir-tx, shamir-engine, shamir-query-types, shamir-storage) plus shamir-server + shamir-client from the security/NEW waves |
| New crates | 2 (`shamir-wal`, `shamir-tx`) |
| Documentation files | 12 in `docs/pre-transactional/` |

---

## 7. Ближайшие цели

### Critical — the original Stage-7 blockers are closed; two audit follow-ups are in flight

Closed and committed:
  - Crash recovery wired into the OPEN path (CRIT-A, `ff816d3`) — a
    durable inflight `WalEntryV2` is now replayed before the server
    serves; previously `recover_v2_inflight` existed but was never
    called on bootstrap.
  - MVCC version state restored from durable markers on open (CRIT-B,
    `ff816d3`) — `RepoTxGate` is seeded with
    `max(last_committed_marker, max_inflight)` so post-restart versions
    never regress and SSI keeps working across a restart.
  - SSI write-side conflict detection (Stage 5.1) and read-tracking
    plumbing (HIGH-C) — DONE; final SELECT-routing in flight (I.1, below).
  - GC (Stage 6.1-6.4) — DONE.

In flight (parallel tasks landing alongside this revision — see §11):
  - **I.1** executor `BatchOp::Read` tx-threading (closes the last SSI
    read-side gap).
  - **I.2** full index-config catalogue replay on recovery.

### High (enables clients)

**Stage 4.E — SDK updates** ✅ Already wired. `BatchRequest.isolation`
and `TransactionInfo` have been in the wire schema since Stage 4.A.

### Medium (long-tail correctness)

**Stage 5.rest — Reconciliation:**
- MemBuffer bypass (D3 implementation).
- Migration coordinator integration with tx.
- Audit log.
- Auto-verify watchdog.
- Repo-level `LayeredInterner` integration → closes limitations 2-5.

### Low (ops maturity)

**Stage 6 — GC + telemetry:**
- History store GC worker (drop versions older than `min_alive`).
- Prometheus / OpenTelemetry exporter.
- `max-tx-lifetime` cap.

**Stage 7.rest:**
- Multi-connection e2e harness.
- 12 concurrent scenarios from `08-tests-landing.md`.

---

## 8. Открытые вопросы

### Q1. Should `IndexWriteOp` carry a table token?

Currently `Vec<IndexWriteOp>` is global per tx. WAL emission resorts
to broadcast (`table_id_interned: 0`) for index ops. Per-op table
attribution would be cleaner but requires touching all 6 backends'
`plan_*` methods and `apply_index_ops`.

**Tradeoff:** broadcast works for InMemoryRepo (cheap), may not scale
for production backends with N tables × M postings replay cost.

**Resolution:** revisit in Stage 5 alongside `LayeredInterner`
integration that already touches `IndexWriteOp` for id-remap.

### Q2. `WalEntryV2` size cap?

Current emission can include arbitrary numbers of Put/Delete ops
(one per staged record). For a 10k-record batch the entry is ~1 MB.
RepoWalManager doesn't enforce a size limit.

**Tradeoff:** unbounded entries can cause OOM on recovery or memory
spike during Phase 4 begin. But realistic txs are small.

**Resolution defer to Stage 5:** add `max_tx_size_bytes` config
(default 64 MB). Exceeding → tx aborts with `tx_too_large`.

### Q3. Interaction with MemBuffer

`MemBuffer` is the existing write-back cache. Tx writes currently
bypass it (Phase 5 calls `base.transact` directly on data_store —
which may or may not include MemBuffer wrapper depending on the
backend stack).

**Question:** should tx commits warm MemBuffer's `dirty` set? Or
strictly bypass?

**Current answer (Stage 4):** strict bypass — commit IS the
durability point, no buffering tail. Test: `non_tx_writes_go_through_membuffer`
still works because non-tx Set call retains MemBuffer routing.

**Stage 5 work:** explicit `D3` test enforces "tx commit bypasses
MemBuffer, non-tx writes through MemBuffer, both visible after their
respective flush points".

### Q4. Phase 5 batching strategy

Currently each table in `tx.write_set` gets its own `base.transact(ops)`
call — N tables → N transact calls (each potentially fsync). Cross-table
atomicity is only at the **logical** level (WAL marker), not at the
physical-write level.

**Question:** is per-table transact acceptable? Or do we need a
multi-store transact primitive (one fsync covering all tables)?

**Current answer:** per-table acceptable. Recovery uses the WAL
marker as the source of truth — if any per-table transact succeeded
but a later one failed, recovery would re-apply the failed ones from
WAL on next open. Atomicity = "all visible together OR all
re-applied on next start".

### Q5. Default isolation `Snapshot` vs `Serializable`

`Snapshot` is default per current wire format. Documented in `D5`
+ §4.6. Last-writer-wins on conflicts; lost update possible under
contention.

**Question for prod ops:** should default be elevated to
`Serializable` once Stage 5 closes the conflict-detection gap? Or
keep user-explicit?

**Current stance:** keep `Snapshot` as default. SSI overhead is
non-trivial (read-set tracking + commit-time validation). Users
opt-in via `"isolation": "serializable"` for critical txs.

---

## 9. Уроки за два дня работы

### Что сработало

- **Атомизация на uniform sub-stages** (each landable, each <1 hour
  crush delegation). `cargo fmt + clippy + tests` зелёные на каждом
  шаге.
- **`docs/pre-transactional/*` план up-front.** Каждый этап имел
  спецификацию до начала кода. Crush'у можно было давать precise
  prompts.
- **Compromises закрыты честно.** Stage 4.G/H — не "TODO Stage 5",
  а реальные closure'ы с тестами.
- **Test-first для каждого решения.** D1-D9 имеют correctness tests;
  бенчмарки добавлены для hot paths (D5, D7 indirectly).
- **Honest production limitations.** Документированы в трёх местах
  (05-executor-isolation, FLAKY_TESTS, this REVIEW). Не скрыты.

### Что встретили как сложности

- **Crush envelope vs reality.** Несколько раз crush'евая работа
  была технически сделана но `git commit` не выполнился. Решение:
  всегда самостоятельная верификация diff'а + commit от руки если
  нужно.
- **API churn между sub-stages.** Например, `IndexWriteOp` без
  table token — обнаружилось только при wiring WAL recovery. Решилось
  through schema extensions + honest broadcast placeholder.
- **Network blips.** Несколько `crush run` упали на сети z.ai —
  retry в той же session обычно решал.

### Чему стоит научиться

- **Анти-pattern "schema set in stone".** `WalOpV2` enum пришлось
  расширять трижды (table_id в Put/Delete, потом в Index*). Уроки:
  thinking through ALL consumers of an enum at design time.
- **Дисциплина по `cargo bench`.** D5 obligation requires benches per
  decision. Мы их добавляли пост-фактум (4.H.2/3). Лучше — bench
  *рядом* с тестом на каждом sub-stage.

---

## 10. Финальное состояние

**Транзакции работают E2E** для SI (happy path тестируется
`execute_batch_transactional_si_happy_path`). Crash recovery wired into
the open path (CRIT-A) and MVCC version state restored from durable
markers on bootstrap (CRIT-B); acceptance suite covers the §7.2
scenarios.

**Serializable isolation:** the conflict-detection *machinery* is now
real end-to-end on the write side (5.1) and on the read side the
plumbing is committed (HIGH-C). It is **not yet** fully wired for plain
SELECTs until the executor read-routing task (I.1) lands — tracked as
in-progress in §11, not as "done". This is the one place the prior
revision of this doc over-claimed (§5.1 "CLOSED" while §10 said "not
production-ready"); the two statements are reconciled here and in §5.1.

**SDK:** `BatchRequest.isolation` + `TransactionInfo` have been in the
wire schema since Stage 4.A, so clients can already opt into
`serializable`.

**Ops maturity** (telemetry exporter, multi-conn / real-crash subprocess
harness) — see §11 follow-ups; non-blocking for SI.

---

## 11. Post-audit hardening (two review waves on top of Phase A)

After Phase A "landed", two adversarial audit passes scrutinised the tx
layer and the server for correctness and security holes the green test
suite was not catching. This section records what those waves CLOSED (all
committed — SHAs below) and, honestly, what they LEFT OPEN. The prior
revision of this doc leaned optimistic; the intent here is to correct
toward honesty, not to declare victory.

### Closed and committed

| ID | What was wrong | Fix | Commit |
|---|---|---|---|
| **CRIT-A** | `recover_v2_inflight` existed but was **never called** on the open path — a crash between commit Phase 4 (`wal.begin`) and Phase 7 (`wal.commit`) silently lost the committed tx on restart. | Recovery replay invoked from `shamir_db::init` (both open paths), before the server accepts requests; a recovery failure is propagated, not swallowed. | `ff816d3` |
| **CRIT-B** | MVCC version counter was re-initialised from scratch on open, so post-restart commit versions could regress below already-durable data → SSI breakage / version reuse. | `RepoTxGate` seeded with `max(LastCommittedVersion marker, max_inflight)` and `NextTxId` from the persisted marker on bootstrap. | `ff816d3` |
| **HIGH-A** | The non-tx unique-write path takes a per-table `unique_write_lock`; the tx commit path took only `commit_lock`. A non-tx unique write could interleave between a committer's Phase 2.6 unique re-check and its Phase 5c posting write → duplicate unique value. | Commit Phase 2.5 acquires each affected table's `unique_write_lock` (deterministic token order, ABBA-free under `commit_lock`) and holds it across 2.6→5c, then releases. Unifies the lock both paths contend on. | `c967317` |
| **HIGH-C** | SSI read-set was populated only from unit tests; in production `read_set` was empty at commit, so Serializable silently degraded to Snapshot. | `read_set` → `scc::HashMap`; added `record_read_shared(&self, …)`; wired `TableManager::read_one_tx` to record reads through the shared reference. (Final SELECT routing = I.1, in flight.) | `c967317` |
| **unique-under-commit_lock** | Stage-time `validate_unique_*` is optimistic; two concurrent txs claiming the same unique value both passed it. | `TxContext::unique_guards` + authoritative byte-equal `info_store.get(index_key)` re-validation under `commit_lock` (Phase 2.6). Closes the ACID hole. | `fbb4aac` |
| **HNSW relocation** | Vector staging lived in an adapter-held `scc::HashMap<TxId, …>` plus a `tables_with_hnsw_staging` set the tx carried so abort could broadcast a rollback. | Staging moved into `TxContext::staged_vectors`; abort = RAII drop, no broadcast. Commit Phase 5d promotes exactly the tx's footprint. | `6492c32` |
| **NEW-1** | WS pre-auth connections could send an arbitrarily large frame before authenticating (memory-amplification DoS). | Pre-auth frame size cap on the WS path (mirrors the TCP 4 KiB pre-auth cap). | `e199840` |
| **NEW-2** | Auth failures retried with no penalty. | Exponential backoff applied on the connection path. | `e199840` |
| **password-at-rest** | `CreateUser` stored the password in the clear. | Password hashed at rest. | `fc01a8e` |
| **flaky HNSW delete** | A non-deterministic HNSW delete test. | Made deterministic. | `fc01a8e` |

### Honest follow-ups still OPEN

Not bugs hiding behind green tests — known, documented stage-cut
boundaries. Listed so the next engineer doesn't rediscover them.

> **Update 2026-05-31 — most of this list has since CLOSED:**
> - **I.1** (executor read tx-threading) — ✅ closed (`230f8b5`).
> - **I.2** (index-config catalogue replay) — ✅ closed (`20ee9ed`).
> - **Perf `table_by_token`** (III.1) — ✅ closed (`20ee9ed`).
> - **Real-crash subprocess harness** (II.1) — ✅ closed (`783a7bf`); MED-A
>   extended it with a two-table redb reopen test.
> - **CI `--all-targets`** — ✅ closed (`55adef0`).
> - **Property / fuzz** — ✅ closed (`proptest` dev-dep sanctioned; version-
>   codec + SSI `validate_read_set` property tests landed 2026-05-31).
> - **MED-A** (cross-table physical atomicity) — **WONTFIX-by-design**: a
>   physical multi-store transact would leak backend identity; logical-WAL +
>   idempotent recovery is the correct backend-agnostic answer. See
>   [`../roadmap/PHASE_A_TAILS.md`](../roadmap/PHASE_A_TAILS.md) §1.
>
> Beyond Phase A, **Phase B** (interactive tx) and **Phase C** (phantom
> protection) shipped. The original bullets are kept below for the record.

- **I.1 — executor `BatchOp::Read` tx-threading** — *in progress
  (parallel task).* The read-tracking plumbing (HIGH-C) is committed; the
  remaining work routes a transactional SELECT through `read_tx` with a
  shared `&TxContext` so the `read_set` is populated end-to-end. Until it
  lands, SSI write-skew on plain SELECTs is detected only where a read
  path already threads the tx. May land minutes after this revision.
- **I.2 — full index-config catalogue replay on recovery** — *in
  progress (parallel task).* `replay_v2_op` replays Put/Delete/IndexPut/
  IndexDel/CounterDelta/InternerOverlayMerge against tables that already
  exist; a table token unknown to the repo at recovery time is logged and
  skipped. Replaying the index **configuration** (so a brand-new table's
  indexes are recreated, not just its postings) is the open part.
- **MED-A — Phase 5 physical-write atomicity** — per-table
  `base.transact` means N tables → N transact calls. Cross-table
  atomicity is logical (WAL marker), not a single physical fsync. Recovery
  re-applies on next open, so the invariant is "all visible together OR
  all re-applied", but a true multi-store transact primitive is not built.
- **Real-crash subprocess harness** — current crash-recovery tests
  simulate an inflight WAL entry in-process; a kill-9 subprocess harness
  that actually crashes mid-commit and re-opens is not yet written.
- **Perf — `table_by_token` on the commit hot path** — Phases 1/5b/5c/5d
  each resolve tables by token repeatedly; a per-commit resolution cache
  would cut redundant lookups. Not measured yet.
- **CI — `--all-targets` clippy** — the pre-commit gate runs
  `clippy --workspace --all-targets`; CI does not yet enforce the
  `--all-targets` breadth across every push.
- **Property / fuzz coverage** — SSI conflict detection and the
  version-codec are covered by example-based tests only; property tests
  (interleavings) and a codec fuzz target would harden them.

---

Документ обновляется по мере landing новых stages. Это Phase-A audit-запись;
ревизия 2026-05-31. С тех пор закрыты I.1/I.2/II.1/III.1/CI/property-fuzz,
MED-A — WONTFIX-by-design, и поверх Phase A приземлились **Phase B**
(interactive tx) и **Phase C** (phantom protection / true serializability).
Актуальный снимок проекта — [`../PROJECT_STATE.md`](../PROJECT_STATE.md).
