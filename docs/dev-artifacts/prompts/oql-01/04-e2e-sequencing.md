# Brief: OQL Epic 01 / Phase D — e2e Rust+TS sequencing (task #631)

## Контекст

Роадмап: `docs/dev-artifacts/roadmap/oql/01-sequencing-explicitness.md`, Фаза D.
Фазы A/B/C (#628 `b65b4940`, #629 `72dca050`, #630 `851bf79e`) уже
реализовали и юнит-протестировали: `EdgeKind` (Explicit/DataFlow/Both)
провенанс рёбер DAG, `after` больше не открывает данные, `AfterPathIgnored`
ошибку, fluent `*_after` в Rust-билдере, нормализацию в TS. Эта фаза —
**сквозные (end-to-end) тесты через реальный wire-протокол** (настоящий
сервер, настоящий TCP/WS клиент), а не unit-тесты внутри одного крейта.

## Задача

### 1. Rust e2e

Найди подходящее место среди существующих e2e-наборов
(`crates/shamir-db/tests/` — например рядом с `ddl_wire_e2e/`, или
`crates/shamir-server/tests/db_handler.rs` — grep существующие батч-e2e
тесты как образец структуры/хелперов) и добавь тест:

- Батч из 5 операций со смешанными `after` + `$query` зависимостями
  (например: `create_table` → (after) → `insert` → ($query) → `read` →
  (after) → `update` → ($query на update) → `read2`), исполненный через
  реальный wire round-trip (не in-process вызов планировщика).
- Проверь, что ответ (`BatchResponse`/`execution_plan`) СОДЕРЖИТ провенанс
  рёбер (`edge_provenance`) с ожидаемыми `Explicit`/`DataFlow`/`Both`
  значениями для каждой пары.
- Проверь, что чисто-`after`-зависимость (без `$query`) НЕ даёт зависимому
  op доступа к данным предыдущего (regression e2e для Фазы A, пункт 3 —
  подтверди это уже НЕ через unit, а через реальный сервер).
- Отдельный тест: батч с циклом (смесь `after`+`$query` образующих цикл) —
  сервер возвращает `CircularDependency`-ошибку через wire (не 500/паника).

### 2. TS e2e

`crates/shamir-client-ts/src/__tests__/` — используй существующий
`e2e-harness.ts` (grep как стартуется тестовый сервер в других
`e2e-*.test.ts` файлах, повтори паттерн). Новый файл, например
`e2e-batch-sequencing.test.ts`:

- Тот же сценарий смешанной цепочки, но через TS-билдер (`Batch` из
  `builders/batch.ts`, включая новые fluent `*_after`-подобные вызовы, если
  TS получил свои — проверь, что реализовала Фаза B; если TS-эквивалента
  fluent-метода нет, используй существующий `opts.after`).
- Перенос примера между клиентами: тот же батч, что в Rust e2e (по смыслу),
  должен вести себя идентично — `after: ["@mk"]` (с `@`-префиксом, который
  TS теперь нормализует после Фазы B) работает так же, как без `@`.
- Проверка `edge_provenance` в TS-ответе (типизированный доступ, если типы
  wire-ответа уже включают это поле — проверь `types/`, возможно потребуется
  добавить/обновить TS-тип `BatchResponse` под новое поле `edge_provenance`
  из Фазы A; если тип уже кем-то добавлен — просто используй).

### 3. Если TS wire-типы не знают про `edge_provenance`

Если `crates/shamir-client-ts/src/core/types/` не содержит поля
`edge_provenance` в типе ответа батча — добавь его (опциональное поле,
зеркалящее Rust `BatchResponse.edge_provenance`, сериализованный как
`TMap<String, TMap<String, EdgeKind>>` → JS-объект строка→объект с
`"explicit"`/`"data_flow"`/`"both"` значениями, судя по `#[serde(rename_all
= "snake_case")]` на `EdgeKind`). Это МИНИМАЛЬНОЕ типовое расширение, нужное
только чтобы e2e-тест мог типизированно прочитать поле — не расширяй объём
работы сверх этого.

## Прогон проверок

- Rust e2e: `./scripts/test.sh -p shamir-db --full` и/или
  `-p shamir-server --full` (в зависимости от того, куда добавил тест).
- TS e2e: из `crates/shamir-client-ts` — сначала `cargo build --release -p
  shamir-server` (CARGO_TARGET_DIR=D:\dev\rust\.cargo-target) чтобы снять
  stale-binary guard, ЗАТЕМ `npm test`. Если сборка release долгая — это
  ожидаемо, дождись завершения.
- `cargo fmt`/`cargo clippy -- -D warnings` на затронутых Rust-крейтах.
- `npx tsc --noEmit` на TS.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай production-код Фаз A/B (planner.rs, batch_execute.rs, batch.ts,
  batch.rs) — если что-то не работает так, как ожидается по описанию Фаз
  A/B, это может быть баг — ОПИШИ его в отчёте, не исправляй молча внутри
  этой e2e-задачи.
- НЕ занимайся бенчмарками (Фаза E, #632) или доками (Фаза F, #633).

## Проверка (сделает оркестратор)

- Диф — новые/изменённые файлы в `crates/shamir-db/tests/` или
  `crates/shamir-server/tests/`, `crates/shamir-client-ts/src/__tests__/`,
  возможно `crates/shamir-client-ts/src/core/types/` (edge_provenance поле).
- fmt/clippy чисты; e2e-тесты реально проходят (не просто компилируются).
