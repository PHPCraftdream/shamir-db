# Transport: TCP

## 1. Профили

| Профиль | URI | binding_mode | Использование |
|---|---|---|---|
| `tcp+tls` | `shamir+tcp://...` | `0x01` (tls_exporter) | Default, prod |
| `tcp+plain` | `shamir+tcp://...?plain=1` | `0x00` (none) | **Только** in-process embedded mode и dev-loopback. Production deployments — TLS only. |

`tcp+plain` требует (см. IMPLEMENTATION_GUIDE §2.2):
- Server config `profile = "plain"` AND `allow_plain = true`
- Server bind addr в strict whitelist:
  - IPv4: `127.0.0.0/8`
  - IPv6: `::1` (single, не subnet)
  - Unix domain socket
- `0.0.0.0` или `::` (any-bind) → **NEVER allowed**, server fails старт
- Клиент явно `?plain=1` в URI

**Embedded mode рекомендация:** Unix domain socket (path-based file permissions = auth boundary) предпочтительнее plain TCP. Future: dedicated `TRANSPORT_UNIX.md`.

## 2. Framing

Length-prefixed msgpack:
```
[length: u32 BE][msgpack: length bytes]
```

- Empty frame (length=0) — graceful close
- До auth_ok: `length ≤ MAX_PRE_AUTH_FRAME = 4 KB`
- После auth_ok: `length ≤ MAX_FRAME_SIZE_DATA = 16 MB`
- Frame too large → TCP close без reply

## 3. TLS (tcp+tls)

3.1. **TLS 1.3 only.** TLS 1.2 и earlier reject.

3.2. **Cipher suites:** rustls defaults.

3.3. **Certificate verification на клиенте:** НЕ через CA. Identity = Ed25519 pin.

3.4. **TLS exporter:** `EXPORTER-ShamirDB-AUTH-v1`, контекст пустой, длина 32 байта (RFC 9266).

3.5. **TLS 1.3 0-RTT:** запрещён.

## 4. Channel Binding в auth_message

| Профиль | binding_mode | tls_exporter_or_zeros |
|---|---|---|
| tcp+tls | `0x01` | TLS-Exporter (32 bytes) |
| tcp+plain | `0x00` | bytes(32) zeros |

Несовпадение клиент/сервер policy → handshake fail (auth_message не совпадает).

## 5. Connection Lifecycle

```
TCP connect
  ▼
[tcp+tls only] TLS 1.3 handshake (rustls)
  ▼
auth_init (frame 1)
  ▼
challenge (frame 2)
  ▼
client_proof (frame 3)
  ▼
auth_ok ИЛИ error (frame 4)
  ▼
[active session — {sid, req} ↔ {rid, res}]
  ▼
TCP close ИЛИ logout ИЛИ idle timeout
```

Один TCP = одна active session. Повторный auth_init → close.

## 6. Session Frame Format

После auth_ok все запросы:
```
{
  "sid": bytes(32),               // session_id из auth_ok
  "req": { ... }                  // request body
}
```

Response:
```
{
  "rid": Optional<u32>,           // request id для correlation
  "res": { ... } | "error": "..."
}
```

## 7. Connection String Examples

```
shamir+tcp://alice@db.example.com:7331?pin=base64url(SHA256(server_pub))
shamir+tcp://alice@127.0.0.1:7334?plain=1
shamir+tcp://alice@10.0.0.5:7331?pin=...&accept_new_host=1   # TOFU first time
```

## 8. Test Checklist

- Round-trip auth с TLS, с plain (loopback), с TOFU pin, с out-of-band pin
- Frame too large → TCP close
- Empty frame → graceful close
- Повторный auth_init → close
- TLS 1.2 client → reject
- TLS 0-RTT → reject
- Plain профиль на non-loopback → server fails to start
- Audit event `auth_success` содержит `transport: "tcp"`
