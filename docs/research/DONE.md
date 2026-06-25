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
параллельно закрыт **E6** (FK-actions). Затем — **кампания Phase E
(Completeness & Operability)** целиком: 9 фаз (if_exists, table-cascade,
DropFunction-guard, RENAME TABLE, RETURNING-симметрия, DESCRIBE, EXPLAIN, e2e
FTS/vector/call, unit C2 + doc-fixes), закрывшая action-items **A3, C1, C2, D2,
E3, E4, F1–F5, M5**. Затем **E.4-followon** (F.1 RENAME INDEX, F.2 populated-table
rename со снятием MVCC-барьера, F.3 RENAME REPO) закрыл **E1 полностью** (#250).
Остался из тройки только **A2** (access-дефолты, P0).

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

### (попутно) Rust `FieldBuilder::foreign_key_on_delete()` → ✅
- **Связано с:** B-паритет + Phase D. Rust-билдер хардкодил `on_delete=Restrict`;
  добавлен явный выбор действия (паритет с TS `foreignKey(t,f,{onDelete})`).
- **Коммит:** `3c57230`.
- **Статус:** ✅ verified (часть фикса #236, закоммичен и запушен).

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
- **Статус:** ✅ done & verified; закоммичен и запушен (`f0c64a6` fix engine/db,
  `3c57230` builder, `f0ac57e` un-skip TS e2e). Файлы: `admin_schema.rs`,
  `admin_table_index.rs`, `ddl/schema.rs`, `declarative_schema_fk_ondelete_e2e.rs`,
  un-skip в `e2e-fk-ondelete.test.ts`.

---

## Phase E — Completeness & Operability (полная кампания, 9 фаз)

План: `docs/research/NEXT-CAMPAIGN.md`. Брифы: `docs/prompts/{ddl-lifecycle,
oql-surface,headline-e2e,verification}/`. Стратегия: commit-per-phase, каждая
фаза верифицирована полным гейтом (fmt + `clippy --workspace --all-targets
-D warnings` + `./scripts/test.sh`) перед коммитом. Запушено
(`5a0ad7e..40f66f3`).

| Фаза | Что | Action-item | Коммит | Статус |
|---|---|---|---|---|
| **E.1** | `if_exists` на всех drop-ops (идемпотентные миграции/CI) | E3 (часть) | `0449075` | ✅ |
| **E.2** | table-level `cascade` на `drop_table` (свои индексы/валидаторы/схема; FK-guard не обходится) | E3 (часть) | `9a30339` | ✅ |
| **E.3** | integration-покрытие DropFunction-as-validator guard (`drop_refused_bound`; сам guard был с Phase D.3) | A3-остаток | `5c7d51f` | ✅ |
| **E.4** | `RENAME TABLE` (Object 1) — rekey каталога + reverse-index + copy_store; честные guard'ы (populated/schema/destination) | E1 (часть) | `a7dcda5` | ✅ (Object 1) |
| **E.5** | RETURNING-симметрия INSERT/DELETE + фикс латентного update-projection бага | D2 / M7 | `cd0d00b` | ✅ |
| **E.6** | `DESCRIBE TABLE` — полная форма (schema+indexes+validators+retention+buffer+owner/mode) из существующих reads | E4 / G5 | `daca34d` | ✅ |
| **E.7** | EXPLAIN / dry-run plan preview (планировщик без материализации) | M5 | `40f66f3` | ✅ |
| **E.8** | e2e FTS / vector / `call` через release-сервер (9/9) | C1 | `3280e96` | ✅ |
| **E.9** | wire-shape unit на 6 field-констрейнтов (C2) + doc-fixes F1–F5 | C2, F1–F5 | `fef73c8`,`fc14e46` | ✅ |

### E.4 — архитектурный барьер (вскрыт, честно ограничен)
RENAME таблицы **с данными** невозможен простым rekey: живые версии строк живут
в in-memory MVCC-overlay (`cells`/`VersionedOverlay`), не переносимом store-level
копией (history вакуумится при `Retention::current_only`). Поэтому E.4 покрыл
**RenameTable для пустых таблиц** + guard `cell_count>0`, отказывающий rename
populated-таблицы (вместо тихой потери данных) + guard'ы на schema-bearing и
destination-exists. **Остаток** (RenameRepo/RenameIndex + снятие MVCC-барьера)
был вынесен в follow-on (#250) и **полностью закрыт** кампанией E.4-followon
(F.1/F.2/F.3) — см. ниже; MVCC-барьер снят в F.2.

### Верификация финального дерева
`fmt --all` ✓ · `clippy --workspace --all-targets -D warnings` ✓ 0 · Rust
**2363/2363** · TS-юниты **462/462** · e2e **9/9** против свежесобранного сервера.
Каждая фаза проверена оркестратором (zero-trust) перед коммитом.

---

## E.4-followon — RENAME INDEX / REPO + populated-table (полный трек E1, #250)

Декомпозиция остатка **#250** на три leaf-фазы. План:
`docs/research/E4-FOLLOWON-PLAN.md`. Брифы: `docs/prompts/ddl-lifecycle/
{05-rename-index,06-populated-rename,07-rename-repo}.md`. Стратегия:
commit-per-phase, **вся работа делегирована crush** (при падениях — рестарт в той
же сессии), каждая фаза zero-trust-верифицирована оркестратором (поймано и
починено **3 реальных дефекта**, которые агент не заметил). e2e — через
`SHAMIR_SERVER_BIN` override (debug-сервер вместо 25-мин release).

| Фаза | Что | Тесты | Коммит | Статус |
|---|---|---|---|---|
| **F.1** | `RENAME INDEX` — rekey записи индекса; sorted/index2 in-place, regular/unique hash drop+rebuild (physical key хэширует `name_interned`) | Rust integ 3/3 · TS-unit +4 · e2e 2/2 | `1ac6b91` | ✅ |
| **F.2** | populated `RENAME TABLE` — `MvccStore::drain_to_history` (overlay→history) + снят guard `cell_count>0`; **MVCC-барьер E.4 СНЯТ** | drain unit 8/8 · rename 4/4 · durability cold-restart · e2e 2/2 | `722f05d` | ✅ |
| **F.3** | `RENAME REPO` — zero-copy rekey реестра DbInstance + каталога (репо — логический ключ; под-сторы переезжают целиком) | Rust integ 3/3 · TS-unit 102 · e2e 3/3 · core 474 | `5e3ea60` | ✅ |

**Снятый барьер (F.2):** живые версии строк из MVCC-overlay теперь синхронно
дренируются в durable history ПЕРЕД store-copy → populated-rename не теряет
данные. Доказано durability-тестом (insert → rename → drop `ShamirDb` → reopen
fjall → все строки целы, старое имя не резолвится).

**Побочно (F.3):** вскрыт и починен латентный клиентский баг `extractRepo`
(`client.ts`) — для ReadQuery с array-form `from` (`Query.withRepo`) возвращал
`'main'` вместо реального репо → de-intern промах для non-default репо.

**Известная limitation (как в rename_table):** validator bindings по
`table_ref="db/repo/table"` оставляют dangling refs при rename репо/таблицы —
вне scope (отдельный follow-on, если понадобится).

**Верификация:** каждая фаза — `clippy --workspace --all-targets -D warnings` ✓ ·
`fmt` ✓ · полная тестовая лестница вплоть до e2e TS.

---

## Сводная карта

| Action-item | Статус | Где |
|---|---|---|
| A1 (fail-open) | ✅ снято (ложная тревога) | `0d3fd13`, `f27283b` |
| A3 (drop-guard) | ✅ DropTable (Phase D.3) + DropFunction (E.3) | Phase D.3 / `5c7d51f` |
| B1 (Batch-сеттеры) | ✅ | #237 |
| B3 (`$expr`/`$cond`) | ✅ | #238 |
| C1 (e2e FTS/vector/call) | ✅ = E.8 | `3280e96` |
| C2 (Phase B/C unit) | ✅ = E.9 | `fef73c8` |
| D1 (keyset) | ✅ e2e | `3fc215d`…`118d955` |
| D2 / M7 (RETURNING) | ✅ = E.5 | `cd0d00b` |
| E3 (if_exists + table cascade) | ✅ = E.1 + E.2 | `0449075`, `9a30339` |
| E4 / G5 (DESCRIBE) | ✅ = E.6 | `daca34d` |
| M5 (EXPLAIN) | ✅ = E.7 | `40f66f3` |
| E6 (FK-actions) | ✅ = Phase D | таблица выше |
| E1 (RENAME) | ✅ **полностью**: E.4 (TABLE) + F.1 (INDEX) + F.2 (populated) + F.3 (REPO) | `a7dcda5`,`1ac6b91`,`722f05d`,`5e3ea60` |
| F1–F5 (doc-fixes) | ✅ = E.9 | `fc14e46` |
| **#236 (D.2/D.3 e2e gap)** | ✅ fixed & verified, закоммичен/запушен | `f0c64a6` |

**Осталось из «трёх главных»:** A2 (открытые access-дефолты `0o777`/owner=System)
— единственный настоящий невыполненный P0. Прочий остаток (B2/B4/B5–B7, C3, E2,
E5) — в `ACTION-ITEMS.md`. E1-follow-on (#250) **закрыт** (см. E.4-followon выше).
