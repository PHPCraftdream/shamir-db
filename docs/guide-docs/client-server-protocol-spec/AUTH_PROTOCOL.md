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
    "server_pub_key": bytes(32),         // Ed25519 public — current
    "identity_sig": bytes(64),           // см. §6 — signed by current Ed25519 priv
    "session_id": bytes(32),             // CSPRNG
    "expires_at_ns": u64,                // unix nanos (унифицировано с tickets_invalid_before_ns)
    "resumption_ticket": Optional<bytes>,    // см. SESSION_RESUMPTION.md
    "resumption_expires_at_ns": Optional<u64>,
    "kdf_upgrade_required": Optional<bool>,  // см. §13
    "rotation_in_progress": Optional<{       // Только когда server identity в overlap window
       "previous_pub": bytes(32),            // старый Ed25519 pub
       "identity_sig_previous": bytes(64),   // identity_input подписан previous_priv
       "transition_until_ns": u64,           // когда previous_priv будет zeroized
       "rotation_proof": bytes(64)           // sign(previous_priv, ROTATION_PROOF_PAYLOAD), см. §6.5
    }>
  }
}
```

`rotation_in_progress` — **присутствует** в auth_ok только когда `server_ed25519_priv_previous` ещё активен (в течение 7-day overlap после `rotateServerIdentity`). См. §6.5 для процедуры обработки клиентом.

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
  name: String,                         // PRECIS UsernameCaseMapped + NFC, ≤ 255 байт
  salt: bytes(16),
  stored_key: bytes(32),
  server_key: bytes(32),
  memory_kb: u32,
  time: u32,
  parallelism: u32,
  argon2_version: u8,                   // 0x13
  roles: Vec<String>,
  tickets_invalid_before_ns: u64,       // unix nanos. INITIAL VALUE = 0 при createUser/bootstrap.
                                         // Любой новый ticket будет иметь original_auth_at_ns >> 0 → check passes.
                                         // См. SESSION_RESUMPTION.md
  created_at_ns: u64,
  updated_at_ns: u64
}
```

**Username length validation:** длина в **байтах** (после NFC + UsernameCaseMapped) ≤ 255 проверяется **до** сериализации. Reject с `bootstrap_failed` / `createUser_failed` иначе. Soft recommendation: ≤ 64 байт для UX и audit log compactness.

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
    salt = "SHAMIR-FAKE-SALT-v1",                // 19 bytes ASCII (domain separation)
    info = username_nfc,
    L    = 80
)
fake_salt        = fake_blob[0..16]
fake_stored_key  = fake_blob[16..48]
fake_server_key  = fake_blob[48..80]
```
`server_secret = random(32)` хранится в SystemStore (`__system__/server_meta`), **ротируется каждые 30 дней** с overlap-окном (см. IMPLEMENTATION_GUIDE.md).

**Rotation behavior для fake path** [NORMATIVE]: во время overlap window для fake_blob используется **только current** `server_secret` (не previous). Иначе атакующий, наблюдающий timing changes между sessions, мог бы detect факт ротации через subtle differences в HKDF cache patterns. `server_secret_previous` используется **только** для backward-compat decrypt существующих state (если требуется), но не для генерации новых fake values.

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
expires_at_ns    = now_ns + SESSION_MAX_AGE_NS    // 24h в nanos

identity_input = "SHAMIR-IDENTITY-v1"
              || SHA256(server_pub_key)     // включён в подпись (защита от key-substitution)
              || u8(transport_kind)
              || u8(binding_mode)
              || tls_exporter_or_zeros(32)
              || auth_message
              || session_id(32)
              || u64_be(expires_at_ns)
identity_sig = Ed25519::sign(server_ed25519_priv, identity_input)
```

5.2.5. На fail → `{"error": "authentication_failed"}`. Backoff per `(client_ip_subnet, username_hash)` где subnet = `/24 IPv4` или `/64 IPv6`. Backoff: `100ms × 2^N`, cap 30s, reset 5 мин неактивности.

**Reset on success** [NORMATIVE]: при **успешной** аутентификации server **немедленно удаляет** запись `FailureState` для `(subnet, username_hash)`. Иначе legitimate user после нескольких typo получит persistent backoff даже после успешного login. Aналогично `lockout_state` для пары (если pre-threshold) — clear at success.

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

