# Этап 1. Write isolation layer

**Срок:** 7-8 дней. **Зависит от:** Этап 0 (foundations).
**Блокирует:** Этап 2 (TxContext) и далее.

Цель — устранить direct-write side effects из горячих write-путей.
После этого этапа любой mutating call можно либо **применить
немедленно** (non-tx), либо **сохранить в журнал** (tx) — выбор
делает caller, а не сам код index/HNSW.

## 1.1. `IndexWriteOp` enum + planner-style API

**Проблема.** Все index2 backends (FTS, FTS ranked, Functional) и
старый `IndexManager::on_record_*` пишут напрямую `self.store.set(...)`
/ `self.store.remove(...)` внутри `on_insert/update/delete`. Эти
writes — side effects, которые невозможно откатить без записанного
журнала.

**Решение.** Разделить «спланировать ops» и «применить ops». Trait
становится:

```rust
pub enum IndexWriteOp {
    SetPosting { key: Bytes, value: Bytes },
    RemovePosting { key: Bytes },
    // BM25 stats — атомарный delta на FtsRankedBackend's in-memory
    // counters. Apply phase синхронно вызывает stats.on_insert/delete.
    BumpFtsStats { doc_len: u32, sign: i8 /* +1 or -1 */ },
}

#[async_trait]
pub trait IndexBackend: Send + Sync {
    fn descriptor(&self) -> &IndexDescriptor;

    // OLD:
    // async fn on_insert(&self, rid, val) -> Result<(), IndexError>;

    // NEW:
    async fn plan_insert(&self, rid: RecordId, val: &InnerValue)
        -> Result<Vec<IndexWriteOp>, IndexError>;
    async fn plan_update(&self, rid: RecordId, old: &InnerValue, new: &InnerValue)
        -> Result<Vec<IndexWriteOp>, IndexError>;
    async fn plan_delete(&self, rid: RecordId, val: &InnerValue)
        -> Result<Vec<IndexWriteOp>, IndexError>;

    async fn lookup(&self, q: IndexQuery) -> Result<IndexResult, IndexError>;
    async fn rebuild(&self, src: Arc<dyn Store>) -> Result<(), IndexError>;
}

/// Apply phase — отдельная функция, не trait method.
/// Non-tx путь: вызывается сразу после plan_*.
/// Tx путь: ops накапливаются в TxContext, applied в commit вместе
/// с data writes одним `store.transact(...)`.
pub async fn apply_index_ops(
    ops: &[IndexWriteOp],
    store: &dyn Store,
    backend: &dyn IndexBackend,
) -> Result<(), IndexError>;
```

**Касается:**
- `fts_backend.rs` — `plan_*` возвращают `SetPosting / RemovePosting`.
- `fts_ranked_backend.rs` — добавляет `BumpFtsStats` для doc_count
  / sum_doc_len. На non-tx путь stats обновляется в `apply_index_ops`.
- `functional_backend.rs` — `plan_*` строят hash + posting key.
- Старый `IndexManager::on_record_*` — переписывается аналогично.
- `SortedIndexManager::on_record_*` — то же.

**Non-tx wrapper** в `TableManager`:

```rust
async fn index2_on_insert(&self, rid, val) -> DbResult<()> {
    let backends = self.index2_registry.all_backends().await;
    for b in &backends {
        let ops = b.plan_insert(rid, val).await?;
        apply_index_ops(&ops, &self.info_store, b.as_ref()).await?;
    }
    Ok(())
}
```

То же что и сейчас (нулевой overhead) — просто промежуточный
аллоc `Vec<IndexWriteOp>`. Один `SmallVec<[IndexWriteOp; 8]>`
покрывает 99% случаев без heap alloc.

**Acceptance.**
- Все existing index2 unit tests зелёные (нет регрессии семантики).
- `cargo bench --bench order_by_pipeline` не регрессирует > 5%
  (Vec alloc стоимость через SmallVec — в noise).
- New unit-test: `plan_insert` + manual `apply_index_ops` даёт то же
  состояние, что прямой `on_insert` дал бы — нативный equivalence
  test.

## 1.2. HNSW staging

**КРИТИЧЕСКАЯ ПРОБЛЕМА.** `hnsw_rs::Hnsw::insert` необратим. Сейчас
HNSW state мутируется немедленно в `HnswAdapter::upsert`. Если tx,
которая позвала upsert, потом aborts — точки **останутся в графе**.
Soft-delete tombstones отфильтровывают их при search, но граф
постоянно растёт + overscan ×2 не спасает при многих aborts.

**Решение.** Insert в HNSW делается **только на commit**. До этого
кандидаты лежат в staging.

