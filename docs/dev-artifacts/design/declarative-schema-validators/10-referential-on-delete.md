בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Phase D — Referential ON DELETE (reverse-FK / RESTRICT · CASCADE · SET NULL)

**Статус:** design / план (не реализовано).
**Предшественники:** Phase C2 (forward FK existence), Phase C3 (unique).
**Связанные задачи:** `docs/dev-artifacts/research/ACTION-ITEMS.md` A1 (FK/unique fail-open
под autocommit), A3 (guard на дропах/удалении), E6 (FK actions).

---

## 1. Мотивация

Сегодня FK — **forward-only**: `foreign_key { ref_table, ref_field }` на
*ссылающейся* таблице проверяет существование цели при записи ссылающейся
строки (Phase C2). Обратной стороны нет: при удалении строки в *целевой*
таблице движок не проверяет «кто на меня ссылается» → накапливаются тихие
сироты. Нужен **ON DELETE**-контракт: запретить (или каскадно обработать)
удаление строки, на которую есть живые ссылки.

## 2. Что УЖЕ есть (фундамент — не строить заново)

Проверено чтением кода:

- **Хук на DELETE существует и production-grade.** `execute_delete_tx`
  (`crates/shamir-engine/src/table/write_exec.rs:526-642`):
  - детектит привязки `WriteOp::Delete` *до* скана (стр. 539-542);
  - несёт **байты удаляемой записи** рядом с id (стр. 554-615) — старая запись
    доступна хуку без второго чтения стора;
  - гоняет `run_validators_view(WriteOp::Delete, old=Some(view), new=None,
    …, resolver)` (стр. 630-639); ошибка валидатора → `validator_failure_to_db_error`
    → **удаление отклоняется**.
- **`WriteOp::Delete`** — полноценный вариант (`shamir-query-types/src/validator/mod.rs:15`);
  `bind_validator { ops: [Delete] }` уже работает.
- **Cross-table доступ** прокинут: `execute_delete_tx` принимает
  `resolver: Option<&dyn TableResolver>` — валидатор может читать другие таблицы.

**Следствие:** уже сегодня reverse-FK выразим как *ручной WASM-валидатор* на
Delete. Phase D — это **декларативная обёртка** поверх существующего хука, а не
новый механизм исполнения.

## 3. Каверзный момент — resolver на delete-пути

Cross-table reverse-FK lookup работает **только когда `resolver` wired**. В
проде сервер оборачивает каждый батч в tx-контекст с `Some(self.resolver)`
(tx-mode ветка `query_runner.rs:494`) — поэтому forward-FK/unique уже enforced
и под autocommit (зелёные e2e `autocommit also enforces`; A1 в `ACTION-ITEMS.md`
был ложной тревогой от устаревших комментариев). НО **delete-путь** (`query_runner`
delete-арм) на implicit-ветке передаёт `resolver: None` (`:449/526`-паттерн).
**Phase D.1 обязана прокинуть resolver на delete-путь** — иначе reverse-FK
RESTRICT не увидит ссылок. Препятствие — HRTB-замыкание `run_implicit_batch_tx`
(future заимствует только `tx`), вероятно нужен Arc-shareable resolver. Это и
есть та «латентная непоследовательность», что осталась от A1.

## 4. Дизайн

### 4.1 DTO

`ForeignKeyRef` (в `shamir-query-types/src/admin/types/schema_ops.rs` /
engine-mirror `validator/schema/foreign_key.rs`) расширяется:

```
ForeignKeyRef {
    ref_table: String,
    ref_field: FieldPath,
    on_delete: FkAction,   // NEW, default = Restrict
    // on_update: FkAction, // опционально, фаза D.4
}

enum FkAction { NoAction, Restrict, Cascade, SetNull }
```

**Развилка по умолчанию (требует решения):** `Restrict` (безопасно, отклоняет
удаление при ссылках) vs `NoAction` (SQL-дефолт, не проверяет). Рекомендация —
**`Restrict`** как дефолт: «безопасно по умолчанию» согласуется с дисциплиной
проекта; `NoAction` оставить как явный opt-out.

### 4.2 Reverse-reference lookup

Чтобы на удалении строки в таблице T найти «кто ссылается на T», нужен обратный
обзор FK-деклараций. Два варианта:

- **(a) Скан схем репозитория** по `ref_table == T` — лениво, без нового
  состояния; стоимость O(таблиц) на delete, кэшируемо.
