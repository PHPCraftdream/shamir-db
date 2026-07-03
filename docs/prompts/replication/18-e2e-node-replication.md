בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# E2E репликации в tests/e2e (napi repl-метод + 16-replication.test.js)

> Контекст: верхнеуровневый чёрный-ящик e2e `tests/e2e/` гоняет Node-биндинг
> `shamir-client-node` против живого `shamir-server`. Репликации там нет.
> Rust `shamir-client` уже имеет `Client::repl()` (3d81db90); napi-биндинг —
> нет. R0 pull-API (Hello/Pull) исполним end-to-end уже сейчас.

## Часть A — napi-метод `repl`

В `crates/shamir-client-node/src/lib.rs` добавить по ТОЧНОМУ образцу
`execute` (строки ~196-208):

```rust
/// Privileged replication pull-API (REPLICATION §5). Takes a msgpack
/// `ReplRequest` Buffer, returns a msgpack `ReplResponse` Buffer. The
/// session must hold the `replicator` role (or be superuser).
#[napi]
pub async fn repl(&self, req: Buffer) -> Result<Buffer> {
    // FFI boundary — raw serde is the sanctioned exception (CLAUDE.md):
    // we deserialize a request that ARRIVED as bytes, not construct a query.
    let repl_req: shamir_client::ReplRequest = rmp_serde::from_slice(&req[..])
        .map_err(|e| Error::from_reason(format!("invalid repl payload: {e}")))?;
    let guard = self.inner.lock().await;
    let client = guard.as_ref().ok_or_else(|| Error::from_reason("client closed"))?;
    let resp = client.repl(repl_req).await.map_err(to_napi)?;
    let bytes = rmp_serde::to_vec_named(&resp)
        .map_err(|e| Error::from_reason(format!("encode repl response: {e}")))?;
    Ok(Buffer::from(bytes))
}
```
Сверь реэкспорт `ReplRequest`/`ReplResponse` из `shamir_client` (lib.rs
реэкспортит `shamir_query_types::wire`; если `ReplRequest` не виден —
добавь `pub use` в shamir-client lib.rs или импортируй через
`shamir_query_types::wire::repl::ReplRequest`, что уже доступно как
зависимость). Проверь `crates/shamir-client-node/Cargo.toml` — если
`shamir-query-types` не в deps, а тип нужен, реэкспортни через `shamir_client`.

## Часть B — `tests/e2e/tests/16-replication.test.js`

По образцу `08-admin-ddl.test.js` (`module.exports = async function
({ client, fixtures, test, assert, assertEq }) => {...}`). Хелперы:
кодирование msgpack — глянь, как `client.execute` получает `Buffer`
(helpers кодируют batch; для repl нужно закодировать `ReplRequest` в
msgpack — используй тот же msgpack-энкодер, что и остальной suite; см.
`tests/e2e/helpers/` и как строится `batch` Buffer в других тестах).

Сценарии:
1. **Setup:** как superuser (bootstrap admin) создать db `app` + repo
   `main` + таблицу `items`, записать несколько строк (обычный
   `client.execute` batch). Создать replicator-юзера:
   `client.create_scram_user("repl", "pw", ["replicator"])`. Дать ему
   доступ к `app/main` (chmod/chown через admin batch ИЛИ, если сложно,
   создать ресурсы так, чтобы replicator имел доступ; сверь как это
   делают permission-тесты — либо replicator superuser+replicator как
   fallback с комментарием).
2. **ReplHello:** подключиться как `repl` (второй `ShamirClient.connect`),
   закодировать `{ repl_op: "hello", proto_ver: 1, node_id: "n1" }` в
   msgpack, `client.repl(buf)`, декодировать ответ → проверить
   `repl_kind === "hello"`, `leader_epoch === 1`, `repos` содержит app/main.
3. **ReplPull:** `{ repl_op: "pull", db: "app", repo: "main",
   from_version: 0, limit: 100 }` → `repl_kind === "pull"`,
   `current_version > 0`, `events` (bytes) непустые.
4. **Deny-by-default:** создать обычного юзера без роли, `repl` Hello →
   `repl_kind === "error"`, `code === "bad_role"`.

Форма wire ReplRequest/ReplResponse — см.
`crates/shamir-query-types/src/wire/repl.rs` (repl_op/repl_kind
snake_case-теги, `events` — serde_bytes).

## Окружение (ВАЖНО)

`shamir-client-node` — MSVC-only, собирается отдельно (`npm run build` =
`cargo build --release -p shamir-server` + `napi build`). В этом окружении
сборка napi может быть НЕДОСТУПНА. Тогда:
- Напиши код (napi-метод + JS-тест) корректно.
- Попробуй `cargo build -p shamir-client-node` (только компиляция Rust-
  части, без napi) — если toolchain есть; если нет, отметь.
- НЕ проваливай задачу из-за невозможности запустить `npm test` в этом
  окружении — это ожидаемо. Верификация napi-метода — по компиляции
  где возможно + ревью; JS-тест — по соответствию образцу 08.
- В финальном сообщении ЧЕТКО укажи: собралось/не собралось, что
  проверено, что требует MSVC-хоста для прогона.

## Definition of done

- napi `repl` метод в lib.rs (по образцу execute, FFI-serde exception).
- `16-replication.test.js` со сценариями Hello/Pull/deny.
- Компиляция Rust-части где возможно; JS-тест соответствует образцу.
- Финальное сообщение: статус сборки, что верифицировано, MSVC-требования.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.