6.3. **Pinning model.** Pin = `SHA256(server_pub_key)` 32 байта, где `server_pub_key` — **raw 32-byte Ed25519 public key encoding** (RFC 8032 §5.1.5 compressed Y coordinate). НЕ SPKI/DER wrapping. Эта же raw form используется во всех `SHA256(server_pub_key)` operations (§5.2.4 identity_sig, §11.3.3 bootstrap, §12.2 rotation).

Источник:
- (a) URI param: `shamir+tcp://alice@host?pin=base64url(SHA256(pub))` (recommended for prod, native клиенты)
- (b) `~/.shamir/known_hosts` запись от предыдущего подключения (TOFU, native клиенты)
- (c) **Embedded constant в JS bundle** (browser admin UI only — см. CLIENT_BROWSER §4.2; известные limitations browser path docs там же + SECURITY_MODEL §4.9)

Для (a) и (b): если ни тот, ни другой not set И `--accept-new-host` не задан → клиент refuses подключение.

`known_hosts` сопровождается integrity tag — см. IMPLEMENTATION_GUIDE §7.

6.4. **Server identity rotation:** admin command `rotateServerIdentity` (§12.2). Поддерживает overlap-окно 7 дней (фиксировано) с подписью старым ключом перехода к новому.

### 6.5. Rotation orphan protection

**Проблема:** клиент offline во время rotation broadcast (§12.2 — broadcast только для активных сессий) → имеет old pin → следующее full SCRAM получает identity_sig от **new** priv → verify fail → "server compromised" → user thinks server hijacked, но это legitimate rotation.

**Решение:** во время overlap window сервер **дополнительно** включает `rotation_in_progress` payload в `auth_ok` (см. §2.4):
- `previous_pub` — старый Ed25519 pub
- `identity_sig_previous` = `Ed25519::sign(previous_priv, identity_input)` где `identity_input` **идентичен** §5.2.4 (с `server_pub_key = current_pub`). Обе подписи (current и previous) бьются над **тем же byte-exact identity_input**.
- `transition_until_ns`
- `rotation_proof` = `Ed25519::sign(previous_priv, ROTATION_PROOF_PAYLOAD)` где
  ```
  ROTATION_PROOF_PAYLOAD =
      "SHAMIR-ROTATE-PROOF-v1"              // 22 bytes ASCII
   || SHA256(previous_pub)                  // anti key-substitution
   || current_pub(32)                       // new key endorsed by old
   || u64_be(transition_until_ns)
  ```
  Подписан `previous_priv` — доказывает что rotation подписан кем-то с previous_priv. **Не доказывает что это legitimate server** (см. Security caveat ниже).

**Client handling (NORMATIVE):**

1. Verify `identity_sig` против `current_pub` через `verify_strict`. Fail → `server_signature_invalid`, disconnect.
2. Если `pinned_hash == SHA256(current_pub)` → already pinned, proceed.
3. Если `pinned_hash == SHA256(previous_pub)` AND `rotation_in_progress` present:
   - Verify `identity_sig_previous` против `previous_pub` (verify_strict) — должна совпасть с тем же byte-exact `identity_input` что используется для `identity_sig`
   - Verify `rotation_proof` против `previous_pub` (verify_strict)
   - Verify `transition_until_ns > now_ns` — overlap не истёк
   - **Verify `transition_until_ns ≤ now_ns + 7 days + 1 hour clock_skew`** (HIGH-2 fix): rotation_proof generated by attacker-with-leaked-key с far-future timestamp → reject
   - **Pin update is NEVER automatic.** Поведение клиента зависит от mode:
     - **Interactive CLI:** ВСЕГДА показать prompt с обоими fingerprints (old/new) и `transition_until_ns`. User должен **explicitly** confirm. Без confirmation → fail-closed, disconnect.
     - **Non-interactive (CI/cron/script):** ВСЕГДА fail-closed (disconnect, exit code != 0). Operator must use **explicit `--accept-rotation` flag** with awareness of security implications. Без флага — никогда auto-update pin.
4. Иначе (pinned ≠ current AND pinned ≠ previous) → `server_identity_changed`, disconnect.

