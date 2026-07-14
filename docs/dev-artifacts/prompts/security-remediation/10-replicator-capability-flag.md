# Brief: replicator role → authoritative capability flag (taskId #621, часть #615)

## Контекст

`crates/shamir-server/src/db_handler/repl_handler.rs:39,46` — `replicator`
сейчас обычная строковая роль:

```rust
const REPLICATOR_ROLE: &str = "replicator";
...
if !(session.permissions.is_superuser || session.permissions.has_role(REPLICATOR_ROLE)) {
```

В отличие от `superuser`, который после task #557/#559 стал authoritative
`bool`-флаг на `PersistedUser`, с зарезервированной строкой на границе
записи ролей и отдельным DDL (`SetSuperuser`). Пользователь решил:
сделать `replicator` СИММЕТРИЧНО, отдельным `DbRequest::SetReplicator`,
зеркалящим `SetSuperuser` максимально буквально.

**Важное упрощение (не требуется мигрировать легаси)**: `"replicator"`
никогда раньше не был зарезервирован — значит, в отличие от `superuser`'s
миграции (`migrate_roles_and_warm`, которая переносит СУЩЕСТВУЮЩИЕ
персистентные записи со строкой "superuser" в роли на флаг), здесь
МИГРАЦИЯ НЕ НУЖНА: просто зарезервировать строку с этого момента и
завести флаг с дефолтом `false`. Также НЕ нужен `last-remaining-replicator`
guard (`set_superuser`'s "cannot revoke from the last remaining
superuser" — это специфично для необходимости всегда иметь хотя бы
одного админа; для replicator нет такого инварианта — ноль репликаторов
это нормальное состояние).

## Задача

### 1. `PersistedUser`/`UserRecord`-подобные структуры

`crates/shamir-server/src/user_directory.rs`:
- `struct PersistedUser` (рядом с `pub(crate) superuser: bool`, строка
  ~126) — добавь `pub(crate) replicator: bool` с `#[serde(default)]`
  (важно для обратной совместимости десериализации СУЩЕСТВУЮЩИХ
  персистентных blob'ов без этого поля — `serde`'s `#[serde(default)]`
  на поле, не структуре, чтобы не задеть остальные поля).
- Конструктор `PersistedUser::new(...)` (строка ~144-155) — добавь
  параметр `replicator: bool` (вызывающие обнови на `false` по
  умолчанию, если это не логин-путь).
- `struct UserDirectoryState` (строка ~205-220) — добавь
  `pub replicator: bool` рядом с `pub superuser: bool`.
- Везде, где строится `UserDirectoryState { superuser: ..., ... }` —
  добавь `replicator: user.replicator` (grep `UserDirectoryState {` для
  всех construction site).

### 2. `set_replicator` метод

