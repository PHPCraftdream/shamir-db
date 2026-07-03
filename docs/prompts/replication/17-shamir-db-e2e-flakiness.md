בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Расследовать + починить флейковость shamir-db e2e под @e2e --full

> Контекст: наблюдение из R0-c (2026-07-03). Не связано с репликацией.

## Симптом (проверенный факт)

Под `./scripts/test.sh @e2e --full` (leader = shamir-db + shamir-server,
высокая параллельность nextest) наблюдалось ~46 падений в интеграционных
e2e-тестах крейта **shamir-db**, среди них:
`cas_sequenced_e2e`, `ddl_wire_e2e::error_codes::error_code_access_denied_ddl`,
`ddl_wire_e2e::lifecycle::*`, `declarative_schema_e2e`,
`declarative_schema_stamping_replay_e2e`, `native_parity_e2e`,
`purge_history::purge_denied_without_permission`,
`query_auth::{test_database_level_permission, test_no_permissions_denies,
test_repo_level_permission}`, `validators_lifecycle::*`.

**Ключевой факт:** `error_code_access_denied_ddl` (и, вероятно, остальные)
**проходят в ИЗОЛЯЦИИ** на HEAD:
`./scripts/test.sh -p shamir-db --full -E 'test(error_code_access_denied_ddl)'`
→ PASS. То есть это НЕ детерминированный регресс, а флейк под параллельной
нагрузкой. Класс совпадает с тем, что уже чинилось в бенчах серией
`fix(bench): access_denied …` (Strategy A `owned_enforced` по умолчанию,
System-owned ресурсы теперь `0o700` вместо старого открытого `0o777`).

## Задача

1. **Воспроизвести под нагрузкой.** Запусти shamir-db интеграционные e2e
   под полной параллельностью, повтори несколько раз, чтобы флейк всплыл:
   ```
   ./scripts/test.sh -p shamir-db --full > run1.log 2>&1; echo exit=$?
   ```
   (повтори 3-5 раз; собери, КАКИЕ тесты падают и с какими сообщениями —
   `access_denied`? паника? timeout? конфликт стора?). НЕ пайпи в grep на
   лету — пиши в файл, потом grep по файлу (см. CLAUDE.md).
2. **Найти корень.** Гипотезы, проверь по коду и логам падений:
   - **Общий data-dir / имя ресурса между тестами** — два теста создают db
     с одинаковым именем в общей директории и топчут ACL/стор друг друга?
     Ищи хардкод-имена (`app`, `testdb`, фикс-порты) и общий `TempDir`.
   - **Гонка ACL-состояния** — System-owned ресурс `0o700`, тест ждёт
     доступ, но параллельный тест меняет владельца/права.
   - **Глобальное состояние процесса** — статические реестры, `OnceCell`,
     общий интернер/аллокатор-состояние, sccache между тест-бинарями.
   - **Порт/файл-лок гонка** — если поднимается сервер на фикс-порту.
   Локализуй ОДИН корень (или несколько), подтверди по коду тестов
   (`crates/shamir-db/tests/**`) и харнесса.
3. **Починить — НЕ поднимая таймаут** (CLAUDE.md: таймаут-мазок запрещён).
   Правильные фиксы: уникальные имена ресурсов/директорий per-тест (напр.
   через `TempDir` + уникальный db-name), корректная выдача ACL в setup'е
   теста (как в `fix(bench)` серии — `create_db_as`/`add_repo_as` под нужным
   Actor вместо надежды на открытые дефолты), устранение shared-state.
4. **Доказать стабильность.** После фикса прогони shamir-db e2e под
   параллельностью 5+ раз подряд — 0 падений. Приведи цифры.

## Границы

- Чини ТОЛЬКО тесты shamir-db e2e и, при нужде, их харнесс/фикстуры. НЕ
  меняй продакшн-код движка, если корень не в нём (а если в нём — сначала
  опиши находку в финальном сообщении, минимальный фикс допустим, но
  выделяй его логически).
- Тесты ТОЛЬКО через `./scripts/test.sh` (raw `cargo test` заблокирован).
- Gate перед завершением: `cargo fmt` тронутых крейтов `--check` чистый;
  `cargo clippy -p shamir-db --all-targets -- -D warnings` чистый;
  shamir-db e2e зелёные и СТАБИЛЬНЫ (5+ прогонов).

## Definition of done

- Корень назван и подтверждён по коду.
- Фикс применён (unique-per-test изоляция / корректный ACL-setup / убрано
  shared-state), таймауты НЕ трогали.
- shamir-db e2e стабильно зелёные под параллельностью (5+ прогонов, 0 fail).
- Финальное сообщение: корневая причина, что изменено, лог стабильности
  (N прогонов × exit=0), тронутые файлы.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
