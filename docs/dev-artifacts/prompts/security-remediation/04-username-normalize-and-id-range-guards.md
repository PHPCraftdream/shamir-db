# Brief: username normalize on create + numeric range guards (taskId #605, audit residual, P1)

Две независимые, но сгруппированные аудитом мелкие находки. Можно делать
последовательно одним патчем/коммитом — они не пересекаются по файлам,
кроме обоих живущих в security-hygiene кластере.

## Часть A — username не нормализуется на create-пути

### Контекст

`crates/shamir-server/src/connection/handshake.rs:199` (login-путь):

```rust
let username = match NormalizedUsername::from_raw(&init.user) {
    Ok(u) => u,
    Err(_) => return Err(HandshakeError::Decode),
};
```

`NormalizedUsername::from_raw` (`crates/shamir-connect/src/common/username.rs:40`)
применяет RFC 8265 PRECIS `UsernameCaseMapped` (width mapping → case
fold → NFC → directionality/restriction checks) — это защита от
confusable-username атак (два визуально одинаковых, но байтово разных
username, которые логин трактует как один и тот же аккаунт, а create —
как два разных).

`crates/shamir-server/src/db_handler/admin.rs`, функция `create_scram_user`
(строки ~84-168, уже трогали её в задаче #604 — HMAC-гейт теперь первым
делом ПОСЛЕ permission-check) — принимает `name: String` **прямо с wire**
и передаёт его В СЫРОМ ВИДЕ в `admin.user_dir.insert(name.clone(), record)`
(строка ~134) и в `DbResponse::UserCreated { name, ... }` (эхо обратно
клиенту). Никакой нормализации нет. `FjallUserDirectory::insert`
(`crates/shamir-server/src/user_directory.rs:940`) тоже ничего не
нормализует — хранит байты username как есть, ключ = сырая строка.

Итог: можно создать `"Alice"` и `"аlice"` (кириллическая «а») как ДВА
разных аккаунта через `create_scram_user`, но при логине оба
PRECIS-нормализуются в одну и ту же форму — путаница/потенциальный обход
контроля доступа через визуально идентичные имена.

### Задача A

