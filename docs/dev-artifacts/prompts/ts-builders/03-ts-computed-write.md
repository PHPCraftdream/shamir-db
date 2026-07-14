בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ①.5 — TS computed-write parity (B6)

Кампания **① Builder parity & DX**, этап ①.5 (B6). Источник:
`docs/dev-artifacts/research/coverage-ts-query-builder.md` G5. Дать TS-стороне **паритет с Rust
`write::Doc`**: возможность встраивать вычисляемые выражения (`$fn`/`$ref`/
`$query`) в значения write-операций (insert/update/upsert). Surface-only. Пакет:
`crates/shamir-client-ts`. Объём: M.

## Контекст (заземление)
- **Rust-образец** `crates/shamir-query-builder/src/write/doc.rs`: `Doc::set(key,
  value: impl Into<FilterValue>)` принимает литералы И выражения (`col`/`func`/
  `qref`); FilterValue и QueryValue делят один serde-wire-encoding (round-trip).
- **TS сейчас:** `write.insert(table, values)` / `UpdateBuilder.set(obj)` /
  `write.upsert(table, key, value)` берут `WireValue` (`src/core/types/write.ts:33`).
  Вычисляемое значение (`{ created_at: {$fn:"NOW"}, total: {$ref:"price"} }`)
  встроить нельзя без ручной сборки wire-формы.

## Что сделать (выбери самый идиоматичный для TS путь — приоритет ниже)

**Основное — расширить `WireValue`** (`src/core/types/write.ts`): добавить в
union вычисляемые FilterValue-формы (`{$fn}`/`{$ref}`/`{$query}`/`{$expr}`/
`{$cond}`/`{$param}`), чтобы значения в insert/update/upsert принимали выражения,
построенные существующими `filter.fn()`/`filter.ref()`/`filter.queryRef()`/
`filter.expr()`/`filter.cond()`/`filter.param()`. Цель — идиоматичный JS-литерал:
```ts
write.insert('events', { created_at: filter.fn('NOW'), total: filter.ref('price') })
```
- Расширение **аддитивно** (union extension) — НЕ ломай существующих WireValue-
  пользователей. Сверь рекурсивную форму WireValue (вложенные объекты/массивы),
  чтобы выражения допускались на любой глубине значения.
- Переиспользуй существующие `filter.*`-конструкторы (они уже дают `{$fn}` и т.п.)
  — НЕ дублируй формы.

**Опционально (если ложится в идиому)** — тонкий `doc()`-helper (паритет/
discoverability с Rust `Doc`), строящий объект значений с `.set(key, value)`.
Не обязателен, если расширения `WireValue` достаточно и оно чище.

## Бонус (опционально) — 4 pre-existing tsc-ошибки
В `src/__tests__/e2e-schema-validators.test.ts` (строки 55/82/109/110) есть 4
pre-existing ошибки `Record<string, unknown>` не присваивается `WireValue` —
ровно эта write-value область. **Если** чистое расширение `WireValue` делает их
присваиваемыми — хорошо, ошибки уйдут. **Если** для них нужен отдельный фикс
типа в тест-файле (типизировать `record` как `WireValue`) — это вне scope B6,
НЕ трогай тест, просто отметь в финальном тексте. Не вноси НОВЫХ ошибок.

## Тесты (обязательно)
Wire-shape unit-тесты в `src/core/builders/__tests__/write.test.ts` по образцу
соседних:
- `insert` с вычисляемым значением → `{ ..., values:[{ created_at:{"$fn":"NOW"} }] }`;
- `update().set({...})` с `$ref` → корректная wire-форма;
- `upsert` value с вычисляемым;
- литералы по-прежнему работают (регрессия не сломана).

## Гейт (прогнать самому, всё зелёное)
```
cd crates/shamir-client-ts
npx vitest run write        # + полный npx vitest run (всё зелёное, кроме pre-existing server-gated e2e)
npx tsc --noEmit            # число ошибок: было 4 (e2e-schema-validators), должно стать ≤4. НИКОГДА >4.
```
Сервер не нужен (unit wire-shape).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или любую мутирующую git-команду. Только редактируй файлы; коммитит оркестратор.
- Surgical: расширение типа + (опц.) helper + тесты. Аддитивно, без слома существующих WireValue-юзеров. Импорты — в шапку.
- Заверши финальным текстом: file:line изменённого + вывод `npx vitest run write` (PASS) + tsc (число ошибок до/после) + судьба 4 pre-existing ошибок.
