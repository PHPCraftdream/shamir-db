# Brief: CreateScramUser HMAC gate (taskId #604, audit residual, P1)

## Контекст

`DbRequest::SetSuperuser` (`crates/shamir-query-types/src/wire/db_message.rs:173-182`)
несёт обязательный `hmac: Option<String>` и гейтится в
`crates/shamir-server/src/db_handler/admin.rs`, функция `set_superuser`
(строки ~206-248) — inline-проверка (НЕ через `check_destructive_hmacs`,
потому что это top-level `DbRequest`, а не `BatchOp`):

```rust
// 3. Inline HMAC gate (this op is NOT a BatchOp, so it bypasses
//    `check_destructive_hmacs`).
use shamir_query_types::hmac as canon;
let canonical = canon::canonical_set_superuser(&user, on);
let Some(tag) = hmac.as_ref() else {
    return DbResponse::Error {
        code: "hmac_required".into(),
        message: "set_superuser missing `hmac` field".into(),
    };
};
if !canon::verify_tag_hex(&session.hmac_key(), &canonical, tag) {
    return DbResponse::Error {
        code: "hmac_mismatch".into(),
        message: "set_superuser `hmac` does not match canonical input".into(),
    };
}
```

`DbRequest::CreateScramUser` (тот же файл, строки ~50-66) — операция
РАВНОЙ чувствительности (создаёт SCRAM-аккаунт, способный логиниться на
сервер) — не несёт `hmac` вообще, и её обработчик `create_scram_user`
(`crates/shamir-server/src/db_handler/admin.rs:84-168`) не делает НИКАКОЙ
HMAC-проверки. Это несимметрично и является находкой аудита.

`session.hmac_key()` (`crates/shamir-connect/src/server/session.rs:225`)
и приватный `derive_hmac_key` (строки 201-209) — HMAC-ключ выводится
**только** из `session_id` (`SHA256("shamir-db hmac key v1\0" || session_id)`),
что по документированному замыслу означает: **клиент, зная свой
собственный `session_id`, обязан уметь сам вычислить тот же ключ** — цитата
из doc-комментария `hmac_key()`:

> "Derived purely from `session_id` via a domain-separated SHA-256, so a
> JS / native client that has the bearer token can compute the same key
> without any extra wire field."

НО сейчас этой функции нигде не существует в публичном виде для клиента —
`derive_hmac_key` приватная (`fn`, не `pub fn`) внутри `Session` в
`shamir-connect`. Единственный существующий вызывающий код
`shamir-client::Client::create_scram_user`
(`crates/shamir-client/src/client.rs:813-838`) — единственный НЕ-тестовый
вызывающий (`crates/shamir-client-node/src/lib.rs:265`, Node.js биндинг).

**Важно:** если просто сделать HMAC обязательным на сервере, не научив
клиент его вычислять, единственный реальный вызывающий (`shamir-client-node`)
сломается (`hmac_required` на каждый вызов). Поэтому задача включает
провести ключ через общую точку, а не только гейт на сервере.

`crates/shamir-connect/src/common/crypto.rs:84` уже содержит публичный
`pub fn sha256(data: &[u8]) -> [u8; 32]` — им можно тривиально
реализовать `derive_hmac_key`'s логику публично, без дублирования кода.

## Задача

### 1. Общая функция вывода HMAC-ключа сессии

В `crates/shamir-connect/src/common/crypto.rs` добавить публичную функцию
(рядом с существующим `sha256`):

```rust
/// Derive a session's HMAC key from its `session_id` — pure
/// `SHA256("shamir-db hmac key v1\0" || session_id)`. Both the server
/// (`Session::hmac_key`) and any client holding its own `session_id` can
/// compute this identically; it adds zero authentication strength beyond
/// the bearer token, but proves "deliberate construction" for confirmation
/// tags on destructive/sensitive top-level ops.
pub fn derive_session_hmac_key(session_id: &[u8; 32]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(22 + 32);
    buf.extend_from_slice(b"shamir-db hmac key v1\0");
    buf.extend_from_slice(session_id);
    sha256(&buf)
}
```

(Точная сигнатура/имя — на твоё усмотрение, если естественнее вписывается
в существующий стиль модуля; главное — публичная, доступная и из
`shamir-connect::server::session`, и из `shamir-client`.)

