# Brief: aggregate WASM fuel across nested Stores (taskId #612, partial HIGH)

## Контекст

`crates/shamir-wasm-host/src/wasm/wasm_function.rs:58-63` (doc-комментарий
модуля):

```
FOLLOW-UP (deferred, task #495 scope-down): fuel is still RESET to a full
budget per nested Store, so nested calls do not draw down from a shared
per-request fuel budget — wall-clock + epoch cap total time but not total
instructions across the fan-out. A genuine AGGREGATE cross-Store fuel
budget (threading a shared remaining-fuel counter through `host_call.rs`)
is a larger Store-lifecycle change left as a documented follow-up.
```

Каждый вложенный вызов `ctx.call(...)` (host import `call`, реализован в
`crates/shamir-wasm-host/src/wasm/host_call.rs`) создаёт НОВЫЙ
`wasmtime::Store` (в `WasmFunction::call`, `wasm_function.rs:430-434`) и
СБРАСЫВАЕТ fuel до полного бюджета (`store.set_fuel(limits.fuel)`).
Значит guest-функция, рекурсивно вызывающая себя (или другие функции) N
раз, может исполнить N × `limits.fuel` инструкций суммарно, а не
`limits.fuel` — instruction-count cap не является реальным пределом при
фан-ауте. Wall-clock (`tokio::time::timeout`) и epoch-interruption всё ещё
ограничивают ОБЩЕЕ время, так что это НЕ open-ended DoS, но это residual
gap, который явно назван в аудите.

## Хорошая новость: изменение локализовано внутри `shamir-wasm-host`

Трогать `shamir-db`'s call sites (`function_management.rs`,
`core.rs::build_invoke_ctx`) НЕ нужно — они не знают о `WasmLimits`
вообще (это deep внутренний концепт `shamir-wasm-host`). Вся работа — в
трёх файлах: `context.rs` (добавить поле), `wasm_function.rs` (сеять
бюджет на depth-0, читать/списывать на каждом Store), `host_call.rs`
(протащить бюджет в child `FnCtx`, exactly как уже протаскиваются
`depth`/`depth_limit`).

## Задача

### 1. `FnCtx` — новое поле `fuel_budget`

`crates/shamir-wasm-host/src/context.rs`, `struct FnCtx` (строка ~296) —
добавь:

```rust
/// Shared remaining-fuel counter for the aggregate cross-Store fuel
/// budget (task #612). `None` in a fresh top-level `FnCtx` — the
/// top-level Store creation in `WasmFunction::call` lazily seeds it
/// from `WasmLimits::fuel` on first use and threads the SAME `Arc`
/// through every nested `ctx.call` (via `host_call.rs`'s child `FnCtx`
/// construction), so instruction consumption across the WHOLE fan-out
/// draws down from one shared budget instead of resetting per Store.
fuel_budget: Option<Arc<std::sync::atomic::AtomicI64>>,
```

Добавь во ВСЕ существующие конструкторы (`new()`, `with_globals()`) —
инициализируй `None` (не меняй их публичные сигнатуры, только тело).

Добавь builder + getter рядом с `with_depth`/`depth()`-подобными
методами:

```rust
/// Builder: attach a shared aggregate fuel-budget counter (task #612).
/// Used internally by `host_call.rs` to thread the SAME counter into a
/// nested call's `FnCtx` — do not call this from outside `shamir-wasm-host`.
pub(crate) fn with_fuel_budget(mut self, budget: Arc<std::sync::atomic::AtomicI64>) -> Self {
    self.fuel_budget = Some(budget);
    self
}

/// The aggregate fuel-budget counter, if one has been seeded (always
/// `Some` once a top-level `WasmFunction::call` has run at least once
/// for this ctx chain).
pub(crate) fn fuel_budget(&self) -> Option<&Arc<std::sync::atomic::AtomicI64>> {
    self.fuel_budget.as_ref()
}
```

(`pub(crate)` — этот механизм внутренний для `shamir-wasm-host`, не
публичный API; проверь что `wasm_function.rs`/`host_call.rs` видят
`pub(crate)` члены `context.rs` — если модульная видимость не позволяет,
подбери минимально достаточную видимость.)

### 2. `HostState` — протащить `Arc` в Store

