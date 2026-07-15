# OQL Roadmap 02 — `$cond`/`$expr`: доэвалюировать мёртвый примитив значений

Основание: `docs/dev-artifacts/research/oql/04-conditionals-feasibility.md` §1.
`$cond` (if/then/else над значением) объявлен ВЕЗДЕ — wire-типы
(`FilterValue::Cond`, `filter/cond.rs`), Rust-билдер (`val/cond.rs`),
TS-билдер (`filter.ts::cond()`), парсер движка (`parser.rs:345,436`) — но
центральный резолвер `resolve_filter_query` (`crates/shamir-engine/src/query/filter/resolve.rs`)
роняет его в `_ => None`. Любой `$cond` в проде молча даёт «значения нет».

Самый дешёвый план из четырёх. Зависимости: нет. Блокирует: 03 (условное
исполнение переиспользует эвалюацию условия).

---

## Фаза A — OQL-движок

1. **Эвалюация `Cond` в `resolve_filter_query`**: рекурсивно вычислить
   `condition: Box<Filter>` существующим filter-eval'ом против текущего
   контекста (row + resolved_refs + params), выбрать `then`/`or_else`,
   рекурсивно зарезолвить выбранную ветку (ветки сами могут быть `$cond`/
   `$fn`/`$query`/`$param`). Глубина уже ограничена лимитом filter-деревьев.
2. **Эвалюация `Expr`** — тем же паттерном (аудит `$expr`-варианта: если он
   дублирует `$fn`, рассмотреть удаление вместо реализации — решить в ADR;
   релизов не было, удалять можно свободно).
3. **Пройти по местам, где `$cond` сейчас «collapse to Null»** и снять
   костыли: `crates/shamir-db/src/shamir_db/execute/helpers.rs:211` (Call
   params), `schema_validator.rs:355`. `predicate_range.rs:55` оставить
   консервативным (динамическое значение не даёт статических байтов — это
   корректно).
4. **Ясная ошибка вместо silent-None** для невычислимого условия (например,
   `$query` внутри condition на незадекларированный dep) — согласовать с
   silent-miss семантикой `$param` (отчёт 01) и зафиксировать выбор в доке.

## Фаза B — Query builders

**Rust** (`crates/shamir-query-builder/src/val/cond.rs`): API уже есть —
проверить эргономику вложенного `$cond` (switch-case цепочка
vip/regular/newbie из докстрингов `cond.rs`), добавить хелпер-цепочку
`when(...).then(...).otherwise(...)` если текущая сборка вложенности
громоздка. **TS** (`filter.ts::cond()`): паритет с Rust, включая
использование в write-значениях (SET/computed).

## Фаза C — Юнит-тесты

- resolve: `$cond` в WHERE (обе ветки), в SET-значении Update, в
  Insert-values, в аргументах `$fn`, вложенный `$cond` (3 уровня,
  switch-case паттерн), `$query`/`$param`-ветки, невычислимое условие.
- Регрессия: снятые костыли (helpers.rs, schema_validator) — `$cond` в Call
  params и при schema-валидации даёт вычисленное значение, не Null.
- Билдеры: Rust `cond_tests.rs` расширить рантайм-кейсами; TS
  `__tests__/filter.test.ts` — сериализация + новые хелперы.

## Фаза D — E2E

- **Rust e2e**: батч с Update, где SET-значение — `$cond` над `$query`-рефом
  предыдущего запроса; транзакционный вариант.
- **TS e2e** (`src/__tests__/`): тот же сценарий через TS-билдер; проверка,
  что vip/regular/newbie switch-case из доки работает end-to-end.

## Фаза E — Бенчмарки

- Микробенч resolve: фильтр с `$cond` vs эквивалентный плоский фильтр —
  подтвердить, что эвалюация не добавляет аллокаций в per-row горячем цикле
  (изолированный `CARGO_TARGET_DIR`, `shamir_bench_utils::tune`).

## Фаза F — Доки

- `docs/guide-docs/`: раздел «Условные значения (`$cond`)» с примерами
  Rust+TS билдеров, включая switch-case через вложенность.
- Протокол-спека (`docs/guide-docs/client-server-protocol-spec/`): уточнить
  семантику невычислимого условия.
- ADR: судьба `$expr` (реализовать или удалить).

## Критерии готовности

- `$cond` вычисляется во всех точках, где `FilterValue` резолвится; ни одного
  пути «молча Null» не осталось (кроме задокументированного predicate_range).
- Гейт: fmt/clippy/`./scripts/test.sh --full` + `npm test` зелёные.