В `crates/shamir-connect/src/server/session.rs`, `Session::hmac_key`
(строка ~225) и приватный `derive_hmac_key` (строка ~201) — переиспользовать
новую общую функцию вместо дублирования логики (оставь кэширование
`hmac_key_cache.get_or_init(...)` как есть, просто замени тело
`derive_hmac_key` на вызов `crate::common::crypto::derive_session_hmac_key`).

### 2. Wire — добавить `hmac` в `CreateScramUser`

`crates/shamir-query-types/src/wire/db_message.rs`, вариант
`DbRequest::CreateScramUser` (строки ~50-66):

```rust
CreateScramUser {
    name: String,
    password: String,
    #[serde(default)]
    roles: Vec<String>,
    /// Hex-encoded HMAC-SHA256 tag over the canonical form — always
    /// required (unconditional, symmetric with `SetSuperuser`'s gate;
    /// task #604).
    hmac: Option<String>,
},
```

Обнови doc-комментарий над вариантом, упомянув HMAC-гейт (по аналогии с
комментарием над `SetSuperuser`).

### 3. Канонический хелпер для HMAC

`crates/shamir-query-types/src/hmac.rs` — добавить рядом с
`canonical_set_superuser` (строка ~383):

```rust
/// Canonical input for `CreateScramUser`'s HMAC confirmation tag.
/// Password is NEVER part of the canonical input (same convention as
/// `canonical_create_user`) — the tag confirms "you meant to create this
/// account with these roles", not the credential. Roles are joined in the
/// order given — caller and verifier must agree on ordering (no sorting
/// here, matches how the wire field is defined: `Vec<String>` as-is).
pub fn canonical_create_scram_user(name: &str, roles: &[String]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = vec![b"create_scram_user", name.as_bytes()];
    for r in roles {
        parts.push(r.as_bytes());
    }
    join_null(&parts)
}
```

(Смотри `join_null`'s сигнатуру в этом же файле — убедись, что типы
совпадают; `Vec<&[u8]>` — ориентировочно, подгони под реальную сигнатуру
`join_null`.)

### 4. Серверный гейт

`crates/shamir-server/src/db_handler/admin.rs`, функция `create_scram_user`
(строки 84-168) — добавь параметр `hmac: Option<String>` в сигнатуру и
вставь inline-гейт СРАЗУ ПОСЛЕ проверки `is_superuser` (строка ~91-96) и
ДО unwrap'а `AdminGlue` — то есть в ТОЙ ЖЕ позиции, что и `set_superuser`'s
шаги 1→3 (permission-check first, потом hmac gate, потом остальная логика):

```rust
use shamir_query_types::hmac as canon;
let canonical = canon::canonical_create_scram_user(&name, &roles);
let Some(tag) = hmac.as_ref() else {
    return DbResponse::Error {
        code: "hmac_required".into(),
        message: "create_scram_user missing `hmac` field".into(),
    };
};
if !canon::verify_tag_hex(&session.hmac_key(), &canonical, tag) {
    return DbResponse::Error {
        code: "hmac_mismatch".into(),
        message: "create_scram_user `hmac` does not match canonical input".into(),
    };
}
```

Обнови doc-комментарий функции по аналогии с `set_superuser`'s (описать
порядок: permission → hmac → остальная логика).

`crates/shamir-server/src/db_handler/handler.rs:299-303` — прокинь новое
поле в вызов:
```rust
DbRequest::CreateScramUser { name, password, roles, hmac } => {
    create_scram_user(self.admin.as_ref(), session, name, password, roles, hmac).await
}
```

### 5. Клиент — вычисляет тег сам, не ломает существующего вызывающего

`crates/shamir-client/src/client.rs`, метод `create_scram_user`
(строки ~813-838) — НЕ меняй публичную сигнатуру метода (не добавляй
параметр `hmac` — вызывающие снаружи, включая `shamir-client-node`, не
должны знать о HMAC-плюмбинге, клиент вычисляет тег сам используя
`self.session_id`, которое уже есть полем структуры `Client`):

```rust
let tag = {
    let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
    let canonical = shamir_query_types::hmac::canonical_create_scram_user(name, &roles);
    shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
};
let mut req = DbRequest::CreateScramUser {
    name: name.to_string(),
    password: password.as_str().to_owned(),
    roles,
    hmac: Some(tag),
};
```

