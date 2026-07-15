# OQL: вложенные батчи — реальная рекурсия или DAG-иллюзия?

**Дата:** 2026-07-15. Read-only исследование кода репозитория shamir-db.

## Краткий вердикт (честный ответ)

**Да, это НАСТОЯЩАЯ рекурсивная вложенность батчей как структура данных, с настоящим
рекурсивным исполнителем на движке.** Это не «DAG-зависимости, замаскированные под
вложенность»: `BatchOp` имеет вариант `Batch(SubBatchOp)`, где `SubBatchOp.batch` — это
полноценный `BatchRequest` (то есть batch-in-batch-in-batch рекурсивно, wire-ключ
`"batch"`), и движок исполняет его рекурсивным вызовом `execute_batch_impl` с
инкрементом глубины.

**НО** формулировка «вложенный батч имеет доступ к значениям родительских батчей»
верна только с оговоркой: **никакого лексического скоупинга родительских алиасов нет**.
Вложенный батч самодостаточен (self-contained): его `$query`-ссылки валидируются
только против ЕГО СОБСТВЕННОЙ карты `queries` — прямая ссылка `$query @outer_alias`
изнутри саб-батча даст ошибку `UnknownAlias`. Доступ к родительским значениям
осуществляется **только через явный механизм `bind` + `$param`**: родитель при
объявлении саб-батча явно привязывает параметры (значения-литералы, `$query`-ссылки
на СВОИ sibling-алиасы или `$param`-пропагацию из своего собственного скоупа), а
внутренние операции читают их как `{"$param": "name"}`. Пропагация через уровни —
цепочкой bind→$param→bind.

Глубина ограничена `BatchLimits::max_nesting_depth` (default 4).

---

## 1. Структура данных: batch внутри batch — есть буквально

`BatchOp` (enum операций внутри entry) имеет вариант вложенного батча:

- `crates/shamir-query-types/src/batch/batch_op.rs:142-143`:
  ```rust
  /// Nested sub-batch — recursive execution with its own tx scope.
  Batch(SubBatchOp),
  ```
- Wire-диспетчеризация по ключу `"batch"` — `batch_op.rs:419-420`:
  ```rust
  } else if has("batch") {
      qv_to::<SubBatchOp, _>(&bytes).map(BatchOp::Batch)
  ```
- `SubBatchOp` содержит ПОЛНЫЙ `BatchRequest` + карту биндингов —
  `crates/shamir-query-types/src/batch/sub_batch_op.rs:11-16`:
  ```rust
  pub struct SubBatchOp {
      pub batch: BatchRequest,
      #[serde(default, skip_serializing_if = "TMap::is_empty")]
      pub bind: TMap<String, FilterValue>,
  }
  ```

`BatchRequest.queries: TMap<String, QueryEntry>` (`batch_request.rs:87`), а
`QueryEntry.op: BatchOp` (`query_entry.rs:36-37`) — значит рекурсия типов замкнута:
batch → entry → op → Batch(SubBatchOp) → batch → … Любая глубина представима в
самой структуре данных.

`is_write` для саб-батча рекурсивен — `batch_op.rs:752`:
```rust
BatchOp::Batch(sub) => sub.batch.queries.values().any(|qe| qe.op.is_write()),
```

## 2. Исполнение на движке: настоящая рекурсия, не флэттенинг

Дизайн-документ `docs/dev-artifacts/roadmap/NESTED_BATCHES.md` (строки 10-18,
«Approved decisions») зафиксировал: **«True nesting (recursive batch executor), NOT
flatten»** — хотя §4 того же документа изначально рекомендовал флэттенинг, финальное
решение (revision 2026-06-09) — рекурсия. Реализация соответствует финальному решению.

- `crates/shamir-engine/src/query/batch/query_runner.rs:106-108` — саб-батч
  перехватывается до admin-диспетчера:
  ```rust
  // Sub-batch — handle before is_admin() so we can recurse rather
  // than delegating to AdminExecutor (which has no recursion seam).
  if let BatchOp::Batch(sub) = &entry.op {
  ```
- `query_runner.rs:167-177` — буквальный рекурсивный вызов исполнителя:
  ```rust
  // Recurse into the sub-batch.
  let inner_response = execute_batch_impl(
      &sub.batch, self.resolver, self.admin, self.invoker,
      self.actor.clone(), self.db_name,
      self.depth + 1,
      &resolved_params,
  ).await ...
  ```
