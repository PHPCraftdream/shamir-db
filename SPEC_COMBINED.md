# ShamirDB Authentication Protocol v1 — Complete Specification

> Single-file consolidated specification. Source: `spec/` directory + `ROADMAP.md`.
> Order matches recommended reading flow.
>
> Prepared for external review.

---

## Table of Contents

- Part 1: README — Overview & Navigation
- Part 2: AUTH_PROTOCOL — Transport-agnostic SCRAM core
- Part 3: SECURITY_MODEL — Adversary model, threats, recovery
- Part 4: SESSION_RESUMPTION — Fast reconnect tickets
- Part 5: TRANSPORT_TCP — TCP binding
- Part 6: TRANSPORT_WS — WebSocket binding
- Part 7: ADMIN_UI_HOSTING — Static UI + Bearer REST
- Part 8: CLIENT_BROWSER — Browser SDK guidelines
- Part 9: IMPLEMENTATION_GUIDE — Operational details
- Part 10: ROADMAP — v1.1+ planned features

---


---

## Part 1: README

> Source file: `spec/README.md`

## ShamirDB Protocol Specification

Спецификация transport-agnostic аутентификации и сессий. Один auth протокол, много транспортов.

### Принципы

1. **Простота** — каждый документ читается изолированно, ≤ 600 строк
2. **Универсальность** — auth не зависит от транспорта
3. **Security first** — все ревью-фиксы внутри (см. SECURITY_MODEL.md)
4. **Browser-friendly** — JS/WASM клиенты first-class

### Архитектура

```
                  ┌─────────────────────────────────┐
                  │  AUTH_PROTOCOL.md               │
                  │  Transport-agnostic SCRAM       │
                  │  + Ed25519 + channel binding    │
                  └──────────────┬──────────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              │                  │                  │
        ┌─────▼─────┐      ┌─────▼─────┐     ┌──────▼──────┐
        │   TCP     │      │  WS (wss) │     │  Admin UI   │
        │ (TLS|plain│      │ native +  │     │ static +    │
        │ loopback) │      │ browser   │     │ Bearer REST │
        └───────────┘      └───────────┘     └─────────────┘
```

### Документы

#### Core (нормативные)
- **[AUTH_PROTOCOL.md](AUTH_PROTOCOL.md)** — handshake, key derivation, errors. Transport-agnostic.
- **[SESSION_RESUMPTION.md](SESSION_RESUMPTION.md)** — fast reconnect, anti-downgrade rules.

#### Reference (informative)
- **[SECURITY_MODEL.md](SECURITY_MODEL.md)** — adversary model, threat coverage, non-guarantees.
- **[IMPLEMENTATION_GUIDE.md](IMPLEMENTATION_GUIDE.md)** — operational details (storage, observability, audit, recovery runbooks).

#### Transport bindings
- **[TRANSPORT_TCP.md](TRANSPORT_TCP.md)** — TCP (TLS или plain loopback).
- **[TRANSPORT_WS.md](TRANSPORT_WS.md)** — WebSocket (wss; native + browser endpoints).
- **[ADMIN_UI_HOSTING.md](ADMIN_UI_HOSTING.md)** — static admin UI + Bearer REST.

#### Clients
- **[CLIENT_BROWSER.md](CLIENT_BROWSER.md)** — browser SDK: WASM crypto, CSP, anti-XSS.

#### Future (вне `spec/`)
- **[../ROADMAP.md](../ROADMAP.md)** — v1.1+ planned features.

#### Test vectors
- `test-vectors/auth_v1.json` — **release blocker** для v1 (см. AUTH_PROTOCOL §16).

### Версионирование

- `auth_init.version: u8` — major version **AUTH_PROTOCOL.md**. Единственная версия в handshake.
- Каждый документ имеет свою версию в header. Backward-compat = minor bump. Wire-breaking = major bump.
- Domain tags привязаны к version своего документа: `SHAMIR-TICKET-v2` может появиться без `SHAMIR-AUTH-v2`.
- Compatibility matrix — IMPLEMENTATION_GUIDE.md §9.

### Статус

**v1 — draft.** Ревью пройдены (3 итерации, 3 reviewer perspectives). Test vectors — TBD при имплементации.

---

## Part 2: AUTH_PROTOCOL

> Source file: `spec/AUTH_PROTOCOL.md`

## ShamirDB Authentication Protocol v1

**Transport-agnostic** аутентификация. Только последовательность сообщений и crypto. Конверты (TCP/WS) — в TRANSPORT_*.md. Operational детали (метрики, логи, recovery) — в IMPLEMENTATION_GUIDE.md.

База: **SCRAM** (RFC 5802 idea), **Argon2id** для KDF, **Ed25519** для server identity, **HMAC-SHA256** для proof и transcript, **HKDF-SHA256** для key derivation.

---

### 1. Принципы

1.1. **Один auth flow**, один transport-agnostic протокол. Различия — только в конверте.

1.2. **Plain password никогда не покидает клиент.** Регистрация/смена пароля — derived ключи.

1.3. **Argon2id ровно в одном месте:** `password → salted_password` на клиенте.

1.4. **Server identity** через Ed25519, независимо от транспортного TLS. Pinning по `SHA256(server_pub_key)`.

1.5. **Channel binding** включает: TLS exporter (если есть), transport_kind, binding_mode. Защита от UKS/triple-handshake/cross-transport downgrade.

1.6. **Constant-time discipline** для anti-enumeration.

1.7. **Browser-friendly** через WASM (см. CLIENT_BROWSER.md). Те же примитивы, ослабленный channel binding (явно объявлен).

---

### 2. Сообщения

Сериализация: **msgpack** (любая valid RFC compliant — `auth_message` имеет независимую canonical форму, см. §4). Поля типа `bytes(N)` → msgpack `bin`.

#### 2.1. `auth_init` (Client → Server)

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

#### 2.2. `challenge` (Server → Client)

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

#### 2.3. `client_proof` (Client → Server)

```
{ "client_proof": bytes(32) }
```

#### 2.4. `auth_ok` (Server → Client)

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

#### 2.5. `error` (Server → Client)

```
{ "error": "authentication_failed" }                      // generic для auth/replay/lockout
{ "error": "rate_limited", "retry_after": u32 }
{ "error": "server_busy", "retry_after": u32 }
{ "error": "unsupported_version" }
```

---

### 3. Регистрация

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

### 4. Канонический auth_message

Используется для подписей и proof. **Все** имплементации обязаны байт-в-байт идентичный результат.

#### 4.1. Формат

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

#### 4.2. transport_kind, binding_mode, tls_exporter

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

#### 4.3. Server policy для `binding_mode` (NORMATIVE)

Сервер для каждого listener конфигурирует **разрешённые** `binding_mode`:
- TCP+TLS listener: `binding_mode == 0x01` (требует exporter)
- TCP plain listener: `binding_mode == 0x00`
- WSS native listener: `binding_mode == 0x01`
- WSS browser listener: `binding_mode == 0x02` (отдельный endpoint, см. TRANSPORT_WS.md)