(Проверь точные пути импорта `derive_session_hmac_key`/`canonical_create_scram_user`/
`compute_tag_hex` под реальную структуру модулей — `shamir_query_types::hmac`
уже используется в `shamir-server`, `shamir_connect::common::crypto` — новый
путь из шага 1. `shamir-client`'s Cargo.toml уже тянет `shamir-query-types`
и `shamir-connect` как зависимости — проверь `cargo check -p shamir-client`
для точных use-путей.)

Обнови doc-комментарий метода, упомянув, что тег вычисляется автоматически.

### 6. Тесты

`crates/shamir-server/tests/hmac_gate.rs` — по аналогии с существующими
тестами на `SetSuperuser`/другие HMAC-гейты в этом файле (ищи
`canon::compute_tag_hex` использования как образец), добавь минимум 3
теста для `CreateScramUser`:
- Missing `hmac` → `code == "hmac_required"`.
- Неверный `hmac` (например, тег посчитан для другого имени) →
  `code == "hmac_mismatch"`.
- Верный `hmac` → успех (`UserCreated`).

Плюс: убедись, что существующие тесты, которые УЖЕ конструируют
`DbRequest::CreateScramUser { ... }` напрямую (без `hmac`) —
`crates/shamir-server/tests/change_password_e2e.rs`,
`crates/shamir-server/tests/db_handler.rs`,
`crates/shamir-server/tests/permission_e2e.rs`,
`crates/shamir-server/tests/repl_pull_e2e.rs` — компилируются и проходят
после добавления обязательного поля. Поле `hmac: Option<String>` не
ломает компиляцию структурных литералов только если ты либо обновишь ВСЕ
эти литералы, добавив вычисленный (или dummy, если тест не про
разрешение — но тогда тест теперь ДОЛЖЕН начать падать на
`hmac_required`, чего быть не должно для тестов, не проверяющих HMAC
специально) тег, либо используешь `..Default::default()`-подобный паттерн
если структура это позволяет (у enum-вариантов нет `Default`, так что
скорее всего нужно проставить реальный `hmac: Some(tag)` во всех этих
местах, вычисленный тем же способом, что в п.6 выше). Пройдись по каждому
файлу и почини компиляцию + семантику (тест не должен начать падать на
`hmac_required`, если раньше проверял что-то другое).

### 7. Прогон проверок

- `cargo check --workspace --all-targets` (переезд поля `hmac` в enum-
  варианте затрагивает несколько крейтов — `shamir-query-types`,
  `shamir-server`, `shamir-client`, `shamir-client-node`; проверь, что всё
  собирается, включая `shamir-client-node`, если он собирается в этом
  окружении — если MSVC-toolchain недоступен и он не собирается на этой
  машине, просто зафиксируй это в отчёте, не блокируй остальное).
- `cargo fmt -p shamir-query-types -p shamir-connect -p shamir-server -p shamir-client -- --check`
- `cargo clippy -p shamir-query-types -p shamir-connect -p shamir-server -p shamir-client --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-query-types -p shamir-connect -p shamir-server -p shamir-client --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит диф и закоммитит.

- Не трогай `BatchOp::CreateUser` / `canonical_create_user` (другая,
  DB-уровневая операция — не путать с top-level `CreateScramUser`).
- Не меняй `set_superuser`'s код кроме случаев, когда общая функция
  `derive_session_hmac_key` требует правки его вызова внутри
  `session.rs` (шаг 1) — сам гейт `set_superuser` не трогай.
- Не пытайся чинить/собирать `shamir-client-node`, если у тебя нет MSVC
  toolchain в этом окружении — просто зафиксируй в отчёте, что этот крейт
  не проверялся локально по независящей от задачи причине (он и так
  исключён из дефолтного workspace build, `Cargo.toml` `exclude`).

## Проверка (сделает оркестратор)

- Диф ограничен перечисленными файлами (crypto.rs, session.rs,
  db_message.rs, hmac.rs, admin.rs, handler.rs, client.rs + тестовые
  файлы), без побочных правок.
- `cargo check --workspace --all-targets` зелёный (кроме
  `shamir-client-node`, если MSVC недоступен — фиксируется явно).
- fmt/clippy по перечисленным крейтам чисты.
- `./scripts/test.sh` по перечисленным крейтам зелёный, включая новые
  3 теста в `hmac_gate.rs`.
- Существующие тесты, конструирующие `CreateScramUser` напрямую, либо
  обновлены и проходят, либо (если тест не про HMAC) не начали падать на
  `hmac_required`.
