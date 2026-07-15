# Brief: OQL Epic 04 / Phase B — движок: `ForEachOp` (task #653)

## Контекст

ADR: `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` (прочитай
ПОЛНОСТЬЮ перед началом — все 5 решений там зафиксированы с обоснованием).

Существующий рекурсивный паттерн саб-батча — образец для переиспользования:
`crates/shamir-query-types/src/batch/sub_batch_op.rs` (`SubBatchOp { batch:
BatchRequest, bind: TMap<String, FilterValue> }`),
`crates/shamir-engine/src/query/batch/query_runner.rs` (обработка
`BatchOp::Batch(sub)` — рекурсивный вызов `execute_batch_impl` с `depth +
1`, резолв `bind` в скоупе родителя, читается внутри как `$param`).

## Задача

### 1. `ForEachOp` — новый тип

`crates/shamir-query-types/src/batch/` — новый файл `for_each_op.rs` (по
аналогии с `sub_batch_op.rs`, "один файл — один экспорт"):

```rust
pub struct ForEachOp {
    pub over: FilterValue,
    pub bind_row: String,
    pub batch: BatchRequest,
}
```

Добавь `BatchOp::ForEach(ForEachOp)` (wire-ключ `"for_each"`) в
`crates/shamir-query-types/src/batch/batch_op.rs`, по аналогии с
существующим `BatchOp::Batch(SubBatchOp)` (wire-ключ `"batch"`,
`batch_op.rs:419-420` — grep точную диспетчеризацию). `is_write` —
`ForEach(fe) => fe.batch.queries.values().any(|qe| qe.op.is_write())`
(идентично существующему `Batch(sub)`-случаю, `batch_op.rs:752`).

### 2. `BatchLimits::max_iterations`

`crates/shamir-query-types/src/batch/batch_limits.rs` (или где реально
лежит `BatchLimits`) — новое поле `max_iterations: usize`, дефолт `1000`
(зафиксировано в ADR Decision 3). Обнови `Default` impl и любые
сериализационные тесты, ожидающие точный набор полей.

### 3. `BatchPlanner` — `ForEach` как "чёрный ящик"

`crates/shamir-query-types/src/batch/planner.rs::plan` — `ForEach`-узел
должен: (а) извлечь зависимости из `over` (переиспользуй
`extract_deps_from_filter_value`, уже исправленную в Epic03/B для #642) —
это deps РОДИТЕЛЬСКОГО уровня для узла `ForEach`; (б) НЕ разворачивать
тело (`fe.batch`) в родительский DAG — тело останется непланированным на
этом уровне, будет распланировано ЗАНОВО в рантайме (тот же паттерн, что
`SubBatchOp` уже делает — сверься с тем, как `Batch(sub)` обрабатывается в
`extract_dependencies`/nesting-depth коде, `max_nesting_depth_of_queries`
должен учитывать глубину тела `ForEach` так же, как тела `Batch(sub)`).

Статический DoS-гейт (ADR Decision 3): если `over` — литеральный список
(`FilterValue::Array` без `$query`-рефа внутри), длина списка ИЗВЕСТНА на
этапе планирования — проверь
`literal_len × fe.batch.queries.len() ≤ limits.max_queries` (или похожий
разумный бюджет — реши по месту, ADR не даёт точную формулу, только
принцип "произведение iterations × ops(тела) учитывается"). Если `over` —
`$query`-column-реф (динамический), длина неизвестна на этом этапе —
пропусти статическую проверку, полагайся на рантайм-гейт (пункт 4).

### 4. Executor — K-кратное исполнение

`crates/shamir-engine/src/query/batch/batch_execute.rs`/`query_runner.rs` —
при встрече `BatchOp::ForEach(fe)`:

1. Резолвь `fe.over` через `resolve_filter_query` (или напрямую через
   `resolve_query_ref_column`, если `over` — колонка `@alias[].field` —
   grep существующий код для этого механизма, вероятно уже используется
   в In/NotIn-фильтрах, Epic01 research §2) — получи `Vec<QueryValue>`.
2. Рантайм-гейт: если `actual_len > limits.max_iterations` — верни ошибку
   ДО первой итерации (не частично исполняй и потом падай).
3. Для каждого элемента `i` в `0..actual_len`: рекурсивно вызови
   `execute_batch_impl` на `fe.batch` с `depth + 1` и с
   `params: { fe.bind_row.clone(): element_i }` (тот же путь, что
   `SubBatchOp`'s `bind`-резолв — просто здесь `params`-карта строится по
   одному ключу `bind_row`, не по множеству ключей `bind`).
4. Накопи результаты итераций в `Vec<QueryValue>` (карта внутренних
   алиасов каждой итерации, тот же формат, что уже отдаёт один саб-батч,
   `query_runner.rs:185-205`) → упакуй в `QueryResult.value =
   Some(QueryValue::List(iterations))` (ADR Decision 2).
5. Ошибка на итерации `i`: в транзакционном контексте — пробросить через
   `?` (абортит весь батч, ADR Decision 4); вне транзакции —
   stop-at-first (прервать цикл на первой ошибке, вернуть её, НЕ собирать
   частичные успехи — ADR Decision 4 явно выбрал этот вариант).
6. Авторизация/`distinct_repos`: тело `fe.batch` учитывается КАК ШАБЛОН
   (весь набор op тела) независимо от рантайм-числа итераций (ADR
   Decision 5) — проверь, что `is_write`/`distinct_repos`-логика уже
   корректно это делает через пункт 1 (`ForEach::is_write`), без
   дополнительных изменений.

## Тесты

Минимальные, доказывающие корректность (полное покрытие — Фаза D, #655,
не здесь):
- `ForEachOp` serde round-trip.
- 0/1/N итераций через `execute_batch`.
- `bind_row`-значение реально доступно внутри тела как `$param`.
- `max_iterations` превышен → ошибка до первой итерации.
- Ошибка на итерации `i` внутри tx → абортит весь батч.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-engine -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings` (ПОЛНЫЙ workspace
  — Фазы A/B предыдущих эпиков обе спотыкались на пропущенных полях в
  других крейтах при росте структур)
- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-query-builder`/`crates/shamir-client-ts` — это
  Фаза C (#654), не в scope здесь.
- НЕ реализуй `@loop[i].alias[j].field`-индексированную адресацию через
  уровни — ADR отложил это как открытый вопрос, не обязательный для этой
  фазы.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-query-types/src/batch/` (новый
  for_each_op.rs, batch_op.rs, batch_limits.rs, planner.rs, tests/),
  `crates/shamir-engine/src/query/batch/` (batch_execute.rs,
  query_runner.rs, tests/).
- fmt/clippy чисты — включая ПОЛНЫЙ workspace clippy.
- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full` зелёный.
