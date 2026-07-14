# Transactions — Implementation Analysis

Companion doc to [`TRANSACTIONS.md`](./TRANSACTIONS.md). Where that
file gives the *what* (engine-managed MVCC over plain KV), this one
gives the *how* — concrete recon of today's machinery, the minimal
type / engine changes needed, the rule that keeps the non-transactional
path at zero overhead, and the test scenarios that will prove the thing
actually works under contention.

Status: **superseded by `docs/dev-artifacts/pre-transactional/`**. The implementation
landed across Stages 0-7 in 92+ commits. See `docs/dev-artifacts/pre-transactional/REVIEW.md`
for the definitive state-of-the-world.

---

## Что показывает текущая машинерия

### Слои сверху вниз

```
TableManager   ──>  Table       ──>  Arc<dyn Store>   (data_store: __data__<table>)
            │
            ├──>  InternerManager  ──>  Arc<dyn Store>   (info_store: __info__<table>)
            │       — DashMap<String, u64>
            │       — persist() пишет весь словарь под RecordId::system("internals")
            │
            ├──>  RecordCounter    ──>  Arc<dyn Store>   (info_store)
            │       — атомарный счётчик в памяти
            │
            └──>  IndexManager     ──>  Arc<dyn Store>   (info_store)
                  — on_record_created / on_record_updated / on_record_deleted
                  — каждый хук пишет 1+ ключей в info_store
```

`Store::set/get/remove/insert` принимают `RecordKey = Bytes` (variable
length) — это даёт полную свободу кодирования версий внутри ключа без
менять trait.

### Где транзакции пройдут гладко

- **`Store::set/get/insert/remove`** — атомарная единица записи у любого
  backend'а. На этом строим всё.
- **`Table`** — тонкая обёртка (один `Arc<dyn Store>`). Перехват через
  `MvccStore` тривиален.
- **`__data__` / `__info__` разделение** — system-state физически
  отделён от пользовательских записей. Можно версионировать с разной
  частотой GC и независимо.

### Где станет больно

1. **Interner — shared mutable state.** `interner.touch_ind(key)` СРАЗУ
   модифицирует in-memory словарь. Если tx абортит, abort'нутые слоты
   остаются в памяти и в следующем `persist()` улетят на диск. Нужен
   **overlay** (см. ниже).
2. **`execute_set` сейчас сканирует весь store** для поиска по
   key-fields → `O(n)`. В транзакции с 50 set'ами получаем `O(50·n)`.
   Это уже сейчас плохо — под tx катастрофа. Нужна secondary lookup
   тропа (через index, либо через PK convention).
3. **`query_value_to_inner` модифицирует interner на каждый Value→Inner.**
   Без overlay tx aborts оставляют грязь в interner.
4. **IndexManager пишет 1+N записей в info_store на каждое data write.**
   Эти писания идут **отдельной кодовой ветвью**, не через `Table`.
   Чтобы tx был атомарным — индексные writes тоже надо гнать через
   `MvccStore` и буферизовать в одном write_set с data writes.
5. **RecordCounter — atomic counter в памяти + persist отдельно.** Если
   abort — откат счётчика неудобен (`fetch_sub`?). Решение: counter
   увеличивается **at commit time** одной операцией, а не при каждом
   insert. Это перенос assignment в commit-фазу.

---

## Фундаментальные изменения в типах и движках

### Минимум необходимое для Phase A

#### 1. `Store` trait расширить: `iter_prefix_stream(prefix, batch_size)`

Без этого нельзя сделать range-scan по `<key>::<version>`. Все наши
backends физически это умеют (B-tree / LSM упорядочены), просто метод
не выставлен. Default impl — через `iter_stream` + filter (медленно,
но работает). Нативные impls на каждом бэкенде используют их `range()`
API.

