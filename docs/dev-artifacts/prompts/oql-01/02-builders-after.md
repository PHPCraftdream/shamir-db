# Brief: OQL Epic 01 / Phase B — билдеры Rust+TS (task #629)

## Контекст

Роадмап: `docs/dev-artifacts/roadmap/oql/01-sequencing-explicitness.md`, Фаза B.
Фаза A (#628, коммит `b65b4940`) уже сделала:
- `crates/shamir-query-types/src/batch/alias.rs` — `pub fn extract_base_alias(s: &str) -> String`
  и `pub(crate) fn split_path_tail(s: &str) -> Option<(String, String)>` (детект мусорного
  path-хвоста в `after`, например `"mk[0].id"`).
- `BatchError::AfterPathIgnored { alias, raw }` — новый вариант ошибки.
- `EdgeKind` (Explicit/DataFlow/Both) — провенанс рёбер DAG.

## Задача

### 1. Rust builder — переиспользовать shared `extract_base_alias`

`crates/shamir-query-builder/src/batch/batch.rs:783-788` содержит СВОЮ
локальную копию `extract_base_alias` (дубликат логики из planner.rs, теперь
устаревший — есть общая версия в `shamir-query-types::batch::alias`). Замени
локальную функцию на импорт `shamir_query_types::batch::alias::extract_base_alias`
(проверь точный путь реэкспорта — возможно `shamir_query_types::batch::extract_base_alias`
если alias.rs реэкспортирован через `batch/mod.rs`; посмотри, как это сделано
в Фазе A). Убедись, что `shamir-query-builder` уже зависит от
`shamir-query-types` (должно быть — grep `Cargo.toml`).

Также `try_build()` (batch.rs:682-726) сейчас валидирует `after`-рефы БЕЗ
проверки мусорного path-хвоста — добавь ту же проверку, что появилась в
планировщике (Фаза A): если `after`-строка содержит path-хвост
(`split_path_tail` возвращает `Some`), верни `BuildError`-вариант,
аналогичный `BatchError::AfterPathIgnored` (добавь новый вариант в
`BuildError`, найди его определение рядом в `shamir-query-builder`).

### 2. Rust builder — fluent-вариант `after` на entry (не только пост-фактум)

Текущий `pub fn after(&mut self, dependent: &Handle, on: &Handle)`
(batch.rs:730-737) — отдельный метод, вызываемый ПОСЛЕ регистрации обоих
op'ов, оба аргумента одного типа `&Handle` (легко перепутать порядок,
компилятор не спасёт). Добавь fluent-альтернативу на самом `Handle`,
возвращаемом при регистрации entry — например метод
`Handle::after(&self, batch: &mut Batch, on: &Handle)` НЕ подходит (Handle
не должен держать `&mut Batch`). Вместо этого рассмотри паттерн, где `add`/
`query`/etc возвращают билдер-обёртку, позволяющую сразу вызвать
`.after(&on)` в месте регистрации — например через промежуточный тип
`EntryHandle<'a>` с `&'a mut Batch` внутри, либо через опциональный параметр
`after: &[&Handle]` прямо в сигнатуре регистрирующих методов (`query`,
`insert`, etc. — grep все публичные методы регистрации entry в `batch.rs`).
Выбери решение, минимизирующее breaking-диф по существующим вызовам (проект
не выпускался — ломать можно, но старайся, чтобы существующие тесты
адаптировались минимальными правками). Сохрани существующий
`pub fn after(&mut self, dependent: &Handle, on: &Handle)` — он не удаляется,
просто добавляется более локальный fluent-способ.

Обнови докстринг `after()` (строка 730-731) правилом одной фразой:
"`$query` создаёт ребро само по себе; `after` нужен только там, где нет
потока данных (например DDL→DML ordering)."

### 3. TS builder — паритет нормализации + build() валидация

