# Brief: bootstrap secrets zeroize/redact (taskId #603, audit residual, P1)

## Контекст

`crates/shamir-connect/src/server/bootstrap.rs:213-227`:

```rust
/// Wire view: client → server `bootstrap` (the actual create-superuser).
#[derive(Debug, Clone)]
pub struct BootstrapRequest {
    /// 32-byte token from operator (out-of-band channel).
    pub token: [u8; 32],
    /// Username (post-NFC + UsernameCaseMapped).
    pub user: NormalizedUsername,
    /// Per-user salt.
    pub salt: [u8; limits::SALT_BYTES],
    /// Stored key (= SHA256(client_key)).
    pub stored_key: [u8; 32],
    /// Server key (= HMAC(salted_password, "Server Key")).
    pub server_key: [u8; 32],
    /// KDF parameters used.
    pub kdf_params: KdfParams,
}
```

`token` (bootstrap secret from the operator) и `server_key` (SCRAM server
key — crypto material) сидят в структуре с обычным `#[derive(Debug, Clone)]`
— значит `{:?}`-логирование где угодно (паника, `dbg!`, случайный
`log::debug!("{:?}", req)`) печатает их plaintext-байты, и `.clone()`
множит число копий секрета в памяти без гарантии затирания.

**Важное наблюдение (уже сделано в этом же файле, используй как образец):**
1. `BootstrapState` (строки 36-59, тот же файл) УЖЕ имеет кастомный `Debug`,
   который редактирует `bootstrap_token_hash` — используй как прямой
   шаблон стиля (`f.debug_struct(...).field(...).finish()`).
2. `zeroize::Zeroizing` уже импортирован в этом файле
   (`use zeroize::Zeroizing;`, строка 26) и уже используется:
   - `issue_token(...) -> Result<Zeroizing<[u8; 32]>>` (строка 108)
   - `try_complete_bootstrap(..., server_key: Zeroizing<[u8; 32]>, ...)`
     (строка 140) — этот метод УЖЕ ожидает `server_key` как `Zeroizing`.
   Значит зависимость и паттерн полностью готовы, просто структура
   `BootstrapRequest` — единственное место, где секрет всё ещё голый `[u8;32]`.
3. `Clone` на `BootstrapRequest` НЕ используется НИГДЕ в кодовой базе —
   проверено `grep -rn "\.clone()" crates/shamir-connect/src/server/bootstrap.rs
   crates/shamir-connect/src/client/bootstrap.rs` (пусто). Значит `Clone`
   можно просто убрать целиком, без замены на secure-clone обёртку.
4. Единственное место, которое КОНСТРУИРУЕТ `BootstrapRequest` — функция
   `build_request` в `crates/shamir-connect/src/client/bootstrap.rs`
   (строки ~85-108):
   ```rust
   Ok(BootstrapRequest {
       token,
       user,
       salt,
       stored_key: derived.stored_key.0,
       server_key: server_key_bytes,
       kdf_params,
   })
   ```

## Задача

1. В `crates/shamir-connect/src/server/bootstrap.rs`:
   - Заменить `pub token: [u8; 32]` → `pub token: Zeroizing<[u8; 32]>`.
   - Заменить `pub server_key: [u8; 32]` → `pub server_key: Zeroizing<[u8; 32]>`.
   - Убрать `#[derive(Debug, Clone)]` над `BootstrapRequest`, оставить
     только (если структура где-то требует `Clone` по факту компиляции —
     проверь `cargo check`; ожидание по анализу выше — не требует).
   - Написать кастомный `impl core::fmt::Debug for BootstrapRequest`,
     редактирующий `token` и `server_key` (как `"<REDACTED>"` или похожий
     маркер, см. стиль `BootstrapState`'s Debug выше в этом же файле).
     Остальные поля (`user`, `salt`, `stored_key`, `kdf_params`) — печатать
     как есть, они не являются тем же классом секрета, что `token`/`server_key`
     (salt и stored_key — производные значения, не сырой пароль/токен;
     решение брифа сознательно ограничено СПИСКОМ из ревью: `token` +
     `server_key`, не расширяй scope без необходимости).
   - `stored_key` НЕ трогать (вне scope — ревью называло только `token` и
     `server_key`).

2. В `crates/shamir-connect/src/client/bootstrap.rs`, функция `build_request`:
   - Поправить конструктор под новые типы полей: `token` уже приходит
     параметром `token: [u8; 32]` — оборачивай в `Zeroizing::new(token)`
     при заполнении структуры (сигнатуру самой `build_request` не трогай,
     если не потребуется — предпочтительно оставить входной параметр
     плоским `[u8; 32]` и оборачивать только на границе конструирования
     `BootstrapRequest`, чтобы не расширять blast radius на вызывающих
     `build_request` код).
   - `server_key: server_key_bytes` → `server_key: Zeroizing::new(server_key_bytes)`.

3. Проверить все остальные места, где поля `token`/`server_key` структуры
   `BootstrapRequest` читаются (если такие появятся при `cargo check` —
   `Zeroizing<T>` реализует `Deref<Target=T>`, так что `&req.token` там,
   где ожидается `&[u8;32]`, должно продолжить работать через deref
   coercion; если где-то нужен owned `[u8;32]` — используй `*req.token`
   или `req.token.to_owned()`/явный `Zeroizing::into_inner`, смотри по
   ошибке компиляции).

4. Прогнать:
   - `cargo check -p shamir-connect --all-targets` — должен собраться.
   - `cargo fmt -p shamir-connect -- --check`
   - `cargo clippy -p shamir-connect --all-targets -- -D warnings`
   - `./scripts/test.sh -p shamir-connect` — существующий bootstrap-related
     тестовый набор должен остаться зелёным (если такие тесты есть —
     `grep -rln "BootstrapRequest\|build_request" crates/shamir-connect/src/**/tests/ crates/shamir-connect/tests/ 2>/dev/null` для ориентира).

5. (Опционально, если легко и не расширяет scope) — добавить один
   маленький unit-тест, подтверждающий, что `format!("{:?}", request)` НЕ
   содержит plaintext байтов `token`/`server_key` (например: сконструировать
   `BootstrapRequest` с известным токеном, `format!("{:?}", req)`, `assert!`
   что байтовое representation токена (`format!("{:?}", token_bytes)` или
   его hex) НЕ встречается в выводе, а строка `"<REDACTED"` встречается).

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит диф и закоммитит.

- Не трогай `BootstrapHello`/`BootstrapChallenge` (соседние структуры в том
  же файле) — их поля (nonce, pub key, signature) не являются секретами
  того же класса и не входят в scope этой задачи.
- Не трогай `stored_key` — не в scope.
- Не меняй сигнатуру `try_complete_bootstrap` — она уже принимает
  `Zeroizing<[u8;32]>` для `server_key`, это уже корректно.
- Не расширяй задачу на server_meta.rs / server_launcher.rs (`shamir-server`)
  — там отдельная реализация bootstrap-состояния, вне scope этой задачи.

## Проверка (сделает оркестратор)

- Диф ограничен `bootstrap.rs` (server) + `bootstrap.rs` (client), без
  побочных правок в несвязанных файлах.
- `cargo check -p shamir-connect --all-targets` зелёный.
- `cargo fmt -p shamir-connect -- --check` и
  `cargo clippy -p shamir-connect --all-targets -- -D warnings` чисты.
- `./scripts/test.sh -p shamir-connect` зелёный.
- Если добавлен redaction-тест — проверить, что он реально ловит plaintext
  (временно закомментировать редактирование Debug и убедиться, что тест
  падает — опционально, но приветствуется).
