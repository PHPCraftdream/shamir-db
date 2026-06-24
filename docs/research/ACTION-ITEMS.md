בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Action Items — что реально нужно сделать

Сводный, приоритизированный план по итогам пяти исследований + адверсариального
`REVIEW.md` + моей прямой сверки с кодом (`META-REVIEW.md`). Сгруппировано по
**релевантности** (область → срочность). Каждый пункт помечен:

- **Источник** — какой отчёт его поднял.
- **Статус** — ✅ verified (подтверждён чтением кода в этой сессии) / 📄 reported
  (claim отчёта, мной не перепроверен).
- **Объём** — грубая оценка (S / M / L).

> Принцип проекта (`docs/roadmap/PLAN.md` §3): OQL/DDL — object-native, не SQL.
> Поэтому часть «пробелов» зрелых СУБД (JOIN, CTE, window, текстовый фронтенд)
> — **намеренно вне скоупа**, а не недоделка. Они вынесены в §G отдельно как
> «не делать (по дизайну)», чтобы не путать с реальной работой.

---

## A. Корректность и безопасность — P0 (делать первым)

Эти пункты — про молчаливые дыры инвариантов и доступа. Самое важное во всём
корпусе.

### A1. FK/unique молча не срабатывают под autocommit ✅ verified
- **Источник:** `completeness-ddl.md` (§1.2, Residual risk #1); подтверждено
  `REVIEW.md` R5.
- **Факт:** `schema_engine/validator/schema/schema_validator.rs:106` и `:160-164`
  — обе проверки за `if let Some(db) = ctx.db()`, который `Some` только в
  tx-mode. Single-statement INSERT под autocommit **обходит и FK, и unique**.
  Комментарии в коде это прямо признают.
- **Что сделать:** либо (а) wire-ить `ValidatorDb`-resolver и на autocommit-пути
  (неявная одно-оп транзакция всё равно открывает tx внутри — дать ей resolver),
  либо (б) если осознанно — задокументировать как явный контракт «реляционные
  проверки только в явных транзакциях» и заставить `unique`/`foreign_key`
  fail-closed (ошибка DDL-времени «требует tx») вместо тихого пропуска.
- **Объём:** M. **Это correctness-bug, не задача документации.**

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

### A3. Дропы без referential-guard ✅ partial / 📄
- **Источник:** `completeness-ddl.md` G3.
- **Факт:** `DropValidator` отказывает при `bound_in≠∅` (хорошо). Но `DropTable`
  не отказывает, если на неё ссылается чужой FK-rule; `DropFunction` не
  отказывает, если функция привязана как валидатор. (claim про DropValidator я
  пометил «не перепроверял» вслед за REVIEW.)
- **Что сделать:** симметричные guard-проверки на дроп таблицы/функции; либо
  `cascade`, либо `restrict`-отказ.
- **Объём:** M.

---

## B. Полнота билдеров — реальные дыры (P1)

Места, где клиент не может выразить то, что умеет движок/wire. Все ✅ verified.

### B1. Rust `Batch`: нет сеттеров `result_encoding` / `interner_epochs` ✅
- **Источник:** `coverage-rust-query-builder.md` #36/#37.
- **Факт:** `batch/batch.rs:628` хардкодит `ResultEncoding::default()`; сеттера
  нет. v2 id-keyed pass-through (перф-путь) недоступен из билдера.
- **Сделать:** chainable `.result_encoding(enc)` + `.interner_epochs(map)`.
- **Объём:** S (тривиально). **Самый дешёвый перф-relevant пункт.**

### B2. Rust `FieldBuilder`: нет `.one_of()` ✅
- **Источник:** `coverage-rust-query-builder.md` #26 (TS его уже имеет — `ddl.ts:621`).
- **Факт:** греп `one_of` по `shamir-query-builder/src` — пусто. `ConstraintsDto.one_of`
  на wire есть (`schema_ops.rs:65`), сеттера в билдере нет.
- **Сделать:** `.one_of(values)` в `ddl/schema.rs::FieldBuilder` (паритет с TS).
- **Объём:** S.

### B3. Rust: нет конструкторов `FilterExpr` (`$expr`) / `Cond` (`$cond`) ✅
- **Источник:** `coverage-rust-query-builder.md` #26/#27 (HIGH в его gap-list).
- **Факт:** `val/filter_value.rs` заканчивается на `qref_all()`; обёрток нет.
  TS их уже имеет (`filter.expr()`/`filter.cond()`) — Rust отстаёт.
- **Сделать:** `val::expr(op,args)` + удобные обёртки (`add/concat/…`),
  `val::cond(if,then,else)`.
- **Объём:** M (богатое под-API: 18 операторов).

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

### C1. Нет e2e для FTS / vector / `call` ✅ (по моим знаниям сессии)
- **Источник:** `coverage-ts-tests.md` P0 (1/2/3).
- **Факт:** ни один e2e не создаёт FTS- или vector-индекс и не гоняет
  similarity/`fts`-запрос; ни один e2e не вызывает stored-функцию через `call()`
  (хотя `createFunction` e2e-покрыт). Серде-регрессия в `Fts`/`VectorSimilarity`/
  `CallOp` пройдёт unit-тесты и тихо сломает фичу.
- **Сделать:** по e2e-кейсу на каждую: createIndex(fts)+fts-query;
  createIndex(vector)+top-k; createFunction+call+assert result.
- **Объём:** M. **Headline-фичи с нулевым интеграционным покрытием.**

### C2. Phase B/C FieldBuilder-констрейнты без unit-тестов ✅ verified
- **Источник:** `coverage-ts-tests.md` (§3.4, P3 #13).
- **Факт:** `scalar/oneOf/format/compare/foreignKey/unique` (как констрейнты
  поля) покрыты **только** server-gated e2e. Без бинаря сервера — **нулевое**
  покрытие в дефолтном `vitest run`.
- **Сделать:** wire-shape unit-тесты на эти 6 сеттеров в `ddl.test.ts` (как уже
  сделано для остальных) — слой билдера должен быть покрыт независимо от сервера.
- **Объём:** S. Дешёвая страховка.

### C3. Тонкое e2e: `commitMigration`-success, `dropUser`/`dropRole`, `chgrp` 📄
- **Источник:** `coverage-ts-tests.md` P2/P3.
- **Факт:** e2e-миграция гоняет только rollback-путь; `dropUser`/`dropRole`/
  `chgrp` — unit-only.
- **Сделать:** добить успешный commit-путь + по e2e-кейсу на дроп user/role и
  на chgrp-эффект.
- **Объём:** S-M.

---

## D. Эволюция OQL — реальные кандидаты (P2)

### D1. Keyset / cursor-пагинация (DTO-surface) ✅ engine-ready
- **Источник:** `completeness-oql.md` H3 — назван «самой дешёвой high-impact
  победой».
- **Факт:** движок уже умеет sorted-index seek (`read_planner.rs:403`
  `try_plan_order_limit_fast_path`). Не хватает только DTO-поверхности
  (`Pagination::After(key)`); сейчас только offset-пагинация → deep-page O(offset).
- **Сделать:** добавить `After(keyset)`-вариант в `Pagination` + план seek.
- **Объём:** M. **Лучший ROI в языке: машинерия есть, нужен только surface.**

### D2. RETURNING-симметрия для INSERT/DELETE 📄
- **Источник:** `completeness-oql.md` §2.8 (M7).
- **Факт:** `UpdateOp` имеет `UpdateSelect`; INSERT/DELETE returning слабее/
  асимметричен.
- **Сделать:** привести returning-семантику к единому виду.
- **Объём:** M.

---

## E. Эволюция DDL — реальные кандидаты (P2)

### E1. `RENAME` для db/repo/table/index/role/group/folder 📄
- **Источник:** `completeness-ddl.md` G6.
- **Факт:** переименовывать умеют только функции и валидаторы. Rename — самая
  дешёвая неразрушающая эволюция, отсутствует повсеместно.
- **Объём:** M.

### E2. `DEFAULT`-значения полей 📄
- **Источник:** `completeness-ddl.md` G9.
- **Факт:** поле можно `required`, но движок не подставит значение (литерал/
  computed) на insert. Нет server-side `created_at`-штампа.
- **Сделать:** опционально завязать на «mutating/transform validators» (в
  `VALIDATORS.md` отмечены как future).
- **Объём:** M-L.

### E3. `if_exists` на дропах + `cascade` на уровне таблицы 📄
- **Источник:** `completeness-ddl.md` G2.
- **Факт:** `if_not_exists` на create есть; `if_exists` на drop нет нигде;
  `cascade` только на db/repo. Скрипты не идемпотентны.
- **Объём:** S-M. Операционно важно для CI/миграций.

### E4. `DESCRIBE` / `SHOW CREATE` (полная форма объекта) 📄
- **Источник:** `completeness-ddl.md` G5.
- **Факт:** `list_*` отдаёт только имена; нет одной операции, возвращающей
  полную форму таблицы (schema+indexes+validators+retention+buffer+owner/mode).
- **Объём:** M. Нужно SDK/тулингу.

### E5. Две дороги к uniqueness (schema-rule vs index-flag) — согласовать 📄
- **Источник:** `completeness-ddl.md` G15.
- **Факт:** `ConstraintsDto.unique` (через валидатор) и `CreateIndexOp.unique`
  (на уровне индекса) — разные пути enforcement. Риск рассогласования (см. также
  A1: rule-путь вообще fail-open под autocommit).
- **Сделать:** свести к одному источнику истины уникальности.
- **Объём:** M. Тесно связано с A1.

### E6. FK-actions (`ON DELETE`/`ON UPDATE`) 📄
- **Источник:** `completeness-ddl.md` G7.
- **Факт:** FK — forward-only existence; удаление referenced-строки не каскадит
  и не блокирует → тихие сироты.
- **Объём:** L.

---

## F. Гигиена самих отчётов — быстрые правки (P3)

Подтверждённые мной неточности в уже закоммиченных доках. Чинить только по
явной просьбе (правки доков, не кода).

- **F1.** `coverage-rust-query-builder.md`: сводка ❌ говорит «5» (стр.94) и «7»
  (стр.290) — реально **10**. Привести к одному числу. ✅ verified.
- **F2.** `coverage-ts-tests.md`: `it()`-счётчики занижены на 15–40% — взять из
  `vitest run` (ddl=75, e2e=74, filter=40, admin=42, select=28…). ✅ verified.
- **F3.** `completeness-oql.md` §1.6: «12 folders» → **11** (canonical под
  crypto, `lib.rs:60`). ✅ verified.
- **F4.** `completeness-ddl.md` §1.5: парентезу «(no challenge/response)» убрать/
  уточнить — SCRAM-handshake существует (`protocol.ts`/`scram.ts`); Argon2id
  относится к at-rest хешированию, не к отсутствию протокола. ✅ verified.
- **F5.** `coverage-ts-query-builder.md` #180: Rust `one_of` помечен ✅ — на деле
  Rust-сеттера нет (это ещё один «TS exceeds Rust»). Поправить рейтинг. ✅ verified.

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

## Сводная карта приоритетов

| Tier | Пункты | Суть |
|---|---|---|
| **P0 — корректность/безопасность** | A1, A2, A3 | FK/unique fail-open; открытые access-дефолты; guard на дропах |
| **P1 — билдеры + тесты** | B1–B4, C1, C2 | дешёвые билдер-сеттеры; e2e FTS/vector/call; unit на Phase B/C |
| **P2 — эволюция языка** | D1, E1, E3, E5 | keyset-пагинация (лучший ROI); RENAME; if_exists; unify unique |
| **P3 — доки + DX** | F1–F5, B5–B7, C3, D2, E2/E4/E6 | правки отчётов; DX-билдеры; досборка e2e; DEFAULT/DESCRIBE/FK-actions |

**Если делать ровно три вещи:** A1 (fail-open), A2 (access-дефолты), D1
(keyset) — две закрывают молчаливые дыры, третья — самая дешёвая
производительная победа.
