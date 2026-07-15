# Brief: OQL Epic 03 / Phase B — движок: `when`, planner fix (#642), executor skip (task #645)

## Контекст

ADR: `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`
(прочитай ПОЛНОСТЬЮ перед началом — все 4 решения там зафиксированы с
обоснованием, эта задача их реализует буквально).

Ключевая находка ADR: баг #642 (планировщик не рекурсирует в
`FilterValue::Cond`/`Expr`/`FnCall` при извлечении `$query`-зависимостей)
ЗАТРАГИВАЕТ `when` напрямую — WHERE-фильтры и (будущий) `when` используют
ОДНУ функцию `extract_deps_from_filter_value`
(`crates/shamir-query-types/src/batch/planner.rs:343-358`). Эта задача
ЗАКРЫВАЕТ #642 КАК ЧАСТЬ своей реализации.

## Задача

### 1. Исправить `extract_deps_from_filter_value` (закрывает #642)

`crates/shamir-query-types/src/batch/planner.rs:343-358` — текущий код:

```rust
match value {
    FilterValue::Array(arr) => { /* recurse */ }
    FilterValue::QueryRef { alias, .. } => { /* record dep */ }
    _ => {}
}
```

Добавь рекурсию:
- `FilterValue::Cond { cond }` → рекурсивно вызови эту же функцию на
  `cond.condition` (обход `Filter`-дерева через `extract_deps_from_filter`,
  раз `condition: Box<Filter>`), `cond.then`, `cond.or_else`.
- `FilterValue::Expr { expr }` → рекурсивно на каждый элемент `expr.args`.
- `FilterValue::FnCall { call }` → рекурсивно на каждый аргумент
  `call.args()`.

Проверь точные имена полей/методов в `crates/shamir-query-types/src/filter/`
(`Cond`, `FilterExpr`, `FnCall`) — подгони под реальные сигнатуры.

### 2. `when: Option<Filter>` на `QueryEntry`

`crates/shamir-query-types/src/batch/query_entry.rs` — добавь поле рядом с
`after` (строка ~50): `pub when: Option<Filter>`, с
`#[serde(default, skip_serializing_if = "Option::is_none")]` (backward
compatible — отсутствие поля = сегодняшнее безусловное исполнение). Обнови
конструктор/`Default`, если есть.

### 3. `BatchPlanner` — `when`-рефы участвуют в DAG

