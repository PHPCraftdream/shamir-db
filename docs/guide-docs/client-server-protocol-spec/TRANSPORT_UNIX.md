# Transport: Unix / Named Pipe (Local IPC)

Local-machine transport — Unix domain socket on POSIX, Windows Named Pipe
on Windows. One protocol section covers both: the wire-level handshake is
byte-identical on both OS primitives (`shamir-transport-ipc` exposes them
behind the same `IpcListener`/`IpcStream` API — callers, including this
spec, never need to distinguish which OS primitive is live).

## 1. Profile

| Профиль | URI | transport_kind | binding_mode | Использование |
|---|---|---|---|---|
| `unix` | `shamir+unix://...` | `0x03` | `0x00` (none) | Same-host embedded / sidecar deployments. |

Единственный профиль — нет `tls`/`plain` развилки, как у TCP: local IPC
никогда не несёт TLS (см. §5 Access control — OS-level boundary заменяет
transport encryption).

## 2. Framing

Идентично TRANSPORT_TCP.md §2 — length-prefixed msgpack:
```
[length: u32 BE][msgpack: length bytes]
```
Переиспользуется буквально: `shamir_transport_tcp::framing::{read_frame,
write_frame, ...}` работает над `IpcStream` без изменений (`UnixStream` и
`NamedPipeServer` оба реализуют `AsyncRead + AsyncWrite`).

- Empty frame (length=0) — graceful close
- До auth_ok: `length ≤ MAX_PRE_AUTH_FRAME = 4 KB`
- После auth_ok: `length ≤ MAX_FRAME_SIZE_DATA = 16 MB`
- Frame too large → close без reply

## 3. Нет TLS

Ни TLS 1.3, ни TLS 1.2 — эта развилка не существует для local IPC.
`binding_mode` всегда `0x00`, `tls_exporter_or_zeros` всегда
`bytes(32)` all-zeros (та же семантика, что у `tcp+plain`, см.
TRANSPORT_TCP.md §4).

## 4. Channel Binding в auth_message

| Профиль | binding_mode | tls_exporter_or_zeros |
|---|---|---|
| unix | `0x00` | bytes(32) zeros |

## 5. Access control [NORMATIVE]

Отсутствие TLS компенсируется OS-level access boundary на САМ канал —
кто вообще может открыть сокет/pipe — а не заменой SCRAM-аутентификации:
SCRAM (полный Argon2id handshake) выполняется как обычно на первом
подключении; OS boundary решает **кто может дойти до auth_init**, SCRAM
решает **каким DB-пользователем он аутентифицируется**. Это дополняющие,
а не взаимозаменяющие механизмы.

### 5.1. Unix domain socket
Владелец сокет-файла — процесс сервера. `IpcListener::bind` выставляет
`0600` (owner read/write only) СРАЗУ после `bind`, до принятия первого
соединения — сервер отказывает в старте, если `chmod` не удался.

