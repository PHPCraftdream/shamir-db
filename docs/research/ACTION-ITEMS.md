בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Action Items — что реально нужно сделать

Сводный, приоритизированный план по итогам пяти исследований + адверсариального
`REVIEW.md` + моей прямой сверки с кодом (`META-REVIEW.md`). Сгруппировано по
**релевантности** (область → срочность). Каждый пункт помечен:

- **Источник** — какой отчёт его поднял.
- **Статус** — ✅ verified (подтверждён чтением кода в этой сессии) / 📄 reported
  (claim отчёта, мной не перепроверен).
- **Объём** — грубая оценка (S / M / L).

> **Это список ОСТАВШЕЙСЯ работы.** Уже выполненное вынесено в `DONE.md`
> (A1 снят, B1, B3, D1 keyset, Phase D / E6 FK-actions, и вся **кампания Phase E**:
> A3-полностью, C1, C2, D2, E3, E4, M5-EXPLAIN, F1–F5, E1-частично) и здесь
> не повторяется.

> Принцип проекта (`docs/roadmap/PLAN.md` §3): OQL/DDL — object-native, не SQL.
> Поэтому часть «пробелов» зрелых СУБД (JOIN, CTE, window, текстовый фронтенд)
> — **намеренно вне скоупа**, а не недоделка. Они вынесены в §G отдельно как
> «не делать (по дизайну)», чтобы не путать с реальной работой.

---

## A. Корректность и безопасность — P0 (делать первым)

Эти пункты — про молчаливые дыры инвариантов и доступа.

> A1 (FK/unique fail-open) снят как ложная тревога — см. `DONE.md`.

### A2. Открытые access-дефолты (`0o777`, owner=System), гейт не везде ✅ verified
- **Источник:** `completeness-ddl.md` G10 (назван ship-blocker).
- **Факт:** `crates/shamir-types/src/access.rs:104` `pub const OPEN: u16 = 0o777`;
  `:172` «Open default: owner = System, group = None, mode = 0o777». Всё
  world-rwx, пока гейт не включат повсеместно.
- **Что сделать:** довести owner-on-create (создатель = владелец, не System) +
  переход open→enforced дефолта; пройтись по всем admin-путям, что гейт
  вызывается единообразно. Сверить с `docs/roadmap/DDL.md` §0/§3 (там трек уже
  заведён).
- **Объём:** L. Блокер для любого multi-tenant деплоя.

> A3 (`DropFunction`-as-validator guard) — ✅ сделано (Phase E.3:
> `drop_refused_bound` + integration-покрытие; guard был с Phase D.3). Вместе с
> A3-DropTable (Phase D.3) referential lifecycle на дропах закрыт. См. `DONE.md`.

---

## B. Полнота билдеров — реальные дыры (P1)

Места, где клиент не может выразить то, что умеет движок/wire. Все ✅ verified.

> B1 (`result_encoding`/`interner_epochs`) и B3 (`$expr`/`$cond`) — сделаны,
> см. `DONE.md`. Заодно добавлен `FieldBuilder::foreign_key_on_delete()`.

### B2. Rust `FieldBuilder`: нет `.one_of()` ✅
- **Источник:** `coverage-rust-query-builder.md` #26 (TS его уже имеет — `ddl.ts:621`).
- **Факт:** греп `one_of` по `shamir-query-builder/src` — пусто. `ConstraintsDto.one_of`
  на wire есть (`schema_ops.rs:65`), сеттера в билдере нет.
- **Сделать:** `.one_of(values)` в `ddl/schema.rs::FieldBuilder` (паритет с TS).
- **Объём:** S.

### B4. Rust `InsertOp.records_idmsgpack` не выставлен ✅
- **Источник:** `coverage-rust-query-builder.md` #30.
- **Факт:** `write/insert.rs:55` хардкод `Vec::new()`. id-keyed msgpack путь
  (v2-оптимизация) без точки входа в билдере.
- **Сделать:** `Insert::row_idmsgpack(bytes)` или `Doc::build_idmsgpack()`.
- **Объём:** M.