**MUST:** Server **обязан** rejектить `auth_init` где `binding_mode` не входит в listener policy **до** запуска Argon2id (защита от DoS amplification — иначе атакующий flood'ит mismatched binding_mode и forces 128 MB × time=4 на каждый запрос).

Reject = silent close без error message (анти-fingerprinting listener policy).

#### 4.4. Anti-downgrade свойство

Любое расхождение между сторонами → различные `auth_message` → различные `client_proof` → SCRAM verify fail → reject.

Per-listener policy (§4.3) исключает MITM подмену `binding_mode` в `auth_init`.

---

### 5. SCRAM Verification

#### 5.1. Client computes

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

#### 5.2. Server verifies

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

#### 5.3. Client verifies

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

### 6. Server Identity (Ed25519)

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

#### 6.5. Rotation orphan protection

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

##### Security caveat (CRITICAL — operator MUST understand)

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

### 7. Session

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
    pending_changepw_challenge: Option<{    // см. §12.5
        server_nonce_cp: bytes(32),
        client_nonce_cp: bytes(32),
        issued_at_ns: u64,
    }>
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

### 8. Лимиты и DoS защита

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

8.5. **Latency padding** применяется к **всем** ответам сервера в auth flow (challenge response И negative-path responses). Цель — устранить timing oracle между real-vs-fake user paths (fake уходит в HKDF, real — в DashMap lookup; на microsecond уровне это различимо). Реализация: задержка до `target_constant_time_ms = max(jitter_ms, fixed_floor_ms)` где `fixed_floor_ms = 50` (защищает от LAN/loopback нанo-timing) + `jitter_ms = uniform[0, 25]` (статистический шум).

Trade-off: добавляет ~50ms latency per handshake. Acceptable — Argon2id уже занимает ~2с.

8.6. **Restart warmup window**: первые 60 секунд после старта сервер applies глобальный rate limit `RATE_LIMIT_AUTH_INIT_PER_SUBNET / 4 = 2.5/sec` пока in-memory state warmup'ится из persisted snapshots. Закрывает restart-replay window для distributed attackers.

8.7. **Server clock requirements**: server MUST использовать synchronized clock (NTP с smoothed time источниками или PTP). Большие clock jumps (>5 секунд назад) могут invalidate live tickets; jumps вперёд могут expire active sessions. При detection clock anomaly (`abs(now - last_observed) > 5s`) — log warning event + рекомендуется manual `revokeAllTickets`.

---

### 9. Constant-time Discipline

Защита состоит из **двух независимых слоёв**:

#### 9.1. Branch-equivalent code paths

Одинаковые библиотечные вызовы на real-vs-fake путях. Применяется к:
- §5.2.1—5.2.4 SCRAM verify (real / fake path)
- §11 Bootstrap token check — `ConstantTimeEq(SHA256(token), stored_hash)`
- §5.3 Client mutual auth — все compare через `subtle::ConstantTimeEq` / `@noble/equalBytes`

**Ed25519 verify** — variable-time на public inputs (RFC 8032 §6). OK криптографически — нет remote oracle.

#### 9.2. Wall-clock padding (см. §8.5)

Branch-equivalent **недостаточно** для anti-enumeration на microsecond уровне. Конкретно: real path делает DashMap lookup, fake path — HKDF derivation. Эти операции занимают разное время даже при идентичной структуре кода.

Защита: **latency padding** перед отправкой challenge response и negative-path responses (§8.5). Это устраняет timing oracle на сетевом уровне.

#### 9.3. Что НЕ покрывается

- **Cache side-channels** (Argon2id internals, HMAC implementations vary by CPU)
- **Cross-VM timing** на shared host
- **Spectre/Meltdown classes** — out of scope (A8 в SECURITY_MODEL §1)

Mitigation для этих случаев = generic errors + generic latency + не запускать на shared host без isolation.

---

### 10. Хранение

См. IMPLEMENTATION_GUIDE.md §1 (нормативные SystemStore схемы, file locations, file permissions, MAC integrity для known_hosts).

Краткая структура:
- `__system__/users/{user_id}` — см. §3.5
- `__system__/server_meta` — server_secret + lockout_secret + ed25519 keypair + ticket_key + audit_chain_key + bootstrap state
- In-memory: handshake states, auth_failures, lockout state, consumed_counters
- **Lockout state и consumed_counters** persisted in SystemStore с **батчингом** (NORMATIVE: flush ≤ 5 секунд OR при достижении threshold, синхронный flush на graceful shutdown)

---

### 11. Bootstrap

#### 11.1. Trigger

При первом запуске сервера если выполнено **всё**:
- `__system__/users` пуст
- `__system__/server_meta.bootstrap_token_hash IS NULL`
- `__system__/server_meta.superuser_ever_existed == false`

Защита от silent re-bootstrap при corrupted backup: после первого успешного bootstrap флаг `superuser_ever_existed = true` навсегда (даже после deletion всех юзеров).

#### 11.2. Token issuance

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

#### 11.3. Bootstrap connection

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

#### 11.4. Recovery

Lost admin → `shamir-server --regen-bootstrap --confirm` (требует stop сервера + физический/SSH доступ + флаг `--confirm`):
- Server reads stdin для confirmation phrase
- Generate новый `bootstrap_token`, `superuser_ever_existed` остаётся `true`
- Output token per `--bootstrap-token-*` config
- Audit event `bootstrap_regen` записан
- Старые admin'ы НЕ удаляются автоматически — operator делает manual cleanup после re-bootstrap

---

### 12. Admin Commands

Все выполняются внутри активной auth-сессии. Authorization — `is_superuser` в `SessionPermissions`. Audit event на каждое выполнение.

#### 12.1. `createUser`

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

#### 12.2. `rotateServerIdentity`

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

#### 12.3. `unlockUser`

```
Request:  { "unlockUser": { "user": String } }
Response: { "ok": {} }
```
Сбрасывает **И** `lockout_state` **И** `auth_failures` для всех subnet ключей данного user. Иначе пользователь застревает в высоком backoff после unlock.

#### 12.4. `kickSession`

```
Request:  { "kickSession": {
              "user": Option<String>,
              "session_id_prefix": Option<bytes>     // hex prefix ≥4 bytes
           }}
Response: { "ok": { "killed_count": u32 } }
```

**Атомарно (single transaction):** kill matching sessions + update `user.tickets_invalid_before_ns = now_ns` для затронутых юзеров (защита от resumption через украденный ticket с устаревшими ролями).

#### 12.5. `changePassword` (self-service)

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

## [NORMATIVE] Single in-flight challenge per session_id:
## Server stores pending_changepw_challenge в Session struct (§7.2).
## Повторный changePasswordChallenge от той же session → **invalidates** previous
## (overwrites pending_changepw_challenge). Multi-tab user должен это понимать —
## первый submit fails если другой tab инициировал свой challenge ранее.

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

## Client derives заново (Argon2id ~2с):
salted_old      = Argon2id(old_password, salt, kdf_params)
client_key_old  = HMAC(salted_old, "Client Key")
stored_key_old  = SHA256(client_key_old)
client_sig_cp   = HMAC(stored_key_old, auth_message_cp)
client_proof_old = client_key_old XOR client_sig_cp

## New material:
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
## kdf_params от клиента игнорируются — server применяет current defaults

Step 5 — Server verifies:
## Lookup session.pending_changepw_challenge — должен быть present
## (иначе client пытается submit без prior changePasswordChallenge → reject)
## Использует server_nonce_cp + client_nonce_cp ИЗ session.pending state, не из network message.

client_signature = HMAC(user.stored_key, auth_message_cp)
recovered = client_proof_old XOR client_signature
ok = ConstantTimeEq(SHA256(recovered), user.stored_key)

## Atomic on success: clear session.pending_changepw_challenge (single-use), persist new keys.

Step 6 — Server → Client: { "ok": {} } или { "error": "authentication_failed" }
```

12.5.1. Server проверяет SCRAM proof старого пароля **без plain password** и **без серверного Argon2id** (нет DoS amplification).

12.5.2. Server **игнорирует** client-supplied kdf_params — всегда server defaults.

12.5.3. **Все сессии юзера убиваются** (включая текущую) И `tickets_invalid_before_ns = now_ns`. Клиент должен переаутентифицироваться.

12.5.4. Serialized per user (mutex). Atomic update.

#### 12.6. `updateUser` (admin)

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

(Принудительная смена пароля — v1.1; для v1: admin удаляет user через CLI и создаёт заново через `createUser`, или просит юзера сменить через §12.5 self-service.)

#### 12.7. Информационные команды

`whoami`, `listSessions`, `serverInfo` — schemas и поведение в IMPLEMENTATION_GUIDE.md §13. Не security-критичны, не требуют superuser (кроме `listSessions` всех юзеров).

---

### 13. Argon2id Parameter Migration

13.1. KDF params хранятся per-user в user record (§3.5).

13.2. Глобальный server config — `kdf_params_current` — для новых регистраций и migration target. Должен соответствовать floor (§3.7.2).

13.3. При login сервер использует stored kdf_params для verify.

13.4. Если `user.kdf_params != kdf_params_current` AND все params strictly_weaker (memory_kb/time/parallelism все ≤ current):
- В `auth_ok` сервер шлёт `"kdf_upgrade_required": true`.
- Клиент использует **тот же** two-step flow что в §12.5 (`changePasswordChallenge` → `changePassword`) с тем же паролем для re-derive под новыми params.
- Transparent UX. Audit event `kdf_params_upgraded`.

13.5. Anti-enumeration: для unknown user сервер возвращает **current** kdf_params. Старые юзеры с устаревшими params видны атакующему — known trade-off.

---

### 14. Errors

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

### 15. Encoding

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

### 16. Test Vectors

**Release blocker для v1.** Файл `spec/test-vectors/auth_v1.json` обязан содержать полный набор. Inline minimal example для bootstrapping имплементаций:

#### Example: auth_message hex dump

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

Полный test-vectors JSON содержит:
- `kdf_canonical_string` (legacy compat reference)
- `Argon2id(password="hello world!1", salt=fixed, params=defaults)` → 32-byte output
- `client_proof`, `server_signature`, `identity_sig` для полного flow
- `fake_blob` через HKDF для fixed username → 80 байт hex
- Resumption ticket: encrypt/decrypt round-trip с fixed key/nonce
- Identity rotation `signed_by_old` для fixed inputs

Каждая имплементация (Rust native, browser SDK) обязана pass всех vectors.

---

### 17. Domain Separation Tags

| Tag | Использование |
|---|---|
| `"Client Key"` | HMAC(salted_password) → client_key |
| `"Server Key"` | HMAC(salted_password) → server_key |
| `"SHAMIR-FAKE-SALT-v1"` | HKDF salt для fake values |
| `"SHAMIR-AUTH-v1"` | Header auth_message |
| `"SHAMIR-CHGPW-v1"` | Header auth_message_cp в changePassword |
| `"SHAMIR-IDENTITY-v1"` | Префикс identity_sig |
| `"SHAMIR-BOOTSTRAP-v1"` | Bootstrap challenge sig |
| `"SHAMIR-ROTATE-v1"` | Identity rotation event sig (active session broadcast) |
| `"SHAMIR-ROTATE-PROOF-v1"` | Rotation proof в auth_ok (orphan client recovery, §6.5) |
| `"EXPORTER-ShamirDB-AUTH-v1"` | TLS exporter label |
| `"SHAMIR-TICKET-v1"` | См. SESSION_RESUMPTION.md |

---

### 18. Versioning

18.1. `auth_init.version: u8` — major version **AUTH_PROTOCOL.md**. Единственная версия в handshake.

18.2. Каждый документ имеет свою версию в header. Backward-compat = minor bump. Wire-breaking = major bump.

18.3. Domain tags привязаны к **document version**. `SHAMIR-TICKET-v2` может появиться без `SHAMIR-AUTH-v2`.

18.4. Compatibility matrix — IMPLEMENTATION_GUIDE.md §9.

---

### 19. См. также

- **SECURITY_MODEL.md** — adversary model, threat coverage, non-guarantees, recovery overview
- **IMPLEMENTATION_GUIDE.md** — operational details (storage, observability, audit log, log redaction, dependencies, recovery runbooks, admin command schemas)
- **SESSION_RESUMPTION.md** — fast reconnect через ticket
- **TRANSPORT_TCP.md, TRANSPORT_WS.md** — конкретные transport bindings
- **ADMIN_UI_HOSTING.md** — static admin UI delivery + REST для активных сессий
- **CLIENT_BROWSER.md** — browser SDK guidelines
- **../ROADMAP.md** — v1.1+ planned features

---

## Part 3: SECURITY_MODEL

> Source file: `spec/SECURITY_MODEL.md`

## ShamirDB Security Model v1

Adversary model, threat coverage, non-guarantees, compromise recovery overview.

Operational детали (метрики, audit log, log redaction, recovery runbooks) — **IMPLEMENTATION_GUIDE.md**.
Future hardening — **../ROADMAP.md**.

---

### 1. Adversary Model

| ID | Adversary | В scope v1? |
|---|---|---|
| A1 | Passive network observer | Yes |
| A2 | Active network MITM (включая corporate proxy с rogue CA) | Yes |
| A3 | Offline DB snapshot (`__system__/*`) | Yes |
| A4 | Live host RAM read | Partial — defence-in-depth |
| A5 | Malicious admin | Out of scope |
| A6 | Compromised client device | Partial — known_hosts MAC, no client priv |
| A7 | Supply chain | Out of scope |
| A8 | Cache-timing / Spectre | Out of scope (acknowledged) |
| A9 | Hardware tampering / cold boot | Out of scope |
| A10 | DoS — botnets, amplification | In scope |
| A11 | Single-process RCE → all secrets | **Acknowledged limitation** (см. §4.13) |
| A12 | Compromised admin UI origin (CDN hijack, malicious deploy) | Partial — для browser path documented limitation |

---

### 2. Threat Coverage

| Угроза | Adv | Защита |
|---|---|---|
| Passive eavesdropping | A1 | Транспортный TLS |
| Active MITM | A2 | TLS + Ed25519 server pin + channel_binding в auth_message |
| Password sniffing | A1, A2 | SCRAM: пароль не покидает клиент |
| Server impersonation после DB leak | A3 | Ed25519 priv в server_meta отдельно от users |
| User enumeration via timing | A1, A2 | Constant-time fake values via HKDF, generic errors, padded latency |
| User enumeration via channel binding | A2 | binding_mode embedded в auth_message → MITM detection |
| KDF param downgrade | A2 | Raw kdf_params в auth_message; server-side floor |
| Mid-session downgrade через changePassword | A5 | Server игнорирует client kdf_params, всегда defaults |
| Online brute-force | A2 | Backoff + rate limit per (subnet, user_hash), silent lockout |
| Distributed brute-force через ботнет | A2, A10 | Backoff per subnet; lockout cross subnets per user |
| Offline brute-force после DB leak | A3 | Argon2id 128 MB / time=4 — memory-hard |
| Replay handshake | A2 | TLS prevents within session; channel_binding per-connection |
| Bootstrap MITM | A2 | Out-of-band pin обязателен; client_nonce в подписи; native-only |
| Bootstrap token leak via logs | A6 | TTY-only OR file mode; no logging; configurable TTL |
| Bootstrap CAS race | A2, A10 | Mutex + CAS. Invariant `bootstrap_token_hash NULL ⇔ superuser_ever_existed` |
| Silent re-bootstrap при corrupted backup | A6 | `superuser_ever_existed` flag персистентен |
| TOFU bypass via known_hosts deletion | A6 | known_hosts integrity tag (см. IMPLEMENTATION_GUIDE) |
| Session theft via file read | A6 | Session не persist'ится; ticket в memory only (browser) |
| Cross-transport resumption downgrade | A2, A6 | binding_strength monotonicity rule |
| Resumption ticket replay | A6 | Monotonic counter с synchronous persist |
| Stale resumption после revoke ролей | A5 (mitigated) | `tickets_invalid_before` инвалидирует tickets, обновляется через kickSession/changePassword/updateUser |
| Identity rotation event replay | A2 | Per-recipient `session_id` (32 байта) в подписи |
| Identity rotation chain attack (stale pin) | A2 | `SHA256(old_pub)` в подписи |
| KCI (Key Compromise Impersonation) | A3 | Ed25519 priv независим от пароля |
| DoS — Argon2id flood | A10 | Semaphore 64 concurrent; permit только на Argon2id; pre-Argon2id binding_mode check |
| DoS — slowloris pre-proof | A10 | Pre-state GC через 10s |
| DoS — frame flood after auth | A10 | PER_SESSION_MEM=64 MB |
| DoS — admin lockout | A10 | Lockout per (subnet, user). Emergency `--regen-bootstrap` |
| Restart-replay window | A2 | Warmup rate limit `/4` первые 60s |
| Audit log truncation | A3 | Periodic `last_audit_hmac` checkpoint в server_meta + startup verify |
| **Browser TLS MITM → session hijack** | A2, A12 | TLS exporter недоступен в browser → relay attack возможен. SCRAM защищает password, но `session_id` может быть hijacked. См. §4.9 + recommend native client для admin operations. |
| Backup restore counter rollback → ticket replay | A6, A2 | Mandatory `revokeAllTickets` при любом restore (см. IMPLEMENTATION_GUIDE §5.7) |

---

### 3. Compromise Recovery (overview)

Detailed runbooks — IMPLEMENTATION_GUIDE.md §5.

| Утечка | Что атакующий получает | Высокоуровневое действие |
|---|---|---|
| `server_secret` | Possible enumeration precompute | Rotate с overlap 7 дней |
| `lockout_secret` | Может вычислять lockout state keys offline (tracking attempts not modifying) | Rotate с migration state |
| `server_ed25519_priv` | Server impersonation | Kill switch → `rotateServerIdentity` → re-pin клиентам |
| `audit_chain_key` | Может forge log entries | Rotate с overlap 30 дней; compromised entries marked stale |
| Audit log truncation (offline tampering) | Скрыть последние events | `last_audit_*` checkpoint в server_meta detects при startup verify |
| DB users snapshot | Offline brute (Argon2id-bound) | Force password rotation всем |
| Полный SystemStore | Всё выше + ticket_key + audit_chain_key | Полный teardown + re-bootstrap + audit forensics |
| Client password | User access | User меняет пароль |
| Client `known_hosts` | TOFU bypass attempt | Integrity MAC спасает; иначе re-pin |
| Bootstrap token (в TTL) | Создание admin (если ещё не использован) | TTL спасает; alert на bootstrap_used event |
| `ticket_key` | Forge tickets для всех юзеров | `revokeAllTickets` |
| Browser session ticket (XSS) | One resume-then-counter-blocked | `kickSession` + `revokeUserTickets` |

---

### 4. Non-Guarantees

ShamirDB Auth Protocol v1 НЕ гарантирует:

4.1. **Forward secrecy против host RAM compromise.** TLS 1.3 даёт против network observer.

4.2. **Per-request integrity внутри сессии** на уровне application protocol. TLS / channel binding покрывают transport.

4.3. **Session non-transferability.** session_id — bearer. RAM read на клиенте → имперсонация до idle TTL.

4.4. **Защита от malicious admin.** Superuser имеет полный доступ. Audit log с HMAC chain (IMPL §3.3) даёт forensics, но не prevention.

4.5. **Username confidentiality.** Username виден серверу. Длина наблюдаема через TLS frame size.

4.6. **Защита от cache-timing attacks.** Mitigation = generic errors + generic latency.

4.7. **Защита от physical access.** Cold-boot, evil maid. Mitigation = mlock + disable_core_dumps best-effort.

4.8. **PQ resistance** для server identity. Ed25519 — Shor's. См. ROADMAP.

4.9. **Browser-mode security parity с native.** Browser лишён TLS exporter API → channel_binding ослаблен (`binding_mode=0x02`). Mitigations: strict TOFU + out-of-band pinning + HSTS + CSP.

**Practical attack vectors при TLS MITM в browser path** (corporate proxy с installed CA, DNS hijack + rogue Let's Encrypt cert):
- Атакующий перехватывает `GET /admin/static/main.<hash>.js`
- Подменяет embedded Ed25519 server pin на свой
- Браузер юзера загружает modified bundle, доверяет attacker pin
- Атакующий проксирует SCRAM messages — relay attack
- **Password не утекает** (SCRAM делает свою работу — proof не reusable)
- НО **session_id может быть hijacked**: атакующий получает auth_ok с валидным session_id для своей connection

**Mitigation для admin operations: использовать native client с out-of-band Ed25519 pin.** Browser admin UI приемлем для read-only / low-stakes operations.

Resumption tickets из browser path **не могут upgrade** в native session при `allow_browser_ticket_upgrade=false` (server config).

4.10. **Password length confidentiality.** В v1 password length не передаётся серверу и не хранится.

4.11. **Resumption replay через graceful crash window — устранено** (synchronous fsync). Но: если SystemStore сам corrupt (disk failure), counter может откатиться → ticket replay в окне до восстановления. Mitigation: `revokeAllTickets` после disaster recovery.

4.12. **Lockout state warmup window.** Первые 60 секунд после restart применяется reduced rate limit (`/4`) пока in-memory state warmup'ится. Без этого distributed attacker мог бы получить burst attempts.

4.13. **Single-process trusted server.** ShamirDB v1 = monolithic process с всеми secrets (`server_secret`, `lockout_secret`, `server_ed25519_priv`, `ticket_key`, `audit_chain_key`) в одной address space. **RCE в любой части = compromise всех secrets simultaneously**. Mitigations:
- mlock на priv keys (best-effort)
- disable_core_dumps (best-effort per OS)
- Минимизация attack surface (no FFI с untrusted libraries, no eval)
- Run в отдельном UID/container с restricted privileges
- v1.1+ ROADMAP: privilege separation (отдельный signer process для Ed25519 ops)

**Это не уязвимость**, это design choice для simplicity v1. Документируется явно чтобы deployment expectations были realistic.

4.14. **Clock dependency.** Auth flow зависит от server clock (timestamps в tickets, expires_at, tickets_invalid_before_ns). Server MUST использовать synchronized clock (NTP/PTP). При clock jumps > 5s — recommended manual `revokeAllTickets`. Audit event `clock_anomaly_detected` если abs(now - last_observed) > 5s.

---

### 5. Standards Compliance

| Standard | Status | Notes |
|---|---|---|
| RFC 5802 (SCRAM) | Modified | PBKDF2→Argon2id (memory-hard upgrade); GS2 framing→msgpack; SASLprep→PRECIS |
| RFC 9106 (Argon2) | Compliant | Argon2id v1.3 (0x13) |
| RFC 5869 (HKDF) | Compliant | HKDF-SHA256 для anti-enumeration fake values |
| RFC 8032 (Ed25519) | Compliant | `verify_strict`: reject non-canonical S, small-order pub, mixed-cofactor |
| RFC 9266 (Channel Binding via TLS Exporter) | Compliant | Label `EXPORTER-ShamirDB-AUTH-v1`, ctx empty, L=32 |
| RFC 5077 (Session Tickets) | Inspired | Self-contained AES-256-GCM encrypted ticket с AAD binding |
| RFC 8265 (PRECIS) | Compliant | UsernameCaseMapped profile (NFC + casefold + ban control/format) |
| NIST SP 800-63B | Mostly compliant | Min 12 chars (>NIST 8); salt 128 bits (>32). Argon2id не в NIST allow-list (FIPS profile в ROADMAP) |
| OWASP ASVS 4.0 V2.4/V2.7/V3.3/V9 | Compliant | Argon2id 128 MB > OWASP min 19 MiB; OOB pin; session timeouts; TLS |

---

### 6. См. также

- **AUTH_PROTOCOL.md** — нормативный протокол
- **IMPLEMENTATION_GUIDE.md** — operational details + audit chain HMAC
- **SESSION_RESUMPTION.md** — ticket protocol
- **../ROADMAP.md** — v1.1+ roadmap

---

## Part 4: SESSION_RESUMPTION

> Source file: `spec/SESSION_RESUMPTION.md`

## ShamirDB Session Resumption v1

Быстрый reconnect (~10ms вместо ~2s Argon2id). Поддержка cross-transport с **анти-downgrade защитой**. Multi-device через **ticket families** (одно устройство не invalidates ticket другого).

---

### 1. Когда полезно

- Mobile NAT rebinding (новый TCP/IP)
- Cross-transport switch (TCP → WS) **в пределах одного security tier**
- CLI tools — короткоживущие команды
- Failover к replica
- **Multi-device:** laptop refresh не ломает mobile session

---

### 2. Ticket Format

#### 2.1. Plaintext

```
ticket_plain = canonical_msgpack({
  "version": 1,                           // u8
  "user_id": bytes(16),
  "username_nfc": String,
  "permissions": SessionPermissions,      // см. AUTH_PROTOCOL §7.3
  "transport_kind_at_auth": u8,
  "binding_mode_at_auth": u8,
  "channel_binding_at_auth": bytes(32),
  "ticket_family_id": bytes(16),          // см. §6.2 — per-device lineage
  "original_auth_at_ns": u64,             // unix nanos; ВСЕГДА от первого full SCRAM, не обновляется при refresh
  "expires_at_ns": u64,
  "family_counter": u64                   // monotonic в пределах family_id
})
```

`ticket_plain` использует **canonical msgpack** (lex-sorted keys, smallest int encoding, no NaN) — потому что AAD валидация и detection tampering зависят от bit-exact bytes.

**Why `_ns` (nanoseconds)** вместо seconds: устраняет race window в 1 секунду между `original_auth_at` и `tickets_invalid_before` (см. §6.3).

#### 2.2. Wire format

```
ticket_wire = struct {
  version: u8 = 1,                        // визибл в envelope, используется для AAD + dispatch
  nonce: bytes(12),                       // CSPRNG, per-ticket
  ciphertext_len: u16_be,
  ciphertext: bytes,                      // AES-256-GCM ciphertext (encrypts ticket_plain)
  tag: bytes(16)
}

// AAD строится только из полей envelope (visible до decrypt):
aad = "SHAMIR-TICKET-v1" || u8(version)

ciphertext, tag = AES-256-GCM(
    key   = ticket_key,
    nonce = nonce,
    plaintext = ticket_plain,             // canonical msgpack от §2.1
    aad   = aad
)
```

**Почему AAD содержит только `version`:**
- AAD строится **до** decrypt — должна быть выводима из envelope (visible bytes), не из plaintext.
- `transport_kind_at_auth` и `binding_mode_at_auth` — **внутри** ciphertext. Их integrity полностью покрывает GCM tag над ciphertext+plaintext.
- Атакующий, попытавшийся подменить любое поле в ciphertext (без знания ticket_key) → GCM tag verify fail → reject.
- Cross-version защита: атакующий не может использовать v2 ticket_key для расшифровки v1 ticket — domain tag `"SHAMIR-TICKET-v1"` + version в AAD различаются.

Размер: ~150-300 байт.

---

### 3. Ticket Key

3.1. `ticket_key: bytes(32)` хранится в `__system__/server_meta`.

3.2. **Ротация каждые 24 часа** автоматически:
- `previous = current; current = random(32)`
- Через 24 часа `previous = NULL`
- При decrypt сервер пробует `current`, потом `previous`. Constant-time не требуется.

3.3. **Emergency rotation:** admin command `revokeAllTickets` — `current = random(32); previous = NULL` немедленно. Все existing tickets invalid.

3.4. Audit event на каждую ротацию: `rotate_ticket_key`.

---

### 4. Issuance

4.1. Server выдаёт ticket в `auth_ok` (опционально):
```
{
  "auth_ok": {
    ...,
    "resumption_ticket": bytes,           // ticket_wire
    "resumption_expires_at_ns": u64
  }
}
```

4.2. Issued при:
- **Initial auth после полного SCRAM:** `ticket_family_id = random(16)`, `original_auth_at_ns = now_ns`, `family_counter = 1`. **Каждое новое full SCRAM создаёт новую family** (laptop full SCRAM → family_A; mobile full SCRAM → family_B).
- **`refreshTicket`** команда в активной сессии: новый ticket наследует **тот же** `ticket_family_id` и `original_auth_at_ns`. `family_counter` инкрементируется внутри family. Server **немедленно invalidates** previous ticket для этой family (атомарный update `current_ticket_id` per-session).

`original_auth_at_ns` и `ticket_family_id` обновляются **только** при full SCRAM re-auth. Это обеспечивает:
- Multi-device: laptop refresh (family_A counter→2,3,4) не invalidates mobile ticket (family_B counter=1). См. §6.2.
- Корректную работу `tickets_invalid_before` инвалидации (см. §6.3): инвалидируются **все** families юзера.

4.3. **Не issued** для:
- Bootstrap-сессий (single-use admin создание)
- HTTP-bearer-сессий (HTTP не поддерживает primary auth — см. ADMIN_UI_HOSTING.md)

---

### 5. Resume Flow

5.1. Клиент инициирует **новое соединение** на любом supported transport.

5.2. Transport handshake (TLS / WSS) — стандартный.

5.3. Клиент шлёт **первое сообщение**:
```
{
  "resume": {
    "ticket": bytes,                      // ticket_wire
    "client_nonce": bytes(32),
    "binding_mode_now": u8,
    "channel_binding_now": bytes(32)
  }
}
```

5.4. Server проверяет в **строгом порядке**:

```
1. Parse ticket_wire envelope (version, nonce, ciphertext_len, ciphertext, tag)
2. Validate envelope.version supported (e.g., == 1); unsupported → resumption_failed
3. Construct aad = "SHAMIR-TICKET-v1" || u8(envelope.version)
4. AES-256-GCM decrypt(ticket_key, nonce, ciphertext, tag, aad):
   - Try current ticket_key first
   - Fail → try previous ticket_key
   - Both fail (tag verify) → resumption_failed
   - Success → ticket_plain bytes
5. Parse ticket_plain (canonical msgpack)
6. Verify ticket_plain.version == envelope.version (defense-in-depth)
7. Verify ticket_plain.expires_at_ns > now_ns
8. Lookup user by user_id; if not exists → resumption_failed
9. Verify ticket_plain.original_auth_at_ns > user.tickets_invalid_before_ns
   (СТРОГОЕ >, не >= — защита от race в 1ns timing collision)
10. SECURITY DOWNGRADE CHECK (см. §6.1):
    binding_strength(binding_mode_now) >= binding_strength(ticket_plain.binding_mode_at_auth)
    (Если allow_browser_ticket_upgrade = false в server config:
     binding_mode_now == binding_mode_at_auth required, см. §6.1)
11. ATOMIC compare-and-swap по (user_id, family_id):
    key = (ticket_plain.user_id, ticket_plain.ticket_family_id)
    if ticket_plain.family_counter > consumed_counters[key]:
        consumed_counters[key] = ticket_plain.family_counter
        SYNCHRONOUS DURABLE persist (fsync + storage engine durability — см. IMPLEMENTATION_GUIDE §1.3)
        proceed
    else:
        resumption_failed
12. Создаёт новую Session с binding_mode_now, channel_binding_now
13. Reply auth_ok с новым session_id и (опц) новым ticket (та же family_id, counter+1)
```

5.5. На fail на любом шаге → `{"error": "resumption_failed"}` (generic).

5.6. На success:
```
{
  "resume_ok": {
    "session_id": bytes(32),
    "expires_at_ns": u64,
    "resumption_ticket": Optional<bytes>,
    "resumption_expires_at_ns": Optional<u64>
  }
}
```

#### 5.7. Resumption во время identity rotation

Если ticket был issued под `previous` Ed25519 keypair и `transition_until_ns > now_ns`:

**v1 поведение:** server отвергает resume → клиент выполняет full re-auth.

После full re-auth client получает `auth_ok` с **`rotation_in_progress`** payload (см. AUTH §6.5) → handles pin update согласно §6.5 (interactive prompt mandatory или `--accept-rotation` flag для non-interactive). Только после успешного pin update сессия используется.

Цена: клиенты со stale tickets во время окна rotation теряют ~2 секунды (Argon2id full re-auth) на первое подключение + interactive prompt time. Acceptable trade-off против complexity multi-key signing в resume_ok.

После окна rotation (`now_ns > transition_until_ns`) resumption работает нормально с current keypair. `rotation_in_progress` отсутствует в `resume_ok` schema (§5.6) — только в `auth_ok`.

---

### 6. Безопасность

#### 6.1. Anti-downgrade rule (CRITICAL)

```
binding_strength:
  binding_mode == 0x00 (none / plain)            → 0
  binding_mode == 0x02 (tls_no_export browser)   → 1
  binding_mode == 0x01 (tls_exporter)            → 2

resume reject if binding_strength(now) < binding_strength(at_auth)
```

**Семантика:** ticket выпущенный в TLS-exporter сессии не может быть resumed в plain или browser сессии. Browser ticket по умолчанию **может** быть resumed в native (upgrade OK). Plain ticket — куда угодно.

**Strict mode** через server config `allow_browser_ticket_upgrade = false` (default `true`):
- Browser ticket (binding_mode=0x02) НЕ может быть resumed в native (binding_mode=0x01)
- Закрывает hypothetical pivot: атакующий с украденным browser ticket (XSS) → resume в native session с stronger trust
- Trade-off: legitimate user не может upgrade с browser session на native CLI без re-auth
- Recommended для high-security deployments

#### 6.2. Anti-replay через per-family monotonic counter

Server поддерживает per-(user_id, family_id) counter:
```
consumed_counters: DashMap<(user_id, ticket_family_id), u64>
```

`ticket_family_id` создаётся **per device / per initial auth** (см. §4.2). Refreshed tickets наследуют family. Каждое новое full SCRAM = новая family.

При resume: `ticket.family_counter > consumed_counters[(user_id, family_id)]` (atomic CAS) → consume + **synchronous durable persist** → update.

**Multi-device benefit:** laptop refresh (family_A counter→3) НЕ invalidates mobile ticket (family_B counter=1). Каждое устройство имеет свою независимую цепочку.

**SYNCHRONOUS DURABLE persist обязателен.** "Durable" значит:
- POSIX `fsync` на counter file/page
- SQLite: `PRAGMA synchronous=FULL` + WAL + checkpoint OR `PRAGMA synchronous=EXTRA`
- Embedded engines (sled/redb/fjall): equivalent guarantees
- ext4: mount option `barrier=1` (default modern)
- Power-fail testing — release blocker для production deployment
См. IMPLEMENTATION_GUIDE §1.3 [NORMATIVE].

Иначе при crash в окне flush counter откатывается → атакующий с украденным ticket может resume повторно. Cost = один disk-sync per resume — приемлемо (resume = редкая операция).

**Defence properties:**
- Прямой replay: ticket consumed → family counter advanced → старый rejected
- Race: atomic CAS обеспечивает строгое serialized consume per-family
- LRU eviction атак нет (counter не evicted внутри valid window)
- Crash-restart: durable persist гарантирует consistency
- Multi-device: family isolation предотвращает self-DoS

**Background GC** [NORMATIVE]: server каждые 60 секунд удаляет entries из `consumed_counters` где `last_observed_at + RESUMPTION_MAX_CHAIN_AGE < now` (т.е. family уже не может выдать валидный ticket — `original_auth_at_ns + 24h < now`). Это предотвращает unbounded growth при многих устройствах × годах работы. Удаление безопасно — никакой ticket для этой family уже не пройдёт `expires_at_ns` check (§5.4 step 4).

`last_observed_at` = max(time consumed, time issued) для family. Tracked рядом с counter в DashMap или derived из `original_auth_at_ns + RESUMPTION_MAX_CHAIN_AGE` (если family создана только initial auth — оценка достаточна).

#### 6.3. Ticket invalidation

Сервер invalidates **все** tickets **всех families** юзера через `tickets_invalid_before_ns` (см. AUTH_PROTOCOL §3.5). Triggers:
- `kickSession` admin command (§12.4 AUTH)
- `changePassword`
- `updateUser` с new roles (§12.6 AUTH)
- Manual `revokeUserTickets` admin command (см. §7)

При resume (§5.4 step 6): `ticket.original_auth_at_ns > user.tickets_invalid_before_ns` обязательно. **Строгое `>`** (не `>=`) исключает race window даже если timestamp resolution коллидирует.

`original_auth_at_ns` НЕ обновляется при `refreshTicket` (§4.2) — это обеспечивает что вся цепочка refreshed tickets (вся family) разом invalidates.

#### 6.4. Cross-transport allowed scenarios

| at_auth | now | Allowed? |
|---|---|---|
| TLS exporter (TCP) | TLS exporter (WS) | ✓ same tier |
| TLS exporter (WS) | TLS exporter (TCP) | ✓ same tier |
| Browser (WS no exp) | TLS exporter (TCP) | ✓ upgrade |
| TLS exporter | Browser | ✗ DOWNGRADE — reject |
| TLS exporter | plain (loopback) | ✗ DOWNGRADE — reject |
| plain | TLS exporter | ✓ upgrade |
| plain | plain | ✓ same tier |

**Plain ticket** (`binding_mode_at_auth = 0x00`) может быть resumed на любом transport. Это intentional для embedded handoff scenarios. Если undesired — server config `disable_plain_ticket_upgrade = true` отвергает resume `0x00 → 0x01/0x02`.

#### 6.5. Что НЕ защищает

- **Stolen ticket на secure transport** — атакующий может resume **один раз** до того как counter инкрементнётся легитимным клиентом. Mitigation: `kickSession` + `revokeUserTickets` при подозрении.
- **Compromised ticket_key** — атакующий forge для всех юзеров. Mitigation: ротация каждые 24 часа + emergency `revokeAllTickets`.
- **Browser sessionStorage XSS** — НЕ применимо (ticket в memory only, см. CLIENT_BROWSER §5.2).

#### 6.6. Лимиты

| Параметр | Значение |
|---|---|
| `RESUMPTION_TTL` | 1 час с issue (per-ticket) |
| `RESUMPTION_MAX_CHAIN_AGE` | = `SESSION_MAX_AGE` (24 часа) от `original_auth_at` — full re-auth обязателен |
| `RESUMPTION_RATE_LIMIT_PER_SUBNET` | 30/мин (унифицировано с AUTH §8 на subnet) |
| `TICKET_KEY_ROTATION` | 24 часа (overlap 24 часа) |

После `original_auth_at + RESUMPTION_MAX_CHAIN_AGE` ticket не выдаётся → клиент должен сделать full re-auth.

---

### 7. Disabling Resumption

7.1. **Server config** `--no-resumption` — server не выдаёт tickets, отвергает `resume`.

7.2. **Per-user revocation** через `revokeUserTickets` admin command — `user.tickets_invalid_before_ns = now_ns`. Все existing tickets invalidated.

7.3. **Per-session** через `kickSession` — также обновляет `tickets_invalid_before_ns`.

7.4. **Global emergency** через `revokeAllTickets` — rotates `ticket_key` без overlap (`previous = NULL`).

---

### 8. Errors

| Error | Trigger |
|---|---|
| `resumption_failed` | Generic — expired, invalid, replayed, downgrade attempt, ticket_key mismatch |
| `resumption_disabled` | Server config disabled |
| `rate_limited` | Resume rate exceeded per subnet |

---

### 9. Audit Events

См. IMPLEMENTATION_GUIDE.md §3.

- `resumption_used` (sampled)
- `resumption_replay_detected` (always)
- `resumption_downgrade_blocked` (always — security event)
- `revoke_user_tickets`
- `revoke_all_tickets`
- `rotate_ticket_key`

---

### 10. Implementation Notes

10.1. **Tickets и Session — разные структуры.** Ticket = stateless wire. Session = per-connection state.

10.2. **family_counter persistence:** **SYNCHRONOUS DURABLE** (fsync + storage engine durability) перед `resume_ok`. См. §6.2 + IMPLEMENTATION_GUIDE §1.3. Не batched.

10.3. **Atomic CAS in DashMap:** `consumed_counters.entry((user_id, family_id)).and_modify(|c| if ticket.family_counter > *c { *c = ticket.family_counter; consume_ok = true })` + immediate durable persist.

10.4. **Tests (release blocker):**
- Round-trip: issue → resume → new session active
- Replay: same ticket twice → second `resumption_failed`
- Expired (past `expires_at_ns`): fail
- Downgrade: TLS-bound ticket on plain transport → fail
- Upgrade: browser ticket on TLS-exporter transport → ok (default config)
- Strict mode: browser→native upgrade rejected when `allow_browser_ticket_upgrade=false`
- Cross-transport same tier: TCP↔WS → ok
- **Multi-device family isolation:** device A refresh advances family_A; device B's family_B ticket still valid
- **Race attack:** strict `>` comparison: `original_auth_at_ns == tickets_invalid_before_ns` → reject
- Counter persistence: resume → server crash → next attempt with same family_counter → fail
- `tickets_invalid_before_ns` обновлён → tickets всех families юзера: fail
- `refreshTicket` сохраняет `original_auth_at_ns` И `ticket_family_id` → kickSession invalidates all families
- Emergency `revokeAllTickets` → all existing tickets fail
- Key rotation: ticket issued under previous key resumes after rotation; invalid после `previous = NULL`
- AAD tampering: modify aad bytes → GCM fail
- Plain ticket upgrade: with `disable_plain_ticket_upgrade=true` → fail; without → ok
- **Property-based downgrade test** (proptest): random ticket params + random session params, assert anti-downgrade invariants

---

### 11. Browser-specific

См. CLIENT_BROWSER.md §5.

**Critical:** browser **НЕ** хранит ticket в `localStorage` / `sessionStorage` / `IndexedDB` / cookies. Ticket — **только** в memory JS variable. Tab close = full re-auth (~2с Argon2id, acceptable trade-off против XSS escalation).

---

## Part 5: TRANSPORT_TCP

> Source file: `spec/TRANSPORT_TCP.md`

## Transport: TCP

### 1. Профили

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

### 2. Framing

Length-prefixed msgpack:
```
[length: u32 BE][msgpack: length bytes]
```

- Empty frame (length=0) — graceful close
- До auth_ok: `length ≤ MAX_PRE_AUTH_FRAME = 4 KB`
- После auth_ok: `length ≤ MAX_FRAME_SIZE_DATA = 16 MB`
- Frame too large → TCP close без reply

### 3. TLS (tcp+tls)

3.1. **TLS 1.3 only.** TLS 1.2 и earlier reject.

3.2. **Cipher suites:** rustls defaults.

3.3. **Certificate verification на клиенте:** НЕ через CA. Identity = Ed25519 pin.

3.4. **TLS exporter:** `EXPORTER-ShamirDB-AUTH-v1`, контекст пустой, длина 32 байта (RFC 9266).

3.5. **TLS 1.3 0-RTT:** запрещён.

### 4. Channel Binding в auth_message

| Профиль | binding_mode | tls_exporter_or_zeros |
|---|---|---|
| tcp+tls | `0x01` | TLS-Exporter (32 bytes) |
| tcp+plain | `0x00` | bytes(32) zeros |

Несовпадение клиент/сервер policy → handshake fail (auth_message не совпадает).

### 5. Connection Lifecycle

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

### 6. Session Frame Format

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

### 7. Connection String Examples

```
shamir+tcp://alice@db.example.com:7331?pin=base64url(SHA256(server_pub))
shamir+tcp://alice@127.0.0.1:7334?plain=1
shamir+tcp://alice@10.0.0.5:7331?pin=...&accept_new_host=1   # TOFU first time
```

### 8. Test Checklist

- Round-trip auth с TLS, с plain (loopback), с TOFU pin, с out-of-band pin
- Frame too large → TCP close
- Empty frame → graceful close
- Повторный auth_init → close
- TLS 1.2 client → reject
- TLS 0-RTT → reject
- Plain профиль на non-loopback → server fails to start
- Audit event `auth_success` содержит `transport: "tcp"`

---

## Part 6: TRANSPORT_WS

> Source file: `spec/TRANSPORT_WS.md`

## Transport: WebSocket

Только `wss://`. Plain WS удалён в v1 (browser требует TLS, native может использовать TCP plain).

### 1. Профили

| Профиль | URI | binding_mode | Endpoint |
|---|---|---|---|
| `ws+tls` (native) | `shamir+ws://...` over `wss://` | `0x01` (tls_exporter) | `/shamir/v1` |
| `ws+tls_browser` | `shamir+ws://...` over `wss://` | `0x02` (tls_no_export) | `/shamir/v1/browser` |

Server **two listeners на разных endpoints/портах**: native клиенты подключаются к `/shamir/v1`, browser admin UI — к `/shamir/v1/browser`. Endpoint определяет policy `binding_mode`. **Никакого UA-detection.**

Native клиент пытающийся `/shamir/v1/browser` → допустим (но downgrade), browser пытающийся `/shamir/v1` → fail на handshake (нет TLS exporter API).

### 2. WebSocket Setup

2.1. WS subprotocol negotiation: клиент шлёт `Sec-WebSocket-Protocol: shamir-v1`. Сервер confirm same. Mismatch → 400.

2.2. WS binary frames только. Text frames → close 1003 (`unsupported data`).

2.3. **Только msgpack** wire encoding. JSON canonical удалён в v1.

2.4. **TLS 1.3 0-RTT запрещён** для WSS (сервер не должен принимать early data). Same as TCP+TLS (TRANSPORT_TCP §3.5) — защита от replay 0-RTT данных + forward secrecy.

### 3. Channel Binding

| Endpoint | binding_mode | tls_exporter_or_zeros |
|---|---|---|
| `/shamir/v1` | `0x01` | TLS-Exporter `EXPORTER-ShamirDB-AUTH-v1` (32 bytes) |
| `/shamir/v1/browser` | `0x02` | bytes(32) zeros |

`binding_mode = 0x02` явно сигнализирует "TLS присутствует, но клиент не имеет API для exporter — browser path". Это **policy decision**, embedded в auth_message → MITM не может switch без поломки proof.

Browser-mode security trade-offs — см. SECURITY_MODEL §4.9 + CLIENT_BROWSER.md §4.

### 4. Connection Lifecycle

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

### 5. Session Frame Format

Идентично TRANSPORT_TCP §6 — `{sid, req}` ↔ `{rid, res}` в msgpack-binary WS message.

### 6. Close Codes

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

### 7. Heartbeat

WS ping/pong каждые 30 секунд (server-initiated). Client должен отвечать pong в течение 10 секунд. Иначе server close 1006.

Не заменяет SESSION_IDLE_TTL (30 минут) — это для detection мёртвого TCP без RST.

### 8. Frame Size Limits

Идентичны TCP. Pre-auth `≤ 4 KB`, data `≤ 16 MB`. Превышение → close 1009.

### 9. Origin Header

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

### 10. Test Checklist

- WS upgrade + auth round-trip native (`/shamir/v1`)
- WS upgrade + auth round-trip browser (`/shamir/v1/browser`)
- Text frame на binary endpoint → close 1003
- Origin missing на browser endpoint → 400 (browsers всегда шлют Origin)
- Origin mismatch → 403
- Heartbeat dead detection
- Resumption: tcp↔ws same-tier работает; ws_browser → ws_native НЕ может resume (downgrade — не блокировано, это upgrade — OK); tls_exporter → browser endpoint НЕ может resume (это downgrade — block)
- Audit event содержит `transport: "ws"`

---

## Part 7: ADMIN_UI_HOSTING

> Source file: `spec/ADMIN_UI_HOSTING.md`

## Admin UI Hosting

HTTP-сервер для **раздачи статической admin SPA** + REST endpoints для управления **уже-выданной** сессией. **Не** primary auth transport. Auth — через WS или TCP (см. TRANSPORT_WS.md, TRANSPORT_TCP.md).

(Этот документ заменяет removed `TRANSPORT_HTTP.md`.)

---

### 1. Endpoints

```
GET  /admin/                        — admin SPA index.html
GET  /admin/static/*                — bundle (JS/CSS/WASM)
GET  /shamir/v1/health              — liveness probe (no auth)
GET  /shamir/v1/version             — version, supported protocol versions (no auth)
GET  /admin/metrics                 — Prometheus metrics (Bearer admin session)
POST /shamir/v1/query               — REST query (Bearer session_id)
POST /shamir/v1/admin/<command>     — admin commands (Bearer admin session_id)
```

WS upgrade endpoints `/shamir/v1` и `/shamir/v1/browser` — обслуживаются на том же HTTP listener (см. TRANSPORT_WS.md).

---

### 2. Auth Flow

**HTTP не делает primary auth.** Клиент:
1. Получает session_id через WS-handshake (либо native либо browser endpoint).
2. Использует session_id как Bearer token для последующих REST вызовов.

Это унифицирует auth path: все сессии создаются одним flow (WS), HTTP только consumes их.

---

### 3. REST Request Format

```
POST /shamir/v1/query
Authorization: Bearer <base64url(session_id)>
Content-Type: application/msgpack
Body: msgpack({ ...query... })

Response 200:
Content-Type: application/msgpack
Body: msgpack({ ...result... })

Response 401 / Body: msgpack({"error": "session_expired"})
```

**Только msgpack** wire encoding (то же что и WS binary frames). JSON опция удалена в v1 — single encoding путь упрощает имплементацию и закрывает класс interop bugs. curl-friendliness достигается через CLI tool или `xxd`/`msgpack-cli` обёртки.

**Bearer не cookie:**
- Нет CSRF surface
- Browser admin UI хранит session_id в **memory only** (см. CLIENT_BROWSER.md §5)

---

### 4. Static Admin UI Delivery

#### 4.1. Bundle structure

```
/admin/                          → index.html
/admin/static/main.<hash>.js     → app bundle
/admin/static/main.<hash>.css
/admin/static/argon2.<hash>.wasm → ~30 KB
```

URLs содержат content hash для cache busting + Subresource Integrity.

#### 4.2. Server config

```toml
[admin_ui]
enabled = true                        # default false
addr = "0.0.0.0:7335"                 # отдельный listener
allowed_origins = ["https://admin.example.com"]   # для CORS preflight (не auth)
```

#### 4.3. Admin UI not used → endpoint disabled

Если admin UI не нужен — `enabled = false`, `/admin/*` возвращает 404.

---

### 5. Security Headers (mandatory для admin UI)

`GET /admin/*`:

```
Strict-Transport-Security: max-age=31536000; includeSubDomains
Content-Security-Policy: default-src 'self';
                         connect-src 'self' wss://<same-host>;
                         script-src 'self';
                         style-src 'self';
                         img-src 'self' data:;
                         frame-ancestors 'none';
                         form-action 'none';
                         base-uri 'self'
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
Referrer-Policy: no-referrer
Permissions-Policy: geolocation=(), microphone=(), camera=()
Cache-Control: no-store, no-cache, must-revalidate, max-age=0
```

`script-src 'self'` БЕЗ `'wasm-unsafe-eval'`. WASM загружается через `instantiateStreaming(fetch('argon2.wasm'))` который **не** требует `wasm-unsafe-eval` (это только для `WebAssembly.compile()` от inline source).

---

### 6. CORS

6.1. Admin UI fetches только same-origin → CORS не нужен.

6.2. REST `/shamir/v1/*`:
- Default: same-origin only (нет CORS headers)
- Server config может разрешить ограниченные origins (см. §4.2)
- При configured origins — preflight для них только
- `Access-Control-Allow-Credentials: false` (используем Bearer не cookies)

---

### 7. TLS

7.1. **HTTPS only** для production. Plain HTTP — только loopback.

7.2. TLS config — идентичен TRANSPORT_TCP §3 (TLS 1.3, no 0-RTT).

7.3. Browser обычно требует валидный CA cert. Решения:
- Self-signed с manual trust (dev / internal)
- Public CA cert (Let's Encrypt) — **не** меняет identity model: pin всё равно Ed25519 server key

---

### 8. Rate Limits

- `/shamir/v1/health`, `/shamir/v1/version`: 100/sec per IP
- `/admin/*`: 100/sec per IP (static delivery, не security-критично)
- `/shamir/v1/query`: per-session rate limit (server config)
- `/shamir/v1/admin/*`: 10/sec per session

---

### 9. Test Checklist

- GET /admin/ → CSP, HSTS headers presence
- GET /admin/static/argon2.wasm → Content-Type: application/wasm
- POST /shamir/v1/query без Bearer → 401
- POST /shamir/v1/query с invalid Bearer → 401
- POST /shamir/v1/query с expired session_id → 401, `session_expired`
- CSP violations при попытке inline script — browser blocks
- CORS preflight для allowed origin → headers present; для disallowed → no CORS headers
- WASM load via instantiateStreaming работает без `wasm-unsafe-eval`
- Subresource Integrity на bundle script tag

---

## Part 8: CLIENT_BROWSER

> Source file: `spec/CLIENT_BROWSER.md`

## Browser Client SDK

Гайд для имплементации SCRAM-Argon2id auth в browser. Цель: **браузер не уступает по безопасности native клиенту** в той мере, в какой это позволяет WebCrypto API.

---

### 1. Crypto Stack

WebCrypto не имеет Argon2id и Ed25519. Минимум полифилов.

| Примитив | Имплементация | Размер |
|---|---|---|
| Argon2id | `argon2-browser` (WASM) | ~30 KB |
| Ed25519 verify | `@noble/ed25519` v2+ (RFC 8032 strict) | ~5 KB |
| HMAC-SHA256 | WebCrypto `crypto.subtle.sign({name:"HMAC", hash:"SHA-256"})` | native |
| HKDF-SHA256 | WebCrypto `crypto.subtle.deriveBits({name:"HKDF"})` | native |
| SHA-256 | WebCrypto `crypto.subtle.digest("SHA-256")` | native |
| CSPRNG | WebCrypto `crypto.getRandomValues()` | native |
| ConstantTimeEq | `@noble/ciphers/utils` `equalBytes` или custom | ~20 строк |
| msgpack | `@msgpack/msgpack` | ~10 KB |
| UTF-8 NFC | `String.prototype.normalize("NFC")` | native |

**Total bundle overhead:** ~50 KB minified (35 WASM + 5 Ed25519 + 10 msgpack + ~5 SCRAM logic).

---

### 2. Зависимости (npm)

```json
{
  "dependencies": {
    "argon2-browser": "^1.18.0",
    "@noble/ed25519": "^2.1.0",
    "@noble/ciphers": "^0.5.0",
    "@msgpack/msgpack": "^3.0.0",
    "@scure/base": "^1.1.0"
  }
}
```

`@scure/base` — base64url RFC 4648 §5.

Все библиотеки — без native deps. Browser, Node, Deno, Bun.

---

### 3. Critical Implementation Details

#### 3.1. Constant-time comparison

```javascript
import { equalBytes } from '@noble/ciphers/utils';

if (!equalBytes(server_signature, expected_signature)) {
  throw new Error('server_authentication_failed');
}
```

`equalBytes` гарантирует branch-free сравнение.

#### 3.2. Ed25519 strict verify

`@noble/ed25519` v2 default — RFC 8032 strict (small-subgroup rejection):

```javascript
import * as ed from '@noble/ed25519';

const ok = await ed.verifyAsync(signature, message, publicKey);
if (!ok) throw new Error('server_signature_invalid');
```

⚠️ Lock major version: `"@noble/ed25519": "^2"`. v1 имела разные defaults.

#### 3.3. Argon2id — единые параметры

Используются **те же параметры** что выдал сервер в `challenge` (без adaptive variants по UA — удалено в v1).

```javascript
import argon2 from 'argon2-browser';

const result = await argon2.hash({
  pass: password,                       // string
  salt: challenge.salt,                 // Uint8Array(16)
  time: challenge.time,
  mem: challenge.memory_kb,
  parallelism: challenge.parallelism,
  type: argon2.ArgonType.Argon2id,
  hashLen: 32
});
const saltedPassword = result.hash;
```

Default params (см. AUTH_PROTOCOL §3.7): `memory_kb=131072 (128 MB), time=4, parallelism=1`.

**128 MB на mobile** — может быть проблема на старых устройствах с limited RAM (Android ≤ 4GB). На 2026 это редкость, но возможно. Honest 2-3 секунды UI freeze (см. §3.4) лучше чем downgrade params (раскрывает client type).

#### 3.4. UI freeze — Web Worker mandatory

Argon2id блокирует main thread на ~2-3 секунды. **Обязательно** в Web Worker:

```javascript
// auth-worker.js
import argon2 from 'argon2-browser';

self.onmessage = async (e) => {
  const { password, salt, params } = e.data;
  try {
    const result = await argon2.hash({
      pass: password,
      salt,
      time: params.time,
      mem: params.memory_kb,
      parallelism: params.parallelism,
      type: argon2.ArgonType.Argon2id,
      hashLen: 32
    });
    self.postMessage({ ok: true, salted: result.hash });
  } catch (err) {
    self.postMessage({ ok: false, error: err.message });
  }
};

// main.js
const worker = new Worker(new URL('./auth-worker.js', import.meta.url));
worker.postMessage({ password, salt, params });
worker.onmessage = (e) => { /* salted_password ready */ };
```

**UI feedback** — спиннер с явным "Computing crypto…". 2 секунды — это feature (memory-hard), не bug.

**Worker cancel** — terminate если пользователь закрыл modal до завершения.

#### 3.5. Zeroize

JS не даёт гарантий памяти. Best-effort:
- Хранить sensitive в `Uint8Array`, после использования `arr.fill(0)`
- Не передавать пароль как `String` в долгоживущие переменные (immutable, остаются в string pool)
- Custom error types которые не serialize secrets (`toJSON` возвращает `<REDACTED>`)
- Никогда `console.log(password)` или подобное в коде

---

### 4. Channel Binding (browser-mode)

WebCrypto **не предоставляет** TLS exporter. Любой WS-клиент в браузере не может вычислить `EXPORTER-ShamirDB-AUTH-v1`.

#### 4.1. Resolution

Browser клиент шлёт `auth_init.binding_mode = 0x02` ("tls_no_export"). Сервер на browser endpoint (`/shamir/v1/browser`, см. TRANSPORT_WS §1) принимает это значение. На native endpoint — отказ.

В `auth_message`:
```
binding_mode = 0x02
tls_exporter_or_zeros = bytes(32) zeros
```

Это **явный** policy signal — не UA-detection. MITM не может switch'нуть browser клиента в стronger режим (нет API) или ослабить native клиента (server policy на endpoint).

#### 4.2. Trade-off — honest assessment

Browser теряет UKS защиту через TLS exporter. **Mitigations:**

- **Strict Origin matching** на сервере (TRANSPORT_WS §9)
- **HSTS** (`Strict-Transport-Security: max-age=31536000; includeSubDomains`)
- **Out-of-band Ed25519 pinning embedded в production bundle.** TOFU — **strictly dev-only**.
- **CSP `connect-src 'self'`** — JS не может connect никуда кроме origin сервера
- **Subresource Integrity** на bundle (`<script integrity="sha384-...">`)

**Что embedded pin реально защищает (узкий случай):**

Embedded pin защищает **только** от: server identity (Ed25519 priv) compromised **БЕЗ** соответствующего bundle redeploy И клиент имеет cached bundle от предыдущей версии. После rotation атакующий с stolen priv пытается impersonate server — cached bundle с старым pin отвергает соединение.

**Embedded pin НЕ защищает** от любого attacker, способного контролировать bundle delivery:

- **Compromised origin / CDN / malicious deploy** — атакующий просто меняет embedded pin в новом bundle
- **TLS MITM** (corporate proxy с installed root CA, DNS hijack + rogue cert) — атакующий перехватывает `GET /admin/static/main.<hash>.js`, отдаёт свой bundle с своим pin (включая правильный SRI hash для своего bundle)
- **Browser cache poisoning** через MITM — same vector

**Реальная модель безопасности browser path:**
- TLS поверх browser встроенной CA chain — security floor
- + HSTS preload — против downgrade на http
- + CSP — против XSS escalation
- + memory-only secrets — против persistent compromise
- = **примерно как любая web admin panel.** SCRAM защищает credential exchange (пароль не уходит в plaintext даже при MITM), но **session_id может быть hijacked** через relay attack.

**Recommendation:** для high-stakes admin operations (database management, user CRUD, identity rotation, key rotation) — **native CLI с out-of-band pin** обязателен. Browser admin UI приемлем для read-only monitoring / low-stakes config.

Документировано как known limitation в SECURITY_MODEL §4.9 + §2 threat coverage table.

#### 4.3. Anti-downgrade в resumption

Browser sessions создают tickets с `binding_mode_at_auth = 0x02`. По SESSION_RESUMPTION §6.1:
- Browser ticket → native endpoint = upgrade ALLOWED
- Native (TLS exporter) ticket → browser endpoint = downgrade BLOCKED

То есть украденный browser ticket НЕ может быть escalated в native session.

---

### 5. CSP and XSS Defense

#### 5.1. CSP (server-side, см. ADMIN_UI_HOSTING §5)

Strict CSP без `unsafe-inline`, БЕЗ `wasm-unsafe-eval`. WASM грузится через `instantiateStreaming(fetch(...))`.

#### 5.2. JS Storage

**НИКОГДА не использовать:**
- `localStorage` / `sessionStorage` — для **любых** secrets (session_id, ticket, password, ключи)
- `IndexedDB` без encryption (too complex для v1)
- Cookies (см. ADMIN_UI_HOSTING §3)

**Использовать:**
- Memory-only state в JS variable closure-scope админ приложения
- Resumption ticket — **только в memory** (см. SESSION_RESUMPTION §11). Tab close = re-auth (~2с Argon2id, acceptable).

⚠️ **Изменение от предыдущей версии spec:** ticket НЕ хранится в sessionStorage. XSS = leak ticket = takeover был неприемлемый trade-off. Теперь tab close = full re-auth.

#### 5.3. Bundle integrity

- Bundle path содержит SHA256 hash для cache busting + integrity
- Server отдаёт с правильным `Content-Type`
- WASM с `Content-Type: application/wasm`
- Subresource Integrity на `<script>` и `<link>`

#### 5.4. Input sanitization

Все user-supplied data (включая username отображаемый в UI) — escape через template literal-style (React/Vue/Svelte автоматом). **Не использовать `innerHTML` / `dangerouslySetInnerHTML`**.

---

### 6. Connection Code (Pseudo-API)

```javascript
import { ShamirClient } from '@shamir-db/browser';