### 5.2. Windows Named Pipe
Каждый pipe instance создаётся с явным DACL, ограничивающим доступ SID-у
текущего процесса-сервера (`D:P(A;;GA;;;<sid>)` — protected DACL,
Generic-All единственному principal). `PIPE_REJECT_REMOTE_CLIENTS`
установлен (tokio's `ServerOptions` default) — pipe недостижим по SMB с
удалённого хоста независимо от DACL.

### 5.3. Что это НЕ означает
Access control ограничивает **транспортный** доступ (кто может открыть
канал), не то же самое что DB-level авторизация — тот же SCRAM
username/password обмен, что и на TCP/WS, всё ещё аутентифицирует
конкретного DB-пользователя после того, как канал открыт.

## 6. Session resumption через IPC

Ticket-based resume (SESSION_RESUMPTION.md) работает над `unix`
транспортом без изменений: `binding_mode_at_auth = 0x00` — та же
strength-0 категория, что и `tcp+plain`. Cross-transport матрица
(SESSION_RESUMPTION.md §6.4) расширяется на `unix` идентично строке
`plain`:

| at_auth | now | Allowed? |
|---|---|---|
| unix | unix | ✓ same tier |
| unix | tcp+tls / ws | ✓ upgrade |
| plain (tcp) | unix | ✓ same tier (strength 0 → strength 0) |
| tcp+tls / ws | unix | ✗ DOWNGRADE — reject |

`disable_plain_ticket_upgrade` (SESSION_RESUMPTION.md §6.4) применяется
к `unix` так же, как к `tcp+plain` — обе имеют `binding_strength = 0`.

## 7. Connection Lifecycle

```
Unix socket connect / Named Pipe открыт
  ▼
auth_init (frame 1)      -- НЕТ TLS-этапа
  ▼
challenge (frame 2)
  ▼
client_proof (frame 3)
  ▼
auth_ok ИЛИ error (frame 4)
  ▼
[active session — {sid, req} ↔ {rid, res}]
  ▼
close ИЛИ logout ИЛИ idle timeout
```

Один канал = одна active session. Повторный auth_init → close (как у
TCP).

## 8. Session Frame Format

Идентично TRANSPORT_TCP.md §6 — без изменений (транспорт-независимый
слой):
```
{
  "sid": bytes(32),
  "req": { ... }
}
```
Response:
```
{
  "rid": Optional<u32>,
  "res": { ... } | "error": "..."
}
```

## 9. Endpoint addressing

Нет host:port — путь к сокету (Unix) или имя pipe (Windows). Единый URI
вид, платформа резолвится клиентом:
```
shamir+unix://alice@/run/shamir/db.sock       # Unix — абсолютный путь
shamir+unix://alice@shamir-db                 # любая ОС — логическое имя;
                                               # Windows клиент маппит его
                                               # в \\.\pipe\shamir-db
```

## 10. Bootstrap / first-admin provisioning

IMPLEMENTATION_GUIDE.md §2.2 documents that WIRE-PROTOCOL bootstrap (§11
AUTH_PROTOCOL.md) requires `binding_mode == 0x01` — a `unix` listener
(`binding_mode = 0x00`) is in the same position as `tcp+plain` here: no
runtime "first client to connect over this channel becomes admin" flow.

This is not a gap in practice: the server's existing OUT-OF-BAND
provisioning path — `BootstrapMode::Password` / `BootstrapMode::RandomToken`
(`--bootstrap-password` / default random-token-to-file CLI flags) — runs
during `ServerLauncher::launch()` **before any listener's accept loop
starts**, entirely independent of transport or `binding_mode`. It writes
the admin account directly into the user directory, not through the wire
protocol at all. This already matches IMPLEMENTATION_GUIDE §2.2's option
(c) ("Pre-provision admin user через CLI tool, не через wire protocol") —
and, per `crates/shamir-client/tests/smoke_local.rs` and this spec's own
`connectLocal` e2e coverage, it works over the `unix` transport unmodified:
no new provisioning code was needed for this transport.

## 11. Test Checklist

- Round-trip auth через Unix socket, через Named Pipe
- Frame too large → close без reply
- Empty frame → graceful close
- Повторный auth_init → close
- Socket file создаётся с правами `0600` (Unix); DACL ограничен SID
  текущего пользователя (Windows) — сторонний OS-принципал получает отказ
- `PIPE_REJECT_REMOTE_CLIENTS` активен — pipe недостижим удалённо (Windows)
- Ticket issue + resume через `unix` транспорт; `unix → unix`,
  `unix → tcp+tls`, `plain → unix` разрешены; `tcp+tls → unix` отвергнут
  (downgrade)
- Второй клиент обслуживается после отключения первого (проверяет
  instance-rotation у Named Pipe listener — у Unix socket не требуется,
  `accept()` тривиально повторно входит)
- Audit event `auth_success` содержит `transport: "unix"`