`crates/shamir-wasm-host/src/wasm/wasm_function.rs`, `struct HostState`
(строка ~94-100) — добавь поле:

```rust
pub(super) fuel_budget: Arc<std::sync::atomic::AtomicI64>,
```

### 3. `WasmFunction::call` — сеять/читать/списывать бюджет

`wasm_function.rs`, метод `call` (строка ~389+), НЕПОСРЕДСТВЕННО ПЕРЕД
Store creation (строка ~414-434):

```rust
use std::sync::atomic::{AtomicI64, Ordering};

let fuel_budget = ctx
    .fuel_budget()
    .cloned()
    .unwrap_or_else(|| Arc::new(AtomicI64::new(limits.fuel as i64)));

let remaining = fuel_budget.load(Ordering::Relaxed);
if remaining <= 0 {
    return Err(FunctionError::Compute(
        "aggregate fuel budget exhausted across nested calls".into(),
    ));
}
// Still bounded above by this call's own per-Store ceiling — a single
// nested call can never draw MORE than its normal quota even if the
// shared budget has generous headroom left (defense-in-depth).
let grant = (remaining as u64).min(limits.fuel);
```

`HostState { ... }` конструктор (строка ~418-429) — добавь
`fuel_budget: fuel_budget.clone(),`.

`store.set_fuel(limits.fuel)` (строка ~433) → `store.set_fuel(grant)`.

### 4. Списание фактически потреблённого fuel — ОДНА точка выхода

Это самая деликатная часть: сейчас функция `call` имеет МНОЖЕСТВО ранних
`return`/`?`-выходов (instantiate failure, missing export, alloc/call
ошибки, guest trap). Fuel списывается из `store` НЕЗАВИСИМО от того,
какой ветвью функция завершилась — значит нужно прочитать
`store.get_fuel()` и списать с `fuel_budget` РОВНО ОДИН РАЗ, на любом
пути выхода (успех ИЛИ ошибка).

Рекомендуемый подход: заверни существующее "тело" функции (всё после
Store creation, до текущего конца) во внутренний `async` блок/замыкание,
захватывающий `&mut store` по ссылке (НЕ по значению — `store` не
должен быть перемещён внутрь замыкания так, чтобы после его выполнения
`store` стал недоступен снаружи). Например:

```rust
let result: FnResult<QueryValue> = async {
    // ... весь существующий код инстанцирования/вызова/чтения результата,
    // возвращающий FnResult<QueryValue> вместо текущих ранних `return`
    // (замени на `?`-пропагацию где возможно, ИЛИ оставь как отдельную
    // именованную функцию `fn run_instance(store: &mut Store<HostState>, ...) -> FnResult<QueryValue>`
    // если async-замыкание с заимствованием окажется неудобным в Rust
    // из-за borrow checker — на твоё усмотрение, главное соблюсти
    // инвариант "store остаётся доступным ПОСЛЕ этого блока").
}.await;

// Списание — ПОСЛЕ завершения блока, независимо от Ok/Err.
let remaining_after = store.get_fuel().unwrap_or(0);
let consumed = grant.saturating_sub(remaining_after);
fuel_budget.fetch_sub(consumed as i64, Ordering::Relaxed);

result
```

