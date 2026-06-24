בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Done — что уже реализовано по `ACTION-ITEMS.md`

Журнал выполненной работы по итогам исследований (`docs/research/`) +
`REVIEW.md`/`META-REVIEW.md`. Это «обратная сторона» `ACTION-ITEMS.md`: там —
план, здесь — факт. Каждый пункт: ссылка на action-item, краткая суть,
коммиты, статус верификации.

Легенда статуса: ✅ done & verified · ⏳ done, коммит ожидает явной просьбы.

> Дисциплина сессии: коммит/пуш — только по явной просьбе пользователя;
> делегированные брифы — в `docs/prompts/` под git (prompt-first); тесты — через
> `./scripts/test.sh` (nextest), бенчи — в изолированном `CARGO_TARGET_DIR`.

---

## Сводка одной строкой

Из «если делать ровно три вещи» (`ACTION-ITEMS.md` низ) сделаны **две**: **D1
keyset** и **Phase D (reverse-FK ON DELETE)** целиком. Плюс дешёвые билдер-дыры
**B1/B3**, корректность-блок **A1** (снят как ложная тревога после фактчека) и
параллельно закрыт **E6** (FK-actions). Остался из тройки только **A2**
(access-дефолты, P0).

---

## A. Корректность и безопасность

### A1 — FK/unique fail-open под autocommit → ✅ снято как ЛОЖНАЯ ТРЕВОГА
- **Action-item:** A1 (был P0).
- **Итог:** бага в проде нет. `execute_insert_tx` всегда передаёт `Some(tx)`;
  сервер оборачивает каждый батч в tx → FK/unique enforced под autocommit.
  Доказано зелёными e2e `autocommit also enforces FK/unique`.
- **Что сделано:** исправлены устаревшие комментарии в engine, из которых
  родилась ложная тревога; A1 снят с ранга P0 в research-доках.
- **Коммиты:** `0d3fd13` (комментарии engine) · `f27283b` (ретракция в доках).
- **Статус:** ✅ verified.

### A3 — referential-guard на дропах → ✅ покрыто частью Phase D.3
- **Action-item:** A3 (DropTable не отказывал при чужом FK).
- **Итог:** `DropTable` теперь отказывает (`drop_refused_fk`), если таблица
  ещё под чьим-то FK. См. Phase D.3 ниже.
- **Статус:** ✅ verified (DropTable-часть). DropFunction-as-validator — отдельно.

---

## B. Полнота билдеров

### B1 — Rust `Batch`: `result_encoding` / `interner_epochs` → ✅
- **Action-item:** B1 (S, «самый дешёвый перф-relevant»).
- **Сделано:** chainable-сеттеры на `Batch` (v2 id-keyed pass-through доступен из
  билдера).
