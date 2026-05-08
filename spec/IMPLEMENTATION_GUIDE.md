# ShamirDB Implementation Guide

Operational details для имплементаторов сервера и клиентов.

**Mixed normativity.** Sections marked **[NORMATIVE]** — обязательны для security claims из SECURITY_MODEL.md. Альтернативная имплементация может расходиться в **non-normative** разделах без потери wire-compatibility, но ДОЛЖНА соблюдать [NORMATIVE].

---

## 1. SystemStore Layout

### 1.1. `__system__/users/{user_id}` — см. AUTH_PROTOCOL §3.5.

### 1.2. `__system__/server_meta` [NORMATIVE schema]

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

### 1.3. In-memory state [NORMATIVE]

#### `auth_failures: DashMap<(subnet, username_hash), FailureState>`
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

#### `lockout_state: DashMap<(subnet, username_hash), LockoutState>` [NORMATIVE]
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

#### `handshake_states: DashMap<connection_id, HandshakeState>`
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

#### `consumed_counters: DashMap<(user_id, ticket_family_id), u64>` [NORMATIVE]
last_consumed `family_counter` per ticket lineage (см. SESSION_RESUMPTION §6.2).

**SYNCHRONOUS DURABLE persist** перед `resume_ok` reply — **не** batched. "Durable" =:
- POSIX `fsync` после write
- SQLite backend: `PRAGMA synchronous=FULL` + WAL + checkpoint OR `PRAGMA synchronous=EXTRA`
- sled/redb/fjall: equivalent flush-and-sync
- ext4: mount option `barrier=1` (default modern, проверять)
- xfs/btrfs: equivalent write barriers
- **Power-fail testing required** для valid implementation (release blocker per IMPLEMENTATION_GUIDE §11)

См. SESSION_RESUMPTION §10.2 для motivation. **Not batched** потому что batched flush откатывает counter при crash → ticket replay window.

#### `bootstrap_token_files: DashMap<path, expires_at_ns>`
Tracking outstanding bootstrap token files для GC (см. §1.4).

#### `sessions: DashMap<session_id, Arc<Session>>`
Session state — AUTH_PROTOCOL §7.2. Не персистентны. GC каждую минуту.

### 1.4. Restart warmup [NORMATIVE]

В первые 60 секунд после старта server applies глобальный rate limit `RATE_LIMIT_AUTH_INIT_PER_SUBNET / 4 = 2.5/sec` пока in-memory state warmup'ится из persisted snapshots. Закрывает restart-replay window для distributed attackers.

### 1.5. Memory quotas [NORMATIVE]

Помимо `PER_SESSION_MEM = 64 MB` (AUTH §7.4):

- `MAX_TOTAL_SESSION_MEM_PER_SUBNET = 256 MB` — глобальный cap на (input + output буферы + parsed query state) для всех sessions от одного subnet (/24 IPv4, /64 IPv6). Превышение → новые sessions из subnet rejected с `server_busy`.
- Backpressure policy: при достижении 75% от cap — server отклоняет non-critical operations (admin commands proceed) и шлёт `Retry-After` header.
- `MAX_CONCURRENT_ARGON2` рекомендуется derive from RAM: `floor(available_ram_mb / (kdf.memory_kb / 1024 × 2.5))`. Hard cap 64. Защищает от OOM при memory_kb=128, server с 4GB RAM.

### 1.6. known_hosts.bin fallback encryption (NORMATIVE для headless deploy)

Когда OS keychain недоступен (headless server, embedded), `local_key.bin` fallback (см. §7) хранится в `~/.shamir/local_key.bin` chmod 600. **Encryption обязательна:**

- Default: encrypted с пользовательской passphrase через `scrypt` (N=2^17, r=8, p=1) → AES-256-GCM. Passphrase запрашивается интерактивно при первом запуске; хранится в memory keyring/agent для последующих.
- Alternative: `--local-key-file <path>` injected via env var or external secrets manager (Vault, Doppler).
- Plain `local_key.bin` (без encryption) — **только** с `--insecure-local-key` flag и audit warning при каждом запуске.

---

## 2. Server Configuration

### 2.1. Listeners [NORMATIVE listener_policy_mapping]

