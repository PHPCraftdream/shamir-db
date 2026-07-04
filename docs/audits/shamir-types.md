# shamir-types — оптимизация производительности

## Обзор
Фундаментальный крейт: Value-модель, RecordId, string-interner, codecs (JSON/MsgPack/Bincode), sort-codec.
Горячие пути: interner.touch_ind (каждая запись/чтение), msgpack_to_inner / inner_to_msgpack (каждый read/write), Value::Hash (фильтры).

---

## 🔴 Критические оптимизации

### 1. InternerKey: заменить `Bytes` на `u64` inline
**Файл:** `core/interner/interned_key.rs:8`
**Сейчас:** `InternerKey(Bytes)` — heap-allocation через `Bytes::copy_from_slice` на каждый new (1/2/4/8 bytes).
**Проблема:** `Bytes` — heap-аллокация + atomic ref-count + indirect pointer. При `id ≤ u32::MAX` (4 млрд ключей, более чем достаточно) можно хранить inline.
**Решение:**
```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct InternerKey(u64);
```
- `new(id)` = `Self(id)` — zero alloc, one MOV
- `id()` = `self.0` — один register read
- `Hash/Eq/Ord` — тривиально через `u64`
- Сериализация: `.to_be_bytes()` / `.from_be_bytes()` — без heap
- **Ожидаемый эффект:** −1 alloc на каждый `touch_ind` + `get_str` + de-intern path; hash/eq/incmp на inline u64 вместо indirect Bytes. Оценка: −30-50 ns на каждый touch_ind new path.

### 2. Interner: reverse vec clone-and-swap → append-only box
**Файл:** `core/interner/interner.rs:112-124`
**Сейчас:** Каждый `touch_ind` (new key) клонирует **весь** `Vec<Option<UserKey>>` через CAS-loop.
**Проблема:** O(N) clone на каждый new key. При 1000 полей — клонируется 1000 Option<UserKey>.
**Решение:** Использовать `boxcar::Vec` (lock-free append-only concurrent vec) или `std::sync::atomic::AtomicPtr` + raw alloc:
```rust
use boxcar::Vec as AppendVec;
reverse: AppendVec<UserKey>,  // lock-free push, index-based read
```
- `push(key)` — lock-free, O(1), no clone
- `get(id)` — one atomic load, no ArcSwap
- **Ожидаемый эффект:** O(N) clone → O(1) append. На batch-insert 1000 полей: −1000× clone.

### 3. msgpack_to_inner: двойной парсинг
**Файл:** `codecs/interned/messagepack.rs:25-28`
**Сейчас:** `read_value(&mut &*bytes)` парсит весь MsgPack в `rmpv::Value` дерево, затем рекурсивно конвертирует в InnerValue.
**Проблема:** Двойная аллокация — сначала rmpv-дерево, потом InnerValue-дерево. Для записи с 50 полями: 50 лишних аллокаций ключей и значений.
**Решение:** Написать zero-copy cursor-based decoder поверх `rmpv::decode` primitives:
```rust
fn msgpack_to_inner(interner: &Interner, bytes: &[u8]) -> Result<InnerValue, CodecError> {
    let mut cursor = std::io::Cursor::new(bytes);
    decode_value_from_cursor(&mut cursor, interner, 0)
}
```
- Читать тип элемента напрямую через `read_marker()`, без построения rmpv::Value.
- **Ожидаемый эффект:** −50% heap alloc на decode path, −20-30% wall time.

### 4. Value::Hash для Set/Map — allocation-free XOR
**Файл:** `types/value.rs:471-492`
**Сейчас:** Уже использует XOR-sum — ✅ хорошо. Но создаёт `FxHasher::default()` на каждый элемент.
**Проблема:** `FxHasher::default()` — zero-cost, но `v.hash(&mut hasher)` рекурсивно обходит всё поддерево.
**Решение:** Для `InnerValue` (InternerKey variant) — pre-compute hash при создании и хранить в Value. Это убирает рекурсивный обход при hash Map/Set.
- **Ожидаемый эффект:** −O(depth) hash-вычислений при lookup в hash-based индексах.

---

## 🟡 Значимые оптимизации

