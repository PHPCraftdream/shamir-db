# Brief: OQL Epic 03 / Phase D — юнит-тесты conditional execution gap closure (task #647)

## Контекст

Фазы A-C (#644 ADR, #645 `997532cc`, #646 `1dd6e57c`) уже реализовали и
добавили базовое покрытие: `QueryEntry.when`, каскадный skip, `Batch::switch`/
`switchCase`. Существующие тесты:
`crates/shamir-engine/src/query/batch/tests/executor_tests/when_skip_tests.rs`
(4 теста), `crates/shamir-query-builder/src/batch/tests/when_tests.rs`,
`crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts`
(switchCase-секция).

**Эта задача — НЕ повторение уже сделанного.** Прочитай перечисленные
файлы, найди РЕАЛЬНЫЕ пробелы, закрой именно их.

## Известные кандидаты на пробелы

1. **Транзакционная семантика skip**: пропущенный WRITE-op (Insert/Update/
   Delete с `when: false`) внутри ТРАНЗАКЦИОННОГО батча — подтверди тестом,
   что он НЕ попадает в tx write-set, commit проходит чисто, и что при
   COMMIT нет побочных эффектов от пропущенного write. Есть ли такой тест?
2. **Skip саб-батча**: `when` на алиасе, содержащем `BatchOp::Batch(SubBatchOp)`
   (Epic01's вложенный батч) — пропускает ли `when: false` ВСЮ рекурсию
   (внутренние op саб-батча не исполняются вообще)?
3. **`after`-ребро от skipped-op** — уже есть тест "after-only edge does
   NOT cascade" — но проверь: если `B` имеет И `after`-ребро на skipped `A`,
   И СОБСТВЕННЫЙ `when` (независимый, не зависящий от `A`) — `B` исполняется
   по своему `when`, `after`-от-skipped `A` просто означает отсутствие
   упорядочивающего ограничения (A выполнился бы раньше, если бы не был
   skipped, но раз он skipped — просто нет эффекта). Явно протестируй этот
   комбинированный случай.
4. **Множественный каскад (3+ уровня)**: `C` зависит от `B` (DataFlow),
   `B` зависит от `A` (DataFlow), `A` skipped → и `B`, и `C` каскадно
   skipped. Проверено ли это на цепочке длиной 3, не только 2?
5. **`skipped`-статус отличим от `return_only`-фильтрации** — тест,
   явно показывающий разницу: skipped alias присутствует в ответе с
   `skipped: true` (если `return_result: true`), а `return_result: false`
   алиас просто отсутствует из `results` (независимо от `when`).
6. **Rust `switch`/`switchCase`**: тест с 4+ ветками (не только 1-2, как
   уже покрыто) — подтверди корректность накопления `NOT`/`OR`-цепочки на
   большем числе веток.
7. **Ошибка при `when`-эвалюации** (например `when` ссылается на
   несуществующий `$fn` или даёт type mismatch) — какое поведение? (по
   аналогии с `$cond`-condition silent-miss из Epic02 — вероятно тоже
   `false`, не паника — подтверди тестом).

Не ограничивайся списком — если найдёшь другой пробел из ADR/роадмапа/
брифов A-C, закрой и его.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-engine -p shamir-query-builder -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings` (ПОЛНЫЙ workspace)
- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-query-builder --full`
- из `crates/shamir-client-ts`: `npx tsc --noEmit` и
  `npx vitest run src/core/builders`.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ меняй production-код — ТОЛЬКО тесты. Если найдёшь баг в продакшн-коде
  — ОПИШИ его подробно в отчёте (файл:строка, сценарий), НЕ исправляй сам.
- НЕ дублируй уже существующие тесты.

## Проверка (сделает оркестратор)

- Диф ограничен `tests/`-директориями + `__tests__/` в TS — НИ ОДНОГО
  изменения в продакшн-файлах.
- fmt/clippy чисты (включая полный workspace); полный тестовый гейт зелёный.
- Отчёт явно перечисляет: пробелы найдены/закрыты, что уже было покрыто,
  найденные-но-не-исправленные баги продакшн-кода.
