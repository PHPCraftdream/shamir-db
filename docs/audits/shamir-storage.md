# shamir-storage — оптимизация производительности

## Обзор
Трейт `Store`/`Repo` + реализации: InMemory (scc::TreeIndex), Cached (DashMap), MemBuffer (moka), disk backends (redb/sled/fjall/nebari/persy/canopy).
Ключевой крейт — каждый read/write проходит через один из этих бэкендов.

---

## 🔴 Критические оптимизации

### 1. CachedStore: O(N log N) sort на каждом iter_stream
**Файл:** `storage_cached.rs:240-263`
**Сейчас:** `iter_stream` клонирует ВСЮ DashMap в Vec, потом сортирует.
**Проблема:** O(N) clone + O(N log N) sort на каждый full scan. Для 100k записей — миллионы операций.
**Решение:** Заменить `DashMap` на `scc::TreeIndex` (как InMemoryStore). TreeIndex уже отсортирован — `iter_stream` = O(K) без clone/sort. DashMap только для lookup; но TreeIndex даёт и lookup O(log N), и sorted range бесплатно.
- **Ожидаемый эффект:** −100% clone/sort overhead на scan. Для 100k записей: с ~50ms до ~1ms.

### 2. CachedStore: scan_prefix_stream — O(N) filter вместо range scan
**Файл:** `storage_cached.rs:265-290`
**Сейчас:** Full iter + `.filter(starts_with)` — O(N) на каждый prefix scan.
**Решение:** TreeIndex::range от prefix — O(log N + K). Аналогично InMemoryStore.

### 3. MemBufferStore: drain_once — double clone (snapshot + sets)
**Файл:** `storage_membuffer.rs:336-353`
**Сейчас:**
```rust
let snapshots: Vec<(RecordKey, Slot)> = state.dirty.iter()
    .take(batch_size)
    .map(|e| (e.key().clone(), e.value().clone()))  // clone #1
    .collect();
// ...
for (k, slot) in &snapshots {
    sets.push((k.clone(), v.clone()))  // clone #2
}
```
**Проблема:** Каждая dirty entry клонируется дважды. Для batch 256 — 512 лишних клонирований (каждое = Bytes clone, что reference-counted но всё равно overhead).
**Решение:** Одно клонирование — `snapshots` → `sets/removes` напрямую, без промежуточного копирования:
```rust
let mut sets = Vec::with_capacity(batch_size);
for e in state.dirty.iter().take(batch_size) {
    match e.value() {
        Slot::Live(v) => sets.push((e.key().clone(), v.clone())),
        Slot::Tombstone => removes.push(e.key().clone()),
    }
}
```
- **Ожидаемый эффект:** −50% alloc на drain_once.

### 4. MemBufferStore: get() — triple lookup (cache → dirty → inner)
**Файл:** `storage_membuffer.rs:439-469`
**Сейчас:** Cache miss → dirty miss → inner get. Каждый层级 — отдельный lookup.
**Проблема:** На холодном старте 3 lookup'а на key. `moka.get()` — async,DirtyMap.get() — sync, inner.get() — async + `spawn_blocking`.
**Решение:** Для hot reads (типичный случай) — moka cache hit — уже быстро. Но для warm-up:
- Добавить batch prefetch: `get_many` уже делает это хорошо.
- Рассмотреть option: bypass dirty check если dirty пуст (AtomicBool check ≈ 1 ns vs DashMap.get ≈ 20 ns).

---

## 🟡 Значимые оптимизации