В `create_scram_user` (`crates/shamir-server/src/db_handler/admin.rs`),
СРАЗУ ПОСЛЕ HMAC-гейта (который уже добавлен задачей #604) и ДО
reserved-role проверки — нормализовать `name`:

```rust
use shamir_connect::common::username::NormalizedUsername;

let normalized_name = match NormalizedUsername::from_raw(&name) {
    Ok(n) => n,
    Err(_) => {
        return DbResponse::Error {
            code: "invalid_username".into(),
            message: format!("username '{name}' is not a valid PRECIS UsernameCaseMapped identifier"),
        };
    }
};
let name = normalized_name.as_str().to_string();
```

(Проверь точный путь импорта `NormalizedUsername` — модуль
`shamir_connect::common::username`, уже используется в
`shamir-server/src/connection/handshake.rs`.)

Дальше в функции используй уже нормализованный `name` (переприсвоенный,
как в сниппете выше) — существующий код ниже (`insert`, `update_roles`,
`UserCreated`) не требует структурных изменений, только видит
нормализованную строку вместо сырой.

**Важно:** код ошибки `"invalid_username"` — новый (нет прецедента в
кодовой базе на данный момент, проверено `grep -rn "invalid_username"`).
Используй ровно эту строку — она однозначно называет проблему и не
конфликтует с существующими кодами (`user_exists`, `query`,
`permission_denied`, `hmac_required`, `hmac_mismatch`).

Тест: добавь в `crates/shamir-server/tests/hmac_gate.rs` (там уже есть
инфраструктура для `create_scram_user` из задачи #604 — используй тот же
`build_handler_with_admin` фикстур-хелпер) один новый тест:
create_scram_user с заведомо PRECIS-невалидным именем (например, с
управляющим ASCII-символом внутри, `"al\u{0}ice"`, или любым другим
входом, который `UsernameCaseMapped::enforce` гарантированно отклоняет —
посмотри `crates/shamir-connect/src/common/tests/username_tests.rs` для
готовых примеров невалидных строк) — ожидай `code == "invalid_username"`.

## Часть B — отсутствие range guard перед `u64 as i64`

### Контекст

Group id и user/principal id, попадающие в кодовую базу С WIRE (не
сгенерированные сервером), в некоторых местах напрямую кастуются
`as i64` для хранения/фильтрации (`QueryValue::Int`/`FilterValue::Int`
оба `i64`-based). Если внешний вызывающий пришлёт `id > i64::MAX`
(например, `GroupRef::Id { id: u64::MAX }` или `op.user: u64::MAX` в
`AddGroupMemberOp`/`RemoveGroupMemberOp`), каст молча заворачивается в
отрицательное число — не крах, но неопределённое/сюрпризное поведение
(потенциально путает фильтрацию, не соответствует тому, что вызывающий
на самом деле имел в виду).

Сервер-сгенерированные id (group id — монотонный счётчик от 1 в
`access_control.rs:379-457`; principal64 — уже "63-bit projection" по
конструкции, см. `user_directory.rs:511-516`, `mint_unique_user_id`) —
БЕЗОПАСНЫ по построению, никогда не приближаются к `i64::MAX`. Проблема
только там, где `u64` приходит НЕПОСРЕДСТВЕННО с wire от вызывающего:

1. `crate::query::admin::GroupRef::Id { id }` — принимается
   `resolve_group_id` (`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:703-719`)
   БЕЗ проверки диапазона (`GroupRef::Id { id } => Ok(*id)`, строка ~708).
   Это ЕДИНАЯ функция — все 4 wire-входа
   (`crates/shamir-db/src/shamir_db/execute/admin_access.rs:276,328,366,445`
   — `handle_drop_group`/`handle_rename_group`/`handle_add_group_member`/
   `handle_remove_group_member`) проходят через неё, так что guard нужно
   поставить ОДИН РАЗ здесь, а не в 4 местах.

2. `op.user: u64` в `AddGroupMemberOp`/`RemoveGroupMemberOp`
   (`crates/shamir-db/src/shamir_db/execute/admin_access.rs:366-425` и
   `428-...` — `handle_add_group_member`/`handle_remove_group_member`) —
   используется напрямую в `QueryValue::Int(op.user as i64)` (строка
   ~424) без проверки диапазона.

### Задача B

1. В `resolve_group_id` (`access_control.rs:703-719`), ветка
   `GroupRef::Id { id }` — добавить проверку диапазона ДО `Ok(*id)`:
   ```rust
   crate::query::admin::GroupRef::Id { id } => {
       if *id > i64::MAX as u64 {
           return Err(DbError::Validation(format!(
               "group id {id} exceeds the valid i64 range"
           )));
       }
       Ok(*id)
   }
   ```

2. В `handle_add_group_member` и `handle_remove_group_member`
   (`admin_access.rs`) — добавь такую же проверку для `op.user` СРАЗУ
   ПОСЛЕ существующих group-existence/principal-resolver проверок (не
   меняй порядок уже существующих security-checks, только добавь ЕЩЁ
   ОДНУ проверку перед фактическим вызовом `add_group_member_as`/
   `remove_group_member_as`, либо перед местом, где `op.user` первый раз
   участвует в каком-либо `as i64` касте):
   ```rust
   if op.user > i64::MAX as u64 {
       return Err(err_code(
           "query",
           format!("member user id {} exceeds the valid i64 range", op.user),
       ));
   }
   ```
   (Код ошибки `"query"` — обычный fallback-код для input-валидации в
   этом файле, смотри соседние `err`/`err_code` использования для
   консистентности; не изобретай новый код без необходимости.)

3. Если по ходу работы найдёшь ДРУГИЕ явные wire-входы того же класса
   (сырой `u64` id, попадающий в `as i64` без проверки) — оцени, входят
   ли они в тот же риск-класс (внешний ввод, а не сервер-сгенерированный
   счётчик). Если да — добавь такую же проверку. Если генерация id
   сервер-side и по конструкции безопасна (как `group_id`/`principal64`
   выше) — НЕ трогай, не расширяй задачу без необходимости.

### Тесты для части B

В `crates/shamir-db/src/shamir_db/tests/group_tests.rs` (уже существующий
файл с group-related тестами) добавь:
- Тест: `resolve_group_id(&GroupRef::Id { id: u64::MAX })` возвращает
  `Err`.
- Тест (на уровне `BatchOp`/handler, если так проще технически —
  посмотри как остальные тесты в этом файле или в
  `crates/shamir-db/tests/access_ddl.rs` вызывают `AddGroupMemberOp`):
  `add_group_member` с `op.user = u64::MAX` возвращает `Err`/ошибку с
  соответствующим кодом, не паникует и не создаёт запись.

## Прогон проверок

- `cargo fmt -p shamir-db -p shamir-server -- --check`
- `cargo clippy -p shamir-db -p shamir-server --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-db -p shamir-server --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- Не трогай `mint_unique_user_id`/`create_group_as`'s счётчик-генерацию —
  они безопасны по построению, не в scope.
- Не расширяй нормализацию username на другие пути (login уже
  нормализует, BatchOp::CreateUser — другая, DB-уровневая сущность вне
  scope этой задачи — см. коммент в wire `CreateScramUser`, "Distinct
  from BatchOp::CreateUser").
- Не меняй порядок уже существующих проверок в
  `handle_add_group_member`/`handle_remove_group_member` — только
  добавляй новую проверку, не переставляй существующие.

## Проверка (сделает оркестратор)

- Диф ограничен `admin.rs` (server), `access_control.rs`,
  `admin_access.rs` (оба shamir-db), плюс тестовые файлы.
- fmt/clippy по `shamir-db`/`shamir-server` чисты.
- `./scripts/test.sh -p shamir-db -p shamir-server --full` зелёный,
  включая новые тесты.
- Новые тесты реально ловят регресс (падают на коде без фикса — проверь
  мысленно/временным откатом хотя бы для одного теста на выбор).
