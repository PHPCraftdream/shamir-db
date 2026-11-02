# Transport: WebSocket

Только SCRAM WebSocket требует `wss://`; plain SCRAM WS удалён в v1. Bearer WS на listener `kind: http` — отдельный TOKEN_API: нативный `/shamir/v1/ws` использует Authorization, браузерный `/shamir/v1/ws/browser` принимает первый бинарный bearer-кадр. Browser bearer `ws://` разрешён только loopback; native bearer plaintext допускается только по правилам TOKEN_API §11 (loopback по умолчанию либо отдельный native `allow_plaintext_remote` opt-in); это не расширяет browser bearer за loopback. SCRAM endpoints не изменяются.

**Область действия (поправка Token API).** Это правило относится только к SCRAM-listener-ам (`kind: ws`, endpoints `/shamir/v1` и `/shamir/v1/browser`): они остаются только `wss://`. Отдельный `kind: http` bearer listener обслуживает native `/shamir/v1/ws` с Authorization и browser `/shamir/v1/ws/browser` с первым бинарным кадром после upgrade. Для browser bearer `ws://` разрешён только loopback; он не наследует `allow_plaintext_remote`. Native bearer transport использует ровно plaintext policy TOKEN_API §11, включая его явно настраиваемое native разрешение удалённого plaintext. Token endpoints не принимают SCRAM, SCRAM endpoints не принимают tokens.

## 1. Профили

| Профиль | URI | binding_mode | Endpoint |
|---|---|---|---|
| `ws+tls` (native) | `shamir+ws://...` over `wss://` | `0x01` (tls_exporter) | `/shamir/v1` |
| `ws+tls_browser` | `shamir+ws://...` over `wss://` | `0x02` (tls_no_export) | `/shamir/v1/browser` |

Server **two listeners на разных endpoints/портах**: native клиенты подключаются к `/shamir/v1`, browser admin UI — к `/shamir/v1/browser`. Endpoint определяет policy `binding_mode`. **Никакого UA-detection.**

Native клиент пытающийся `/shamir/v1/browser` → допустим (но downgrade), browser пытающийся `/shamir/v1` → fail на handshake (нет TLS exporter API).

Bearer WebSocket на listener `kind: http` — отдельный протокол TOKEN_API, не профиль SCRAM: нативный `/shamir/v1/ws` использует Authorization до upgrade, браузерный `/shamir/v1/ws/browser` — первый бинарный кадр после upgrade. User-Agent не выбирает маршрут. SCRAM `/shamir/v1` и `/shamir/v1/browser`, их binding и handshake остаются без изменений.

The proposed HTTP `POST /shamir/v1/auth/begin` and `POST /shamir/v1/auth/finish` are `kind:http`, not WS or SCRAM routes. Shared enums and SCRAM paths remain unchanged; this is a specification of a proposed, not-yet-implemented feature.

## 2. WebSocket Setup

2.1. WS subprotocol negotiation: клиент шлёт `Sec-WebSocket-Protocol: shamir-v1`. Сервер confirm same. Mismatch → 400.

2.2. WS binary frames только. Text frames → close 1003 (`unsupported data`).

2.3. **Только msgpack** wire encoding. Legacy text-encoding canonical удалён в v1.

2.4. **TLS 1.3 0-RTT запрещён** для WSS (сервер не должен принимать early data). Same as TCP+TLS (TRANSPORT_TCP §3.5) — защита от replay 0-RTT данных + forward secrecy.

## 3. Channel Binding

| Endpoint | binding_mode | tls_exporter_or_zeros |
|---|---|---|
| `/shamir/v1` | `0x01` | TLS-Exporter `EXPORTER-ShamirDB-AUTH-v1` (32 bytes) |
| `/shamir/v1/browser` | `0x02` | bytes(32) zeros |

`binding_mode = 0x02` явно сигнализирует "TLS присутствует, но клиент не имеет API для exporter — browser path". Это **policy decision**, embedded в auth_message → MITM не может switch без поломки proof.

Browser-mode security trade-offs — см. SECURITY_MODEL §4.9 + CLIENT_BROWSER.md §4.

## 4. Connection Lifecycle

