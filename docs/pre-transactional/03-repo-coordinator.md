# Этап 2. Per-repo tx coordinator

**Срок:** 5-6 дней. **Зависит от:** Этап 0 + 1.
**Блокирует:** Этап 3 (MvccStore), Этап 4 (executor).

Цель — собрать в одно место всё что нужно для управления жизненным
циклом одной транзакции внутри одного repo: счётчик версий, точку
синхронизации commit-фазы, контекст с буферами, журнал markerов.

## 2.1. `RepoTxGate`

Один экземпляр на repo. Не используется non-tx путём — нет hot-path
overhead.

```rust
pub struct RepoTxGate {
    /// Сериализует commit-фазу. Не блокирует read и не блокирует
    /// non-tx writes. Только короткий критический участок commit.
    commit_mutex: tokio::sync::Mutex<()>,

    /// Hot-path атомарный счётчик. Инкремент при каждом commit.
    version_counter: AtomicU64,

    /// Durable recovery marker. После публикации commit'а — пишется
    /// в info_store под SysKey::LastCommittedVersion. Reads видят
    /// только версии ≤ этого значения.
    last_committed_version: AtomicU64,

    /// Открытые snapshots — для GC min_alive расчёта.
    active_snapshots: scc::HashMap<u64, ()>,
}

impl RepoTxGate {
    pub async fn open_snapshot(&self) -> SnapshotGuard;        // RAII
    pub async fn commit_lock(&self) -> MutexGuard<'_, ()>;
    pub fn assign_next_version(&self) -> u64;
    pub async fn publish_committed(&self, version: u64) -> DbResult<()>;
    pub fn min_alive(&self) -> u64;                            // для GC
    pub fn last_committed(&self) -> u64;                       // для readers
}
```

**Recovery on open.** При открытии repo:
1. Прочитать `SysKey::LastCommittedVersion` из info_store → seed
   `last_committed_version`.
2. Прочитать `SysKey::NextVersion` (отдельный durable counter,
   обновляется реже) → seed `version_counter`. Если не совпадает с
   last_committed — берём max.

**Acceptance.**
- 100 concurrent `commit_lock + assign_next_version` — все версии
  монотонно растут, нет дубликатов.
- `SnapshotGuard` drop удаляет себя из `active_snapshots`.
- Recovery test: симулируем сохранение `last_committed = 42`,
  перезапускаем gate — `last_committed()` возвращает 42.

## 2.2. `TxContext`

```rust
pub struct TxContext {
    pub tx_id: u64,
    pub repo_id: RepoId,
    pub snapshot_version: u64,
    pub isolation: IsolationLevel,            // Snapshot | Serializable

    // pending writes — одно StagingStore на таблицу
    pub write_set: BTreeMap<TableName, StagingStore>,

    // pending index ops — applied atomically with write_set
    pub index_write_set: Vec<IndexWriteOp>,

    // HNSW staging — applied via HnswAdapter::commit_staged
    pub staged_hnsw_inserts: BTreeMap<TableName, Vec<StagedVector>>,

    // Interner overlay
    pub interner_overlay: scc::HashMap<String, u64>,

    // Per-table counter delta. Apply at commit: counter.add(delta).
    pub counter_delta: HashMap<TableName, i64>,

    // For SSI: (table, key) → version_seen
    pub read_set: HashMap<(TableName, Bytes), u64>,
}

pub enum IsolationLevel { Snapshot, Serializable }

impl TxContext {
    pub fn new(tx_id: u64, repo_id: RepoId, snapshot: u64, iso: IsolationLevel) -> Self;
    pub fn rollback(self); // drop = RAII rollback (just drops everything)
}
```

**Lifetime.** `TxContext` живёт в executor stack frame одного batch
запроса. Drop без commit = автоматический rollback (RAII).

**Cross-table read-write consistency.** `write_set` индексирован
по table — потому что один batch может затронуть N таблиц одного
repo. Read внутри tx (в любой таблице): сначала её
`write_set[table_name].get(key)`, потом `MvccStore::get_at(snapshot)`.

**Acceptance.**
- Round-trip: tx_context.write_set[t1].set + get → видит свой write.
- RAII drop test: создать tx_context, не commit'ить — никаких side
  effects не должно остаться в storage.

## 2.3. `LayeredInterner`

