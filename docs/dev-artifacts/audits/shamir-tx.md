# shamir-tx — оптимизация производительности

## Обзор
Транзакционный контекст: StagingStore (in-memory write buffer), MVCC store, layered interner, changefeed.

## 🔴 Критические

### 1. StagingStore::as_bytes для Live variant — serialize на cada read
**Файл:** `staging_store.rs:36-43`
**Сейчас:** `StagedRow::Live(v) => v.to_bytes()` — msgpack serialize на каждый `as_bytes()`.
**Проблема:** При commit drain — serialize всех staged rows. Но при read через `as_inner()` — deserialize для Bytes variant.
**Решение:** Dual-representation: при staging хранить И InnerValue И serialized Bytes. One-time serialize при insert.
```rust
StagedRow::Full { inner: InnerValue, bytes: Bytes }
```
- **Ожидаемый эффект:** −N serialize/deserialize на commit path.

### 2. MVCCStore — version chain traversal
**Проблема:** Чтение конкретной версии = traversal linked list версий.
**Решение:** Snapshots (copy-on-write) для read-mostly workloads. Или B+ tree по version вместо linked list.

## 🟡 Значимые

### 3. LayeredInterner — double lookup (overlay → base)
**Проблема:** Каждый touch_ind делает два lookup — сначала overlay, потом base interner.
**Решение:** FxHashMap cache как в engine (уже используется в execute_insert_tx).