### 5. InMemoryStore: iter_stream snapshot clone всего дерева
**Файл:** `storage_in_memory.rs:140-146`
**Сейчас:** Snapshot через `Guard::new()` + `.iter().map(|(k,v)| clone).collect()` — клонирует ВСЕ entries.
**Проблема:** Для 1M записей — 1M clone(Bytes) + 1M clone(RecordKey).
**Решение:** Stream-ify: вместо snapshot → vec, использовать `async_stream` с Guard, yield-ить batch'и прямо из итератора (keep guard alive):
```rust
let g = Arc::new(scc::ebr::Guard::new());
Box::pin(stream! {
    let mut batch = Vec::with_capacity(batch_size);
    for (k, v) in self.data.iter(&*g) {
        batch.push((k.clone(), v.clone()));
        if batch.len() == batch_size {
            yield Ok(std::mem::take(&mut batch));
        }
    }
    if !batch.is_empty() { yield Ok(batch); }
})
```
- **Ожидаемый эффект:** Peak memory: O(batch_size) вместо O(N). Latency: первый batch через µs вместо ms.

### 6. CachedStore: `new_with_mode` — load ALL data upfront
**Файл:** `storage_cached.rs:50-63`
**Сейчас:** Конструктор загружает ВСЕ данные из inner. Для 1M записей — долгий старт.
**Решение:** Lazy loading: только cache-fill на miss. Или background load с ready-notify.

### 7. InMemoryStore::set — remove + insert (two ops)
**Файл:** `storage_in_memory.rs:115-122`
**Сейчас:** `remove(&key)` + `insert(key, value)` — два B+ tree traversal.
**Решение:** `TreeIndex::insert` уже upsert? Проверить API `scc::TreeIndex::insert` — он возвращает `Ok(())` on new, `Err` on exists. Нужен `upsert` через `entry` API если доступен. Или: `insert` + если Err → `remove` + `insert` (only on exists, редкий случай).

### 8. Store trait: `async_trait` overhead
**Файл:** `types.rs:30`
**Сейчас:** `#[async_trait]` на всех методах — boxed futures на каждый вызов.
**Проблема:** `Box::new` alloc на каждый `get()`, `set()`, `insert()`. На hot path — лишняя аллокация.
**Решение:** Для hot-path методов (`get`, `set`, `insert`) — заменить на manual `Pin<Box<dyn Future>>` return или использовать `impl Future` в trait (Rust 1.75+). Но это breaking change для всех impl.
- **Альтернатива:** По крайней мере для InMemoryStore — добавить `#[inline]` на методы и проверить что LLVM devirtualizes.

---

## 🟢 Мелкие оптимизации

### 9. RecordKey = Bytes — heap allocation на каждый key
**Файл:** `types.rs:8`
**Сейчас:** `pub type RecordKey = Bytes;` — 16 bytes RecordId → heap allocation.
**Решение:** Для InMemoryStore — использовать `[u8; 16]` как key вместо Bytes (Copy, no heap). Но это требует generics по key type — ломает трейт. Альтернатива: `smallvec` или inline Bytes.

### 10. `fully_unwrap_store` — async loop
**Файл:** `types.rs:331-337`
**Сейчас:** `while let Some(inner) = cur.raw_backend().await` — async await на каждый шаг.
**Проблема:** Обычно 1-2 шага — overhead минимален. Но `raw_backend()` — trait method с `async_trait` → boxed future на каждый вызов.
**Решение:** Sync метод `raw_backend_sync() -> Option<Arc<dyn Store>>` для unwrap без async.

---

## Приоритет
| # | Улучшение | Ожидаемый эффект | Сложность | Path |
|---|-----------|------------------|-----------|------|
| 1 | CachedStore → TreeIndex | −100% sort на scan | Средняя | Read (scan) |
| 2 | CachedStore prefix scan range | O(N)→O(logN+K) | Средняя | Read (prefix) |
| 3 | drain_once single clone | −50% alloc drain | Низкая | Write (flush) |
| 5 | InMemory streaming iter | O(N)→O(batch) peak mem | Средняя | Read (iter) |
| 8 | async_trait removal hot paths | −1 alloc/op | Высокая | Read+Write |
| 7 | InMemoryStore upsert | −50% B+ tree ops on update | Низкая | Write |
| 4 | MemBuffer triple lookup skip | −20ns on hot get | Низкая | Read |
| 6 | CachedStore lazy load | −startup time | Средняя | Init |