```rust
pub enum LayeredInterner<'a> {
    /// Non-tx путь. Чтения/писания идут напрямую в base. Zero overhead.
    Direct(&'a Interner),

    /// Tx путь. Чтения сначала проверяют overlay, потом base. Writes
    /// — только в overlay. Commit мерджит overlay → base.
    Layered {
        base: &'a Interner,
        overlay: &'a scc::HashMap<String, u64>,
        next_overlay_id: &'a AtomicU64,
    },
}

impl<'a> LayeredInterner<'a> {
    pub fn touch(&self, key: &str) -> u64;
    pub fn get_id(&self, key: &str) -> Option<u64>;
    pub fn get_str(&self, id: u64) -> Option<String>;
}

/// Вызывается под commit_lock. Atomic merge overlay → base.
pub async fn commit_interner_overlay(
    base: &Interner,
    overlay: &scc::HashMap<String, u64>,
) -> DbResult<()>;
```

**Тонкость с ID-collision.** Overlay slots выдают id из
`next_overlay_id`, который **отдельный** от base counter. На merge:
для каждой `(key, overlay_id)` в overlay — если `base.get_id(key)`
уже существует (другая tx уже добавила между snapshot и commit),
используем base's id; иначе аллокируем новый base id и пишем. Это
даёт **stable** id'ы после commit — что важно, потому что bytes
записанные с overlay_id должны разрешаться в правильную строку
после merge.

**Реализация id remap.** Если merge заменил overlay_id на другой
base_id, мы должны переписать соответствующие bytes в `write_set`
**до** flush. Это значит: interner merge — **первая** фаза commit'а,
до того как bytes уходят в transact. Остальные фазы используют
финальные ids.

**Acceptance.**
- `Direct` mode не аллоцирует ничего, проходит как plain `&Interner`.
- `Layered` mode: touch новой строки → попадает в overlay, base не
  видит.
- `commit_interner_overlay` — id-remap корректен под конкуренцией
  (две tx одновременно интернируют ту же строку).

## 2.4. Repo-level WAL

**Проблема.** Per-table WAL не годится для batch'а, затрагивающего
N таблиц одного repo: получится N независимых маркеров вместо одного
atomic commit point.

**Решение.** `RepoWalManager` — рядом с per-table WAL, не вместо.
Per-table WAL остаётся для non-tx ops (back-compat). Tx ops пишут
один `WalEntryV2` в repo-level WAL под `SysKey::RepoWalEntry(txn_id)`.

```rust
pub struct RepoWalManager {
    info_store: Arc<dyn Store>,  // shared info store one per repo
    next_txn_id: AtomicU64,
}

impl RepoWalManager {
    pub fn fresh_txn_id(&self) -> u64;
    pub async fn begin(&self, entry: WalEntryV2) -> DbResult<()>;
    pub async fn commit(&self, txn_id: u64) -> DbResult<()>;
    pub async fn list_inflight(&self) -> DbResult<Vec<WalEntryV2>>;
}
```

**Где жить.** Info store самого repo (не отдельный store). Префикс
`SysKey::RepoWalEntry(...)`.

**Recovery on repo open.**
1. `list_inflight` → entries с `commit` ещё не пришедшим.
2. Для каждого forward-fix: применить ops к main + history stores.
3. Удалить entry.

Если crash случился **до** `begin` — entries не было, мы только
потеряли inflight tx (она aborts автоматически).

Если crash случился **между** `begin` и `commit` — entry осталась
в WAL. На open она применяется → atomicity сохранена.

Если crash случился **после** `commit` — entry уже удалена. Tx
видна снаружи (writes в main store) — finalized.

**Acceptance.**
- Симуляция crash mid-commit: `begin → kill → restart`. На open
  ops V2 entry применены. Конечное состояние == expected.
- Симуляция crash после commit: `begin → commit → kill → restart`.
  Tx видна, нет дубля применения (idempotency).
- `list_inflight` после нормального flow возвращает пусто.

## Порядок работы

1. `RepoTxGate` + tests (1 день).
2. `TxContext` + RAII drop tests (1 день).
3. `LayeredInterner` + commit_interner_overlay + id-remap tests
   (1.5 дня).
4. `RepoWalManager` + recovery tests (1 день).
5. Integration: glue вместе — открытие repo сейчас создаёт `Gate`,
   `RepoWalManager`; на shutdown — корректно останавливается (0.5 д).
6. Property test: 100 concurrent virtual-commits против реального
   gate — все версии уникальны, recovery после абсорта чистый
   (1 день).

**Не делаем здесь:**
- Не пишем MvccStore (Этап 3).
- Не интегрируем в read path (Этап 3).
- Не делаем executor wiring (Этап 4).