### 5. json_to_inner / json_value_to_inner — через serde_json::Value
**Файл:** `codecs/interned/json.rs:21-24`
**Сейчас:** `serde_json::from_slice` → `serde_json::Value` → затем конвертация в InnerValue.
**Проблема:** Промежуточное `serde_json::Value` дерево — двойная аллокация.
**Решение:** Написать custom `serde::Deserializer` который сразу строит InnerValue с interning (как уже сделано для UserValue через ValueVisitor). Или использовать `serde_json::Deserializer::from_slice` + `deserialize_any` напрямую.

### 6. encode_str / encode_bytes — byte-by-byte push
**Файл:** `core/sort_codec.rs:113-125`
**Сейчас:** `buf.push(b)` в цикле для каждого байта. `Vec::push` — amortized O(1), но branch на capacity check каждый раз.
**Решение:** `buf.extend_from_slice(s.as_bytes())` для не-нулевых сегментов, только для 0x00 — специальная обработка. Или: size-hint upfront `buf.reserve(s.len() + 2)`.
- **Ожидаемый эффект:** −30% на encode_str для типичных строк.

### 7. RecordId::new — rand::thread_rng
**Файл:** `types/record_id.rs:36`
**Сейчас:** `rand::thread_rng().fill_bytes(&mut bytes[8..16])` — thread-local ChaCha, ~5 ns.
**Проблема:** Для non-cryptographic ID можно использовать `Xoshiro256++` или даже `wyhash`-based PRNG — ~1-2 ns.
**Решение:** thread_local `Cell<Xoshiro256PlusPlus>` или `Pcg64`.
- **Ожидаемый эффект:** −3-4 ns на каждый RecordId::new (hot insert path).

### 8. base_x encode/decode — медленная реализация
**Файл:** `types/base.rs:16-18`
**Сейчас:** `base_x::encode/decode` — generic, работает для любого алфавита, division-based.
**Проблема:** base58 encode 16 bytes ≈ 200-500 ns (big-integer division).
**Решение:** Для 16-byte fixed-size: lookup table или `bs58` crate (оптимизирован specifically для base58).
- **Ожидаемый эффект:** −50-70% на RecordId display/debug formatting.

### 9. `new_map()` без capacity в codec decode
**Файлы:** `codecs/interned/messagepack.rs:162`, `codecs/interned/json.rs:165,229`
**Сейчас:** `new_map()` — без предвыделения.
**Решение:** Использовать `new_map_wc(map.len())` или хотя бы size_hint.

---

## 🟢 Мелкие оптимизации

### 10. `parking_lot` dependency — не используется в hot paths?
**Файл:** `Cargo.toml:24`
**Сейчас:** `parking_lot = "0.12"` в зависимостях. Проверить использование — если только для тестов/fixtures, вынести в `[dev-dependencies]`.

### 11. `regex` dependency — compile-time overhead
**Файл:** `Cargo.toml:40`
**Проблема:** `regex` crate = большой compile time. Если используется только для key validation — заменить на `memchr` или ручную проверку.

### 12. `once_cell` → `std::sync::LazyLock`
**Файл:** `Cargo.toml:17`, используется в `value.rs:293`
**Сейчас:** Уже используется `LazyLock` в value.rs. Проверить, остался ли `once_cell` в зависимостях и убрать если не нужен.

---

## Приоритет
| # | Улучшение | Ожидаемый эффект | Сложность | Path |
|---|-----------|------------------|-----------|------|
| 1 | InternerKey u64 inline | −1 alloc/touch, −30-50ns | Средняя | Write+Read |
| 2 | Reverse vec → boxcar/append-only | O(N)→O(1) new key | Средняя | Write |
| 3 | Zero-copy msgpack decode | −50% alloc decode | Высокая | Read |
| 6 | sort_codec reserve | −30% encode_str | Низкая | Index Write |
| 7 | Faster PRNG RecordId | −3-4ns/insert | Низкая | Write |
| 8 | bs58 for RecordId | −50-70% Display | Низкая | Read |
| 5 | Zero-copy JSON decode | −30% decode | Средняя | Read |
| 4 | Pre-computed Value hash | −O(depth) hash | Средняя | Filter |
| 9 | with_capacity in codecs | −rehash | Низкая | Read+Write |
