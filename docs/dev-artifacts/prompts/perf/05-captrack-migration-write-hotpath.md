בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-#293a — captrack миграция (Партия 1: write hot-path)

> **Target:** заменить `Vec::new()`/`Vec::with_capacity(...)` на `tvec!(...)` в
> 4 файлах write hot-path — `write_exec.rs`, `write_helpers.rs`,
> `table_manager_tx_ops.rs`, `table_manager_crud.rs`. Это первая партия Фазы 2
> (#293) — только Vec, только write-path. Остальные семьи (HashMap, BTreeSet,
> DashMap, scc) и крейты (index, storage, server) — отдельные партии.

## ⛔ Запреты
- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор. НЕ удаляй отслеживаемые. НЕ sub-agent.
- Тесты — ТОЛЬКО `./scripts/test.sh` (raw `cargo test` заблокирован).
- Скоуп СТРОГО 4 файла. НЕ трогать другие файлы, тесты, бенчи.
- НЕ менять семантику. НЕ расставлять новые `with_capacity` где было `Vec::new()`
  с заведомо неизвестным размером — оставлять `cap = 0` (макрос принимает).
- НЕ менять `clippy.toml` workspace — ban расширим отдельной партией.

## Прочитанная реальность

captrack уже подключён в `crates/shamir-engine/Cargo.toml` (коммит `755f10b4`):
```toml
captrack = { path = "../../../captrack", features = ["fxhash"] }
```
Импорт макроса в каждом файле: `use captrack::tvec;` (в шапке файла, как все
остальные impro).

`tvec!("name", cap)` в off-feature (default) раскрывается **буквально в**
`Vec::with_capacity(cap)` — zero overhead, тип = `Vec<T>`. Никакой type-inference
не ломается; код выглядит идентично.

## Конвенция именования

`"<crate>/<module>/<role>"` — машинно-парсимое, человекочитаемое:
- `"engine/write_exec/staged_bytes"`
- `"engine/write_exec/new_base_keys"`
- `"engine/write_helpers/record_ids"`
- `"engine/table_manager_crud/unique_defs"`

Один call-site → одно имя. Если в одной функции 3 разных Vec — 3 разных role-имени.

## Задача — миграция в 4 файлах

Для **каждого** `Vec::new()` / `Vec::with_capacity(N)` в файле:

### Шаблон замены

```rust
// БЫЛО:
let mut staged: Vec<Bytes> = Vec::with_capacity(op.values.len());
// СТАЛО:
let mut staged: Vec<Bytes> = tvec!("engine/write_exec/staged_bytes", op.values.len());

// БЫЛО:
let mut keys: Vec<RecordId> = Vec::new();
// СТАЛО:
let mut keys: Vec<RecordId> = tvec!("engine/write_exec/keys_scratch", 0);
```

`tvec!` **обязан** принять capacity — для `Vec::new()` ставь `0` (макрос требует
literal-name + expr-cap; передавая `0` получаешь идентичное поведение
`Vec::with_capacity(0) == Vec::new()`).

### Файлы (по убыванию приоритета)

1. **`crates/shamir-engine/src/table/write_exec.rs`** (~14 мест). Самый горячий.
2. **`crates/shamir-engine/src/table/write_helpers.rs`** (несколько мест,
   например `apply_defaults`/`record_ids`/`apply_transforms`-snapshot).
3. **`crates/shamir-engine/src/table/table_manager_tx_ops.rs`** (~12 мест).
4. **`crates/shamir-engine/src/table/table_manager_crud.rs`** — там есть
   `unique_defs: Vec<IndexDefinition>` после фикса #290 (`iter_unique_indexes()
   .collect()`). НЕ переписывай эту collect() форму (она тоже даёт Vec); ИЛИ
   замени `let unique_defs: Vec<...> = ...iter_unique_indexes().collect();` на
   собирать через `tvec!` + push в цикле — НЕТ, оставь collect() с явной
   аннотацией: `collect()` уже не Vec::new()/with_capacity (не триггерится
   будущим ban'ом). **Перепиши ТОЛЬКО прямые** `Vec::new()`/`Vec::with_capacity()`.

### Что НЕ трогать в этих файлах

- `.collect::<Vec<_>>()` / `.collect()` в Vec — НЕ переписывать (это не ban-target).
- `.to_vec()`, `vec![...]` macros — НЕ переписывать.
- `Vec<T>` в типах (signatures, fields) — НЕ переписывать (это типы, не конструкторы).
- Тесты внутри файла (если есть `#[cfg(test)] mod tests { ... }` — пропусти их
  пока, отдельная партия).

## use captrack::tvec; — где?

Шапка каждого файла, рядом с другими `use` (top-of-file правило, CLAUDE.md).
Если файл уже содержит `use captrack::...` — расширь существующий `use`.

## Тесты

Поведенческой регрессии быть не должно (макрос раскрывается в идентичный код).
Запусти полный engine-suite:

```
./scripts/test.sh @engine
```

Должно быть 1252/1252 (или текущее число) PASS, идентично до миграции.

## Гейт (прогони сам, приложи ПОЛНЫЙ вывод)

```
./scripts/test.sh @engine
cargo clippy -p shamir-engine --all-targets -- -D warnings
cargo fmt -p shamir-engine -- --check
```

Всё зелёное. Если падает — НЕ ослабляй тест/код, сообщи с диагнозом
(скорее всего ты сломал семантику где-то — откати и перепроверь).

## Финальный отчёт

Список затронутых файлов; счётчик заменённых сайтов (per file); пример
«было/стало» с конвенцией имён; вывод гейта.

Bench-compare (criterion + flamegraph) — делает оркестратор сам через
`scripts/wsl-flame-bench.sh`. Поведенческого ускорения от этой партии НЕТ —
макрос идентичен `with_capacity` в off-feature; ценность партии — подготовка
к Фазе 3 #294 (instrumentation).
