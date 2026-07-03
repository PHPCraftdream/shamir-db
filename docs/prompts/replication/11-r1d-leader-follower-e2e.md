בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R1-d — leader→follower e2e-конвергенция + read-only гейт

> Контекст: `docs/roadmap/REPLICATION.md` §9. Capstone R1 — связывает
> готовое: R0-b `handle_repl` (лидер), R1-a `apply_replicated`, R1-b
> bookmark, R1-c `run_follower_loop` + `ReplSource` импл'ы
> (`InProcessReplSource`, `WireReplSource`) + NodeMode PR4 (read-only гейт).

## Задача

Интеграционный тест: два узла (leader ReadWrite + follower ReadOnly),
follower тянет и СХОДИТСЯ с leader, а клиентские записи на follower'е
отвергаются гейтом.

## Форма (prefer real wire; допустим in-process)

**PREFERRED (capstone-качество):** два реальных сервера через
`ServerLauncher` (по образцу `crates/shamir-server/tests/mvp_e2e.rs` +
`tests/common/`): leader-сервер + follower-сервер. Follower подключается к
leader как replicator-аккаунт (`WireReplSource` поверх `shamir_client::Client`,
R1-c) и гоняет `run_follower_loop` (с `max_iterations` или CancellationToken
для завершения теста).

**FALLBACK (если два-серверный wire-сетап тяжёл в объёме R1-d):** два
in-process `Arc<ShamirDb>` (leader + follower) через `InProcessReplSource`
(R1-c) — это всё равно доказывает конвергенцию + apply-путь; NodeMode-гейт
проверяется на follower-`ShamirDbHandler`. В этом случае отметь в финальном
сообщении, что wire-двухузловой прогон отложен, и что именно им не покрыто.

Файл: `crates/shamir-server/tests/repl_convergence_e2e.rs`.

## Сценарии (обязательные)

1. **Конвергенция:** на leader создать db `app`/repo `main`/таблицу `items`,
   записать N строк (builder-batch, transactional). Запустить follower-loop
   (bounded) до догоняния → на follower'е ЧИТАЮТСЯ те же N строк (данные
   сошлись), `follower.replication_bookmark("app","main") ==` leader
   `current_version`.
2. **Инкремент:** дописать ещё M строк на leader → прогнать loop ещё →
   follower догнал (N+M).
3. **Read-only гейт (PR4):** follower-`ShamirDbHandler` сконструирован с
   `NodeMode::ReadOnly`; клиентский write-batch на follower'е →
   `DbResponse::Error { code: "read_only_replica" }`. При этом
   репликационный apply (loop) на том же follower'е РАБОТАЕТ (гейт — только
   для клиентских запросов через execute(), не для apply_replicated).
4. **Идемпотентность повторного pull:** после догоняния прогнать loop снова
   → apply даёт Skipped, данные и bookmark не меняются (никакого
   дублирования строк).

## Замечания

- Запросы — через query-builder (создание db/repo/таблицы, записи, чтения).
- Follower создаёт локально те же db/repo/таблицу (`items`), что и leader,
  ДО запуска loop (apply_replicated пишет в существующую таблицу по токену
  имени; таблица должна существовать на follower'е — иначе changes
  скипаются с warn). Убедись, что follower-схема заведена.
- Держи tempdir'ы живыми до конца теста.
- Loop завершай детерминированно (`FollowerLoopConfig::with_max_iterations`
  или CancellationToken после проверки догоняния) — НЕ бесконечный sleep.

## Гейт

- `./scripts/test.sh -p shamir-server --full -- repl_convergence` зелёный
  (или `@e2e --full`, но помни про преэкзистирующую shamir-db флейковость —
  ориентируйся на shamir-server: 0 FAIL).
- `cargo fmt -p shamir-server -- --check` чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый.

## Definition of done

- `repl_convergence_e2e.rs` со сценариями 1-4 зелёный.
- Конвергенция + read-only гейт + идемпотентность доказаны.
- Тронут только новый тест-файл (+ common при нужде).
- Финальное сообщение: wire или in-process форма, вывод test.sh, что
  отложено (если fallback).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
