בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-#302 — captrack telemetry-on build fix (TrackedVec ↔ Vec)

> **Target:** починить `cargo build -p shamir-engine --benches --features captrack/telemetry`,
> сейчас 34 ошибки E0308 в 4 файлах из Партии 1 (#296). Сохранить корректный peak-tracking.

## ⛔ Запреты

- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор.
- Тесты — ТОЛЬКО `./scripts/test.sh` (raw `cargo test` заблокирован).
- НЕ менять API tvec! макроса (`tvec!(name, cap)` остаётся как есть в обеих ветках feature).
- НЕ менять семантику бизнес-логики 4 файлов; только тип-приведения.
- НЕ трогать другие крейты, тесты, бенчи.

## Прочитанная реальность

В captrack `tvec!` имеет 2 ветви по feature:
- **off**: раскрывается в `::std::vec::Vec::with_capacity(cap)` → тип `Vec<T>`. Здесь всё работает.
- **on**: раскрывается в `$crate::TrackedVec::with_capacity_named(cap, name)` → тип `TrackedVec<T>`.

`TrackedVec<T>` (`D:\dev\rust\captrack\src\tracked\vec.rs`):
- `Deref<Target = Vec<T>>` + `DerefMut` → 90% операций работают через автодереф (push/iter/len/index).
- `Drop` → пишет `peak_capacity = inner.capacity()` в registry.
- `IntoIterator` → корректно: `record_peak` сначала, `mem::take` + `mem::forget(self)` (Drop не запускается дважды).

**НО**: `From<TrackedVec<T>> for Vec<T>` НЕТ. Поэтому `.into()` не сработает.

В Партии 1 миграции (#296) ВСЕ 34 места используют форму:
```rust
let mut staged: Vec<Bytes> = tvec!("engine/write_exec/staged_bytes", op.values.len());
```
Это работает в off-feature (`Vec<T> = Vec<T>`), но в on-feature `Vec<T> = TrackedVec<T>` → E0308.

Дополнительно: в `write_helpers.rs:417` есть `Ok(Some(result))` где `Option::Some` ожидает `Vec<T>` (тип возврата функции жёсткий).

## Задача — два шага

### Шаг 1. captrack: добавить `From<TrackedVec<T>> for Vec<T>`

В `D:\dev\rust\captrack\src\tracked\vec.rs` — добавить под существующий `impl IntoIterator`:

```rust
impl<T> From<TrackedVec<T>> for Vec<T> {
    fn from(mut tv: TrackedVec<T>) -> Vec<T> {
        // Зеркало `into_iter()`: записываем peak ДО move-out, иначе
        // Drop увидит capacity()==0 после `mem::take` и затрёт реальные данные.
        crate::registry::record_peak(tv.name, tv.inner.capacity());
        let inner = std::mem::take(&mut tv.inner);
        std::mem::forget(tv);
        inner
    }
}
```

Гейт для captrack:
```
cd D:\dev\rust\captrack
cargo build --all-features
cargo build  # default features (no telemetry)
cargo test
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt -- --check
```

### Шаг 2. shamir-engine: убрать `Vec<T>` аннотации, добавить `.into()` на boundary

В 4 файлах:
- `crates/shamir-engine/src/table/write_exec.rs`
- `crates/shamir-engine/src/table/write_helpers.rs`
- `crates/shamir-engine/src/table/table_manager_tx_ops.rs`
- `crates/shamir-engine/src/table/table_manager_crud.rs`

Для КАЖДОГО `let SOMETHING: Vec<T> = tvec!(...)`:

**Правило A.** Если переменная внутри функции используется только через Vec-like API
(`.push`, `.iter`, `.len`, индекс, итерация) — снять аннотацию:
```rust
// БЫЛО:
let mut staged: Vec<Bytes> = tvec!("engine/write_exec/staged_bytes", op.values.len());
// СТАЛО:
let mut staged = tvec!("engine/write_exec/staged_bytes", op.values.len());
```
Все методы через DerefMut продолжают работать в on-feature, и тип в off-feature
тоже `Vec<Bytes>` через inference.

**Правило B.** Если переменная далее возвращается / помещается в struct / передаётся
по значению как `Vec<T>` (где Deref-coercion не срабатывает) — оставить аннотацию И
добавить `.into()`:
```rust
// БЫЛО:
let mut result: Vec<(RecordId, Value<InternerKey>)> = tvec!("name", n);
... result.push(...); ...
Ok(Some(result))
// СТАЛО:
let mut result = tvec!("name", n);
... result.push(...); ...
Ok(Some(result.into()))
```
`.into()` сработает в обеих ветках (off: `Vec<T> -> Vec<T>` идентичность, on: новый `From<TrackedVec<T>> for Vec<T>` из Шага 1).

**Эвристика:** rustc сам подскажет где Правило B нужно (E0308 при компиляции с
`--features captrack/telemetry`). Просто следуй ошибкам компилятора.

## Гейт (агент сам прогоняет, приложи ПОЛНЫЙ вывод)

```
cd D:\dev\rust\shamir-db
cargo build -p shamir-engine --benches
cargo build -p shamir-engine --benches --features captrack/telemetry
cargo clippy -p shamir-engine --all-targets -- -D warnings
cargo fmt -p shamir-engine -- --check
./scripts/test.sh @engine
```

ВСЕ ПЯТЬ ЗЕЛЁНЫЕ. Особо: `./scripts/test.sh @engine` должен показать те же 1261/1261 PASS как до правок (поведенческой регрессии быть не должно — `.into()` это no-op на типах).

Если test.sh падает — НЕ ослабляй тест/код; сообщи с диагнозом.

## Финальный отчёт

- diff в captrack (1 файл) + diff в shamir-engine (4 файла, число `.into()` и снятых аннотаций per file);
- вывод гейта (все 5 команд);
- одно короткое подтверждение что in/out feature ветки больше не расходятся по типу.
