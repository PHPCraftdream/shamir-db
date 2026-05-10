# Transactions — Design

Status: **planned, not yet implemented**. The `BatchRequest.transactional`
flag exists in the wire schema but the executor currently ignores it and
always returns `transaction: None`. This document is the contour for
turning that hollow promise into reality.

> **Note on prior drafts.** Earlier revisions of this file proposed
> "rely on the storage backend's native MVCC (redb / persy / canopy)".
> That direction is **abandoned** — it leaks backend identity into the
> abstraction, gives different semantics depending on which engine
> backs each repo, and breaks the whole point of the unified
> `Store` / `Repo` traits. The current direction below builds the
> transactional layer **above** the trait, on the dumb-KV API — every
> backend behaves identically.
>
> Implementation analysis (current code recon, type changes,
> zero-overhead non-tx path, concurrent-test scenarios) lives in a
> companion file: [`TRANSACTIONS_IMPL.md`](./TRANSACTIONS_IMPL.md).

---

## Что это значит — по-русски

### Где была ошибка

Первая мысль была — «у redb уже есть MVCC, давайте его пробросим
наверх». Это сразу даёт несколько проблем:

1. **Утечка абстракции.** Клиент должен знать, какой бэкенд под
   капотом. Транзакция работает на одном repo, не работает на другом.
2. **Разная семантика.** sled-репо и redb-репо ведут себя
   по-разному. Spec на транзакции становится «зависит».
3. **Теряется смысл `Store` / `Repo` trait'ов.** Их задача — спрятать
   особенности backend'а. А мы наоборот их выпускаем.

### Куда идём

Транзакционный слой строится **поверх простого KV-интерфейса**.
Backend остаётся «тупым» хранилищем — атомарный `set`, range scan,
ничего больше. Вся транзакционная логика живёт в engine layer.
Любой backend (даже `DashMap` без какого-либо tx API) получает
транзакции одинаково.

### Как это устроено

Идея простая и хорошо известна — так делают PostgreSQL,
FoundationDB, etcd, CockroachDB.

**Не пишем `key → value`. Пишем `key:version → value`.**
Storage хранит много версий каждой записи. Engine сам управляет
видимостью.

#### Три кита

**1. Версионированный layout.** Физический ключ —
`<original_key>::<version_u64_be>`. Range scan по префиксу даёт всю
историю. Чтобы получить «значение на момент `version=V`» — берём
наибольшую версию ≤ V (один range query, O(log n) у любого
ordered KV).

**2. Логические часы.** Один атомарный счётчик per repo (хранится в
служебном ключе `__version_counter__`). Каждый commit берёт
`next_version = counter.fetch_add(1)`. Поскольку commit-фаза идёт
через единый Mutex per repo (см. ниже), increment безопасен.

**3. Working set + commit.** Транзакция:

- На `begin` — берёт `snapshot_version = current_counter` (без
  увеличения).
- Writes идут в `Map<Key, Value>` в памяти (working set).
- Reads сначала смотрят в working set (свои writes — first); потом
  в storage (наибольшая версия ≤ snapshot_version).
- Commit: берёт `new_version`, проверяет конфликты (см. ниже),
  пишет всё working set с новой версией.

#### Изоляция — два уровня сложности

**A. Snapshot Isolation + last-writer-wins (простое):**
- Reads видят snapshot.
- Writes на commit пишутся с новой версией — overwrite.
- Lost update возможен (T1 и T2 обе писали в `X`; чья запись позже —
  её версия выше — побеждает).
- Достаточно для большинства кейсов; быстрее.

**B. Serializable Snapshot Isolation (SSI):**
- На read запоминаем `(key, version_seen)` в read-set транзакции.
- На write добавляем в write-set.
- На commit проверяем: для каждого `(key, version_seen)` из read-set —
  `current_max_version(key) == version_seen`? Если другая tx
  обогнала — abort, retry.
- Никаких lost updates. Полный serializability.

A проще и достаточно по умолчанию. B можно включать опционально для
данных где lost update недопустим (счётчики, балансы, бронирования).