const client = new ShamirClient({
  url: 'wss://db.example.com/shamir/v1/browser',
  serverPin: 'base64url(SHA256(server_pub))',  // out-of-band, embedded in bundle
  // OR acceptNewHost: true                     // TOFU (dev only)
});

await client.connect();
await client.auth({ user: 'alice', password: '...' });
const result = await client.query({ ... });
await client.logout();
```

SDK responsibilities: Worker для Argon2id, lifecycle, auto-resume в той же вкладке (ticket в memory), error mapping, zeroize на `beforeunload`.

---

### 7. Browser Compatibility

| Feature | Min Browser |
|---|---|
| WebCrypto HMAC-SHA256, HKDF | All modern |
| WebSocket | All modern |
| Web Workers | All modern |
| WASM (`instantiateStreaming`) | Chrome 61+, Firefox 58+, Safari 15+, Edge 79+ |
| `String.normalize("NFC")` | All modern |
| `crypto.getRandomValues` | All modern |

**Minimum:** Chrome ≥ 61, Firefox ≥ 58, Safari ≥ 15, Edge ≥ 79. **No IE.**

---

### 8. Mobile

8.1. **Battery / CPU:** Argon2id 2 секунды на mobile = заметно, но acceptable. **Один global default**, не adaptive (см. §3.3 — anti-fingerprinting trade-off).

8.2. **NAT rebinding:** мобильные сессии могут менять IP. Resumption tickets решают это **в пределах одной вкладки** (memory-only).

8.3. **WebView (in-app browsers):** ограничения. WASM работает, performance variable.

8.4. **PWA:** admin UI можно установить как PWA (manifest.json). Не меняет security model.

---

### 9. Test Checklist (release blockers)

- Headless browser (Playwright) полный auth round-trip
- Argon2id в Web Worker без UI freeze
- WS reconnect через resumption ticket в same tab
- New tab = full re-auth (ticket не persist)
- TOFU first-connect saves pin, mismatch fails
- CSP violations blocked (inline script, eval, external resource)
- SRI verification на bundle
- WASM via instantiateStreaming (без wasm-unsafe-eval)
- XSS attempt в username display (sanitization works)
- Ed25519 strict verify rejects malleable signatures (test vector)
- Resumption: browser→browser ok; browser→native (upgrade) ok; native→browser (downgrade) blocked
- Audit log content (если admin watches metrics) показывает `transport: "ws"` для browser
- `beforeunload` zeroize ticket + session_id

---

### 10. Future (см. ROADMAP.md)

- WebAuthn second factor для admin
- WebTransport API когда TLS exporter станет доступен в browser
- Service Worker для offline UI shell
- Push notifications для session events

---

## Part 9: IMPLEMENTATION_GUIDE

> Source file: `spec/IMPLEMENTATION_GUIDE.md`

## ShamirDB Implementation Guide

Operational details для имплементаторов сервера и клиентов.

**Mixed normativity.** Sections marked **[NORMATIVE]** — обязательны для security claims из SECURITY_MODEL.md. Альтернативная имплементация может расходиться в **non-normative** разделах без потери wire-compatibility, но ДОЛЖНА соблюдать [NORMATIVE].

---

### 1. SystemStore Layout

#### 1.1. `__system__/users/{user_id}` — см. AUTH_PROTOCOL §3.5.

#### 1.2. `__system__/server_meta` [NORMATIVE schema]

```
{
  // Anti-enumeration
  server_secret: bytes(32),                       // ротируется (§5.1)
  server_secret_previous: Option<bytes(32)>,
  server_secret_rotated_at_ns: u64,
  
  // Lockout state derivation key — отдельный от server_secret
  lockout_secret: bytes(32),                      // НЕ ротируется (lockout state survives)
  
  // Server identity
  server_ed25519_priv: bytes(32),
  server_ed25519_pub: bytes(32),
  server_ed25519_priv_previous: Option<bytes(32)>,    // zeroized после rotation_until_ns
  server_ed25519_pub_previous: Option<bytes(32)>,
  server_ed25519_rotation_until_ns: Option<u64>,      // overlap window end (HIGH-5: блокирует повторный rotateServerIdentity)
  
  // Resumption
  ticket_key: bytes(32),
  ticket_key_previous: Option<bytes(32)>,
  ticket_key_rotated_at_ns: u64,
  
  // Audit log integrity
  audit_chain_key: bytes(32),                     // HMAC key для chained log (см. §3.3)
  audit_chain_key_previous: Option<bytes(32)>,    // overlap при ротации
  audit_chain_key_rotated_at_ns: u64,
  
  // Audit truncation defence (см. §3.3)
  last_audit_hmac: bytes(32),                     // checkpoint предыдущей записи
  last_audit_seq: u64,                            // монотонная sequence
  last_audit_checkpoint_at_ns: u64,
  
  // Bootstrap state
  bootstrap_token_hash: Option<bytes(32)>,
  bootstrap_token_expires_at_ns: Option<u64>,
  superuser_ever_existed: bool,
  
  created_at_ns: u64
}
```

**Все timestamps unix nanoseconds** (см. AUTH §15.5). NTP-disciplined источник обязателен.

POSIX: `chmod 600`, owned by server user. Windows: ACL только owner SID. Backup encrypted-at-rest **отдельно** от users data (например, `age` с recipient key оператора).

#### 1.3. In-memory state [NORMATIVE]

##### `auth_failures: DashMap<(subnet, username_hash), FailureState>`
```
struct FailureState {
    fail_count: u32,
    last_fail: Instant,
    next_allowed: Instant,
}
```
- subnet = `/24 IPv4` или `/64 IPv6`
- `username_hash = HMAC-SHA256(lockout_secret, username_nfc)[..16]` — anti-enumeration через **отдельный** `lockout_secret` (НЕ ротируется, см. §1.2)
- Background GC каждые 60s: удалить entries с `now - last_fail > BACKOFF_RESET`
- Hard cap `MAX_AUTH_FAILURES_ENTRIES = 1M`, LRU eviction
- **[NORMATIVE] Reset on success**: при успешной auth соответствующая `(subnet, username_hash)` entry **немедленно удаляется** из `auth_failures` И `lockout_state` (если pre-threshold). Иначе legitimate user видит persistent backoff после typo. См. AUTH §5.2.5.

##### `lockout_state: DashMap<(subnet, username_hash), LockoutState>` [NORMATIVE]
```
struct LockoutState {
    locked_until: u64,
    fail_count_in_window: u32,
}
```
**Persistent в SystemStore** с **батчингом** (NORMATIVE):
- RECOMMENDED flush interval: 5 секунд
- MUST: ≤ 60 секунд
- Flush немедленно при достижении `LOCKOUT_THRESHOLD`
- **Graceful shutdown (SIGTERM/SIGINT) MUST synchronously flush** перед exit
- Crash (SIGKILL/power loss) → до 5с rollback acceptable (sub-threshold counters); pre-threshold counters могут быть потеряны, но threshold-crossing flush защищает от обхода через restart

##### `handshake_states: DashMap<connection_id, HandshakeState>`
```
struct HandshakeState {
    username_nfc: String,
    client_nonce: [u8; 32],
    server_nonce: [u8; 32],
    salt: [u8; 16],
    is_real_user: bool,
    stored_key_or_fake: [u8; 32],
    server_key_or_fake: [u8; 32],
    transport_kind: u8,
    binding_mode: u8,
    channel_binding: [u8; 32],
    started_at: Instant,
}
```
GC каждые 5s: drop без proof в течение 10s.

##### `consumed_counters: DashMap<(user_id, ticket_family_id), u64>` [NORMATIVE]
last_consumed `family_counter` per ticket lineage (см. SESSION_RESUMPTION §6.2).

**SYNCHRONOUS DURABLE persist** перед `resume_ok` reply — **не** batched. "Durable" =:
- POSIX `fsync` после write
- SQLite backend: `PRAGMA synchronous=FULL` + WAL + checkpoint OR `PRAGMA synchronous=EXTRA`
- sled/redb/fjall: equivalent flush-and-sync
- ext4: mount option `barrier=1` (default modern, проверять)
- xfs/btrfs: equivalent write barriers
- **Power-fail testing required** для valid implementation (release blocker per IMPLEMENTATION_GUIDE §11)

См. SESSION_RESUMPTION §10.2 для motivation. **Not batched** потому что batched flush откатывает counter при crash → ticket replay window.

##### `bootstrap_token_files: DashMap<path, expires_at_ns>`
Tracking outstanding bootstrap token files для GC (см. §1.4).

##### `sessions: DashMap<session_id, Arc<Session>>`
Session state — AUTH_PROTOCOL §7.2. Не персистентны. GC каждую минуту.

#### 1.4. Restart warmup [NORMATIVE]

В первые 60 секунд после старта server applies глобальный rate limit `RATE_LIMIT_AUTH_INIT_PER_SUBNET / 4 = 2.5/sec` пока in-memory state warmup'ится из persisted snapshots. Закрывает restart-replay window для distributed attackers.

#### 1.5. Memory quotas [NORMATIVE]

Помимо `PER_SESSION_MEM = 64 MB` (AUTH §7.4):

- `MAX_TOTAL_SESSION_MEM_PER_SUBNET = 256 MB` — глобальный cap на (input + output буферы + parsed query state) для всех sessions от одного subnet (/24 IPv4, /64 IPv6). Превышение → новые sessions из subnet rejected с `server_busy`.
- Backpressure policy: при достижении 75% от cap — server отклоняет non-critical operations (admin commands proceed) и шлёт `Retry-After` header.
- `MAX_CONCURRENT_ARGON2` рекомендуется derive from RAM: `floor(available_ram_mb / (kdf.memory_kb / 1024 × 2.5))`. Hard cap 64. Защищает от OOM при memory_kb=128, server с 4GB RAM.

#### 1.6. known_hosts.bin fallback encryption (NORMATIVE для headless deploy)

Когда OS keychain недоступен (headless server, embedded), `local_key.bin` fallback (см. §7) хранится в `~/.shamir/local_key.bin` chmod 600. **Encryption обязательна:**

- Default: encrypted с пользовательской passphrase через `scrypt` (N=2^17, r=8, p=1) → AES-256-GCM. Passphrase запрашивается интерактивно при первом запуске; хранится в memory keyring/agent для последующих.
- Alternative: `--local-key-file <path>` injected via env var or external secrets manager (Vault, Doppler).
- Plain `local_key.bin` (без encryption) — **только** с `--insecure-local-key` flag и audit warning при каждом запуске.

---

### 2. Server Configuration

#### 2.1. Listeners [NORMATIVE listener_policy_mapping]

```toml
[server]
data_dir = "./data"

