בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# perf-③.289 — fast-path при пустых validator-bindings (AtomicUsize mirror)

> **Target:** на write hot-path в `run_validators_qv`/`run_validators_view`
> убрать обязательный `validator_bindings.load_full()` (Arc-clone + atomic-incr)
> ДО проверки `applicable.is_empty()`. Большинство таблиц в проде/бенче не имеют
> валидаторов → пустые bindings → atomic-traffic тратится впустую.
> Подпись профиля: часть `atomic_load 2.13%` + `Arc::Drop 1.85%` + `fetch_add
> 1.57%` (после #290). Ожидаемый выигрыш ~1.5-3% на бенче без валидаторов.

## ⛔ Запреты
- НЕ `git reset/checkout/clean/stash/restore/rm` и любая git-мутация дерева/индекса.
  Только редактируй; коммитит оркестратор. НЕ удаляй отслеживаемые. НЕ sub-agent.
- Тесты — ТОЛЬКО `./scripts/test.sh`. Хирургически.
- НЕ меняй семантику валидаторов. НЕ ослабляй invariants.

## Прочитанная реальность

### Структура `TableManager` — `crates/shamir-engine/src/table/table_manager.rs:56`

```rust
pub(super) validator_bindings: Arc<arc_swap::ArcSwap<Vec<crate::validator::ValidatorBinding>>>,
```

`ArcSwap::load_full()` создаёт **новый Arc** на каждый вызов → atomic-incr +
atomic-decr на drop. Per-record в batch insert это N таких операций.

### Hot-path call — `table_manager_validators.rs:117-186` `run_validators_qv`

```rust
// 1. No registry → return Ok early (uses Option::None, no atomics).
let reg = match &self.validator_registry { Some(r) => r, None => return Ok(()) };

// 2. ⚠ ВСЕГДА грузит Arc — atomic-incr — даже если bindings пуст.
let all_bindings = self.validator_bindings.load_full();
let applicable: Vec<&ValidatorBinding> = all_bindings.iter()
    .filter(|b| b.ops.contains(&op)).collect();

if applicable.is_empty() {
    return Ok(());
}
// ...
let scalar_res = self.scalar_resolver.load_full();
// ...
```

### Аналогично — `run_validators_view` (DELETE-путь, table_manager_validators.rs:206+)

Тот же паттерн `load_full()` ДО проверки пустоты.

### Mutation sites — `table_manager_validators.rs`

- `:36 add_validator_binding` — store new Arc
- `:57 remove_validator_binding` — store new Arc

Других путей мутации bindings нет (DDL-only).

## Задача (хирургическая)

### 1. Добавить `bindings_len: AtomicUsize` в `TableManager`

`crates/shamir-engine/src/table/table_manager.rs:56` — сразу под
`validator_bindings`:

```rust
/// Mirror of `validator_bindings.load_full().len()` for the hot-path
/// fast skip. Allows `run_validators_qv`/`run_validators_view` to early-return
/// on the common "no validators bound" case without paying for an
/// `ArcSwap::load_full()` Arc-clone. Updated atomically in
/// `add_validator_binding`/`remove_validator_binding` after the ArcSwap store.
pub(super) bindings_len: std::sync::atomic::AtomicUsize,
```

⚠ Use AtomicUsize, not AtomicBool — len() future-prooft если когда-то понадобится
быстрый `len`-readout без Arc-clone.

### 2. Init = 0 во всех TableManager-constructor'ах

Найди существующие сайты инициализации `validator_bindings: Arc::new(...)` —
рядом добавь `bindings_len: AtomicUsize::new(<actual_len>)`. Для bootstrap из
persisted bindings (там `pv.bindings` уже есть Vec) — `pv.bindings.len()`.
Для пустого `Vec::new()` (system tables / fresh tables) — `0`.

Точки (по моему grep'у — verify сам):
- `table_manager.rs:149-150` (новый TableManager из meta) — два кейса (Some(pv)/None).
- `table_manager.rs:306` (alternative constructor — если есть).

### 3. Update mirror в bind/unbind — `table_manager_validators.rs:36-71`

Внутри `add_validator_binding` — после `self.validator_bindings.store(Arc::new(bindings))` (стр.51):
```rust
self.bindings_len.store(bindings.len(), std::sync::atomic::Ordering::Release);
```
⚠ Но `bindings` уже moved в `Arc::new(...)`. Сделай так: захватить `let new_len = bindings.len()` ДО `store(Arc::new(bindings))`, потом `bindings_len.store(new_len, ...)`.

Аналогично в `remove_validator_binding` (стр.57-71) — в ветке `if removed { ... }`.

### 4. Fast-path в `run_validators_qv` — `table_manager_validators.rs:117-186`

Сразу ПОСЛЕ existing-check `validator_registry == None` (стр.127-130), добавить:

```rust
// Fast skip: most tables have no bound validators. Avoid the
// ArcSwap::load_full() Arc-clone in the empty case.
// Acquire ordering pairs with Release in add/remove_validator_binding.
if self.bindings_len.load(std::sync::atomic::Ordering::Acquire) == 0 {
    return Ok(());
}
```

### 5. То же — в `run_validators_view` (`:206+`)

После `validator_registry == None` check — тот же fast-skip.

### 6. Аналогично проверить `validator_bindings()` getter (стр.20)

Если он используется в hot-path (например в `add_validator_binding` для clone) —
оставить как есть; load_full неизбежен на DDL-пути. Но если есть hot-path call
(grep `self.validator_bindings()` в engine) — оценить, нужен ли fast-skip там тоже.

## Тесты (`./scripts/test.sh`, НЕ raw cargo)

- РЕГРЕССИЯ: existing validator tests зелёные:
  - `./scripts/test.sh -p shamir-engine -- validator`
  - `./scripts/test.sh @engine`
- Новые unit-тесты в `crates/shamir-engine/src/table/tests/`:
  - Mirror == bindings.len() после add/remove (single thread).
  - Concurrent add: 100 потоков bind разных validator_id → final mirror == 100 (или
    matches bindings.len(), если bind идемпотентен).
  - Fast-skip срабатывает: измерить через counter сколько раз `run_validators_qv`
    добралось до load_full при пустых bindings = 0.

⚠ В testing avoid relying on `bindings_len` directly outside engine — keep
`pub(super)` (как validator_bindings) или приватный getter for tests.

## Гейт (прогони сам, приложи ПОЛНЫЙ вывод)

```
./scripts/test.sh @engine
cargo clippy -p shamir-engine --all-targets -- -D warnings
cargo fmt -p shamir-engine -- --check
```

Всё зелёное. Memory-ordering обоснуй коммментом: Release при store (memory
publish — все читатели после load Acquire видят согласованное состояние с
ArcSwap-store).

## Финальный отчёт

Изменённые файлы; сигнатура поля `bindings_len`; «было/стало» для
`run_validators_qv` fast-skip; список mutation sites где обновлён mirror;
вывод гейта.

Bench-compare (criterion + flamegraph до/после) — делает оркестратор сам
через `scripts/wsl-flame-bench.sh`.
