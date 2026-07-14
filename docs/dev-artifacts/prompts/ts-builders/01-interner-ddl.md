בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# ①.2 — TS interner DDL (internerDump / internerTouch)

Кампания **① Builder parity & DX**, этап ①.2 (B5). Источник:
`docs/dev-artifacts/research/coverage-ts-query-builder.md` G7. Rust имеет
`ddl::interner_dump`/`interner_touch` + wire-типы; TS — ноль **user-facing**
покрытия. Добавить wire-типы + два билдер-метода. Surface-only, тонкие обёртки.

Пакет: `crates/shamir-client-ts`. Объём: S.

> **Уточнение (claim завышен):** TS уже шлёт `interner_touch` под капотом
> smart-write (`src/core/client.ts:544`) и обрабатывает `interner_dump`
> (`src/core/field-map.ts`). Недостаёт именно **явного билдер-метода**. Это
> тонкая обёртка над уже работающими wire-формами — НЕ трогай интернер-кэш,
> smart-write, field-map.

## Точная wire-форма (сверено с Rust `shamir-query-types`)
```
{ "interner_dump": "<repo>" }                  // repo default "main"
{ "interner_dump": "<repo>", "since": <u64> }  // since опционален, опускается если null
{ "interner_touch": "<repo>", "names": ["age","name"] }
```
Discriminator-ключ = имя repo (как `changes_since`/`purge_history`/`set_retention`
— интернер живёт на RepoInstance, repo — измерение маршрутизации, не таблица).

## Что сделать

### (1) Wire-типы — `src/core/types/ddl.ts`
Добавить по образцу соседних DDL-op-типов (`ChangesSinceOp`, `SetRetentionOp`):
```ts
export interface InternerDumpOp { interner_dump: string; since?: number }
export interface InternerTouchOp { interner_touch: string; names: string[] }
```
(сверь точное имя/стиль с тем, как типизированы соседние op-ы в этом файле —
union BatchOp, naming-конвенции).

### (2) Билдеры — `src/core/builders/ddl.ts`
Добавить по образцу `changesSince(...)` (builders/ddl.ts:~382) и `setRetention`:
```ts
export function internerDump(opts?: { repo?: string; since?: number }): InternerDumpOp {
  const op: InternerDumpOp = { interner_dump: opts?.repo ?? 'main' };
  if (opts?.since != null) op.since = opts.since;
  return op;
}
export function internerTouch(names: string[], opts?: { repo?: string }): InternerTouchOp {
  return { interner_touch: opts?.repo ?? 'main', names };
}
```
(подгони под фактический стиль файла: как соседи возвращают op, как экспортируются,
default repo — подтверди что `'main'` совпадает с конвенцией changesSince).

### (3) Unit-тесты — `src/core/builders/__tests__/ddl.test.ts`
Добавить wire-shape тесты (`toEqual({...})`) по образцу соседних, exhaustive:
- `internerDump()` → `{ interner_dump: 'main' }`;
- `internerDump({ repo: 'archive', since: 12 })` → `{ interner_dump: 'archive', since: 12 }`;
- `internerDump({ repo: 'x' })` — **since отсутствует** (не `undefined`-ключ);
- `internerTouch(['age','name'])` → `{ interner_touch: 'main', names: ['age','name'] }`;
- `internerTouch(['age'], { repo: 'archive' })` → `{ interner_touch: 'archive', names: ['age'] }`.

## Гейт (прогнать самому, всё зелёное)
```
cd crates/shamir-client-ts
npx vitest run ddl          # builder unit-тесты ddl.test.ts — БЕЗ сервера
```
Плюс typecheck — найди скрипт в `crates/shamir-client-ts/package.json`
(`npm run typecheck` / `npm run build` / `npx tsc --noEmit`) и прогони его зелёным.
Сервер НЕ нужен (это unit wire-shape, не e2e).

## ЖЁСТКИЕ ПРАВИЛА
- ⛔ НЕ используй инструмент agent/sub-agent — падает context-canceled. Читай файлы напрямую.
- ⛔ NEVER `git reset`/`checkout`/`clean`/`stash`/`restore`/`rm` или любую мутирующую git-команду. Только редактируй файлы; коммитит оркестратор.
- Surgical: 2 типа + 2 билдера + тесты. НЕ трогай интернер-кэш/smart-write/field-map. Импорты — в шапку.
- Заверши финальным текстом: file:line добавленного + вывод `npx vitest run ddl` (PASS-строки) + результат typecheck.
