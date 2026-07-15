# Brief: OQL Epic 03 / Phase E — e2e Rust+TS conditional execution (task #648)

## Контекст

Фазы A-D (#644-#647) реализовали и протестировали `QueryEntry.when`,
каскадный skip, `Batch::switch`/`switchCase`. Эта фаза — сквозные (e2e)
тесты через реальный wire-протокол, по образцу
`crates/shamir-client/tests/batch_cond_e2e.rs` (Epic02/D) и
`crates/shamir-client/tests/batch_sequencing_e2e.rs` (Epic01/D).

## Задача

### 1. Rust e2e — `crates/shamir-client/tests/batch_when_e2e.rs`

Канонический сценарий из ADR/роадмапа: транзакционный батч "прочитай
баланс → if достаточно: Insert списания, else: Insert отказа":

- Insert счёта с балансом (`seed`).
- Read баланса (`balance_check`).
- `Batch::when` на Insert "списание" с условием `balance >= amount`
  (используя `$query`-реф на `balance_check`).
- `Batch::when` на Insert "отказ" с комплементарным условием (или через
  `Batch::switch` — предпочтительно, раз это ровно тот сценарий, для
  которого он создан).
- Всё внутри `transactional()`.
- Проверка: при достаточном балансе — insert списания выполнился (записи
  есть), insert отказа skipped (`skipped: true` в ответе). При
  недостаточном — наоборот.
- Реальный wire round-trip, оба исхода как отдельные тесты/сценарии.

Отдельный тест: `switch` с 3 ветками (по аналогии с vip/regular/newbie из
Epic02, но теперь как условное ИСПОЛНЕНИЕ трёх РАЗНЫХ op, не просто
условное значение) — реальный сервер выбирает ровно одну ветку.

### 2. TS e2e — `crates/shamir-client-ts/src/__tests__/e2e-when.test.ts`

Тот же сценарий (баланс/списание/отказ) через TS-билдер (`add(alias, op, {
when })`/`switchCase`), по образцу `e2e-cond.test.ts` (Epic02/D) — используй
`e2e-harness.ts`.

## Прогон проверок

- Rust e2e: `./scripts/test.sh -p shamir-client --full`.
- TS e2e: проверь mtime release-бинарника сервера (может быть уже свежий
  после предыдущих фаз этой сессии); если stale — пересобери
  `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target cargo build --release -p
  shamir-server` (может занять несколько минут).
- `cargo fmt`/`cargo clippy --all-targets -- -D warnings` на затронутых
  Rust-крейтах.
- `npx tsc --noEmit` на TS.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай production-код Фаз A-D — если что-то не работает, как
  ожидается, ОПИШИ баг, не исправляй молча.
- Если увидишь ошибочную директорию вроде `devrust.cargo-target` из-за
  бага экранирования backslash в Git Bash при использовании
  `CARGO_TARGET_DIR` — удали её перед сдачей, не включай в диф.

## Проверка (сделает оркестратор)

- Новые файлы: `crates/shamir-client/tests/batch_when_e2e.rs`,
  `crates/shamir-client-ts/src/__tests__/e2e-when.test.ts`.
- fmt/clippy чисты; e2e-тесты реально проходят против настоящего сервера.
