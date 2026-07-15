# OQL Roadmap 01 — Sequencing: явность и симметрия зависимостей

Основание: `docs/dev-artifacts/research/oql/02-sequencing-dependencies.md`
(10 болевых точек). Механизм КОРРЕКТЕН — план не меняет семантику DAG,
только устраняет неявность, асимметрию и расхождение доков с рантаймом.

Зависимости: нет (независим от планов 02–04). Рекомендуемый порядок: первым —
дешёвый, разгребает почву под 03/04 (те расширяют планировщик).

---

## Фаза A — OQL-движок / query-types

1. **Пометить происхождение рёбер DAG.** `BatchPlan.dependencies` хранит
   источник ребра: `Explicit` (after) | `DataFlow` ($query). Ошибки
   (`CircularDependency`) и `execution_plan` в ответе показывают источник.
   Файл: `crates/shamir-query-types/src/batch/planner.rs`.
2. **`after` перестаёт открывать данные.** `build_resolved_refs` передаёт
   зависимому op'у результаты только `DataFlow`-рёбер; `after`-ребро — чистый
   ordering. (Никаких релизов не было — обратную совместимость не соблюдаем.)
   Файл: `crates/shamir-engine/src/query/batch/batch_execute.rs:293-295`.
3. **Решить судьбу параллелизма стадий** (одно из двух, зафиксировать ADR):
   - (a) честный `tokio::spawn`-per-query внутри стадии для read-only стадий,
     или (b) исправить доки планировщика: стадии — логическая группировка,
     исполнение последовательное. Минимум — (b); (a) — опционально после
     бенчей Фазы E.
4. **Нормализация `after`-строк** — единая функция `extract_base_alias`
   экспортируется и переиспользуется всеми валидаторами; мусорный path в
   `after` (`"mk[0].id"`) → предупреждение или ошибка, а не тихий стрип.

## Фаза B — Query builders

**Rust** (`crates/shamir-query-builder/src/batch/batch.rs`):
- Fluent-вариант на entry: `b.query("x", q).after(&mk)` (метод на handle
  регистрации), сохранив существующий `b.after(&dep, &on)`; в докстринге —
  правило одной фразой: «`$query` создаёт ребро сам; `after` — только для
  op без потока данных».

**TS** (`crates/shamir-client-ts/src/core/builders/batch.ts`):
- `tryBuild()` нормализует `after`-строки той же логикой, что сервер
  (стрип `@`, срез `[`/`.`) — паритет с Rust `try_build`.
- Валидация `after`/`$query` переносится из opt-in `tryBuild()` в основной
  `build()` (релизов не было — ломаем свободно), `tryBuild` остаётся алиасом.
- Рассмотреть переименование batch-`after` (коллизия с keyset-`after` в
  Query) — решить при ревью, зафиксировать в ADR.

## Фаза C — Юнит-тесты

- planner: источник ребра в `BatchPlan` (Explicit vs DataFlow), ошибки с
  указанием источника; нормализация after-строк; мусорный path → ошибка.
- engine: `after`-ребро НЕ даёт `$query`-доступ (регрессия к пункту A2).
- Rust builder: новый fluent `after`; TS: паритет нормализации,
  build()-валидация.
- Места: `crates/shamir-query-types/src/batch/tests/planner_tests.rs`,
  `crates/shamir-engine/src/query/batch/tests/`,
  `crates/shamir-query-builder/src/batch/tests/`,
  `crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts`.

## Фаза D — E2E

- **Rust e2e** (`crates/shamir-db`/`shamir-server` tests): цепочка 5 op со
  смешанными `after`+`$query`; проверка `execution_plan` в ответе (источники
  рёбер); ошибка цикла через wire.
- **TS e2e** (`crates/shamir-client-ts/src/__tests__/`): тот же сценарий
  через TS-билдер; перенос примера между клиентами (`after: ["@mk"]`)
  работает одинаково.

## Фаза E — Бенчмарки

- `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench <batch-bench>`:
  бейзлайн батча 10/50 независимых read-op ДО решения A3; если выбран (a) —
  подтвердить выигрыш ≥20% на независимых стадиях, иначе оставить (b).
- Новый bench обязан вызывать `shamir_bench_utils::tune(...)`.

## Фаза F — Доки

- `docs/guide-docs/`: раздел «Порядок исполнения батча» — правило
  одной фразой, примеры смешанной цепочки, семантика after vs $query.
- Обновить doc-комментарии planner.rs (убрать обещание параллелизма или
  описать его реальные границы).
- ADR: решение по параллелизму + по переименованию `after`.

## Критерии готовности

- Все 10 болевых точек отчёта 02 закрыты кодом, доками или явным ADR.
- Гейт: fmt/clippy/`./scripts/test.sh --full` + `npm test` (TS) зелёные.