- `crates/shamir-engine/src/query/batch/batch_execute.rs:45-56` — комментарий
  подтверждает взаимную рекурсию: «Called recursively by `QueryRunner::run` when it
  encounters a `BatchOp::Batch` entry … Returns a boxed future because the function
  is mutually recursive (QueryRunner::run → execute_batch_impl → execute_plan_impl →
  …)». Параметры `depth: usize` и `params: &TMap<String, QueryValue>` протянуты через
  все уровни (`batch_execute.rs:67-68, 107-108, 279-280, 362-363, 478-479`).
- Каждый уровень вложенности **заново планируется** своим `BatchPlanner::plan`
  (внутри `execute_batch_impl`) — то есть у каждого саб-батча свой собственный
  DAG-план по его собственным алиасам.

Результат саб-батча упаковывается в `QueryResult.value` как msgpack-карта его
внутренних алиасов (`query_runner.rs:185-205`), так что ВНЕШНИЕ операции адресуют его
результаты как `$query @sub.alias_name[0].id`.

### Ограничение глубины

- `crates/shamir-query-types/src/batch/planner.rs:101-108` — статическая проверка
  глубины вложенности на этапе планирования:
  ```rust
  let nesting = Self::max_nesting_depth_of_queries(queries);
  if nesting > limits.max_nesting_depth {
      return Err(BatchError::NestingTooDeep { depth: nesting, max: ... });
  ```
- Default 4 — тест `batch_types_tests.rs:612-613`:
  `assert_eq!(BatchLimits::default().max_nesting_depth, 4);`
- Тесты `nesting_depth_within_limit_ok` / `nesting_depth_exceeded_errors` —
  `crates/shamir-query-types/src/batch/tests/planner_tests.rs:163, 203`.

### Транзакции: tx-in-tx запрещён

`query_runner.rs:113-119` — транзакционный саб-батч внутри уже открытой транзакции
→ ошибка `nested_tx_not_supported`. Тест: `tx_in_tx_rejected`
(`crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs:410`). Семантика
(NESTED_BATCHES.md:5-8): внешний батч — оркестратор; атомарность живёт внутри
саб-батчей («подождать транзакцию»).

## 3. Доступ к родительским значениям: только явный `bind` + `$param`, не лексический скоупинг

Дизайн-решение (NESTED_BATCHES.md:12-13): «Data into sub-batch: explicit `bind`
params (variant P) + `$param` values. **Sub-batch is self-contained (no lexical
scoping of parent aliases)**».

Реализация подтверждает:

1. **Внутренние `$query` не видят внешних алиасов.** Планировщик каждого уровня
   валидирует все `$query`-ссылки против СВОЕЙ карты queries —
   `planner.rs:126-133`:
   ```rust
   for dep in &deps {
       if !aliases.contains(dep) {
           return Err(BatchError::UnknownAlias { ... });
   ```
   Поскольку саб-батч планируется отдельно (свой вызов `BatchPlanner::plan` внутри
   рекурсивного `execute_batch_impl`), внутренняя ссылка `$query @outer_alias`
   упадёт с `UnknownAlias`.

2. **`bind` резолвится в СКОУПЕ РОДИТЕЛЯ.** `query_runner.rs:121-165`: значения
   `bind` — это `FilterValue`, разрешаемые против `resolved_refs` текущего (внешнего)
   уровня (то есть `$query`-ссылки на sibling-алиасы родителя) либо против
   `self.params` родителя (`FilterValue::Param` в bind = пропагация параметра с
   уровня выше — `query_runner.rs:136-147`). Незарезолвленный `$param` в bind →
   ошибка `unbound_param` (тест `unbound_param_in_bind_errors`,
   `sub_batch_tests.rs:623`).

3. **Внутри саб-батча значения читаются как `{"$param": "name"}`** —
   `FilterValue::Param`, билдер `param(name)` в
   `crates/shamir-query-builder/src/val/filter_value.rs:123-135`. Незабинденный
   `$param` внутри фильтра — silent miss (0 записей), не ошибка (тест
   `unbound_param_in_filter_is_silent_miss`, `sub_batch_tests.rs:515`).

Итого «доступ к значениям родительских батчей» есть, но он **explicit-dataflow**:
родитель протаскивает конкретные значения через `bind`; на N уровней вглубь —
цепочкой `bind: { x: {"$param": "x"} }` на каждом уровне. Автоматической видимости
дедовских алиасов нет.