```rust
#[async_trait]
pub trait Store: Send + Sync {
    // existing
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey>;
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool>;
    async fn get(&self, key: RecordKey) -> DbResult<Bytes>;
    async fn remove(&self, key: RecordKey) -> DbResult<bool>;
    fn iter_stream(&self, batch_size: usize) -> RecordStream;

    // NEW
    fn iter_prefix_stream(&self, prefix: &[u8], batch_size: usize) -> RecordStream {
        // default: full scan + filter — works but slow
        scan_with_filter(self, prefix, batch_size)
    }
}
```

Каждый бэкенд переопределяет если умеет нативно (redb / sled / fjall —
все умеют). Default-impl остаётся как fallback и для тестов.

#### 2. `MvccStore` wrapper в engine — НЕ меняет `Store` trait

```rust
pub struct MvccStore {
    main:    Arc<dyn Store>,           // current values: key → value
    history: Arc<dyn Store>,           // old versions: key+version → value
    cache:   DashMap<Bytes, u64>,      // key → current_version (memory-only)
    repo_gate: Arc<RepoTxGate>,
}
```

«Current + history» layout — об этом ниже в разделе «как не
замедлить не-tx».

#### 3. `LayeredInterner` для tx-local intern slots

```rust
pub enum LayeredInterner<'a> {
    Direct(&'a Interner),                                  // non-tx, zero cost
    Layered { base: &'a Interner, overlay: DashMap<String, u64> }, // tx
}

impl<'a> LayeredInterner<'a> {
    pub fn touch(&self, key: &str) -> u64 {
        match self {
            Self::Direct(i)             => i.touch_ind(key).unwrap().key().id(),
            Self::Layered { base, overlay } => {
                if let Some(id) = base.get_id(key)        { id }
                else if let Some(id) = overlay.get(key)   { *id }
                else { /* allocate new id, insert into overlay */ }
            }
        }
    }
}
```

Без tx — `Direct(&interner)`, читаем/пишем напрямую в base, ноль
накладных. В tx — `Layered`, новые slots в overlay; на commit —
atomic merge overlay → base + persist; на abort — drop overlay.

#### 4. `TxContext`

```rust
pub struct TxContext {
    pub snapshot_version: u64,
    pub repo_id: RepoId,
    pub isolation: IsolationLevel,                          // SI | SSI

    // pending state
    pub write_set: HashMap<TableName, HashMap<Bytes, Bytes>>, // data
    pub index_write_set: HashMap<TableName, Vec<IndexUpdate>>, // index ops
    pub interner_overlay: DashMap<String, u64>,
    pub counter_delta: HashMap<TableName, i64>,             // tx-local count change

    // for SSI conflict check
    pub read_set: HashMap<(TableName, Bytes), u64>,         // (key → version_seen)
}
```

Drop без commit — RAII rollback (просто сбрасывает структуру).

#### 5. `RepoTxGate` — per-repo синхронизация commit-фазы

```rust
pub struct RepoTxGate {
    commit_mutex: tokio::sync::Mutex<()>,
    version_counter: AtomicU64,                       // hot path
    last_committed_version: AtomicU64,                // recovery marker
    active_snapshots: DashMap<u64, ()>,               // for GC's min_alive
}
```

#### 6. `TableManager` — методы принимают `Option<&mut TxContext>`

```rust
impl TableManager {
    // before:
    pub async fn execute_set(&self, op: &SetOp) -> DbResult<WriteResult>;

    // after:
    pub async fn execute_set(
        &self,
        op: &SetOp,
        tx: Option<&mut TxContext>,
    ) -> DbResult<WriteResult>;
}
```

При `tx: None` — старый fast path, ноль изменений в поведении. При
`tx: Some(...)` — все writes идут в `tx.write_set`, все reads видят
tx-local view.

#### 7. `IndexManager::on_record_*` — тоже принимают `Option<&mut TxContext>`

Index updates не пишутся в info_store напрямую — они буферизуются в
`tx.index_write_set`. Один commit публикует всё сразу.

#### 8. Background `GcWorker` per repo

```rust
pub struct GcWorker { repo_gate: Arc<RepoTxGate>, history: Arc<dyn Store>, period: Duration }
```