#### Single-writer commit-gate

Чтобы избежать гонки на counter и упростить conflict detection —
держим **в engine один Mutex per repo для commit-фазы**. Не на reads,
не на writes-в-память — только на 5-миллисекундный commit (assign
new_version, conflict check, batch write).

Это инженерный single-writer, не зависящий от backend. Как раз то
поведение, которое мы хотели от redb — но теперь у нас оно есть **на
любом backend**.

#### Garbage Collection

Старые версии копятся → диск растёт. Нужен GC:

- Background task раз в N секунд.
- Знаем `min_alive_snapshot` = минимальный snapshot всех живых tx.
- Все версии < `min_alive_snapshot`, кроме последней per ключ — можно
  удалить.
- Простая реализация: range scan + delete batch.

#### Crash recovery

- Counter в storage → восстанавливается на open.
- Working set ушёл при crash → транзакция aborted (поведение по
  умолчанию).
- Уже committed данные на диске → видны.
- Ключ `__last_committed_version__` атомарно обновляется в самом
  конце commit (после всех writes); reads видят только версии
  ≤ `__last_committed_version__`. Это решает «писали 5 ключей, crash
  после 3-го» — без атомарного маркера новые версии «не видны»,
  значит с точки зрения внешнего мира commit'а не было.

### Что остаётся от backend

Только три гарантии:

1. **Atomic single put** — `set(key, value)` либо весь либо никакой.
   Все наши backends это дают (это базовое свойство любого KV).
2. **Range scan** — `iter_stream` уже есть.
3. **Atomic CAS на одной записи** — для counter increment. Если
   backend не даёт CAS, коммит-mutex в engine закрывает дыру: внутри
   mutex'а простой `read counter → increment → write counter` —
   эквивалентен CAS.

Никакого «настоящего MVCC от backend'а» не требуется. Backend = dumb KV.

### Цена этой красоты

- **+50% storage overhead** на metadata (версии в ключах, история).
- **GC процесс в фоне** — ещё одна вещь, которая может ломаться.
- **Сложность в engine** — реальная, не маленькая (~ 2-3 недели
  аккуратной работы, не 2 дня).
- **Чтения чуть медленнее** — нужен range scan вместо point lookup.
  Можно компенсировать кешем «current version per key».

Взамен:

- Один интерфейс `Store` остаётся одним интерфейсом.
- Все backends работают одинаково (с точки зрения нашего пользователя).
- Семантика транзакций — наша спецификация, не наследие redb / persy /
  sled.
- Можно добавить любой новый backend (RocksDB, LMDB, S3) без
  переписывания tx-логики.
- Тесты транзакций гоняются на in-memory backend → быстрые.

### Что это меняет в дизайне

- `Store` trait не растёт — никаких `begin_write_tx()`. Backend ничего
  не знает о транзакциях.
- В engine появляется новый слой: `MvccStore` — обёртка над
  `Arc<dyn Store>`, который кодирует версии в ключах.
- `TransactionContext` хранит snapshot_version + working set.
- Engine commits идут через `RepoTxGate { mutex, version_counter }`.
- Background `GcWorker` per repo, тикает по таймеру.

Это «движок транзакций» как самостоятельный модуль внутри
`shamir-engine`, а не feature storage layer.

---

## Why two phases

The value cleanly splits into two levels of ambition:

- **Phase A — single-batch transactions.** All operations inside one
  `client.execute(db, batch)` call run atomically. No server-side
  state between calls. Drop-in for the existing API.
- **Phase B — interactive (multi-call) transactions.** A client starts
  a transaction, sends a sequence of independent batches, then commits
  or aborts. Requires session-scoped server state and abort-on-disconnect
  plumbing.

Phase A is enough for ~80% of transactional needs (atomic multi-record
write, read-after-write within the same operation). Phase B is needed
only when a human is in the loop or when client logic spans multiple
network round-trips against the same consistent snapshot.

Phase A first. Phase B when there is concrete demand.

---

## What we require from the storage backend

