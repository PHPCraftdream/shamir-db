# 00. Обзор: карта препятствий и план подготовки

## Карта нынешнего состояния (что мешает MVCC сейчас)

### A. `Store` trait

Есть: `set/get/remove`, `iter_stream`, `scan_prefix_stream`,
`iter_range_stream`, `flush`, `set_many` / `remove_many` /
`insert_many` (атомарны только within одного типа на тех бэкендах
что переопределяют).

Нет:
- **Mixed-op атомарный batch** — `[set k1; remove k2; set k3]` одним
  атомарным актом. Без этого commit-фаза не «одна точка сбоя».
- **CAS** (`compare_and_swap(key, expected, new) -> bool`) — нужен
  для version counter без mutex.

### B. Keyspace

Префиксы (`b"__index__"`, `b"__sorted_index__"`,
`b"__buffer_config__"`, `b"__internals__"`, `b"__counter__"`,
`b"__meta__/..."`, `b"__wal__/..."`, `b"__index2__"`) раскиданы
string-literal'ами по коду. **При MVCC каждое из этих мест должно
превратиться в `<key>::<version>`** — без централизации это trawl
по всему репо.

### C. WAL

Есть: per-таблица `WalManager`,
`WalEntry { txn_id, started_at_ns, counter_delta, ops }`,
`begin/commit/commit_async/list_inflight`, recovery on open.

Нет:
- **Repo-scoped** WAL — одного маркера на batch, который трогает
  две и более таблицы.
- **Inline body** в `WalOp` — сейчас op это просто
  `RecordCreated { record_id }`; recovery читает запись **из
  data_store**. Для MVCC это не пройдёт: tx-uncommitted writes
  ещё нет в data_store на момент crash → recovery их не увидит,
  atomicity сломана. Либо WAL хранит full bytes, либо есть
  отдельный staging store.

### D. MemBuffer

Текущая MemBuffer — это **write-back cache** для перформанса
(`dirty: DashMap<RecordKey, Slot>`, shared между всеми writers,
background flusher). Это **не** transactional staging:

- Нет per-transaction isolation в `dirty`.
- Нет API «atomic flush этих ключей / drop этих».
- Flushes в background независимо от commit/abort.

Архитектурно MemBuffer и tx staging — **разные слои**. MemBuffer над
storage (perf), tx staging над MemBuffer (atomicity), MVCC store
сбоку (versioned reads). Не пытаемся объединить.

### E. Index2 / IndexManager

Все три index2 backend (`fts_backend`, `fts_ranked_backend`,
`functional_backend`) пишут **напрямую** `self.store.set(...)` /
`self.store.remove(...)` внутри `on_insert/update/delete` хуков.
Откатить такие writes без записанного журнала невозможно. То же
касается старого `IndexManager::on_record_*`.

### F. HNSW (КРИТИЧЕСКОЕ)

`hnsw_rs::Hnsw::insert` **необратим** — нативного `remove` нет.
Сейчас у нас soft-delete через `deleted: scc::HashMap<usize, ()>`
tombstones. Это значит: на abort транзакции вставленные точки
**останутся в графе**, лишь будут отфильтрованы при search через
tombstones. Долгосрочно — утечки памяти + деградация recall
(overscan ×2 не спасёт при многих aborts).

### G. Interner

`Interner::touch_ind` сразу глобально видим (даже для других
connections). В tx нужен **overlay**, видимый только tx — merge в
base на commit, drop на abort. Дизайн (`LayeredInterner`) есть в
`TRANSACTIONS_IMPL.md`, в коде нет.

### H. RecordCounter

`AtomicU64` in-memory + persisted `__counter__`. Read/write напрямую.
В tx — `counter_delta` в `TxContext`, apply at commit. Дизайн есть,
кода нет.

### I. Read pipeline

`TableManager::get/iter_stream` не знает про tx context. В tx нужно:
lookup в working_set first, потом snapshot read через
`MvccStore::scan_at(snapshot)`. Это пройдёт **через каждое** место
в planner / read_exec / index_lookup. Большая поверхность.

### J. Background tasks

- MemBuffer flusher
- Auto-verify watchdog
- Будущий GC worker

Все работают с storage напрямую, не зная про tx. Должны работать
только на **committed** snapshot. Если они увидят in-flight tx data
— будут "fixing" недопубликованные changes.

### K. Migration

`MigrationCoordinator` пишет shadow log, копирует data_store. Не
tx-aware. Concurrent migration + open tx на src дадут inconsistent
dst. Нужно либо pause migration во время tx, либо migration сама
становится tx.

### L. Test infrastructure

E2E orchestrator поднимает **один** сервер на весь прогон.
Multi-connection (с разными session) пока не делается. Без этого
невозможно проверить isolation properties.

### M. Wire format

`BatchRequest.transactional: bool` есть, но executor игнорирует.
Нет `isolation`, нет `tx_id`. Нужно расширять wire schema + Node
SDK + Rust SDK + audit logs.

---

## Этапы подготовки

Каждый этап **независим** и **не ломает существующее** (zero overhead
для non-tx).

| Этап | Что | Документ | Срок |
|---|---|---|---|
| 0 | Foundations (keyspace, transact, CAS, WAL inline body) | [01-foundations.md](./01-foundations.md) | 3-4 д |
| 1 | Write isolation layer (`IndexWriteOp`, HNSW staging, `StagingStore`) | [02-write-isolation.md](./02-write-isolation.md) | 7-8 д |
| 2 | Per-repo tx coordinator (`RepoTxGate`, `TxContext`, `LayeredInterner`, repo WAL) | [03-repo-coordinator.md](./03-repo-coordinator.md) | 5-6 д |
| 3 | MVCC store + read pipeline через `Option<&TxContext>` | [04-mvcc-store.md](./04-mvcc-store.md) | 6-7 д |
| 4 | Executor + SI / SSI + cross-repo guard | [05-executor-isolation.md](./05-executor-isolation.md) | 4-5 д |
| 5 | Reconciliation (MemBuffer, migration, audit, watchdog) | [06-reconciliation.md](./06-reconciliation.md) | 4 д |
| 6 | GC + telemetry + max-lifetime cap | [07-gc-telemetry.md](./07-gc-telemetry.md) | 3 д |
| 7 | Tests + wire format + docs | [08-tests-landing.md](./08-tests-landing.md) | 5-6 д |

**Итого** ~6-7 недель сфокусированной работы. После — сам MVCC по
плану в `TRANSACTIONS.md` занимает ~2 недели чистого кода.

## Главные архитектурные решения

Эти пять решений надо зафиксировать **до** старта Этапа 0:

1. **HNSW: stage-on-insert, apply-on-commit.** Без этого abort даёт
   permanent graph pollution. Меняет contract `VectorAdapter`.
2. **Index writes: planner-возвращает-ops, executor-применяет.**
   Без этого нет atomicity для индексов.
3. **MemBuffer и tx staging — разные слои.** Не объединять.
4. **Repo-scoped WAL marker, не table-scoped.** Один маркер per tx
   → одна точка atomic publish.
5. **`Option<&TxContext>` параметр пронизывает весь write+read
   pipeline.** Не generic dispatch — compile-time бинарное
   раздувание не оправдано.

Decision log с обоснованиями каждого решения — в
[architectural-decisions.md](./architectural-decisions.md).
