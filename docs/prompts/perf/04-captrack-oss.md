בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-#295 — captrack OSS-generalize

> **Target:** создать самостоятельный OSS-крейт `captrack` в ОТДЕЛЬНОЙ папке
> **`D:\dev\rust\captrack`** (ВНЕ workspace shamir-db), который любой может взять
> и настроить под себя по трём ортогональным осям: трекинг on/off · хешер
> (дефолт/per-call/произвольный) · запрет (политика потребителя). Дизайн —
> `D:\dev\rust\shamir-db\docs\design\capacity-telemetry.md §5.5` (читай ОБЯЗАТЕЛЬНО).

## ⛔ Локация и инструменты (ВНИМАТЕЛЬНО — другая папка!)

- **Весь код создаётся в `D:\dev\rust\captrack\`** (= `/d/dev/rust/captrack/` в
  bash). Это НЕ shamir-db, отдельный greenfield проект.
- Это **НЕ часть workspace shamir-db** — НЕ добавляй в shamir-db Cargo.toml members.
- **Тесты — обычный `cargo test` / `cargo nextest run` прямо в `/d/dev/rust/captrack`**.
  Там НЕТ perimeter-guard'а shamir-db (он завязан на shamir-db/.cargo/config.toml).
  `./scripts/test.sh` там НЕ существует. Используй `cd /d/dev/rust/captrack && cargo ...`.
- **НЕ делай git-операций** (init/add/commit) в новой папке — оркестратор сам
  сделает `git init` + initial commit после твоей сдачи. Ты создаёшь ТОЛЬКО файлы.
- НЕ трогай shamir-db (ни workspace, ни crates/) — там ничего менять не нужно.
- НЕ sub-agent.

## Источник для копирования

`D:\dev\rust\shamir-db\crates\shamir-captrack\` (#288) — готовая ОСНОВА: 13
макросов, TrackedX обёртки, registry (scc), dump (JSON), тесты. **Прочитай его
целиком**, скопируй и обобщи в новую локацию `D:\dev\rust\captrack`. Не
переписывай с нуля — адаптируй под OSS (отличия ниже). Старую
`crates/shamir-captrack/` НЕ трогай — оркестратор уберёт её отдельно.

## Задача

### 0. Структура папки `D:\dev\rust\captrack\`

2-crate workspace, крейт в корне + proc-macro sub-member:
```
D:\dev\rust\captrack\
├── Cargo.toml               # [package] captrack + [workspace] members=[".","captrack-macros"]
├── src/
│   ├── lib.rs
│   ├── hasher.rs            # CapHasher feature-matrix
│   ├── registry.rs
│   ├── dump.rs
│   └── tracked/...
├── captrack-macros/
│   ├── Cargo.toml           # proc-macro = true
│   └── src/lib.rs           # declare_collections!
├── tests/                   # integration-тесты (или src/tests/)
├── README.md
├── CHANGELOG.md
├── LICENSE-MIT
├── LICENSE-APACHE
└── clippy.toml.example
```

Корневой `Cargo.toml` — и package, и workspace:
```toml
[package]
name = "captrack"
# ... (см. §1)

[workspace]
members = [".", "captrack-macros"]
```

### 1. Крейт `captrack` (корень `D:\dev\rust\captrack\Cargo.toml`)

`Cargo.toml` (package-секция):
```toml
[package]
name = "captrack"
version = "0.1.0"
edition = "2021"
description = "Capacity telemetry for Rust collections — call-site macros that record peak capacity, with zero overhead when disabled."
license = "MIT OR Apache-2.0"
repository = "..."   # placeholder
keywords = ["telemetry", "capacity", "profiling", "collections", "performance"]
categories = ["development-tools::profiling"]

[features]
default = []
telemetry  = ["dep:scc", "dep:serde", "dep:serde_json"]   # ось 1: трекинг
# ось 2A — дефолтный хешер (выбрать ≤1):
fxhash     = ["dep:fxhash"]
ahash      = ["dep:ahash"]
foldhash   = ["dep:foldhash"]
rustc-hash = ["dep:rustc-hash"]