Already supplied by every backend in `shamir-storage` today:

- `set(key, value)` is atomic — partial writes never observable.
- `iter_stream(prefix)` (or full `iter()`) returns a stable scan.
- Reads are not torn (no half-written value visible).

Optionally helpful but not required:

- Atomic compare-and-swap. Used as a fast path for counter increment;
  fall back to commit-gate mutex if absent.
- Bulk `set_many` / batch op. Used as a fast path for committing a
  large working set; fall back to N sequential `set` calls.

We do **not** require:

- Native transactions / WriteTxn / ReadTxn primitives.
- Snapshot isolation in the backend's read API.
- Lock manager / row locks.
- Versioning / time travel.
- Any form of MVCC at the storage layer.

The trait surface stays the way it is. The transactional engine sits
above it.

---

## Phase A — what gets built

### Engine-side modules (new)

```
crates/shamir-engine/src/tx/
├── mvcc_store.rs        — Arc<dyn Store> wrapper that encodes versions
│                          in physical keys
├── version_codec.rs     — `<key>::<version_be_8>` encode / decode
├── tx_context.rs        — snapshot_version + read-set + write-set
├── repo_tx_gate.rs      — per-repo Mutex + version counter (durable)
├── recovery_marker.rs   — `__last_committed_version__` read/write
├── gc.rs                — background "compact below min_alive_snapshot"
└── mod.rs
```

### MVCC store API (engine-internal)

```rust
pub struct MvccStore {
    inner: Arc<dyn Store>,
    table_name: String,
}

impl MvccStore {
    /// Read at a specific snapshot — returns the highest committed
    /// version with `version <= snapshot_version`.
    pub async fn get_at(&self, key: &[u8], snapshot: u64) -> DbResult<Option<Bytes>>;

    /// Write a new version. Caller is responsible for picking
    /// `version` from `RepoTxGate`.
    pub async fn put_versioned(&self, key: &[u8], value: Bytes, version: u64) -> DbResult<()>;

    /// Iterate live records at a given snapshot — for SELECT queries
    /// inside a tx.
    pub fn scan_at(&self, snapshot: u64) -> impl Stream<Item = ...>;
}
```

### Tx context flow

```rust
let gate = repo.tx_gate(); // Arc<RepoTxGate>
let snapshot = gate.current_committed_version();

let mut ctx = TxContext::new(snapshot);

// during execution
let value = ctx.read(&store, key).await?;   // working_set first, then storage
ctx.write(key, value);                       // working_set only

// at commit
let lock = gate.commit_lock().await;          // single-writer fence
let new_version = gate.assign_next_version();
if isolation == Serializable {
    gate.validate_read_set(&ctx.read_set, snapshot)?;  // SSI check
}
for (key, value) in ctx.write_set {
    store.put_versioned(key, value, new_version).await?;
}
gate.publish_committed(new_version).await?;   // updates __last_committed_version__
drop(lock);
```

### Executor integration

Same shape as before: `transactional: true` on the batch opens a
`TxContext`, runs queries serially against it, commits/rollbacks at
the end, and fills in `BatchResponse.transaction = Some(...)`.

Cross-repo rule still applies: a transactional batch must target a
single repo (each repo has its own gate + counter). Multi-repo
atomicity stays out of scope (it would need 2PC at the engine layer —
real but separate work).

### Test coverage

In `tests/e2e/tests/11-transactions.test.js`:

- happy path: 5 writes + 1 read, commit, read back from outside, see all writes
- abort path: 2 writes + deliberately failing op → table unchanged
- read-after-write inside the same tx
- isolation: long tx + parallel write from another connection;
  tx doesn't see the parallel write; outside readers do
- conflict (when SSI is enabled): two tx read same key, both write
  it, second one aborts with `tx_conflict`
- single-repo enforcement: tx batch touching two repos → `not_supported`

Plus pure-Rust integration tests in `crates/shamir-engine/tests/`:

