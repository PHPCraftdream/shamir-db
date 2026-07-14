# Этап 3. MvccStore + read pipeline через `Option<&TxContext>`

**Срок:** 6-7 дней. **Зависит от:** Этап 0-2.
**Блокирует:** Этап 4 (executor).

Цель — собрать слой версионированного хранилища поверх dumb-KV и
протянуть `Option<&TxContext>` через каждое место в read pipeline,
которое сегодня читает напрямую.

## 3.1. `MvccStore`

Layout — **current + history**:
- `main` store: текущая версия (как сегодня; non-tx writes идут только
  сюда).
- `history` store: старые версии под ключами `<key>::<version_be>`.

Запись:

```rust
async fn set_with_versioning(&self, key: Bytes, value: Bytes) -> DbResult<()> {
    // Zero-overhead non-tx ветка.
    if self.gate.active_snapshots_empty() {
        self.main.set(key, value).await?;
        return Ok(());
    }

    // Сохраняем старую версию в history если есть snapshot, для
    // которого она ещё нужна.
    if let Ok(old) = self.main.get(key.clone()).await {
        let old_v = self.version_cache.get(&key).copied().unwrap_or(0);
        if self.gate.has_snapshot_below(old_v) {
            let h_key = encode_version_key(&key, old_v);
            self.history.set(h_key, old).await?;
        }
    }

    self.main.set(key.clone(), value).await?;
    let new_v = self.gate.assign_next_version();
    self.version_cache.insert(key, new_v);
    Ok(())
}
```

Ключевое условие zero-overhead: **если ни одной активной tx нет —
history не пишется**. Это check на одну атомарную операцию
(DashMap is_empty), ветка `if` предсказуема branch-predictor'ом.

Чтение:

```rust
async fn get_at(&self, key: &[u8], snapshot: u64) -> DbResult<Option<Bytes>> {
    // Fast path: текущая версия ≤ snapshot → main.get.
    if let Some(cur_v) = self.version_cache.get(key).copied() {
        if cur_v <= snapshot {
            return self.main.get(key.to_vec().into()).await.map(Some);
        }
    }

    // Slow path: range scan в history, ищем максимальную версию
    // ≤ snapshot.
    let lo = encode_version_key(key, 0);
    let hi = encode_version_key(key, snapshot);
    let mut stream = self.history.iter_range_stream(Some(lo), Some(hi), 1);
    // Берём последний (наибольший) ключ в диапазоне.
    let mut latest = None;
    while let Some(batch) = stream.next().await {
        for (_, val) in batch? {
            latest = Some(val);
        }
    }
    Ok(latest)
}
```

**Version cache** — `scc::HashMap<RecordKey, u64>` в памяти, без
персистентности. Cold start: первое чтение делает scan, дальше O(1).

**Версионный ключ encoding:**

```rust
fn encode_version_key(key: &[u8], version: u64) -> Bytes {
    let mut b = BytesMut::with_capacity(key.len() + 9);
    b.extend_from_slice(key);
    b.put_u8(0x00);            // separator (никогда не встречается в RecordId или SysKey)
    b.put_u64(version);        // big-endian для естественной сортировки
    b.freeze()
}
```

**Сепаратор `0x00`.** `RecordId` это 16 случайных байт — вероятность
включить `0x00` в ключ есть, но `SysKey` варианты идут с фиксированным
префиксом, плюс version_be в конце — это значит ключи `key::42` и
`key::43` лежат рядом в lexicographic order. Альтернатива: encoding
с length-prefix для key. Для V1 берём простой `key + 0x00 + version`
и **проверяем** в encoding test, что `RecordId::to_bytes()` никогда
не имеет trailing-byte которое сольётся (мы можем guarantee через
RecordId layout).

**Acceptance.**
- `get_at` round-trip: write v=5, write v=10, get_at(snapshot=7)
  → видим v=5; get_at(snapshot=15) → видим v=10.
- Zero-overhead bench: один writer пишет 100k раз без snapshot →
  history.len() == 0 и main writes идут с тем же throughput что в
  baseline.
- Version cache cold start: после reopen первый get_at делает scan,
  следующий — O(1).

## 3.2. Read pipeline через `Option<&TxContext>`

Это — **самая большая** поверхность Этапа 3. Идея: каждая read-функция
получает дополнительный параметр `tx: Option<&TxContext>`. Non-tx
сценарий = `None`, тот же код.

**Где меняется:**

