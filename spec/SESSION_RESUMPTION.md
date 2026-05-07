# ShamirDB Session Resumption v1

Быстрый reconnect (~10ms вместо ~2s Argon2id). Поддержка cross-transport с **анти-downgrade защитой**.

---

## 1. Когда полезно

- Mobile NAT rebinding (новый TCP/IP)
- Cross-transport switch (TCP → WS) **в пределах одного security tier**
- CLI tools — короткоживущие команды
- Failover к replica

---

## 2. Ticket Format

### 2.1. Plaintext

```
ticket_plain = canonical_msgpack({
  "version": 1,                           // u8
  "user_id": bytes(16),
  "username_nfc": String,
  "permissions": SessionPermissions,      // см. AUTH_PROTOCOL §7.3
  "transport_kind_at_auth": u8,
  "binding_mode_at_auth": u8,
  "channel_binding_at_auth": bytes(32),
  "original_auth_at": u64,                // ВСЕГДА от первого full SCRAM, не обновляется при refreshTicket
  "expires_at": u64,
  "monotonic_counter": u64                // см. §6
})
```

`ticket_plain` использует **canonical msgpack** (lex-sorted keys, smallest int encoding, no NaN) — потому что `ticket_id = SHA256(ticket_wire)` зависит от bit-exact bytes.

### 2.2. Wire format

```
ticket_wire = struct {
  version: u8 = 1,
  nonce: bytes(12),                       // CSPRNG, per-ticket
  ciphertext_len: u16_be,
  ciphertext: bytes,                      // AES-256-GCM ciphertext
  tag: bytes(16)
}

aad = "SHAMIR-TICKET-v1"
   || u8(version)                         // дублируется из plaintext — anti-cross-version
   || u8(transport_kind_at_auth)
   || u8(binding_mode_at_auth)

ciphertext, tag = AES-256-GCM(
    key   = ticket_key,
    nonce = nonce,
    plaintext = ticket_plain,
    aad   = aad
)
```

`version`, `transport_kind`, `binding_mode` дублируются в AAD (помимо plaintext). При decrypt сервер **формирует AAD из decrypted plaintext fields** и валидирует через GCM tag — атакующий, попытавшийся пересобрать ticket с подменёнными полями, провалит tag check.

Размер: ~150-300 байт.

---

## 3. Ticket Key

3.1. `ticket_key: bytes(32)` хранится в `__system__/server_meta`.

3.2. **Ротация каждые 24 часа** автоматически:
- `previous = current; current = random(32)`
- Через 24 часа `previous = NULL`
- При decrypt сервер пробует `current`, потом `previous`. Constant-time не требуется.

3.3. **Emergency rotation:** admin command `revokeAllTickets` — `current = random(32); previous = NULL` немедленно. Все existing tickets invalid.

3.4. Audit event на каждую ротацию: `rotate_ticket_key`.

---

## 4. Issuance

4.1. Server выдаёт ticket в `auth_ok` (опционально):
```
{
  "auth_ok": {
    ...,
    "resumption_ticket": bytes,           // ticket_wire
    "resumption_expires_at": u64
  }
}
```

4.2. Issued при:
- Initial auth после полного SCRAM — `original_auth_at = now`, `monotonic_counter = 1`.
- `refreshTicket` команда в активной сессии — server **немедленно invalidates** previous ticket (атомарный update `current_ticket_id` per-session). **Новый ticket наследует `original_auth_at` из предыдущего** (НЕ обновляется на `now`). `monotonic_counter` инкрементируется.

`original_auth_at` обновляется **только** при full SCRAM re-auth. Это обеспечивает корректную работу `tickets_invalid_before` инвалидации цепочки (см. §6.3).

4.3. **Не issued** для:
- Bootstrap-сессий (single-use admin создание)
- HTTP-bearer-сессий (HTTP не поддерживает primary auth — см. ADMIN_UI_HOSTING.md)

---

## 5. Resume Flow

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
1. Read aad fields из ticket_wire (version, transport_kind, binding_mode encoded в AAD)
2. Decrypt ticket via current ticket_key, fail → try previous, fail → resumption_failed
   (AES-GCM tag verify автоматически валидирует AAD vs plaintext)
