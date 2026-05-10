# ShamirDB Session Resumption v1

Быстрый reconnect (~10ms вместо ~2s Argon2id). Поддержка cross-transport с **анти-downgrade защитой**. Multi-device через **ticket families** (одно устройство не invalidates ticket другого).

---

## 1. Когда полезно

- Mobile NAT rebinding (новый TCP/IP)
- Cross-transport switch (TCP → WS) **в пределах одного security tier**
- CLI tools — короткоживущие команды
- Failover к replica
- **Multi-device:** laptop refresh не ломает mobile session

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
  "ticket_family_id": bytes(16),          // см. §6.2 — per-device lineage
  "original_auth_at_ns": u64,             // unix nanos; ВСЕГДА от первого full SCRAM, не обновляется при refresh
  "expires_at_ns": u64,
  "family_counter": u64                   // monotonic в пределах family_id
})
```

`ticket_plain` использует **canonical msgpack** (lex-sorted keys, smallest int encoding, no NaN) — потому что AAD валидация и detection tampering зависят от bit-exact bytes.

**Why `_ns` (nanoseconds)** вместо seconds: устраняет race window в 1 секунду между `original_auth_at` и `tickets_invalid_before` (см. §6.3).

### 2.2. Wire format

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

### 5.7. Resumption во время identity rotation

**Determination rule** [NORMATIVE]: ticket считается "issued под previous keypair" если:
```
ticket_plain.original_auth_at_ns < (server_meta.server_ed25519_rotation_until_ns - 7 days_in_ns)
```
То есть ticket_plain.original_auth_at_ns раньше начала текущего overlap window (current rotation начался в `rotation_until_ns - 7 days`). `ticket_plain` сам не содержит epoch field — server derive из timestamp.

Если ticket был issued под `previous` Ed25519 keypair и `transition_until_ns > now_ns`:

**v1 поведение:** server отвергает resume → клиент выполняет full re-auth.

После full re-auth client получает `auth_ok` с **`rotation_in_progress`** payload (см. AUTH §6.5) → handles pin update согласно §6.5 (interactive prompt mandatory или `--accept-rotation` flag для non-interactive). Только после успешного pin update сессия используется.

Цена: клиенты со stale tickets во время окна rotation теряют ~2 секунды (Argon2id full re-auth) на первое подключение + interactive prompt time. Acceptable trade-off против complexity multi-key signing в resume_ok.

После окна rotation (`now_ns > transition_until_ns`) resumption работает нормально с current keypair. `rotation_in_progress` отсутствует в `resume_ok` schema (§5.6) — только в `auth_ok`.

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

**Семантика:** ticket выпущенный в TLS-exporter сессии не может быть resumed в plain или browser сессии. Browser ticket по умолчанию **может** быть resumed в native (upgrade OK). Plain ticket — куда угодно.

**Strict mode** через server config `allow_browser_ticket_upgrade = false` (default `true`):
- Browser ticket (binding_mode=0x02) НЕ может быть resumed в native (binding_mode=0x01)
- Закрывает hypothetical pivot: атакующий с украденным browser ticket (XSS) → resume в native session с stronger trust
- Trade-off: legitimate user не может upgrade с browser session на native CLI без re-auth
- Recommended для high-security deployments

### 6.2. Anti-replay через per-family monotonic counter

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

### 6.3. Ticket invalidation

Сервер invalidates **все** tickets **всех families** юзера через `tickets_invalid_before_ns` (см. AUTH_PROTOCOL §3.5). Triggers:
- `kickSession` admin command (§12.4 AUTH)
- `updateUser` с new roles (§12.5 AUTH)
- Manual `revokeUserTickets` admin command (см. §7)

При resume (§5.4 step 6): `ticket.original_auth_at_ns > user.tickets_invalid_before_ns` обязательно. **Строгое `>`** (не `>=`) исключает race window даже если timestamp resolution коллидирует.

`original_auth_at_ns` НЕ обновляется при `refreshTicket` (§4.2) — это обеспечивает что вся цепочка refreshed tickets (вся family) разом invalidates.

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

7.2. **Per-user revocation** через `revokeUserTickets` admin command — `user.tickets_invalid_before_ns = now_ns`. Все existing tickets invalidated.

7.3. **Per-session** через `kickSession` — также обновляет `tickets_invalid_before_ns`.

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

## 11. Browser-specific

См. CLIENT_BROWSER.md §5.

**Critical:** browser **НЕ** хранит ticket в `localStorage` / `sessionStorage` / `IndexedDB` / cookies. Ticket — **только** в memory JS variable. Tab close = full re-auth (~2с Argon2id, acceptable trade-off против XSS escalation).