## Bootstrap token output — варианты, выбрать один
bootstrap_token_output = "tty"          # tty | file:<path> | command:<cmd>
bootstrap_token_ttl_ns = 3_600_000_000_000     # default 1 час (60·60·1e9 nanos), min 300s=3e11, max 24h=8.64e13

## Argon2id defaults (must satisfy floor §3.7.2 AUTH)
[kdf]
memory_kb = 131072
time = 4
parallelism = 1
## MAX_CONCURRENT_ARGON2 — RECOMMENDED derive from RAM:
##   floor(available_ram_mb / (memory_kb / 1024 * 2.5))
## Hard cap 64 (защита от runaway).
## Без autodetect — fixed:
max_concurrent_argon2 = 32              # для server с 8 GB RAM

## Strict mode hardening (defaults для backward compat, для prod рекомендуется true)
[strict]
allow_browser_ticket_upgrade = true     # false = browser ticket НЕ может быть resumed в native
disable_tofu_in_production = false      # true = клиенты MUST использовать out-of-band pin

## Memory budgets [NORMATIVE]
[limits]
per_session_mem_mb = 64
max_total_session_mem_per_subnet_mb = 256
max_connections_per_ip = 100

## Resumption behavior
[resumption]
disable_plain_ticket_upgrade = false    # true = plain ticket НЕ может resume в TLS transport
                                         # (мнемоника: plain → stronger transport blocked)
                                         # false (default) = embedded handoff OK

