בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# TS-клиент: интернер-aware query builder + сквозное e2e-покрытие — поэтапный план

> Сделано в сторону красоты, удобства и совершенства — ради Всевышнего.

## Цель

1. **Полный TS query builder, который интернирует по-клиентски** — на провод идут
   интернированные `u64`-id полей записей (touch новых + pull существующих), ответы
   де-интернируются id→name. Паритет с Rust-клиентом (`execute_with_touch`).
2. **Сквозное e2e-покрытие** против реального сервера: весь DDL, права доступа,
   декларативные валидаторы, вся работа с данными — через провод (с интернированием).

## Принятые границы (решено)

- **Клиент пакует → id на проводе** (doc 04), билдер слеп к интернированию.
- Интернируются **ТОЛЬКО имена/пути полей ЗАПИСЕЙ** (ключи insert/upsert/set/update,
  рекурсивно по nested map). Имена сущностей (db/repo/table/index/function/role/user) —
  каталоговые ключи, **НЕ интернируются**. Фильтры/select/schema-пути — по Rust-референсу
  (Rust id-пакует только write-записи; фильтры идут строками).

## Эталон (спецификация)

`crates/shamir-client/src/interner_cache_ops.rs` — `execute_with_touch`:
collect_field_names (INSERT.values / SET.key+value / UPDATE.set, рекурсия) → touch_fields
per repo → v2: литеральные INSERT-записи → `records_idmsgpack` (id-keyed msgpack), `$fn` →
строками на `values`, `result_encoding=Id` → execute → `deintern_response`. + `interner_cache.rs`
(FieldMap). Серверный id-keyed decode записей — источник истины для формата байт.

---

## Фазы

### Фаза 1 — Клиентский interner-packing (smart-write path) [В РАБОТЕ, #208]

**Объём:** `encodeRecordIdMsgpack(record, fieldMap)` (string-keyed → id-keyed msgpack,
рекурсия по ключам), `qvHasFnMarker` (детект `$fn`), `executeWithTouch` (collect→touch→
records_idmsgpack→`result_encoding=Id`→deintern), `deinternResponse` (Id-rows→names через
idToName). Гейт v2-пути на серверной query-version; v1 — без изменений.
**Файлы:** `crates/shamir-client-ts/src/core/{client.ts, field-map.ts}` + помощник кодека.
**Тесты:** unit (encode round-trip плоский/nested, $fn-skip, deintern) + **e2e round-trip**
(новые имена полей → id на проводе → имена на чтении).
**Критично:** формат `encodeRecordIdMsgpack` обязан совпасть с серверным id-keyed decode —
иначе тихая порча данных. Свериться с Rust-энкодером и серверным RecordView.
**Acceptance:** `tsc build` + `vitest` зелёные; e2e доказывает id-на-проводе + де-интерн.

### Фаза 2 — Аудит полноты билдера + закрытие пробелов

**Объём:** свести TS-билдеры (`ddl.ts`/`write.ts`/`query.ts`/`filter.ts`/`admin.ts`) против
Rust-builder + серверной op-поверхности (`BatchOp`, `shamir-query-types`). Найти отсутствующие
ops/опции, дореализовать до паритета. Вход: список пробелов, отмеченный агентом Фазы 1.
**Тесты:** билдер-unit (форма wire-op) на новые/недостающие ops.
**Acceptance:** каждый серверный data/DDL/admin-op имеет TS-билдер; `tsc` + `vitest` зелёные.

### Фаза 3 — Общий e2e-харнесс

**Объём:** извлечь `e2e-harness.ts`: `startServer(opts)` с **уникальным/эфемерным портом**
(сейчас `PORT=13760` фиксирован → vitest гонит файлы параллельно → конфликт), `connectAdmin()`,
`connectAs(user, password)`, `setupDb`, `seed`, `br`, `uniqueDbName`. Рефактор существующего
`e2e.test.ts` на харнесс (держать зелёным). Это разблокирует параллельные e2e-файлы.
**Acceptance:** существующий e2e зелёный на новом харнессе; два e2e-файла не конфликтуют по порту.

### Фаза 4 — DDL lifecycle e2e [#209]