Tikает по таймеру; смотрит `min(active_snapshots)`; range-scan
`history` store; удаляет всё < min_alive. Рассеивается по бэкендам
через trait — никакой backend-specific логики.

### Чего не нужно менять

- `Store` trait API меняется только +1 методом, и тот с default impl.
- Backend impls (sled / redb / fjall / persy / nebari / canopy /
  in_memory / cached) — без изменений.
- Wire protocol (`BatchRequest.transactional` уже есть).
- Client SDK (поведение прозрачно: tx flag в batch и tx info в
  response).

---

## Как НЕ замедлить не-транзакционный код

Это краеугольный камень. Решение — **«current + history» layout**
вместо «pure versioned keys».

```
                     ┌──────────────────────────────────────┐
NON-TX (steady):     │  main.set(key, value)                │ ← 1 write, как сейчас
                     │  main.get(key)                       │ ← 1 read,  как сейчас
                     └──────────────────────────────────────┘

TX (any active):     ┌──────────────────────────────────────┐
                     │  if active_snapshots_below(cur_ver): │
                     │    history.set(key+v, old_value)     │ ← copy old aside
                     │  main.set(key, new_value)            │ ← then update
                     └──────────────────────────────────────┘
```

### Ключевое условие zero-overhead

**Если ни одной активной tx нет — `history` store не пишется вообще.**

```rust
// в MvccStore
async fn set_with_versioning(&self, key: Bytes, value: Bytes) -> DbResult<()> {
    if self.repo_gate.has_active_snapshots_below(self.repo_gate.current_version()) {
        // нужно сохранить старую версию для активных tx
        if let Ok(old) = self.main.get(key.clone()).await {
            let old_version = self.cache.get(&key).map(|v| *v).unwrap_or(0);
            self.history
                .set(encode_key_version(&key, old_version), old)
                .await?;
        }
    }
    self.main.set(key.clone(), value).await?;
    let new_version = self.repo_gate.assign_for_commit();
    self.cache.insert(key, new_version);
    Ok(())
}
```

`has_active_snapshots_below` — атомарный read на одном AtomicUsize или
проверка `DashMap::is_empty()` (одна L1-кэш строка). В steady-state без
открытых tx → false → ветка истории мёртвая. Всё ровно как сейчас.

### Read-сторона

**Read non-tx:** `main.get(key)` напрямую. Нет scan, нет codec, нет
overhead. Точно как сейчас.

**Read in tx:**
1. Working set lookup → если есть, return (memory).
2. `cache.get(key)` → current_version.
3. Если `current_version ≤ snapshot` → `main.get(key)` (один get, нет scan).
4. Иначе → range scan history по prefix `key::` с верхней границей
   snapshot (один scan, но только для конкретно overwritten keys).

Cache `key → current_version` строится по мере доступа; при cold start
первое чтение делает scan, дальше O(1).

**Read non-tx когда есть параллельная tx:** **ноль overhead** — non-tx
читателю старые версии не нужны. Он просто `main.get(key)` берёт
latest.

### Interner non-tx

`LayeredInterner::Direct(&interner)` — точно тот же код что сейчас, без
обёрток. Можно даже compile-time dispatch через generics для
абсолютного zero-cost.

### Сводная таблица

| Операция            | Non-tx (всегда)            | Tx (вообще)                  | Tx (та же key одновременно меняется другой tx) |
|---------------------|----------------------------|------------------------------|------------------------------------------------|
| `set`               | 1 main write               | 1 main write + (история если активны старые snapshots) | то же |
| `get`               | 1 main read                | 1 cache + 1 main             | 1 cache + 1 main, либо (cache miss → 1 history scan) |
| Interner `touch`    | 1 hashmap insert           | 1 overlay insert             | то же |
| Index update        | 1+N writes напрямую        | буферизуется в write_set     | то же |
| Counter increment   | atomic in-memory           | counter_delta в TxContext    | то же |

