# Brief: OQL Epic 01 / Phase A — edge provenance, after semantics, parallelism ADR, normalization (task #628)

## Контекст

Роадмап-план: `docs/dev-artifacts/roadmap/oql/01-sequencing-explicitness.md`.
Ресёрч-основание: `docs/dev-artifacts/research/oql/02-sequencing-dependencies.md`.

`BatchPlanner::plan` (`crates/shamir-query-types/src/batch/planner.rs:89-160`)
строит ОДИН dependency-граф из двух сливающихся источников: авто-извлечённые
`$query`-рефы (`extract_dependencies`, строки 118, 213-224, 309-312) и явный
`entry.after` (строки 120-124). После merge `BatchPlan.dependencies`
(`batch_plan.rs:31`, `TMap<String, TSet<String>>`) не различает, какое ребро
откуда взялось. `build_resolved_refs` в
`crates/shamir-engine/src/query/batch/batch_execute.rs:293-295` передаёт
зависимому op'у результаты ВСЕХ deps — включая чисто-ordering `after`-рёбра,
что даёт `after` побочный эффект открытия чужих данных, хотя семантика
заявлена как "просто ordering, не доступ к данным".

Проект НЕ выпускался — обратную совместимость НЕ храним, меняем поведение
свободно.

## Задача

### 1. Провенанс рёбер в `BatchPlan`

Замени/расширь `dependencies: TMap<String, TSet<String>>` так, чтобы для
каждого ребра было известно происхождение: `Explicit` (из `after`) или
`DataFlow` (из `$query`). Одно ребро МОЖЕТ быть отмечено обоими сразу (если
`after` совпадает с существующим `$query`-рефом на тот же alias — не ошибка,
просто дедуп с сохранением обоих флагов). Предложение формы (не обязательно
именно так, но сохрани семантику): либо
`TMap<String, TSet<String>>` остаётся для обратной совместимости API
планировщика внутри крейта, а рядом добавляется
`edge_provenance: TMap<String, TMap<String, EdgeKind>>` (alias →
dep_alias → kind), либо меняешь сам тип `dependencies`. Выбери вариант,
минимизирующий диф в остальных потребителях `BatchPlan` (grep все места,
где `plan.dependencies` читается — они не должны массово ломаться, но раз
обратной совместимости нет, меняй сигнатуру, если это чище).

`EdgeKind` — публичный enum (`Explicit`, `DataFlow`, либо
`bitflags`-подобный вариант, если ребро из обоих источников сразу).

### 2. Ошибки и `execution_plan` показывают источник

`BatchError::CircularDependency { cycle }` (найди определение — вероятно
`crates/shamir-query-types/src/batch/batch_error.rs` или рядом) — добавь в
диагностику (сообщение или структурированное поле) какой тип ребра образовал
цикл, если это осмысленно улучшает читаемость ошибки. `execution_plan` в
`BatchResponse` (найди, как `plan.stages` сериализуется в ответ — grep
`execution_plan` в `shamir-engine`/`shamir-query-types`) — добавь провенанс
рёбер в тот же сериализованный вид, чтобы клиент видел, какие рёбра явные, а
какие data-flow.

### 3. `after` перестаёт открывать данные

