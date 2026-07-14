# Этап 1. Write isolation layer

**Срок:** ~8 дней, разбит на 11 атомарных подэтапов.
**Зависит от:** Этап 0 (foundations).
**Блокирует:** Этап 2 (TxContext) и далее.

Цель — устранить direct-write side effects из горячих write-путей.
После этого этапа любой mutating call можно либо **применить
немедленно** (non-tx), либо **сохранить в журнал** (tx) — выбор
делает caller, а не сам код index/HNSW.

Каждый подпункт ниже — **отдельный landable commit**: не ломает
существующее (старый non-tx путь продолжает работать через
интермедиатные plan→apply wrapper'ы), имеет свои тесты, проходит
pre-commit gate (fmt + clippy `-D warnings` + tests).

## Декомпозиция

| # | Что | Срок |
|---|---|---|
| **1.1.A** | `IndexWriteOp` + `apply_index_ops` helper + default `plan_*` методы в trait | 0.5 дня |
| **1.1.B** | `FtsBackend` — override `plan_*`, переключить wrapper | 0.5 дня |
| **1.1.C** | `FtsRankedBackend` + `BumpFtsStats` variant | 1 день |
| **1.1.D** | `FunctionalBackend` — override `plan_*` | 0.5 дня |
| **1.1.E** | Legacy `IndexManager::on_record_*` → planner-style | 1 день |
| **1.1.F** | `SortedIndexManager::on_record_*` → planner-style | 0.5 дня |
| **1.1.G** | Cleanup: убрать старые `on_*` методы из `IndexBackend` trait | 0.5 дня |
| **1.2.A** | `HnswAdapter::staged` поле + `stage()` API (private) | 0.5 дня |
| **1.2.B** | `commit_staged` / `rollback_staged` методы | 0.5 дня |
| **1.2.C** | `upsert(tx: Option<TxId>)` + in-tx search merge | 1 день |
| **1.3** | `StagingStore` + tests | 0.5 дня |

Линейная зависимость только между **1.1.A → 1.1.B–F** и
**1.2.A → 1.2.B → 1.2.C**. Внутри каждого «треугольника» подэтапы
**могут идти параллельно** (или каждый разработчик берёт свой
backend), но на практике линейный порядок проще.

После всего этапа — `Vec<IndexWriteOp>` накапливается в внешнем
буфере (Stage 2 предоставит `TxContext.index_write_set`); в non-tx
случае wrapper применяет ops сразу же.

---

## 1.1.A. `IndexWriteOp` foundation

**Срок:** 0.5 дня.

### Что

В `crates/shamir-engine/src/index2/write_ops.rs` (новый файл):

```rust
//! Write-op planning primitives for transactional commit.
//!
//! Each `IndexBackend` returns a `Vec<IndexWriteOp>` from
//! `plan_insert / plan_update / plan_delete` instead of writing
//! directly to its store. The caller picks one of two apply paths:
//!
//! - **Non-tx**: `apply_index_ops` runs ops immediately against
//!   the store + backend (e.g. BumpFtsStats hits the live atomic
//!   counter).
//! - **Tx**: ops accumulate in `TxContext.index_write_set`; the
//!   commit phase calls `apply_index_ops` with the merged
//!   `(store, backend)` pair under the commit lock.

use bytes::Bytes;
use crate::index2::backend::{IndexBackend, IndexError};
use shamir_storage::types::Store;

#[derive(Debug, Clone)]
pub enum IndexWriteOp {
    /// Insert or overwrite a posting.
    SetPosting { key: Bytes, value: Bytes },

    /// Delete a posting by key.
    RemovePosting { key: Bytes },

    /// Increment / decrement `FtsRankedBackend::FtsStats` (`doc_count`
    /// + `sum_doc_len`). Carries `(doc_len, sign)`; `sign` is +1 or
    /// -1. Applied to atomic counters; never persisted as a posting.
    BumpFtsStats { doc_len: u32, sign: i8 },
}

/// Apply a slice of ops against a store + backend.
///
/// Non-tx callers invoke this right after `plan_*`. Tx callers
/// invoke this under the commit lock, after merging `TxContext`'s
/// index_write_set with `MvccStore::transact`.
pub async fn apply_index_ops(
    ops: &[IndexWriteOp],
    store: &dyn Store,
    backend: &dyn IndexBackend,
) -> Result<(), IndexError>;
```

В `crates/shamir-engine/src/index2/backend.rs` — расширить trait
default-implemented методами **рядом** со старыми `on_insert/update/
delete` (не удаляя их):

```rust
#[async_trait]
pub trait IndexBackend: Send + Sync {
    // ... existing methods, including on_insert/on_update/on_delete ...

    /// Plan ops for an insert. Default: empty Vec (backends that
    /// haven't migrated to the planner API yet rely on the old
    /// on_insert path).
    async fn plan_insert(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let _ = (rid, rec);
        Ok(Vec::new())
    }

    async fn plan_update(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let _ = (rid, old, new);
        Ok(Vec::new())
    }

    async fn plan_delete(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> Result<Vec<IndexWriteOp>, IndexError> {
        let _ = (rid, rec);
        Ok(Vec::new())
    }

    /// Apply ops directly. Used by `apply_index_ops` helper for
    /// `BumpFtsStats` and similar in-memory state changes that
    /// don't go through the `Store`. Default: no-op.
    async fn apply_in_memory(&self, ops: &[IndexWriteOp]) -> Result<(), IndexError> {
        let _ = ops;
        Ok(())
    }
}
```

Default-impl возвращает пустой Vec — старый код продолжает работать
через `on_*`. Backends opt-in на новый API через override.

### Tests

`crates/shamir-engine/src/index2/write_ops.rs::tests`:

- `apply_set_posting_writes_to_store`.
- `apply_remove_posting_writes_tombstone`.
- `apply_bump_fts_stats_delegates_to_backend` (моковый IndexBackend с
  counter, проверяем что `apply_in_memory` вызван с правильными
  values).
- `apply_empty_ops_noop`.
- `apply_mixed_ops_in_order`.

### Acceptance

- Все existing tests workspace зелёные.
- 5 новых tests для apply path.
- Default `plan_*` возвращает `Ok(vec![])` для всех existing
  backends — bench `order_by_pipeline` не регрессирует.

### Что не делать

- Не переписывать backends на `plan_*` — это 1.1.B–F.
- Не удалять `on_insert/on_update/on_delete` — 1.1.G.

---

## 1.1.B. `FtsBackend` — migrate to planner API

**Срок:** 0.5 дня.

### Что

В `crates/shamir-engine/src/index2/fts_backend.rs`:

1. Override `plan_insert / plan_update / plan_delete` — возвращают
   `Vec<IndexWriteOp>` со списком `SetPosting / RemovePosting`.
2. Существующий `on_insert / on_update / on_delete` остаются как
   **wrappers**: `let ops = self.plan_*(rid, val).await?; apply_index_ops(&ops, &self.store, self).await`.
3. Удалить прямые `self.store.set / remove` из тела `plan_*` — они
   возвращают ops, не пишут.

### Tests

В существующем tests module:

- `plan_insert_returns_set_postings` — assert structure of returned ops.
- `plan_update_returns_set_for_new_remove_for_old`.
- `plan_delete_returns_remove_for_all_postings`.
- `equivalence_plan_apply_vs_direct_on_insert` — для одного и того же
  insert: state store после `on_insert` (старый путь) == state после
  `apply_index_ops(plan_insert)`. Bit-for-bit.

### Acceptance

- Existing tests `fts_*` зелёные.
- 4 новых tests.
- Bench `order_by_pipeline` regression ≤ 5%.

---

## 1.1.C. `FtsRankedBackend` + `BumpFtsStats`

**Срок:** 1 день.

### Что

В `crates/shamir-engine/src/index2/fts_ranked_backend.rs`:

1. Override `plan_insert / plan_update / plan_delete` — возвращают
   `SetPosting / RemovePosting` **плюс** `BumpFtsStats { doc_len, sign }`
   для doc_count / sum_doc_len atomic counters.
2. Override `apply_in_memory` — обрабатывает `BumpFtsStats`:
   `self.stats.on_insert(doc_len)` для sign=+1, `on_delete(doc_len)`
   для sign=-1.
3. `apply_index_ops` helper диспатчит: `Set/RemovePosting → store`,
   `BumpFtsStats → backend.apply_in_memory`.
4. Wrappers (`on_insert / on_update / on_delete`) сохраняются.

### Тонкость

BumpFtsStats применяется **только** в `apply_in_memory`, не вписывается
в store. На commit path (Stage 4) tx context должен вызвать
`apply_in_memory` рядом с `store.transact(set/remove ops)`. Для
non-tx — `apply_index_ops` делает оба call в одном flow.

### Tests

- `plan_insert_bumps_stats_with_sign_plus_one`.
- `plan_delete_bumps_stats_with_sign_minus_one`.
- `bump_fts_stats_doesnt_touch_store`.
- `equivalence_plan_apply_vs_direct_on_insert` (как в 1.1.B).
- `bm25_ranking_unchanged_after_plan_path` — full lookup on 5 docs
  через plan→apply даёт тот же BM25 ranking что direct `on_insert`.

### Acceptance

- Existing tests зелёные.
- 5 новых tests.
- `recall_at_10_on_1k_vectors` не регрессирует.

---

## 1.1.D. `FunctionalBackend` — migrate to planner API

**Срок:** 0.5 дня.

Аналогично 1.1.B, но для `functional_backend.rs`. Hash + posting key
encoding не меняется.

### Tests

- `plan_insert_returns_one_set_posting`.
- `plan_update_returns_remove_old_set_new_if_hash_changes`.
- `plan_update_returns_empty_if_hash_same`.
- `equivalence_plan_apply_vs_direct_on_insert`.

---

## 1.1.E. Legacy `IndexManager` — migrate to planner API

**Срок:** 1 день.

`crates/shamir-engine/src/index/index_manager.rs` — старая
не-index2 машина. Несколько сложнее потому что:

1. У IndexManager есть `on_records_created/_unique_batch` методы —
   они тоже возвращают `Vec<IndexWriteOp>`. Добавить
   `plan_records_created_batch` etc.
2. `unique` validation требует читать существующие postings — это
   read-path, не write. Остаётся внутри plan phase.

### Tests

- `plan_insert_returns_ops_for_each_index`.
- `plan_batch_aggregates_ops_in_order`.
- `equivalence_plan_apply_vs_direct_on_record_created`.
- `unique_index_collision_in_plan_phase` — два insert'а с тем же
  значением unique-индекса в одном plan call → backend returns Err.

### Acceptance

- Все existing tests `index_manager` зелёные.
- 4 новых tests.

---

## 1.1.F. `SortedIndexManager` — migrate to planner API

**Срок:** 0.5 дня.

Аналогично 1.1.E для `sorted_index_manager.rs`. Структура попроще —
sorted index пишет один posting per record.

### Tests

- `plan_insert_returns_sorted_posting`.
- `plan_delete_returns_remove_sorted_posting`.
- `equivalence_plan_apply_vs_direct`.

---

## 1.1.G. Cleanup — убрать старые `on_*` методы из trait

**Срок:** 0.5 дня. **Зависит от:** 1.1.B–F (все backends мигрированы).

### Что

После 1.1.B–F все backends override `plan_*`. Старые `on_insert /
on_update / on_delete` в IndexBackend trait и в каждом backend
становятся wrapper-only:

```rust
async fn on_insert(&self, rid, rec) -> Result<(), IndexError> {
    let ops = self.plan_insert(rid, rec).await?;
    apply_index_ops(&ops, &self.store, self).await
}
```

Это можно либо **оставить на trait** как convenience default (один
default-impl для всех backends), либо **переместить wrappers**
наружу в `TableManager` и удалить `on_*` from trait целиком.

**Выбор: переместить наружу.** Trait чище, single responsibility
(`IndexBackend` только планирует, executor applies).

В `TableManager` добавить helper:

```rust
async fn index_apply_now(&self, ops: &[IndexWriteOp], backend: &dyn IndexBackend) -> DbResult<()> {
    apply_index_ops(ops, &self.info_store, backend).await.map_err(DbError::from)
}
```

И в каждом callsite `backend.on_insert(rid, val).await` заменить на
`let ops = backend.plan_insert(rid, val).await?; self.index_apply_now(&ops, backend.as_ref()).await?;`.

### Tests

- Все existing tests зелёные.
- Никаких новых тестов (это pure refactor).

### Acceptance

- `rg 'fn on_insert\|fn on_update\|fn on_delete' crates/shamir-engine/src/index2/*.rs crates/shamir-engine/src/index/*.rs` — 0 совпадений в trait + impls (только в tests если нужно).
- `IndexBackend` trait — нет `on_*` методов в публичном API.

---

## 1.2.A. `HnswAdapter::staged` поле + private `stage()` API

**Срок:** 0.5 дня.

### Что

В `crates/shamir-engine/src/index2/vector/hnsw_adapter.rs`:

1. Добавить поле `staged: scc::HashMap<TxId, Vec<StagedVector>>` (где
   `TxId` импортируем из `shamir_tx::TxId`).
2. Добавить `struct StagedVector { rid, vec, replaces: Option<usize> }`.
3. Private method `async fn stage(&self, tx_id, rid, vec)` — atomic
   push в `staged[tx_id]`.

### Public API

Не меняется. `upsert(rid, vec)` остаётся как сейчас — non-tx путь.
Только foundation для 1.2.B/C.

### Tests

- `stage_pushes_into_per_tx_buffer` (manual call для тестов).
- `stage_with_different_tx_ids_isolated`.

### Acceptance

- All existing HNSW tests зелёные.
- 2 новых tests.
- Bench `vector_search` не регрессирует.

---

## 1.2.B. `commit_staged` / `rollback_staged`

**Срок:** 0.5 дня. **Зависит от:** 1.2.A.

### Что

```rust
impl HnswAdapter {
    /// Drain staged vectors and apply to the live graph. Tombstone
    /// any pre-existing internal_id this tx was replacing. Atomic
    /// w.r.t. the staged map (single drain operation).
    pub async fn commit_staged(&self, tx_id: TxId) -> Result<(), VectorError>;

    /// Drop all staged vectors for this tx — no graph mutation.
    pub async fn rollback_staged(&self, tx_id: TxId);
}
```

### Tests

- `commit_staged_inserts_all_into_graph` (stage 5 vecs, commit, search
  finds them).
- `rollback_staged_drops_without_graph_mutation` (stage 1000 vecs,
  rollback, graph size unchanged).
- `commit_staged_idempotent_on_empty` (commit без stage'нутых ops —
  no-op).
- `commit_staged_handles_replace` (stage update'а existing rid —
  tombstone old internal_id, insert new).

### Acceptance

- 4 новых tests.
- Existing HNSW tests зелёные.

---

## 1.2.C. `upsert(tx: Option<TxId>)` + in-tx search merge

**Срок:** 1 день. **Зависит от:** 1.2.A, 1.2.B.

### Что

1. Изменить signature `upsert(&self, rid, vec)` → `upsert(&self, rid, vec, tx: Option<TxId>)`:
   - `None` → `insert_now` (текущий behavior).
   - `Some(tx_id)` → `stage(tx_id, rid, vec)`.
2. Аналогично для `delete(rid, tx: Option<TxId>)`.
3. Изменить `search(query, k)` → `search(query, k, tx: Option<TxId>)`:
   - Non-tx: existing logic.
   - Tx: overscan committed graph, отфильтровать tombstones, **смержить**
     с brute-force scan по `staged[tx_id]` (для current tx). Возвращает
     `top-k` из объединённого множества.
4. `VectorAdapter` trait — добавить `tx: Option<TxId>` к сигнатурам
   `upsert/delete/search`. BruteForceAdapter — игнорирует tx (он не
   нужен этой имплементации, но trait единый).

### Tests

- `tx_upsert_then_search_sees_staged` (1.2.A's promise).
- `tx_search_excludes_other_tx_staged` (изоляция между tx).
- `tx_search_overscan_handles_tombstones` (если что-то tombstoned
  pre-commit).
- `non_tx_search_unchanged` (regression guard).

### Acceptance

- 4 новых tests.
- All existing HNSW tests зелёные.
- Bench `hnsw_search_with_staged_100` < 2× от `hnsw_search` (acceptable
  overhead).
- Bench `hnsw_search_non_tx_baseline` ≤ 1% regression.

---

## 1.3. `StagingStore` + tests

**Срок:** 0.5 дня.

### Что

Новый файл `crates/shamir-tx/src/staging_store.rs`:

```rust
//! In-memory write buffer for a single transaction.

use bytes::Bytes;
use scc::HashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey, Store};
use std::sync::Arc;

#[derive(Debug, Clone)]
enum StagedOp { Set(Bytes), Remove }

pub struct StagingStore {
    base: Arc<dyn Store>,
    writes: HashMap<RecordKey, StagedOp>,
}

impl StagingStore {
    pub fn new(base: Arc<dyn Store>) -> Self;

    pub async fn get(&self, k: RecordKey) -> DbResult<Bytes>;
    pub async fn set(&self, k: RecordKey, v: Bytes);
    pub async fn remove(&self, k: RecordKey);

    /// Drain accumulated writes into a `Vec<KvOp>` for an atomic
    /// `base.transact(ops)` call. Consumes self.
    pub fn drain(self) -> Vec<KvOp>;

    /// Number of staged writes — for telemetry / max-size cap.
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

В `crates/shamir-tx/src/lib.rs`:
```rust
pub mod staging_store;
pub use staging_store::StagingStore;
```

### Tests

- `get_after_set_returns_staged_value`.
- `get_after_remove_returns_not_found_even_if_base_has_key`.
- `get_falls_through_to_base_if_not_staged`.
- `set_then_remove_collapses_to_remove`.
- `remove_then_set_collapses_to_set`.
- `drain_produces_atomic_kvop_batch` — после drain все ops в `Vec<KvOp>`
  правильно отражают финальное состояние.
- `len_tracks_unique_keys` (set одного и того же ключа дважды → len=1).

### Acceptance

- 7 новых tests.
- No-overhead на existing code (StagingStore не используется production yet).

---

## Что не делаем здесь

- Не создаём `TxContext` — это Этап 2.
- Не интегрируем в executor — это Этап 4.
- Не трогаем read pipeline через `Option<&TxContext>` — это Этап 3.
- Не пишем `MvccStore` — это Этап 3.
- Не реализуем `LayeredInterner` — это Этап 2.

## Что предостерегает

После 1.1.G старые `on_*` методы исчезают из публичного API
`IndexBackend`. Любой код вне `shamir-engine` который их использовал
сломается на compile-time. Перед 1.1.G:

```bash
rg 'IndexBackend::on_(insert|update|delete)' crates/ --type rust
rg '\.on_insert\(|\.on_update\(|\.on_delete\(' crates/ --type rust \
   --glob '!**/index2/**'
```

Все совпадения должны быть переписаны на planner API заранее. Если
обнаружится callsite в `shamir-db` / `shamir-server` — выделить
отдельный sub-stage `1.1.G.1: migrate downstream callers`.
