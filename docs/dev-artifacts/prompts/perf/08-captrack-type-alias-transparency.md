בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-#302 (replacement) — captrack: type-alias-off-feature для полной прозрачности

> **Target:** добиться того, что исходный код consumer'а **идентичен** в обоих
> режимах `--features captrack/telemetry` ON/OFF. Никаких `.into_vec()` /
> `.into_untracked()` маркеров, которые видны в исходнике только из-за
> telemetry feature. Поведение различается только на уровне типов и Drop, не
> на уровне формы кода.

## ⛔ Запреты

- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор.
- Тесты shamir-engine — ТОЛЬКО `./scripts/test.sh` (raw `cargo test` заблокирован).
  Тесты captrack — обычный `cargo test`.
- НЕ менять `tvec!` (и аналоги) API: контракт `tvec!(name, cap)` сохраняется.
- НЕ трогать другие крейты в shamir-db (`shamir-index`, `shamir-storage` и т.д.),
  только `shamir-engine`. Скоуп — 4 файла из Партии 1 (#296).
- НЕ менять семантику бизнес-логики; только типы / type-aliases / boundary-преобразования.

## Прочитанная реальность — почему предыдущий фикс плох

Предыдущая итерация (commit-less, у тебя в working tree) добавила:
- `From<TrackedVec<T>> for Vec<T>` в `captrack/src/tracked/vec.rs`,
- публичный `pub trait IntoVec<T>` в `captrack/src/lib.rs` (+ 2 impl'а),
- 24 точки `.into_vec()` в 4 файлах shamir-engine.

Проблема: **24 маркера `.into_vec()` существуют в коде ТОЛЬКО потому что
включена feature `telemetry`**. Off-feature они no-op, но в исходнике они
есть. Это утечка feature-flag'а в форму кода → диагностический и
production билды визуально разные.

Корень — `tvec!` возвращает **разные типы** (`Vec<T>` vs `TrackedVec<T>`).
Call-site вынужден различать.

## Дизайн фикса — Path C

**1. `TrackedX` существует в ОБЕИХ ветках feature**:
- off-feature: `pub type TrackedX<T> = StdX<T>` (type alias).
- on-feature: `pub struct TrackedX<T> { inner: StdX<T>, name: &'static str }` (как сейчас).

**2. `tvec!` (и аналоги) — единая ветка макроса**, делегирующая в свободную
функцию `captrack::with_capacity_named_vec::<T>(cap, name)` (или аналог):
- off-feature impl этой функции: `Vec::with_capacity(cap)` (имя игнорируется).
- on-feature impl: `TrackedVec::<T>::with_capacity_named(cap, name)`.

В обоих случаях возвращается `TrackedVec<T>` (который — alias на Vec off-feature
и обёртка on-feature).

**3. Consumer-сайт**: `let mut v: TrackedVec<T> = tvec!(...)` (или дропни
аннотацию — inference сам выберет). **Identical в обоих режимах.**

**4. Boundary (когда наружу нужен голый `Vec<T>`)** — новый макрос
`captrack::untrack!(expr)`:
- off-feature: `($e:expr) => { $e }` — раскрывается в само выражение, без преобразования.
- on-feature: `($e:expr) => { ::std::convert::From::from($e) }` — тип цели берётся из binding, captrack уже имеет `From<TrackedX<T>> for StdX<T>`.

Применять `untrack!(v)` ТОЛЬКО там, где наружу нужен `Vec<T>` (return type,
struct-field, передача в bare-API). Внутри функции — оставляем `TrackedX<T>`,
работаем через Deref/DerefMut как с обычным Vec.

`untrack!()` НЕ генерирует `clippy::useless_conversion` off-feature
(потому что там это просто `$e`, не `.into()`).

## Задача — три шага

### Шаг 1. captrack: симметрия типов в обеих ветках

Файл `D:\dev\rust\captrack\src\lib.rs`:
- УДАЛИ полностью `pub trait IntoVec<T>` и оба его impl'а (вторая итерация
  фикса; больше не нужны).
- Если `pub mod tracked` сейчас под `#[cfg(feature = "telemetry")]` — раздели:
  ```rust
  #[cfg(feature = "telemetry")]
  pub use tracked::{TrackedVec, TrackedVecDeque, ...};

  #[cfg(not(feature = "telemetry"))]
  pub use aliases::*;
  ```
- Создай новый модуль `src/aliases.rs` (только off-feature) с type-alias'ами
  для **всех 13 типов** (симметрия с tracked-модулем):
  ```rust
  #![cfg(not(feature = "telemetry"))]
  pub type TrackedVec<T> = std::vec::Vec<T>;
  pub type TrackedVecDeque<T> = std::collections::VecDeque<T>;
  pub type TrackedHashMap<K, V, S = crate::CapHasher> = std::collections::HashMap<K, V, S>;
  pub type TrackedHashSet<T, S = crate::CapHasher> = std::collections::HashSet<T, S>;
  pub type TrackedBTreeMap<K, V> = std::collections::BTreeMap<K, V>;
  pub type TrackedBTreeSet<T> = std::collections::BTreeSet<T>;
  // и т.д. для индексной/dashmap/scc-семьи — ТОЛЬКО если они в текущем
  // tracked-модуле уже есть. Если каких-то нет (например TrackedBytesMut),
  // под cfg(off-feature) их тоже не добавляй.
  ```
  Зеркалируй ТОЧНО тот же набор имён и параметров, что в on-feature
  `tracked/*.rs`. Если on-feature имеет hasher-параметр со значением по
  умолчанию `CapHasher` — alias тоже должен.

### Шаг 2. captrack: единая ветка макросов через свободные функции

Для `tvec!` (и в той же логике для tvecdeque!, tbtreemap!, tbtreeset!,
tfxmap!, tfxset!, tmap!, tset!, tdashmap!, tsccmap!, tsccset!, tscctree!,
tbytesmut! — всё что есть):

В `src/lib.rs` (или в новом `src/ctor.rs`) добавь свободные cfg-branched
функции:
```rust
// off-feature
#[cfg(not(feature = "telemetry"))]
pub fn vec_with_capacity_named<T>(cap: usize, _name: &'static str) -> TrackedVec<T> {
    Vec::with_capacity(cap)
}

// on-feature
#[cfg(feature = "telemetry")]
pub fn vec_with_capacity_named<T>(cap: usize, name: &'static str) -> TrackedVec<T> {
    TrackedVec::with_capacity_named(cap, name)
}
```
Аналогично для всех 13 семейств (если в семействе уже была функция
с-capacity — оборачивай её; если не было — простая `with_capacity(cap)` /
`with_capacity_and_hasher(...)`).

Перепиши макросы в `src/lib.rs` в **единую ветку**:
```rust
#[macro_export]
macro_rules! tvec {
    ($name:literal, $cap:expr) => {{
        let _: &'static str = $name;
        #[allow(clippy::disallowed_methods)]
        { $crate::vec_with_capacity_named::<_>($cap, $name) }
    }};
}
```
Дублирование макро-веток по cfg БОЛЬШЕ НЕ НУЖНО. cfg-разделение целиком
ушло в свободные функции. `tvec!(...)` теперь возвращает `TrackedVec<T>` в
обоих режимах.

Аналогично для tvecdeque!, tbtreemap!, ... — единая ветка через свободную
функцию.

### Шаг 3. captrack: `untrack!` макрос для boundary

В `src/lib.rs` добавь:
```rust
#[cfg(not(feature = "telemetry"))]
#[macro_export]
macro_rules! untrack {
    ($e:expr) => { $e };
}

#[cfg(feature = "telemetry")]
#[macro_export]
macro_rules! untrack {
    ($e:expr) => { ::std::convert::From::from($e) };
}
```
Импортируй у consumer'а как `use captrack::untrack;`. Никакой `clippy::useless_conversion`
не сработает off-feature (`$e` это не `.into()`).

`From<TrackedVec<T>> for Vec<T>` (добавленный предыдущей итерацией в
`tracked/vec.rs`) — ОСТАВЬ. Он по-прежнему нужен для `untrack!()` в
on-feature режиме. Аналогичные `From<TrackedX<T>> for StdX<T>` impl'ы
добавь для остальных 12 типов В TRACKED-модуле on-feature (чтобы
`untrack!()` работал универсально). Каждый — по образцу `Vec`-а:
```rust
impl<T> From<TrackedX<T>> for StdX<T> {
    fn from(mut tx: TrackedX<T>) -> StdX<T> {
        crate::registry::record_peak(tx.name, /* capacity / len / what fits */);
        let inner = std::mem::take(&mut tx.inner);
        std::mem::forget(tx);
        inner
    }
}
```
Если у какого-то Tracked-типа нет `Drop` с peak-recording, или нет
`mem::take`-friendly Default — пропусти его и оставь TODO-комментарий в
файле (не нужно блокироваться).

### Шаг 4. shamir-engine: миграция 4 файлов под новый дизайн

Откати трейт-зависимость предыдущей итерации:
- `use captrack::{tvec, IntoVec};` → `use captrack::{tvec, untrack};`
- ВСЕ 24 `.into_vec()` → `untrack!(<expr>)`.
- Аннотации `let mut x: Vec<T> = tvec!(...)` → `let mut x: TrackedVec<T> = tvec!(...)`
  (импортировать тип: `use captrack::TrackedVec;`). Если до этого аннотации
  не было — оставь без аннотации.
- `Ok(Some(result.into_vec()))` → `Ok(Some(untrack!(result)))`.
- `tvec!("...", 0).into_vec()` (для пустых-return сайтов) → `untrack!(tvec!("...", 0))`.

Где `for x in &collection` ломается (это было фиксом предыдущей итерации:
`for qv in &resolved_values` → `resolved_values.iter()`), **верни обратно
к `for x in &collection`**: в on-feature теперь нужен `impl IntoIterator
for &TrackedX<T>` который делегирует в Deref — добавь его в captrack
on-feature (если ещё нет):
```rust
impl<'a, T> IntoIterator for &'a TrackedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter { self.inner.iter() }
}
```
Аналогично для `&mut TrackedVec<T>` если нужно. Off-feature это уже работает
автоматически (alias на Vec).

Шаги 1–3 — captrack. Шаг 4 — shamir-engine.

## Гейт (агент сам прогоняет, приложи ПОЛНЫЙ вывод обеих сборок)

**captrack:**
```
cd D:\dev\rust\captrack
cargo build
cargo build --features telemetry
cargo test
cargo test --features telemetry
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features telemetry -- -D warnings
cargo fmt -- --check
```

**shamir-engine:**
```
cd D:\dev\rust\shamir-db
cargo build -p shamir-engine --benches
cargo build -p shamir-engine --benches --features captrack/telemetry
cargo clippy -p shamir-engine --all-targets -- -D warnings
cargo clippy -p shamir-engine --all-targets --features captrack/telemetry -- -D warnings
cargo fmt -p shamir-engine -- --check
./scripts/test.sh @engine
```

ВСЕ ЗЕЛЁНЫЕ. `./scripts/test.sh @engine` должен дать те же 1261/1261 PASS
как до правок (поведение не меняется).

## Финальный отчёт

- diff в captrack (новый `aliases.rs`, обновлённый `lib.rs`, +12 `From` impl'ов в `tracked/*.rs`);
- diff в shamir-engine (4 файла);
- вывод всего гейта (15 команд);
- одно подтверждение: «исходник в обеих feature-ветках теперь идентичен,
  никаких `.into_vec()`/`.into_untracked()` в shamir-engine».

Если для какого-то Tracked-типа симметрию сделать невозможно (например
TrackedBytesMut не имеет `Default`-friendly `mem::take` пути) — поставь
TODO и не блокируй: важна Партия 1 (только Vec).