- VersionCodec round-trip
- MvccStore.get_at picks the right version under a busy history
- RepoTxGate.assign_next_version is monotonic under concurrent calls
- GC respects min_alive_snapshot
- Recovery marker: simulated crash mid-commit → reads see pre-tx state

The crucial property: **all tests run uniformly on every backend**
(InMemory, Sled, Redb, Fjall, Persy, Nebari, Canopy) via a backend
parameter — no per-backend test code. Same tx semantics everywhere.

---

## Phase B — interactive transactions (later)

Sketch only.

```
client.beginTx()                           → { txId }
client.execute(db, batch, { txId })        → BatchResponse
client.commitTx(txId)                      → ok
client.abortTx(txId)                       → ok
```

Server-side state per session:

```rust
struct ActiveTx {
    tx_id: u64,
    repo_path: (String /* db */, String /* repo */),
    ctx: TxContext,                  // snapshot + working set
    expires_at: Instant,             // server-side TTL
    last_activity_ns: u64,
}
```

Hard parts (familiar territory now that the engine machinery exists):

- **Lease management** — TTL ~30 s, configurable.
- **Disconnect → abort** — TCP close handler drops `ActiveTx`.
- **Protocol echoes `tx_id`** in `RequestEnvelope` so dispatcher
  routes correctly.
- **One tx per session** — keep nesting / parallel tx out (YAGNI).
- **Heartbeat op** — for tx that need to outlive the default TTL.

---

## Risks and corners

- **Interner persistence ordering.** TableManager's string interner
  lazy-persists to the `__info__` store. Inside a tx, intern slots
  added during execution must EITHER ride the same versioned write
  set (so they roll back atomically) OR be deferred until after
  commit. Latter is simpler and safe — interner slots are monotonic;
  leaking a slot is harmless, only costs a few bytes.
- **Index updates.** Secondary indexes also need versioned writes
  (otherwise an aborted tx leaves orphan index entries). Index store
  uses the same MvccStore wrapper.
- **Audit log.** Audit chain lives in a different repo entirely;
  cross-repo atomicity is out of scope. Pragmatic compromise: append
  the audit entry **after** successful commit, with `outcome:
  aborted` for failed transactional batches.
- **GC lag.** If GC falls behind, disk grows. Telemetry: emit
  `tx_versions_per_key` histogram + `gc_lag_versions` gauge so
  operators see this.
- **Long-running tx blocks GC.** Min_alive_snapshot is held back by
  the oldest open tx. Phase B in particular needs a max-tx-lifetime
  cap (e.g. 5 min) — otherwise one stuck client wedges the whole
  GC pipeline.
- **Read amplification.** Versioned reads do range scans where
  point reads sufficed before. Mitigation: cache `(key →
  current_version)` in memory per repo; invalidate on commit.

---

## Order of work (when we pick this up)

1. `version_codec.rs` + tests — encode / decode / sort order (1 h)
2. `repo_tx_gate.rs` — Mutex + counter + recovery marker, with
   crash-mid-commit simulated tests (3 h)
3. `mvcc_store.rs` — wrap a `Store`, implement `get_at` / `put_versioned`
   / `scan_at` (4-6 h, depends on prefix-scan ergonomics per backend)
4. `tx_context.rs` — read-set / write-set / read-through-working-set (2 h)
5. Executor integration — single-repo check, serial run, commit/abort,
   `BatchResponse.transaction` filled in (3-4 h)
6. Interner-persist deferral fix (1 h)
7. IndexManager port to MvccStore (2-3 h)
8. GC worker (3 h, including telemetry hooks)
9. Phase A boundary: SI + last-writer-wins (cheap default).
   Optional: SSI mode behind a flag (4 h).
10. Rust integration tests + e2e `11-transactions.test.js` (3-4 h)
11. Docs: this file marked "implemented", LOGIC_FLOW updated, root
    README capability list updated (30 min)

Total: roughly 2-3 weeks of focused work. Phase A only.

The cost is real, but the architectural payoff is also real: every
backend behaves identically, the `Store` trait stays narrow, and
adding a new backend tomorrow doesn't touch a single line of tx code.
