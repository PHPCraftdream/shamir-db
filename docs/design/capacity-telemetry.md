בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Capacity-telemetry — тонкие обёртки для adaptive capacity learning

> **Статус:** дизайн зафиксирован, импл не начат. Источник — пользовательская
> идея после первого flamegraph-passа на `tx_pipeline` бенче, который показал
> **~11% memory-bound** на write hot-path (`memmove` 4.48% + malloc/free family
> 6.7%) и неспособность точечно атрибуцировать аллокации из-за `strip = true` в
> `[profile.release]`. Таска — `#288` (TaskList). Соответствующее место — крейт
> `shamir-collections` (там уже `THasher`/`TMap`/`TSet`).

---

## 0. Зачем

Стандартный паттерн при оптимизации Rust-кода — расставлять `Vec::with_capacity(N)`
там, где известен размер up-front. Проблема: для **многих** мест размер «заведомо
неизвестен» — зависит от selectivity фильтра, кардинальности данных, размера
батча. Без данных приходится **угадывать magic-constants** (`Vec::with_capacity(16)` —
почему 16?) или оставлять `Vec::new()` и платить за серию `realloc()`-ов.

**Идея:** инструментировать коллекции тонкой обёрткой с **именем** и счётчиком
peak capacity. Прогоняем под `--features capacity-telemetry` → получаем JSON с
реальными peak'ами per-call-site → выставляем `with_capacity(peak)` data-driven.

Никаких догадок. Один прогон бенчей — точные числа для всех инструментированных
мест.

---

## 1. Дизайн

### 1.1 Имя — runtime `&'static str` через макрос

Не const generic (`TrackedVec<T, "name">`) — Rust const generics с `&'static str`
неудобны, шумный синтаксис, плохая discoverability. **Runtime `&'static str`**
в конструкторе через макрос-фасад:

```rust
let mut staged = tvec!("write_exec/staged_bytes", op.values.len());
let mut new_keys = tvec!("write_exec/new_base_keys", 0);
```

### 1.2 Макрос раскрывается в РАЗНЫЕ типы по cfg — НЕ type alias

Ключевое требование: в **prod нет лишних прослоек**. В off-feature макрос
раскрывается **буквально в `Vec::with_capacity(...)`** — не в alias `TVec<T>`,
не в zero-sized wrapper, а в сам `Vec`. Гарантированно zero cost, нет ABI-сдвига,
оптимизатор работает с обычным `Vec`.

```rust
#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {
        $crate::telemetry::TrackedVec::with_capacity_named($cap, $name)
    };
    ($name:literal) => {
        $crate::telemetry::TrackedVec::new_named($name)
    };
}

#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => { ::std::vec::Vec::with_capacity($cap) };
    ($name:literal)            => { ::std::vec::Vec::new() };
}
```

`TrackedVec<T>` реализует `Deref<Target = Vec<T>>` + `DerefMut<Target = Vec<T>>`
→ весь `Vec`-API работает прозрачно. Возвращаемый тип у `tvec!()` разный в
on/off (`Vec` vs `TrackedVec`), но Rust type inference + Deref-coercion это
поглощают; код на call-site не меняется.

> ⚠ **Caveat type-inference:** если в коде есть **явная аннотация типа** `let v:
> Vec<T> = tvec!(...)`, в on-feature это поломается (тип будет `TrackedVec`).
> Конвенция: использовать **inferred type** на call-site макроса. Если
> аннотация нужна — снять её и положиться на usage, или использовать
> сокращение `let v = tvec!(...); // : Vec<T>`.

### 1.3 Метрика — ТОЛЬКО peak_capacity (MVP минимум)

```rust
pub struct CapStats {
    pub peak_capacity:   AtomicUsize,
    pub creation_count:  AtomicU64,
}
```

- `peak_capacity` — `fetch_max` от capacity на момент `Drop` каждой instance.
  Capacity растёт **монотонно** в обычном пути (за исключением редкого
  `shrink_to_fit`), так что замер один раз при `Drop` корректно ловит peak за
  жизнь конкретной instance, а `fetch_max` агрегирует через все instance с тем
  же именем.
- `creation_count` — `fetch_add(1)` в конструкторе. Sanity: «сколько раз эта
  стракту создавали». Важно: один call-site может создавать тысячи instance
  за бенч.