**Данные наружу:** алиас саб-батча — обычный `QueryResult`; внешние sibling'и
адресуют внутренние результаты через `$query @sub.inner[0].field`
(`query_runner.rs:190-192`, NESTED_BATCHES.md:14).

## 4. Тесты и примеры «батч внутри батча» — есть, много

- **Движок:** `crates/shamir-engine/src/query/batch/tests/sub_batch_tests.rs` —
  `sub_batch_runs_and_outer_reads_result` (:81), `sub_batch_bind_injects_param`
  (:164), `sub_batch_atomic` (:295), `tx_in_tx_rejected` (:410),
  `unbound_param_in_filter_is_silent_miss` (:515), `unbound_param_in_bind_errors`
  (:623), `param_in_insert_values` (:722).
- **Типы:** `crates/shamir-query-types/src/batch/tests/planner_tests.rs` (nesting
  depth), `batch_types_tests.rs` (serde, default limits).
- **Rust-билдер:** `crates/shamir-query-builder/src/batch/tests/sub_batch_tests.rs`;
  сам билдер — `crates/shamir-query-builder/src/batch/batch.rs`.
- **TS-клиент:** `crates/shamir-client-ts/src/core/builders/batch.ts:106-134` —
  метод `subBatch(alias, inner, opts?)`: принимает `Batch` или сырой `BatchRequest`,
  собирает `{ batch: resolved, bind?: {...} }`. `tryBuild()` (:242-265) валидирует
  `$query`/`after` ссылки — против алиасов СВОЕГО батча (sibling-only), что
  зеркалит серверную семантику. `collectQueryRefs` обходит и `$fn.args`, `$expr.args`,
  `$cond.then/else`; `$param` намеренно не считается ссылкой (:350).

## 5. Отдельный путь реальной рекурсии: WASM

Существует и второй, динамический механизм вложенности — через WASM-функции:

- `crates/shamir-wasm-host/src/wasm/host_db.rs:160-190` — host-функция
  `db_execute(req_ptr, req_len)`: «Reads a msgpack `BatchRequest` from guest memory,
  runs it through the gateway (**same executor a wire client uses**, as the
  function's effective actor)». То есть WASM-функция, вызванная из батча через
  `BatchOp::Call`, может сама сконструировать и исполнить НОВЫЙ полноценный
  `BatchRequest` (который сам может содержать саб-батчи и Call'ы).
- `crates/shamir-wasm-host/src/db_gateway.rs:9` — даже простые `get/insert/query`
  из WASM внутри строятся как одно-оп `BatchRequest`.
- Глубина вложенных WASM-вызовов ограничена своим лимитом —
  `crates/shamir-wasm-host/src/wasm/host_call.rs:83`: `if next_depth > depth_limit`.

Это рекурсия «движок изнутри движка» по вызову, независимая от структурной
вложенности `SubBatchOp`, и она НЕ разделяет транзакционный контекст внешнего батча
(Call — autocommit-делегирование, `query_runner.rs:220-230`).

## Вывод

1. **Batch-in-batch как структура данных — реален** (`BatchOp::Batch(SubBatchOp)`,
   рекурсивные типы, wire-ключ `"batch"`), исполнение — **настоящая рекурсия**
   (`execute_batch_impl` вызывает сам себя через `QueryRunner::run`, boxed mutually
   recursive future), глубина ограничена `max_nesting_depth` (default 4).
2. При этом внутри ОДНОГО уровня «вложенность» зависимостей — это по-прежнему
   плоский DAG (`after` + `$query` между sibling-алиасами, `BatchPlanner`), а КАЖДЫЙ
   уровень саб-батча имеет свой отдельный DAG.
3. **«Доступ к родительским значениям» — не лексический**: внутренний `$query` на
   внешний алиас → `UnknownAlias`. Доступ есть только через явный `bind`-мост
   (значения из родительского скоупа, включая `$query` на sibling'ов родителя и
   пропагацию `$param` с уровня выше) и чтение через `{"$param": name}` внутри.
   Наружу — через `$query @sub.inner_alias[...]`.
4. Дополнительный, отдельный путь реальной рекурсии — WASM-функция (`Call`) может
   исполнить новый `BatchRequest` через host-функцию `db_execute` (свой depth-limit,
   без общей транзакции).

Так что утверждение «в OQL уже можно вкладывать батчи рекурсивно» — **верно**
(до 4 уровней по умолчанию), а «вложенный батч получает доступ к значениям
родительских батчей» — **верно с оговоркой**: только через явные `bind`/`$param`
привязки, а не через прямую видимость родительских алиасов.
