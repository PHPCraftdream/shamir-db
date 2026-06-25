בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ①.1 — Rust builder: три мелкие escape-hatch-дыры

Кампания **① Builder parity & DX**, этап ①.1. Закрывает три низкоуровневые дыры
Rust query-builder (источник: `docs/research/coverage-rust-query-builder.md`
#25/#41/#12). Все три — паритет Rust←TS: TS-сторона эти формы уже умеет, Rust
заставляет собирать wire руками. Surface-only, движок/wire НЕ трогаем.

Крейт: `crates/shamir-query-builder`. Объём: S.

## Что сделать (ровно три helper'а)

### (1) `FnCall::Simple` helper — `val::func_simple(name)`
- **Файл:** `crates/shamir-query-builder/src/val/filter_value.rs` (модуль `val`).
- **Сейчас:** `val::func(name, args)` всегда строит `FilterValue::FnCall { call:
  FnCall::Complex { name, args } }`. Безаргументная wire-форма `{"$fn":"NOW"}`
  (`FnCall::Simple(String)`) конструктора не имеет.
- **Сделать:** добавить `pub fn func_simple(name: impl Into<String>) ->
  FilterValue` → `FilterValue::FnCall { call: FnCall::simple(name) }` (или
  `FnCall::Simple(name.into())` — посмотри точную форму конструктора `FnCall` в
  `shamir-query-types`). Реэкспортни `FnCall` из модуля `val`, если для теста/
  пользователя это удобнее (по образцу существующих реэкспортов).
- Зеркало в TS: `filter.fn(name)` без args (`filter.ts:245`) — паритет.

### (2) `res::function_folder(segments)` helper
- **Файл:** `crates/shamir-query-builder/src/ddl/res.rs`.
- **Сейчас:** есть `database() / store() / table() / function() /
  function_namespace()`, но нет helper'а для варианта
  `ResourceRef::FunctionFolder`.
- **Сделать:** добавить `pub fn function_folder(...) -> ResourceRef` по образцу
  соседних функций. Посмотри точную форму `ResourceRef::FunctionFolder` в
  `shamir-query-types` (сегменты пути — `Vec<String>` или аналог) и зеркаль
  сигнатуру TS `refFunctionFolder()` (`builders/admin.ts:66`).

### (3) `FieldBuilder::set()` / `FieldBuilder::null_type()` type-сеттеры
- **Файл:** `crates/shamir-query-builder/src/ddl/schema.rs` (`FieldBuilder`).
- **Сейчас:** есть type-сеттеры `string()/int()/f64()/dec()/bool()/bin()/list()/
  map()/any()`; нет `.set()` (TypeTag Set) и `.null_type()` (TypeTag Null).
  Работают только через `type_tag("set")` / `type_tag("null")` как escape.
- **Сделать:** добавить два сеттера по образцу существующих (`.string()` и т.п.):
  `pub fn set(self) -> Self` → ставит type-tag "set"; `pub fn null_type(self) ->
  Self` → ставит "null". Имя `null_type` (не `null`) — чтобы не путать с
  null-литералом. Сверь точные строковые теги с `ConstraintsDto`/`TypeTag` в
  `shamir-query-types`/engine (вероятно "set"/"null").

> `one_of` УЖЕ сделан (Phase G.1) — его здесь НЕТ.

## Тесты (обязательно)
Билдер-слой покрыт wire-shape unit-тестами. Добавь по тесту на каждый helper в
тестовую директорию крейта (`crates/shamir-query-builder/src/.../tests/` — найди,
куда сложены тесты для `val`/`ddl::res`/`ddl::schema`, и положи рядом, по образцу
существующих `*_tests.rs`; `tests/mod.rs` — только реэкспорт). Каждый тест строит
helper и `assert_eq!` точную wire-форму (msgpack/QueryValue) против ожидаемой —
тот же стиль, что у соседних тестов. Покрой: `func_simple` → `{"$fn":"NOW"}`;
`function_folder` → корректный `ResourceRef::FunctionFolder`; `set`/`null_type` →
корректный type-tag в `ConstraintsDto`.

## Гейт (прогнать самому, всё зелёное)
```
./scripts/test.sh -p shamir-query-builder
cargo fmt -p shamir-query-builder -- --check
cargo clippy -p shamir-query-builder --all-targets -- -D warnings
```
Тесты — ТОЛЬКО через `./scripts/test.sh` (raw `cargo test` заблокирован
perimeter-guard'ом). НЕ пайпь/grep вывод теста — читай поток целиком.

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай
  файлы напрямую (view/grep/edit).
- ⛔ NEVER `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, или
  любую git-команду, мутирующую рабочее дерево/индекс. Только редактируй файлы;
  коммитит оркестратор.
- Surgical: три helper'а + их тесты. Не трогай несвязанный код/комментарии.
  Импорты — в шапку файла. Один файл = один primary export.
- Queries (если зайдёт речь) — только через билдеры, никакого сырого
  `serde_json::Value`.
- Заверши финальным текстом: что добавил (file:line каждого helper'а + тесты) +
  вывод `./scripts/test.sh -p shamir-query-builder` (PASS-строки).
