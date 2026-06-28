בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Capacity-telemetry — отдельный крейт `shamir-captrack` + clippy-ban на голые конструкторы

> **Статус:** дизайн финализирован после развилок с пользователем (фаза 1 —
> инфраструктура крейта, фаза 2 — clippy-ban + миграция, фаза 3 — анализ).
> Таски — `#288` (фаза 1), `#293` (фаза 2), `#294` (фаза 3). Источник — первый
> targeted flamegraph на `tx_pipeline` (см. `docs/research/WRITE-HOT-PATH-PROFILE-2026-06-28.md`).

---

## 0. Зачем

Стандартный паттерн при оптимизации Rust-кода — расставлять `Vec::with_capacity(N)`
там, где известен размер up-front. Проблема: для **многих** мест размер «заведомо
неизвестен» — зависит от selectivity фильтра, кардинальности данных, размера
батча. Без данных приходится **угадывать magic-constants** (`Vec::with_capacity(16)` —
почему 16?) или оставлять `Vec::new()` и платить за серию `realloc()`-ов.

**Идея:** инструментировать коллекции тонкой обёрткой с **именем** и счётчиком
peak capacity. Прогоняем под `--features capacity-telemetry` → JSON с реальными
peak'ами per-call-site → выставляем `with_capacity(peak)` data-driven.

**Дисциплина** (новое требование): clippy-ban на голые конструкторы всех
коллекций кроме `String` → разработчик **обязан** на каждом call-site
использовать макрос (тогда и size-hint обязателен, и call-site может быть
заmеchen). Невозможно случайно «оставить `Vec::new()` без cap».

---

## 1. Финализированные решения (после развилок с пользователем)