[dependencies]
captrack-macros = { path = "../captrack-macros", version = "0.1.0" }
# Все опциональны:
scc        = { version = "2.2",    optional = true }
serde      = { version = "1",      optional = true, features = ["derive"] }
serde_json = { version = "1",      optional = true }
bytes      = { version = "1",      optional = true }   # tbytesmut!
indexmap   = { version = "2",      optional = true }   # tmap!/tset!
dashmap    = { version = "6",      optional = true }   # tdashmap!
fxhash     = { version = "0.2",    optional = true }
ahash      = { version = "0.8",    optional = true }
foldhash   = { version = "0.1",    optional = true }
rustc-hash = { version = "2",      optional = true }
```
⚠ Сверь точные версии по workspace (могут отличаться). `bytes`/`indexmap`/`dashmap`/
`scc` нужны и для off-feature раскрытия макросов (`tdashmap!` off → `::dashmap::...`),
так что они optional но привязаны к соответствующим макро-семьям. Реши: либо
gate каждую за под-фичей (`dashmap-support`), либо всегда-опционально-но-требуются
при использовании. ПРОЩЕ: сделать их частью `telemetry` И доступными для off через
dev-deps в тестах; реальные потребители (Фаза 2) добавят свои deps. Для самого
крейта: dev-dependencies для тестов off-feature.

### 2. `CapHasher` (ось 2A)

`src/hasher.rs`:
```rust
#[cfg(not(any(feature="fxhash",feature="ahash",feature="foldhash",feature="rustc-hash")))]
pub type CapHasher = std::collections::hash_map::RandomState;

#[cfg(feature = "fxhash")]
pub type CapHasher = fxhash::FxBuildHasher;

#[cfg(feature = "ahash")]
pub type CapHasher = ahash::RandomState;

#[cfg(feature = "foldhash")]
pub type CapHasher = foldhash::fast::RandomState;

#[cfg(feature = "rustc-hash")]
pub type CapHasher = rustc_hash::FxBuildHasher;

// Guard против >1 выбора:
#[cfg(any(
    all(feature="fxhash", feature="ahash"),
    all(feature="fxhash", feature="foldhash"),
    // ... все пары ...
))]
compile_error!("captrack: select at most one default-hasher feature (fxhash/ahash/foldhash/rustc-hash)");
```
⚠ ВАЖНО: дефолт (нет фич) = **RandomState**, не Fx. Это OSS DoS-safe дефолт.
`fxhash` больше НЕ always-on.

### 3. Макросы с `;`-override (ось 2B)

Каждый из 7 hash-макросов получает ВТОРОЙ арм. Пример `tmap!`:
```rust
// off-feature:
#[cfg(not(feature = "telemetry"))]
#[macro_export]
macro_rules! tmap {
    // дефолтный хешер
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        { #[allow(clippy::disallowed_methods)]
          ::indexmap::IndexMap::with_capacity_and_hasher(
              $cap, <$crate::CapHasher as ::core::default::Default>::default()) }
    }};
    // explicit hasher
    ($name:literal, $cap:expr; $hasher:expr) => {{
        let _: &'static str = $name;
        { #[allow(clippy::disallowed_methods)]
          ::indexmap::IndexMap::with_capacity_and_hasher($cap, $hasher) }
    }};
}

