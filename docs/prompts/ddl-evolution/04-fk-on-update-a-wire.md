בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ②.2a — FK ON UPDATE: wire + DTO + builders (additive surface)

Кампания **② DDL-эволюция**, этап ②.2 (FK ON UPDATE), под-этап **a — surface**.
Источник: `docs/research/DDL-EVOLUTION-PLAN.md` §②.2a. Объём: S-M. Риск низкий
(**чистый аддитивный mirror `on_delete`; БЕЗ enforcement** — enforcement = ②.2b,
не трогать). Пакеты: `shamir-query-types`, `shamir-engine`, `shamir-db`,
`shamir-query-builder`, `shamir-client-ts`.

## Задача (одна строка)
Добавить поле `on_update: FkAction` ВЕЗДЕ, где сейчас есть `on_delete` — тот же
serde-default-`NoAction` split — + билдеры `foreign_key_on_update` / TS
`onUpdate` + комбинированный конструктор. Переиспользовать существующий
`FkAction` enum. **Никакого enforcement-кода** (это ②.2b).

## Заземление — `FkAction` уже готов
`crates/shamir-query-types/src/admin/types/fk_action.rs`: enum
`FkAction{NoAction/Restrict/Cascade/SetNull}`, snake_case wire, serde-default
`NoAction`, `is_no_action()` для `skip_serializing_if`. Переиспользуй as-is —
он семантически нейтрален к delete/update.

## Точки для mirror (на каждую — добавь `on_update` рядом с `on_delete`)
1. **DTO** — `crates/shamir-query-types/src/admin/types/schema_ops.rs:102-116`
   (`ForeignKeyDto`). Рядом с
   `#[serde(default, skip_serializing_if = "FkAction::is_no_action")] pub
   on_delete: FkAction` добавь идентично
   `pub on_update: FkAction` (тот же атрибут — legacy-схемы без поля → NoAction,
   байты не меняются).
2. **ForeignKeyRef** — `crates/shamir-engine/src/validator/schema/foreign_key.rs`:
   - поле `pub on_update: FkAction` (рядом с `on_delete` :28).
   - в `new()` :41 — `on_update: FkAction::NoAction` (back-compat).
   - в `with_on_delete()` :46 — добавь `on_update: FkAction::NoAction` в литерал
     (этот конструктор задаёт только delete).
   - **новый** `with_on_update(ref_table, ref_field, on_update)` — зеркало
     `with_on_delete`, ставит `on_delete: NoAction`.
   - **новый** `with_actions(ref_table, ref_field, on_delete, on_update)` —
     задаёт оба (для FK с обоими действиями).
3. **Serialize fk-map** — `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:770-777`:
   рядом с веткой, кладущей `on_delete` в `fk_m` строкой, добавь симметричную для
   `on_update` (тот же `match fk.on_update { … }` + `fk_m.insert("on_update", …)`;
   NoAction → пропустить, как у on_delete).
4. **Deserialize fk-map** — ДВА места:
   - `admin_schema.rs:862-872` (`let on_delete = match m.get("on_delete")…` →
     добавь `on_update` и положи в собираемый DTO/ref).
   - `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:234-242`
     (`let on_delete = match map.get("on_delete")…` + `ForeignKeyRef::with_on_delete(
     ref_table, ref_field, on_delete)` → читай также `on_update` и собери через
     `ForeignKeyRef::with_actions(ref_table, ref_field, on_delete, on_update)`).
5. **Rust-билдер** — `crates/shamir-query-builder/src/ddl/schema.rs`:
   - струк-литералы `ForeignKeyDto { ref_table, ref_field, on_delete }` на :249
     (`foreign_key`) и :269 (`foreign_key_on_delete`) — добавь `on_update:
     FkAction::NoAction` (plain FK и delete-only FK сохраняют текущее поведение —
     **НЕ** ставь Restrict по умолчанию для on_update: это аддитивность, не менять
     поведение существующих FK-юзеров).
   - **новый** `foreign_key_on_update(ref_table, ref_field, on_update)` — зеркало
     `foreign_key_on_delete` :263, ставит `on_delete: FkAction::Restrict`
     (builder-safe default для delete), `on_update` из аргумента.
   - **новый** `foreign_key_with_actions(ref_table, ref_field, on_delete,
     on_update)` — оба явно.
6. **TS** — `crates/shamir-client-ts/src/core/types/ddl.ts:55-65`
   (`on_update?: FkAction` рядом с `on_delete?`) +
   `crates/shamir-client-ts/src/core/builders/ddl.ts:750-761`
   (`foreignKey(table, field, { onDelete, onUpdate })` — добавь `onUpdate?` в
   opts, `on_update: opts?.onUpdate ?? 'no_action'` — **NoAction** по умолчанию,
   аддитивно; onDelete остаётся `?? 'restrict'`).

## Тесты (обязательно)
- **serde round-trip** (Rust): FK с `on_update: Cascade` round-trips; legacy DTO
  без `on_update` → `NoAction`; FK с обоими (on_delete=SetNull, on_update=Cascade)
  round-trips. Положи рядом с существующими fk/on_delete serde-тестами (найди:
  `grep -rn "on_delete" crates --include=*.rs -l | grep -i test`).
- **builder** (Rust): `foreign_key_on_update(...)`, `foreign_key_with_actions(...)`
  дают корректный DTO. `foreign_key(...)` → on_update=NoAction (регрессия).
- **TS** wire-shape: `foreignKey('t','f',{onUpdate:'cascade'})` →
  `{ ..., on_update:'cascade', on_delete:'restrict' }`; без opts → on_update
  отсутствует/no_action.

## Гейт (прогони сам, всё зелёное — НЕ коммить)
- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-db
  -p shamir-query-builder` (+ `-- foreign_key` / `-- fk` для фокуса).
- `cargo fmt -p shamir-query-types -p shamir-engine -p shamir-db
  -p shamir-query-builder -- --check` + `cargo clippy --workspace --all-targets
  -- -D warnings`.
- TS: `cd crates/shamir-client-ts && npx vitest run ddl && npx tsc --noEmit`
  (не вноси НОВЫХ tsc-ошибок сверх 4 pre-existing).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или мутирующую
  git-команду (НЕ удаляй `run.log`/отслеживаемые; scratch-логи — в /tmp). Только
  редактируй; коммитит оркестратор.
- ⛔ НЕ реализуй enforcement (restrict/cascade/setnull на UPDATE-пути) — это ②.2b,
  отдельный этап. Здесь ТОЛЬКО surface (поле + serde + builders).
- Surgical, аддитивно. one-file-one-export; импорты в шапку. Билдер-only.
- Заверши финальным текстом: изменённые файлы (file:line) + вывод гейта.