- **Статус:** ✅ verified (task #237).

### B3 — Rust `val::expr` (`$expr`) / `val::cond` (`$cond`) → ✅
- **Action-item:** B3 (M, 18 операторов).
- **Сделано:** конструкторы `val::expr(op,args)` / `val::cond(if,then,else)` +
  удобные обёртки — паритет с TS `filter.expr()/cond()`.
- **Статус:** ✅ verified (task #238).

### (попутно) Rust `FieldBuilder::foreign_key_on_delete()` → ⏳
- **Связано с:** B-паритет + Phase D. Rust-билдер хардкодил `on_delete=Restrict`;
  добавлен явный выбор действия (паритет с TS `foreignKey(t,f,{onDelete})`).
- **Статус:** ⏳ в рабочем дереве (часть фикса #236, коммит ожидает просьбы).

---

## D. Эволюция OQL

### D1 — Keyset / cursor-пагинация → ✅ end-to-end
- **Action-item:** D1 (P2, «лучший ROI: машинерия есть, нужен surface»).
- **Сделано:** `Pagination::After { key, limit }` (wire-тег `"After"`,
  PascalCase) на уровне DTO; engine sorted-index **seek** (строго-после ASC /
  строго-до DESC, exclusive); Rust `Query::after` + TS `.after` билдеры; e2e
  зелёный (3/3). `Pagination` потерял `Copy/Eq` (Vec<QueryValue>) → ручной
  `PartialEq` по каноничному msgpack.
- **Коммиты:** `3fc215d` (DTO) · `4cff2fe` (engine seek) · `bfe0660` (Rust
  билдер) · `118d955` (TS) · `e774683`.
- **Статус:** ✅ verified e2e (tasks #231–233, #240).

---

## E. Эволюция DDL

### E6 — FK-actions (`ON DELETE`) → ✅ реализовано как «Phase D» (см. ниже)
- **Action-item:** E6 (был L, «тихие сироты»).
- **Итог:** реализованы `RESTRICT` / `CASCADE` / `SET NULL` (+ `NoAction`
  дефолт для backward-compat) и drop-guard. `ON UPDATE` — вне текущего скоупа.

---

## Phase D — reverse-FK `ON DELETE` (полный трек E6)

Дизайн: `docs/design/declarative-schema-validators/10-referential-on-delete.md`.

| Под-фаза | Что | Коммит | Статус |
|---|---|---|---|
| **D.0** | `FkAction` DTO + `on_delete` на `ForeignKeyDto` + билдеры + serde round-trip | `bf6b320`, `3fc215d`, `e774683` | ✅ |
| **D.1** | `ON DELETE RESTRICT` — reverse-FK discovery + delete-gate (`fk_restrict`) | `cf11378` | ✅ e2e |
| **D.2** | `ON DELETE CASCADE` + `SET NULL` — `plan_cascade`/`apply_cascade_plan` (рекурсия + cycle-guard, depth=32) | `dc4b3a3` | ✅ e2e (после #236) |
| **D.3** | drop-guard — `DropTable` отказывает под живым FK (`drop_refused_fk`) | `dc4b3a3` | ✅ e2e (после #236) |

### Bug #236 — почему D.2/D.3 «не работали» через сервер (КОРЕНЬ)

Изначально D.2/D.3 проходили engine-юнит (in-memory `SchemaValidator`), но
**молча не срабатывали через сервер**. Корень — два дефекта в каталог-пути,
которого юниты не касались:

1. **Главный — писатель каталога терял `on_delete`.**
   `admin_schema::insert_constraint_fields` сериализовал у FK **только**
   `ref_table`/`ref_field`, **не** `on_delete` → при чтении он дефолтился в
   `NoAction` → вся reverse-FK discovery (RESTRICT-гейт, CASCADE, SET NULL)
   тихо отключалась. (Исходная гипотеза «баг в `plan_cascade`» была неверна —
   план корректен, до него не доходил нужный `on_delete`.)
   **Фикс:** писать `on_delete` (snake_case; `NoAction` опускаем → legacy-строки
   байт-идентичны).

2. **Вторичный — drop-guard читал некогерентный кэш.**
   Guard читал in-memory validator-bindings, которые **некогерентны** между
   admin-`DbInstance` и engine-инстансом execute-пути → пусто.
   **Фикс:** читать персистентный каталог (`system_store.load_table_record` +
   `SCHEMA_FIELD`).

- **Как найден:** in-process регрессионный тест через **реальный `db.execute`**
  (тот же путь, что у сервера) — `crates/shamir-db/tests/declarative_schema_fk_ondelete_e2e.rs`;
  итерация ~2 c/прогон вместо 13-мин пересборки сервера. Контрольный RESTRICT
  воспроизвёл провал → инструментирование discovery показало `on_delete=NoAction`
  → дошёл до писателя.
- **Верификация:** Rust in-process 5/5 · TS e2e через свежесобранный сервер 5/5
  (D.1 RESTRICT ×2, D.2 CASCADE, D.2 SET NULL, D.3 drop-guard) · гейт `fmt` +
  `clippy --all-targets -D warnings` + lib 1810/1810.
- **Статус:** ⏳ done & verified; коммит ожидает явной просьбы. Файлы в рабочем
  дереве: `admin_schema.rs`, `admin_table_index.rs`, `ddl/schema.rs`,
  новый `declarative_schema_fk_ondelete_e2e.rs`, un-skip в `e2e-fk-ondelete.test.ts`.

---

## Сводная карта

| Action-item | Статус | Где |
|---|---|---|
| A1 (fail-open) | ✅ снято (ложная тревога) | `0d3fd13`, `f27283b` |
| A3 (drop-guard) | ✅ DropTable-часть | Phase D.3 |
| B1 (Batch-сеттеры) | ✅ | #237 |
| B3 (`$expr`/`$cond`) | ✅ | #238 |
| D1 (keyset) | ✅ e2e | `3fc215d`…`118d955` |
| E6 (FK-actions) | ✅ = Phase D | таблица выше |
| **#236 (D.2/D.3 e2e gap)** | ⏳ fixed & verified, коммит ждёт | рабочее дерево |

**Осталось из «трёх главных»:** A2 (открытые access-дефолты `0o777`/owner=System)
— единственный настоящий невыполненный P0.
