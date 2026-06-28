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
> (A1 снят, B1, B3, D1 keyset, Phase D / E6 FK-actions, вся **кампания Phase E**:
> A3, C1, C2, D2, E3, E4, M5-EXPLAIN, F1–F5; **E.4-followon**: E1 полностью;
> **кампания Phase G**: B2 `one_of`, B4 `row_idmsgpack`, C3 e2e-lifecycle, **A2
> access-enforcement**; **кампания ②**: E1-остаток RENAME folder/group/role/db,
> E6 `ON UPDATE`, E5 unify-uniqueness, E2 литерал-`DEFAULT`; **кампания ③**:
> computed-`DEFAULT` + server-stamping (transform-фреймворк), TS `litU64`/`bin`,
> e2e-добивка, hardening) и здесь не повторяется. **Все P0–P3 закрыты** — из
> research-корпуса open-работы не осталось; живой фронтир — Movement C (репликация).

> Принцип проекта (`docs/roadmap/PLAN.md` §3): OQL/DDL — object-native, не SQL.
> Поэтому часть «пробелов» зрелых СУБД (JOIN, CTE, window, текстовый фронтенд)
> — **намеренно вне скоупа**, а не недоделка. Они вынесены в §G отдельно как
> «не делать (по дизайну)», чтобы не путать с реальной работой.

---

## A. Корректность и безопасность — P0 (делать первым)

Эти пункты — про молчаливые дыры инвариантов и доступа.

> A1 (FK/unique fail-open) снят как ложная тревога — см. `DONE.md`.

> ### A2 (открытые access-дефолты `0o777`/owner=System) — ✅ **СДЕЛАНО = Phase G.4**
> owner-on-create (G.4a, уже было) + единообразный `Action::Create` гейт на
> create-путях (G.4b, `7ef8860`) + переход дефолта **`open 0o777 → enforced
> 0o700`** для новых объектов, Strategy A (G.4c, `e9769b4`, + починка 51 фикстуры)
> + group-path negative/positive e2e (G.4d, `356aaf0`). Последний настоящий P0
> закрыт. См. `DONE.md` (раздел «Phase G»).

> A3 (`DropFunction`-as-validator guard) — ✅ сделано (Phase E.3:
> `drop_refused_bound` + integration-покрытие; guard был с Phase D.3). Вместе с
> A3-DropTable (Phase D.3) referential lifecycle на дропах закрыт. См. `DONE.md`.

---

## B. Полнота билдеров — реальные дыры (P1)

Места, где клиент не может выразить то, что умеет движок/wire. Все ✅ verified.

> B1 (`result_encoding`/`interner_epochs`) и B3 (`$expr`/`$cond`) — сделаны,
> см. `DONE.md`. Заодно добавлен `FieldBuilder::foreign_key_on_delete()`.

> ### B2 (Rust `FieldBuilder::one_of()`) — ✅ **СДЕЛАНО = Phase G.1** (`3753cbb`)
> `.one_of(values)` в `ddl/schema.rs::FieldBuilder` (паритет с TS). См. `DONE.md`.

> ### B4 (Rust `Insert::row_idmsgpack`) — ✅ **СДЕЛАНО = Phase G.2** (`f32ed0c`)
> `Insert::row_idmsgpack(bytes)` + проброс в `InsertOp.records_idmsgpack` вместо
> хардкода `Vec::new()`; `ByteBuf` реэкспортнут из query-types. См. `DONE.md`.

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

> ### C3 (тонкое e2e: commit-migration / dropUser-dropRole / chgrp) — ✅ **СДЕЛАНО = Phase G.3** (`09eeeed`)
> e2e commit-путь миграции (start→cutover_ready→commit→dst readable→status
> not_found) + dropUser/dropRole (HMAC) + chgrp readback. +4 it() (39/39). См. `DONE.md`.

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

> E1 (`RENAME` — table/index/repo + populated-table) — ✅ **СДЕЛАНО ПОЛНОСТЬЮ**.
> `RENAME TABLE` (Phase E.4) + кампания **E.4-followon**: F.1 RENAME INDEX,
> F.2 populated-table rename (снят MVCC-overlay-барьер через
> `MvccStore::drain_to_history`), F.3 RENAME REPO. См. `DONE.md`. Остаток
> `folder/group/role/db` RENAME — ✅ **СДЕЛАН ЦЕЛИКОМ в кампании ② (②.1a-d)**:
> `RenameFunctionFolder` (path-rekey + потомки), `RenameGroup` (id-keyed →
> display-name), `RenameRole` (name-keyed → rekey ссылок в users), **`RenameDb`**
> (чистый каталог-rekey, вариант γ — физ-путь декуплён в persisted `path`-поле,
> rename без fs-move; предпосылка отложения «нужен on-disk каскад» оказалась
> ложной). G6 закрыт полностью. См. `DONE.md`.

