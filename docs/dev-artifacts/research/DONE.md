בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Done — что уже реализовано по `ACTION-ITEMS.md`

Журнал выполненной работы по итогам исследований (`docs/dev-artifacts/research/`) +
`REVIEW.md`/`META-REVIEW.md`. Это «обратная сторона» `ACTION-ITEMS.md`: там —
план, здесь — факт. Каждый пункт: ссылка на action-item, краткая суть,
коммиты, статус верификации.

Легенда статуса: ✅ done & verified · ⏳ done, коммит ожидает явной просьбы.

> Дисциплина сессии: коммит/пуш — только по явной просьбе пользователя;
> делегированные брифы — в `docs/dev-artifacts/prompts/` под git (prompt-first); тесты — через
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
Наконец, **кампания Phase G** закрыла оставшиеся P1-билдеры (**B2** `one_of`,
**B4** `row_idmsgpack`, **C3** e2e lifecycle) и последний настоящий **P0 — A2**
(access-дефолты): G.4 сменила дефолт `open 0o777 → enforced 0o700` для новых
объектов (Strategy A) + единообразный create-гейт + group-path e2e. **Все три
«главных» (D1, Phase D, A2) и весь приоритетный остаток — сделаны.**

Затем **кампания ② (DDL-эволюция)** закрыла E1-остаток (RENAME folder/group/role/
**db**), E6 `ON UPDATE`, E5 unify-uniqueness, E2 литерал-`DEFAULT`. И наконец
**кампания ③ (transform-фреймворк)** превратила литерал-`DEFAULT` в полноценный
декларативный transform-проход (computed-`DEFAULT` + server-stamping
`created_at`/`updated_at`, replay-безопасность доказана) + TS `litU64`/`bin` +
e2e-добивка + hardening catalogue-конверсий — закрыв **последнюю осознанно
отложенную (A)-мини-кампанию**. **Из research-корпуса open-работы не осталось;
живой фронтир — Movement C (репликация, `PHASE-H-PLAN.md`).**

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

Дизайн: `docs/dev-artifacts/design/declarative-schema-validators/10-referential-on-delete.md`.

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

План: `docs/dev-artifacts/research/NEXT-CAMPAIGN.md`. Брифы: `docs/dev-artifacts/prompts/{ddl-lifecycle,
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
`docs/dev-artifacts/research/E4-FOLLOWON-PLAN.md`. Брифы: `docs/dev-artifacts/prompts/ddl-lifecycle/
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

## Phase G — Builders finish (B2/B4/C3) + Access enforcement (A2)

Кампания закрыла оставшиеся P1-билдер-дыры и единственный настоящий **P0** —
открытые access-дефолты. План: `docs/dev-artifacts/research/PHASE-G-PLAN.md`. Брифы:
`docs/dev-artifacts/prompts/{query-builder,e2e,access}/*.md`. Стратегия: **вся работа строго
делегирована crush**, commit-per-phase, каждая фаза zero-trust-верифицирована
оркестратором (поймано **2 реальных пробела**, которые crush оставил —
см. ниже). e2e — через `SHAMIR_SERVER_BIN` на debug-сервер.

| Фаза | Что | Тесты | Коммит | Статус |
|---|---|---|---|---|
| **G.1** (B2) | Rust `FieldBuilder::one_of()` — паритет с TS `oneOf()` | wire-round-trip 2/2 | `3753cbb` | ✅ |
| **G.2** (B4) | Rust `Insert::row_idmsgpack()` — вход в id-keyed msgpack write; `ByteBuf` реэкспортнут из query-types | wire-round-trip 2/2 | `f32ed0c` | ✅ |
| **G.3** (C3) | e2e: commit-migration (start→cutover_ready→commit→dst readable→status not_found) · dropUser/dropRole (HMAC) · chgrp readback | e2e +4 (39/39) | `09eeeed` | ✅ |
| **G.4a** (A2) | owner-on-create — **уже было сделано** прежними слайсами (все mode-bearing ресурсы штампуют `owned_by`); верифицировано | — | — | ✅ |
| **G.4b** (A2) | единообразный `Action::Create` гейт на `create_db/repo/table` (снят TODO authz-gap); аддитивно под OPEN | shamir-db 450/450 · e2e 39/39 | `7ef8860` | ✅ |
| **G.4c** (A2) | **P0**: дефолт `open 0o777 → enforced 0o700` для НОВЫХ объектов (Strategy A); legacy грузится OPEN через `from_record`. `ResourceMeta::owned_enforced` + 7 create-сайтов + починка **51 фикстуры** (A/B/C/D категории) | rust `--full` 4501/4501 · e2e 708/708 | `e9769b4` | ✅ |
| **G.4d** (A2) | e2e group-path: членство в группе + chgrp + group-bits грантит доступ; removal → re-denied | e2e +1 (709/709) | `356aaf0` | ✅ |