| # | Решение | Обоснование |
|---|---|---|
| **1** | **Отдельный крейт `shamir-captrack`** | Не в `shamir-collections`. Изолирует proc-macro/registry/dump, видимость workspace-wide через workspace dep. |
| **2** | **Call-site макросы `tvec!`/`tmap!`/...** | Без proc-macro, без derive — простой `macro_rules!`. Точечное обрамление, ноль hygiene-проблем. (Derive `#[derive(CapTrack)]` — possible v2, не сейчас.) |
| **3** | **В off-feature макрос раскрывается в голый конструктор** | Не type alias `TVec<T>`, а буквально `Vec::with_capacity($cap)`. **Ноль прослоек в проде.** Zero overhead, нет ABI-сдвига. |
| **4** | **Capacity обязателен в макросе** | `tvec!("name", cap)` — единственная форма. Нет `tvec!("name")` без cap. Это заставляет думать о размере на call-site. Для BTreeSet/BTreeMap (нет `with_capacity`) — capacity всё равно принимается (используется в on-feature для compare с peak), а в off-feature раскрывается в `BTreeSet::new()` (игнорируется). |
| **5** | **Только peak_capacity + creation_count** | MVP минимум. Не histogram, не realloc_count. Прямой ответ на «какой `with_capacity` ставить». |
| **6** | **clippy-ban через `disallowed-methods`** | Workspace-wide. Запрещены ВСЕ голые конструкторы коллекций кроме `String`. По семьям типов разбит на фазы (#293). |
| **7** | **String — исключение** | По требованию пользователя. String чаще всего создаётся для `format!`/error messages → capacity-планирование редко нужно. |
| **8** | **Феatures-gate `capacity-telemetry` (по default off)** | Бенчи включают; обычные тесты/прод — off. |
| **9** | **Dump — явный вызов в конце bench main** | `dump_capacity_stats("target/capacity-stats/<bench>.json")`. Auto-Drop globals ненадёжен. |
| **10** | **Все hash-макросы используют FxHasher по умолчанию** | Workspace ideology (CLAUDE.md §4 «Fx hash»): THasher = BuildHasherDefault<FxHasher>. Стандартный `RandomState` в 2-5× дороже. Все макросы `tmap!`/`tfxmap!`/`tset!`/`tdashmap!`/`tsccmap!`/etc. раскрываются в `with_capacity_and_hasher(c, ShamirHasher::default())`, не голый `with_capacity`. |
| **11** | **`#[allow(clippy::disallowed_methods)]` внутри раскрытия макроса** | Clippy анализирует **expanded** код. Когда Фаза 2 #293 включит `disallowed-methods` ban на `Vec::with_capacity`, наш макрос (раскрывающийся в `Vec::with_capacity`) **сам** триггернёт lint на каждом call-site. Решение — оборачивать каждое раскрытие в `{ #[allow(clippy::disallowed_methods)] expr }`. Атрибут распространяется на statement внутри expansion → callers получают код БЕЗ warning'а. **Стандартный паттерн для коллизий-в-собственном-макросе.** |

---

## 2. Крейт `shamir-captrack`

### 2.1 Структура

```
crates/shamir-captrack/
├── Cargo.toml
└── src/
    ├── lib.rs              # re-exports + макросы (cfg-gated)
    ├── tracked/
    │   ├── mod.rs
    │   ├── vec.rs          # TrackedVec<T>
    │   ├── string.rs       # (опц., v2 — String не запрещён)
    │   ├── hashmap.rs      # TrackedHashMap<K,V,S>
    │   ├── fxhashmap.rs    # TrackedFxHashMap (через THasher)
    │   ├── hashset.rs
    │   ├── indexmap.rs     # TrackedIndexMap (= TMap)
    │   ├── btreeset.rs
    │   ├── btreemap.rs
    │   ├── vecdeque.rs
    │   ├── bytesmut.rs
    │   ├── dashmap.rs
    │   ├── scc_hashmap.rs
    │   └── scc_treeindex.rs
    ├── registry.rs         # глобальный реестр CapStats
    └── dump.rs             # JSON-dump в target/capacity-stats/
```

### 2.2 Cargo.toml

```toml
[package]
name    = "shamir-captrack"
version = "0.1.0"
edition = "2021"

[features]
default = []
# Включает Tracked-обёртки и регистрацию. Без неё макросы раскрываются
# в голые ::std::vec::Vec::with_capacity (zero overhead).
capacity-telemetry = ["dep:scc", "dep:serde", "dep:serde_json"]

[dependencies]
scc        = { version = "...", optional = true }
serde      = { version = "...", optional = true, features = ["derive"] }
serde_json = { version = "...", optional = true }
# В off-feature dependencies нет — крейт буквально пустой.
```

### 2.3 Tracked-типы (пример Vec)

```rust
#[cfg(feature = "capacity-telemetry")]
pub struct TrackedVec<T> {
    inner: Vec<T>,
    name: &'static str,
}

#[cfg(feature = "capacity-telemetry")]
impl<T> TrackedVec<T> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        crate::registry::record_creation(name);
        Self { inner: Vec::with_capacity(cap), name }
    }
}

#[cfg(feature = "capacity-telemetry")]
impl<T> std::ops::Deref for TrackedVec<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> { &self.inner }
}

#[cfg(feature = "capacity-telemetry")]
impl<T> std::ops::DerefMut for TrackedVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> { &mut self.inner }
}

#[cfg(feature = "capacity-telemetry")]
impl<T> Drop for TrackedVec<T> {
    fn drop(&mut self) {
        crate::registry::record_peak(self.name, self.inner.capacity());
    }
}
```

`Deref<Target=Vec<T>>` + `DerefMut` → весь `Vec`-API работает прозрачно
(push/pop/iter/sort/...). Возврат макроса разный в on/off (Vec vs TrackedVec),
type-inference + Deref-coercion это поглощают.

⚠ **Type-inference caveat:** если в коде есть **явная аннотация** `let v:
Vec<T> = tvec!(...)`, в on-feature это поломается (тип будет `TrackedVec`).
Конвенция: использовать **inferred type** на call-site макроса. Если аннотация
нужна — снять, или использовать `let v: TrackedVec<T>` под cfg.

### 2.4 Макросы — раскрытие cfg-gated

```rust
// ── tvec! ─────────────────────────────────────────────────────
#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {
        $crate::TrackedVec::with_capacity_named($cap, $name)
    };
}

#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {
        ::std::vec::Vec::with_capacity($cap)
    };
}
```

В off — раскрывается **буквально в `Vec::with_capacity($cap)`**, без какого
бы то ни было wrapper'а или type alias. Совершенно прозрачно для оптимизатора.

Аналогично для всех типов. Для `BTreeSet`/`BTreeMap` (нет `with_capacity`):
- Off → `::std::collections::BTreeSet::new()` (cap игнорируется).
- On → `TrackedBTreeSet::new_named($name)` (но cap-hint всё равно принимается
  для future-API совместимости; используется в peak-сравнении).

### 2.5 Registry

```rust
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::sync::OnceLock;
use scc::HashMap;

pub struct CapStats {
    pub peak_capacity:  AtomicUsize,
    pub creation_count: AtomicU64,
}

impl CapStats {
    fn new() -> Self {
        Self {
            peak_capacity:  AtomicUsize::new(0),
            creation_count: AtomicU64::new(0),
        }
    }
}

static REGISTRY: OnceLock<HashMap<&'static str, CapStats>> = OnceLock::new();

fn registry() -> &'static HashMap<&'static str, CapStats> {
    REGISTRY.get_or_init(HashMap::new)
}

pub fn record_creation(name: &'static str) {
    let reg = registry();
    if let Some(entry) = reg.get(&name) {
        entry.creation_count.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = reg.insert(name, CapStats::new());
        if let Some(entry) = reg.get(&name) {
            entry.creation_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn record_peak(name: &'static str, cap: usize) {
    if let Some(entry) = registry().get(&name) {
        entry.peak_capacity.fetch_max(cap, Ordering::Relaxed);
    }
}
```

⚠ Использован `scc::HashMap` — lock-free, шардированный, доступ через
`get`/`insert` без полного lock'а. Имена `&'static str` гарантируют, что
ключи живут до конца программы.

### 2.6 Dump

```rust
use std::path::Path;
use serde::Serialize;

#[derive(Serialize)]
struct CapStatsEntry {
    name:           &'static str,
    peak_capacity:  usize,
    creation_count: u64,
}

#[derive(Serialize)]
struct CapStatsDump {
    version: u32,
    stats:   Vec<CapStatsEntry>,
}

pub fn dump_capacity_stats(path: impl AsRef<Path>) -> std::io::Result<()> {
    let mut entries: Vec<CapStatsEntry> = Vec::new();
    registry().scan(|name, stats| {
        entries.push(CapStatsEntry {
            name: *name,
            peak_capacity:  stats.peak_capacity.load(Ordering::Relaxed),
            creation_count: stats.creation_count.load(Ordering::Relaxed),
        });
    });
    entries.sort_by_key(|e| std::cmp::Reverse(e.peak_capacity));

    let dump = CapStatsDump { version: 1, stats: entries };
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(f, &dump)?;
    Ok(())
}
```

⚠ Off-feature: `dump_capacity_stats` либо `#[cfg(feature = ...)]`-only, либо
no-op fn always available. **Делаем no-op** — bench-код может звать
безусловно, не оборачивая в cfg.

---

## 3. Clippy ban (Фаза 2 — отдельная таска #293)

### 3.1 Расширение `clippy.toml` workspace

```toml
disallowed-methods = [
    # Existing — scc::*::len() is O(N).
    { path = "scc::HashMap::len",     reason = "scc::*::len() is O(N) — use AtomicUsize mirror" },
    { path = "scc::HashSet::len",     reason = "scc::*::len() is O(N) — use AtomicUsize mirror" },
    { path = "scc::TreeIndex::len",   reason = "scc::*::len() is O(N) — use AtomicUsize mirror" },
    { path = "dashmap::DashMap::len", reason = "dashmap::DashMap::len() is O(N)" },

    # NEW (capacity-telemetry): голые конструкторы коллекций запрещены —
    # вместо них макросы из shamir-captrack ({tvec!,tmap!,...}). String —
    # исключение (не требует capacity-планирования).

    # Vec
    { path = "std::vec::Vec::new",           reason = "use shamir_captrack::tvec!(\"name\", cap)" },
    { path = "std::vec::Vec::with_capacity", reason = "use shamir_captrack::tvec!(\"name\", cap)" },

    # HashMap (incl. via FxHashMap aliases)
    { path = "std::collections::HashMap::new",                       reason = "use tfxmap! or tmap!" },
    { path = "std::collections::HashMap::with_capacity",             reason = "use tfxmap! or tmap!" },
    { path = "std::collections::HashMap::with_hasher",               reason = "use tfxmap!" },
    { path = "std::collections::HashMap::with_capacity_and_hasher",  reason = "use tfxmap!" },

    # HashSet
    { path = "std::collections::HashSet::new",                       reason = "use tfxset! or tset!" },
    { path = "std::collections::HashSet::with_capacity",             reason = "use tfxset! or tset!" },
    { path = "std::collections::HashSet::with_hasher",               reason = "use tfxset!" },
    { path = "std::collections::HashSet::with_capacity_and_hasher",  reason = "use tfxset!" },

    # BTreeMap / BTreeSet
    { path = "std::collections::BTreeMap::new", reason = "use tbtreemap!(\"name\", cap_hint)" },
    { path = "std::collections::BTreeSet::new", reason = "use tbtreeset!(\"name\", cap_hint)" },

    # VecDeque
    { path = "std::collections::VecDeque::new",            reason = "use tvecdeque!(\"name\", cap)" },
    { path = "std::collections::VecDeque::with_capacity",  reason = "use tvecdeque!(\"name\", cap)" },

    # bytes
    { path = "bytes::Bytes::new",            reason = "use tbytes!(\"name\", cap)" },
    { path = "bytes::BytesMut::new",         reason = "use tbytesmut!(\"name\", cap)" },
    { path = "bytes::BytesMut::with_capacity", reason = "use tbytesmut!(\"name\", cap)" },

    # indexmap (через TMap/TSet)
    { path = "indexmap::IndexMap::new",            reason = "use tmap!(\"name\", cap)" },
    { path = "indexmap::IndexMap::with_capacity",  reason = "use tmap!(\"name\", cap)" },
    { path = "indexmap::IndexSet::new",            reason = "use tset!(\"name\", cap)" },
    { path = "indexmap::IndexSet::with_capacity",  reason = "use tset!(\"name\", cap)" },

    # dashmap
    { path = "dashmap::DashMap::new",            reason = "use tdashmap!(\"name\", cap)" },
    { path = "dashmap::DashMap::with_capacity",  reason = "use tdashmap!(\"name\", cap)" },
    { path = "dashmap::DashMap::with_hasher",    reason = "use tdashmap!(\"name\", cap)" },
    { path = "dashmap::DashMap::with_capacity_and_hasher", reason = "use tdashmap!(\"name\", cap)" },

    # scc
    { path = "scc::HashMap::new",            reason = "use tsccmap!(\"name\", cap)" },
    { path = "scc::HashMap::with_capacity",  reason = "use tsccmap!(\"name\", cap)" },
    { path = "scc::HashSet::new",            reason = "use tsccset!(\"name\", cap)" },
    { path = "scc::TreeIndex::new",          reason = "use tscctree!(\"name\", cap_hint)" },

    # String / &str — НЕ запрещены (пользовательское исключение).
]
```

### 3.2 Стратегия миграции (#293)

**Per-семья**: Vec → HashMap → BTreeSet/BTreeMap → VecDeque → bytes → indexmap →
dashmap → scc. Каждая семья — **отдельный коммит**, гейт зелёный после каждого.

**Исключения через `#[allow(clippy::disallowed_methods)]`**:
- Тесты/фикстуры (capacity не релевантен → `#![cfg_attr(test, allow(...))]`).
- Бенчи (мерят сами hot-path точно).
- Сторонний код (impl блоки на std-типах не приходят к нам, но cargo может).

⚠ Альтернатива: workspace `[workspace.lints]` (Cargo 1.74+) — позволяет
точечно разрешать в подкрейтах. Использовать.

---

## 4. Workflow применения (Фаза 3 — таска #294)

1. Прогон бенча с `--features shamir-captrack/capacity-telemetry`.
2. В конце bench main: `shamir_captrack::dump_capacity_stats("target/capacity-stats/<bench>.json")`.
3. Прочитать JSON, отсортировать по `peak_capacity` desc.
4. Data-driven `with_capacity(peak)` (или `peak * 110 / 100` margin) — обновить
   call-sites макросов `tvec!("name", PEAK_VALUE)` вместо предыдущих guess'ов.
5. Re-bench без feature → criterion compare → верифицировать ускорение.

---

## 5. Скоуп (по фазам)

| Фаза | Таска | Скоуп | Risk |
|---|---|---|---|
| **1** | #288 | Крейт + макросы + Tracked-обёртки + registry + JSON dump + unit-тесты | S — изолирован |
| **2** | #293 | `clippy.toml` ban + workspace-wide миграция call-sites (per-семья) | **L** — сотни мест |
| **3** | #294 | Bench instrumentation + JSON analysis + data-driven `with_capacity` | M |

**Сводно:** ~500-800 строк инфраструктуры (#288) + per-семья патчи (~50-200
сайтов каждая) (#293) + узкий data-patch (#294).

---

## 6. Открытые вопросы (на будущее)

- **Histogram `final_len` (p50/p95/p99)** — добавить в v2, если peak окажется
  over-provisioning'ом (peak << p95).
- **`realloc_count`** — добавить, если интересна стоимость роста vs peak.
  Требует hook на `push` (дороже Drop-only).
- **`shrink_to_fit` detection** — единственный случай, когда capacity на Drop
  меньше peak. Если важно — добавить on-push update.
- **Hierarchy / aggregation** — `engine/write_exec/*` показывать как группу.
  В v1 — flat JSON; в v2 — pre-aggregated tree-view.
- **Per-bench vs cumulative** — global registry агрегирует все бенчи в одной
  prog. Для per-bench — `reset_capacity_stats()` между бенчами.
- **Derive `#[derive(CapTrack)]`** — proc-macro, оборачивающий все
  collection-поля struct. Гипотетический v2, если call-site обрамление
  окажется недостаточным.