#### Security caveat (CRITICAL — operator MUST understand)

`rotation_proof` доказывает только: *подписан кем-то владеющим previous_priv*. Это **НЕ** доказательство legitimate server, если `previous_priv` был **скомпрометирован** в любой момент за всю историю pinning'а.

**Concrete attack vector:**
- Атакующий получил `previous_priv` (стары backup, leaked dev машина, social engineering admin)
- Server делает routine planned `rotateServerIdentity` (без подозрений)
- Атакующий with network position (corporate proxy, ISP, MITM-capable) генерирует свою keypair `(att_pub, att_priv)`
- Атакующий перехватывает client connection и отдаёт fake auth_ok:
  - `server_pub_key = att_pub`
  - `identity_sig = sign(att_priv, identity_input_with_att_pub)` — валидно
  - `rotation_in_progress.rotation_proof = sign(LEAKED_previous_priv, ROTATION_PROOF_PAYLOAD with att_pub)` — валидно
- Client с old pin верифицирует rotation_proof успешно → user prompt asks "trust new identity?"
- Если user click yes (или operator forgot to verify out-of-band) → permanent pin redirect → all future connections MITM'd

**Mitigations:**

1. **Если `previous_priv` подозревается compromised**: использовать **emergency rotation** (`--identity-revoked` flag, IMPLEMENTATION_GUIDE §5.2), НЕ planned `rotateServerIdentity`. Emergency rotation НЕ выпускает rotation_in_progress payload — orphan клиенты получают `server_identity_changed` и выполняют manual re-pin out-of-band.
2. **Operators MUST** verify rotation through second channel (signed announcement по email, GPG-signed bulletin, etc.) перед confirming на каждом клиенте.
3. **Browser admin UI** (CLIENT_BROWSER.md): rotation_in_progress prompt должен показывать **оба** fingerprints visually и требовать typed confirmation, не просто click.
4. Server SHOULD include `transition_until_ns ≤ now + 7 days` (по умолчанию). Не давайте slack window > 7 дней.

Это закрывает orphan client UX problem **без** auto-trust. Cryptographic proof reduces user burden (compared to full out-of-band re-pin), but final trust decision = manual.

---

## 7. Session

7.1. `session_id = random(32)` выдаётся при `auth_ok`.

7.2. Server in-memory state:
```
struct Session {
    user_id: bytes(16),
    username: String,
    permissions: SessionPermissions,        // см. §7.3 — snapshot at auth
    created_at_ns: u64,                     // unix nanos — для validity check (§7.5)
    last_activity: AtomicU64,
    transport_kind: u8,                     // tcp=0x01, ws=0x02
    binding_mode: u8,                       // см. §4.2
    channel_binding_at_auth: bytes(32),     // снят с auth — для resumption check
}
```

(Nonces `client_nonce`/`server_nonce` НЕ хранятся в Session после handshake.)

7.3. **SessionPermissions** — snapshot ролей в момент auth:
```
struct SessionPermissions {
    is_superuser: bool,
    roles: Vec<String>,
    // имплементация может добавлять precomputed bitmasks
}
```
Изменение ролей админом (`updateUser`, §12.5) автоматически invalidates все existing sessions И tickets для затронутого юзера.

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

7.5. **Per-request session validity check** [NORMATIVE]:

**На каждом** request в активной сессии (после auth_ok) сервер проверяет **до** обработки запроса:

```
if session.created_at_ns <= user.tickets_invalid_before_ns:
    close session with error "session_invalidated"
    audit event session_evicted{reason="invalidated"}
    return
```

Это закрывает race между `updateUser`/`kickSession` и in-flight requests/resumes:
- Resume создаёт новую Session с `created_at_ns = now_ns`
- Если updateUser обновил `tickets_invalid_before_ns = now_ns_later` после создания этой сессии — следующий request от неё detected как invalid → kicked
- Без этой проверки сессия escape'нула updateUser snapshot и жила до idle TTL

Cost: один `u64 <=` compare per request — тривиально.

7.6. После auth_ok все запросы несут `session_id` (transport-specific framing — см. TRANSPORT_*.md).

7.7. Session не персистентна. Restart сервера → re-auth (или resume через ticket).

