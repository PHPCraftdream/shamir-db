בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Остаточный флейк query_index2 под параллельностью (второй корень)

> Контекст: продолжение #380. Первый корень (`TableManager.bindings_len`
> per-clone desync) уже починен коммитом 6ed6d575 — флейки упали с ~46 до 1.
> Остался ОДИН независимый флейк.

## Симптом (проверенный факт)

Под `./scripts/test.sh -p shamir-db --full` (полная параллельность nextest)
падает:
```
FAIL shamir-db::query_index2 covering_index_include_stored_in_meta
crates\shamir-db\tests\query_index2.rs:538: assertion `left == right` failed:
  expected exactly one sorted index   (defs.len() != 1)
```
Тест создаёт СВОЙ `ShamirDb::init_memory()` (изолированный инстанс), db
`testdb`, таблицу `users`, один sorted-index `score_sorted`, затем читает
`table.sorted_indexes().iter_indexes()` и ждёт РОВНО 1. В изоляции проходит;
под параллельностью `defs.len()` != 1.

## Ведущая гипотеза (проверить ПЕРВОЙ)

Каждый тест изолирован через `init_memory()`, значит cross-test утечка
возможна ТОЛЬКО через **process-global / `static` состояние**. Подозрение:
sorted-index / index2 каталог хранится в глобальном реестре, ключёванном по
**table token** (`table_token_for("users")` — детерминированный хеш имени,
ОДИНАКОВ во всех инстансах). Два параллельных теста, оба создающие таблицу
`users` с sorted-index, видят индексы друг друга → `defs.len()` > 1.

Проверь:
- Где живёт `sorted_indexes()` / `iter_indexes()` — это per-`TableManager`
  инстанс, или глобальный `static`/`OnceCell`/`lazy_static` реестр по токену?
  (`crates/shamir-engine/src/index2/**`, `crates/shamir-engine/src/table/**`).
- Есть ли `static`/`OnceLock`/глобальный `scc::HashMap` по table-token для
  index2-метаданных, НЕ привязанный к конкретному `ShamirDb`/repo-инстансу?
- Тот же ли класс, что `bindings_len` (общий счётчик/реестр, где нужна
  per-instance изоляция)?

## Задача

1. Воспроизвести (несколько прогонов `-p shamir-db --full`, собрать в файл;
   флейк редкий — гоняй в цикле).
2. Найти корень по коду (подтвердить/опровергнуть гипотезу глобального
   реестра по токену).
3. Починить: изолировать состояние per-`ShamirDb`/per-repo (не глобально по
   токену), ЛИБО, если это тестовая гонка read-after-write, дождаться
   durable-видимости корректно. НЕ поднимать таймауты. НЕ ослаблять assert.
4. Доказать стабильность: 5+ прогонов `-p shamir-db --full` подряд, 0
   падений (или хотя бы 0 падений `covering_index_include_stored_in_meta`).

## Границы

- Если корень в продакшн-коде (глобальный реестр) — минимальный фикс в
  движке, логически выделенный; опиши находку.
- Тесты ТОЛЬКО через `./scripts/test.sh`.
- Gate: `cargo fmt` тронутых крейтов `--check` чистый; `cargo clippy` тронутых
  `-- -D warnings` чистый; `./scripts/test.sh @oracle` зелёный (если тронул
  engine); `-p shamir-db --full` стабилен.

## Definition of done

- Корень назван и подтверждён по коду.
- Фикс применён (изоляция состояния / корректная синхронизация), таймауты и
  assert НЕ трогали.
- 5+ прогонов `-p shamir-db --full` без этого флейка.
- Финальное сообщение: корень, что изменено, лог стабильности, тронутые файлы.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
