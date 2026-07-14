בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# PR0 — смоук-тест «changefeed на system repo» (test-only)

> Контекст: `docs/dev-artifacts/roadmap/REPLICATION.md` §7.1 (V1a),
> `docs/dev-artifacts/research/REPLICATION-PRE-REFACTOR-2026-06-30.md` §Б PR0.

## Задача

Написать RED→GREEN смоук-тест в крейте `shamir-db`, подтверждающий, что
системные записи (создание пользователя) проходят через changefeed —
т.е. что репликация аккаунтов будет обычным data-потоком.

Тест: выполнить `create_user` (через builder `shamir_query_builder::ddl::create_user`
внутри админ-батча — см. существующий пример
`crates/shamir-db/src/shamir_db/tests/execute_tests.rs::test_create_user_hashes_password_at_rest`),
затем прочитать журнал системного repo через
`ShamirDb::read_changelog_from(<system-db>, <system-repo>, 1, N)`
(`crates/shamir-db/src/shamir_db/shamir_db/changelog.rs`) и убедиться,
что там есть событие с таблицей `users` (или соответствующей системной
таблицей — выясни точные имена db/repo/table системного стора по
`crates/shamir-db/src/shamir_db/system_store.rs`).

## Требования

- Тест кладётся в существующий tests-layout `shamir-db`
  (`crates/shamir-db/src/shamir_db/tests/` — по topic-файлу; если есть
  подходящий файл про changelog — туда, иначе новый
  `system_changefeed_tests.rs`, зарегистрированный в `tests/mod.rs`).
- `#[tokio::test]`, in-memory init (`ShamirDb::init_memory()`), JSON-литералы
  в тестах многострочные.
- Запрещено конструировать запросы raw-JSON — только query builder.
- Если тест окажется RED (system repo не подключён к changefeed) — НЕ чини
  движок. Зафиксируй в финальном сообщении точную причину (какой код-путь
  не эмитит событие), оставь тест `#[ignore]` с комментарием `// PR0b:` и
  причиной. Оркестратор решит про PR0b.
- Прогон тестов ТОЛЬКО через `./scripts/test.sh -p shamir-db -- <фильтр>`
  (raw `cargo test` заблокирован перимeter-guard'ом).
- Гейт перед завершением: `cargo fmt -p shamir-db -- --check` и
  `cargo clippy -p shamir-db --all-targets -- -D warnings` чистые.

## Definition of done

- Новый тест существует, зелёный (или `#[ignore]` + диагноз RED-причины).
- Никакие другие файлы не тронуты, кроме теста и его `mod.rs`-манифеста.
- Финальное сообщение: путь к тесту, GREEN/RED-вердикт, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