Рядом с `set_superuser` (строка ~690-732) — скопируй ПОЧТИ буквально, БЕЗ
last-remaining guard и БЕЗ `superuser_count`-подобного счётчика (не
заводи новый счётчик — не нужен, ничего не гейтится по "сколько их
осталось"):

```rust
pub fn set_replicator(&self, username: &str, on: bool, now_ns: u64) -> Result<bool> {
    let _guard = self.write_lock.lock();

    let blob = match self.read_blob(username)? {
        Some(b) => b,
        None => return Err(Error::InvalidInput("user not found")),
    };
    let mut user: PersistedUser = rmp_serde::from_slice(&blob)
        .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

    if user.replicator == on {
        return Ok(false); // already at the requested state — no-op
    }

    user.replicator = on;
    // Spec §12.6-style: privilege change must invalidate existing sessions.
    if now_ns > user.tickets_invalid_before_ns {
        user.tickets_invalid_before_ns = now_ns;
    }
    let new_bytes = rmp_serde::to_vec_named(&user)
        .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
    self.users
        .insert(username.as_bytes(), new_bytes.as_slice())
        .map_err(|e| Error::Encoding(format!("fjall: insert: {e}")))?;
    self.db
        .persist(PersistMode::SyncAll)
        .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

    if let Some(id) = user.user_id_array() {
        self.update_cache(&id, user.tickets_invalid_before_ns);
    }
    Ok(true)
}
```

(Подгони под точное имя trait/impl — `set_replicator` должен жить там же,
где `set_superuser`, скорее всего как метод конкретного impl'а
`FjallUserDirectory`, не обязательно как часть публичного trait, если
`set_superuser` тоже не часть более общего trait — проверь.)

### 3. Резервирование строки в `update_roles`

Строка ~999-1003 — добавь рядом:

```rust
if roles.iter().any(|r| r == "replicator") {
    return Err(Error::InvalidInput(
        "\"replicator\" is a reserved role name — use SetReplicator to grant/revoke replication access",
    ));
}
```

### 4. `SessionPermissions`

`crates/shamir-connect/src/server/session.rs`:
- `struct SessionPermissions` — добавь `pub is_replicator: bool`.
- `from_roles` — `is_replicator = roles.iter().any(|r| r == "replicator")`
  (тот же паттерн, что `is_superuser`).
- `new(is_superuser: bool, roles: Vec<String>)` — переименуй/расширь
  сигнатуру до `new(is_superuser: bool, is_replicator: bool, roles: Vec<String>)`
  (это ЛОМАЕТ существующих вызывающих — это ОК, найди и почини ВСЕ
  call sites: `handshake.rs:427`, `resume.rs:423`, плюс любые тесты,
  использующие `SessionPermissions::new` — grep их все).
- `has_role` — оставь как есть (used elsewhere for generic roles, не
  трогай).

### 5. Wire — `DbRequest::SetReplicator`

`crates/shamir-query-types/src/wire/db_message.rs` — добавь вариант
рядом с `SetSuperuser` (копируй структуру буквально):

```rust
/// Grant or revoke replication API access on an existing SCRAM-directory
/// account (task #621 — mirrors SetSuperuser's shape/gate exactly, no
/// last-remaining guard). Requires an already-superuser session AND an
/// HMAC confirmation tag.
SetReplicator {
    user: String,
    on: bool,
    hmac: Option<String>,
},
```

`crates/shamir-query-types/src/hmac.rs` — добавь рядом с
`canonical_set_superuser`:

```rust
pub fn canonical_set_replicator(user: &str, on: bool) -> Vec<u8> {
    join_null(&[
        b"set_replicator",
        user.as_bytes(),
        if on { b"true" } else { b"false" },
    ])
}
```

### 6. Handler

`crates/shamir-server/src/db_handler/admin.rs` — добавь `set_replicator`
функцию рядом с `set_superuser` (строка ~206-272), копируя буквально
структуру (permission gate → HMAC gate → op), БЕЗ last-remaining
специфики (замени на вызов `admin.user_dir.set_replicator(&user, on, now_ns)`
вместо `set_superuser`; коды ошибок: тот же `not_found`/`query` fallback,
БЕЗ `invalid_owner` — этот код специфичен для last-superuser отказа,
которого здесь нет).

`crates/shamir-server/src/db_handler/handler.rs` — добавь диспатч-ветку
`DbRequest::SetReplicator { user, on, hmac } => set_replicator(self.admin.as_ref(), session, user, on, hmac).await,`
рядом с существующей `SetSuperuser`-веткой.

### 7. Реальные call sites `SessionPermissions::new`

`crates/shamir-server/src/connection/handshake.rs:427` и
`crates/shamir-connect/src/server/resume.rs:423` — обнови на
`SessionPermissions::new(user_state.superuser, user_state.replicator, user_state.roles)`
(подставь точное имя переменной state — `user_state`/`state` смотри по
контексту каждого файла).

### 8. Сам гейт в `repl_handler.rs`

`crates/shamir-server/src/db_handler/repl_handler.rs:39,46` — убери
`const REPLICATOR_ROLE`, замени проверку на:

```rust
if !(session.permissions.is_superuser || session.permissions.is_replicator) {
```

### 9. Тесты

- `crates/shamir-server/tests/hmac_gate.rs` (уже есть паттерн для
  `SetSuperuser` — ищи там тесты на `set_superuser` HMAC missing/wrong/
  correct) — добавь симметричные 3 теста для `SetReplicator`.
- Тест, что обычная роль `["replicator"]` через `GrantRole`/`update_roles`
  теперь отклоняется (`"replicator" is a reserved role name"`), симметрично
  существующему тесту на `"superuser"` reserved (найди его как образец,
  вероятно в `crates/shamir-server/tests/` или `user_directory`-related
  тестах).
- Тест на `repl_handler.rs`'s гейт: сессия с `is_replicator=true` (но не
  superuser) допускается к replication API; сессия без обоих флагов —
  нет. Если уже есть такой тест через строковую роль — обнови его под
  новый флаг вместо роли.

## Прогон проверок

- `cargo fmt -p shamir-query-types -p shamir-connect -p shamir-server -- --check`
- `cargo clippy -p shamir-query-types -p shamir-connect -p shamir-server --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-connect -p shamir-server --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ добавляй last-remaining-replicator guard — не нужен, разное от
  superuser по природе инварианта.
- НЕ добавляй счётчик наподобие `superuser_count` для replicator — не
  нужен, ничего не гейтится по количеству.
- НЕ пытайся мигрировать существующие персистентные записи со строкой
  "replicator" в роли — такой миграции не нужно, строка никогда раньше
  не была валидной ролью в продакшене (просто зарезервируй с этого
  момента, `#[serde(default)]` на новом поле покрывает старые записи
  без него).
- НЕ трогай `superuser`-специфичный код (last-remaining guard,
  `superuser_count`) — не в scope.

## Проверка (сделает оркестратор)

- Диф ограничен `user_directory.rs`, `session.rs`, `db_message.rs`,
  `hmac.rs`, `admin.rs`, `handler.rs`, `repl_handler.rs`,
  `handshake.rs`, `resume.rs`, плюс тесты.
- fmt/clippy чисты.
- `./scripts/test.sh` по перечисленным крейтам зелёный, включая новые
  тесты.
- Все существующие call sites `SessionPermissions::new`/`::from_roles`
  компилируются (grep confirm, не только перечисленные 2 продакшен-сайта
  — есть ещё бенчи/тесты, использующие `from_roles`, у них НЕ меняется
  сигнатура, только `new`).