**Вывод:** non-tx путь не замедляется ни на одну операцию. Tx путь
медленнее только при реальной contention — что архитектурно честно.

---

## Конкурирующие тесты — провоцируем поломки

Цель — **доказать что транзакции реально работают под параллельной
нагрузкой**, не только в happy path. Нужны **множественные
подключения** (две session_id, два TLS-канала). Наш Node SDK это
позволяет (`ShamirClient.connect` многократно).

### Каркас

```js
const c1 = await ShamirClient.connect({...}); // session 1
const c2 = await ShamirClient.connect({...}); // session 2

// Параллельные tx через Promise.all
const [r1, r2] = await Promise.all([
  c1.execute(db, { id: 't1', transactional: true, queries: {...} }),
  c2.execute(db, { id: 't2', transactional: true, queries: {...} }),
]);
```

### Сценарии

#### 1. Lost update (документирует поведение SI+LWW по умолчанию)

```
c1: tx { read X=10, sleep 50ms,  write X=20 }
c2: tx { read X=10, sleep 100ms, write X=30 }
parallel
final: X=30 (последний коммит побеждает; T1 update lost)
```

Ассерт: `final == 30` — это **ожидаемое** поведение для
Snapshot Isolation + last-writer-wins. Тест документирует, не
сигнализирует ошибку.

#### 2. Lost update detected (SSI mode)

```
c1: tx serializable { read X=10, write X=20 }
c2: tx serializable { read X=10, write X=30 }
parallel
expected: один коммитится OK, второй отклоняется с code='tx_conflict'
retry conflicted с актуальным X
final: X=20 + 30 (или X=40, в зависимости от семантики приложения)
```

#### 3. Phantom protection

```
c1: tx begin
c1: SELECT count where age>=18  → 5
c2: INSERT user{age=25} (вне tx)
c1: SELECT count where age>=18  → 5      ← snapshot держит
c1: commit
c1 (вне tx): SELECT count where age>=18 → 6
```

#### 4. Write skew (классический doctor-on-call)

```
table: doctors {id, on_call: bool}, два врача оба on_call=true
c1: tx { count where on_call=true → 2; if >1 then write self.on_call=false }
c2: тот же tx
parallel

SI:  оба видят 2, оба себя выключают → final 0 врачей on_call (банк лопнул)
SSI: один абортится, retry видит уже одного включённого, не выключается → final 1
```

«Инвариант защищается только SSI» — критический тест границы между
двумя уровнями изоляции.

#### 5. Counter race

```
table: counter{id="x", n: 0}
100 параллельных JS tx'ов: { read counter, write counter+1 }

SI+LWW:  final n < 100 (lost updates под contention)
SSI:     аборты + retry → eventually final == 100
```

Полезен график timing: SSI медленнее под high contention, но
корректно. Демонстрирует trade-off.

#### 6. Read-after-write inside tx (свои writes видны самим себе)

```
c1: tx begin
c1: write X=20 (working set)
c1: read X → 20      ← видит свой write
c2: read X (вне tx) → старое (10)
c1: commit
c2: read X → 20
```

#### 7. Snapshot isolation under parallel writes

```
c1: tx begin → snapshot v100
c2: many independent writes (delete records, insert new ones)
c1: продолжает читать → видит v100 как было (snapshot стена)
c1: commit
```

Ассерт: c1 SELECT count = старое число, не новое.

#### 8. Crash mid-commit (recovery)

Жёстче — kill -9 сервера в момент commit:

- `c1.execute(db, ... transactional: true)` начат
- `process.kill(serverPid, 'SIGKILL')` пока tx коммитится
- restart server, открыть, прочитать данные
- ассерт: либо ВСЕ writes из tx видны, либо НИ ОДНОГО (атомарность
  через `__last_committed_version__` маркер)

#### 9. GC под активной tx

- c1 begins long-running read-heavy tx
- c2 делает 10000 writes (создавая большую историю)
- GC должен НЕ удалить версии нужные c1
- c1 продолжает читать корректно
- c1 commits → GC пробуждается → чистит историю

