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
> (A1 снят, B1, B3, D1 keyset, Phase D / E6 FK-actions, A3-DropTable) и здесь
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

### A3. `DropFunction` без referential-guard (остаток) 📄
- **Источник:** `completeness-ddl.md` G3.
- **Сделано:** `DropTable` теперь отказывает под живым FK (`drop_refused_fk`) —
  Phase D.3, см. `DONE.md`. `DropValidator` уже отказывает при `bound_in≠∅`.
- **Остаток:** `DropFunction` не отказывает, если функция привязана как
  валидатор — добавить симметричный guard (`cascade`/`restrict`-отказ).
- **Объём:** S-M.

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

> D1 (keyset/cursor-пагинация) — сделано end-to-end, см. `DONE.md`.

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
  (на уровне индекса) — разные пути enforcement. Риск рассогласования.
- **Сделать:** свести к одному источнику истины уникальности.
- **Объём:** M.

> E6 (FK-actions `ON DELETE`) — реализовано как Phase D (RESTRICT/CASCADE/
> SET NULL + drop-guard), см. `DONE.md`. `ON UPDATE` — вне текущего скоупа.

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

## Сводная карта приоритетов (осталось)

| Tier | Пункты | Суть |
|---|---|---|
| **P0 — корректность/безопасность** | A2, A3 | открытые access-дефолты; guard на `DropFunction` |
| **P1 — билдеры + тесты** | B2, B4, C1, C2 | билдер-сеттеры (`one_of`/idmsgpack); e2e FTS/vector/call; unit на Phase B/C |
| **P2 — эволюция языка** | E1, E3, E5 | RENAME; if_exists; unify unique |
| **P3 — доки + DX** | F1–F5, B5–B7, C3, D2, E2/E4 | правки отчётов; DX-билдеры; досборка e2e; DEFAULT/DESCRIBE |

**Следующий настоящий P0:** A2 (открытые access-дефолты `0o777`/owner=System) —
единственный оставшийся блокер уровня корректности/безопасности.

> Выполненное (A1, B1, B3, D1, Phase D/E6, A3-DropTable) — в `DONE.md`.
