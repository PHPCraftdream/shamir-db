בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-③.288 — крейт `shamir-captrack` (Фаза 1)

> **Target:** реализовать инфраструктурный крейт для capacity-telemetry.
> Только сам крейт — никаких изменений в shamir-engine/index/etc. Это
> фундамент для Фазы 2 (#293) и Фазы 3 (#294).

## ⛔ Запреты
- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор. НЕ удаляй отслеживаемые. НЕ sub-agent.
- Тесты — ТОЛЬКО `./scripts/test.sh` (raw `cargo test` заблокирован perimeter-guard).
- НЕ трогай workspace `clippy.toml` (это Фаза 2 #293).
- НЕ трогай shamir-engine/index/storage/etc. (это Фаза 2 #293).
- НЕ добавляй workspace-уровневые dep'ы на shamir-captrack в другие крейты
  (это Фаза 2 — пока крейт стоит «в стороне»).

## Прочитанная реальность

Дизайн зафиксирован полностью: `docs/design/capacity-telemetry.md` (читай целиком
ДО кода — там §1 финализированные решения, §2 структура крейта, §2.3 пример
TrackedVec, §2.4 макросы cfg-gated, §2.5 registry, §2.6 dump).

Главные ключевые решения (не пере-решай):
1. **Отдельный крейт `shamir-captrack`** (не модуль в shamir-collections).
2. **Call-site макросы** через `macro_rules!` (НЕ proc-macro, НЕ derive).
3. **В off-feature макрос раскрывается в ГОЛЫЙ конструктор**: `tvec!("name", 5)`
   → буквально `::std::vec::Vec::with_capacity(5)`. НЕ type alias, НЕ wrapper.
4. **Capacity ОБЯЗАТЕЛЕН в макросе**: `tvec!("name", cap)` — единственная форма.
5. **Метрика — ТОЛЬКО peak_capacity (AtomicUsize, fetch_max в Drop) +
   creation_count (AtomicU64, fetch_add в ctor)**. НИЧЕГО больше.
6. **String — БЕЗ обёртки** (пользовательское исключение; макрос `tstring!`
   НЕ создаём в MVP — просто пользоваться `String::with_capacity`).
7. **dump_capacity_stats — всегда доступная функция**, в off-feature no-op.

## Задача

### 1. Создать новый крейт `crates/shamir-captrack/`

Зарегистрировать в workspace `Cargo.toml` `[workspace] members = [...]`.
Не добавлять в `[workspace.dependencies]` ещё (это Фаза 2).

### 2. `Cargo.toml`

```toml
[package]
name = "shamir-captrack"
version = "0.1.0"
edition = "2021"
license = "..."  # как у соседних крейтов

[features]
default = []
capacity-telemetry = ["dep:scc", "dep:serde", "dep:serde_json"]

[dependencies]
scc        = { workspace = true, optional = true }
serde      = { workspace = true, optional = true, features = ["derive"] }
serde_json = { workspace = true, optional = true }
bytes      = { workspace = true, optional = true }  # для TrackedBytesMut
indexmap   = { workspace = true, optional = true }  # для TrackedIndexMap
dashmap    = { workspace = true, optional = true }  # для TrackedDashMap
fxhash     = { workspace = true, optional = true }  # для FxHasher в Tracked maps

# В default off — крейт буквально пустой, ноль deps.
```

Сверь имена workspace deps с реальным `Cargo.toml` workspace (могут быть с
другими префиксами). Если необходимая dep отсутствует — оставь TODO в брифе,
не добавляй workspace-dep самовольно.

### 3. Структура

```
src/
  lib.rs              # re-exports + макросы
  registry.rs         # глобальный реестр (cfg-gated)
  dump.rs             # dump_capacity_stats (cfg-gated body + no-op stub)
  tracked/
    mod.rs            # re-exports всех Tracked*
    vec.rs            # TrackedVec<T>
    hashmap.rs        # TrackedHashMap<K,V,S>
    fxhashmap.rs      # TrackedFxHashMap<K,V>
    hashset.rs        # TrackedHashSet<T,S>
    indexmap.rs       # TrackedIndexMap<K,V,S> (для tmap!)
    indexset.rs       # TrackedIndexSet<T,S>
    btreemap.rs       # TrackedBTreeMap<K,V>
    btreeset.rs       # TrackedBTreeSet<T>
    vecdeque.rs       # TrackedVecDeque<T>
    bytesmut.rs       # TrackedBytesMut
    dashmap.rs        # TrackedDashMap<K,V,S>
    scc_hashmap.rs    # TrackedSccHashMap<K,V,S>
    scc_hashset.rs    # TrackedSccHashSet<T,S>
    scc_treeindex.rs  # TrackedSccTreeIndex<K,V>
```

Правило организации: **один файл — один primary export** (см. CLAUDE.md).

### 4. Tracked<T> wrapper template (пример Vec)

```rust
// src/tracked/vec.rs
#[cfg(feature = "capacity-telemetry")]
use crate::registry;

#[cfg(feature = "capacity-telemetry")]
pub struct TrackedVec<T> {
    inner: Vec<T>,
    name: &'static str,
}

#[cfg(feature = "capacity-telemetry")]
impl<T> TrackedVec<T> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: Vec::with_capacity(cap),
            name,
        }
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
        registry::record_peak(self.name, self.inner.capacity());
    }
}

// IntoIterator чтобы можно было `for x in tvec` напрямую
#[cfg(feature = "capacity-telemetry")]
impl<T> IntoIterator for TrackedVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(mut self) -> Self::IntoIter {
        // Сделать peak-record перед into_iter (потому что инструкция
        // сбрасывает inner). Но IntoIter поглощает self → drop вызовется
        // после. Проверь поведение: если Drop вызывается → ОК; если нет
        // (move из inner) → запиши peak здесь явно перед std::mem::take.
        // НАЙДИ ПРАВИЛЬНЫЙ ПУТЬ: вероятно std::mem::take + явный record + manual return iter.
        registry::record_peak(self.name, self.inner.capacity());
        let inner = std::mem::take(&mut self.inner);
        // Manual drop без записи второго peak (mem::take оставил Vec::new()):
        std::mem::forget(self);
        inner.into_iter()
    }
}
```

⚠ `IntoIterator` — тонкость. Если consumed через `into_iter`, нужно записать
peak до того, как inner будет moved. Подумай аккуратно — лучше тестом.

То же для `Tracked*<T,V>` — Deref/DerefMut на inner, record_creation в
конструкторе, record_peak в Drop. Для `BTreeMap`/`BTreeSet` (нет `with_capacity`):
`new_named(cap_hint: usize, name: &'static str)` принимает cap_hint но игнорирует
для inner; peak меряется по `inner.len()` (не capacity, у B-tree её нет) в Drop.
Для `scc::*` и `DashMap`: то же — peak по `len()` (allow disallowed_methods
inline с обоснованием «telemetry only»). Для `BytesMut`: peak = `inner.capacity()`.

### 5. Макросы (`lib.rs`)

#### 5.1 ⚠ КРИТИЧНО: clippy-collision внутри собственного макроса

Clippy анализирует **expanded** код. Когда workspace включит Фазу 2 #293 с
`disallowed-methods` на `Vec::with_capacity` (etc.), наш макрос, раскрывающийся
в `Vec::with_capacity`, **сам триггернёт lint** на каждом call-site. Это
заблокирует всю работу.

**Решение:** оборачивать каждое раскрытие в `{ #[allow(clippy::disallowed_methods)]
expr }`. `#[allow]` распространяется на statement внутри macro expansion →
callers получают код БЕЗ warning'а. Стандартный паттерн.

#### 5.2 ⚠ КРИТИЧНО: быстрый хешер (FxHasher / THasher) по умолчанию

Workspace ideology (CLAUDE.md §4 «Fx hash»): все hash-коллекции по дефолту
используют `THasher = BuildHasherDefault<FxHasher>`. Стандартный `HashMap::new()`
с `RandomState` дороже на 2-5×. Поэтому ВСЕ наши hash-макросы раскрываются
в `with_capacity_and_hasher(cap, THasher::default())`, не голый `with_capacity`.

Источник THasher: `shamir-collections::THasher` (или эквивалентный путь —
verify). Если нельзя зависеть от shamir-collections (циклическая dep?), —
создай локально `pub type ShamirHasher = std::hash::BuildHasherDefault<fxhash::FxHasher>`
в `lib.rs` shamir-captrack и юзай его в раскрытии. Используй ту же дефолтную
ideology по workspace.

#### 5.3 Шаблон раскрытия

```rust
// ── tvec! (нет хеширования) ────────────────────────────────────
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {{
        // _name прибит для consistency; в off — игнорируется компилятором.
        let _: &'static str = $name;
        #[allow(clippy::disallowed_methods)]
        let __v = ::std::vec::Vec::with_capacity($cap);
        __v
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {
        $crate::TrackedVec::with_capacity_named($cap, $name)
    };
}

// ── tfxmap! (HashMap с FxHasher по умолчанию) ──────────────────
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tfxmap {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        #[allow(clippy::disallowed_methods)]
        let __m = ::std::collections::HashMap::with_capacity_and_hasher(
            $cap,
            $crate::ShamirHasher::default(),
        );
        __m
    }};
}

#[cfg(feature = "capacity-telemetry")]
#[macro_export]
macro_rules! tfxmap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedFxHashMap::with_capacity_named($cap, $name)
    };
}

// ── tdashmap! (DashMap с FxHasher по умолчанию) ────────────────
#[cfg(not(feature = "capacity-telemetry"))]
#[macro_export]
macro_rules! tdashmap {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        #[allow(clippy::disallowed_methods)]
        let __d = ::dashmap::DashMap::with_capacity_and_hasher(
            $cap,
            $crate::ShamirHasher::default(),
        );
        __d
    }};
}
// ...и т.д. для каждого hash-типа.
```

#### 5.4 Полный набор макросов (13 штук)

| Макрос | Off-feature → (`#[allow]` обёрнутые) | Хешер |
|---|---|---|
| `tvec!("n", c)` | `Vec::with_capacity(c)` | — |
| `tvecdeque!("n", c)` | `VecDeque::with_capacity(c)` | — |
| `tbtreemap!("n", _c)` | `BTreeMap::new()` (cap игнор) | — |
| `tbtreeset!("n", _c)` | `BTreeSet::new()` (cap игнор) | — |
| `tbytesmut!("n", c)` | `BytesMut::with_capacity(c)` | — |
| `tfxmap!("n", c)` | `HashMap::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tfxset!("n", c)` | `HashSet::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tmap!("n", c)` | `IndexMap::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tset!("n", c)` | `IndexSet::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tdashmap!("n", c)` | `DashMap::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tsccmap!("n", c)` | `scc::HashMap::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tsccset!("n", c)` | `scc::HashSet::with_capacity_and_hasher(c, ShamirHasher::default())` | Fx |
| `tscctree!("n", _c)` | `scc::TreeIndex::new()` (cap игнор) | — |

⚠ `ShamirHasher` либо re-export из shamir-collections (если нет circular dep),
либо локально определённый alias в shamir-captrack `lib.rs`:
```rust
pub type ShamirHasher = std::hash::BuildHasherDefault<fxhash::FxHasher>;
```

⚠ Каждое раскрытие — в `{ #[allow(clippy::disallowed_methods)] expr }`. Это
ключ к тому, чтобы Фаза 2 #293 (ban в clippy.toml) НЕ заблокировала наш
собственный макрос.

### 6. Registry (`registry.rs`)

Только при `capacity-telemetry`. См. §2.5 в дизайн-доке:

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
    if reg.get(&name).is_none() {
        let _ = reg.insert(name, CapStats::new());
    }
    if let Some(entry) = reg.get(&name) {
        entry.creation_count.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_peak(name: &'static str, cap: usize) {
    if let Some(entry) = registry().get(&name) {
        entry.peak_capacity.fetch_max(cap, Ordering::Relaxed);
    }
}
```

⚠ scc::HashMap::insert — `Result<_, _>` (выдаёт Err при дубликате). Использовать
`entry()` API для atomic-insert-or-get, или handle Err как «уже создано». См.
scc docs. Race: если два потока одновременно увидят `is_none() == true`, оба
попытаются insert — один Err'нется, второй Ok. Эта race безопасна.

### 7. Dump (`dump.rs`)

```rust
#[cfg(feature = "capacity-telemetry")]
mod inner {
    use crate::registry::CapStats;
    use serde::Serialize;
    use std::path::Path;
    use std::sync::atomic::Ordering;

    #[derive(Serialize)]
    struct Entry {
        name:           &'static str,
        peak_capacity:  usize,
        creation_count: u64,
    }
    #[derive(Serialize)]
    struct Dump { version: u32, stats: Vec<Entry> }

    pub fn dump_capacity_stats(path: impl AsRef<Path>) -> std::io::Result<()> {
        let mut entries: Vec<Entry> = Vec::new();
        crate::registry::registry().scan(|name, stats| {
            entries.push(Entry {
                name: *name,
                peak_capacity:  stats.peak_capacity.load(Ordering::Relaxed),
                creation_count: stats.creation_count.load(Ordering::Relaxed),
            });
        });
        entries.sort_by_key(|e| std::cmp::Reverse(e.peak_capacity));
        let dump = Dump { version: 1, stats: entries };
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(f, &dump)?;
        Ok(())
    }
}

#[cfg(feature = "capacity-telemetry")]
pub use inner::dump_capacity_stats;

#[cfg(not(feature = "capacity-telemetry"))]
pub fn dump_capacity_stats<P: AsRef<std::path::Path>>(_path: P) -> std::io::Result<()> {
    Ok(()) // no-op в off-feature
}
```

`registry::registry()` нужен `pub(crate)`. Сверь имя scan-метода у scc (`scan`
или `for_each`).

### 8. Re-exports в `lib.rs`

```rust
// макросы экспортируются через #[macro_export] на самих макросах.

// Tracked-типы (только в on-feature, но re-export safe в обоих режимах если
// вообще их нет в off):
#[cfg(feature = "capacity-telemetry")]
pub use tracked::{
    TrackedVec, TrackedHashMap, TrackedFxHashMap, TrackedHashSet,
    TrackedIndexMap, TrackedIndexSet, TrackedBTreeMap, TrackedBTreeSet,
    TrackedVecDeque, TrackedBytesMut, TrackedDashMap,
    TrackedSccHashMap, TrackedSccHashSet, TrackedSccTreeIndex,
};

pub use dump::dump_capacity_stats; // доступна в обоих режимах
```

### 9. Тесты

#### 9.1 Off-feature (default — `./scripts/test.sh -p shamir-captrack`)

```rust
#[test]
fn tvec_off_feature_is_plain_vec() {
    let v: Vec<u32> = tvec!("test/vec", 16);
    assert_eq!(v.capacity(), 16);
    // Тип — голый Vec<u32>, никакого wrapper.
    let _: Vec<u32> = v;
}
```

Проверка ГЛАВНОГО инварианта: в off-feature макрос даёт буквально `Vec`.

Аналогично для каждого макроса. Достаточно по одному кейсу на тип.

#### 9.2 On-feature — `cargo test -p shamir-captrack --features capacity-telemetry`

⚠ Здесь raw `cargo test` нужен для feature-gated теста — perimeter-guard
блокирует. **Проверь:** руководствуется ли `./scripts/test.sh` через
`-- --features capacity-telemetry`? Если да — использовать. Если нет, оставь
TODO и попроси оркестратора (вариант: добавь `[[test]]` test-bin с
`required-features = ["capacity-telemetry"]` — тогда `./scripts/test.sh
-p shamir-captrack --features capacity-telemetry` будет работать через
nextest по конфигу).

Тесты:
- `tvec_records_peak_on_drop` — создать TrackedVec, push несколько, drop, проверить registry peak == capacity.
- `tvec_records_creation_count` — создать 5 instances, проверить creation_count == 5.
- `peak_is_max_across_instances` — несколько instances с разной capacity, проверить peak == max.
- `concurrent_peak_record` — 10 threads × N records each, проверить peak >= max видимый поток.
- `dump_writes_valid_json` — записать stats, dump в tempfile, прочитать JSON,
  проверить структура { version: 1, stats: [...] } отсортирована desc.
- `deref_works_like_vec` — push/iter/len через Deref работают одинаково.

### 10. Гейт (прогони сам, приложи ПОЛНЫЙ вывод)

```
./scripts/test.sh -p shamir-captrack
cargo clippy -p shamir-captrack --all-targets -- -D warnings
cargo fmt -p shamir-captrack -- --check
```

Для on-feature тестов — реши по доступному API ./scripts/test.sh; если нет
вариантов — отметь TODO и оставь raw `cargo test --features capacity-telemetry`
для последующего гейта (оркестратор поможет).

## Финальный отчёт

Изменённые/новые файлы; сигнатуры публичного API (макросы + Tracked-типы +
dump); пример off-feature раскрытия `tvec!("test", 5)` → `Vec::with_capacity(5)`
(пастишь expanded macro если можешь); вывод гейта.
