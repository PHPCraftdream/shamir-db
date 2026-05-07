# ShamirDB Authentication Protocol v1

**Transport-agnostic** аутентификация. Только последовательность сообщений и crypto. Конверты (TCP/WS) — в TRANSPORT_*.md. Operational детали (метрики, логи, recovery) — в IMPLEMENTATION_GUIDE.md.

База: **SCRAM** (RFC 5802 idea), **Argon2id** для KDF, **Ed25519** для server identity, **HMAC-SHA256** для proof и transcript, **HKDF-SHA256** для key derivation.

---

## 1. Принципы

1.1. **Один auth flow**, один transport-agnostic протокол. Различия — только в конверте.

1.2. **Plain password никогда не покидает клиент.** Регистрация/смена пароля — derived ключи.

1.3. **Argon2id ровно в одном месте:** `password → salted_password` на клиенте.

1.4. **Server identity** через Ed25519, независимо от транспортного TLS. Pinning по `SHA256(server_pub_key)`.

1.5. **Channel binding** включает: TLS exporter (если есть), transport_kind, binding_mode. Защита от UKS/triple-handshake/cross-transport downgrade.

1.6. **Constant-time discipline** для anti-enumeration.

1.7. **Browser-friendly** через WASM (см. CLIENT_BROWSER.md). Те же примитивы, ослабленный channel binding (явно объявлен).

---

## 2. Сообщения

Сериализация: **msgpack** (любая valid RFC compliant — `auth_message` имеет независимую canonical форму, см. §4). Поля типа `bytes(N)` → msgpack `bin`.

### 2.1. `auth_init` (Client → Server)

```
{
  "auth_init": {
    "user": String,                      // UTF-8 NFC + PRECIS UsernameCaseMapped
    "client_nonce": bytes(32),           // CSPRNG, не all-zeros
    "binding_mode": u8,                  // см. §4.2
    "version": 1
  }
}
```

### 2.2. `challenge` (Server → Client)

```
{
  "challenge": {
    "salt": bytes(16),
    "kdf": "argon2id",
    "memory_kb": u32,
    "time": u32,
    "parallelism": u32,
    "argon2_version": u8,                // 0x13 (RFC 9106 v1.3)
    "server_nonce": bytes(32)
  }
}
```

### 2.3. `client_proof` (Client → Server)

```
{ "client_proof": bytes(32) }
```

### 2.4. `auth_ok` (Server → Client)

```
{
  "auth_ok": {
    "server_signature": bytes(32),       // SCRAM HMAC proof
    "server_pub_key": bytes(32),         // Ed25519 public
    "identity_sig": bytes(64),           // см. §6
    "session_id": bytes(32),             // CSPRNG
    "expires_at": u64,                   // unix seconds
    "resumption_ticket": Optional<bytes>,// см. SESSION_RESUMPTION.md
    "kdf_upgrade_required": Optional<bool>  // см. §13
  }
}
```

### 2.5. `error` (Server → Client)

```
{ "error": "authentication_failed" }                      // generic для auth/replay/lockout
{ "error": "rate_limited", "retry_after": u32 }
{ "error": "server_busy", "retry_after": u32 }
{ "error": "unsupported_version" }
```

---

## 3. Регистрация

3.1. Доступна только через `bootstrap` (§11) или `createUser` admin command (§12.1).

3.2. **Password policy** проверяется клиентом (server не может verify через SCRAM by design):
- `PASSWORD_MIN_LENGTH = 12 символов` UTF-8
- `PASSWORD_MAX_LENGTH = 1024 символа`
- Запрет: empty, only whitespace, single repeated char

3.3. Клиент локально:
```
salt           = random(16)
salted_password = Argon2id(password, salt, server_kdf_params)
client_key     = HMAC-SHA256(salted_password, "Client Key")
stored_key     = SHA256(client_key)
server_key     = HMAC-SHA256(salted_password, "Server Key")
zeroize: password, salted_password, client_key
```

3.4. На сервер уходит `{salt, stored_key, server_key, kdf_params}`. **Plain password и `password_length` НЕ передаются.**

3.5. Сервер сохраняет в `__system__/users/{user_id}`:
```
{
  name: String,                         // PRECIS UsernameCaseMapped + NFC
  salt: bytes(16),
  stored_key: bytes(32),
  server_key: bytes(32),
  memory_kb: u32,
  time: u32,
  parallelism: u32,
  argon2_version: u8,                   // 0x13
  roles: Vec<String>,
  tickets_invalid_before: u64,          // см. SESSION_RESUMPTION.md
  created_at: u64,
  updated_at: u64
}
```

3.6. Сервер **никогда** не хранит: `password`, `salted_password`, `client_key`.

3.7. Argon2id default параметры:
```
memory_kb = 131072    (128 MB)
time = 4
parallelism = 1       (см. §3.7.1)
argon2_version = 0x13
```