```rust
pub struct HnswAdapter {
    dim: u32,
    metric: VectorMetric,
    ef_search: usize,
    hnsw: Arc<Hnsw<'static, f32, ShamirDist>>,
    rid_map: scc::HashMap<usize, RecordId>,
    rid_to_internal: scc::HashMap<RecordId, usize>,
    deleted: scc::HashMap<usize, ()>,
    next_id: AtomicUsize,

    // NEW: per-tx staging
    staged: scc::HashMap<TxId, Vec<StagedVector>>,
}

struct StagedVector {
    rid: RecordId,
    vec: Vec<f32>,
    /// Если upsert заменял существующий rid — тут id, который надо
    /// tombstone'нуть **только** при commit. На abort — id остаётся
    /// живым (rollback к pre-tx state).
    replaces: Option<usize>,
}

impl HnswAdapter {
    pub async fn upsert(&self, rid: RecordId, vec: &[f32], tx: Option<TxId>) {
        match tx {
            None => self.insert_now(rid, vec).await,         // как сейчас
            Some(tx_id) => self.stage(tx_id, rid, vec).await, // accumulate
        }
    }

    pub async fn commit_staged(&self, tx_id: TxId);   // batch insert + apply tombstones
    pub async fn rollback_staged(&self, tx_id: TxId); // drop staged entry
}
```

**Тонкое место.** Внутри одной tx может быть upsert(A) → search →
upsert(B). `search` должен видеть A. Решения:

- **Вариант 1 (выбираем):** search в tx делает overscan, потом
  фильтрует tombstones, потом **сливает** результат с brute-force
  scan по staged vectors. Дорого только если staged велик; обычно
  < 1k записей в tx.
- Вариант 2 (отклоняется): insert в HNSW + tombstone on abort. Это
  то что у нас сейчас, и мы видели его проблему.

**Acceptance.**
- New unit-test `tx_upsert_then_search_sees_staged`: tx делает
  upsert + search в той же tx → видит staged vector.
- `tx_rollback_does_not_pollute_graph`: 1000 staged inserts, rollback,
  затем 1000 committed inserts → recall + memory как при чистом
  графе.
- Bench `hnsw_with_staging.rs`: search + 100 staged < 2× от search
  без staged (overhead приемлем).

## 1.3. `StagingStore`

**Проблема.** Нужно in-memory буфер для writes одной транзакции с
read-through-staging semantics.

**Решение.** Тонкая обёртка над `Arc<dyn Store>`:

```rust
pub struct StagingStore {
    base: Arc<dyn Store>,
    writes: scc::HashMap<RecordKey, StagedOp>,
}

enum StagedOp { Set(Bytes), Remove }

impl StagingStore {
    pub fn new(base: Arc<dyn Store>) -> Self { ... }

    /// Lookup сначала в writes, потом в base.
    pub async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        if let Some(op) = self.writes.read_async(&k, |_, v| v.clone()).await {
            return match op {
                StagedOp::Set(b) => Ok(b),
                StagedOp::Remove => Err(DbError::NotFound(...)),
            };
        }
        self.base.get(k).await
    }

    pub async fn set(&self, k: RecordKey, v: Bytes);
    pub async fn remove(&self, k: RecordKey);

    /// Atomic flush: собирает все ops в Vec<KvOp> и пишет одним
    /// `base.transact(ops)`. ИЛИ возвращает Vec<KvOp> для upper
    /// layer чтобы тот объединил с другими таблицами в один
    /// repo-scope transact.
    pub fn drain(self) -> Vec<KvOp>;
}
```

**Используется:**
- Этап 2: `TxContext.write_set: BTreeMap<TableId, StagingStore>`.
- Также для **Phase A0** (если возьмём short-cut путь): batch
  executor создаёт один StagingStore, прогоняет все queries, flushes
  атомарно.

**Acceptance.**
- `staging_get_after_set` — пишем k=v через staging, читаем — видим v.
- `staging_get_after_remove` — пишем remove через staging, read
  возвращает NotFound даже если в base ключ есть.
- `staging_drain_produces_atomic_batch` — drain → `Vec<KvOp>` с
  ожидаемым порядком/содержимым.

## Порядок работы

1. `IndexWriteOp` enum + apply_index_ops (0.5 дня).
2. Переписать `fts_backend` → `plan_*` (1 день).
3. `fts_ranked_backend` + BumpFtsStats (1.5 дня — care for stats
   atomicity).
4. `functional_backend` (0.5 дня).
5. Старый `IndexManager` (1 день).
6. `SortedIndexManager` (0.5 дня).
7. HNSW staging — `staged` field, upsert with tx, commit_staged,
   rollback_staged, in-tx search merge with staged (2 дня).
8. `StagingStore` + tests (0.5 дня).

**Не делаем здесь:**
- Не создаём `TxContext` (Этап 2).
- Не интегрируем в executor (Этап 4).
- Не трогаем read pipeline (Этап 3).
