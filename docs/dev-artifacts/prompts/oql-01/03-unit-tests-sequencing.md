# Brief: OQL Epic 01 / Phase C — юнит-тесты sequencing (task #630)

## Контекст

Роадмап: `docs/dev-artifacts/roadmap/oql/01-sequencing-explicitness.md`, Фаза C.
Фазы A (#628, `b65b4940`) и B (#629, `72dca050`) УЖЕ добавили существенное
покрытие как часть своей реализации:
- `crates/shamir-query-types/src/batch/tests/planner_tests.rs` — edge
  provenance (Explicit/DataFlow/Both), AfterPathIgnored (bracket/dot).
- `crates/shamir-engine/src/query/batch/tests/planner_tests.rs` — то же на
  уровне движка через wire msgpack.
- `crates/shamir-engine/src/query/batch/tests/executor_tests/query_runner_tests.rs` —
  `build_resolved_refs` напрямую (Explicit исключён, DataFlow включён, Both
  включён).
- `crates/shamir-engine/src/query/batch/tests/executor_tests/query_refs_tests.rs` —
  `after_only_dep_does_not_resolve_query_ref` (сквозной тест через
  `execute_batch`).
- `crates/shamir-query-builder/src/batch/tests/after_tests.rs` —
  `try_build_catches_after_path_tail`, fluent `*_after` эквивалентность.
- `crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts` —
  нормализация `@`/path-tail, garbage path rejection.

**Эта задача — НЕ повторение уже сделанного.** Твоя работа: (1) прочитать
все перечисленные файлы, (2) найти РЕАЛЬНЫЕ пробелы в покрытии (что описано
в роадмапе/брифах A и B, но не протестировано никем), (3) закрыть именно их.
Если после ревью пробелов не найдено — явно напиши об этом в отчёте вместо
того чтобы писать тавтологичные тесты.

## Известные кандидаты на пробелы (проверь каждый, добавь тест если
отсутствует)

1. **Смешанная цепочка 3+ op**: `after` + `$query` в одной цепочке из ≥3
   алиасов (A → B по `$query`, B → C по `after`, C → D по обоим сразу) —
   есть ли такой тест где-либо? Если только попарные — добавь один
   сквозной multi-hop.
2. **Цикл, образованный ЧИСТО `after`-рёбрами** (без единого `$query`) —
   `CircularDependency` должен всё равно сработать. Проверь, есть ли тест
   именно на цикл из одних `after` (не смешанный).
3. **`AfterPathIgnored` при self-reference одновременно** — если `after`
   ссылается на path-хвост СВОЕГО ЖЕ алиаса (`after: ["self[0].id"]`) — какая
   ошибка побеждает (`AfterPathIgnored` или `SelfReference`)? Задокументируй
   реальное поведение тестом, если оно не очевидно/не покрыто.
4. **`edge_provenance` в сериализованном `BatchResponse`** (wire-уровень,
   не внутренний `BatchPlan`) — есть ли тест, что клиент реально ПОЛУЧАЕТ
   провенанс через настоящий сервер-round-trip (не просто unit на
   `BatchPlan`)? Если только внутренний план протестирован — добавь
   integration-тест на `BatchResponse.edge_provenance` сериализацию/десериализацию.
5. **TS `build()` теперь throws** (было: только `tryBuild()`) — проверь, что
   ВСЕ существующие TS-тесты, которые раньше полагались на "build() тихо
   принимает мусор", либо обновлены, либо явно проверяют новое поведение.
   Grep `\.build()` в тестах batch.ts на предмет пропущенных мест.
6. **Rust `Batch::after` (пост-фактум) vs fluent `*_after`** — тест, что оба
   способа дают ИДЕНТИЧНЫЙ `BatchRequest` на выходе (не просто "оба
   работают", а именно эквивалентность сериализованного результата).
7. **Именование коллизии `after`** (Query keyset vs Batch dependency) — есть
   ли тест/пример, показывающий, что оба метода сосуществуют в одном файле
   без конфликта компиляции (не баг, но хороший regression guard против
   будущего рефакторинга, который случайно смержит два типа)?

Не ограничивайся этим списком — если при чтении существующих тестов найдёшь
другой явный пробел из роадмапа/брифов A/B, закрой и его.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-engine -p shamir-query-builder -- --check`
- `cargo clippy -p shamir-query-types -p shamir-engine -p shamir-query-builder --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-query-builder --full`
- из `crates/shamir-client-ts`: `npx tsc --noEmit` и `npm test` (unit-тесты
  зелёные, e2e stale-binary failures допустимы).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ меняй продакшн-код (planner.rs, batch_execute.rs, batch.ts и т.п.) —
  ТОЛЬКО тесты. Если при написании теста обнаружишь баг в продакшн-коде —
  ОПИШИ его подробно в финальном отчёте (файл:строка, сценарий
  воспроизведения), но НЕ исправляй сам — это будет отдельная задача.
- НЕ дублируй тесты, которые уже покрывают ровно тот же сценарий (см. список
  уже существующих тестов выше) — только реальные пробелы.

## Проверка (сделает оркестратор)

- Диф ограничен `tests/`-директориями перечисленных крейтов + возможно
  `__tests__/` в TS — НИ ОДНОГО изменения в продакшн-файлах.
- fmt/clippy чисты; полный тестовый гейт зелёный.
- Отчёт агента явно перечисляет: какие пробелы найдены и закрыты, какие
  проверены и оказались УЖЕ покрыты (не задвоены), и есть ли найденные, но
  НЕ исправленные баги продакшн-кода (для отдельной задачи).
