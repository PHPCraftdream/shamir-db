# Brief: OQL Epic 02 / Phase A — эвалюация `$cond`/`$expr` (task #635)

## Контекст

Роадмап: `docs/dev-artifacts/roadmap/oql/02-cond-value-evaluation.md`.
Ресёрч-основание: `docs/dev-artifacts/research/oql/04-conditionals-feasibility.md` §1.

`FilterValue::Cond` (структура `Cond` в
`crates/shamir-query-types/src/filter/cond.rs`: `condition: Box<Filter>`,
`then: FilterValue`, `or_else: FilterValue`) объявлен в wire-типах,
Rust-билдере (`crates/shamir-query-builder/src/val/cond.rs`), TS-билдере
(`crates/shamir-client-ts/src/core/builders/filter.ts::cond()`), и парсере
движка (`crates/shamir-engine/src/query/common/parser.rs:345,436`,
функция `cond_from_value`). НО центральный резолвер значений
`resolve_filter_query`
(`crates/shamir-engine/src/query/filter/resolve.rs:158-203`) обрабатывает
только `Null/Bool/Int/Float/String/Binary/FieldRef/QueryRef/FnCall/Param` —
`Cond` и `Expr` попадают в финальный `_ => None` (строка 201). Любой
`$cond`/`$expr` в проде сегодня молча резолвится в "значения нет".

Проект НЕ выпускался — обратную совместимость НЕ храним.

## Задача

### 1. Эвалюация `Cond` в `resolve_filter_query`

Добавь match-arm ПЕРЕД финальным `_ => None` (строка 201):

```rust
FilterValue::Cond(cond) => {
    if cond.condition.matches(record, ctx) {
        resolve_filter_query(&cond.then, record, ctx)
    } else {
        resolve_filter_query(&cond.or_else, record, ctx)
    }
}
```

(Проверь точное имя варианта — возможно `FilterValue::Cond(Box<Cond>)` или
структурный вариант с именованными полями `{ condition, then, or_else }` —
посмотри реальное определение в `cond.rs`/`filter_value.rs` и подгони
паттерн-матчинг. `Filter::matches` — уже существующий метод
`crates/shamir-engine/src/query/filter/filter_node.rs:228`, принимает
`record: &(impl RecordRef + ?Sized)`, `ctx: &FilterContext` — те же
параметры, что уже есть в `resolve_filter_query`, так что вызов
тривиален.) Рекурсия: выбранная ветка (`then`/`or_else`) сама рекурсивно
резолвится через `resolve_filter_query` — она может быть литералом,
`$query`, `$fn`, вложенным `$cond` и т.д. Тестами (Фаза C, не здесь)
будет проверена глубина ≥3 уровней.

### 2. Эвалюация `Expr` — или явный ADR на удаление

Найди определение `FilterValue::Expr` (grep в
`crates/shamir-query-types/src/filter/`). Изучи, что он делает семантически —
если он ДУБЛИРУЕТ `FilterValue::FnCall` (та же роль — вычислить скалярное
выражение через funclib), рассмотри вариант "удалить `Expr` полностью"
вместо того чтобы реализовывать вторую параллельную систему эвалюации.
Прими решение и задокументируй в новом файле
`docs/dev-artifacts/design/oql-02-expr-fate-adr.md` (2-4 абзаца: что такое
`Expr`, чем отличается/не отличается от `FnCall`, решение — реализовать
эвалюацию по аналогии с `Cond`, ИЛИ удалить вариант и все связанные
типы/парсер-код/билдер-API). Если решаешь реализовать — сделай это тем же
паттерном, что `Cond` (рекурсивный вызов `resolve_filter_query` на
под-выражения). Если решаешь удалить — убери `Expr` из
`FilterValue`/парсера/билдеров Rust+TS (это расширяет диф за пределы
`resolve.rs`, но обоснованно, если это мёртвый дублирующий код).

### 3. Снять костыли "collapse to Null"

- `crates/shamir-db/src/shamir_db/execute/helpers.rs:211` — комментарий
  "$cond collapse to Null here; not meaningful as Call params" — проверь
  актуальность этого комментария после твоей эвалюации в `resolve.rs`
  (если это отдельный, независимый путь резолва для Call-параметров, не
  использующий `resolve_filter_query` — обнови его тоже на реальную
  эвалюацию; если он использует `resolve_filter_query` косвенно — просто
  убери устаревший комментарий).
- `crates/shamir-engine/src/validator/schema/schema_validator.rs:355` —
  аналогично, проверь и обнови/убери устаревшее "expression-варианты не
  литералы" рассуждение, если оно теперь неверно.
- `crates/shamir-engine/src/tx/predicate_range.rs:55` — **НЕ трогай**, этот
  код КОРРЕКТНО остаётся консервативным (динамическое значение `$cond` не
  даёт статических байтов для predicate range — это архитектурно верно,
  не костыль).

### 4. Ошибка вместо silent-None для невычислимого условия

Если `cond.condition` внутри себя ссылается на `$query`-alias, которого нет
в `ctx.resolved_refs` (незадекларированная зависимость) — сегодня
`Filter::matches` скорее всего просто трактует отсутствующее значение как
`false` (сравнение с `None` = не совпадает). Реши: оставить эту silent-miss
семантику (согласуется с существующим поведением `$param`-silent-miss,
описанным в `docs/dev-artifacts/research/oql/01-nested-batch-recursion.md`)
ИЛИ явно документировать, что `$cond` наследует то же поведение, что и
любой другой `FilterValue` в фильтре — сравнение с отсутствующим значением
уже даёт `false` по всей кодовой базе, значит `$cond`'s condition ничем не
отличается. Скорее всего здесь ничего менять не нужно (унаследованное
поведение уже консистентно) — просто добавь однострочный doc-комментарий
на `resolve_filter_query`'s новый `Cond`-arm, разъясняющий это явно, вместо
изобретения нового поведения.

## Тесты

Минимальные тесты, доказывающие, что твой код работает (полное покрытие —
отдельная задача #637 Epic02/C, не в scope здесь):
- `$cond` в WHERE фильтре — обе ветки (true/false condition).
- Вложенный `$cond` (2 уровня) — `then`/`or_else` сами являются `$cond`.
- Если реализовал `Expr` — аналогичный минимальный тест; если удалил —
  подтверди, что удаление компилируется и существующие тесты не сломаны.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-engine -p shamir-db -- --check`
- `cargo clippy -p shamir-query-types -p shamir-engine -p shamir-db --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-db --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-query-builder`/`crates/shamir-client-ts` — это
  отдельная задача #636 (Epic02/B), не в scope здесь (если только удаление
  `Expr` не требует правки билдеров — тогда минимально необходимая правка
  допустима, но не расширяй её сверх удаления мёртвого API).
- НЕ трогай `predicate_range.rs:55` — уже корректен, см. пункт 3.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-engine/src/query/filter/resolve.rs`,
  возможно `crates/shamir-query-types/src/filter/` (если `Expr` удалён),
  `crates/shamir-db/src/shamir_db/execute/helpers.rs`,
  `crates/shamir-engine/src/validator/schema/schema_validator.rs`, плюс
  новый файл `docs/dev-artifacts/design/oql-02-expr-fate-adr.md`.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-db --full` зелёный.