**P0 закрыт (G.4c):** новые объекты приватны владельцу (owner-rwx); world-rwx
больше не дефолт. Strategy A — enforced только для вновь создаваемых; существующие
каталог-записи без поля `mode` грузятся как OPEN (обратная совместимость). Спека
доступа не ослаблена: traversal-тесты открывают предков явно (target-проверки
сохранены), default-assertion тесты обновлены на enforced + добавлен явный
`chmod 0o777`-путь (покрытие open-семантики сохранено).

**Zero-trust-уловы (G.4c):** (1) crush сделал движок, но НЕ починил 51 упавшую
фикстуру (его гейт оборвался до прогона) → ре-делегация в ту же сессию с
классификацией провалов A/B/C/D; (2) параллельный `rust --full` + e2e дал Windows
file-lock на `shamir-server.exe` → перезапуск rust-suite отдельно.

**Заметка по охвату:** create-ops (`CreateDb/Repo/Table/...`) — `is_admin`
(superuser-only на wire), поэтому G.4b create-гейт не выразим через e2e (не-superuser
до него не доходит); его enforced-покрытие — в Rust-интеграции (`sec1_ddl_gate_e2e`,
`facade_gateway_acl_tests`). Гейт `authorize_access` стоит на всех admin-путях.

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
| B2 (`FieldBuilder::one_of`) | ✅ = G.1 | `3753cbb` |
| B4 (`Insert::row_idmsgpack`) | ✅ = G.2 | `f32ed0c` |
| C3 (e2e commit-migration/dropUser/dropRole/chgrp) | ✅ = G.3 | `09eeeed` |
| **A2 (open access-дефолты `0o777`)** | ✅ **= Phase G.4** (b: create-гейт; c: enforced дефолт; d: group-path e2e) | `7ef8860`,`e9769b4`,`356aaf0` |
| B5–B7 (DX-билдеры) | ✅ = **кампания ①** (Builder parity & DX) | `d611adc`…`2f085e4e` |
| E1-остаток (folder/group/role RENAME) | ✅ = **②.1a/b/c** | `1bdb1e39`,`8ec5889e`,`eba3d562` |
| E6 `ON UPDATE` (FK-actions) | ✅ = **②.2a/b** | `b45ea51e`,`214b15bb` |
| E5 (unify-uniqueness) | ✅ = **②.3a/b** (defense-in-depth) | `816e1484`,`6c98c029` |
| E2 (`DEFAULT`, literal) | ✅ = **②.4a/b/c** | `1f8eddd7`,`da0e1f9e`,`bc796114` |
| E2-остаток (computed-`DEFAULT` + server-stamping) | ✅ = **кампания ③.2** (transform-фреймворк) | `e77ec918`,`03eff061`,`c6e3e627`,`284fcad5` |
| TS `lit_u64`/`bin` (coverage-ts G9/G10) | ✅ = **③.3a** | `c7bb487d` |
| TS e2e-добивка + tsc-долг (coverage-ts-tests P1) | ✅ = **③.1b** | `2dae54d6` |
| SelectExpr (coverage G8/#8) | ⏸ осознанно отложен (B) — движок не исполняет | решение в `CAMPAIGN-3-PLAN.md §③.3b` |

**Все P0–P3 закрыты.** A2 (открытые access-дефолты) — Phase G.4. B5–B7 — кампания
①. E1-остаток / E6 `ON UPDATE` / E5 / E2 — кампания ②. E1-follow-on (#250) закрыт
(E.4-followon). Остаток по DDL — только осознанно отложенное (см. ниже).

---

## Кампания ② — DDL-эволюция & корректность (твин Phase E)

План: `DDL-EVOLUTION-PLAN.md`. Конвейер per-stage: prompt-first
(`docs/dev-artifacts/prompts/ddl-evolution/`) → `/crush` → zero-trust verify (оркестратор сам:
дифф + `fmt`+`./scripts/test.sh`+`clippy --workspace`+TS) → коммит. Дизайн-проходы
②.3a/②.4a и решение по ②.1d — оркестратор сам (прецедент ①.4 boundary).

| Этап | Суть | Коммит | Тесты |
|---|---|---|---|
| ②.1a RENAME folder | path-rekey записи+потомков, ResourceMeta сохранён | `1bdb1e39` | 7 Rust + TS |
| ②.1b RENAME group | id-keyed → смена display-name (без rekey ссылок) | `8ec5889e` | 6 Rust + 2 TS |
| ②.1c RENAME role | name-keyed → re-key + rekey ссылок во всех users | `eba3d562` | 5 Rust + TS |
| ②.1d RENAME db | чистый каталог-rekey (γ): databases/repositories/tables + in-memory map, `path` неизменен | `a3928add` | 7 e2e (completeness/durable-reopen/guards/data) |
| ②.2a FK `ON UPDATE` surface | `on_update: FkAction` сквозь DTO/Ref/builders, back-compat | `b45ea51e` | 13 fk |
| ②.2b FK `ON UPDATE` enforce | `fk_on_update.rs`: no-op gate → Restrict/Cascade-rekey/SetNull | `214b15bb` | 9 (+delete-путь не сломан) |
| ②.3a/b unify-uniqueness | (B) defense-in-depth: нормативный контракт + coherence | `816e1484`,`6c98c029` | 33 unique |
| ②.4a/b/c DEFAULT (literal) | design (B) + surface + stamp на insert (явный-NULL keystone) | `1f8eddd7`,`da0e1f9e`,`bc796114` | 29 default |

**Развилки (решения оркестратора, в `DDL-EVOLUTION-PLAN.md`):**
- **②.1d RENAME db → (γ) каталог-rekey** (сначала отложен, затем реализован):
  пересмотр предпосылки — boot берёт repo-путь из persisted `path`-поля
  (`core.rs:154-164`), НЕ из имени db → физ-локация декуплена → rename без
  fs-move/handle-drain/reopen/crash-window. «Самый тяжёлый случай» оказался
  лёгким (твин RENAME role). Точка риска — полнота rekey (grep-аудит: 3
  db_name-каталога). Durable-reopen RED поймал zero-trust-прогон оркестратора
  (envelope соврал 7/7; реально 6/7) — корень оказался test-env shared-temp
  accumulation, не код; фикс — `tempfile::tempdir` изоляция.
- **②.3a → (B)**: DDL-инвариант `unique`-rule⟹index уже есть, probe O(1) → слои
  комплементарны; probe не снимать.
- **②.4a → (B) узкий литерал**: константный DEFAULT replay-safe by-construction →
  без mutating-фреймворка; computed (`now()`) → будущая (A)-мини-кампания.
- **②.2b**: computed new-value → REJECT (не тихий skip); depth=1 (исключает FK-cycle).

**Закрывает action-items:** E1-остаток вкл. RENAME db (G6 целиком), E6 `ON UPDATE`
(G7), E5 (G15), E2 (G9). Все запушены (`master` синхрон).

---

## Кампания ③ — Завершающая досборка (transform-фреймворк · тесты · мелочи)

План: `CAMPAIGN-3-PLAN.md`. Конвейер per-stage: prompt-first
(`docs/dev-artifacts/prompts/campaign-3/`) → делегирование агентам **`sh`** (по явной просьбе
пользователя — не `/crush`) → zero-trust verify (оркестратор сам: дифф +
`./scripts/test.sh` + `clippy --workspace` + TS `vitest`/`tsc`) → коммит per-stage
при спокойном дереве. Дизайн-проход ③.2a и решение ③.3b — оркестратор сам.

**Главный итог:** ②.4 литерал-`DEFAULT` вырос в полноценный **декларативный
transform-фреймворк** — закрыта последняя «осознанно отложенная (A)-мини-кампания»
(computed-`DEFAULT` + server-stamping). Валидаторы из чисто-CHECK стали
**CHECK + декларативный transform** (mutating BEFORE-проход); replay-безопасность
доказана durable-reopen-тестом (transforms на admission, НЕ на WAL-replay).

| Этап | Суть | Коммит | Тесты |
|---|---|---|---|
| ③.1a TS unit Phase B/C | 6 FieldBuilder-сеттеров (`scalar/oneOf/format/compare/foreignKey/unique`) — оказались УЖЕ покрыты wire-shape unit (Phase E.9); верифицировано | — (уже было) | ddl.test 116/116 |
| ③.1b TS e2e-добивка | +25 server-gated e2e (`like/ilike/regex`, `isNull/exists/contains*`, `page`, `distinct`, `select.func/aggregateFn`, `history`) + почин 4 tsc-долгов (`WriteValue` вместо `Record<string,unknown>`) | `2dae54d6` | e2e +25, tsc 0 |
| ③.2a дизайн transform | оркестратор: декларативный `apply_transforms` у точки ②.4c (pre-encode), НЕ мутация в `run_validators_loop` (там после encode, read-only линза); boundary в `CAMPAIGN-3-PLAN.md` | `c1eefd4c` | — |
| ③.2b framework | `TransformSpec{ComputedDefault,AutoNowAdd,AutoNow}` + `schema_transforms()` (близнец `schema_defaults`) + `apply_transforms` у `write_exec:152` ДО encode (fast-skip, now_ns раз на батч); CHECK-валидаторы не тронуты | `e77ec918` | 11 unit, @engine 1239 |
| ③.2c computed-DEFAULT | `default: Option<QueryValue>` → `Option<FilterValue>` (литерал И выражение); аггрегатор split: литерал → `apply_defaults`, выражение → `ComputedDefault` через `eval_write_value`; builders Rust+TS | `03eff061` | @engine 1248, qt+qb 706, ddl.test 119 |
| ③.2d server-stamping | флаги `auto_now`/`auto_now_add` + builders; `apply_transforms` +`is_insert` гейт (AutoNow каждый write; AutoNowAdd/ComputedDefault только insert); UPDATE/UPSERT wiring | `c6e3e627` | @engine 1252, e2e 4/4 |
| ③.2e тесты replay | **KEYSTONE** durable-reopen: `created_at`/`updated_at` bit-identical после reopen (transforms на admission, не на replay); порядок transform-перед-CHECK (passes/rejects) | `284fcad5` | replay e2e 3/3, @engine 1252 |
| ③.3a TS litU64/bin | filter-хелперы (зеркало Rust); `litU64`→`number` (НЕ bigint: `@msgpack/msgpack` бросает на BigInt — `client.ts:461`); `bin` сахар-нормализатор | `c7bb487d` | filter.test 47 |
| ③.3b SelectExpr | **развилка (B) — НЕ строим**: движок `read_exec:83` не исполняет `SelectExpr` (парсится, проекция игнор/reject) → билдер для невыполняемого типа породил бы тихо-игнор-запросы; отложено до спроса | (решение в плане) | — |
| ③.h1 hardening | убран silent-fallback из 3 catalogue `FilterValue↔QueryValue`-сайтов: литералы → прямой match (`query_value_to_filter_value`), выражения → msgpack но с `log::warn` вместо тихого `Null`/drop | `ddcb6321` | @engine 1252, qt+qb+db 840 |

**Развилки (решения оркестратора):**
- **③.2a → декларативный transform-проход** (НЕ общий mutating-трейт): скоуп ③ =
  literal + computed-default + created/updated_at; общий side-effecting AFTER-trigger
  (G13) — future. Критическая находка: **encode ПРЕДШЕСТВУЕТ валидации**
  (`write_exec:106→185`) → черновик «мутировать в `run_validators_loop`» был бы
  неверен (loop после encode, read-only); решение — `apply_transforms` у `apply_defaults`.
- **③.2c → `default: Option<FilterValue>`** (унификация литерал+выражение, FilterValue —
  надмножество с `$fn`/`$expr`), источник скаляров `builtin_scalars()` (консистентно с
  `resolve_computed_record`).
- **③.3b → (B) задокументировать-отложить** (обосновано фактами движка `read_exec:83`/
  `aggregate:663`).

**Zero-trust-уловы (оркестратор поймал):**
- **③.3a:** агент заявил «bigint сериализуется как JSON-int» — ЛОЖЬ: провод msgpack,
  `@msgpack/msgpack` бросает на BigInt (`client.ts:461-467`) → `litU64(>2^53)` крашнул
  бы encode. Переделегировано → `litU64` возвращает `number` (lossy, msgpack-safe).
- **③.2c:** msgpack round-trip с silent `unwrap_or(Null)` → завёл и выполнил ③.h1.
- **③.1d-урок применён в ③.2e:** durable-reopen тест использует `tempfile::tempdir`
  (не `std::env::temp_dir`) — изоляция data_root между прогонами.

**Закрывает:** последнюю «(A)-мини-кампанию» (computed-`DEFAULT`/server-stamping) из
`completeness-ddl.md` G9-остатка + `VALIDATORS.md` "Future extensions" (mutating
BEFORE-валидаторы); coverage-ts `lit_u64`/`bin` (G9/G10). SelectExpr (G8/#8) —
осознанно отложен (B). Все запушены (`master`, `b0e1f5a2`).

**Осознанно отложено (НЕ блокеры, отдельные кампании):**
- **Phase H — Leader-Follower репликация** (Movement C, единственный незакрытый
  charter-пилон «I») — план `PHASE-H-PLAN.md`, ждёт слова о направлении.
- **AFTER / side-effecting триггеры** (`completeness-ddl.md` G13) — transform —
  это BEFORE-mutating; общие AFTER-hooks (audit-append, event-out) — future.
- **Perf write-path group-commit** — альтернативная кампания (закрытый пилон «H»).
