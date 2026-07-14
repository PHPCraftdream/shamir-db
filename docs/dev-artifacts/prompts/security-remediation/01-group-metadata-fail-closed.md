# Brief: Group metadata fail-closed (taskId #602, audit residual, P0)

## Контекст

Ревью устранения аудитов (2026-07-14) нашло реальный residual fail-open в
`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`, метод
`resource_meta`, ветка `ResourcePath::Group`:

```rust
ResourcePath::Group { name } => {
    let group_ref = crate::query::admin::GroupRef::Name { name: name.clone() };
    let group_id = match self.resolve_group_id(&group_ref).await {
        Ok(id) => id,
        Err(_) => return Ok(ResourceMeta::open()),   // <-- БАГ: строка ~185
    };
    match self.system_store.load_group(group_id).await {
        Ok(Some(rec)) => Ok(ResourceMeta { /* ... */ }),
        Ok(None) => Ok(ResourceMeta::open()),
        Err(e) => {
            log::warn!("resource_meta: failed to load group '{}' meta: {e}", name);
            Err(e)                                   // <-- ПРАВИЛЬНО: соседняя ветка
        }
    }
}
```

`resolve_group_id` (тот же файл, метод `resolve_group_id`, строка ~703-719)
возвращает `DbResult<u64>`:

```rust
pub async fn resolve_group_id(
    &self,
    group_ref: &crate::query::admin::GroupRef,
) -> DbResult<u64> {
    match group_ref {
        crate::query::admin::GroupRef::Id { id } => Ok(*id),
        crate::query::admin::GroupRef::Name { name } => {
            let groups = self.system_store.load_groups().await?;   // storage error пробрасывается сюда
            let id = groups
                .iter()
                .find(|g| g.get("name").and_then(|v| v.as_str()) == Some(name.as_str()))
                .and_then(|g| g.get("group_id").and_then(|v| v.as_u64()))
                .ok_or_else(|| DbError::NotFound(format!("group '{}' not found", name)))?; // подтверждённый not-found
            Ok(id)
        }
    }
}
```

Проблема: `resource_meta`'s ветка `ResourcePath::Group` делает
`Err(_) => return Ok(ResourceMeta::open())` — она НЕ различает:
- подтверждённый not-found (`DbError::NotFound(_)` из явного `ok_or_else` выше) — это ЛЕГИТИМНЫЙ fallback на `open()` (несуществующая группа — не ошибка для resolve meta, это уже согласованная политика, см. комментарий в файле над этой веткой строка ~178-180);
- storage error (пробрасывается через `.await?` от `self.system_store.load_groups()` — реальный I/O/catalogue-page-corruption fault) — это ДОЛЖНО быть `Err`, не тихий откат к «открыто для всех».

`DbError` (crates/shamir-storage/src/error.rs) — обычный `thiserror` enum,
`NotFound(String)` — отдельный, матчибельный вариант. `DbError` уже
импортирован в access_control.rs (`use crate::{DbError, DbResult};`, строка 14).

Соседняя ветка `load_group` (см. код выше, `Ok(None) => open()`,
`Err(e) => Err(e)`) — уже правильный образец. Нужно привести
`resolve_group_id`-ветку к тому же принципу, но матчинг идёт по варианту
самого `Err`, а не по `Ok`/`None`, потому что `resolve_group_id`
возвращает `DbResult<u64>`, а не `DbResult<Option<u64>>`.

## Задача

