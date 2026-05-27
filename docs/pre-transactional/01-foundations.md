# Этап 0. Foundations

**Срок:** 3-4 дня. **Блокатор:** всех остальных этапов.

Цель — заложить три низкоуровневых примитива, на которых стоит весь
последующий transactional layer. Каждый из четырёх пунктов
самостоятелен и не ломает существующее поведение.

## 0.1. Keyspace consolidation

**Проблема.** Префиксы типа `b"__index__"`, `b"__meta__"`,
`b"__counter__"` разбросаны string-literal'ами по коду. При переходе
к версионированию ключей `<key>::<version_be>` пришлось бы trawl'ить
весь репо.

**Решение.** Один `keyspace.rs` в `shamir-engine` с typed enum:

```rust
pub enum SysKey {
    Counter,
    Internals,
    BufferConfig,
    Indexes,                                        // index2 metadata
    WalEntry(u64),
    IndexPosting { idx_id: u32, tag: u8, payload: Bytes },
    SortedIndex { name_id: u64, value: Bytes },
}

impl SysKey {
    pub fn to_bytes(&self) -> Bytes;
    pub fn parse(b: &[u8]) -> Option<Self>;
}
```

Все callsites, которые сейчас строят ключ через literal, заменяются
на `SysKey::Variant.to_bytes()`. Это позволит **в одном месте**
поздним этапом добавить `::<version>` суффикс к нужным вариантам.

**Acceptance.**
- Все `b"__..."` literal'ы в `crates/shamir-engine/src/` (кроме
  тестов и одного central definition) удалены.
- `rg 'b"__'` в production code возвращает 0 совпадений вне
  `keyspace.rs`.
- `cargo test --workspace --lib` зелёный.

## 0.2. `Store::transact(ops: Vec<KvOp>)`

**Проблема.** Сейчас атомарны только однотипные batch'и
(`set_many` / `remove_many`). Mixed-op batch (set + remove одной
транзакцией) на backend не поддерживается.

**Решение.** Добавить в `Store` trait:

```rust
pub enum KvOp {
    Set(RecordKey, Bytes),
    Remove(RecordKey),
}

#[async_trait]
pub trait Store: Send + Sync {
    // ... existing methods ...

    /// Атомарный mixed-op batch. Default impl последовательный —
    /// НЕ атомарный. Бэкенды с native write tx переопределяют.
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        for op in ops {
            match op {
                KvOp::Set(k, v) => { self.set(k, v).await?; }
                KvOp::Remove(k) => { self.remove(k).await?; }
            }
        }
        Ok(())
    }
}
```

Native impls для:
- **redb** — `WriteTxn::commit`.
- **sled** — `Batch` + `apply_batch`.
- **fjall** — `WriteBatch`.
- **persy** — `Tx::commit`.
- **nebari** — `WriteTransaction::commit_with`.
- **canopy** — native transactional write.
- **in_memory** — `parking_lot::Mutex` lock на DashMap (грубо, но
  атомарно). Не используется на горячем пути всё равно.
- **cached / membuffer** — proxy на inner.

**Acceptance.**
- Trait-level test: mixed `[Set, Remove, Set]` batch виден либо весь,
  либо никак (нет partial state visible через concurrent read).
- Property-test: `transact(ops)` over an empty store; затем
  concurrent observer проверяет, что либо все ops применены, либо
  ни один.
- Бэкенд compare bench (`storage_backend_compare.rs`) не регрессирует
  на `set_many` / `remove_many` сценариях.

## 0.3. `Store::compare_and_swap`

**Проблема.** Version counter для MVCC требует atomic increment, не
теряющий событий под contention. Mutex на каждый commit терпим, но
CAS быстрее.

**Решение.** Добавить в `Store` trait:

```rust
/// Атомарный CAS. Возвращает true, если запись была равна
/// expected_old и стала new. False — значение не совпало (никаких
/// изменений). `expected_old = None` означает «запись отсутствует».
async fn compare_and_swap(
    &self,
    key: RecordKey,
    expected_old: Option<Bytes>,
    new: Bytes,
) -> DbResult<bool> {
    // Default через transact + локальный mutex (медленнее, но
    // корректно везде).
    self.cas_via_mutex(key, expected_old, new).await
}
```

Native impls для бэкендов с CAS:
- **redb** — `WriteTxn::insert` с явной проверкой.
- **sled** — `Tree::compare_and_swap`.
- **persy** — `Tx::insert_record` (есть variants для CAS).

In-memory / membuffer fall through to default mutex-based impl
(per-store `parking_lot::Mutex`).

**Acceptance.**
- 100 параллельных задач делают `CAS(key, n, n+1)` начиная с `n = 0`
  до 100; финальное значение = 100. Никаких лост updates.
- Default impl даёт ту же гарантию (in-memory backend проходит тот
  же тест).

## 0.4. WAL inline body

**Проблема.** Сейчас `WalOp::RecordCreated { record_id }` — без
самих bytes. Recovery читает запись из data_store. Для MVCC это не
работает: tx-uncommitted writes **ещё нет** в data_store на момент
crash mid-commit → recovery не видит, atomicity ломается.

**Решение.** Новый `WalOpV2` рядом со старым (старый продолжает
работать для non-tx path):

```rust
pub enum WalOpV2 {
    Put { rid: RecordId, body: Bytes },           // body inline
    Delete { rid: RecordId },
    IndexPut { idx_id: u32, key: Bytes, value: Bytes },
    IndexDel { idx_id: u32, key: Bytes },
    InternerOverlayMerge { entries: Vec<(u64, String)> },
    CounterDelta { table_id: TableId, delta: i64 },
}

pub struct WalEntryV2 {
    pub txn_id: u64,
    pub repo_id: RepoId,         // repo-scoped, не table-scoped
    pub started_at_ns: u64,
    pub ops: Vec<WalOpV2>,
}
```

V1 entries остаются — старый non-tx путь продолжает писать V1.
V2 entries появляются только при tx writes. Recovery умеет оба.

**Размер WAL.** Inline body означает рост WAL на размер всех tx
writes. Для batch с 10k inserts по 1KB — это 10MB WAL entry. Это
приемлемо: tx commits редкие, размер ограничен длиной batch.

**Acceptance.**
- Round-trip test: написать V2 entry с 100 ops → прочитать → сверить
  байты.
- Recovery test: симулированный crash после `begin(V2)`, до
  `commit`. На open видим inflight V2 entry. Forward-fix применяет
  ops (Put / Delete / IndexPut / IndexDel) — конечное состояние
  data_store + info_store == ожидаемое.
- WAL size bench: 10k V2 inserts → entry < 11MB (overhead < 10%).

## Порядок работы

1. `keyspace.rs` + замена literal-ов (1-1.5 дня).
2. `Store::transact` + native impls + property test (1 день).
3. `Store::compare_and_swap` + CAS test (0.5 дня).
4. `WalOpV2` / `WalEntryV2` + serde + recovery test (1 день).

**Не делаем здесь:**
- Не трогаем index2 hooks (это Этап 1).
- Не создаём `TxContext` (это Этап 2).
- Не пишем MvccStore (это Этап 3).
- Не используем `WalOpV2` в production yet — только тесты.
