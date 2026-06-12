# S.H.A.M.I.R. — Обзорный аудит производительности

## Цель
Максимальное ускорение чтения и записи при разных условиях: fx hash, O(1), scc, алгоритмы.

---

## Сводная таблица критических оптимизаций

| # | Крейт | Проблема | Решение | Ожидаемый эффект | Сложность | Path |
|---|-------|----------|---------|------------------|-----------|------|
| 1 | shamir-types | InternerKey = Bytes (heap alloc на каждый key) | InternerKey(u64) inline | −1 alloc/op × весь проект | Средняя | R+W |
| 2 | shamir-types | Interner reverse-vec clone-and-swap O(N) | boxcar::Vec (lock-free append) | O(N)→O(1) new key | Средняя | W |
| 3 | shamir-types | msgpack_to_inner — двойной parse через rmpv::Value | Cursor-based zero-copy decoder | −50% alloc decode | Высокая | R |
| 4 | shamir-engine | resolve_field_ref: InternerKey::new(id) на каждом lookup | Cached InternerKey в compile time (SmallVec<InternerKey>) | −1M heap alloc/scan | Средняя | R |
| 5 | shamir-engine | FilterNode::In — O(N) linear scan | Pre-built HashSet для literals | O(N)→O(1) | Низкая | R |
| 6 | shamir-engine | UPDATE: per-row set вместо batch | Store::transact(vec![KvOp]) | −N× fsync | Средняя | W |
| 7 | shamir-engine | execute_insert non-tx: нет batch intern cache | FxHashMap cache как в tx path | −90% DashMap lookups | Низкая | W |
| 8 | shamir-storage | CachedStore: O(N log N) sort на iter_stream | DashMap → scc::TreeIndex | −100% sort overhead | Средняя | R |
| 9 | shamir-storage | MemBuffer drain_once: double clone | Single-pass clone | −50% alloc drain | Низкая | W |
| 10 | shamir-server | Subscription filter на serde_json::Value | FilterNode на InnerValue | −50-80% sub CPU | Высокая | R+W |
| 11 | shamir-engine | merge_inner_maps clone всего old_record | In-place / copy-on-write | −90% clone UPDATE | Средняя | W |
| 12 | shamir-tx | StagedRow Live: serialize на каждый as_bytes | Dual representation (inner + bytes) | −N serialize commit | Средняя | W |

---

## Архитектурные рекомендации (по приоритету)

### Tier 1 — Максимальный эффект / Реализовать первыми

**1. InternerKey: Bytes → u64 (shamir-types)**
Это фундаментальное изменение, которое ускорит ВСЁ. Каждый interned key lookup в системе (filter, codec, index, subscription) платит heap alloc через `Bytes::copy_from_slice`. Inline u64 = zero alloc, один register MOV. Затрагивает shamir-types, shamir-engine, shamir-tx, shamir-index, но интерфейс `id(): u64` уже есть — внутренности меняются локально.

**2. FilterNode::In → HashSet (shamir-engine)**
Минимальное изменение с огромным эффектом. При compile time для `$in: [...]` с literal values — построить `TSet<InnerValue>`. Lookup O(1) вместо O(N). Для batch-запросов с `$in` на 100+ values — 100× ускорение filter eval.

**3. CachedStore → TreeIndex (shamir-storage)**
Заменить DashMap на scc::TreeIndex в CachedStore. Убирает O(N log N) sort на каждом full scan и O(N) prefix filter. CachedStore используется для индексов — каждый index scan проходит через него.

### Tier 2 — Значительный эффект

**4. Batch intern cache для non-tx insert (shamir-engine)**
В `execute_insert` (non-tx path) добавить `FxHashMap<String, InternerKey>` cache — уже реализовано в tx path (`execute_insert_tx`). 10 строк кода, −90% DashMap lookups на batch insert.

**5. Subscription FilterNode на InnerValue (shamir-server)**
Сейчас каждый push event: InnerValue → serde_json::Value → filter_matches_value(json). Заменить на FilterNode (уже есть в engine) + matches на InnerValue. Убирает serde alloc на каждый event × subscriber.

**6. resolve_field_ref cached keys (shamir-engine)**
CompactPath хранит `SmallVec<u64>` — каждый lookup делает `InternerKey::new(id)`. Хранить `SmallVec<InternerKey>` в FilterNode, строить один раз при compile. −1 heap alloc per field per row.

### Tier 3 — Средний эффект

**7. UPDATE batched write (shamir-engine)**
Per-row `set_returning_version` → собрать все updates в `Vec<KvOp>` → `Store::transact()`. Один backend transaction вместо N. Особенно важно для redb/nebari/persy (N fsync → 1 fsync).

**8. Zero-copy msgpack decode (shamir-types)**
msgpack_to_inner строит промежуточное rmpv::Value дерево. Cursor-based decoder (read_marker + read payload) — без промежуточного дерева. −50% heap alloc на decode path.

**9. Interner reverse-vec: append-only (shamir-types)**
CAS-loop clone-and-swap → lock-free append (boxcar::Vec или raw AtomicPtr). O(N) clone → O(1) append при каждом новом ключе.

---

## Крейты без оптимизаций

Эти крейты либо proc-macros (compile-time only), либо тонкие API слои, либо уже хорошо оптимизированы:

- **shamir-query-builder** — builder, не hot path
- **shamir-query-builder-macros** — proc-macro, compile-time only
- **shamir-sdk-macros** — proc-macro, compile-time only
- **shamir-tunables** — atomic reads, zero-overhead ✅
- **shamir-sdk** — thin API layer
- **shamir-db** — facade, делегирует в engine/server
- **shamir-connect** — crypto по definition медленный, уже хорошо оптимизирован
- **shamir-transport-tcp/ws** — I/O-bound, минимальный CPU overhead

## Крейты уже хорошо оптимизированные

- **shamir-index** — SIMD vector distance (AVX-512/AVX2/NEON), zero-copy posting keys, fxhash ✅
- **shamir-tunables** — AtomicUsize Relaxed loads ✅
- **shamir-collections** — fxhash по умолчанию ✅

---

## Порядок реализации

Рекомендую реализовывать в таком порядке — каждый шаг даёт измеримый эффект и не блокирует следующие:

1. **FilterNode::In HashSet** (shamir-engine) — 1 час, мгновенный эффект
2. **Batch intern cache non-tx** (shamir-engine) — 30 мин
3. **InternerKey u64 inline** (shamir-types) — 1 день, фундаментальный эффект
4. **CachedStore TreeIndex** (shamir-storage) — полдня
5. **Subscription InnerValue filter** (shamir-server) — полдня
6. **UPDATE batched write** (shamir-engine) — полдня
7. **resolve_field_ref cached keys** (shamir-engine) — 2 часа (после InternerKey u64)
8. **Zero-copy msgpack decode** (shamir-types) — 1-2 дня
9. **Interner append-only vec** (shamir-types) — полдня
10. **merge_inner_maps in-place** (shamir-engine) — полдня

Общая оценка: реализация Tier 1 (пункты 1,2,3) может дать **2-5× ускорение** на типичных read/write workloads.