Если рефакторинг во внутреннюю функцию/замыкание слишком инвазивен для
существующей структуры — альтернатива: добавь fuel-списание ПЕРЕД
КАЖДЫМ существующим `return Err(...)` и в конце перед финальным `Ok(...)`
(более многословно, но менее рискованно для непреднамеренной поломки
borrow-checker'а). Выбери подход, который компилируется чище — оба
корректны, если гарантируют "списание происходит ровно один раз для
каждого вызова `call`, на любом пути выхода".

### 5. `host_call.rs` — протащить `Arc` в child `FnCtx`

`crates/shamir-wasm-host/src/wasm/host_call.rs`, Phase 1 (строка ~54-62,
где `registry`/`batch_ctx`/`globals`/`next_depth`/`depth_limit`
клонируются из `state`) — добавь:

```rust
let fuel_budget = state.fuel_budget.clone();
```

Строка ~87-90 (`child_ctx` construction) — добавь
`.with_fuel_budget(fuel_budget)`:

```rust
let child_ctx = FnCtx::with_globals(globals)
    .with_registry(reg)
    .with_depth(next_depth)
    .with_depth_limit(depth_limit)
    .with_fuel_budget(fuel_budget);
```

## Тесты

`crates/shamir-wasm-host/src/tests/wasm_tests.rs` — уже есть отличный
образец (`wasm_fuel_exhaustion_traps`, `wasm_wall_clock_deadline_interrupts_cpu_bound_guest`,
строки ~133-199). Добавь новый тест:

`wasm_aggregate_fuel_exhausted_across_nested_calls` (имя ориентировочное):
- Зарегистрируй ДВЕ WAT-функции (или используй `FunctionRegistry` с
  одной функцией, вызывающей саму себя рекурсивно через host import
  `call`, если такой WAT-фикстур уже есть в тестах — посмотри
  существующие `call`-related WAT в `wasm_tests.rs`/соседних файлах для
  готового образца рекурсивного вызова).
- Установи `limits.fuel` на небольшое значение (например 10_000),
  `depth_limit` достаточно большим, чтобы рекурсия не упёрлась в
  depth-limit раньше, чем в fuel.
- Каждый рекурсивный вызов потребляет какое-то количество fuel; суммарно
  за N вызовов бюджет должен исчерпаться и функция должна вернуть
  `FunctionError::Compute` с сообщением, содержащим "aggregate fuel
  budget exhausted" (а не тихо продолжить работать, как было бы ДО
  фикса, когда каждый Store получал полный `limits.fuel` заново).
- **Проверка регресса**: убедись, что тест ДЕЙСТВИТЕЛЬНО падает на
  СТАРОМ коде (до фикса) — если непонятно как проверить без временного
  отката, хотя бы обоснуй логически в комментарии теста, почему старое
  поведение ("fuel resets per Store") НЕ поймало бы эту ситуацию.
- Второй тест (не обязателен, но желателен): один top-level вызов БЕЗ
  вложенных `ctx.call` — убедись, что дефолтное поведение (когда бюджет
  не исчерпан ни разу за фан-аут) идентично старому: функция с
  `limits.fuel` достаточным для её собственной работы завершается
  успешно, как и раньше (не путай "агрегатный бюджет" с "любой fuel
  теперь всегда меньше").

## Прогон проверок

- `cargo fmt -p shamir-wasm-host -- --check`
- `cargo clippy -p shamir-wasm-host --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-wasm-host --full`
- Также прогони `./scripts/test.sh -p shamir-db --full` (function
  invocation e2e тесты в `shamir-db` косвенно зависят от этого пути —
  `functions_e2e.rs`/`functions_lifecycle.rs` не должны сломаться).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `depth`/`depth_limit` recursion-guard логику — она уже
  правильно реализована и не пересекается с fuel (два разных, независимых
  предела).
- НЕ трогай wall-clock/epoch-interruption механизм (`deadline`,
  `set_epoch_deadline`) — уже правильно реализован, не в scope.
- НЕ меняй публичный API `shamir-db` (`build_invoke_ctx`,
  `invoke_function_in_db*`) — весь фикс должен уместиться внутри
  `shamir-wasm-host`, если тебе кажется что нужно touch shamir-db —
  остановись и перепроверь дизайн, скорее всего это не так.
- НЕ убирай доку-комментарий про "FOLLOW-UP (deferred, task #495
  scope-down)" молча — обнови его, отразив что фикс теперь реализован
  (task #612), не удаляй историю решения.

## Проверка (сделает оркестратор)

- Диф ограничен `context.rs`, `wasm_function.rs`, `host_call.rs` (все
  три в `shamir-wasm-host`), плюс новый(е) тест(ы) в `wasm_tests.rs`.
- НИКАКИХ изменений в `shamir-db`/`shamir-server` — если диф их
  затрагивает, это сигнал, что дизайн пошёл не по плану, нужно
  разобраться перед коммитом.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-wasm-host --full` и
  `./scripts/test.sh -p shamir-db --full` зелёные.
- Новый тест реально ловит регресс: рекурсивный фан-аут с маленьким
  per-call `limits.fuel`, но большим количеством вызовов, теперь
  исчерпывает АГРЕГАТНЫЙ бюджет и падает — а не продолжает работать
  бесконечно (в пределах wall-clock), как было до фикса.
