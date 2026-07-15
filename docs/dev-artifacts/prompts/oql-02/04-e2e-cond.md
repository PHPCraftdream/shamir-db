# Brief: OQL Epic 02 / Phase D — e2e Rust+TS $cond (task #638)

## Контекст

Роадмап: `docs/dev-artifacts/roadmap/oql/02-cond-value-evaluation.md`, Фаза D.
Фазы A (#635 `cdcfc0f3`), B (#636 `28f265f7`), C (#637 `6ebaa8c3`) уже
реализовали и юнит-протестировали эвалюацию `$cond`/`$expr` в
`resolve_filter_query`, `switch_case`-сахар в билдерах. Эта фаза — сквозные
(e2e) тесты через реальный wire-протокол.

**ВАЖНАЯ ПОПРАВКА к исходному роадмапу**: план изначально предполагал
сценарий "$cond в SET-значении Update" — но задача #641 (найдена в Фазе B)
обнаружила, что `$cond` СЕГОДНЯ НЕ компонуется в write-значения
(`UpdateOp.set`/`SetOp.value` типизированы как `QueryValue`, структурно не
принимающий `FilterValue::Cond`). Это отдельная, ещё не решённая задача.
**Используй вместо этого `$cond`/`switch_case` в WHERE-фильтре** (полностью
рабочий путь уже сегодня) — например: `db.query('users').where($cond-based
condition сравнивающая вычисленное значение)` или fetch с `$cond` в
значении сравнения (`filter.eq('tier', cond(...))`).

## Задача

### 1. Rust e2e

По образцу `crates/shamir-client/tests/batch_sequencing_e2e.rs` (реальный
`ServerLauncher` + TCP + SCRAM, из Epic01/D) — новый файл
`crates/shamir-client/tests/batch_cond_e2e.rs`:

- Батч: insert нескольких `users` с полем `score`, затем `read`-запрос с
  WHERE-фильтром, где значение сравнения — `switch_case`/`cond` (например
  vip/regular/newbie классификация по `score`, приведённая в докстрингах
  `cond()`/`switch_case()` как канонический пример) — вычисленное через
  `$cond` значение сравнивается с полем записи, реальный сервер должен
  вернуть корректно отфильтрованные записи.
- Отдельный тест: вложенный `$cond` (3 уровня, тот же switch-case паттерн)
  через реальный wire — подтверди, что движок вычисляет его корректно (не
  просто заглушку/дефолт).
- Отдельный тест: `$cond`, чья ветка — `$query`-реф на результат
  предыдущего запроса в батче (кросс-query условное значение) — реальный
  wire round-trip.

### 2. TS e2e

`crates/shamir-client-ts/src/__tests__/e2e-cond.test.ts` (по образцу
`e2e-batch-sequencing.test.ts` из Epic01/D — используй `e2e-harness.ts`).
Тот же vip/regular/newbie switch-case сценарий через TS-билдер
(`filter.switchCase`/`filter.cond` из Фазы B), проверка, что e2e-результат
совпадает с ожидаемым.

## Прогон проверок

- Rust e2e: `./scripts/test.sh -p shamir-client --full`.
- TS e2e: из `crates/shamir-client-ts` — если release `shamir-server`
  бинарник уже свежий (проверь mtime, возможно уже собран из Epic01/D) —
  просто `npm test`; если stale — собери
  `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target cargo build --release -p
  shamir-server` сначала (может занять несколько минут, это ожидаемо).
- `cargo fmt`/`cargo clippy -- -D warnings` на затронутых Rust-крейтах.
- `npx tsc --noEmit` на TS.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ пытайся протестировать `$cond` в write SET-значениях — это невозможно
  сегодня (см. поправку выше), не в scope, отдельная задача #641.
- НЕ трогай production-код Фаз A/B/C — если что-то не работает так, как
  ожидается, это может быть баг — ОПИШИ его в отчёте, не исправляй молча.

## Проверка (сделает оркестратор)

- Новые файлы: `crates/shamir-client/tests/batch_cond_e2e.rs`,
  `crates/shamir-client-ts/src/__tests__/e2e-cond.test.ts`.
- fmt/clippy чисты; e2e-тесты реально проходят против настоящего сервера
  (не просто компилируются).