// on-feature:
#[cfg(feature = "telemetry")]
#[macro_export]
macro_rules! tmap {
    ($name:literal, $cap:expr) => {
        $crate::TrackedIndexMap::<_, _, $crate::CapHasher>::with_capacity_named($cap, $name)
    };
    ($name:literal, $cap:expr; $hasher:expr) => {
        $crate::TrackedIndexMap::with_capacity_and_hasher_named($cap, $hasher, $name)
    };
}
```
Не-hash макросы (`tvec!`/`tvecdeque!`/`tbtreemap!`/`tbtreeset!`/`tbytesmut!`/
`tscctree!`) — БЕЗ `;`-арма (нет хешера). `tvec!` как в #288.

### 4. `TrackedHashMap<K,V,S>` дженерик (ось 2B)

Сейчас в #288 есть `TrackedFxHashMap<K,V>` (Fx-хардкод) + `TrackedHashMap<K,V,S>`.
**Слей в один дженерик** `TrackedHashMap<K,V,S = CapHasher>`:
```rust
pub struct TrackedHashMap<K, V, S = crate::CapHasher> {
    inner: std::collections::HashMap<K, V, S>,
    name: &'static str,
}
impl<K, V, S: Default> TrackedHashMap<K, V, S> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self { inner: HashMap::with_capacity_and_hasher(cap, S::default()), name }
    }
}
impl<K, V, S> TrackedHashMap<K, V, S> {
    pub fn with_capacity_and_hasher_named(cap: usize, hasher: S, name: &'static str) -> Self {
        registry::record_creation(name);
        Self { inner: HashMap::with_capacity_and_hasher(cap, hasher), name }
    }
}
// Deref/DerefMut/Drop как в #288.
```
То же для `TrackedIndexMap<K,V,S>`, `TrackedHashSet<T,S>`, `TrackedIndexSet<T,S>`,
`TrackedDashMap<K,V,S>`, `TrackedSccHashMap<K,V,S>`, `TrackedSccHashSet<T,S>`.
Удали `TrackedFxHashMap` (заменён `TrackedHashMap<K,V,CapHasher>` через макрос).

### 5. Companion proc-macro `D:\dev\rust\captrack\captrack-macros\` (ось 2C)

`Cargo.toml`:
```toml
[package]
name = "captrack-macros"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
[lib]
proc-macro = true
[dependencies]
proc-macro2 = "1"
quote = "1"
syn = { version = "2", features = ["full"] }
```

`declare_collections! { hasher = MyHasher, prefix = my }` → эмитит делегирующие
обёртки. ВНУТРИ proc-macro генерируем токены (без dollar-escaping проблемы):
```rust
// псевдо-вывод для prefix=my, hasher=MyHasher:
#[macro_export]
macro_rules! my_map {
    ($name:literal, $cap:expr) => { $crate_or_captrack::tmap!($name, $cap; <MyHasher as ::core::default::Default>::default()) };
    ($name:literal, $cap:expr; $h:expr) => { $crate_or_captrack::tmap!($name, $cap; $h) };
}
// аналогично my_vec! my_set! my_fxset! my_dashmap! my_sccmap! ... (все 13)
```
⚠ Путь до captrack-макросов в выводе: используй `::captrack::tmap!` (абсолютный
путь — потребитель должен иметь captrack в deps). Документируй это требование.
⚠ Парсинг аргументов declare_collections — через syn: `hasher = <Type>, prefix =
<ident>`. Оба обязательны (или prefix default = "t"? — сделай prefix
ОБЯЗАТЕЛЬНЫМ чтобы не конфликтовать с captrack::tvec! при импорте обоих).
Для не-hash макросов (my_vec! и т.д.) хешер не нужен — просто делегируй
`::captrack::tvec!($name, $cap)`.

captrack `lib.rs`: `pub use captrack_macros::declare_collections;`.

### 6. clippy.toml.example + README (ось 3)

`D:\dev\rust\captrack\clippy.toml.example` — полный список disallowed-methods (см.
дизайн-док §3.1 / реальный список голых конструкторов). Каждый с
`reason = "use captrack::tX!(...)"`.

`D:\dev\rust\captrack\README.md` — секции:
- What it does (peak capacity tracking, zero-overhead off).
- Quick start (`use captrack::tvec; let v = tvec!("name", 16);`).
- Ось 1: enable telemetry (`features=["telemetry"]`) + dump.
- Ось 2: hasher — default RandomState; `features=["ahash"]` for default replace;
  `tmap!("n",c; h)` per-call; `declare_collections!` for custom default.
- Ось 3: enforcing via clippy (скопировать clippy.toml.example целиком/частично).
- License.

`crates/captrack/CHANGELOG.md` — `## 0.1.0 — Initial release`.

### 7. Тесты

В captrack/src/tests/ (по правилам организации):
- off_feature (default=RandomState): макросы → голые типы.
- off_feature с `--features fxhash`: CapHasher == FxBuildHasher (проверить тип
  через `TypeId` или просто что HashMap собирается с Fx).
- on_feature (`--features telemetry`): peak записывается, dump JSON валиден.
- per_call_override: `tmap!("n", 8; CustomBuildHasher)` использует CustomBuildHasher
  (можно проверить через counter в кастомном Hasher).
- declare_collections: в тесте `captrack::declare_collections!{hasher=Fx, prefix=q}`,
  затем `q_map!("n", 4)` работает + (on-feature) телеметрия пишется.
- compile_error guard: (не тестируется автоматически легко — оставь doc-коммент).

### 8. Гейт (прогони сам, приложи ПОЛНЫЙ вывод) — ОБЫЧНЫЙ cargo в новой папке

```
cd /d/dev/rust/captrack
cargo test                                       # off-feature default (RandomState)
cargo test --features telemetry
cargo test --features fxhash
cargo test --features telemetry,ahash
cargo test -p captrack-macros
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --all-targets --features telemetry -- -D warnings
cargo fmt --all -- --check
```
⚠ Обычный `cargo test` — НЕ `./scripts/test.sh` (его там нет; perimeter-guard
shamir-db не действует в `/d/dev/rust/captrack`). Если `cargo nextest` установлен —
можешь использовать `cargo nextest run` для красоты, но обычный `cargo test`
достаточен (новый крейт без deadlock-рисков).

Всё зелёное.

## Финальный отчёт

Новые файлы (все пути в `D:\dev\rust\captrack\`); публичный API (CapHasher, 13
макросов с `;`-арм, declare_collections!, Tracked*<K,V,S>); пример всех 3 уровней
хешера (off-default RandomState / features=ahash / per-call `;` / declare_collections);
вывод всего гейта (test-комбо + clippy + fmt). НЕ делал git — оркестратор
сделает `git init` + commit в новой папке.