3. Verify ticket_plain.version == aad.version
4. Verify ticket_plain.expires_at > now
5. Verify ticket_plain.version supported
6. Lookup user by user_id; if not exists → resumption_failed
7. Verify ticket_plain.original_auth_at >= user.tickets_invalid_before
   (защита от revoked role / kicked session)
8. SECURITY DOWNGRADE CHECK (см. §6.1):
   binding_strength(binding_mode_now) >= binding_strength(ticket_plain.binding_mode_at_auth)
9. ATOMIC compare-and-swap: 
   if ticket_plain.monotonic_counter > user.last_consumed_counter:
       user.last_consumed_counter = ticket_plain.monotonic_counter
       SYNCHRONOUS persist user.last_consumed_counter (fsync)
       proceed
   else:
       resumption_failed
10. Создаёт новую Session с binding_mode_now, channel_binding_now
11. Reply auth_ok с новым session_id и (опц) новым ticket
12. (Если активна identity rotation — см. §5.7)
```

5.5. На fail на любом шаге → `{"error": "resumption_failed"}` (generic).

5.6. На success:
```
{
  "resume_ok": {
    "session_id": bytes(32),
    "expires_at": u64,
    "resumption_ticket": Optional<bytes>,
    "resumption_expires_at": Optional<u64>
  }
}
```

### 5.7. Resumption во время identity rotation

Если ticket был issued под `previous` Ed25519 keypair и `transition_until > now`:

**v1 поведение:** server отвергает resume → клиент выполняет full re-auth и сразу получает identity_sig от **current** ключа. Это простейшая семантика без двойных подписей и inline rotation events.

Цена: клиенты со stale tickets во время окна rotation теряют ~2 секунды (Argon2id full re-auth) на первое подключение. Acceptable trade-off против complexity multi-key signing.

После окна rotation (`now > transition_until`) resumption работает нормально с current keypair.

---

## 6. Безопасность

### 6.1. Anti-downgrade rule (CRITICAL)

```
binding_strength:
  binding_mode == 0x00 (none / plain)            → 0
  binding_mode == 0x02 (tls_no_export browser)   → 1
  binding_mode == 0x01 (tls_exporter)            → 2