## Listeners — каждый со своим binding_mode policy
[[listener]]
addr = "0.0.0.0:7331"
transport = "tcp"
profile = "tls"                         # → binding_mode = 0x01 ENFORCED

[[listener]]
addr = "0.0.0.0:7332"
transport = "ws"
profile = "tls"                         # → binding_mode = 0x01 ENFORCED, endpoint /shamir/v1

[[listener]]
addr = "0.0.0.0:7333"
transport = "ws"
profile = "tls_browser"                 # → binding_mode = 0x02 ENFORCED, endpoint /shamir/v1/browser

[[listener]]
addr = "127.0.0.1:7334"                 # MUST be loopback (см. §2.2)
transport = "tcp"
profile = "plain"                       # → binding_mode = 0x00 ENFORCED
allow_plain = true                      # explicit opt-in mandatory

[admin_ui]
enabled = true
addr = "0.0.0.0:7335"
allowed_origins = ["https://admin.example.com"]
```

##### Bootstrap token output options

- `tty` (default): печать в stdout **только** если `isatty(stdout)` AND процесс не systemd-managed (проверка `INVOCATION_ID` env var). Иначе server fails с инструкцией.
- `file:<path>`: атомарно создать `chmod 600` файл. **Strongly recommend tmpfs/ramdisk path** (`/run/shamir/bootstrap.token` или `/dev/shm/...`) — обычные filesystem пути попадают в backup/AV/EDR/cloud sync ecosystem. Server **MUST** удалить после use или TTL.
- `command:<cmd>`: pipe token в external command (e.g., `pass insert shamir/bootstrap`, `age -r recipient -e -o /vault/token.age`, `gpg -e -r ops@example.com`). Token не касается обычного диска.

**WARNING:** systemd / journald / docker logs / k8s log shippers **захватывают stdout** даже при `isatty` check (через TTY emulation). Пред-deployment проверять `journalctl -u shamirdb` после bootstrap — токен не должен присутствовать.

**MUST:** profile→binding_mode mapping enforced server-side. Server rejects auth_init с `binding_mode` не в listener policy **до** Argon2id (DoS-amp защита; см. AUTH §4.3).

#### 2.2. Plain TCP listener constraints [NORMATIVE]

Server при старте **проверяет** и fails если:
- `profile = "plain"` AND `allow_plain != true`
- `profile = "plain"` AND `addr` НЕ в whitelist:
  - IPv4: `127.0.0.0/8`
  - IPv6: `::1` (single address, не subnet)
  - Unix domain socket (path-based)
- `profile = "plain"` AND `0.0.0.0` или `::` (любой "any-bind") → **NEVER allowed**
- `addr` resolves to multiple addresses вне whitelist

**Bootstrap on plain loopback не поддерживается в v1.** Bootstrap (§11 AUTH) требует `binding_mode == 0x01` (TLS exporter). Embedded deployment с plain TCP loopback должен:
- (a) Поднять TLS listener временно для первого bootstrap, потом switch к plain, ИЛИ
- (b) Использовать `--regen-bootstrap` flag через CLI с предзаготовленными credentials, ИЛИ
- (c) Pre-provision admin user через CLI tool (не через wire protocol)

Operational note: pure-plain embedded deployments — это explicit limitation v1. v1.1+ может добавить unix-socket-based bootstrap с file-permission-based authority.

#### 2.3. Browser-only deployment warning

Если `[admin_ui].enabled = true` AND нет ни одного `[[listener]]` с `profile = "tls"` (native) — server warning при старте: "browser endpoint без native peer — admin клиенты forced на binding_mode=0x02 (ослабленный режим)". Не блокер, но привлекает внимание.

---

### 3. Audit Log [NORMATIVE]

#### 3.1. Format

Append-only structured log в `__system__/audit_log` (или structured tracing с file backend).

JSON-line (одна запись на строку, no inner whitespace в production):
```json
{"seq":42,"ts_ns":1717000000000000000,"event":"auth_success","transport":"tcp","user":"alice","ip_subnet":"192.0.2.0/24","session_id_prefix":"a1b2c3d4","result":"ok","details":{},"prev_hmac":"base64url(32)","hmac":"base64url(32)"}
```

**Поля:**
- `seq: u64` — монотонная sequence (защита от truncation, см. §3.3)
- `ts_ns: u64` — unix nanos
- `event: String` — event type (см. §3.2)
- `transport: String` — обязательно для всех events
- `user`, `ip_subnet`, `session_id_prefix`, `result`, `details` — context
- `prev_hmac: base64url(32)` — chain link
- `hmac: base64url(32)` — entry integrity

`ip_subnet` = текущего connection.

#### 3.2. Минимум v1 events

**Auth lifecycle:** `auth_success` (sampled), `auth_failed` (rate-limited 1/мин per (subnet, user)), `auth_aborted` (sampled).

**Bootstrap:** `bootstrap_used`, `bootstrap_regen`, `bootstrap_token_file_orphan_cleaned`.

**Lockout:** `lockout_triggered`, `lockout_released`, `revoke_all_lockouts`.

**User management:** `user_created`, `user_deleted`, `roles_changed`, `password_changed`, `kdf_params_upgraded`.

**Sessions:** `kick_session`, `session_evicted{reason}`.

**Resumption:** `resumption_used` (sampled), `resumption_replay_detected` (always), `resumption_downgrade_blocked` (always).

**Rotation:** `rotate_server_identity`, `rotate_server_secret`, `rotate_ticket_key`, `rotate_audit_chain_key`.

**Audit integrity:** `audit_chain_verify_failed` (startup truncation/tamper detection).

**Revocation:** `revoke_user_tickets`, `revoke_all_tickets`.

#### 3.3. HMAC chaining + truncation defence (NORMATIVE v1)

##### Chain construction

```
hmac = HMAC-SHA256(audit_chain_key, 
                   prev_hmac || canonical(entry_without_hmac))