Ассерт: c1 во время своей жизни видит консистентный snapshot;
после c1.commit размер history store падает.

#### 10. Nested не работает

В одном `execute()` `transactional` флаг один — на уровень API
nesting просто невозможен. Тест: убедиться что нет случайного
способа войти во вложенную tx.

### Где разместить

```
tests/e2e/tests/
├── 11-transactions.test.js              ← Phase A core (single connection)
├── 12-transactions-concurrent.test.js   ← multiple connections, race scenarios
└── 13-transactions-recovery.test.js     ← crash + restart, server lifecycle
```

#11 запускается обычным way через основной orchestrator. #12 спавнит
**вторую** connection в orchestrator'е. #13 — единственный кто реально
kill'ит server и спавнит заново (ломает обычный «один сервер на весь
прогон» pattern; запускается отдельно или с дополнительным flag'ом).

### Rust-уровень

`crates/shamir-engine/tests/transactions/`:

- `version_codec_tests.rs` — round-trip + сохранение sort-order
- `mvcc_store_tests.rs` — `get_at` versioned reads под нагрузкой
- `tx_gate_tests.rs` — concurrent `assign_next_version` (нет дубликатов),
  recovery marker корректность
- `gc_tests.rs` — GC корректно держит `min_alive_snapshot`,
  не удаляет нужное
- `interner_overlay_tests.rs` — overlay flush на commit, drop на abort

Эти быстрые (in-memory backend), запускаются в основном `cargo test`
sweep — изоляция transactional engine от storage backend.

---

## Что должно быть в порядке *до* старта реализации

Прежде чем писать первый код:

1. **Согласовать default isolation level.** SI+LWW по умолчанию или SSI?
   SI проще, быстрее, но lost-update возможен. SSI сильнее, но требует
   read-set tracking + retry-логику в клиенте.
2. **Согласовать `transactional` semantics на existing flag.** Сейчас
   там `bool`. Нужно ли расширять до enum `{ None, SI, SSI }`?
   Wire-compat: `false` → None, `true` → SI (default). Добавить
   `transaction_isolation: "serializable"` опцией.
3. **Решить судьбу `execute_set` O(n) scan.** До того как tx
   реализуется, scan'ы внутри tx будут гнать GC и блокировать всё.
   Возможно нужно сначала ввести primary key index (отдельный sprint),
   потом tx.
4. **Подтвердить готовность переписать IndexManager.** Около 30-40%
   его поверхности затронется (все `on_record_*` хуки + `unique`
   валидация).

---

## Order of work

1. `version_codec.rs` + tests — encode/decode/sort-order (1 ч)
2. `iter_prefix_stream` — добавить в `Store` trait + native impls
   во всех бэкендах (4 ч)
3. `RepoTxGate` — Mutex + counter + recovery marker, с симулятором
   crash-mid-commit (3 ч)
4. `MvccStore` — wrap `Store`, реализовать current+history layout
   с zero-overhead путём (4-6 ч)
5. `LayeredInterner` — overlay-обёртка (1 ч)
6. `TxContext` — read-set / write-set / read-through-overlay (2 ч)
7. `TableManager` — методы с `Option<&mut TxContext>` (3-4 ч)
8. `IndexManager` — порт хуков на TxContext-aware (3-4 ч)
9. Executor integration — single-repo check, serial run, commit/abort,
   `BatchResponse.transaction` filled in (2 ч)
10. SI mode (LWW, без conflict detection) — заработает first (2 ч)
11. SSI mode за флагом — read-set validation на commit (4 ч)
12. `GcWorker` per repo (3 ч)
13. Rust integration tests (3 ч)
14. e2e #11 + #12 + #13 (5-6 ч)
15. docs: `TRANSACTIONS.md` → "implemented", `LOGIC_FLOW.md` обновить,
    root README capability list (1 ч)

**Total: ~2-3 недели сфокусированной работы.** Phase A only. Без
interactive transactions.
