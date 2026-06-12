# shamir-engine — оптимизация производительности

## Обзор
Ядро БД: TableManager, read/write exec, filter compile+eval, validators, tx/commit, WAL recovery.
Самый большой и критичный крейт — каждый hot path идёт через него.

---

## 🔴 Критические оптимизации

### 1. resolve_field_ref: InternerKey::new(id) на каждом lookup
**Файл:** `query/filter/resolve.rs:19-31`
**Сейчас:**
```rust
pub fn resolve_field_ref<'a>(record: &'a InnerValue, path: &[u64]) -> Option<&'a InnerValue> {
    for &id in path {
        match cur {
            InnerValue::Map(map) => {
                let key = InternerKey::new(id);  // heap alloc via Bytes!
                cur = map.get(&key)?;
            }
```
**Проблема:** `InternerKey::new(id)` создаёт `Bytes` через `copy_from_slice` — heap alloc на КАЖДЫЙ field lookup на КАЖДОЙ записи. Для фильтра `{age: {$gt: 20}}` при 1M записей = 1M heap alloc только на key construction.
**Решение:** Если InternerKey станет `u64` inline (см. shamir-types #1), эта проблема исчезает. До того: cached `InternerKey` в compile time — `CompactPath` хранить как `SmallVec<InternerKey>` вместо `SmallVec<u64>`.
- **Ожидаемый эффект:** −1M heap alloc на 1M filtered rows. Ускорение filter eval на ~30-50%.

### 2. FilterNode::In — linear scan через values
**Файл:** `query/filter/filter_node.rs:191-242`
**Сейчас:** `for (i, fv) in values.iter().enumerate()` — O(N) linear scan на каждую запись.
**Проблема:** `{status: {$in: ["a","b","c",...100 values]}}` × 1M rows = 100M comparisons.
**Решение:** При compile time, если все values literal — построить `HashSet<InnerValue>` (или `TSet` с Fx hash). Lookup O(1) вместо O(N).
```rust
FilterNode::InHashSet {
    field_path: CompactPath,
    set: TSet<InnerValue>,  // pre-built at compile time
    negate: bool,
}
```
- **Ожидаемый эффект:** O(N) → O(1) для `$in` с literals. Для 100 values: 100× speedup.

### 3. UPDATE: per-row set_returning_version вместо batch
**Файл:** `table/write_exec.rs:303-324`
**Сейчас:** `for (id, old_record) in &matched { ... self.set_returning_version(*id, &new_record).await?; }` — по одному set на каждую запись.
**Проблема:** N async await + N backend writes. Для UPDATE 1000 rows = 1000 отдельных I/O.
**Решение:** Batched update: `Store::transact(vec![KvOp::Set(...); N])` — один backend transaction.
- **Ожидаемый эффект:** −N× fsync/commit overhead. Для redb/nebari/persy: 1000× fsync → 1× fsync.

### 4. execute_insert: query_value_to_inner per row без batch interning
**Файл:** `table/write_exec.rs:55-64`
**Сейчас:** `query_value_to_inner(&resolved, interner)` для каждого value. Каждый вызов делает `touch_ind` для каждого ключа — DashMap lookup.
**Проблема:** 100 rows × 10 fields = 1000 DashMap lookups, из которых 990 — cache hits (повторяющиеся имена полей).
**Решение:** Per-batch intern cache (уже сделано для tx path в `execute_insert_tx` — `FxHashMap<String, InternerKey>`). Проделать то же для non-tx path.
- **Ожидаемый эффект:** −90% DashMap lookups на batch insert.

---

## 🟡 Значимые оптимизации

### 5. compile_filter: Regex::new на каждый LIKE/REGEX
**Файл:** `query/filter/compile.rs:71-98`
**Сейчас:** `Regex::new(pattern)` при компиляции — хорошо (один раз). ✅
**Но:** Для FTS brute-force (`FtsMatch`) — токенизация строки на КАЖДОЙ записи.
**Решение:** Pre-tokenize query при compile, хранить `Vec<String>` tokens. ✅ Уже сделано.

### 6. merge_inner_maps — clone всего old_record
**Файл:** `table/write_exec.rs:305`
**Сейчас:** `let new_record = merge_inner_maps(old_record, set_map);` — клонирует всю Map.
**Проблема:** Для 50-field record + 2 fields to update — клонируются все 50 entries.
**Решение:** In-place update если old_record mutably borrowed, или copy-on-write (struct-of-arcs).
- **Ожидаемый эффект:** −90% clone overhead на UPDATE.

### 7. FilterNode::ContainsAny / ContainsAll — nested loop O(N×M)
**Файл:** `query/filter/filter_node.rs:288-300`
**Сейчас:** `values.iter().any(|fv| ... list.iter().any(...))` — O(N×M).
**Решение:** Pre-build `TSet` для ContainsAny/ContainsAll literal values.

### 8. read_impl — batch_size hardcoded 1000
**Файл:** `table/read_exec.rs:73`
**Сейчас:** `let batch_size = 1000;` — hardcoded.
**Решение:** Использовать `shamir_tunables::store_defaults::FULL_SCAN_BATCH` — уже есть константа.

---

## Приоритет
| # | Улучшение | Ожидаемый эффект | Сложность | Path |
|---|-----------|------------------|-----------|------|
| 1 | InternerKey cache в resolve | −1M heap alloc/filter | Средняя (зависит от types) | Read (filter) |
| 2 | InHashSet для $in | O(N)→O(1) | Низкая | Read (filter) |
| 3 | Batched UPDATE | −N× fsync | Средняя | Write |
| 4 | Batch intern cache (non-tx) | −90% DashMap lookups | Низкая | Write |
| 6 | merge_inner_maps in-place | −90% clone on UPDATE | Средняя | Write |
| 7 | ContainsAny/All HashSet | O(N×M)→O(N) | Низкая | Read (filter) |