```
HTTP GET /shamir/v1{,/browser} + Upgrade: websocket + Sec-WebSocket-Protocol: shamir-v1
  ▼
WS open (binary)
  ▼
auth_init (binary message 1)
  ▼
challenge (binary message 2)
  ▼
client_proof (binary message 3)
  ▼
auth_ok ИЛИ error (binary message 4)
  ▼
[active session]
  ▼
WS close 1000 ИЛИ logout
```

## 5. Session Frame Format

Идентично TRANSPORT_TCP §6 — `{sid, req}` ↔ `{rid, res}` в msgpack-binary WS message.

## 6. Close Codes

| Code | Meaning |
|---|---|
| 1000 | Normal closure (logout / session ended) |
| 1002 | Protocol error (malformed binary message) |
| 1003 | Unsupported data (text frame received) |
| 1008 | Policy violation (rate limit, lockout) |
| 1009 | Message too large |
| 1011 | Server error |
| 4000 | `authentication_failed` |
| 4001 | `unsupported_version` |
| 4002 | `server_busy` |
| 4003 | `heartbeat_timeout` |
| 4010 | `token_sid_mismatch` (только токен-соединения, TOKEN_API.md §10) |
| 4011 | `token_revoked` (только токен-соединения) |
| 4012 | `token_expired` (только токен-соединения) |

Коды 4010-4012 выдаёт только listener `kind: http`; `4003` остаётся `heartbeat_timeout` на обоих видах соединений.

Только для bearer `/shamir/v1/ws/browser`: `4000 authentication_failed` — любая непроверенная ошибка кадра/credentials; `4013 token_channel_denied` и `4014 token_plaintext_write_denied` — только после проверки; `1013 server_busy` — исчерпание active cap после upgrade. Семантика кодов закрытия SCRAM не меняется; native bearer сохраняет HTTP-отказы до upgrade.

## 7. Heartbeat

The server sends a Ping with an opaque 8-byte payload every 30 seconds. A Pong counts only when its payload matches the latest Ping; the 10-second deadline begins after that Ping has been written. If no matching Pong arrives while the reader is reading the socket, the server sends Close 4003 (`heartbeat_timeout`) and drops the socket. Pongs do not reset the authenticated idle timer.



Не заменяет SESSION_IDLE_TTL (30 минут) — это для detection мёртвого TCP без RST.

## 8. Frame Size Limits

Для SCRAM WS пределы как TCP: pre-auth `≤ 4 KB`, data `≤ 16 MB`, превышение → `1009`. Для bearer browser endpoint TOKEN_API §10 отдельно задаёт первый кадр до проверки: максимум 256 байт, ожидание 5 секунд; некорректный/чрезмерный кадр или истечение времени → `4000`, не `1009`. После аутентификации предел bearer data frame — `≤ 16 MB`.

## 9. Origin Header

9.1. Browser ВСЕГДА шлёт `Origin` (per WS spec). Native клиенты обычно НЕ шлют.

9.2. Server policy per endpoint:

**`/shamir/v1/browser` (browser endpoint):**
- `Origin` **REQUIRED**. Отсутствие → 400.
- Mismatch с `allowed_origins` → 403.
- Защита от downgrade: native клиент без Origin **не может** случайно/злонамеренно попасть в browser-mode endpoint. (Browser всегда шлёт Origin per spec, поэтому требование не блокирует legitimate browsers.)

**`/shamir/v1` (native endpoint):**
- `Origin` optional.
- Если присутствует — match с `allowed_origins` (admin UI deployment может share origin).
- Если отсутствует — допустим (native client).

Никакого `--no-origin-check` flag.

Эти правила относятся к SCRAM. Браузерный bearer endpoint `/shamir/v1/ws/browser` дополнительно проверяет Origin и authority Host до upgrade по TOKEN_API §10; политика SCRAM не меняется. Bearer endpoint не использует cookie-аутентификацию или выбор режима по User-Agent.

## 10. Test Checklist

- WS upgrade + auth round-trip native (`/shamir/v1`)
- WS upgrade + auth round-trip browser (`/shamir/v1/browser`)
- Text frame на binary endpoint → close 1003
- Origin missing на browser endpoint → 400 (browsers всегда шлют Origin)
- Origin mismatch → 403
- Heartbeat dead detection
- Resumption: tcp↔ws same-tier работает; ws_browser → ws_native НЕ может resume (downgrade — не блокировано, это upgrade — OK); tls_exporter → browser endpoint НЕ может resume (это downgrade — block)
- Audit event содержит `transport: "ws"`
