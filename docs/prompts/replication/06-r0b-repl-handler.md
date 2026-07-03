בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# R0-b — repl_handler (ReplHello + ReplPull + leader_epoch + long-poll + auth)

> Контекст: `docs/roadmap/REPLICATION.md` §5.1/§5.2/§5.4/§5.6, R0-a wire-типы
> уже в `crates/shamir-query-types/src/wire/repl.rs`
> (`ReplRequest`/`ReplResponse`/`ReplRepoInfo`). Заменяет placeholder-ветку
> `DbRequest::Repl(_)` в handler.rs.

## Задача

Реализовать leader-side обработчик privileged репликационного pull-API.
Новый файл `crates/shamir-server/src/db_handler/repl_handler.rs` (по стилю
`subscribe_handler.rs`); диспатч из `handler.rs`.

## Готовые API (использовать, НЕ изобретать)

- `session_actor(session) -> Actor` (handler.rs, `pub(super)`).
- `SessionPermissions::has_role(&self, "replicator")` (PR2); `is_superuser`
  как bypass.
- `self.db.list_dbs() -> Vec<String>`; `self.db.get_db(name)` →
  `DbInstance::list_repos() -> Vec<String>`.
- `self.db.current_commit_version(db, repo).await -> Option<u64>`.
- `self.db.read_changelog_from_journal(db, repo, from_version, limit).await
  -> Option<JournalRead>` (`shamir_engine::JournalRead { events:
  Vec<ChangelogEvent>, gap_at: Option<u64> }` — проверь точные поля).
- Авторизация: `self.db.authorize_access(&actor, &path, Action::Read).await
  -> Result<(), AccessError>`, где
  `path = shamir_types::access::ResourcePath::store(db, repo)` (repo — это
  «store»), `Action = shamir_types::access::Action::Read`. (Сверь точные
  пути импорта — возможно реэкспорт через `shamir_db::access`.)
- Кодирование событий: `rmp_serde::to_vec_named(&events)` → `Vec<u8>` для
  `ReplResponse::Pull.events` (serde_bytes-поле).

## leader_epoch

Поле `pub(super) leader_epoch: u64` в `ShamirDbHandler` (default **1**) +
билдер `with_leader_epoch(mut self, epoch: u64) -> Self`, ровно как
`node_mode`/`with_node_mode` (PR4). Каждый `ReplResponse` несёт это значение.
(Персистентность epoch и bump при promote — R3; сейчас статичное поле.)

## Логика

### Диспатч (handler.rs)

Заменить placeholder `DbRequest::Repl(_) => …` на
`DbRequest::Repl(repl_req) => DbResponse::Repl(self.handle_repl(session, repl_req).await)`.

### Гейт роли (в начале handle_repl)

Если НЕ (`session.permissions.is_superuser` ||
`session.permissions.has_role("replicator")`) → вернуть
`ReplResponse::Error { leader_epoch, code: "bad_role".into(), message:
"replication requires the `replicator` role".into() }`. Deny-by-default.

### ReplHello { proto_ver, node_id }

- (Опционально можно проверить `proto_ver`; при несовместимости — Error
  `unsupported_proto`. Для R0 прими любой, но оставь TODO.)
- Собрать `repos: Vec<ReplRepoInfo>`: для каждого db из `list_dbs()`, для
  каждого repo из `get_db(db).list_repos()` — проверить
  `authorize_access(actor, store(db,repo), Read)`; если Ok — добавить
  `ReplRepoInfo { db, repo, current_version: current_commit_version(db,repo)
  .unwrap_or(0), journal_floor: 0 }`. Репо без доступа НЕ включать (не течёт
  информация о существовании).
- Вернуть `ReplResponse::Hello { leader_epoch, repos }`.

### ReplPull { db, repo, from_version, limit, wait_ms }

1. `authorize_access(actor, store(&db,&repo), Read)` → Err ⇒
   `ReplResponse::Error { leader_epoch, code: "denied_repo", message }`.
2. Long-poll: прочитать журнал `read_changelog_from_journal(&db,&repo,
   from_version, limit as usize)`. Если событий нет И `wait_ms` = Some(ms>0):
   в цикле с коротким шагом (напр. `tokio::time::sleep(Duration::from_millis(
   50))`) до дедлайна `now+ms` перечитывать, пока не появятся события ИЛИ не
   истечёт бюджет. НИКАКОГО hanging-состояния на лидере (§5.1) — это
   вежливый poll, не подписка. §5.6: pull НЕ держит commit_lock и не тормозит
   писателей — только читает журнал.
   - ВАЖНО: не спамить `sleep` при limit=0 или невалидных значениях —
     ограничь число итераций (deadline-based), покрой тестом что пустой
     хвост с wait_ms возвращается за ~wait_ms, а не виснет.
3. `None` из journal-read (db/repo не существует) ⇒
   `ReplResponse::Error { leader_epoch, code: "unknown_repo", message }`.
4. Успех: `events_bytes = rmp_serde::to_vec_named(&jr.events)?`;
   `current_version = current_commit_version(&db,&repo).await.unwrap_or(
   from_version)`; вернуть `ReplResponse::Pull { leader_epoch, events:
   events_bytes, gap_at: jr.gap_at, current_version }`.

## Тесты (db_handler/tests/repl_handler_tests.rs, register in tests/mod.rs)

`#[tokio::test]`, in-memory `ShamirDb`, fixture-хендлер как в
`node_mode_tests.rs` (create_db_as/add_repo_as под `alice`). Сессии строить
с нужными ролями через `SessionPermissions::from_roles`.

1. **deny без роли:** обычная сессия (без `replicator`) → `ReplHello` даёт
   `Error{code:"bad_role"}`.
2. **hello с ролью:** сессия с ролью `replicator` И владелец/доступ к repo →
   `ReplResponse::Hello` содержит нужный repo с `current_version`.
3. **pull возвращает события:** записать N строк на лидере, `ReplPull`
   from_version=0 → `Pull` c непустыми `events` (декодировать
   `rmp_serde::from_slice::<Vec<ChangelogEvent>>` и проверить count),
   `current_version > 0`.
4. **pull deny без grant:** сессия с ролью `replicator`, но без доступа к
   repo (другой владелец) → `Error{code:"denied_repo"}`.
5. **long-poll не виснет:** `ReplPull` с `wait_ms: Some(200)` на пустом
   хвосте (from_version = current) возвращается за разумное время (< ~1s) с
   пустыми events. (Проверь, что не виснет; строгий тайминг не нужен.)
6. **leader_epoch:** хендлер с `with_leader_epoch(7)` → любой ответ несёт
   `leader_epoch == 7`.

## Гейт

- `./scripts/test.sh @server` зелёный.
- `cargo fmt -p shamir-server -- --check` чистый.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` чистый.

## Definition of done

- repl_handler.rs + диспатч (placeholder убран) + поле/билдер leader_epoch.
- 6 тестов зелёные; deny-by-default соблюдён.
- Тронуты только repl_handler.rs, handler.rs, mod.rs (re-export при нужде),
  tests/mod.rs, tests/repl_handler_tests.rs.
- Финальное сообщение: список тронутых файлов, вывод test.sh, любые TODO.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