### B5. TS: interner-DDL билдеры (`internerDump`/`internerTouch`) 🟡 уточнить
- **Источник:** `coverage-ts-query-builder.md` G7 — но **claim завышен**: TS
  **реально шлёт** `interner_touch` в `client.ts:544` и обрабатывает
  `interner_dump` в `field-map.ts`. Нет именно user-facing билдер-метода в
  `builders/ddl.ts`.
- **Сделать (если нужно):** тонкие `internerDump()`/`internerTouch()` в `ddl.ts`
  поверх уже существующих wire-форм. Низкий приоритет — smart-write уже
  покрывает основной сценарий.
- **Объём:** S.

### B6. TS: `Doc`-билдер для вычисляемых write-значений (`$ref`/`$fn`) 📄
- **Источник:** `coverage-ts-query-builder.md` G5.
- **Факт (по отчёту):** TS-пользователь шлёт plain JS-объекты, не может
  встроить `$ref`/`$fn`/`$query` в write-значения без ручной сборки wire-формы.
- **Сделать:** допускать `FilterValue` в позициях `WireValue`, либо `Doc`-класс.
- **Объём:** M. Может быть осознанным дизайном (JS-литералы) — согласовать.

### B7. TS: `subscribe` `deliver_call`, `tryBuild()`-валидация, типизированный `Handle` 📄
- **Источник:** `coverage-ts-query-builder.md` G6/G4/G3.
- **Сделать:** `deliverCall`-опция; `tryBuild()` с проверкой `$query`-алиасов и
  `after`-зависимостей; `Handle`/`RowRef` для типобезопасных `$query`-путей.
- **Объём:** M (вместе). DX-улучшения, не блокеры.

---

## C. Покрытие тестами — реальные дыры (P1)

> C1 (e2e FTS/vector/call) — ✅ сделано (Phase E.8: по e2e-кейсу на каждую через
> release-сервер, 9/9). См. `DONE.md`.
> C2 (Phase B/C field-констрейнты unit) — ✅ сделано (Phase E.9: +7 wire-shape
> unit на 6 сеттеров в `ddl.test.ts`). См. `DONE.md`.

### C3. Тонкое e2e: `commitMigration`-success, `dropUser`/`dropRole`, `chgrp` 📄
- **Источник:** `coverage-ts-tests.md` P2/P3.
- **Факт:** e2e-миграция гоняет только rollback-путь; `dropUser`/`dropRole`/
  `chgrp` — unit-only.
- **Сделать:** добить успешный commit-путь + по e2e-кейсу на дроп user/role и
  на chgrp-эффект.
- **Объём:** S-M.

---

## D. Эволюция OQL — реальные кандидаты (P2)

> D1 (keyset/cursor-пагинация) — сделано end-to-end, см. `DONE.md`.
> D2 / M7 (RETURNING-симметрия INSERT/DELETE) — ✅ сделано (Phase E.5: `select`
> на DeleteOp/InsertOp + проекция полей; заодно починен латентный
> update-projection баг). См. `DONE.md`.
> M5 (EXPLAIN / dry-run plan) — ✅ сделано (Phase E.7: флаг `explain` на ReadQuery,
> preview плана без материализации). См. `DONE.md`.

---

## E. Эволюция DDL — реальные кандидаты (P2)

### E1. `RENAME` (остаток) — repo/index + populated-table 🟡 частично сделано
- **Источник:** `completeness-ddl.md` G6.
- **Сделано:** `RENAME TABLE` (Phase E.4, Object 1) — rekey каталога +
  reverse-index + `copy_store`, с честными guard'ами (populated/schema/
  destination). См. `DONE.md`.