**Объём:** каждый DDL-op из `ddl.ts` сквозь сервер: create/list/drop для
db/repo/table/index/function/folder/validator; bind/unbind validator; buffer-config (set/alter/get);
retention (setRetention/purgeHistory/changesSince); migrations (start/commit/rollback/status);
schema DDL (setTableSchema/add/remove/getTableSchema). Паттерн: op → list отражает → drop убирает.
**Acceptance:** все DDL-ops покрыты; `vitest` зелёный.

### Фаза 5 — Декларативные валидаторы e2e [#210]

**Объём:** `setTableSchema` (type/min/max/required/one_of/nested) → insert валидного проходит /
невалидного → assert коды (`missing_required`/`out_of_range`/`type_mismatch`/…); `foreignKey` →
`fk_violation` + `fk_requires_index`; `unique` → `unique_violation`; scalar/format/compare;
`addSchemaRule` (начинает отклонять) / `removeSchemaRule` (перестаёт) / `getTableSchema`;
reconnect (персистентность). **Caveat:** FK/unique применяются ТОЛЬКО в tx-mode (ctx.db=Some) —
разобраться, как TS даёт tx-путь; документировать, что single-op autocommit их пропускает.
**Acceptance:** accept/reject + смена/удаление правил + FK/unique доказаны через провод.

### Фаза 6 — Права доступа / access-control e2e [#211]

**Объём:** createUser/createRole/grantRole/revokeRole; chmod/chown/chgrp + createGroup/
addGroupMember; второй клиент как непривилегированный → DDL запрещён + data запрещена
(`access_denied`); после grant → разрешено; accessTree/permission. Покрыть `admin.ts`.
**Acceptance:** denied-vs-allowed доказаны на DDL И data; коды прав корректны.

### Фаза 7 — Вся работа с данными e2e [#212]

**Объём:** дополнить сверх существующего `e2e.test.ts`: versioning (atVersion/atTimestamp),
upsert-семантика, batch-атомарность, delete-all, крупные/вложенные фильтры, projection/agg
edge-cases. Главное — данные идут через interner-packing Фазы 1 (id на проводе), e2e проверяет
round-trip (запись с именами → id на проводе → имена на чтении).
**Acceptance:** data-поверхность покрыта; round-trip через интернер доказан.

---

## Последовательность и зависимости

```
Ф1 (packing, #208) ─┐
                    ├─► Ф2 (полнота билдера) ─► Ф3 (харнесс) ─► Ф4 ─► Ф5 ─► Ф6 ─► Ф7
(Ф3 нужна всем e2e; Ф2 информируется пробелами Ф1; Ф4-7 последовательны)
```

- **Ф3 (харнесс) обязательна перед Ф4-7.** Ф2 можно частично вести параллельно Ф3 (разные файлы),
  но e2e-гейт (vitest поднимает сервер) НЕ параллелится между агентами → **агенты последовательны**.

## Стратегия исполнения

- **@o46l, по одному агенту на фазу, последовательно** (директива юзера + TS-гейт поднимает
  сервер на порту → параллельные агенты конфликтуют; worktree-изоляция избыточна для одного крейта).
- **Per-фаза:** агент прогоняет `tsc build` + `vitest` до зелёного; **мой zero-trust** (сам
  прогоняю e2e + проверяю round-trip/коды); **атомарный коммит** фазы (санкция по слову юзера).
- Реализация — агенты; **верификация — моя** (envelope = claim, не приёмка).

## Риски

1. **Формат id-keyed msgpack ≠ серверный decode** (Ф1) → тихая порча. Митигация: сверка с Rust-
   энкодером + серверным RecordView; e2e round-trip как доказательство.
2. **Порт-конфликт e2e** (Ф3) → флэйки. Митигация: эфемерный порт per-file.
3. **FK/unique только в tx-mode** (Ф5) → autocommit тихо пропускает. Известный предел Rust-Phase-C;
   документировать в e2e, не выдавать за полное покрытие.
4. **Полнота билдера** (Ф2) — риск «думали покрыли, а op нет». Митигация: аудит против серверного
   `BatchOp`, а не «на глаз».

## Не в объёме (отдельной санкцией)

- Закрытие autocommit-FK/unique дыры (провести resolver в implicit-tx) — Rust-сторона.
- Phase D+ referential actions (cascade/restrict).
- func-nav (#195, Rust Phase-0 follow-up).