**НЕ копим (MVP):** `final_len` histogram (p50/p95/p99), `realloc_count`,
`wasted_bytes`. Все три полезны, но дороже по overhead и шумят в первом
проходе. Добавятся в v2 при необходимости.

### 1.4 Хранилище — глобальный реестр по имени

```rust
use std::sync::OnceLock;
use scc::HashMap;

static REGISTRY: OnceLock<HashMap<&'static str, CapStats, THasher>> = OnceLock::new();

fn registry() -> &'static HashMap<&'static str, CapStats, THasher> {
    REGISTRY.get_or_init(|| HashMap::with_hasher(THasher::default()))
}
```

`scc::HashMap` — потому что lock-free; имена инструментированных коллекций
агрегируются across all threads workspace-wide. Имена ДОЛЖНЫ быть **уникальны**
across workspace; рекомендованная конвенция: `"<crate>/<module>/<role>"` →
`"engine/write_exec/staged_bytes"`.

### 1.5 Где жить — крейт `shamir-collections`

Концептуально подходит: там уже `THasher`/`TMap`/`TSet`/`Fx`-helpers — это
**workspace-уровневые типизированные коллекции**. Новый модуль
`shamir-collections/src/telemetry.rs`. Макросы (`tvec!`, `tmap!` etc.) —
экспортируются из корня `shamir-collections`. Внутренняя реализация —
`#[cfg(feature = "capacity-telemetry")]`-блок.

### 1.6 Обёрнутые типы (MVP)

| Макрос | В off-feature → | В on-feature → |
|---|---|---|
| `tvec!("n", c)` | `Vec::with_capacity(c)` | `TrackedVec::with_capacity_named(c, "n")` |
| `tstring!("n", c)` | `String::with_capacity(c)` | `TrackedString::with_capacity_named(c, "n")` |
| `tfxmap!("n", c)` | `FxHashMap::with_capacity_and_hasher(c, FxHasher::default())` | `TrackedFxHashMap::with_capacity_named(c, "n")` |
| `tmap!("n", c)` | `TMap::with_capacity(c)` | `TrackedTMap::with_capacity_named(c, "n")` |
| `tset!("n", c)` | `TSet::with_capacity(c)` | `TrackedTSet::with_capacity_named(c, "n")` |
| `tbtreeset!("n")` | `BTreeSet::new()` | `TrackedBTreeSet::new_named("n")` |
| `tbtreemap!("n")` | `BTreeMap::new()` | `TrackedBTreeMap::new_named("n")` |
| `tvecdeque!("n", c)` | `VecDeque::with_capacity(c)` | `TrackedVecDeque::with_capacity_named(c, "n")` |
| `tbytes!("n", c)` | `BytesMut::with_capacity(c)` | `TrackedBytesMut::with_capacity_named(c, "n")` |
| `tsccmap!("n", c)` | `scc::HashMap::with_capacity(c)` | `TrackedSccMap::with_capacity_named(c, "n")` |
| `tscctree!("n")` | `scc::TreeIndex::new()` | `TrackedSccTree::new_named("n")` |
| `tdashmap!("n", c)` | `DashMap::with_capacity(c)` | `TrackedDashMap::with_capacity_named(c, "n")` |

⚠ Для `BTreeSet`/`BTreeMap` `with_capacity` нет — у обёрток только `new_named`.
Peak меряется по `len` на Drop (для B-tree это approximation, но достаточная
для понимания «N узлов в среднем»).

⚠ Для `scc::*` и `DashMap` peak меряется по `len()` (но это `O(N)` для scc —
помечаем `#[allow(clippy::disallowed_methods)] // O(N) ack: telemetry only`).

### 1.7 Dump — явный, в конце бенча

```rust
pub fn dump_capacity_stats(path: impl AsRef<Path>) -> std::io::Result<()> { ... }
```

Формат — **JSON**, отсортирован по `peak_capacity` desc для удобства чтения:

```json
{
  "version": 1,
  "stats": [
    { "name": "engine/write_exec/staged_bytes", "peak_capacity": 1024,
      "creation_count": 50000 },
    { "name": "engine/write_helpers/record_ids", "peak_capacity": 768,
      "creation_count": 50000 },
    ...
  ]
}
```

**Path convention:** `target/capacity-stats/<bench_name>.json`. Создание
каталога — внутри `dump_capacity_stats`.

Auto-dump через global `Drop` — **нет**, ненадёжен при panic (порядок drop'а
static-ов недетерминирован). Явный вызов в конце bench `main()` или в
criterion `after_iter` — лучше.

