בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ①.3 — TS DX: Handle / tryBuild / deliverCall / inline where (B7)

Кампания **① Builder parity & DX**, этап ①.3 (B7). Источник:
`docs/dev-artifacts/research/coverage-ts-query-builder.md` G3/G4/G6/G1. Четыре независимых
DX-улучшения TS-билдера, где Rust удобнее. Surface-only. Пакет:
`crates/shamir-client-ts`. Объём: M (вместе). Делать все четыре.

Принцип: **переиспользуй существующие примитивы** (`filter.queryRef`, `filter.*`
leaf-конструкторы, `and/or`, `call()`), не дублируй логику путей/форм. Зеркаль
Rust-семантику (file:line ниже), но в TS-идиоме.

---

## G3 — типизированный `Handle` / `RowRef` (самый ценный)

**Rust-образец:** `crates/shamir-query-builder/src/batch/handle.rs`:
- `Handle { alias }`: `.column(field)` → `$query` путь `"[].field"` (nested:
  `"[].a.b"`); `.row(i)` → `RowRef`; `.first()` = `row(0)`; `.all()` → qref без
  пути.
- `RowRef { alias, index }`: `.field(f)` → `"[i].field"`; `.get()` → `"[i]"`.

**TS-цель:** новый файл `src/core/builders/handle.ts` — классы `Handle` и
`RowRef` с теми же методами; пути строятся через существующий
`filter.queryRef(alias, path)` (НЕ дублируй формат). `field` принимает
`string | string[]` (nested → join '.').
- `Handle.column(field)` → `queryRef(alias, '[].' + dotted)`;
- `Handle.row(i)` → `RowRef`; `.first()` → `row(0)`; `.all()` → `queryRef(alias)` (без пути);
- `RowRef.field(f)` → `queryRef(alias, '[' + i + '].' + dotted)`; `.get()` → `queryRef(alias, '[' + i + ']')`.

**⚠ Compat-инвариант (НЕ сломать):** `Batch.add()` сейчас возвращает `this`
(чейнинг `.add(...).add(...)` — `builders/batch.ts:79`). НЕ меняй возврат
`add()`. Вместо этого добавь **фактори** `Batch.handle(alias: string): Handle`
(и/или `Batch.ref(alias)`) — возвращает `Handle` для уже зарегистрированного
алиаса. Так типобезопасные `$query`-пути доступны, а чейнинг цел.

---

## G4 — `Batch.tryBuild()` валидация

**Rust-образец:** `Batch::try_build` (`crates/shamir-query-builder/src/batch/batch.rs`)
— проверяет `$query`-ref алиасы и `after`-зависимости на build-time.

**TS-цель:** метод `Batch.tryBuild(): BatchRequest` в `builders/batch.ts`.
Собери объявленные алиасы (ключи `queries`-мапы построенного `BatchRequest`).
Обойди каждую op: найди `$query`-рефы (форма `{ $query: alias, path? }` в значениях
фильтров — рекурсивно по filter-дереву) и `after`-массивы. Брось описательную
`Error`, если: (а) рефнутый алиас не объявлен; (б) `after`-зависимость
ссылается на несуществующий алиас. На успехе — верни построенный `BatchRequest`
(как `build()`). `build()` оставь без изменений (unchecked); `tryBuild()` =
валидированный `build()`.

---

## G6 — `subscribe` deliver_call

**TS-цель:** `builders/subscribe.ts`. Сейчас `SubscribeSource` поддерживает
`deliver: 'records'|'keys'` и `handle` (sub-batch); `resolveDeliverMode`
(`subscribe.ts:67`) их разбирает. Добавь `call?: CallOp` (или опцию
`deliverCall`) → `DeliverMode.Call`. Wire-тип `{ call: CallOp }` уже есть
(`types/subscribe.ts:44`). Переиспользуй существующий `call()`-билдер для
`CallOp`. Взаимоисключение с `deliver`/`handle` — расширь существующую
conflict-проверку.

---

## G1 — inline `whereEq` / `whereGt` / … + `whereGroup` (низший приоритет)

**Rust-образец:** макрос `where_methods!` (`query/conds.rs` / `query.rs`) —
24+ inline filter-and-combine метода.

**TS-цель:** `Query` (`builders/query.ts`). Добавь inline-методы, строящие leaf
`Filter` через существующие `filter.*` и AND-комбинирующие в `where` (как
`andWhere`): `whereEq/whereNe/whereGt/whereGte/whereLt/whereLte/whereIn/whereLike`
(+ что есть у Rust). Плюс OR-варианты `orWhereEq/...`. Плюс
`whereGroup(cb)`/`whereGroupOr(cb)` для вложенных групп. **Переиспользуй**
`filter.*` + `and/or`, не дублируй. Это самый низкоприоритетный пункт — свободные
`filter.*` идиоматичны; но цель кампании — полнота, делай аккуратно.

---

## Тесты (обязательно, по каждому пункту)
Wire-shape unit-тесты по образцу соседних, в `src/core/builders/__tests__/`:
- Handle/tryBuild → `batch.test.ts`;
- deliverCall → `subscribe.test.ts`;
- inline where → `query.test.ts`.
Каждый строит и `toEqual` точную wire-форму. Для tryBuild — тест на throw при
битом `$query`-алиасе И на успех при валидном.

## Гейт (прогнать самому, всё зелёное)
```
cd crates/shamir-client-ts
npx vitest run            # все builder unit-тесты — БЕЗ сервера
```
+ typecheck (`npx tsc --noEmit` или скрипт из package.json). **Не добавляй новых
tsc-ошибок**: на master есть РОВНО 4 pre-existing ошибки в
`src/__tests__/e2e-schema-validators.test.ts` — их не трогай, но и новых не
вноси (сверь число до/после).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или любую мутирующую git-команду. Только редактируй файлы; коммитит оркестратор.
- Surgical: 4 фичи + тесты. НЕ ломай `Batch.add()` чейнинг. Импорты — в шапку. Один файл = один primary export (Handle/RowRef → handle.ts).
- Queries — только через билдеры. Заверши финальным текстом: file:line по каждому из 4 пунктов + вывод `npx vitest run` (PASS-строки) + tsc (число ошибок до/после, должно остаться 4).