resume reject if binding_strength(now) < binding_strength(at_auth)
```

**Семантика:** ticket выпущенный в TLS-exporter сессии не может быть resumed в plain или browser сессии. Browser ticket может быть resumed в native (upgrade OK). Plain ticket — куда угодно.

### 6.2. Anti-replay через monotonic counter

Server поддерживает per-user `last_consumed_counter: u64`. Каждый issued ticket имеет уникальный `monotonic_counter` (incremented на каждом issue/refresh).

При resume: `ticket.counter > last_consumed_counter` (atomic CAS) → consume + **synchronous fsync** перед `resume_ok` ответа клиенту → update.

**SYNCHRONOUS persist обязателен:** иначе при crash в окне `flush_interval` (5s default) counter откатывается → атакующий с украденным ticket может resume повторно. Cost = один disk-sync per resume — приемлемо потому что resume редкая операция (раз в час, сравнимо с SCRAM cost).

**Defence properties:**
- Прямой replay: ticket consumed → counter advanced → старый rejected
- Race: atomic CAS обеспечивает строгое serialized consume
- LRU eviction атак нет (counter не evicted)
- Crash-restart: synchronous persist гарантирует consistency

### 6.3. Ticket invalidation

Сервер invalidates все tickets юзера через `tickets_invalid_before` (см. AUTH_PROTOCOL §3.5). Triggers:
- `kickSession` admin command (§12.4 AUTH)
- `changePassword`
- `updateUser` с new roles (§12.6 AUTH)
- Manual `revokeUserTickets` admin command (см. §7)

При resume: `ticket.original_auth_at >= user.tickets_invalid_before` обязательно.

`original_auth_at` НЕ обновляется при `refreshTicket` (§4.2) — это обеспечивает что вся цепочка refreshed tickets разом invalidates.

### 6.4. Cross-transport allowed scenarios

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

### 6.5. Что НЕ защищает

- **Stolen ticket на secure transport** — атакующий может resume **один раз** до того как counter инкрементнётся легитимным клиентом. Mitigation: `kickSession` + `revokeUserTickets` при подозрении.
- **Compromised ticket_key** — атакующий forge для всех юзеров. Mitigation: ротация каждые 24 часа + emergency `revokeAllTickets`.
- **Browser sessionStorage XSS** — НЕ применимо (ticket в memory only, см. CLIENT_BROWSER §5.2).

### 6.6. Лимиты

| Параметр | Значение |
|---|---|
| `RESUMPTION_TTL` | 1 час с issue (per-ticket) |
| `RESUMPTION_MAX_CHAIN_AGE` | = `SESSION_MAX_AGE` (24 часа) от `original_auth_at` — full re-auth обязателен |
| `RESUMPTION_RATE_LIMIT_PER_SUBNET` | 30/мин (унифицировано с AUTH §8 на subnet) |
| `TICKET_KEY_ROTATION` | 24 часа (overlap 24 часа) |

После `original_auth_at + RESUMPTION_MAX_CHAIN_AGE` ticket не выдаётся → клиент должен сделать full re-auth.

---

## 7. Disabling Resumption

7.1. **Server config** `--no-resumption` — server не выдаёт tickets, отвергает `resume`.

7.2. **Per-user revocation** через `revokeUserTickets` admin command — `user.tickets_invalid_before = now`. Все existing tickets invalidated.

7.3. **Per-session** через `kickSession` — также обновляет `tickets_invalid_before`.

7.4. **Global emergency** через `revokeAllTickets` — rotates `ticket_key` без overlap (`previous = NULL`).

---

## 8. Errors

| Error | Trigger |
|---|---|
| `resumption_failed` | Generic — expired, invalid, replayed, downgrade attempt, ticket_key mismatch |
| `resumption_disabled` | Server config disabled |
| `rate_limited` | Resume rate exceeded per subnet |

---

## 9. Audit Events

См. IMPLEMENTATION_GUIDE.md §3.

- `resumption_used` (sampled)
- `resumption_replay_detected` (always)
- `resumption_downgrade_blocked` (always — security event)
- `revoke_user_tickets`
- `revoke_all_tickets`
- `rotate_ticket_key`

---

## 10. Implementation Notes

10.1. **Tickets и Session — разные структуры.** Ticket = stateless wire. Session = per-connection state.

10.2. **monotonic_counter persistence:** **SYNCHRONOUS** (fsync) перед `resume_ok`. См. §6.2. Не batched (это было design error в predшествующей версии spec).

10.3. **Atomic CAS in DashMap:** `entry().and_modify(|c| if ticket.counter > *c { *c = ticket.counter; consume_ok = true })` + immediate persist.

10.4. **Tests (release blocker):**
- Round-trip: issue → resume → new session active
- Replay: same ticket twice → second `resumption_failed`
- Expired (past `expires_at`): fail
- Downgrade: TLS-bound ticket on plain transport → fail
- Upgrade: browser ticket on TLS-exporter transport → ok
- Cross-transport same tier: TCP↔WS → ok
- Counter persistence: resume → server crash → next attempt with same counter → fail
- `tickets_invalid_before` обновлён → existing ticket: fail
- `refreshTicket` сохраняет `original_auth_at` → kickSession инвалидирует всю цепочку
- Emergency `revokeAllTickets` → all existing tickets fail
- Key rotation: ticket issued under previous key resumes after rotation; invalid после `previous = NULL`
- AAD tampering: modify aad bytes → GCM fail
- Plain ticket upgrade: with `disable_plain_ticket_upgrade=true` → fail; without → ok

---

## 11. Browser-specific

См. CLIENT_BROWSER.md §5.

**Critical:** browser **НЕ** хранит ticket в `localStorage` / `sessionStorage` / `IndexedDB` / cookies. Ticket — **только** в memory JS variable. Tab close = full re-auth (~2с Argon2id, acceptable trade-off против XSS escalation).