7.8. Disconnect transport → session evict через 5 секунд (window для transport switch via resumption).

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

8.2. **Pre-handshake state GC** каждые 5 секунд: state без proof в течение `HANDSHAKE_TIMEOUT` (= `max(15s, kdf_time × 5)`) → drop.

**Важно**: GC timeout **MUST** быть ≥ `HANDSHAKE_TIMEOUT`. Иначе legitimate mobile/slow-network клиенты с медленным Argon2id (~3-5s) + network latency теряют state до прихода `client_proof` → cryptic auth_failed без real cause. Раньше spec указывал hardcoded 10s — fixed на ≥ HANDSHAKE_TIMEOUT (выровнено с §8 table).

8.3. **Один TCP/WS connection** = один активный handshake state. Повторный `auth_init` в том же connection → close.

8.4. **Lockout silent**: response identical с `authentication_failed`. Внутреннее состояние не раскрывается.

8.5. **Latency padding** применяется к **всем** ответам сервера в auth flow (challenge response И negative-path responses). Цель — устранить timing oracle между real-vs-fake user paths (fake уходит в HKDF, real — в DashMap lookup; на microsecond уровне это различимо). Реализация: задержка до `target_constant_time_ms = fixed_floor_ms + uniform[0, jitter_max_ms]` где `fixed_floor_ms = 50` (защищает от LAN/loopback нанo-timing) и `jitter_max_ms = 25` (статистический шум). Эффективный диапазон: `[50, 75]` ms — соответствует диаграмме 01 шаг 14 ("50ms floor + uniform[0,25] jitter").

Trade-off: добавляет ~50ms latency per handshake. Acceptable — Argon2id уже занимает ~2с.

8.6. **Restart warmup window**: первые 60 секунд после старта сервер applies глобальный rate limit `RATE_LIMIT_AUTH_INIT_PER_SUBNET / 4 = 2.5/sec` пока in-memory state warmup'ится из persisted snapshots. Закрывает restart-replay window для distributed attackers.

8.7. **Server clock requirements**: server MUST использовать synchronized clock (NTP с smoothed time источниками или PTP). Большие clock jumps (>5 секунд назад) могут invalidate live tickets; jumps вперёд могут expire active sessions. При detection clock anomaly (`abs(now - last_observed) > 5s`) — log warning event + рекомендуется manual `revokeAllTickets`.

---

## 9. Constant-time Discipline

Защита состоит из **двух независимых слоёв**:

### 9.1. Branch-equivalent code paths

Одинаковые библиотечные вызовы на real-vs-fake путях. Применяется к:
- §5.2.1—5.2.4 SCRAM verify (real / fake path)
- §11 Bootstrap token check — `ConstantTimeEq(SHA256(token), stored_hash)`
- §5.3 Client mutual auth — все compare через `subtle::ConstantTimeEq` / `@noble/equalBytes`

**Ed25519 verify** — variable-time на public inputs (RFC 8032 §6). OK криптографически — нет remote oracle.

### 9.2. Wall-clock padding (см. §8.5)

Branch-equivalent **недостаточно** для anti-enumeration на microsecond уровне. Конкретно: real path делает DashMap lookup, fake path — HKDF derivation. Эти операции занимают разное время даже при идентичной структуре кода.

Защита: **latency padding** перед отправкой challenge response и negative-path responses (§8.5). Это устраняет timing oracle на сетевом уровне.

### 9.3. Что НЕ покрывается

- **Cache side-channels** (Argon2id internals, HMAC implementations vary by CPU)
- **Cross-VM timing** на shared host
- **Spectre/Meltdown classes** — out of scope (A8 в SECURITY_MODEL §1)

Mitigation для этих случаев = generic errors + generic latency + не запускать на shared host без isolation.

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
11.2.2. Сервер сохраняет атомарно: `bootstrap_token_hash = SHA256(token)`, `bootstrap_token_expires_at_ns = now_ns + bootstrap_token_ttl_ns`.

`bootstrap_token_ttl` configurable (server config):
- Default: **1 час**
- Min: **5 минут**
- Max: **24 часа**

Air-gapped deployments (где token нужно физически доставить к KMS) могут использовать max.