1. В `access_control.rs`, ветка `ResourcePath::Group`, заменить:
   ```rust
   Err(_) => return Ok(ResourceMeta::open()),
   ```
   на матчинг по варианту ошибки:
   ```rust
   Err(DbError::NotFound(_)) => return Ok(ResourceMeta::open()),
   Err(e) => return Err(e),
   ```
   (Оставить существующий комментарий над веткой, поправить его при
   необходимости, чтобы явно называть это "not-found → open, storage error →
   Err" — сейчас комментарий говорит только про "not-found falls back to
   open" и не упоминает storage error вовсе.)

2. Regression-тест. Место: `crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs`
   — в этом файле уже есть ГОТОВАЯ инфраструктура fault-injection для ровно
   такого сценария (audit #540, строки ~625-785): модуль `mod failing_store`
   (`FailingStore` — обёртка над `InMemoryStore`, при `armed=true` возвращает
   `DbError::Storage(...)` на каждый read) и хелпер
   `shamir_with_failing_databases_table()`, который через
   `RepoInstance::install_table_for_test("databases", tbl)` подменяет
   таблицу `"databases"` в SYSTEM_REPO на fault-injecting store, плюс два
   теста-образца: `resource_meta_fails_closed_on_storage_error` и
   `authorize_access_denies_when_resource_meta_errors`.

   Нужно СКОПИРОВАТЬ этот паттерн для таблицы `"groups"` (константа
   `TABLE_GROUPS = "groups"` в `crates/shamir-db/src/shamir_db/system_store.rs:37`,
   используется и `load_groups()`, и `load_group()` — обе идут через
   `self.table(TABLE_GROUPS)`):

   - Новый хелпер `shamir_with_failing_groups_table()` — тот же паттерн, что
     `shamir_with_failing_databases_table()`, но `install_table_for_test("groups", tbl)`.
   - Новый тест `resource_meta_group_fails_closed_on_storage_error`:
     1. Создать `ShamirDb` с failing "groups"-таблицей (unarmed).
     2. Создать группу через `shamir.create_group("testgroup").await.unwrap()`
        (метод в `access_control.rs:375`).
     3. Sanity: unarmed — `shamir.resource_meta(&ResourcePath::Group { name: "testgroup".into() }).await` возвращает `Ok(meta)` с реальным owner (не default-open).
     4. Армировать fault (`fault.armed.store(true, Ordering::SeqCst)`).
     5. Вызвать `resource_meta(&ResourcePath::Group { name: "testgroup".into() })` — под БАГОМ это `Ok(ResourceMeta::open())` (owner=System, mode 0o777 — доступно всем). ПОСЛЕ фикса — должно быть `Err(_)`.
     6. `assert!(result.is_err(), "...")` с тем же духом сообщения, что в
        `resource_meta_fails_closed_on_storage_error`.
   - Опционально (если легко) — второй тест по аналогии с
     `authorize_access_denies_when_resource_meta_errors`, но для Group +
     `Action::Read` через non-owner `Actor::User(999)` — не обязателен, если
     первый тест уже красный→зелёный демонстрирует фикс, но лучше добавить
     для симметрии с существующим Database-тестом.

   **TDD дисциплина**: сначала убедиться, что новый тест ПАДАЕТ на текущем
   (бажном) коде — запустить `./scripts/test.sh -p shamir-db -- resource_meta_group_fails_closed_on_storage_error`
   ДО фикса кода в п.1, затем применить фикс, затем убедиться что тест
   зеленеет. Если делаешь оба шага (тест + фикс) одним патчем — всё равно
   проверь мысленно/локально, что assertion действительно бы упал на старом
   коде (не тавтологичный тест).

3. Проверить, что ничего больше не сломалось:
   `./scripts/test.sh -p shamir-db` (lib) должен быть зелёным.
   `cargo fmt -p shamir-db -- --check` и
   `cargo clippy -p shamir-db --all-targets -- -D warnings` — чисто.

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит диф, перегонит тесты и закоммитит.

- Не трогай другие ветки `resource_meta` (Database/Store/Table/Record/Index/User/Root) — они уже корректны, эта задача только про `Group`.
- Не меняй сигнатуру `resolve_group_id` и не трогай вызывающий код в
  `admin_access.rs` (там `resolve_group_id` используется в других местах с
  иной семантикой — `if_exists`, rename и т.д. — не в scope этой задачи).
- Не переименовывай существующие тесты/хелперы в `access_meta_tests.rs` —
  только добавляй новые рядом.

## Проверка (сделает оркестратор)

- Диф читается построчно, изменение ровно в описанной ветке + новый тест(ы).
- Новый тест реально падает на старом коде (проверка "тест не тавтологичный").
- `./scripts/test.sh -p shamir-db` зелёный.
- `cargo fmt -p shamir-db -- --check` и `cargo clippy -p shamir-db --all-targets -- -D warnings` чисты.
