# Production Server — current state + remaining work

После сессии получили **`crates/shamir-server`** — production-server
крейт с 7 модулями (3500+ строк), который связывает `shamir-connect` +
transport-биндинги + durable backends в runnable binary. **Сборка
чистая** через `cargo check`. Тесты по модулям пройдены агентами при
их запуске; полный `cargo test` workspace упирается в долгое linking
parent-крейта `shamir-db` на Windows и не завершён в рамках сессии.

## Что готово

| Модуль | Файл | Строк | Тесты | Статус |
|--------|------|-------|-------|--------|
| User directory (durable) | `user_directory.rs` | 338 | 8/8 | ✅ |
| Audit appender (HMAC-chain + JSON-line) | `audit_appender.rs` | 492 | 6/6 | ✅ |
| Server meta store (secrets + identity + bootstrap) | `server_meta.rs` | 485 | 9/9 | ✅ |
| ktav config schema | `config.rs` | 361 | 9/9 | ✅ |
| Background scheduler | `scheduler.rs` | 303 | written | 🟡 not validated this session |
| DB handler bridge | `db_handler.rs` | 355 | 7/7 | ✅ |
| Connection orchestration | `connection.rs` | ~600 | none | 🟡 compiles, needs e2e |
| Main binary | `main.rs` | ~150 | none | 🟡 compiles, listeners not yet bound |

Плюс: **WS profile enforcement** (`shamir-transport-ws::listener`) — 9
тестов, release-blocker закрыт.

## Что НЕ готово (gaps)

### 1. E2E integration test (`tests/e2e_full_pipeline.rs`)

Что покрыть:
- Поднять server in-process с TempDir для всех redb-файлов.
- shamir-connect HandshakeBuilder клиент → SCRAM → request → response.
- Verify: durable state survives restart (lockout state, counters,
  audit chain, sessions invalidated).

Размер: ~250 строк.

### 2. Connection.rs gaps

- `tickets_invalid_before_ns` lookup по `user_id` (для §7.5 check) —
  сейчас всегда возвращает 0. Нужен secondary index `user_id -> name`
  в `RedbUserDirectory` или прямой lookup по uid.
- Real audit emit с `details_canonical_msgpack` — сейчас всегда пустой
  Vec.
- Identity rotation orphan recovery — `complete_auth_ok` не вызывается;
  `auth_ok` не несёт `rotation_in_progress` payload даже во время
  overlap window. Wiring через `build_rotation_in_progress_payload` +
  `with_rotation_in_progress`.
- Resumption ticket issuance — `issue_initial_ticket` не вызывается;
  `auth_ok` не несёт `resumption_ticket`. Wiring через `complete_auth_ok`.
- KDF upgrade detection — `needs_kdf_upgrade` не зовётся.
- Idle TTL grace на disconnect — сессия удаляется не через 5s grace,
  а сразу при close (через session GC). Spec §7.8 говорит 5s grace
  для resume через transport-switch.

### 3. Main.rs gaps

- **Listeners не bind'ятся**. Сейчас main только ждёт SIGTERM — accept
  loops для каждого `ListenerConfig` ещё не написаны. Нужно ~150
  строк: для каждого listener создать `TcpListener` через
  `bind_validated`, в цикле accept → spawn `connection::handle_connection`.
- TLS cert generation/load на первом старте — сейчас `TlsConfig.cert_path`
  / `key_path` не используются.
- WS endpoint dispatch — для WS листенеров после TLS accept нужен
  `accept_native_ws` или `accept_browser_ws` (с `BrowserOriginPolicy`),
  потом адаптация фрейминга через `ws_send` / `ws_recv_into` поверх
  обычного `read_frame_into` / `write_frame_into` интерфейса.
- Bootstrap CLI flags: `--regen-bootstrap`, `--print-bootstrap-token`.
- Identity rotation admin command wiring (rotateServerIdentity).
- Audit emit на server boot: `server_started`.

### 4. Полный `cargo test --workspace` валидация

В сессии не прошёл из-за времени linking parent-crate `shamir-db`.
Нужно прогнать в отдельном окне с `--release` (быстрее link) или с
`mold`/`lld` linker.

## Итоговая трудоёмкость для v1 ship

- E2E test: ~3 часа (включая отладку TLS handshake в test fixture).
- Connection.rs gaps: ~4 часа (все 6 пунктов).
- Main.rs listeners: ~2 часа.
- TLS cert lifecycle: ~1 час.
- Bootstrap CLI: ~1 час.
- WS dispatch: ~1 час.
- Полный test sweep + bug fix iteration: ~3 часа.

**Итого: 15-18 часов.**

## Рекомендуемая последовательность

1. **Прогнать `cargo test --workspace`** в свежей сессии (без других
   cargo процессов). Зафиксировать что 32 + 7 + новых = ~40 тестов
   `shamir-server` действительно проходят.
2. **E2E test**: критично — без него wiring не валидирован.
3. **Main listeners**: критично — без них сервер не принимает соединения.
4. **Connection gaps в порядке security severity**:
   1. tickets_invalid_before_ns lookup (§7.5 — release blocker).
   2. complete_auth_ok wiring (resumption + rotation).
   3. Idle TTL grace.
   4. Audit details + KDF upgrade.
5. **Bootstrap CLI + TLS lifecycle**.
6. **Final sweep + audit chain verify on startup**.

## Минимальный путь до runnable demo

Если нужно **просто доказать что протокол работает end-to-end** в
production server context (без полной hardening), достаточно:

1. Дописать accept loop в main.rs (~100 строк).
2. Один e2e test (~150 строк).

После этого можно `cargo run -p shamir-server -- --config example.ktav`
и реально подключиться `shamir-connect` клиентом.