11.2.3. **Token output** (server config):
- `--bootstrap-token-tty` (default): печать в stdout **только** если `isatty(stdout)`. Иначе server fails с инструкцией.
- `--bootstrap-token-file <path>`: атомарно создать `chmod 600` файл, записать токен. Server **MUST** удалять файл (a) при `bootstrap_used` event AND (b) фоновым GC при `now_ns > bootstrap_token_expires_at_ns`. На startup server проверяет file existence vs `bootstrap_token_hash IS NULL` и удаляет orphan файл (audit event `bootstrap_token_file_orphan_cleaned`).
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
    "server_time": u64,                    // unix nanos, передаётся на проводе для verify
    "identity_sig_bootstrap": bytes(64)
}}
```
где
```
identity_sig_bootstrap = Ed25519::sign(priv,
    "SHAMIR-BOOTSTRAP-v1"
    || SHA256(server_pub_key)              // защита от key-substitution; SHA-256 от raw 32-byte Ed25519 encoding (RFC 8032 §5.1.5)
    || u8(transport_kind)
    || tls_exporter(32)
    || client_nonce(32)                    // anti-replay binding к этому клиенту
    || u64_be(server_time)
)
```

**Все** поля подписываемого payload передаются клиенту явно (`server_pub_key`, `server_time` на wire; `transport_kind`, `tls_exporter`, `client_nonce` известны клиенту локально). Без этого клиент не может воссоздать payload для verify.

11.3.4. Клиент **MUST** валидировать **в указанном порядке** (любой fail → disconnect, plain password не уходит):
- (a) `ConstantTimeEq(SHA256(server_pub_key), pinned_hash)` — pin check
- (b) Ed25519 verify_strict(server_pub_key, identity_input, identity_sig_bootstrap) — подпись valid
- (c) **`ConstantTimeEq(client_nonce_in_signed_payload, client_nonce_отправленный_в_bootstrap_hello)`** — anti-replay challenge другому клиенту
- (d) `abs(now - server_time) ≤ 60 секунд` — clock anomaly detection

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
- Validate `bootstrap_token_expires_at_ns > now_ns` AND `ConstantTimeEq(SHA256(token), bootstrap_token_hash)`
- Validate `kdf_params == server_defaults` (защита от malicious client'а)
- Validate username:
  - PRECIS UsernameCaseMapped + NFC
  - **Если username уже существует** → `bootstrap_failed` (no overwrite). Operator должен сначала удалить collision через `--list-users` / `--delete-user` CLI.
- Создаёт user с ролью `superuser`
- Set `bootstrap_token_hash = NULL`, `bootstrap_token_expires_at_ns = NULL`, `superuser_ever_existed = true`
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
Response: { "ok": { "new_pub": bytes(32), "transition_until_ns": u64 } }
```

Процедура:
1. **Pre-condition check** (NORMATIVE — HIGH-5 fix): server **отклоняет** rotateServerIdentity если `now_ns < server_ed25519_rotation_until_ns` (т.е. previous rotation overlap window ещё не закончился). Error: `rotation_in_progress_already`.
   - Защита: иначе `previous_pub` (исходный) был бы перезаписан вторым rotation → клиенты с original pin становятся permanently locked out (нет rotation chain length > 1 в v1).
   - Operator workflow: ждать 7 дней между rotations OR использовать emergency rotation (`--identity-revoked`) для immediate revoke.
