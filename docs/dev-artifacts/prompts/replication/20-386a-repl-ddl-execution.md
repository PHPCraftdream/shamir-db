בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# 386-a — исполнение репл-DDL: storage + dispatch (repo `system`)

> Контекст: репл-DDL ops уже в BatchOp (#372, 143dc060), но
> `admin_dispatch.rs` их не исполняет. Модель — как users/roles: писать в
> таблицы repo `system` через SystemStore + tx-commit (V1a → сами
> реплицируются). ТОЛЬКО персистентность + CRUD + чтение; запуск
> follower-loop — 386-b (НЕ здесь).

## Паттерн (следуй существующему буквально)

Изучи `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs::
handle_create_user` — это эталон: `system_store().users_table()` →
`SetOp { set, key, value }` → `set_via_implicit_tx(&table, &set_op)` →
`interner().persist()` → `admin_result(...)`. Диспетчеризация —
`admin_dispatch.rs` (`BatchOp::CreateUser(op) => self.handle_create_user(op)`).
SystemStore — `crates/shamir-db/src/shamir_db/system_store.rs` (добавь
таблицы по образцу `users_table()`).

## Задача

1. **SystemStore-таблицы:** добавить `replication_profiles`, `publications`,
   `subscriptions` (по образцу существующих `users`/`roles` таблиц в
   system_store.rs — их регистрация в SystemStore init + аксессоры
   `*_table()`). Ключ — имя сущности (`name`).
2. **Хендлеры** (новый файл `execute/admin_replication.rs`, по образцу
   admin_users_roles.rs; подключить в модуль):
   - `handle_create_replication_profile(op)` → записать
     `{ name, streams }` в `replication_profiles` по ключу name.
   - `handle_drop_replication_profile(op)` → удалить по name (как
     handle_drop_user — delete_via_implicit_tx).
   - `handle_create_publication` / `handle_drop_publication` → `publications`.
   - `handle_create_subscription` → `subscriptions` (запись
     `{ name, upstream, publication, profile, state: "active" }`; state поле
     для 386-b pause/resume). `handle_drop_subscription` → удалить.
   - `handle_alter_subscription(op)` → прочитать запись, применить SubAction
     (Pause → state="paused", Resume → state="active", SetProfile(p) →
     profile=p), записать обратно.
   - `handle_list_publications` / `handle_list_subscriptions` → прочитать
     все записи таблицы, вернуть массив (как handle_list для databases —
     admin_result с массивом).
   - `handle_replication_status` → пока: вернуть список подписок с их state
     (lag добавит 386-b). 
3. **Диспатч** в admin_dispatch.rs: добавить 10 веток
   `BatchOp::CreateReplicationProfile(op) => self.handle_create_replication_profile(op).await` и т.д.
4. **Superuser-гейт** — репл-DDL мутации требуют superuser (как остальной
   admin; проверь, есть ли общий гейт в handler.execute is_admin-ветке, или
   нужен authorize внутри хендлера, как authorize_user_lifecycle). Сделай
   как ближайший аналог (create_role/create_group).

## Тесты (crates/shamir-db/src/shamir_db/tests/ или execute-тесты)

`#[tokio::test]`, in-memory ShamirDb, запросы через query-builder
(`shamir_query_builder::ddl::replication::*`, 904755a8):
- create_publication → list_publications содержит её.
- create_subscription → list_subscriptions содержит; поля upstream/
  publication/profile корректны.
- alter_subscription pause → list показывает state "paused"; resume →
  "active"; set_profile → profile обновлён.
- drop_publication/subscription → list больше не содержит.
- create_replication_profile со streams → сохранён и читается.

## Гейт

- `./scripts/test.sh -p shamir-db` зелёный.
- `cargo fmt -p shamir-db -- --check` чистый.
- `cargo clippy -p shamir-db --all-targets -- -D warnings` чистый.

## Definition of done

- 3 system-таблицы + 10 хендлеров + диспатч + superuser-гейт.
- CRUD+list круг-трипы зелёные (запросы builder-only).
- follower-loop НЕ запускается (это 386-b) — только персистентность.
- Финальное сообщение: тронутые файлы, как решён гейт, вывод test.sh.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