`crates/shamir-engine/src/query/batch/batch_execute.rs:293-295` (или рядом —
найди `build_resolved_refs` / аналог, комментарий "Each query's FilterContext
gets only the resolved_refs from its declared dependencies") — измени так,
чтобы `resolved_refs`, передаваемые зависимому op'у, строились ТОЛЬКО из
`DataFlow`-рёбер (используя провенанс из пункта 1), а не из полного deps-сета.
Если `after`-alias совпадает с существующим `$query`-рефом (edge и Explicit,
и DataFlow) — доступ к данным остаётся (потому что есть настоящий
`$query`-реф), просто чистый ordering-only `after` его больше не даёт.

### 4. ADR по параллелизму стадий — МИНИМУМ (b), (a) опционально

Реши документально (доки planner.rs обещают параллелизм стадий — строки
1-4, 38-43 doc-комментария модуля — а executor
(`crates/shamir-engine/src/query/batch/batch_execute.rs`, комментарий
~строки 254-269) честно гоняет всё последовательно). Минимальное требование
этой подзадачи — **(b): исправить doc-комментарий модуля `planner.rs`**,
чтобы он не обещал параллелизм, которого нет — переформулировать в духе
"stages are a LOGICAL grouping of independent queries; the executor may run
them sequentially or in parallel depending on implementation" со ссылкой на
`batch_execute.rs`'s комментарий про `try_join_all`-эксперимент. НЕ пытайся
реализовать реальный `tokio::spawn`-per-query параллелизм в этой подзадаче —
это опционально и отложено до бенчей Фазы E (задача #632), вне scope сейчас.

Зафиксируй это решение (минимум (b), (a) отложено) как короткий ADR-файл
`docs/dev-artifacts/design/oql-01-stage-parallelism-adr.md` (2-3 абзаца:
контекст, решение, обоснование "почему не сейчас").

### 5. Нормализация `after`-строк — единая функция

`extract_base_alias` (planner.rs:325-329, стрипает `@`, срезает по `[`/`.`)
уже приватная в planner.rs и используется 3 раза локально. Сделай её
`pub(crate)` или вынеси в отдельный публичный модуль внутри
`shamir-query-types` (например `crates/shamir-query-types/src/batch/alias.rs`
или похожее — выбери по месту, минимизируя диф), чтобы её могли
переиспользовать: (а) Rust query-builder (`crates/shamir-query-builder`,
задача #629, НЕ в этой подзадаче — просто убедись, что функция экспортируется
из `shamir-query-types` так, чтобы `shamir-query-builder` (который зависит от
`shamir-query-types`) мог её импортировать), (б) валидация мусорного path в
`after` — если `after`-строка содержит `[`/`.` (path-хвост, например
`"mk[0].id"`), это сейчас тихо стрипается до базового alias. Добавь явное
предупреждение или ошибку (реши сам, что уместнее — если мусорный path это
всегда developer-ошибка типа "я думал after ссылается на конкретное значение
такое-то", то это скорее ошибка на этапе planning, а не warning; обоснуй свой
выбор в комментарии кода). Обнови `BatchError` новым вариантом, если выбрана
ошибка (например `AfterPathIgnored { alias: String, raw: String }` или
подобное имя — назови по своему усмотрению).

## Тесты

Существующие тесты в `crates/shamir-query-types/src/batch/tests/planner_tests.rs`
НЕ должны сломаться семантически (если меняешь публичный тип
`BatchPlan.dependencies`, обнови все существующие тесты, которые к нему
обращаются). Добавь новые тесты (минимум):
- edge provenance правильно проставлен для чистого `after`, чистого
  `$query`, и смеси (`after` + `$query` на один и тот же alias).
- `after`-only зависимость НЕ передаёт resolved_refs зависимому op (тест на
  уровне `crates/shamir-engine/src/query/batch/tests/`).
- Мусорный path в `after` даёт задокументированное поведение (ошибка или
  warning, что бы ты ни выбрал).

(Полное покрытие юнит-тестами — отдельная задача #630 Epic01/C; здесь
достаточно тестов, доказывающих, что твой код в этой фазе работает — не
обязательно исчерпывающих регрессионных наборов, те придут в #630.)

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-engine -- --check`
- `cargo clippy -p shamir-query-types -p shamir-engine --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-query-builder` и `crates/shamir-client-ts` — это
  отдельная задача #629 (Epic01/B), не в scope этой подзадачи.
- НЕ реализуй настоящий параллелизм стадий (tokio::spawn-per-query) — только
  ADR + doc-фикс (минимум (b)).
- НЕ переименовывай `after` — это отдельное решение для Фазы B/ADR, не здесь.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-query-types/src/batch/` (planner.rs,
  batch_plan.rs, batch_error.rs, возможно новый alias-модуль, tests/),
  `crates/shamir-engine/src/query/batch/` (batch_execute.rs, tests/),
  плюс новый файл `docs/dev-artifacts/design/oql-01-stage-parallelism-adr.md`.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-query-types -p shamir-engine --full` зелёный.