2. Server генерит новый keypair, atomic store: `previous_pub = current_pub; previous_priv = current_priv; current_pub = new_pub; current_priv = new_priv; rotation_until_ns = now_ns + 7 days`.
3. **Per-recipient signing:** для каждой активной сессии server вычисляет **отдельную** `Ed25519::sign(...)`. Кэширование подписей запрещено — payload уникален per-recipient.
4. Broadcast event:
```
{ "identity_rotation": {
    "old_pub": bytes(32),
    "new_pub": bytes(32),
    "transition_until_ns": u64,
    "recipient_session_id": bytes(32),         // полный sid (не prefix), уникален per recipient
    "signed_by_old": bytes(64)
}}
where signed_by_old = Ed25519::sign(old_priv,
    "SHAMIR-ROTATE-v1"
    || SHA256(old_pub)                         // domain binding to current key (anti-stale-pin attack)
    || new_pub
    || u64_be(transition_until_ns)
    || recipient_session_id(32)                // anti-replay between recipients
)
```
5. Клиент: проверяет **в указанном порядке** (любой fail → close connection без обновления pin, audit event `identity_rotation_invalid`):
   - (a) `ConstantTimeEq(SHA256(old_pub), pinned_hash)` — old_pub matches currently pinned
   - (b) `ConstantTimeEq(recipient_session_id, my_session_id)` — message не для другого recipient
   - (c) Ed25519 verify_strict(old_pub, signed_by_old payload, signed_by_old signature) — подпись valid
   - (d) `transition_until_ns > now_ns + 60_000_000_000` (60s в nanos) — overlap window реалистичен
   - (e) **`transition_until_ns ≤ now_ns + 7 days + 1 hour clock_skew`** — upper bound (HIGH-2 fix)
   
   На success: обновляет pin (interactive prompt mandatory для interactive clients, или explicit `--accept-rotation` flag для non-interactive). См. §6.5 client handling — те же правила.
6. Через `transition_until_ns` server zeroize old priv. После этой точки `rotateServerIdentity` снова доступен.

**Двойная rotation:** запрещена в течение overlap window (см. step 1 pre-condition). Operator должен:
- Ждать `transition_until_ns` истечения, затем повторно вызвать `rotateServerIdentity` (если нужно ещё раз ротировать)
- ИЛИ использовать emergency rotation (`--identity-revoked`) для immediate revoke без orphan client recovery

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

**Атомарно (single transaction):** kill matching sessions + update `user.tickets_invalid_before_ns = now_ns` для затронутых юзеров (защита от resumption через украденный ticket с устаревшими ролями).

### 12.5. `updateUser` (admin)

```
Request:  { "updateUser": {
              "user": String,
              "roles": Option<Vec<String>>
           }}
Response: { "ok": { "changes_applied": bool } }
```

**No-op semantic:** если `roles == None` AND ничего не меняется реально → server возвращает `{ok: {changes_applied: false}}` **без** обновления `tickets_invalid_before_ns` и **без** kill sessions. Защита от silent DoS через repeated `updateUser(alice)` без аргументов (иначе атакующий-admin или buggy script могли бы forced full SCRAM каждую минуту).

**Если есть реальные изменения** — атомарная процедура (single transaction, persist barrier):

```
1. Update user record (roles → new value) → persist (durable)
2. Set user.tickets_invalid_before_ns = now_ns → persist (durable)
3. (Persist barrier — все subsequent reads видят новое значение)
4. Optional: snapshot active sessions matching user_id, close connections (best-effort eviction)
5. Audit event roles_changed
6. Response { changes_applied: true }
```

**Race protection — двухуровневая:**
- (a) **In-flight resumption:** новый resume after step 2 fails check `original_auth_at_ns > tickets_invalid_before_ns` (SESSION_RESUMPTION §5.4 step 9, strict `>`)
- (b) **Sessions созданные между step 2 и step 4** (resume concurrent): покрываются **per-request session validity check** (§7.5) — на следующем request session detected as invalidated и kicked
- Step 4 = best-effort eager eviction для immediate kill TCP connection. Без §7.5 step 4 был бы insufficient race window.

(Self-service смена пароля удалена в v1: admin пересоздаёт юзера через CLI delete + `createUser` (§12.1). Self-service password rotation — кандидат на v1.1.)

### 12.5.1. `setSuperuser` (admin)

```
Request:  { "setSuperuser": {
              "user": String,
              "on": bool,
              "hmac": Option<String>           // hex HMAC-SHA256 tag — UNCONDITIONAL
           }}
Response: { "superuser_set": { "user": String, "on": bool } }
```

Grant (`on=true`) or revoke (`on=false`) the superuser flag on an existing
SCRAM-directory account. Requires an already-superuser session. The HMAC
tag is **unconditional** — every call must supply it (the canonical form is
`b"set_superuser\0<user>\0<on>"` with `<on>` as the literal `"true"`/`"false"`).

This is a **top-level `DbRequest`** (not a `BatchOp`): it dispatches through
the server's connection layer, which has direct access to the user directory
— the batch engine does not. It mirrors `createUser`'s (§12.1) top-level
shape for the same reason. After success the target's outstanding tickets
are invalidated via `tickets_invalid_before_ns` so a stale ticket can never
serve the old privilege state.

