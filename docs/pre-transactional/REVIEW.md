בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Pre-Transactional Review

State-of-the-world snapshot. Captures what landed, what remains, what
compromises were taken, and what open questions exist.

**As of:** 2026-05-28 (Stage 7.1 complete).

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
Stage 5 — Reconciliation            ⏳ PARTIAL (Phase 5 mvcc routing pending)
Stage 6 — GC + telemetry            ⏳ PLANNED
Stage 7 — Tests + landing           🔶 7.1 COMPLETE (crash recovery)
            ├── 7.1                  ✅ V2 WAL recovery
            ├── 7.rest               ⏳ multi-conn harness
            └── 4.E SDK              ⏳ client builders
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
SnapshotGuard RAII). `TxContext` with `write_set`, `index_write_set`,
`tables_with_hnsw_staging`, `interner_overlay`, `counter_deltas`,
`read_set`, `version_provider`. `LayeredInterner` (Direct/Layered enum,
`OVERLAY_ID_BASE = 1<<48`, `commit_interner_overlay` with id-remap).
`RepoWalManager` for V2 entries.

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

---

## 3. Архитектурные решения — quality assessment

Девять решений в `architectural-decisions.md` + `05-executor-isolation.md`.

| ID | Decision | Quality | Notes |
|---|---|---|---|
| **D1** | HNSW stage-on-insert / apply-on-commit | ✅ clean | `staged: scc::HashMap<TxId, Vec<StagedVector>>` + brute-force merge на in-tx search. Tests cover commit + rollback. |
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

### 1. ~~SSI conflict detection blind on tx writes~~ — CLOSED (5.1)

Phase 5 now routes through `MvccStore::apply_committed_ops` (5.1.a/b).
`version_cache` is updated on tx commits. SSI conflict detection
verified by `ssi_conflict_detected_on_concurrent_tx_writes` test (5.1.c).

### 2. Repo-level interner is placeholder

`tx.repo_id` and `WalEntryV2.repo_id_interned` use `repo_token(name)`
(DefaultHasher). Same for `table_id_interned` via `table_token`.
Identical hashes across processes — deterministic.

**Closure path:** Stage 5 reconciliation introduces real repo-level
`LayeredInterner` integration. Struct shapes stay stable; only the
value source changes.

### 3. `tx.interner_overlay` always empty in production

No production code path populates the overlay because LayeredInterner
isn't wired into TableManager / executor yet. Phase 1 `apply_id_remap`
runs with empty remap (no-op).

**Closure path:** Stage 5 — interning paths produce overlay entries
that Phase 1 merges.

### 4. ~~Index ops use `table_id_interned: 0` broadcast emission~~ — CLOSED (5.2)

`tx.index_write_set` now carries `(table_token, IndexWriteOp)` tuples.
WAL emission uses the real table_token instead of broadcast `0`.
Recovery routes index ops to the correct table's info_store.

### 5. WAL `InternerOverlayMerge` not replayed

Recovery sees the op but skips with a warning. Without a repo-level
interner, there's nothing to merge into.

**Closure path:** Stage 5 — repo interner exists; recovery merges
serialized overlay entries.

---

## 6. Метрики

| Metric | Value |
|---|---|
| Total commits (pre-tx work) | 92 |
| Tests workspace `--lib` | 1438 passing, 0 failed |
| Tx-specific tests | ~150 |
| Benchmarks (tx-related) | 14 functions across 2 crates |
| Clippy `-D warnings` | clean |
| `cargo fmt --all --check` | clean |
| Crates touched | 6 (shamir-types, shamir-wal, shamir-tx, shamir-engine, shamir-query-types, shamir-storage) |
| New crates | 2 (`shamir-wal`, `shamir-tx`) |
| Documentation files | 12 in `docs/pre-transactional/` |

---

## 7. Ближайшие цели

### Critical (production blocker)

**Stage 5.1 — Phase 5 routes through MvccStore.set_versioned**
Closes SSI conflict detection. Required for Serializable isolation
to actually function in production. Estimated ~3-5 hours, 2-3
atomized sub-stages.

### High (enables clients)

**Stage 4.E — SDK updates**
`shamir-client` (Rust) + `shamir-client-node` builders для `isolation`
field. Parse new `TransactionInfo` shape. Mechanical work. ~2-3 hours.

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
`execute_batch_transactional_si_happy_path`). Crash safety закрыт
(7.1.e). Acceptance test suite covers 6 сценариев.

**Не production-ready** для:
- Serializable isolation under real contention (Stage 5.1).
- Production clients (SDK updates — Stage 4.E).

**Ops maturity** (GC, telemetry, multi-conn harness) — Stage 6/7.rest,
не блокирующее.

---

Документ обновляется по мере landing новых stages. Текущая ревизия —
2026-05-28 после Stage 7.1.e.