```rust
// TableManager
impl TableManager {
    pub async fn get(&self, rid: RecordId, tx: Option<&TxContext>) -> DbResult<InnerValue>;
    pub fn iter_stream(&self, tx: Option<&TxContext>, batch_size: usize) -> RecordStream;
    pub fn list_stream(&self, tx: Option<&TxContext>, batch_size: usize) -> impl Stream<...>;
    pub fn filter_stream(&self, tx: Option<&TxContext>, filter: &Filter, ...) -> ...;
    // Лень: где сейчас просто `&self.table.data_store()` — замена на
    // self.read_through(tx).get(...).
}
```

Helper:

```rust
impl TableManager {
    /// Один lookup путь через tx и mvcc.
    pub async fn read_one(&self, rid: RecordId, tx: Option<&TxContext>) -> DbResult<Bytes> {
        if let Some(tx) = tx {
            // 1. Look in write_set first
            if let Some(staged) = tx.write_set.get(&self.name) {
                if let Some(v) = staged.try_get(rid).await? {
                    return Ok(v);
                }
            }
            // 2. mvcc snapshot read
            return self.mvcc.get_at(&rid.to_bytes(), tx.snapshot_version).await?;
        }
        // Non-tx fast path — как сейчас.
        self.table.data_store().get(rid.to_bytes()).await
    }
}
```

**Index2 read tx-aware:**

```rust
impl IndexBackend {
    async fn lookup(&self, q: IndexQuery, tx: Option<&TxContext>) -> Result<IndexResult, IndexError>;
}
```

Tx-Some lookup: planner backend должен принять во внимание staged
ops в `tx.index_write_set` (записи, ещё не во flushed postings).
Simple approach: после backend.lookup на committed snapshot —
дополнить/удалить result согласно tx-local index_write_set. Аналогично
HNSW staging.

**HNSW search в tx:**

```rust
impl HnswAdapter {
    pub async fn search(&self, query: &[f32], k: u32, tx: Option<TxId>) -> Result<...>;
    // Если tx == Some, делаем overscan по graph (committed only —
    // tombstones применяем) + brute-force merge с staged_inserts[tx_id].
}
```

**Acceptance.**
- Все existing read tests зелёные (non-tx == None — нулевой overhead).
- New test: tx inserts X = 10, в той же tx читает X → видит 10.
- New test: tx1 пишет X=10 не commit'ит, tx2 на параллельном
  snapshot читает X → видит pre-tx state (Snapshot Isolation).
- Bench `engine_perf.rs` не регрессирует > 2% на non-tx путях
  (Vec<None> branch predictor friendly).

## 3.3. Index2 store transition

Подменить `Arc<dyn Store>` внутри backends на `Arc<MvccStore>`. Это
автоматически делает postings versioned. Текущий contract `set` /
`remove` сохраняется (через `IndexWriteOp` Этапа 1.1) — MvccStore
их версионирует прозрачно.

Гарантия: tx-uncommitted postings не видны другим tx до commit,
потому что `MvccStore::set` пишет в history с new_version > snapshot.

**Acceptance.**
- 100 параллельных tx: каждая пишет различные постинги. Concurrent
  observers видят только committed postings.
- Recovery test: crash после commit но до GC (history полная) —
  reads видят правильный snapshot.

## 3.4. Sorted index

`SortedIndexManager` тоже использует MvccStore. Это даёт **versioned
sorted index** — range scan в snapshot видит только postings ≤ snapshot.

Это меняет range scan API — теперь `iter_range_stream` от
`MvccStore` фильтрует по версии. По существу это `get_at` для каждого
ключа в диапазоне — то есть тяжелее. Mitigation: cache `(prefix
range → current version range)` если профайлер покажет необходимость.

**Acceptance.**
- range scan в tx видит только version ≤ snapshot.
- Bench `sorted_index_range`: regress в tx mode < 30%, non-tx не
  регрессирует.

## Порядок работы

1. `encode_version_key` + tests (0.5 дня).
2. `MvccStore::set_with_versioning` + zero-overhead branch test
   (1 день).
3. `MvccStore::get_at` + version cache (1 день).
4. `TableManager::read_one`/`iter_stream`/`filter_stream` принимают
   `Option<&TxContext>` (1.5 дня — широкий рефакторинг сигнатур, но
   тривиальный per-callsite).
5. `IndexBackend::lookup` принимает tx + merge со staged ops (1 день).
6. `HnswAdapter::search` в tx с merge со staged (1 день).
7. `SortedIndexManager` через MvccStore (0.5 дня).
8. Integration tests: round-trip writes through tx with mvcc reads
   (0.5 дня).

**Не делаем здесь:**
- Не пишем executor commit logic (Этап 4).
- Не делаем SI / SSI validation (Этап 4).
- Не пишем GC (Этап 6).