- **(b) Обратный индекс FK** в catalogue (`03-storage-catalogue.md`) — `T →
  [(referencing_table, referencing_field)]`, обновляемый при set_table_schema.
  Быстрее на hot-path, но новое состояние для поддержки.

Рекомендация — начать с **(a)** (кэш FK-карты в `ArcSwap`, инвалидируемый на
schema-change), мигрировать на (b) при нужде.

### 4.3 Engine — точка входа

В `execute_delete_tx`, в существующем delete-validator-проходе (или рядом, как
встроенная reverse-FK проверка перед `delete_tx`):

1. Для удаляемой строки взять значения `ref_field` (через `RecordView` — байты
   уже собраны).
2. Для каждой referencing-таблицы спросить через `resolver`:
   `exists_referencing(referencing_table, referencing_field, value)`.
3. По `FkAction`:
   - `Restrict` / `NoAction(strict)` → есть ссылка ⇒ `field_error … "fk_restrict"`
     ⇒ удаление отклонено;
   - `Cascade` → каскадно удалить referencing-строки (в той же tx, с guard
     рекурсии/глубины — переиспользовать sub-batch depth guard);
   - `SetNull` → обновить referencing-строки, обнулив `referencing_field`
     (требует nullable-поля; иначе DDL-ошибка времени bind).
4. **Resolver обязателен**: на autocommit delete-пути wire-ить resolver (или
   fail-closed «reverse-FK требует tx» — симметрично решению A1).

### 4.4 Builder

- Rust (`ddl/schema.rs::FieldBuilder`):
  `.foreign_key(table, field).on_delete(FkAction::Restrict)`.
- TS (`builders/ddl.ts`): `.foreignKey(table, field, { onDelete: 'restrict' })`.

## 5. Поэтапный план (TDD red→green→refactor)

### Фаза D.0 — DTO + builder (S)
- Добавить `FkAction` + `on_delete` в `ForeignKeyRef` (wire + engine mirror),
  serde-дефолт `Restrict`.
- Rust/TS builder сеттеры `.on_delete()` / `{onDelete}`.
- 🔴 serde round-trip тест на новый вариант; 🟢 минимальная сериализация.

### Фаза D.1 — RESTRICT (M) — основной кейс пользователя
- Reverse-FK lookup (вариант (a): FK-карта по `ref_table`, кэш в `ArcSwap`).
- В `execute_delete_tx` — проверка перед `delete_tx`; `Restrict` ⇒ `fk_restrict`
  ошибка ⇒ отклонение.
- Resolver wired на delete-пути (закрывает A1 для delete).
- 🔴 e2e: insert parent+child(FK) → delete parent → reject `fk_restrict`;
  delete child → delete parent OK. 🔴 regression: под autocommit reverse-FK
  НЕ молчит (fail-closed или enforced).

### Фаза D.2 — CASCADE + SET NULL (M)
- `Cascade`: каскадное удаление referencing-строк в той же tx, depth-guard.
- `SetNull`: update referencing-строк, обнуление поля; bind-time проверка
  nullable.
- 🔴 e2e на оба действия + цепочки (A→B→C cascade), цикл-guard.

### Фаза D.3 — drop-guard симметрия (S, закрывает A3)
- `DropTable` отказывает, если на таблицу есть живые FK-ссылки (если не
  `cascade`); `DropFunction` отказывает, если функция привязана как валидатор.
- 🔴 e2e: drop referenced table → reject; drop с cascade → OK.

### Фаза D.4 — (опц.) ON UPDATE (M)
- `on_update: FkAction` для изменения ключевого значения цели. Низкий приоритет
  (ключи обычно иммутабельны).

## 6. Открытые вопросы (нужно решение)

1. **Дефолт `on_delete`** — `Restrict` (рекомендую) vs `NoAction`?
2. **Autocommit reverse-FK** — wire resolver (тихо работает) vs fail-closed
   («reverse-FK требует tx»)? Должно совпасть с финальным решением A1 для
   forward-FK/unique, чтобы поведение было единообразным.
3. **Reverse-lookup** — скан схем (a) сразу, индекс (b) потом — ОК?
4. **Cascade-глубина/циклы** — переиспользовать sub-batch depth guard или
   отдельный лимит?

## 7. Связь с ACTION-ITEMS

Закрывает: **A3** (referential guard на дропах/удалении), **E6** (FK-actions),
и часть **A1** (resolver на delete-пути → reverse-FK не fail-open). Forward-FK/
unique fail-open на *write*-пути (ядро A1) остаётся отдельной задачей, но
решение по autocommit-resolver здесь должно быть с ней согласовано.