**Note (task #557):** the literal `"superuser"` string is RESERVED at the
directory write boundary — supplying it via `createUser`'s `roles` field
surfaces a `query`-class error. Use this op (`setSuperuser`) to grant admin
powers; ordinary role strings are attached via `updateUser` (§12.5) or the
RBAC `grant_role`/`revoke_role` batch ops ("role" is a plain string label,
task #549 — there is no "role object" to create/drop/rename/list).

### 12.6. Информационные команды

`whoami`, `listSessions`, `serverInfo` — schemas и поведение в IMPLEMENTATION_GUIDE.md §13. Не security-критичны, не требуют superuser (кроме `listSessions` всех юзеров).

---

## 13. Argon2id Parameter Migration

13.1. KDF params хранятся per-user в user record (§3.5).

13.2. Глобальный server config — `kdf_params_current` — для новых регистраций и migration target. Должен соответствовать floor (§3.7.2).

13.3. При login сервер использует stored kdf_params для verify.

13.4. Если `user.kdf_params != kdf_params_current` AND все params strictly_weaker (memory_kb/time/parallelism все ≤ current):
- В `auth_ok` сервер шлёт `"kdf_upgrade_required": true` как **информационный hint** для клиента/operator audit.
- В v1 self-service KDF upgrade **отсутствует** (вместе с self-service password rotation — удалена в v1). Migration выполняется admin-driven: пересоздание юзера через `createUser` (§12.1) с current `kdf_params_current`.
- Audit event `kdf_params_upgrade_required` (server-side hint).
- v1.1 ROADMAP: dedicated `kdfUpgrade` endpoint, который re-derive'ит `stored_key`/`server_key` под current params без kill sessions.

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
- Pinned **Unicode version 15.1** для v1 (test vectors зависят от этой версии — см. §16)

Lookup и хранение — **после** нормализации. Длина измеряется в **байтах** UTF-8 (после NFC), max 255.

**Forbidden character handling** [NORMATIVE]: при detection запрещённого символа в `auth_init.user` сервер **MUST**:
1. НЕ запускать Argon2id (constant-time discipline)
2. Возвращать generic `{"error": "authentication_failed"}` (без раскрытия причины)
3. Закрыть connection
4. Audit event `auth_failed{reason="invalid_username_chars"}` (только во внутренний log)

**Cross-language consistency** требование: Rust impl (через `unicode-normalization` + `precis-profiles`) и JS impl (через `String.prototype.normalize("NFC").toLowerCase()` + custom blacklist) должны pass одинаковые test vectors. Mismatch → release blocker.

15.4. **Password не нормализуется** (NIST SP 800-63B §5.1.1.2). UTF-8 bytes как есть.

15.5. **Timestamp convention** [NORMATIVE]: **все** wire-level и persisted timestamps используют **unix nanoseconds** (`u64`) с суффиксом `_ns` в именах полей. Никаких `_secs` / `_at` без суффикса в protocol-level fields.

Применяется к: `expires_at_ns`, `tickets_invalid_before_ns`, `created_at_ns`, `updated_at_ns`, `original_auth_at_ns`, `bootstrap_token_expires_at_ns`, `transition_until_ns`, `last_audit_checkpoint_at_ns`, `audit log ts_ns`, и т.д.

Server clock drift acceptable если `< 5s` (см. §8.7). Implementations **MUST** use monotonic-corrected unix nanos (NTP-disciplined) — наивный `gettimeofday` уязвим к user-space clock manipulation на host.

---

## 16. Test Vectors

**Release blocker для v1.** Все векторы — фиксированные, byte-exact, вычисленные
запуском РЕАЛЬНЫХ crypto-функций Rust-имплементации с фиксированными входами
(не hand-computed). Каждая имплементация (Rust native, browser SDK) обязана
воспроизвести pinned hex побайтово.

### 16.1. Расположение и формат

Векторы живут в `crates/shamir-connect/test-vectors/` в виде **per-vector
JSON+TOML пар** (git-diffable, human-readable, по одному файлу на категорию):

- `.json` — canonical cross-language source of truth (browser/TS SDK грузит эти
  файлы напрямую).
- `.toml` — тот же вектор; потребляется Rust-тестами через `include_str!` +
  `toml::from_str` (см. `common/tests/test_vectors_tests.rs`).

> Исторически §16 требовала единый `test-vectors/auth_v1.msgpack`. Такой файл
> в репозитории **никогда не существовал**; per-vector JSON+TOML пары — реально
> установленная, работающая конвенция (см. `README.md` в той же папке).
> Msgpack использовался бы только как *derived* экспорт из JSON, если потребуется
> — но не как отдельный source of truth. Схема каждого файла:
> `{ "name", "spec_section", "inputs": {...}, "expected": {...} }`.

Все векторы разделяют **один когерентный фиксированный сценарий**:
`username="alice"`, фиксированные nonces/salt, `KdfParams::DEFAULT`
(memory_kb=131072, time=4, parallelism=1, argon2_version=0x13),
transport_kind=tcp, binding_mode=tls_exporter, фиксированный tls_exporter — те же
входы, что в inline-примере ниже. Векторы цепочку образуют end-to-end:
auth_message → Argon2id → SCRAM proofs → identity_sig.

### 16.2. Inline example: auth_message hex dump

```
Inputs:
  username = "alice"          (5 bytes UTF-8 NFC casemapped)
  client_nonce = 00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff
  server_nonce = 20212223...3e3f   (32 bytes 0x20..0x3f)
  salt         = 5051...5e5f       (16 bytes 0x50..0x5f)
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

Total auth_message length: 14 + 2 + 5 + 32 + 32 + 16 + 4+4+4+1 + 1+1+32 + 1 = 149 bytes
```

Закреплён в `auth_message_default.{json,toml}`.

### 16.3. Полный набор векторов (файлы в `crates/shamir-connect/test-vectors/`)

| Файл | Категория | Что pinned |
|---|---|---|
| `auth_message_default.{json,toml}` | auth_message construction | 149-byte canonical auth_message (inline example выше) |
| `kdf_canonical_string_default.{json,toml}` | kdf_canonical_string | 13-byte BE-serialization of KdfParams |
| `argon2id_default.{json,toml}` | Argon2id | `Argon2id("hello world!1", fixed salt, DEFAULT)` → 32-byte salted_password |
| `scram_flow_default.{json,toml}` | SCRAM chain | DerivedKeys → client_proof + server_signature над pinned auth_message |
| `identity_sig_default.{json,toml}` | identity_sig | Ed25519 (fixed seed) → identity_input + 64-byte signature |
| `fake_blob_default.{json,toml}` | fake_blob | HKDF(server_secret, "alice") → 80-byte blob (salt‖stored_key‖server_key) |
| `resumption_ticket_roundtrip.{json,toml}` | resumption ticket | AES-256-GCM encrypt/decrypt над realistic TicketPlain (msgpack) |
| `identity_rotation_signed_by_old.{json,toml}` | identity rotation | Ed25519 old→new rotation event signature (signed_by_old) |

Каждый вектор проверяется Rust-тестом в `common/tests/test_vectors_tests.rs`,
который перезапускает РЕАЛЬНУЮ функцию с фиксированными входами и assert'ит
byte-for-byte equality против pinned `expected`. Любой drift (reorder
domain-tag, изменение HKDF info-string, иное кодирование KdfParams) ломает тест.

---

## 17. Domain Separation Tags

| Tag | Использование |
|---|---|
| `"Client Key"` | HMAC(salted_password) → client_key |
| `"Server Key"` | HMAC(salted_password) → server_key |
| `"SHAMIR-FAKE-SALT-v1"` | HKDF salt для fake values |
| `"SHAMIR-AUTH-v1"` | Header auth_message |
| `"SHAMIR-IDENTITY-v1"` | Префикс identity_sig |
| `"SHAMIR-BOOTSTRAP-v1"` | Bootstrap challenge sig |
| `"SHAMIR-ROTATE-v1"` | Identity rotation event sig (active session broadcast) |
| `"SHAMIR-ROTATE-PROOF-v1"` | Rotation proof в auth_ok (orphan client recovery, §6.5) |
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
- **../roadmap/ROADMAP.md** — v1.1+ planned features