### E2. `DEFAULT`-значения полей 📄 → ✅ **СДЕЛАНО (кампания ②.4)**
- **Источник:** `completeness-ddl.md` G9.
- **Было:** поле можно `required`, но движок не подставит значение на insert.
- **Факт:** ✅ литерал-`DEFAULT` реализован (②.4) **И computed-`DEFAULT` +
  server-stamping — ✅ СДЕЛАНЫ (кампания ③.2)**. ②.4: `default: Option<QueryValue>`
  + штамп на INSERT (`apply_defaults`); явное значение (вкл. явный NULL) не
  перетирается; replay-safe. ③.2: `default` расширен до `Option<FilterValue>`
  (литерал И выражение `$fn`) — computed-`DEFAULT` через `eval_write_value`;
  `auto_now`/`auto_now_add` server-stamping `created_at`/`updated_at`; общий
  декларативный `apply_transforms` (pre-encode, близнец ②.4c); replay-безопасность
  доказана durable-reopen-тестом (transforms на admission, не на WAL-replay).
  Бывшая «(A)-мини-кампания mutating-валидаторов» закрыта. См. `DONE.md`
  (раздел «Кампания ③»), `CAMPAIGN-3-PLAN.md`.

> E3 (`if_exists` на дропах + table-level `cascade`) — ✅ сделано (Phase E.1:
> `if_exists` на всех drop-ops; Phase E.2: `cascade` на `drop_table`). См. `DONE.md`.
> E4 / G5 (`DESCRIBE` / `SHOW CREATE`) — ✅ сделано (Phase E.6: `DescribeTableOp`
> компонует полную форму из существующих reads). См. `DONE.md`.

### E5. Две дороги к uniqueness (schema-rule vs index-flag) — согласовать 📄 → ✅ **СДЕЛАНО (кампания ②.3)**
- **Источник:** `completeness-ddl.md` G15.
- **Было:** `ConstraintsDto.unique` (через валидатор-probe) и `CreateIndexOp.unique`
  (на уровне индекса) — разные пути; риск рассогласования.
- **Факт:** ✅ согласовано через **(B) defense-in-depth** (②.3a дизайн): два слоя
  КОМПЛЕМЕНТАРНЫ, не дубль — probe (логический fail-fast, чистая `unique_violation`,
  O(1) через обязательный индекс) поверх index-guard (физическая атомарность,
  HIGH-A race-closing), связаны DDL-инвариантом `validate_unique_indexes`
  (`unique`-rule ⟹ unique-index, иначе `unique_requires_index`). Зафиксировано
  нормативным two-layer контрактом в коде + coherence-тестами (②.3b). probe НЕ
  снят (снятие потеряло бы чистую ошибку/семантику ради мнимого выигрыша). См.
  `DONE.md`, `DDL-EVOLUTION-PLAN.md §②.3`.

> E6 (FK-actions `ON DELETE`) — реализовано как Phase D (RESTRICT/CASCADE/
> SET NULL + drop-guard), см. `DONE.md`. `ON UPDATE` — ✅ **СДЕЛАН в кампании ②.2**
> (`fk_on_update.rs`: no-op gate → Restrict/Cascade-rekey/SetNull на UPDATE-пути,
> триггер «referenced value changed», single-field MVP). См. `DONE.md`,
> `DDL-EVOLUTION-PLAN.md §②.2`.

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
| **P0 — корректность/безопасность** | ✅ нет | A2 закрыт (Phase G.4) |
| **P1 — билдеры** | ✅ нет | B2, B4 сделаны (Phase G.1/G.2) |
| **P2 — эволюция языка** | ✅ нет | E5 unify-uniqueness закрыт (кампания ②.3) |
| **P3 — DX + досборка** | ✅ нет | B5–B7 (кампания ①), E2 DEFAULT (кампания ②.4) |

**Все P0–P3 закрыты.** **RENAME db** — ✅ сделан (②.1d, чистый каталог-rekey).
**(A)-мини-кампания** mutating/transform-валидаторов (computed-`DEFAULT`/
server-stamping) — ✅ сделана (**кампания ③.2**). **Из research-корпуса open-работы
не осталось.** Осознанно отложенное (отдельные кампании, НЕ блокеры): **Phase H —
репликация** (`PHASE-H-PLAN.md`, единственный незакрытый charter-пилон «I»),
AFTER/side-effecting триггеры (G13), perf group-commit.

> Выполненное (A1, A2, B1, B2, B3, B4, C1, C2, C3, D1, Phase D/E6, **вся кампания
> Phase E**: A3, D2, E3, E4, M5, F1–F5; **E.4-followon**: E1 полностью = F.1/F.2/F.3;
> **Phase G**: B2/B4/C3 + A2 = G.1–G.4; **кампания ① Builder parity**: B5–B7;
> **кампания ② DDL-эволюция**: E1-остаток (folder/group/role RENAME), E6 `ON
> UPDATE`, E5 unify-uniqueness, E2 `DEFAULT`) — в `DONE.md`.