3.7.1. **Обоснование `parallelism=1`:** OWASP 2024 рекомендует p=1 для serverside memory-hard KDF. Memory cost (`m=128 MB`) превышает OWASP minimum (19 MiB) в 6.7×. p=1 даёт predictable per-handshake CPU cost вместо параллельных lane'ов; атакующий с GPU всё равно parallelize'ит между попытками, не внутри одной.

3.7.2. **Hard floor (server config):**
- `KDF_MIN_MEMORY_KB = 19456` (19 MiB OWASP min)
- `KDF_MIN_TIME = 2`
- `KDF_MIN_PARALLELISM = 1`

Server при старте reject config ниже floor. Защита от misconfigured admin.

---

## 4. Канонический auth_message

Используется для подписей и proof. **Все** имплементации обязаны байт-в-байт идентичный результат.

### 4.1. Формат

```
auth_message =
    "SHAMIR-AUTH-v1"                        (14 bytes ASCII fixed header)
 || u16_be(byte_len(username_nfc)) || username_nfc
 || client_nonce(32)
 || server_nonce(32)
 || salt(16)
 || u32_be(memory_kb)                       // raw kdf params
 || u32_be(time)
 || u32_be(parallelism)
 || u8(argon2_version)                      // 0x13
 || u8(transport_kind)                      // см. §4.2
 || u8(binding_mode)                        // см. §4.2
 || tls_exporter_or_zeros(32)               // см. §4.2
 || u8(supported_version)                   // = 1
```

**`byte_len(username_nfc)`:** длина в **байтах** (не Unicode code points) после NFC + UsernameCaseMapped, max 255. JS reference: `new TextEncoder().encode(username.normalize('NFC').toLowerCase()).byteLength`.

KDF параметры включены **raw bytes** (не hash). Любое изменение → другой auth_message → SCRAM proof не совпадёт → защита от downgrade.

### 4.2. transport_kind, binding_mode, tls_exporter

```
transport_kind:
  0x01 = tcp
  0x02 = ws
  (другие — резерв для future TRANSPORT_*.md)

binding_mode:
  0x00 = none           (plain transport, нет TLS)
  0x01 = tls_exporter   (есть TLS, exporter извлечён клиентом и сервером)
  0x02 = tls_no_export  (есть TLS, но клиент не имеет API для exporter — browser path)
```

Значение `tls_exporter_or_zeros`:
```
binding_mode == 0x01 → TLS-Exporter(label="EXPORTER-ShamirDB-AUTH-v1", context="", L=32)
binding_mode == 0x00 → bytes(32) all zeros
binding_mode == 0x02 → bytes(32) all zeros (browser; ослабленный режим)
```

**Enum extension rule:** unknown enum values → fail-closed (reject auth_init / reject session / reject ticket). Adding new enum value = AUTH minor bump; downstream documents должны bump compatibility matrix entry (см. IMPLEMENTATION_GUIDE §9).

### 4.3. Server policy для `binding_mode` (NORMATIVE)

Сервер для каждого listener конфигурирует **разрешённые** `binding_mode`:
- TCP+TLS listener: `binding_mode == 0x01` (требует exporter)
- TCP plain listener: `binding_mode == 0x00`
- WSS native listener: `binding_mode == 0x01`
- WSS browser listener: `binding_mode == 0x02` (отдельный endpoint, см. TRANSPORT_WS.md)

