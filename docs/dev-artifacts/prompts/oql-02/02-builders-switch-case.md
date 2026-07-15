# Brief: OQL Epic 02 / Phase B — эргономика `$cond`/switch-case (task #636)

## Контекст

Роадмап: `docs/dev-artifacts/roadmap/oql/02-cond-value-evaluation.md`, Фаза B.
Фаза A (#635, `cdcfc0f3`) реализовала реальную эвалюацию `$cond`/`$expr` в
движке — `resolve_filter_query` теперь честно вычисляет ternary и
рекурсивно резолвит выбранную ветку. Билдеры сегодня дают только ПЛОСКИЙ
ternary:

- Rust: `crates/shamir-query-builder/src/val/cond.rs::cond(condition, then,
  or_else) -> FilterValue`.
- TS: `crates/shamir-client-ts/src/core/builders/filter.ts::cond(ifFilter,
  then, orElse) -> { $cond: CondValue }`.

Switch-case (несколько веток vip/regular/newbie из докстрингов) сегодня
достижим только через РУЧНУЮ вложенность:
`cond(c1, v1, cond(c2, v2, cond(c3, v3, default)))` — читаемо для 2-3
уровней, но многословно и легко ошибиться со скобками для 4+ веток.

## Задача

### 1. Rust — `switch`-хелпер поверх `cond`

Добавь в `crates/shamir-query-builder/src/val/cond.rs` (или новый файл
рядом, если один-файл-один-экспорт того требует — реши по месту) функцию,
разворачивающую список `(condition, value)`-пар плюс дефолт в цепочку
вложенных `cond()`:

```rust
pub fn switch_case(
    cases: Vec<(Filter, FilterValue)>,
    default: impl Into<FilterValue>,
) -> FilterValue {
    // свернуть cases в цепочку cond(...) справа налево, default — база
}
```

(Сигнатура — предложение, адаптируй под идиомы крейта — например
`impl IntoIterator<Item = (Filter, FilterValue)>` вместо `Vec`, или
builder-паттерн `Switch::new().case(f1, v1).case(f2, v2).default(v3)`, если
он естественнее ложится на остальной стиль билдера в этом файле/крейте —
посмотри на соседние билдеры для консистентности стиля перед выбором).
Добавь докстринг с примером (используй тот же vip/regular/newbie сценарий,
что уже описан в `cond()`'s докстринге, чтобы показать преемственность).

### 2. TS — паритетный хелпер

`crates/shamir-client-ts/src/core/builders/filter.ts` — добавь эквивалент
(`switchCase(cases: [Filter, FilterValue][], default: FilterValue)` или
похожую сигнатуру, паритетную с Rust-версией по семантике, не обязательно
идентичную по синтаксису — TS и Rust билдеры и так не идентичны по стилю
везде в этом крейте).

### 3. `$cond` в write-значениях (SET/computed)

Проверь: работает ли `cond()`/новый `switch_case()`/TS `cond()` уже сегодня
как значение в `write::update`/`write::upsert`'s SET-клаузе (не только в
`WHERE`)? Пройди по типам — `FilterValue` используется и в фильтрах, и в
write-значениях (`crates/shamir-query-types/src/write/` — grep, как
`InsertOp`/`UpdateOp` типизируют значения полей). Если типы это уже
позволяют (скорее всего да, раз `FilterValue` — общий тип), просто
подтверди тестом (Фаза C, не здесь — но один минимальный тест здесь для
собственной уверенности допустим). Если ЕСТЬ преграда (например
write-значения типизированы уже как что-то более узкое, не принимающее
`Cond`) — опиши это явно в отчёте, НЕ делай масштабный рефакторинг типов
write-значений в рамках этой задачи — это будет отдельным находкой.

## Тесты

Минимальные, доказывающие работоспособность (полное покрытие — Фаза C,
#637, не здесь):
- Rust: `switch_case` с 3 ветками (vip/regular/newbie) даёт эквивалентный
  результат ручной вложенности `cond(cond(cond(...)))`.
- TS: `switchCase` — то же самое, сериализация в ожидаемый вложенный
  `$cond`-wire-формат.

## Прогон проверок

- `cargo fmt -p shamir-query-builder -- --check`
- `cargo clippy -p shamir-query-builder --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-builder --full`
- из `crates/shamir-client-ts`: `npx tsc --noEmit` и `npm test` (unit-часть,
  игнорируй предсуществующие e2e-фейлы из `docs/dev-artifacts/prompts/oql-01/`
  контекста — stale binary guard / принципал-баг #634, не в scope).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `crates/shamir-engine` (движок) — Фаза A уже сделана, не в scope.
- НЕ делай масштабный рефакторинг типов write-значений, если обнаружишь
  преграду в пункте 3 — только опиши находку.

## Проверка (сделает оркестратор)

- Диф ограничен `crates/shamir-query-builder/src/val/` (+ tests),
  `crates/shamir-client-ts/src/core/builders/filter.ts` (+ tests).
- fmt/clippy чисты (Rust), `tsc --noEmit` чист (TS).
- `./scripts/test.sh -p shamir-query-builder --full` зелёный; TS unit-тесты
  зелёные.
