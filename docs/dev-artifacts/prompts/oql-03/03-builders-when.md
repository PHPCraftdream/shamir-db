# Brief: OQL Epic 03 / Phase C — билдеры Rust+TS для `when`/switch (task #646)

## Контекст

Фаза B (#645, `997532cc`) реализовала `QueryEntry.when: Option<Filter>` и
каскадный skip в движке. Эта задача — билдерская эргономика.

## Задача

### 1. Rust (`crates/shamir-query-builder/src/batch/batch.rs`)

По образцу существующих `*_after`-методов (Epic01/B, `query_after`,
`insert_after`, etc. — та же файл) добавь `.when(filter)`-метод на
регистрирующие методы (`query_if`/параметр `when: Option<Filter>` в уже
существующих `*_after`-подобных сигнатурах, или отдельный fluent-метод,
устанавливающий `when` на уже зарегистрированный `Handle` — выбери подход,
консистентный со стилем `after` из Epic01/B, посмотри на тот код перед
выбором).

Добавь switch-хелпер: `b.switch(value_handle: &Handle)` (или похожая
сигнатура) `.case(condition_value, op)...default(op)` — генерирует N entry
с комплементарными `when`-фильтрами (`case1`, `AND(NOT case1, case2)`, ...,
`default`), по аналогии с тем, как это уже описано в ADR
(`docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`, Decision
1) — прочитай ADR перед реализацией.

### 2. TS (`crates/shamir-client-ts/src/core/builders/batch.ts`)

Паритетный API: `add(alias, op, { when: Filter })` (аналогично `{ after:
string[] }` из Epic01/B) + `switchCase(...)`-хелпер, генерирующий группу
`add`-вызовов с комплементарными `when`.

### 3. `build()`/`tryBuild()` — валидация `when`

`when`-фильтр может содержать `$query`-рефы — они должны проверяться той же
логикой, что уже проверяет `$query`/`after` в `try_build()`/`build()`
(Epic01/B) — незадекларированный alias внутри `when` должен давать ту же
ошибку `UnknownAlias`, что и везде.

## Тесты

Минимальные, доказывающие работоспособность (полное покрытие — Фаза D,
#647, не здесь):
- Rust: `when` на entry — сериализация в ожидаемый wire-формат.
- Rust: `switch`-хелпер генерирует корректные комплементарные `when`.
- TS: паритет.

## Прогон проверок

- `cargo fmt -p shamir-query-builder -- --check`
- `cargo clippy -p shamir-query-builder --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-builder --full`
- из `crates/shamir-client-ts`: `npx tsc --noEmit` и
  `npx vitest run src/core/builders`.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-query-types`/`crates/shamir-engine` — Фаза B
  уже сделана.
- ПОСЛЕ любого изменения — проверь `cargo clippy --workspace --all-targets
  -- -D warnings` (не только затронутые крейты!) — Фазы A и B этого эпика
  обе споткнулись на этом (пропущенные поля в конструкторах QueryResult в
  других крейтах). Если найдёшь такое — почини мелкие мехнические сайты
  (добавление недостающего поля в существующий литерал структуры), это
  разрешено и ожидается.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-query-builder/src/batch/` (+ tests),
  `crates/shamir-client-ts/src/core/builders/batch.ts` (+ tests).
- fmt/clippy чисты — включая `cargo clippy --workspace --all-targets -- -D
  warnings` (не только затронутые крейты).
- `./scripts/test.sh -p shamir-query-builder --full` зелёный; TS unit-тесты
  зелёные.