**MUST:** Server **обязан** rejектить `auth_init` где `binding_mode` не входит в listener policy **до** запуска Argon2id (защита от DoS amplification — иначе атакующий flood'ит mismatched binding_mode и forces 128 MB × time=4 на каждый запрос).

Reject = silent close без error message (анти-fingerprinting listener policy).

### 4.4. Anti-downgrade свойство

Любое расхождение между сторонами → различные `auth_message` → различные `client_proof` → SCRAM verify fail → reject.

Per-listener policy (§4.3) исключает MITM подмену `binding_mode` в `auth_init`.

---

## 5. SCRAM Verification

### 5.1. Client computes

5.1.1. **Pre-Argon2id validation** challenge.kdf_params:
```
memory_kb ≤ 262144  (256 MB)
time ≤ 8
parallelism ≤ 8
argon2_version == 0x13
```
Превышение → disconnect, локально `kdf_params_rejected`.

5.1.2. **Self-validation**: клиент использует **те же params что прислал сервер** в auth_message. SCRAM proof на mismatched params не сойдётся.

5.1.3. Derive:
```
salted_password  = Argon2id(password, salt, kdf_params)
client_key       = HMAC-SHA256(salted_password, "Client Key")
server_key       = HMAC-SHA256(salted_password, "Server Key")
zeroize: password, salted_password

stored_key       = SHA256(client_key)
client_signature = HMAC-SHA256(stored_key, auth_message)         // RFC 5802 §3
client_proof     = client_key XOR client_signature
zeroize: client_key
```

### 5.2. Server verifies

5.2.1. **Anti-enumeration через HKDF** — для несуществующего user сервер генерирует детерминистический fake блок:
```
fake_blob = HKDF-SHA256(
    ikm  = server_secret,
    salt = "SHAMIR-FAKE-SALT-v1",                // 18 bytes ASCII (domain separation)
    info = username_nfc,
    L    = 80
)
fake_salt        = fake_blob[0..16]
fake_stored_key  = fake_blob[16..48]
fake_server_key  = fake_blob[48..80]
```
`server_secret = random(32)` хранится в SystemStore (`__system__/server_meta`), **ротируется каждые 30 дней** с overlap-окном (см. IMPLEMENTATION_GUIDE.md).

5.2.2. Server использует либо real (`stored_key, server_key, salt`) либо fake — **constant-time** branch. Те же библиотечные вызовы.

5.2.3. Verify:
```
client_signature     = HMAC-SHA256(stored_key, auth_message)     // или fake_stored_key
recovered_client_key = client_proof XOR client_signature
ok = ConstantTimeEq(SHA256(recovered_client_key), stored_key)
```

5.2.4. **Все три криптографические аутентификации сервера** вычисляются ВСЕГДА до accept/reject (constant-time discipline):

```
server_signature = HMAC-SHA256(server_key, auth_message)
session_id       = random(32)               // discarded if auth fails — no timing oracle
expires_at       = now + SESSION_MAX_AGE

identity_input = "SHAMIR-IDENTITY-v1"
              || SHA256(server_pub_key)     // включён в подпись (защита от key-substitution)
              || u8(transport_kind)
              || u8(binding_mode)
              || tls_exporter_or_zeros(32)
              || auth_message
              || session_id(32)
              || u64_be(expires_at)
identity_sig = Ed25519::sign(server_ed25519_priv, identity_input)
```

5.2.5. На fail → `{"error": "authentication_failed"}`. Backoff per `(client_ip_subnet, username_hash)` где subnet = `/24 IPv4` или `/64 IPv6`. Backoff: `100ms × 2^N`, cap 30s, reset 5 мин.

`username_hash = HMAC-SHA256(lockout_secret, username_nfc)[..16]`. См. IMPLEMENTATION_GUIDE §1.3 — `lockout_secret` отдельный от `server_secret`, **не ротируется** (защита lockout state от orphan на ротации anti-enumeration secret).

### 5.3. Client verifies

```
// 1. SCRAM mutual auth
expected = HMAC-SHA256(server_key, auth_message)
ConstantTimeEq(expected, server_signature)
zeroize: server_key

// 2. Pin check (TOFU или out-of-band)
pinned_hash = load_pin_or_known_hosts(host)
received_hash = SHA256(server_pub_key)
if pinned_hash is None:
    require explicit user consent OR --accept-new-host flag
    save_known_hosts(host, received_hash)
elif !ConstantTimeEq(pinned_hash, received_hash):
    disconnect("server_identity_changed")

// 3. Ed25519 signature (RFC 8032 strict — small-subgroup rejection)
Ed25519::verify_strict(server_pub_key, identity_input, identity_sig)
```

Любой fail → disconnect.

---

## 6. Server Identity (Ed25519)

6.1. Генерируется при первом запуске. `ed25519-dalek::SigningKey::generate()`.

6.2. **Хранение priv:** `__system__/server_meta` с `chmod 600` + `mlock` (best-effort) + `disable_core_dumps` (best-effort per-OS). Backup — encrypted-at-rest **отдельно** от users data.

6.3. **Pinning model.** Pin = `SHA256(server_pub_key)` 32 байта. Источник:
- (a) URI param: `shamir+tcp://alice@host?pin=base64url(SHA256(pub))` (recommended for prod)
- (b) `~/.shamir/known_hosts` запись от предыдущего подключения (TOFU)

Если ни (a) ни (b) И не задан `--accept-new-host` → клиент refuses подключение.

`known_hosts` сопровождается integrity tag — см. IMPLEMENTATION_GUIDE §7.

6.4. **Server identity rotation:** admin command `rotateServerIdentity` (§12.2). Поддерживает overlap-окно 7 дней (фиксировано) с подписью старым ключом перехода к новому.

---

## 7. Session

7.1. `session_id = random(32)` выдаётся при `auth_ok`.

7.2. Server in-memory state:
```
struct Session {
    user_id: bytes(16),
    username: String,
    permissions: SessionPermissions,        // см. §7.3
    created_at: Instant,
    last_activity: AtomicU64,
    transport_kind: u8,                     // tcp=0x01, ws=0x02
    binding_mode: u8,                       // см. §4.2
    channel_binding_at_auth: bytes(32),     // снят с auth — для resumption check
}
```

(Nonces `client_nonce`/`server_nonce` НЕ хранятся в Session после handshake. См. §12.5 — `changePassword` использует свой challenge cycle.)

7.3. **SessionPermissions** — snapshot ролей в момент auth:
```
struct SessionPermissions {
    is_superuser: bool,
    roles: Vec<String>,
    // имплементация может добавлять precomputed bitmasks
}
```
Изменение ролей админом (`updateUser`, §12.7) автоматически invalidates все existing sessions И tickets для затронутого юзера.

7.4. Лимиты:
| Параметр | Значение |
|---|---|
| `SESSION_MAX_AGE` | 24 часа |
| `SESSION_IDLE_TTL` | 30 минут |
| `MAX_SESSIONS_PER_USER` | 16 |
| `PER_SESSION_MEM` | 64 MB |

**Overflow behavior** для `MAX_SESSIONS_PER_USER`:
- При попытке 17-й сессии: oldest idle session evicted (LRU по `last_activity`).
- Если **все 16 active** (last_activity < 1 минуты назад) → reject новый login с `authentication_failed` (generic, anti-enumeration).
- Audit event `session_evicted{reason="max_sessions_lru"}` или `auth_failed{reason="max_sessions"}` (только во внутренний log, наружу generic).

7.5. После auth_ok все запросы несут `session_id` (transport-specific framing — см. TRANSPORT_*.md).

7.6. Session не персистентна. Restart сервера → re-auth (или resume через ticket).

7.7. Disconnect transport → session evict через 5 секунд (window для transport switch via resumption).

---

## 8. Лимиты и DoS защита

| Параметр | Значение | Назначение |
|---|---|---|
| `MAX_PRE_AUTH_FRAME` | 4 KB | До auth_ok (включая admin commands ~4KB) |
| `MAX_FRAME_SIZE_DATA` | 16 MB | Query/response, server tunable |
| `USERNAME_MAX_BYTES` | 255 (после NFC + UsernameCaseMapped) |
| `RATE_LIMIT_AUTH_INIT_PER_SUBNET` | 10/sec sliding | `/24 IPv4` или `/64 IPv6` (унифицировано с backoff) |
| `BACKOFF_BASE` | 100 ms × 2^N | per (subnet, username_hash), cap 30s |
| `BACKOFF_RESET` | 5 минут неактивности |
| `LOCKOUT_THRESHOLD` | 50 fails/час per (subnet, username_hash) |
| `LOCKOUT_AUTO_RESET` | 24 часа |
| `KDF_MAX_MEMORY` | 256 MB |
| `KDF_MAX_TIME` | 8 |
| `KDF_MAX_PARALLEL` | 8 |
| `HANDSHAKE_TIMEOUT` | max(15s, kdf_time × 5) |
| `MAX_CONCURRENT_ARGON2` | 64 |
| `MAX_CONNECTIONS_PER_IP` | 100 |

8.1. **Argon2id semaphore** (`MAX_CONCURRENT_ARGON2`) берётся **только** на время фактического Argon2id. Pre-state до proof занимает 1KB слот, не permit.

8.2. **Pre-handshake state GC** каждые 5 секунд: state без proof в течение 10с → drop.

8.3. **Один TCP/WS connection** = один активный handshake state. Повторный `auth_init` в том же connection → close.

8.4. **Lockout silent**: response identical с `authentication_failed`. Внутреннее состояние не раскрывается.

8.5. **Latency padding для negative paths**: rate_limited / lockout / server_busy ответы задерживаются до `target_constant_time = max(jitter_ms, kdf_time_seconds * 1000)`.

8.6. **Restart warmup window**: первые 60 секунд после старта сервер applies глобальный rate limit `RATE_LIMIT_AUTH_INIT_PER_SUBNET / 4 = 2.5/sec` пока in-memory state warmup'ится из persisted snapshots. Закрывает restart-replay window для distributed attackers.

---

## 9. Constant-time Discipline

"Constant-time" = **branch-equivalent**: одинаковые библиотечные вызовы и memory access patterns на real-vs-fake путях. **Не** wall-clock padding.

Применяется к:
- 5.2.1—5.2.4 SCRAM verify (real / fake path)
- §11 Bootstrap token check — `ConstantTimeEq(SHA256(token), stored_hash)`
- 5.3 Client mutual auth — все compare через `subtle::ConstantTimeEq` / `@noble/equalBytes`

**Ed25519 verify** — variable-time on public inputs (RFC 8032 §6). Это OK криптографически — нет remote oracle. Boolean result branching на клиенте — безопасен.

**Cache side-channel** (Argon2id internals, HMAC variability) — **не покрывается**. Защита: generic errors, generic latency.

---

## 10. Хранение

См. IMPLEMENTATION_GUIDE.md §1 (нормативные SystemStore схемы, file locations, file permissions, MAC integrity для known_hosts).

Краткая структура:
- `__system__/users/{user_id}` — см. §3.5
- `__system__/server_meta` — server_secret + lockout_secret + ed25519 keypair + ticket_key + audit_chain_key + bootstrap state
- In-memory: handshake states, auth_failures, lockout state, consumed_counters
- **Lockout state и consumed_counters** persisted in SystemStore с **батчингом** (NORMATIVE: flush ≤ 5 секунд OR при достижении threshold, синхронный flush на graceful shutdown)

---

## 11. Bootstrap

### 11.1. Trigger

При первом запуске сервера если выполнено **всё**:
- `__system__/users` пуст
- `__system__/server_meta.bootstrap_token_hash IS NULL`
- `__system__/server_meta.superuser_ever_existed == false`

Защита от silent re-bootstrap при corrupted backup: после первого успешного bootstrap флаг `superuser_ever_existed = true` навсегда (даже после deletion всех юзеров).

### 11.2. Token issuance

11.2.1. `bootstrap_token = random(32)`, prefix `shbst1_` для git-secret-scanners.
11.2.2. Сервер сохраняет атомарно: `bootstrap_token_hash = SHA256(token)`, `bootstrap_token_expires_at = now + bootstrap_token_ttl`.

`bootstrap_token_ttl` configurable (server config):
- Default: **1 час**
- Min: **5 минут**
- Max: **24 часа**

Air-gapped deployments (где token нужно физически доставить к KMS) могут использовать max.

11.2.3. **Token output** (server config):
- `--bootstrap-token-tty` (default): печать в stdout **только** если `isatty(stdout)`. Иначе server fails с инструкцией.
- `--bootstrap-token-file <path>`: атомарно создать `chmod 600` файл, записать токен. Server **MUST** удалять файл (a) при `bootstrap_used` event AND (b) фоновым GC при `now > expires_at`. На startup server проверяет file existence vs `bootstrap_token_hash IS NULL` и удаляет orphan файл (audit event `bootstrap_token_file_orphan_cleaned`).
- **Запрещено** логирование через `tracing!` / `log!`.

11.2.4. Рядом печатается `SERVER_PUB_FINGERPRINT: base64url(SHA256(server_ed25519_pub))` для out-of-band pinning.

### 11.3. Bootstrap connection

Bootstrap **обязан** работать только на **native client** с TLS exporter (`binding_mode == 0x01`). Browser bootstrap **запрещён в v1**.

11.3.1. URI: `shamir+tcp://host:port?bootstrap=1&pin=base64url(...)`. Без `pin` — refuse слать token.

11.3.2. Клиент → server: `{ "bootstrap_hello": { "client_nonce": bytes(32) } }` (≤ 256 байт). Client_nonce включается в подписываемый payload — защита от replay challenge другому клиенту.

11.3.3. Server → client:
```
{ "bootstrap_challenge": {
    "server_pub_key": bytes(32),
    "identity_sig_bootstrap": bytes(64)
}}
```
где
```
identity_sig_bootstrap = Ed25519::sign(priv,
    "SHAMIR-BOOTSTRAP-v1"
    || SHA256(server_pub_key)              // защита от key-substitution
    || u8(transport_kind)
    || tls_exporter(32)
    || client_nonce(32)                    // anti-replay binding к этому клиенту
    || u64_be(server_time)
)
```

11.3.4. Клиент валидирует pin = `SHA256(server_pub_key)` (constant-time) и Ed25519 подпись. Mismatch → disconnect, **plain password не уходит**.

11.3.5. Клиент локально вычисляет derived материал (как §3.3) с **серверными default params**, шлёт:
```
{
  "bootstrap": {
    "token": bytes(32),
    "user": String,
    "salt": bytes(16),
    "stored_key": bytes(32),
    "server_key": bytes(32),
    "memory_kb": u32,
    "time": u32,
    "parallelism": u32,
    "argon2_version": u8
  }
}
```

11.3.6. Server атомарно (mutex + CAS):
- Validate `expires_at > now` AND `ConstantTimeEq(SHA256(token), bootstrap_token_hash)`
- Validate `kdf_params == server_defaults` (защита от malicious client'а)
- Validate username:
  - PRECIS UsernameCaseMapped + NFC
  - **Если username уже существует** → `bootstrap_failed` (no overwrite). Operator должен сначала удалить collision через `--list-users` / `--delete-user` CLI.
- Создаёт user с ролью `superuser`
- Set `bootstrap_token_hash = NULL`, `bootstrap_token_expires_at = NULL`, `superuser_ever_existed = true`
- Invariant: после успеха `bootstrap_token_hash IS NULL AND superuser EXISTS`

11.3.7. На fail → `{"error": "bootstrap_failed"}` (generic).

### 11.4. Recovery

Lost admin → `shamir-server --regen-bootstrap --confirm` (требует stop сервера + физический/SSH доступ + флаг `--confirm`):
- Server reads stdin для confirmation phrase
- Generate новый `bootstrap_token`, `superuser_ever_existed` остаётся `true`
- Output token per `--bootstrap-token-*` config
- Audit event `bootstrap_regen` записан
- Старые admin'ы НЕ удаляются автоматически — operator делает manual cleanup после re-bootstrap

---

## 12. Admin Commands

Все выполняются внутри активной auth-сессии. Authorization — `is_superuser` в `SessionPermissions`. Audit event на каждое выполнение.

### 12.1. `createUser`

```
Request:  { "createUser": {
              "name": String,
              "salt": bytes(16),
              "stored_key": bytes(32),
              "server_key": bytes(32),
              "memory_kb": u32,
              "time": u32,
              "parallelism": u32,
              "argon2_version": u8,
              "roles": Vec<String>
           }}
Response: { "ok": { "user_id": bytes(16) } }
```
Server validates:
- `kdf_params == server_defaults` (anti-enumeration invariant)
- `kdf_params >= floor` (§3.7.2)
- Username unique (PRECIS + NFC normalized для lookup)

### 12.2. `rotateServerIdentity`

```
Request:  { "rotateServerIdentity": {} }                  // нет параметров; window фиксировано 7 дней
Response: { "ok": { "new_pub": bytes(32), "transition_until": u64 } }
```

Процедура:
1. Server генерит новый keypair, store as `previous = current; current = new`.
2. **Per-recipient signing:** для каждой активной сессии server вычисляет **отдельную** `Ed25519::sign(...)`. Кэширование подписей запрещено — payload уникален per-recipient.
3. Broadcast event:
```
{ "identity_rotation": {
    "old_pub": bytes(32),
    "new_pub": bytes(32),
    "transition_until": u64,
    "recipient_session_id": bytes(32),         // полный sid (не prefix), уникален per recipient
    "signed_by_old": bytes(64)
}}
where signed_by_old = Ed25519::sign(old_priv,
    "SHAMIR-ROTATE-v1"
    || SHA256(old_pub)                         // domain binding to current key (anti-stale-pin attack)
    || new_pub
    || u64_be(transition_until)
    || recipient_session_id(32)                // anti-replay between recipients
)
```
4. Клиент: проверяет `signed_by_old` против currently pinned `old_pub` (`ConstantTimeEq(SHA256(old_pub), pinned_hash)`), проверяет `recipient_session_id == my_session_id` (constant-time), валидирует Ed25519 подпись. Обновляет pin (с user confirmation если interactive).
5. Через `transition_until` server zeroize old priv.

**Emergency rotation (без grace окна):** server config `--identity-revoked` flag (см. IMPLEMENTATION_GUIDE §5.2). Active sessions terminate, new sessions reject пока operator не выполнит rotation properly.

**Resumption во время rotation окна:** см. SESSION_RESUMPTION §5.

### 12.3. `unlockUser`

```
Request:  { "unlockUser": { "user": String } }
Response: { "ok": {} }
```
Сбрасывает **И** `lockout_state` **И** `auth_failures` для всех subnet ключей данного user. Иначе пользователь застревает в высоком backoff после unlock.

### 12.4. `kickSession`

```
Request:  { "kickSession": {
              "user": Option<String>,
              "session_id_prefix": Option<bytes>     // hex prefix ≥4 bytes
           }}
Response: { "ok": { "killed_count": u32 } }
```

**Атомарно (single transaction):** kill matching sessions + update `user.tickets_invalid_before = now` для затронутых юзеров (защита от resumption через украденный ticket с устаревшими ролями).

### 12.5. `changePassword` (self-service)

Любой пользователь меняет свой пароль внутри своей сессии. **Двухшаговый flow с fresh challenge** (не использует старый auth_message — Session struct не хранит nonces).

```
Step 1 — Request challenge:
Client → Server: { "changePasswordChallenge": {
    "client_nonce_cp": bytes(32)                // CSPRNG, anti-malicious-server-replay
}}

Step 2 — Server fresh challenge:
Server → Client: { "challenge_cp": {
    "server_nonce_cp": bytes(32),               // CSPRNG, per-request
    "salt": bytes(16),                          // current user salt
    "memory_kb": u32, "time": u32,              // current user kdf params
    "parallelism": u32, "argon2_version": u8
}}

Step 3 — Both compute auth_message_cp:
auth_message_cp =
    "SHAMIR-CHGPW-v1"
 || u16_be(byte_len(username_nfc)) || username_nfc
 || session_id(32)                              // binding к текущей сессии
 || client_nonce_cp(32)                         // anti malicious-server replay
 || server_nonce_cp(32)                         // anti malicious-client replay
 || salt(16)
 || u32_be(memory_kb) || u32_be(time) || u32_be(parallelism) || u8(argon2_version)
 || u8(transport_kind)                          // symmetry с main auth_message §4.1
 || u8(binding_mode)
 || channel_binding_at_auth(32)                 // session.channel_binding_at_auth

# Client derives заново (Argon2id ~2с):
salted_old      = Argon2id(old_password, salt, kdf_params)
client_key_old  = HMAC(salted_old, "Client Key")
stored_key_old  = SHA256(client_key_old)
client_sig_cp   = HMAC(stored_key_old, auth_message_cp)
client_proof_old = client_key_old XOR client_sig_cp

# New material:
new_salt = random(16)
new_salted = Argon2id(new_password, new_salt, server_defaults)
new_client_key = HMAC(new_salted, "Client Key")
new_stored_key = SHA256(new_client_key)
new_server_key = HMAC(new_salted, "Server Key")
zeroize: old_password, salted_old, client_key_old, new_password, new_salted, new_client_key

Step 4 — Client → Server:
{ "changePassword": {
    "client_proof_old": bytes(32),
    "new_salt": bytes(16),
    "new_stored_key": bytes(32),
    "new_server_key": bytes(32)
}}
# kdf_params от клиента игнорируются — server применяет current defaults

Step 5 — Server verifies:
client_signature = HMAC(user.stored_key, auth_message_cp)
recovered = client_proof_old XOR client_signature
ok = ConstantTimeEq(SHA256(recovered), user.stored_key)

Step 6 — Server → Client: { "ok": {} } или { "error": "authentication_failed" }
```

12.5.1. Server проверяет SCRAM proof старого пароля **без plain password** и **без серверного Argon2id** (нет DoS amplification).

12.5.2. Server **игнорирует** client-supplied kdf_params — всегда server defaults.

12.5.3. **Все сессии юзера убиваются** (включая текущую) И `tickets_invalid_before = now`. Клиент должен переаутентифицироваться.

12.5.4. Serialized per user (mutex). Atomic update.

### 12.6. `updateUser` (admin)

```
Request:  { "updateUser": {
              "user": String,
              "roles": Option<Vec<String>>
           }}
Response: { "ok": {} }
```

**Атомарно (single transaction). Строгий порядок шагов** (защита от race с in-flight resumption):

```
1. Update user record (roles если задан) → persist
2. Set user.tickets_invalid_before = now → persist
3. (Persist barrier — дальнейшие resume будут видеть новое tickets_invalid_before)
4. Snapshot active sessions matching user_id
5. Kill snapshotted sessions (close connections)
6. Audit event roles_changed
```

Между шагом 2 и снапшотом любое in-flight resume увидит обновлённый `tickets_invalid_before` и отвергнется. Без шага 3 (persist barrier) гонка возможна.

(Принудительная смена пароля — будет в v1.1; для v1: admin удаляет user через CLI и создаёт заново через `createUser`, или просит юзера сменить через §12.5 self-service.)

### 12.7. Информационные команды

`whoami`, `listSessions`, `serverInfo` — schemas и поведение в IMPLEMENTATION_GUIDE.md §13. Не security-критичны, не требуют superuser (кроме `listSessions` всех юзеров).

---

## 13. Argon2id Parameter Migration

13.1. KDF params хранятся per-user в user record (§3.5).

13.2. Глобальный server config — `kdf_params_current` — для новых регистраций и migration target. Должен соответствовать floor (§3.7.2).

13.3. При login сервер использует stored kdf_params для verify.

13.4. Если `user.kdf_params != kdf_params_current` AND все params strictly_weaker (memory_kb/time/parallelism все ≤ current):
- В `auth_ok` сервер шлёт `"kdf_upgrade_required": true`.
- Клиент использует **тот же** two-step flow что в §12.5 (`changePasswordChallenge` → `changePassword`) с тем же паролем для re-derive под новыми params.
- Transparent UX. Audit event `kdf_params_upgraded`.

13.5. Anti-enumeration: для unknown user сервер возвращает **current** kdf_params. Старые юзеры с устаревшими params видны атакующему — known trade-off.

---

## 14. Errors

14.1. Server-side auth: `{"error": "authentication_failed"}` (generic — unknown user / bad password / bad proof / bad binding / lockout).

14.2. Client-side локальные:
- `server_authentication_failed` — server proof не сошёлся
- `server_signature_invalid` — Ed25519 verify failed
- `server_identity_changed` — TOFU mismatch
- `kdf_params_rejected` — params превышают локальные limits
- `known_hosts_integrity_failed` — MAC файла не сошёлся

14.3. Server-side прочие:
- `rate_limited` (с retry_after)
- `server_busy` (с retry_after)
- `unsupported_version`
- `bootstrap_failed`

14.4. **Никогда не раскрывается:** существование юзера, причина auth_failed, lockout state.

---

## 15. Encoding

15.1. **Wire format:** msgpack (любая RFC compliant имплементация). Поля типа `bytes(N)` → msgpack `bin`. Map ordering, integer encoding — **не ограничены**.

15.2. **`auth_message` (§4) имеет независимую canonical форму** — explicit byte concatenation. Это единственная структура, требующая bit-exact реализации между сторонами.

15.3. **Username нормализация** (RFC 8265 **UsernameCaseMapped** profile):
- NFC normalization
- Case-folded (lowercase) — защита от homograph атак
- Запрещены: control chars (Cc), bidi-format chars (Cf вне allow-list), private-use plane

Lookup и хранение — **после** нормализации. Длина измеряется в **байтах** UTF-8.

15.4. **Password не нормализуется** (NIST SP 800-63B §5.1.1.2). UTF-8 bytes как есть.

---

## 16. Test Vectors

**Release blocker для v1.** Файл `spec/test-vectors/auth_v1.json` обязан содержать полный набор. Inline minimal example для bootstrapping имплементаций:

### Example: auth_message hex dump

```
Inputs:
  username = "alice"          (5 bytes UTF-8 NFC casemapped)
  client_nonce = 00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff
  server_nonce = 2030...4f    (32 bytes 0x20..0x3f)
  salt         = 5051..5f     (16 bytes 0x50..0x5f)
  memory_kb=131072 (0x00020000), time=4, parallelism=1, argon2_version=0x13
  transport_kind=0x01 (tcp), binding_mode=0x01 (tls_exporter)
  tls_exporter = aabbccdd...  (32 bytes 0xaa..0xc9)
  supported_version = 1

auth_message bytes:
  5348414d 49522d41 5554482d 7631      # "SHAMIR-AUTH-v1" (14)
  0005                                  # u16_be(5) length
  616c6963 65                          # "alice"
  00112233 44556677 8899aabb ccddeeff
  00112233 44556677 8899aabb ccddeeff  # client_nonce(32)
  20212223 24252627 28292a2b 2c2d2e2f
  30313233 34353637 38393a3b 3c3d3e3f  # server_nonce(32)
  50515253 54555657 58595a5b 5c5d5e5f  # salt(16)
  00020000                              # memory_kb=131072 (BE)
  00000004                              # time=4 (BE)
  00000001                              # parallelism=1 (BE)
  13                                    # argon2_version
  01                                    # transport_kind=tcp
  01                                    # binding_mode=tls_exporter
  aabbccdd ... (32 bytes)               # tls_exporter
  01                                    # supported_version
```

Полный test-vectors JSON содержит:
- `kdf_canonical_string` (legacy compat reference)
- `Argon2id(password="hello world!1", salt=fixed, params=defaults)` → 32-byte output
- `client_proof`, `server_signature`, `identity_sig` для полного flow
- `fake_blob` через HKDF для fixed username → 80 байт hex
- Resumption ticket: encrypt/decrypt round-trip с fixed key/nonce
- Identity rotation `signed_by_old` для fixed inputs

Каждая имплементация (Rust native, browser SDK) обязана pass всех vectors.

---

## 17. Domain Separation Tags

| Tag | Использование |
|---|---|
| `"Client Key"` | HMAC(salted_password) → client_key |
| `"Server Key"` | HMAC(salted_password) → server_key |
| `"SHAMIR-FAKE-SALT-v1"` | HKDF salt для fake values |
| `"SHAMIR-AUTH-v1"` | Header auth_message |
| `"SHAMIR-CHGPW-v1"` | Header auth_message_cp в changePassword |
| `"SHAMIR-IDENTITY-v1"` | Префикс identity_sig |
| `"SHAMIR-BOOTSTRAP-v1"` | Bootstrap challenge sig |
| `"SHAMIR-ROTATE-v1"` | Identity rotation sig |
| `"EXPORTER-ShamirDB-AUTH-v1"` | TLS exporter label |
| `"SHAMIR-TICKET-v1"` | См. SESSION_RESUMPTION.md |

---

## 18. Versioning

18.1. `auth_init.version: u8` — major version **AUTH_PROTOCOL.md**. Единственная версия в handshake.

18.2. Каждый документ имеет свою версию в header. Backward-compat = minor bump. Wire-breaking = major bump.

18.3. Domain tags привязаны к **document version**. `SHAMIR-TICKET-v2` может появиться без `SHAMIR-AUTH-v2`.

18.4. Compatibility matrix — IMPLEMENTATION_GUIDE.md §9.

---

## 19. См. также

- **SECURITY_MODEL.md** — adversary model, threat coverage, non-guarantees, recovery overview
- **IMPLEMENTATION_GUIDE.md** — operational details (storage, observability, audit log, log redaction, dependencies, recovery runbooks, admin command schemas)
- **SESSION_RESUMPTION.md** — fast reconnect через ticket
- **TRANSPORT_TCP.md, TRANSPORT_WS.md** — конкретные transport bindings
- **ADMIN_UI_HOSTING.md** — static admin UI delivery + REST для активных сессий
- **CLIENT_BROWSER.md** — browser SDK guidelines
- **../ROADMAP.md** — v1.1+ planned features
