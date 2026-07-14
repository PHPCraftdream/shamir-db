# ShamirDB Security Model v1

Adversary model, threat coverage, non-guarantees, compromise recovery overview.

Operational детали (метрики, audit log, log redaction, recovery runbooks) — **IMPLEMENTATION_GUIDE.md**.
Future hardening — **../roadmap/ROADMAP.md**.

---

## 1. Adversary Model

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

## 2. Threat Coverage

| Угроза | Adv | Защита |
|---|---|---|
| Passive eavesdropping | A1 | Транспортный TLS |
| Active MITM | A2 | TLS + Ed25519 server pin + channel_binding в auth_message |
| Password sniffing | A1, A2 | SCRAM: пароль не покидает клиент |
| Server impersonation после DB leak | A3 | Ed25519 priv в server_meta отдельно от users |
| User enumeration via timing | A1, A2 | Constant-time fake values via HKDF, generic errors, padded latency |
| User enumeration via channel binding | A2 | binding_mode embedded в auth_message → MITM detection |
| KDF param downgrade | A2 | Raw kdf_params в auth_message; server-side floor |
| Mid-session downgrade через admin user update | A5 | Server игнорирует client kdf_params в `createUser`, всегда defaults |
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
| Stale resumption после revoke ролей | A5 (mitigated) | `tickets_invalid_before` инвалидирует tickets, обновляется через kickSession/updateUser |
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

## 3. Compromise Recovery (overview)

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

## 4. Non-Guarantees

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

## 5. Standards Compliance

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

## 6. См. также

- **AUTH_PROTOCOL.md** — нормативный протокол
- **IMPLEMENTATION_GUIDE.md** — operational details + audit chain HMAC
- **SESSION_RESUMPTION.md** — ticket protocol
- **../roadmap/ROADMAP.md** — v1.1+ roadmap
