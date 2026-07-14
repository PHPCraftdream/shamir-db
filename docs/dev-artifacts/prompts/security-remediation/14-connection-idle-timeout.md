# Brief: post-auth connection idle timeout (taskId #616 pt.3, low severity)

## Контекст

Аудит `docs/dev-artifacts/audits/2026-07-06-client-surface-parity.md`,
§2.2 — среди прочего называет: сервер не защищён от клиента, который
успешно аутентифицировался и затем НАВСЕГДА замолчал (никогда больше не
шлёт фрейм). Client-side таймауты (`request_timeout`/`connect_timeout`
в Rust, `requestTimeoutMs`/`connectTimeoutMs` в TS) УЖЕ реализованы
(помечены "Finding 2.2" в комментариях обоих клиентов — эта часть уже
закрыта в прошлой сессии, не трогай). Единственное, что реально
ОТСУТСТВУЕТ — **server-side idle timeout ПОСЛЕ аутентификации**.

`crates/shamir-server/src/connection/connection_context.rs:53-58` —
`auth_init_timeout` бьёт ТОЛЬКО до первого `auth_init` фрейма (ещё до
аутентификации, `handshake.rs:567-584`). После входа в
`request_loop.rs`'s reader loop (строки ~253-271) —

```rust
tokio::select! {
    read_res = reader.read_frame_into(MAX_FRAME_SIZE_DEFAULT, &mut frame_buf) => { ... }
    _ = &mut writer_handle => { ... }
}
```

— НЕТ таймаута вообще. Аутентифицированный клиент, который открыл
соединение и больше никогда не шлёт фрейм, держит connection task +
session slot + TCP socket БЕСКОНЕЧНО.

## Задача

### 1. Новый tunable

`crates/shamir-tunables/src/lib.rs`, `instance_defaults` (рядом с
`CONN_MAX_IN_FLIGHT`/`POST_AUTH_RATE_LIMIT_PER_SEC`) — добавь:

```rust
/// Maximum idle time on an authenticated connection before the server
/// closes it (task #616 pt.3). Resets on every frame received. A
/// generous default — legitimate clients send SOMETHING (even a Ping)
/// well within this window; a silent connection past it is either dead
/// or abandoned and should not hold a session slot + socket forever.
pub const CONN_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
```

### 2. `ConnectionContext` — новое поле

`crates/shamir-server/src/connection/connection_context.rs` — добавь
`pub idle_timeout: Duration` рядом с `pub auth_init_timeout: Duration`
(строка ~58), обнови конструктор (строка ~101,121) соответственно.

### 3. Гейт в `request_loop.rs`

Найди `tokio::select!` блок (строки ~253-271, внутри `'conn: loop`) —
добавь ТРЕТЬЮ ветку с таймером, который СБРАСЫВАЕТСЯ на каждой успешно
прочитанной frame (не общий wall-clock таймаут запроса — именно idle,
т.е. "сколько времени прошло с ПОСЛЕДНЕГО фрейма"):

```rust
tokio::select! {
    read_res = reader.read_frame_into(MAX_FRAME_SIZE_DEFAULT, &mut frame_buf) => {
        match read_res {
            Ok(()) => {}
            Err(_) => {
                drop(permit);
                break;
            }
        }
    }
    _ = &mut writer_handle => {
        drop(permit);
        writer_done = true;
        break;
    }
    _ = tokio::time::sleep(ctx.idle_timeout) => {
        // No frame arrived within the idle window — close the
        // connection (task #616 pt.3). Not an error path per se, just
        // reclaiming an abandoned/dead connection's resources.
        tracing::info!(idle_timeout_secs = ctx.idle_timeout.as_secs(), "connection idle timeout");
        drop(permit);
        break;
    }
}
```

(Проверь точное место вставки — `tokio::select!` пересоздаётся на каждой
итерации цикла `'conn: loop`, так что `tokio::time::sleep(ctx.idle_timeout)`
внутри select ЕСТЕСТВЕННО пересоздаётся/сбрасывается на каждой итерации —
это и даёт "idle с момента последнего фрейма" семантику без ручного
управления таймером. Не нужен отдельный `Instant`/reset-логика.)

Обнови teardown-комментарий вверху файла (строки ~25-29 "Teardown on
any exit path") — добавь idle-timeout как ещё один exit path.

### 4. Прокинуть конфиг в `server_launcher.rs`

Найди, где `ConnectionContext` реально конструируется в
`crates/shamir-server/src/server/server_launcher.rs` (grep
`auth_init_timeout` для образца) — добавь
`idle_timeout: shamir_tunables::instance_defaults::CONN_IDLE_TIMEOUT`
рядом.

## Тесты

Найди существующий тест на `auth_init_timeout` (grep в
`crates/shamir-server/tests/` — вероятно есть e2e-тест на timeout при
handshake) и напиши симметричный: аутентифицированная сессия, которая
после успешного логина НЕ шлёт ни одного запроса — сервер должен
закрыть соединение по истечении `idle_timeout`. Для теста используй
явно МАЛЕНЬКОЕ значение `idle_timeout` (передай через тестовый
`ConnectionContext`/`make_test_config`-подобный хелпер, не полагайся на
дефолтные 600 секунд) — проверь, что соединение реально закрывается
(например, попытка чтения с клиентской стороны сокета получает EOF/
закрытие в разумное время после `idle_timeout`, не раньше и не
намного позже).

## Прогон проверок

- `cargo fmt -p shamir-tunables -p shamir-server -- --check`
- `cargo clippy -p shamir-tunables -p shamir-server --all-targets -- -D warnings`
- `./scripts/test.sh -p shamir-tunables -p shamir-server --full`

## Что НЕЛЬЗЯ делать

⛔ НИКОГДА не запускай `git reset` / `checkout` / `clean` / `stash` /
`restore` / `rm`, `git commit`, `git push`, или любую git-команду, которая
меняет рабочее дерево, индекс или историю. Только редактируй файлы —
оркестратор сам проверит и закоммитит.

- НЕ трогай `auth_init_timeout` — отдельный, уже правильно
  реализованный механизм для ДО-аутентификационной фазы.
- НЕ трогай client-side timeout код (`shamir-client/src/client.rs`,
  `shamir-client-ts/src/core/client.ts`) — уже реализовано в прошлой
  сессии (Finding 2.2), НЕ в scope этой задачи.
- НЕ путай idle-timeout с per-request wall-clock timeout
  (`max_execution_time_secs`, отдельный существующий механизм для
  долгих `Execute`-запросов) — это разные концепции, не смешивай их.

## Проверка (сделает оркестратор)

- Диф ограничен `lib.rs` (shamir-tunables), `connection_context.rs`,
  `request_loop.rs`, `server_launcher.rs` (все shamir-server), плюс
  новый тест.
- fmt/clippy чисты.
- `./scripts/test.sh -p shamir-tunables -p shamir-server --full`
  зелёный, включая новый тест на idle-timeout закрытие соединения.