```toml
[server]
data_dir = "./data"

# Bootstrap token output — варианты, выбрать один
bootstrap_token_output = "tty"          # tty | file:<path> | command:<cmd>
bootstrap_token_ttl_ns = 3_600_000_000_000     # default 1 час (60·60·1e9 nanos), min 300s=3e11, max 24h=8.64e13

# Argon2id defaults (must satisfy floor §3.7.2 AUTH)
[kdf]
memory_kb = 131072
time = 4
parallelism = 1
# MAX_CONCURRENT_ARGON2 — RECOMMENDED derive from RAM:
#   floor(available_ram_mb / (memory_kb / 1024 * 2.5))
# Hard cap 64 (защита от runaway).
# Без autodetect — fixed:
max_concurrent_argon2 = 32              # для server с 8 GB RAM

# Strict mode hardening (defaults для backward compat, для prod рекомендуется true)
[strict]
allow_browser_ticket_upgrade = true     # false = browser ticket НЕ может быть resumed в native
disable_tofu_in_production = false      # true = клиенты MUST использовать out-of-band pin

# Memory budgets [NORMATIVE]
[limits]
per_session_mem_mb = 64
max_total_session_mem_per_subnet_mb = 256
max_connections_per_ip = 100

# Resumption behavior
[resumption]
disable_plain_ticket_upgrade = false    # true = plain ticket НЕ может resume в TLS transport
                                         # (мнемоника: plain → stronger transport blocked)
                                         # false (default) = embedded handoff OK

# Listeners — каждый со своим binding_mode policy
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

#### Bootstrap token output options

- `tty` (default): печать в stdout **только** если `isatty(stdout)` AND процесс не systemd-managed (проверка `INVOCATION_ID` env var). Иначе server fails с инструкцией.
- `file:<path>`: атомарно создать `chmod 600` файл. **Strongly recommend tmpfs/ramdisk path** (`/run/shamir/bootstrap.token` или `/dev/shm/...`) — обычные filesystem пути попадают в backup/AV/EDR/cloud sync ecosystem. Server **MUST** удалить после use или TTL.
- `command:<cmd>`: pipe token в external command (e.g., `pass insert shamir/bootstrap`, `age -r recipient -e -o /vault/token.age`, `gpg -e -r ops@example.com`). Token не касается обычного диска.

**WARNING:** systemd / journald / docker logs / k8s log shippers **захватывают stdout** даже при `isatty` check (через TTY emulation). Пред-deployment проверять `journalctl -u shamirdb` после bootstrap — токен не должен присутствовать.

**MUST:** profile→binding_mode mapping enforced server-side. Server rejects auth_init с `binding_mode` не в listener policy **до** Argon2id (DoS-amp защита; см. AUTH §4.3).

### 2.2. Plain TCP listener constraints [NORMATIVE]

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

### 2.3. Browser-only deployment warning

Если `[admin_ui].enabled = true` AND нет ни одного `[[listener]]` с `profile = "tls"` (native) — server warning при старте: "browser endpoint без native peer — admin клиенты forced на binding_mode=0x02 (ослабленный режим)". Не блокер, но привлекает внимание.

---

## 3. Audit Log [NORMATIVE]

### 3.1. Format

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

### 3.2. Минимум v1 events

**Auth lifecycle:** `auth_success` (sampled), `auth_failed` (rate-limited 1/мин per (subnet, user)), `auth_aborted` (sampled).

**Bootstrap:** `bootstrap_used`, `bootstrap_regen`, `bootstrap_token_file_orphan_cleaned`.

**Lockout:** `lockout_triggered`, `lockout_released`, `revoke_all_lockouts`.

**User management:** `user_created`, `user_deleted`, `roles_changed`, `password_changed`, `kdf_params_upgraded`.

**Sessions:** `kick_session`, `session_evicted{reason}`.

**Resumption:** `resumption_used` (sampled), `resumption_replay_detected` (always), `resumption_downgrade_blocked` (always).

**Rotation:** `rotate_server_identity`, `rotate_server_secret`, `rotate_ticket_key`, `rotate_audit_chain_key`.

**Audit integrity:** `audit_chain_verify_failed` (startup truncation/tamper detection).

**Revocation:** `revoke_user_tickets`, `revoke_all_tickets`.

### 3.3. HMAC chaining + truncation defence (NORMATIVE v1)

#### Chain construction

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

#### Tamper detection

- Per-entry: `hmac` не сходится → entry corrupted/forged
- Chain: `prev_hmac` поля цепочки → разрыв = compromised в этой точке

#### Truncation defence

Атакующий offline удалил последние N entries. Без checkpoint защиты — недетектируемо.

**Защита (NORMATIVE):**
- Каждые 60 секунд OR каждые 1000 entries (whichever first) server атомарно persist'ит в `server_meta` поля `last_audit_hmac`, `last_audit_seq`, `last_audit_checkpoint_at`.
- Также — на graceful shutdown (SIGTERM/SIGINT).
- На startup server verify: парсит audit_log хвост → находит самую последнюю запись → если её `seq < last_audit_seq` ИЛИ её `hmac != last_audit_hmac` → **alert** (`shamir_audit_chain_verify_failures_total` increments + log warning + opt-in operator интервенция).

#### Key rotation

`rotateAuditChainKey` admin command:
1. `audit_chain_key_previous = current; current = random(32); audit_chain_key_rotated_at = now`
2. Через 30 дней background task: `audit_chain_key_previous = NULL`
3. Verify legacy entries (issued под previous): tooling пробует current, потом previous
4. Audit event `rotate_audit_chain_key`

#### Verify tooling

Out-of-band binary `shamir-audit-verify` читает `__system__/server_meta` (`audit_chain_key{,_previous}`, `last_audit_*`) + `audit_log` файл, replays HMAC chain entry-by-entry, reports first mismatch и truncation status.

---

## 4. Log Redaction Policy [NORMATIVE]

### 4.1. Запретный список — НИКОГДА в log/tracing/error messages

- `password` (любой формы)
- `salted_password`, `client_key`, `server_key` (full)
- `stored_key` (full — допустим `prefix(4 bytes hex)` для correlation)
- `server_secret`, `lockout_secret`, `server_ed25519_priv`, `ticket_key`, `audit_chain_key`, `audit_chain_key_previous`
- `session_id` (full — допустим `prefix(8 bytes hex)`)
- `client_proof`, `server_signature`, `identity_sig`
- `client_nonce`, `server_nonce`
- `bootstrap_token`
- `ticket` plain или wire format

### 4.2. Allow-list

- Username (NFC) — subject to GDPR/PII per jurisdiction
- IP subnet (/24 или /64)
- Timestamp
- Error code
- Session prefix (8 hex)
- Stored_key prefix (4 hex)

### 4.3. CI test (mandatory) [NORMATIVE]

```rust
#[test]
fn test_no_secret_leak_in_logs() {
    let password = "uniquetestpass1234567890";
    let captured = capture_tracing(|| run_full_auth(password));
    assert!(!captured.contains("uniquetestpass"));
    // повторить для всех secrets из 4.1 с magic patterns
}
```

### 4.4. Type-level enforcement (Rust)

Все типы containing secrets → custom `Debug` → `<REDACTED>`. Marker trait `Sensitive`. `Display` запрещён.

---

## 5. Compromise Recovery — Detailed Runbooks

### 5.1. server_secret leaked

1. Generate new: `new_secret = random(32)`
2. Atomic update: `server_secret_previous = current; server_secret = new; server_secret_rotated_at = now`
3. Через 7 дней background task: `server_secret_previous = NULL`
4. Audit event `rotate_server_secret`

### 5.2. server_ed25519_priv leaked (TIME-CRITICAL)

1. **Kill switch:** server config `--identity-revoked` → server отвечает только generic auth failures. Active sessions terminate.
2. Generate new keypair.
3. Set `previous = current; current = new; rotation_until = now + 7 days`.
4. Restart server (`--identity-revoked` → off).
5. Через активные сессии broadcast `identity_rotation` (см. AUTH §12.2).
6. **Force out-of-band re-pin** для всех клиентов (announcement через email / pager).
7. Через 7 дней `previous = NULL`.

### 5.3. lockout_secret leaked

`lockout_secret` ротация **не делается обычно** (lockout state ключи зависят от него, ротация = orphan state). При compromise:
1. `revokeAllLockouts` admin command (clears in-memory + persisted lockout state)
2. Generate new `lockout_secret`
3. Restart (новые fail counts с новыми ключами)

Trade-off: атакующий получает clean slate (50 attempts заново). Acceptable если лучше чем продолжать с compromised secret.

### 5.4. DB users snapshot leaked

1. Force re-auth всем: `revokeAllUserSessions` + `revokeAllTickets` (закрывает existing sessions, форсит SCRAM на всех)
2. Out-of-band notification всем: смените пароль (через self-service §12.5 AUTH)
3. Audit для potential data exfil
4. Rate-limit increased temporarily
5. Не trigger полного teardown (Argon2id offline brute = годы для 12+ char паролей)

(`expire_password_now` flag в `updateUser` запланирован v1.1 — требует поля `password_must_change` в user record + handling в auth_ok flow. Сейчас mass session revocation + user notification.)

### 5.5. Full SystemStore compromised — полный teardown

1. Stop server
2. Generate fresh `server_meta`: новые secrets, ed25519, ticket_key, audit_chain_key, `superuser_ever_existed = false`
3. Backup users (если хочется сохранить usernames + stored_keys для force change); опционально wipe
4. Restart → bootstrap re-enable
5. Out-of-band announcement
6. Audit forensics

### 5.6. Lost admin password

`shamir-server --regen-bootstrap --confirm` (требует stop сервера + физический доступ + флаг + stdin confirmation phrase):

1. Generate новый `bootstrap_token`, `superuser_ever_existed` остаётся `true`
2. Output token per `bootstrap_token_output` config
3. Audit event `bootstrap_regen`
4. Restart → operator делает re-bootstrap для нового admin
5. Operator manually удаляет старого locked-out admin (если есть)

### 5.7. Backup restore (counter rollback prevention) — MANDATORY

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

## 6. Observability — Metrics

Prometheus-style, exposed на `/admin/metrics` с `Authorization: Bearer <admin_session>`.

### 6.1. Counters

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

### 6.2. Histograms

- `shamir_auth_duration_seconds{transport}`
- `shamir_argon2id_duration_seconds`
- `shamir_handshake_state_lifetime_seconds`
- `shamir_resumption_fsync_latency_seconds` — alert если p99 > 100ms (recommend NVMe или WAL group-commit)
- `shamir_audit_log_append_latency_seconds`

### 6.3. Gauges

- `shamir_active_sessions{transport}`
- `shamir_inflight_handshakes`
- `shamir_argon2id_semaphore_available`
- `shamir_auth_failures_tracked_keys`

### 6.4. Suggested Alerts

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

## 7. known_hosts (Native Client) [NORMATIVE]

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

### 7.1. Server Identity Rotation — известные клиенты

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

### 7.2. TOFU production hardening

Server config `[strict] disable_tofu_in_production = true`:
- Server возвращает `tofu_disabled` warning в auth_ok если клиент использовал `--accept-new-host` (по протоколу: client SHOULD signal но это honor-system)
- Audit event `tofu_consent_used` записан с user, ip, timestamp
- Operators могут alert на любое появление этого event

Client side: `--accept-new-host` flag всегда печатает loud stderr warning + audit-grade log entry даже без server hint.

---

## 8. SystemStore Atomicity Requirements [NORMATIVE]

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

## 9. Versioning Compatibility Matrix

| AUTH | SESSION_RESUMPTION | TRANSPORT_TCP | TRANSPORT_WS | Compatible? |
|---|---|---|---|---|
| v1 | v1 | v1 | v1 | ✓ |
| v1 | v1.1 (new optional field) | v1 | v1 | ✓ backward compat |
| v1 | v2 | v1 | v1 | ✗ wire-breaking |
| v2 | v1 | v1 | v1 | ✗ auth_message changed |

`auth_init.version` отражает только AUTH_PROTOCOL.md major.

**Enum extension rule** (NORMATIVE): `binding_mode`, `transport_kind`, `version` — owned by AUTH. SESSION_RESUMPTION и TRANSPORT_*.md MUST treat unknown enum values as fail-closed. Adding new enum value = AUTH minor bump.

---

## 10. Deployment

10.1. **v1 supports single-instance only.** SystemStore — single-writer (file lock рекомендуется).

10.2. **Embedded mode:** process-local DB. Plain TCP loopback OR Unix socket (предпочтительно).

10.3. **Multi-node clustering:** out of scope v1. См. ROADMAP.

---

## 11. Test Plan (release blockers)

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

## 12. Зависимости (Rust имплементация)

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

## 13. Optional Admin Command Schemas

Не security-критичны, поэтому не в AUTH_PROTOCOL.

### 13.1. `whoami`
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

### 13.2. `listSessions`
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

### 13.3. `serverInfo`
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

### 13.4. `revokeUserTickets`, `revokeAllTickets`, `revokeAllLockouts`, `rotateAuditChainKey`

Schemas очевидны из имени; semantic см. в AUTH_PROTOCOL §12, SECURITY_MODEL §3, IMPLEMENTATION_GUIDE §5.

---

## 14. См. также

- **AUTH_PROTOCOL.md** — нормативный протокол
- **SECURITY_MODEL.md** — adversary model + threat coverage
- **SESSION_RESUMPTION.md** — ticket protocol
- **TRANSPORT_TCP.md / TRANSPORT_WS.md** — transport bindings
- **ADMIN_UI_HOSTING.md** — admin UI delivery
- **CLIENT_BROWSER.md** — browser SDK
- **../ROADMAP.md** — future hardening