```

`audit_chain_key` хранится в `server_meta` (§1.2). Первая запись использует `prev_hmac = bytes(32) zeros`, `seq = 1`. Каждая последующая — `prev_hmac` предыдущей записи, `seq = prev_seq + 1`.

**`canonical(entry_without_hmac)`** — детерминистическая байт-сериализация полей (без `hmac`):
```
canonical = u64_be(seq)
         || u64_be(ts_ns)
         || u16_be(byte_len(event)) || event_utf8
         || u8(byte_len(transport))  || transport_utf8
         || u8(byte_len(user))       || user_utf8        // empty string если null
         || u8(byte_len(ip_subnet))  || ip_subnet_utf8
         || bytes(8) session_id_prefix                    // zeros если null
         || u8(byte_len(result))     || result_utf8
         || u32_be(byte_len(details_msgpack)) || details_canonical_msgpack
         || prev_hmac(32)
```

`details_canonical_msgpack` — msgpack с lex-sorted map keys, smallest int encoding (как ticket_plain в SESSION_RESUMPTION §2.1). Гарантирует bit-exact reproducibility между имплементациями.

##### Tamper detection

- Per-entry: `hmac` не сходится → entry corrupted/forged
- Chain: `prev_hmac` поля цепочки → разрыв = compromised в этой точке

##### Truncation defence

Атакующий offline удалил последние N entries. Без checkpoint защиты — недетектируемо.

**Защита (NORMATIVE):**
- Каждые 60 секунд OR каждые 1000 entries (whichever first) server атомарно persist'ит в `server_meta` поля `last_audit_hmac`, `last_audit_seq`, `last_audit_checkpoint_at`.
- Также — на graceful shutdown (SIGTERM/SIGINT).
- На startup server verify: парсит audit_log хвост → находит самую последнюю запись → если её `seq < last_audit_seq` ИЛИ её `hmac != last_audit_hmac` → **alert** (`shamir_audit_chain_verify_failures_total` increments + log warning + opt-in operator интервенция).

##### Key rotation

`rotateAuditChainKey` admin command:
1. `audit_chain_key_previous = current; current = random(32); audit_chain_key_rotated_at = now`
2. Через 30 дней background task: `audit_chain_key_previous = NULL`
3. Verify legacy entries (issued под previous): tooling пробует current, потом previous
4. Audit event `rotate_audit_chain_key`

##### Verify tooling

Out-of-band binary `shamir-audit-verify` читает `__system__/server_meta` (`audit_chain_key{,_previous}`, `last_audit_*`) + `audit_log` файл, replays HMAC chain entry-by-entry, reports first mismatch и truncation status.

---

### 4. Log Redaction Policy [NORMATIVE]

#### 4.1. Запретный список — НИКОГДА в log/tracing/error messages

- `password` (любой формы)
- `salted_password`, `client_key`, `server_key` (full)
- `stored_key` (full — допустим `prefix(4 bytes hex)` для correlation)
- `server_secret`, `lockout_secret`, `server_ed25519_priv`, `ticket_key`, `audit_chain_key`, `audit_chain_key_previous`
- `session_id` (full — допустим `prefix(8 bytes hex)`)
- `client_proof`, `server_signature`, `identity_sig`
- `client_nonce`, `server_nonce`
- `bootstrap_token`
- `ticket` plain или wire format

#### 4.2. Allow-list

- Username (NFC) — subject to GDPR/PII per jurisdiction
- IP subnet (/24 или /64)
- Timestamp
- Error code
- Session prefix (8 hex)
- Stored_key prefix (4 hex)

#### 4.3. CI test (mandatory) [NORMATIVE]

```rust
#[test]
fn test_no_secret_leak_in_logs() {
    let password = "uniquetestpass1234567890";
    let captured = capture_tracing(|| run_full_auth(password));
    assert!(!captured.contains("uniquetestpass"));
    // повторить для всех secrets из 4.1 с magic patterns
}
```

#### 4.4. Type-level enforcement (Rust)

Все типы containing secrets → custom `Debug` → `<REDACTED>`. Marker trait `Sensitive`. `Display` запрещён.

---

### 5. Compromise Recovery — Detailed Runbooks

#### 5.1. server_secret leaked

1. Generate new: `new_secret = random(32)`
2. Atomic update: `server_secret_previous = current; server_secret = new; server_secret_rotated_at = now`
3. Через 7 дней background task: `server_secret_previous = NULL`
4. Audit event `rotate_server_secret`

#### 5.2. server_ed25519_priv leaked (TIME-CRITICAL)

1. **Kill switch:** server config `--identity-revoked` → server отвечает только generic auth failures. Active sessions terminate.
2. Generate new keypair.
3. Set `previous = current; current = new; rotation_until = now + 7 days`.
4. Restart server (`--identity-revoked` → off).
5. Через активные сессии broadcast `identity_rotation` (см. AUTH §12.2).
6. **Force out-of-band re-pin** для всех клиентов (announcement через email / pager).
7. Через 7 дней `previous = NULL`.

#### 5.3. lockout_secret leaked

`lockout_secret` ротация **не делается обычно** (lockout state ключи зависят от него, ротация = orphan state). При compromise:
1. `revokeAllLockouts` admin command (clears in-memory + persisted lockout state)
2. Generate new `lockout_secret`
3. Restart (новые fail counts с новыми ключами)

Trade-off: атакующий получает clean slate (50 attempts заново). Acceptable если лучше чем продолжать с compromised secret.

#### 5.4. DB users snapshot leaked

1. Force re-auth всем: `revokeAllUserSessions` + `revokeAllTickets` (закрывает existing sessions, форсит SCRAM на всех)
2. Out-of-band notification всем: смените пароль (через self-service §12.5 AUTH)
3. Audit для potential data exfil
4. Rate-limit increased temporarily
5. Не trigger полного teardown (Argon2id offline brute = годы для 12+ char паролей)

(`expire_password_now` flag в `updateUser` запланирован v1.1 — требует поля `password_must_change` в user record + handling в auth_ok flow. Сейчас mass session revocation + user notification.)

#### 5.5. Full SystemStore compromised — полный teardown

1. Stop server
2. Generate fresh `server_meta`: новые secrets, ed25519, ticket_key, audit_chain_key, `superuser_ever_existed = false`
3. Backup users (если хочется сохранить usernames + stored_keys для force change); опционально wipe
4. Restart → bootstrap re-enable
5. Out-of-band announcement
6. Audit forensics

#### 5.6. Lost admin password

`shamir-server --regen-bootstrap --confirm` (требует stop сервера + физический доступ + флаг + stdin confirmation phrase):

1. Generate новый `bootstrap_token`, `superuser_ever_existed` остаётся `true`
2. Output token per `bootstrap_token_output` config
3. Audit event `bootstrap_regen`
4. Restart → operator делает re-bootstrap для нового admin
5. Operator manually удаляет старого locked-out admin (если есть)

#### 5.7. Backup restore (counter rollback prevention) — MANDATORY

При restore SystemStore из backup `consumed_counters` могут откатываться → ticket replay window. **Mandatory recovery step:**

1. Restore SystemStore из backup
2. **Перед** start сервера: `shamir-server --revoke-all-tickets-on-start` flag
3. Server при старте инвалидирует `ticket_key` (rotates без overlap, `ticket_key_previous = NULL`)
4. Audit event `revoke_all_tickets{reason="backup_restore"}` записан
5. Все клиенты делают full re-auth (~2с Argon2id, acceptable для recovery scenario)
6. После — нормальная работа

Документировать в operations runbook: **"Любой backup restore SystemStore = mandatory revokeAllTickets"**.

Аналогично — при disk corruption suspected, replication failover, OS reinstall с restored data dir.

---

### 6. Observability — Metrics

Prometheus-style, exposed на `/admin/metrics` с `Authorization: Bearer <admin_session>`.

#### 6.1. Counters

- `shamir_auth_init_total{result, transport}`
- `shamir_auth_complete_total{result, transport}`
- `shamir_lockouts_total`
- `shamir_bootstrap_attempts_total{result}`
- `shamir_argon2id_total`
- `shamir_session_created_total{transport}`
- `shamir_session_evicted_total{reason, transport}` (logout|idle|max_age|kicked|max_sessions_lru|disconnect)
  - **Note:** `disconnect` reason — после 5s grace окна без resumption (AUTH §7.7)
- `shamir_resumption_used_total{result}` (ok|expired|invalid|downgrade_blocked|replay)
- `shamir_frame_oversized_total{transport}`
- `shamir_admin_command_total{command, result}`
- `shamir_audit_chain_verify_failures_total`

#### 6.2. Histograms

- `shamir_auth_duration_seconds{transport}`
- `shamir_argon2id_duration_seconds`
- `shamir_handshake_state_lifetime_seconds`
- `shamir_resumption_fsync_latency_seconds` — alert если p99 > 100ms (recommend NVMe или WAL group-commit)
- `shamir_audit_log_append_latency_seconds`

#### 6.3. Gauges

- `shamir_active_sessions{transport}`
- `shamir_inflight_handshakes`
- `shamir_argon2id_semaphore_available`
- `shamir_auth_failures_tracked_keys`

#### 6.4. Suggested Alerts

- Lockout rate spike: `rate(shamir_lockouts_total[5m]) > baseline × 3`
- Server busy: `shamir_inflight_handshakes / MAX_CONCURRENT_ARGON2 > 0.8`
- Identity rotation: `increase(shamir_admin_command_total{command="rotateServerIdentity"}[1h]) > 0`
- Bootstrap usage anomaly: `shamir_bootstrap_attempts_total > 1`
- Resumption downgrade: `rate(shamir_resumption_used_total{result="downgrade_blocked"}[5m]) > 0`
- Audit chain corruption: `shamir_audit_chain_verify_failures_total > 0`
- Resumption fsync slow: `histogram_quantile(0.99, shamir_resumption_fsync_latency_seconds) > 0.1` — рекомендация мигрировать на NVMe или включить WAL group-commit
- TOFU consent usage in production: `rate(shamir_admin_command_total{command="accept_new_host"}[1h]) > 0` — если `disable_tofu_in_production = true`, должно быть 0
- Clock skew: `abs(time() - shamir_last_observed_time) > 5` — manual `revokeAllTickets` recommended

---

### 7. known_hosts (Native Client) [NORMATIVE]

Клиент: `~/.shamir/known_hosts` + `~/.shamir/known_hosts.mac`.

Format `known_hosts`:
```
host:port  base64url(SHA256(server_pub_key))  added_at_ns
```

`known_hosts.mac` = HMAC-SHA256(`local_key`, file_content).

`local_key` хранится в (priority order):
- macOS: Keychain (Service: "ShamirDB", Account: "known-hosts-mac")
- Linux: freedesktop secret-service (D-Bus)
- Windows: Credential Manager
- Headless: `~/.shamir/local_key.bin` encrypted (см. §1.6)

При чтении:
1. Если `local_key` недоступен → **fail-closed**, требовать out-of-band pin
2. Verify `MAC == HMAC(local_key, file_content)` — constant-time
3. Mismatch → **fail-closed**
4. При file replace (rotation): atomic rename + новый MAC

При несовпадении owner / permissions → **fail-closed**.

#### 7.1. Server Identity Rotation — известные клиенты

Когда сервер выполняет `rotateServerIdentity` (AUTH §12.2), клиенты получают `identity_rotation` event в активной сессии. Procedure:

1. Клиент проверяет `signed_by_old` против currently pinned `old_pub`
2. Если valid: client SHOULD prompt user (interactive CLI) с информацией:
   - Old pin: `base64url(SHA256(old_pub))`
   - New pin: `base64url(SHA256(new_pub))`
   - Transition until: `<timestamp>`
3. На user confirmation: atomic update known_hosts entry для host:port
4. Recompute MAC, persist
5. Если non-interactive (CLI script): **fail-closed**, требовать manual `--pin <new>` для следующего подключения

Audit event `client_known_hosts_updated` (если client logging включён).

#### 7.2. TOFU production hardening

Server config `[strict] disable_tofu_in_production = true`:
- Server возвращает `tofu_disabled` warning в auth_ok если клиент использовал `--accept-new-host` (по протоколу: client SHOULD signal но это honor-system)
- Audit event `tofu_consent_used` записан с user, ip, timestamp
- Operators могут alert на любое появление этого event

Client side: `--accept-new-host` flag всегда печатает loud stderr warning + audit-grade log entry даже без server hint.

---

### 8. SystemStore Atomicity Requirements [NORMATIVE]

8.1. Все записи `__system__/*` — single-record atomic (rename-into-place / sqlite WAL / engine txn).

8.2. Cross-record updates — single transaction:
- bootstrap (create admin + invalidate token + set superuser_ever_existed)
- changePassword (update user record + invalidate sessions + invalidate tickets)
- updateUser (update record + tickets_invalid_before + kick sessions)
- rotateServerIdentity (update keypair + start transition window + audit)

8.3. Crash recovery checks при старте:
- `superuser_ever_existed AND no superuser users` → log warning (manual investigate, не auto-bootstrap)
- `bootstrap_token_hash present AND superuser_ever_existed = true` → cleanup token (стале)
- Lockout state vs persisted: replay batched in-memory state

---

### 9. Versioning Compatibility Matrix

| AUTH | SESSION_RESUMPTION | TRANSPORT_TCP | TRANSPORT_WS | Compatible? |
|---|---|---|---|---|
| v1 | v1 | v1 | v1 | ✓ |
| v1 | v1.1 (new optional field) | v1 | v1 | ✓ backward compat |
| v1 | v2 | v1 | v1 | ✗ wire-breaking |
| v2 | v1 | v1 | v1 | ✗ auth_message changed |

`auth_init.version` отражает только AUTH_PROTOCOL.md major.

**Enum extension rule** (NORMATIVE): `binding_mode`, `transport_kind`, `version` — owned by AUTH. SESSION_RESUMPTION и TRANSPORT_*.md MUST treat unknown enum values as fail-closed. Adding new enum value = AUTH minor bump.

---

### 10. Deployment

10.1. **v1 supports single-instance only.** SystemStore — single-writer (file lock рекомендуется).

10.2. **Embedded mode:** process-local DB. Plain TCP loopback OR Unix socket (предпочтительно).

10.3. **Multi-node clustering:** out of scope v1. См. ROADMAP.

---

### 11. Test Plan (release blockers)

11.1. **Test vectors** — `spec/test-vectors/auth_v1.json` обязателен (см. AUTH §16).

11.2. **Integration tests:**
- Full TCP+TLS auth round-trip
- Full WS native auth + resume
- Browser path (binding_mode=0x02) auth + resume в same tier
- Cross-transport same-tier resumption (TCP↔WS)
- Anti-downgrade resumption rejection
- Bootstrap (token TTL, CAS race в parallel attempt, file orphan cleanup, command pipe mode)
- Identity rotation (broadcast, signed_by_old, transition_until, per-recipient signing)
- Lockout (threshold, silent error, backoff)
- Channel binding mismatch detection
- Argon2id semaphore exhaustion
- Pre-Argon2id binding_mode rejection (DoS-amp)
- changePassword fresh challenge flow
- updateUser → ticket invalidation (всех families)
- Restart warmup window
- **Multi-device family isolation:** device A refresh не invalidates device B
- **Race attack:** strict `>` comparison при tickets_invalid_before_ns

11.3. **Log redaction tests** (§4.3) — mandatory CI gate.

11.4. **Audit chain integrity tests** — verify chain HMAC across N entries + truncation detection.

11.5. **Constant-time tests** (best-effort): synthetic timing для real-vs-fake user paths.

11.6. **Property-based tests (proptest)** — release blocker:
- Anti-downgrade invariants: random ticket params + random session params, assert downgrade always rejected
- Family isolation: random multi-device scenarios, assert no cross-family interference
- AAD tampering: random byte mutations always rejected by GCM

11.7. **Pre-auth fuzzing (cargo-fuzz / AFL)** — release blocker:
- Frame parsing на pre-auth path (≤ 4 KB)
- msgpack deserialization для auth_init, bootstrap_hello
- Должен быть memory-safe + reject all malformed inputs without panic

11.8. **Power-fail testing** для durability — release blocker:
- Resume → kill -9 server во время fsync → restart → assert ticket cannot replay
- Test на target storage backend (SQLite/sled/redb/fjall)

11.9. **Unicode normalization test vectors** — release blocker:
- Pin Unicode version (15.1 для v1)
- Test vectors для edge cases: combining marks, zero-width chars, casefold ambiguities, NFC vs NFD
- Cross-language consistency: Rust output == JS output (`String.normalize("NFC").toLowerCase()`)
- Reject non-stable normalization implementations

---

### 12. Зависимости (Rust имплементация)

```toml
argon2 = "0.5"               # подтверждённая версия на момент v1 freeze
hmac = "0.12"
hkdf = "0.12"
sha2 = "0.10"
ed25519-dalek = { version = "2.1", features = ["zeroize"] }   # use verify_strict
subtle = "2.5"
zeroize = { version = "1.7", features = ["derive"] }
rmp-serde = "1.3"
unicode-normalization = "0.1"
precis-profiles = "0.1"
aes-gcm = "0.10"             # для resumption ticket
```

Browser deps — см. CLIENT_BROWSER.md §2.

Версии могут обновляться; spec обновляется только при breaking compat changes в библиотеках.

---

### 13. Optional Admin Command Schemas

Не security-критичны, поэтому не в AUTH_PROTOCOL.

#### 13.1. `whoami`
```
Request:  { "whoami": {} }
Response: {
  "ok": {
    "user_id": bytes(16),
    "username": String,
    "roles": Vec<String>,
    "is_superuser": bool,
    "session_expires_at_ns": u64
  }
}
```

#### 13.2. `listSessions`
```
Request:  { "listSessions": { "user": Option<String> } }   // None = own sessions; superuser может смотреть все
Response: {
  "ok": {
    "sessions": [
      {
        "session_id_prefix": bytes(8),
        "user": String,
        "transport": "tcp" | "ws",
        "binding_mode": u8,
        "ip_subnet": String,
        "created_at_ns": u64,
        "last_activity": u64
      }
    ]
  }
}
```

#### 13.3. `serverInfo`
```
Request:  { "serverInfo": {} }
Response: {
  "ok": {
    "version": "1.0.0",
    "protocol_version": 1,
    "kdf_params_current": { ... },
    "supported_transports": ["tcp", "ws"]
  }
}
```

#### 13.4. `revokeUserTickets`, `revokeAllTickets`, `revokeAllLockouts`, `rotateAuditChainKey`

Schemas очевидны из имени; semantic см. в AUTH_PROTOCOL §12, SECURITY_MODEL §3, IMPLEMENTATION_GUIDE §5.

---

### 14. См. также

- **AUTH_PROTOCOL.md** — нормативный протокол
- **SECURITY_MODEL.md** — adversary model + threat coverage
- **SESSION_RESUMPTION.md** — ticket protocol
- **TRANSPORT_TCP.md / TRANSPORT_WS.md** — transport bindings
- **ADMIN_UI_HOSTING.md** — admin UI delivery
- **CLIENT_BROWSER.md** — browser SDK
- **../ROADMAP.md** — future hardening

---

## Part 10: ROADMAP

> Source file: `ROADMAP.md`

## ShamirDB Roadmap

Future features beyond v1 spec. Не нормативные, не binding обещания.

### Auth Protocol

#### v1.1 (короткий горизонт)

- **HIBP-style breach check** при password set/change (online k-anonymity API ИЛИ offline static set)
- **Argon2id parameter auto-tuning** на старте сервера (benchmark под текущее железо)
- **Channel binding RFC 9266 формальное соответствие** — формальная attestation если interop требует
- **WebAuthn second factor** для admin operations (browser-native)
- **`expire_password_now` flag** в updateUser (требует field `password_must_change` в user record)
- **DPoP-like request signing** для session tokens — channel-bound MAC per request, защищает session_id от bearer-token theft
- **Privilege separation** — отдельный signer process для Ed25519 ops (RCE compartmentalization)

(Audit log HMAC chaining и Bootstrap token TTL configurable — перенесены в v1, см. IMPLEMENTATION_GUIDE.md §3.3 и AUTH_PROTOCOL.md §11.2.2.)

#### v1.2

- **TRANSPORT_QUIC.md** — QUIC native binding. Переиспользует AUTH_PROTOCOL без изменений.
- **TRANSPORT_UDP.md** — UDP datagram binding (для embedded sensors / WireGuard-style overlay). Mandatory L1 (HMAC) per packet.
- **Unix socket transport** — file permissions = auth boundary, no SCRAM needed (отдельный mode).

#### v2 (несовместимо)

- **Hybrid PQ identity:** Ed25519 + ML-DSA-65 (FIPS 204). Pin = `SHA256(ed25519_pub || mldsa_pub)`. Migration без breaking handshake (server поддерживает оба, клиент verify оба).
- **Hybrid PQ key exchange** в TLS (X25519+MLKEM768) — ждём rustls full support
- **FIPS profile:** alternative kdf=PBKDF2-HMAC-SHA256, signature=ECDSA P-256. Configurable.
- **Cluster mode:** shared SystemStore + sticky sessions OR distributed session store
- **OAuth/OIDC bridge** для SSO интеграций

### Database Engine (вне scope auth spec)

См. отдельные документы (TBD):
- Query language v2
- Replication
- Sharding
- Backup tooling
- Migration tooling

---

Roadmap не binding — фичи могут переехать между версиями или быть отброшены.
