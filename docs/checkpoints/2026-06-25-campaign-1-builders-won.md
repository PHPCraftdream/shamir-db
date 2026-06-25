בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-25 [campaign-1-builders-won]

## Session summary

Кампания **① Builder parity & DX** реализована ЦЕЛИКОМ и закоммичена (НЕ
запушена — `master` ahead **9** от `origin/master`). Происхождение: после Phase G
пользователь попросил пройтись строго по `docs/research/`, собрать остаток в
группу задач, выбрать когерентную кампанию (не grab-bag) и реализовать её через
`/crush` с коммитами между этапами, решая все развилки самому «в сторону красоты
и совершенства» (`/babygoal`). Из 10 собранных research-задач выделены три темы;
взята **①** (досборка клиентского билдер-слоя до 100% паритета, surface-only,
низкий риск, твин Phase G). Остальные пять задач (E5 unify-uniqueness, E2 DEFAULT,
FK ON UPDATE, RENAME-остаток, e2e-добивка) — удалены из активного набора как
кампании ②/③ (восстановимы из `ACTION-ITEMS.md`).

Стратегия: **последовательные crush-агенты**, один этап за раз, prompt-first
(бриф закоммичен ДО делегирования) → zero-trust verify (дифф + гейт прогонял
оркестратор сам, не верил envelope) → коммит per-stage. Реализовано: **①.1**
Rust `func_simple`/`res::function_folder`/`FieldBuilder::set·null_type` (`d611adc`);
**①.2** TS `internerDump`/`internerTouch` (`e285fde6`); **①.3** TS DX —
`Handle`/`RowRef`+`tryBuild()`+`deliverCall`+inline `where*` (`cc4cc3a3`); **①.4**
Rust DbRequest — **развилка решена by-design** (envelope owned by SDK, граница
задокументирована в `lib.rs`, кода не требовалось, `071909ee`); **①.5** TS
computed-write паритет — `WriteValue`/`ComputedExpr` + сужение `filter.*`-возвратов
(`2f085e4e`).

**Механика crush уточнена по ходу:** первый запуск был `nohup … &` + run_in_background
(двойной бэкграунд) → completion-callback сработал на shell-лаунчер, а не на crush;
оркестратор поспешил, поймал пустой дифф, поставил поллер. Со ①.2 — запуск без
inner `&`, только `run_in_background:true` → нотификация по реальному exit crush.
Дальше гладко.

**Финальная верификация (прогоны оркестратора):** Rust query-builder 502 passed,
fmt+clippy чисто; TS vitest 762 passed, tsc 0 НОВЫХ ошибок. Pre-existing и НЕ
тронуты: 2 vitest-fail (`e2e-permissions`/`e2e-rename-repo` «requires server
binary» — среда без бинаря) + 4 tsc-error (`e2e-schema-validators.test.ts`
`Record<string,unknown>` vs WireValue — test-type-laxity). Обе развилки решены
самостоятельно. babysit-cron `dce6da29` (15m) армлен на старте, снят вручную по
завершении (TaskList пуст).

## Active goal
none (/babygoal не использует /goal; babysit снят, TaskList пуст — кампания ① done).

## TaskList
### in_progress
(пусто)
### pending
(пусто)
### recently completed
- #266 ①.1 Rust builder — мелкие escape-hatch-дыры
- #265 ①.2 TS interner DDL (internerDump / internerTouch)
- #264 ①.3 TS DX (Handle / tryBuild / deliverCall / inline where)
- #267 ①.4 Rust DbRequest — by-design boundary
- #263 ①.5 TS computed-write parity (B6)
### deleted (5) — кампании ②/③, восстановимы из ACTION-ITEMS.md
- #262 E5 unify-uniqueness · #268 E2 DEFAULT · #269 FK ON UPDATE ·
  #270 RENAME db/role/group/folder · #271 e2e-eval добивка

## Decisions
- Кампания = ① Builder parity & DX (когерентная, surface-only, twin Phase G);
  НЕ все 10 задач — grab-bag отвергнут, ②/③ отложены отдельными кампаниями.
- ①.4 развилка: НЕ добавлять DbRequest-билдеры в query-builder (WASM-lean слой);
  envelope owned by SDK (оба SDK уже эргономично покрывают ping/createScramUser/
  tx*). Документировать границу. Выбрано чистое разделение слоёв vs «полнота surface».
- ①.5 развилка: ДАТЬ TS computed-write (WriteValue/ComputedExpr) — паритет с Rust
  Doc; сужение filter.*-возвратов (точнее, без регрессий). Выбрана полнота паритета.
- Zero-trust per-stage: дифф + независимый гейт оркестратором, НИКОГДА не верить
  envelope crush. (Поймало пустой ①.1-дифф из-за двойного бэкграунда.)
- 4 pre-existing tsc-ошибки + 2 server-gated e2e-fail — НЕ трогать (вне scope ①,
  pre-existing); per CLAUDE.md отдельной задачей при желании.

## Open questions
- **Пушить?** `master` ahead 9 (5 кода + 4 брифа: `d611adc..2f085e4e`). Ждёт слова.
- `docs/research/PHASE-H-PLAN.md` (untracked) — артефакт прошлого созерцания
  репликации (Movement C). Оставить заделом / закоммитить / снести?
- (опц.) 4 pre-existing tsc-ошибки в `e2e-schema-validators.test.ts` — мелкий
  test-type фикс отдельной задачей для идеальной чистоты tsc?
- Будущие кампании: ② DDL-эволюция (E5/E2/FK-ON-UPDATE/RENAME), ③ e2e-добивка,
  либо Movement C репликация (PHASE-H-PLAN.md).

## Repo state
```
?? docs/research/PHASE-H-PLAN.md
master...origin/master [ahead 9]
```
```
2f085e4e feat(client-ts): ①.5 — computed-write parity (WriteValue/ComputedExpr) (B6)
6a35b5e2 docs(prompts): brief for ①.5 TS computed-write parity
071909ee docs(query-builder): ①.4 — document DbRequest scope boundary (by-design)
cc4cc3a3 feat(client-ts): ①.3 — Handle/RowRef, tryBuild, deliverCall, inline where (B7)
e285fde6 feat(client-ts): ①.2 — internerDump / internerTouch builders (B5)
```