`crates/shamir-client-ts/src/core/builders/batch.ts`:
- `tryBuild()` (строка ~242+) сравнивает `after`-строки БУКВАЛЬНО
  (`declared.has(dep)`, не нормализует `@`/path-хвост) — в отличие от
  сервера и Rust `try_build`, которые стрипают `@` и режут по `[`/`.`.
  Добавь ту же нормализацию (JS-порт логики `extract_base_alias`: срез
  leading `@`, срез по первому `[` или `.`) перед сравнением с `declared`.
  Если после нормализации остаётся path-хвост (сервер теперь ЭТО ОТВЕРГАЕТ
  как `AfterPathIgnored`, см. Фазу A) — TS `tryBuild()` тоже должен бросить
  `Error` с аналогичным сообщением, а не тихо принимать.
- Перенеси валидацию `$query`/`after`-ссылок из opt-in `tryBuild()` в
  основной `build()` (строка ~210+) — проект не выпускался, обратную
  совместимость не хранить. `tryBuild()` остаётся как алиас/обёртка над
  `build()` для существующих вызывающих мест (не удаляй метод — так меньше
  диф в вызывающем коде и тестах).

### 4. Коллизия имени `after` — batch vs keyset pagination

`crates/shamir-query-builder/src/query/query.rs:199` — keyset-pagination
`.after(key, limit)` на `Query`/`ReadQuery`, и `Batch.after(dep, on)` —
dependency ordering. Оцени: это два несвязанных смысла одного имени метода в
одном SDK (Query vs Batch — разные типы, так что компилятор их не путает, но
человек — может). Реши сам: либо оставить как есть с явным доккомментарием у
ОБОИХ методов, перекрёстно ссылающимся друг на друга ("не путать с
Batch::after" / "не путать с Query::after"), либо переименовать один из них
(например `Batch::after` → `Batch::run_after`/`Batch::depends_on` — по
своему усмотрению). Зафиксируй решение коротким комментарием в коде,
объясняющим выбор (не обязательно отдельный ADR-файл — двух строк
достаточно). То же самое проверь и реши для TS (`filter.ts`'s keyset `after`
vs `batch.ts`'s dependency `after`).

## Тесты

Минимальные тесты, доказывающие, что твой код работает (полное покрытие —
отдельная задача #630 Epic01/C, не в scope здесь):
- Rust: `try_build()` теперь ловит мусорный path в `after` (новый тест в
  `crates/shamir-query-builder/src/batch/tests/`).
- Rust: fluent-вариант `after` на entry работает эквивалентно
  пост-фактум-варианту (если добавил новый метод/параметр).
- TS: `tryBuild()` нормализует `@`/path-хвост в `after` так же, как сервер;
  мусорный path бросает `Error` (новый тест в
  `crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts`).
- TS: `build()` теперь тоже валидирует (не только `tryBuild()`).

## Прогон проверок

- `cargo fmt -p shamir-query-builder -- --check`
- `cargo clippy -p shamir-query-builder --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-builder --full`
- из `crates/shamir-client-ts`: `npx tsc --noEmit` и `npm test` (vitest;
  игнорируй e2e-тесты, падающие из-за предсуществующего "stale
  shamir-server binary" guard — это не связано с этой задачей).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-query-types`/`crates/shamir-engine` — это Фаза A
  (#628), уже сделана и закоммичена, не в scope здесь.
- НЕ реализуй ничего из Фазы C/D/E/F (юнит-тесты полного покрытия, e2e,
  бенчи, доки) — только билдеры + минимальные доказывающие тесты выше.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-query-builder/src/batch/` (+ possibly
  `query.rs`/`filter.ts` для комментария про коллизию имени `after`),
  `crates/shamir-client-ts/src/core/builders/batch.ts` (+ его тесты).
- fmt/clippy чисты (Rust), `tsc --noEmit` чист (TS).
- `./scripts/test.sh -p shamir-query-builder --full` зелёный;
  `npm test` (TS) unit-тесты зелёные (e2e stale-binary failures допустимы).
