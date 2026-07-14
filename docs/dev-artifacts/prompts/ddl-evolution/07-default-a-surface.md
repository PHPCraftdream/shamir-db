בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.4b — E2 DEFAULT: surface (DTO + Constraints + builders + persist)

Кампания **② DDL-эволюция**, этап ②.4 (E2 DEFAULT), под-этап **b — surface**.
Источник: `docs/dev-artifacts/research/DDL-EVOLUTION-PLAN.md` §②.4 (читай блок «✅ РЕШЕНО (②.4a)»
ПЕРВЫМ). Объём: M. Риск низкий (**чистый аддитивный surface; БЕЗ stamp-enforcement**
— enforcement = ②.4c, не трогать). Пакеты: `shamir-query-types`, `shamir-engine`,
`shamir-db`, `shamir-query-builder`, `shamir-client-ts`.

## Задача (одна строка)
Добавить `default: Option<QueryValue>` (литерал-константа) в schema-constraints
ВЕЗДЕ — DTO + runtime Constraints + persist/load + оба билдера + TS — по образцу
существующего value-constraint `one_of`. **Никакого stamp-кода на write-пути**
(это ②.4c).

## Решение ②.4a (кратко)
Узкий литерал-DEFAULT: `default` — константный QueryValue, штампуется на INSERT
до валидации для отсутствующих полей (enforcement в ②.4c). Здесь — только
объявление поля сквозь все слои.

## Точки (mirror существующего `one_of: Option<Vec<QueryValue>>` — он тоже
## value-constraint, в отличие от bool `unique`/`required`)
1. **DTO** — `crates/shamir-query-types/src/admin/types/schema_ops.rs:48-96`
   (`ConstraintsDto`). Добавь рядом с `one_of`:
   `#[serde(default, skip_serializing_if = "Option::is_none")] pub default:
   Option<QueryValue>` (legacy-схемы без поля → None, аддитивно). ⚠ Имя поля
   `default` — это ключевое слово Rust только как идентификатор переменной; как
   имя поля структуры — ОК (используется через `self.default`). Если serde-rename
   нужен для wire-ключа `"default"` — он и так `default` (имя поля = wire-ключ).
2. **runtime Constraints** — `crates/shamir-engine/src/validator/schema/constraints.rs:34-92`.
   Добавь `pub default: Option<QueryValue>` рядом с `one_of` :54. Обнови
   `Default`-derive (Option → None автоматически, struct уже `#[derive(Default)]`?
   — сверь; если ручной Default, добавь поле).
3. **DTO→Constraints load** — `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:158`
   (литерал `Constraints { ... }`). Читай `default` из persisted map и положи в
   литерал (зеркаль, как читается `one_of` / `unique`).
4. **persist + DTO-десериализация** — `crates/shamir-db/src/shamir_db/execute/admin_schema.rs`
   (где констрейнты сериализуются в map для каталога И десериализуются в
   ConstraintsDto). Найди, как `one_of` кладётся/читается, зеркаль `default`
   (это QueryValue — клади как есть, не строкой).
5. **Engine-билдер** — `crates/shamir-engine/src/validator/schema/rule_builder.rs`
   (рядом с `one_of` :171). Добавь `pub fn default(mut self, value: QueryValue)
   -> Self { self.constraints.default = Some(value); self }`. ⚠ Метод назван
   `default` — конфликт с `Default::default`? Это inherent-метод на RuleBuilder,
   не trait — ОК, но добавь `#[allow(clippy::should_implement_trait)]` если
   clippy ругнётся (мы НЕ реализуем Default::default).
6. **Query-builder** — `crates/shamir-query-builder/src/ddl/schema.rs` (FieldBuilder,
   рядом с `one_of` если есть, или другими constraint-методами). Добавь
   `pub fn default(mut self, value: QueryValue) -> Self` ставящий
   `self.constraints.default = Some(value)`. Тот же `#[allow(...)]` при нужде.
7. **TS** — найди FieldRule/constraint тип и билдер (`grep -rn "one_of\|oneOf\|
   unique" crates/shamir-client-ts/src/core/{types,builders}` — где живут
   constraints). Добавь `default?: WireValue` (или подходящий value-тип) в тип +
   `.default(value)` / `default:` в билдере по фактическому образцу `oneOf`.

## Тесты (обязательно)
- **serde round-trip** (Rust): ConstraintsDto с `default: Some(Int(42))`
  round-trips; legacy без поля → None. Рядом с существующими constraints-serde
  тестами.
- **builder** (Rust, оба билдера): `rule(["x"]).int().default(Int(5))` и
  query-builder FieldBuilder дают Constraints/DTO с `default = Some(...)`.
- **TS** wire-shape: builder с default → `{ ..., default: <value> }`.
- ⚠ НЕ тестируй stamp-поведение (нет insert-штампа в этом этапе — это ②.4c).

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-db
  -p shamir-query-builder -- default` (+ `-- schema` / `-- constraint` если
  фокус шире).
- `cargo fmt -p <те 4 крейта> -- --check` + `cargo clippy --workspace
  --all-targets -- -D warnings`.
- TS: `cd crates/shamir-client-ts && npx vitest run ddl && npx tsc --noEmit`
  (не вноси НОВЫХ tsc-ошибок сверх 4 pre-existing).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или мутирующую
  git-команду (НЕ удаляй `run.log`/отслеживаемые; scratch-логи — в /tmp). НЕ коммить.
- ⛔ НЕ реализуй stamp/insert-default-enforcement — это ②.4c. Здесь ТОЛЬКО объявление
  поля сквозь слои + билдеры + serde.
- Surgical, аддитивно, mirror `one_of`. one-file-one-export; импорты в шапку.
  Билдер-only, без raw serde_json::Value (QueryValue — ок). Тесты только через
  `./scripts/test.sh`.
- Заверши финальным текстом: изменённые файлы (file:line) + вывод гейта.
