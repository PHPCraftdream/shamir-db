בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Rust cross-language msgpack fixture + wire round-trip (репл-DDL)

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION-CLIENT-SURFACE.md` §4. Rust-билдер
> `crates/shamir-query-builder/src/ddl/replication.rs` (904755a8), op-типы
> `repl_ops.rs` (143dc060). RE-SCOPED: leader/follower e2e уже в R1-d,
> wire-pull в R0-c — здесь только cross-language контракт.

## Задача

Зафиксировать канонический msgpack-контракт для репл-DDL ops (эталон для
TS-паритета в #376) + доказать wire round-trip.

## Часть A — канонический fixture

1. Выбрать репрезентативный набор ops (все 10) с ФИКСИРОВАННЫМИ полями
   (детерминизм — никаких timestamp/random), напр.:
   - `replication_profile("cluster").stream(repl_scope("app").repo("main").build(), Pull, ReadOnly)`
   - `publication("pub_all").scope(repl_scope("app").build())`
   - `subscription("sub1", "leader:9above", "pub_all", "cluster")` (адрес — фикс. строка)
   - `alter_subscription("sub1").set_profile("cluster2")`
   - `alter_subscription("sub1").pause()`
   - drop_* и три read-only.
2. Для каждого: `BatchOp` → `rmp_serde::to_vec_named(&op)` → hex-строка.
3. Сохранить как fixture-файл `crates/shamir-query-builder/tests/fixtures/repl_ddl_msgpack.json`
   (или `.rs` const-таблица; предпочти JSON `{ "<op_label>": "<hex>" }` —
   его прочитает и TS в #376). Каталог создать при нужде.

## Часть B — Rust регресс-тест

Тест (integration `tests/` или в builder-crate): для каждого op'а
пересобрать через билдер → `to_vec_named` → hex → сравнить с fixture. Ловит
случайный дрейф wire-формы Rust-стороны.

## Часть C — wire round-trip

Тест: `BatchOp` (репл-DDL) → `rmp_serde::to_vec_named` → представить как
`QueryValue`/map → `BatchOp::from_query_value` (или тем путём, что
использует сервер для декода batch-op'ов — сверься, как это делают
существующие round-trip тесты в `shamir-query-types/src/batch/tests/`) →
равно исходному `BatchOp`. Все 10 ops. Это доказывает, что то, что
производит билдер, сервер способен разобрать обратно в тот же вариант.
(Исполнения на сервере нет — только декод-контракт.)

## Замечания

- Детерминизм fixture — критичен: те же входные строки → те же байты.
  Никаких HashMap-итераций в сериализуемых полях (Vec/поля struct — ок,
  порядок стабилен).
- Если `to_vec_named` даёт map с порядком полей = порядок объявления struct
  — задокументируй это в комментарии fixture (важно для TS #376).
- Не подключай тяжёлых зависимостей — hex через `format!("{:02x}")` руками
  или существующий hex-крейт, если уже в dev-deps.

## Гейт

- `./scripts/test.sh -p shamir-query-builder` (+ `-p shamir-query-types`
  если round-trip там) зелёный.
- `cargo fmt` тронутых крейтов чистый.
- `cargo clippy` тронутых крейтов `-- -D warnings` чистый.

## Definition of done

- Fixture-файл с hex msgpack для 10 ops (детерминированный).
- Регресс-тест (билдер воспроизводит fixture) + wire round-trip (10 ops).
- Комментарий про порядок ключей map (для TS #376).
- Финальное сообщение: путь к fixture, тронутые файлы, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