`crates/shamir-query-types/src/batch/planner.rs::plan` — для каждого entry
с `when: Some(filter)`, извлеки зависимости из `filter` (переиспользуй
исправленную из пункта 1 `extract_deps_from_filter`) и добавь их в общий
deps-сет ТОЙ ЖЕ провенанс-логикой, что уже используется для `op`'s
собственных зависимостей (`EdgeKind::DataFlow`, если `when` содержит
`$query`-реф — по аналогии с тем, как `op`'s зависимости уже собираются).
DAG остаётся статическим — валидация `UnknownAlias`/циклов/лимитов по
ПОЛНОМУ множеству entry, независимо от рантайм-значения `when` (ADR
Decision 3).

### 4. `QueryResult` — `skipped: bool`

`crates/shamir-query-types/src/read/query_result.rs` — добавь поле
`#[serde(default, skip_serializing_if = "std::ops::Not::not")] pub skipped:
bool` (или похожий idiomatic способ пропустить сериализацию `false` —
посмотри, как это принято в крейте) на `QueryResult`. `Default`/конструкторы
— `skipped: false` по умолчанию.

### 5. Executor — эвалюация `when` + каскадный skip

`crates/shamir-engine/src/query/batch/batch_execute.rs`
(`execute_plan_impl`/`execute_plan_tx_impl`) и/или
`crates/shamir-engine/src/query/batch/query_runner.rs` (`QueryRunner::run`)
— перед исполнением `op` для alias с `when: Some(filter)`:

- Скомпилируй/вычисли `filter` через существующий `Filter::matches`/
  `compile_filter` (тот же путь, что WHERE) против `resolved_refs`
  текущего alias (те же `resolved_refs`, что уже строятся для `op`'s
  зависимостей — `when`'s зависимости из пункта 3 должны попасть в тот же
  `resolved_refs`, что и `op`'s собственные).
- Если `filter` отсутствует (`None`) — исполнение безусловное (сегодняшнее
  поведение, без изменений).
- Если `filter` вычисляется в `false` — НЕ исполняй `op`. Результат для
  этого alias — `QueryResult { skipped: true, records: vec![], ... }` (не
  клади его в `all_results` как обычный результат ИЛИ клади с
  `skipped: true` — реши, какой вариант проще интегрировать с каскадом
  ниже, но убедись, что зависимый `$query`-реф на skipped alias НЕ
  находит нормальных данных).
- **Каскад** (ADR Decision 2): если alias `B` зависит от skipped `A` через
  `EdgeKind::DataFlow`/`Both` — `B` тоже автоматически становится skipped
  (не исполняется, тот же `skipped: true` статус), БЕЗ ошибки. Через чисто
  `EdgeKind::Explicit` (`after`) — НЕ каскадирует (независимое исполнение,
  если у `B` нет собственного `when` или `DataFlow`-связи с `A`).
- Реализуй каскад ПРАВИЛЬНО ПО СТАДИЯМ (планировщик уже строит `stages:
  Vec<Vec<String>>` в топологическом порядке) — при обработке каждой
  стадии проверяй, есть ли среди зависимостей текущего alias (по
  `DataFlow`/`Both`-рёбрам) хоть один skipped — если да, текущий alias
  тоже skipped, БЕЗ вызова `when`-эвалюации (уже решено каскадом, не нужно
  проверять собственный `when`, если он есть — обоснуй в коде, что каскад
  главнее собственного `when`, или наоборот, если ADR подразумевает другое
  — перечитай ADR внимательно на этот счёт, если там нет explicit
  указания — прими решение и явно задокументируй его комментарием в коде).

### 6. Repo-скоуп/is_write/авторизация — пессимистичная модель (уже
верно сегодня)

Проверь, что `distinct_repos`/`is_write`-классификация/`begin_tx` УЖЕ
считают ВСЕ объявленные op независимо от `when` (поскольку это происходит
на этапе планирования, ДО рантайм-эвалюации `when` — просто по конструкции
кода, `when`-поле ничего не меняет в этой логике, раз она не смотрит на
рантайм-значения). Если найдёшь место, которое ошибочно пытается смотреть
на `when` при классификации (не должно быть, но проверь) — исправь, чтобы
классификация оставалась независимой от `when`.

## Тесты

Минимальные, доказывающие корректность (полное покрытие — Фаза D, #647,
не здесь):
- `extract_deps_from_filter_value` теперь извлекает `$query`-реф из
  вложенного `$cond`/`$expr`/`$fn` (регрессионный тест на баг #642).
- `when: Some(filter)` — оба исхода (true/false) через `execute_batch`.
- Каскадный skip: `B` зависит от `A` (skipped) через `$query` → `B` тоже
  skipped.
- `after`-only зависимость от skipped `A` НЕ каскадирует `B`.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-engine -- --check`
- `cargo clippy -p shamir-query-types -p shamir-engine --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-query-builder`/`crates/shamir-client-ts` — это
  отдельная задача #646 (Epic03/C), не в scope здесь.
- НЕ реализуй switch-case builder-сахар — это Фаза C.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-query-types/src/batch/` (planner.rs,
  query_entry.rs, tests/), `crates/shamir-query-types/src/read/query_result.rs`,
  `crates/shamir-engine/src/query/batch/` (batch_execute.rs,
  query_runner.rs, tests/).
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full` зелёный,
  включая тест, доказывающий фикс бага #642.