### 1.8 Feature-gate

```toml
# shamir-collections/Cargo.toml
[features]
default = []
capacity-telemetry = ["dep:scc"]  # уже есть в workspace
```

Бенчи включают феатуру через `--features shamir-collections/capacity-telemetry`
ИЛИ через `[dev-dependencies] shamir-collections = { features = ["capacity-telemetry"] }`.
Прод и `--lib` тесты — без феатуры; zero overhead.

---

## 2. Workflow применения

1. **Реализовать** `shamir-collections::telemetry` (макросы + 12 обёрток с
   Deref + registry + JSON-dump). Покрыть unit-тестами с включённой феатурой:
   `tvec!()` создаёт TrackedVec, peak обновляется, dump пишет JSON.
2. **Получить target-list** — top-N hot allocators из flamegraph с символами
   (см. ниже §3). Первое применение — **точечное**, не обмазывать всё подряд.
3. **Заменить** `Vec::new()` / `Vec::with_capacity(0)` / `BTreeSet::new()` на
   соответствующие макросы в top-N местах. Имя — `<crate>/<module>/<role>`.
4. **Прогнать** целевые бенчи с `--features capacity-telemetry`, вызвать
   `dump_capacity_stats("target/capacity-stats/<bench>.json")` в конце main.
5. **Прочитать JSON** → выставить data-driven `with_capacity(peak)`
   (или `peak * 110 / 100` как safety margin против edge-case'ов).
6. **Re-bench** без feature → верифицировать ускорение (criterion compare).

---

## 3. Где сейчас «слепые» места — target-list для первого инструментирования

Из flamegraph `tx_pipeline` (`tx_overhead/batch_pipeline`, 15s × 16 configs,
26k samples, ~11% memory-bound):

| % self-time | Категория | Где (нужно подтвердить символами) |
|---|---|---|
| 4.48% | `memmove` | Vec realloc / msgpack write / Bytes copy |
| 3.53% | `memcmp` | Hash-eq на `Vec<u8>`/`&str` keys / BTreeMap cmp / msgpack markers |
| 6.7% | `malloc/free` family | Per-record новые Vec/String/Bytes |

Кандидаты в коде (`grep` write hot-path):
- `write_exec.rs:117` `new_base_keys: Vec<(String, u64)>` — per-batch без cap.
- `write_helpers.rs:379` `BTreeSet::<RecordId>::new()` — потенциальный главный
  источник `memcmp` (B-tree compare на каждом insert).
- `Bytes::copy_from_slice` на каждой строке (msgpack-encode output).
- `String::to_string()` в intern-cache (`write_exec.rs:148, 154`).

**Точная атрибуция — после flamegraph с символами** (фоновый прогон с
`CARGO_PROFILE_RELEASE_DEBUG=2 CARGO_PROFILE_RELEASE_STRIP=false`).

---

## 4. Открытые вопросы (на будущее)

- **Histogram p50/p95/p99 для `final_len`** — добавить в v2, если выставление
  `with_capacity(peak)` окажется over-provisioning'ом (peak << p95).
- **`realloc_count`** — добавить если интересна именно стоимость роста, а не
  итоговый peak. Требует hook на `push` (дороже Drop-only метрики).
- **`shrink_to_fit` detection** — единственный случай, когда capacity на Drop
  меньше реального peak. Если в коде станет важен — добавить on-push update.
- **Hierarchy / aggregation** — `engine/write_exec/*` показывать как
  одна группа. В v1 — flat JSON; в v2 — pre-aggregated tree-view.
- **Per-bench vs cumulative** — сейчас global registry агрегирует все бенчи в
  одной prog. Для per-bench нужен `reset_capacity_stats()` между бенчами.

---

## 5. Скоуп

S-M по объёму. ~300-500 строк (макросы + 12 wrappers с Deref + registry +
JSON-dump + unit-тесты). Изменения в `shamir-collections/Cargo.toml`
(новый feature), `shamir-collections/src/lib.rs` (re-export модуля и макросов),
новый `shamir-collections/src/telemetry.rs`. Не блокер; берётся **после**
первого targeted flamegraph-passа (даст target-list).

После реализации — отдельный коммит-серия: (1) инфраструктура; (2) первая
волна инструментации top-N hot allocators; (3) JSON-анализ + data-driven cap.
