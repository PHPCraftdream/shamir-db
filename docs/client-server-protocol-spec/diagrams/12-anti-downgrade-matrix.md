# 12 — Anti-Downgrade Decision Matrix

Decision tables для resumption + listener policy. Сюда же — compromise recovery actions.

## binding_strength rules (SESSION_RESUMPTION §6.1)

```
binding_strength:
  binding_mode == 0x00 (none / plain)            → 0
  binding_mode == 0x02 (tls_no_export browser)   → 1
  binding_mode == 0x01 (tls_exporter)            → 2

resume reject if binding_strength(now) < binding_strength(at_auth)
```

## Cross-transport resumption matrix

| at_auth (ticket) | now (resume connection) | Default | strict mode<br/>`allow_browser_ticket_upgrade=false` | strict mode<br/>`disable_plain_ticket_upgrade=true` |
|---|---|---|---|---|
| TLS exporter (TCP) | TLS exporter (TCP) | ✅ same tier | ✅ | ✅ |
| TLS exporter (TCP) | TLS exporter (WSS) | ✅ cross-transport | ✅ | ✅ |
| TLS exporter (WSS) | TLS exporter (TCP) | ✅ cross-transport | ✅ | ✅ |
| Browser (WSS) | TLS exporter (TCP) | ✅ upgrade | ❌ rejected | ✅ |
| Browser (WSS) | Browser (WSS) | ✅ same tier | ✅ | ✅ |
| TLS exporter | Browser | ❌ DOWNGRADE | ❌ | ❌ |
| TLS exporter | plain (loopback) | ❌ DOWNGRADE | ❌ | ❌ |
| Browser | plain | ❌ DOWNGRADE | ❌ | ❌ |
| plain (loopback) | TLS exporter | ✅ upgrade | ✅ | ❌ rejected |
| plain | Browser | ✅ upgrade | ✅ | ❌ rejected |
| plain | plain | ✅ same tier | ✅ | ✅ |

## Listener policy → binding_mode enforcement

| Listener config | Server requires `auth_init.binding_mode` | Channel binding value |
|---|---|---|
| `transport=tcp, profile=tls` | `0x01` | TLS exporter (32 bytes) |
| `transport=tcp, profile=plain` (loopback only) | `0x00` | bytes(32) zeros |
| `transport=ws, profile=tls` (endpoint /shamir/v1) | `0x01` | TLS exporter |
| `transport=ws, profile=tls_browser` (endpoint /shamir/v1/browser) | `0x02` | bytes(32) zeros |

**MUST:** Server rejects auth_init с `binding_mode` не в listener policy **до** Argon2id (DoS-amp защита). Reject = silent close.

**Browser endpoint protection:** `/shamir/v1/browser` requires `Origin` header (browser always sends per WS spec, native typically не шлёт). Native client случайно/злонамеренно подключающийся к browser endpoint → 400.

## Compromise Recovery Decision Matrix (SECURITY_MODEL §3 + IMPL §5)

| Что утекло | Severity | Действие |
|---|---|---|
| `server_secret` | 🟡 Medium | Rotate с overlap 7d (`server_secret_previous`) |
| `lockout_secret` | 🟡 Medium | `revokeAllLockouts` + restart с new lockout_secret (clean slate trade-off) |
| `server_ed25519_priv` | 🔴 Critical | (1) Kill switch `--identity-revoked` (2) Generate new keypair (3) **Force out-of-band re-pin all clients** (4) Audit forensics |
| `audit_chain_key` | 🟠 High | Rotate с overlap 30d. Compromised entries marked stale. Verify legacy через previous key. |
| Audit log truncation | 🟠 High | `last_audit_*` checkpoint detects при startup → operator интервенция |
| DB users snapshot | 🟠 High | `revokeAllUserSessions` + `revokeAllTickets` + force password rotation всем (out-of-band notification) |
| **Полный SystemStore** | 🔴 Critical | Полный teardown: новый server_meta, fresh keys, `superuser_ever_existed = false`, force re-bootstrap |
| Client password | 🟡 User-level | User меняет пароль (через self-service §12.5) |
| Client `known_hosts` | 🟡 Medium | Integrity MAC спасает; иначе re-pin out-of-band |
| Bootstrap token (в TTL) | 🟠 High (если activated) | TTL 1h спасает; alert на `bootstrap_used` event если source unexpected |
| `ticket_key` | 🟠 High | `revokeAllTickets` (no overlap, immediate `previous=NULL`) |
| Browser session ticket (XSS) | 🟡 Medium | One resume-then-counter-blocked. Mitigation: `kickSession` + `revokeUserTickets` |
| `previous_ed25519_priv` (during overlap) | 🟠 High | Emergency rotation (`--identity-revoked`), НЕ planned. Без emergency: orphan recovery (§6.5) даёт MITM окно |
| Backup restore (counter rollback) | 🟠 High | **MANDATORY** `--revoke-all-tickets-on-start` flag |

## Bootstrap state invariants

```
PRE-BOOTSTRAP STATE:
  bootstrap_token_hash IS NULL
  superuser_ever_existed == false
  __system__/users пуст
  → ALLOWS first bootstrap

POST-BOOTSTRAP STATE:
  bootstrap_token_hash IS NULL  (consumed)
  superuser_ever_existed == true (PERSISTENT, never reset)
  superuser EXISTS in __system__/users
  → BLOCKS bootstrap (any future call → bootstrap_failed)

INVARIANT (always):
  superuser_ever_existed == true ⇔ bootstrap was successfully used at least once
  (corruption recovery: never auto-bootstrap if this flag is true,
   even if users пуст — manual investigate required)

REGEN STATE:
  shamir-server --regen-bootstrap --confirm
  → bootstrap_token_hash != NULL  (fresh token)
  → superuser_ever_existed remains true
  → first existing superuser unaffected (operator manual cleanup if needed)
```

## Identity rotation invariants

```
NORMAL STATE:
  current_priv, current_pub set
  previous_priv = NULL, previous_pub = NULL
  rotation_until_ns = NULL
  → rotateServerIdentity ALLOWED

ROTATION OVERLAP:
  current_priv, current_pub = NEW
  previous_priv, previous_pub = OLD  (kept for 7 days)
  rotation_until_ns = now + 7 days  (set at rotation time)
  → rotateServerIdentity REJECTED (rotation_in_progress_already)
  → auth_ok includes rotation_in_progress payload (orphan recovery)
  → resume of "previous-issued" ticket REJECTED (force re-auth)

POST-OVERLAP:
  Background task at rotation_until_ns:
  - zeroize previous_priv
  - previous_pub = NULL
  - rotation_until_ns = NULL
  → BACK TO NORMAL

EMERGENCY:
  --identity-revoked flag
  → kill switch (active sessions terminate)
  → no rotation_in_progress payload (orphan recovery DISABLED)
  → orphan clients see server_signature_invalid → manual re-pin out-of-band
```