- **Остаток (задача #250):** RenameRepo / RenameIndex (+db/role/group/folder) и
  снятие архитектурного барьера — rename **populated** таблицы требует миграции
  in-memory MVCC-overlay (вторжение в `shamir-tx`).
- **Объём:** M (repo/index) + L (overlay-миграция).

### E2. `DEFAULT`-значения полей 📄
- **Источник:** `completeness-ddl.md` G9.
- **Факт:** поле можно `required`, но движок не подставит значение (литерал/
  computed) на insert. Нет server-side `created_at`-штампа.
- **Сделать:** опционально завязать на «mutating/transform validators» (в
  `VALIDATORS.md` отмечены как future).
- **Объём:** M-L.

> E3 (`if_exists` на дропах + table-level `cascade`) — ✅ сделано (Phase E.1:
> `if_exists` на всех drop-ops; Phase E.2: `cascade` на `drop_table`). См. `DONE.md`.
> E4 / G5 (`DESCRIBE` / `SHOW CREATE`) — ✅ сделано (Phase E.6: `DescribeTableOp`
> компонует полную форму из существующих reads). См. `DONE.md`.

### E5. Две дороги к uniqueness (schema-rule vs index-flag) — согласовать 📄
- **Источник:** `completeness-ddl.md` G15.
- **Факт:** `ConstraintsDto.unique` (через валидатор) и `CreateIndexOp.unique`
  (на уровне индекса) — разные пути enforcement. Риск рассогласования.
- **Сделать:** свести к одному источнику истины уникальности.
- **Объём:** M.

> E6 (FK-actions `ON DELETE`) — реализовано как Phase D (RESTRICT/CASCADE/
> SET NULL + drop-guard), см. `DONE.md`. `ON UPDATE` — вне текущего скоупа.

---

## F. Гигиена самих отчётов — быстрые правки (P3) → ✅ ВСЕ СДЕЛАНЫ (Phase E.9)

F1–F5 исправлены в Phase E.9 (коммит `fc14e46`): F1 счётчик ❌→10; F2 `it()`-
счётчики из `vitest run` (total→692); F3 «12 folders»→11; F4 формулировка SCRAM
challenge/response; F5 Rust `one_of` ✅→❌ (B2 ещё открыт). См. `DONE.md`.

---

## G. НЕ делать — вне скоупа по дизайну (для ясности, не работа)

Эти «пробелы» зрелых СУБД — осознанный выбор object-native архитектуры
(`PLAN.md` §3), не недоделка. Перечислены, чтобы не путать с реальной работой:

- Текстовый SQL-фронтенд — никогда (object-native forever).
- Мультитабличный JOIN, correlated subquery, set-операции (UNION/INTERSECT),
  CTE, window-функции — заменены композицией батчей (`$query`), stored-proc
  (`CallOp`) и reactive sub-batch. Не на роадмапе read-пути; живой фронтир —
  Movement C (репликация/«I»), не ширина языка.
- Geo/spatial, graph-traversal, PIVOT, ROLLUP/CUBE — niche, не на роадмапе.
- `ALTER TABLE ADD/DROP COLUMN` — бессмыслен для schemaless-стора (MessagePack +
  interned fields); «alter» = индексы/буфер/валидаторы/доступ, каждый своей
  операцией (`DDL.md` §1).
- SQL `CHECK`-ключевое слово — заменено валидаторами (богаче: WASM + scalar/
  format/compare-rules).

---

## Сводная карта приоритетов (осталось)

| Tier | Пункты | Суть |
|---|---|---|
| **P0 — корректность/безопасность** | A2 | открытые access-дефолты (`0o777`/owner=System) |
| **P1 — билдеры** | B2, B4 | билдер-сеттеры (`one_of` / `records_idmsgpack`) |
| **P2 — эволюция языка** | E1-остаток (#250), E5 | RenameRepo/Index + populated-overlay; unify unique |
| **P3 — DX + досборка** | B5–B7, C3, E2 | DX-билдеры; тонкое e2e (commit/dropUser/chgrp); DEFAULT |

**Следующий настоящий P0:** A2 (открытые access-дефолты `0o777`/owner=System) —
единственный оставшийся блокер уровня корректности/безопасности.

> Выполненное (A1, B1, B3, D1, Phase D/E6, **вся кампания Phase E**: A3, C1, C2,
> D2, E3, E4, M5, F1–F5, E1-частично) — в `DONE.md`.
