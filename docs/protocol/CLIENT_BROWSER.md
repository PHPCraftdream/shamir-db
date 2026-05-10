# Browser Client SDK

Гайд для имплементации SCRAM-Argon2id auth в browser. Цель: **браузер не уступает по безопасности native клиенту** в той мере, в какой это позволяет WebCrypto API.

---

## 1. Crypto Stack

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

## 2. Зависимости (npm)

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

## 3. Critical Implementation Details

### 3.1. Constant-time comparison

```javascript
import { equalBytes } from '@noble/ciphers/utils';

if (!equalBytes(server_signature, expected_signature)) {
  throw new Error('server_authentication_failed');
}
```

`equalBytes` гарантирует branch-free сравнение.

### 3.2. Ed25519 strict verify

`@noble/ed25519` v2 default — RFC 8032 strict (small-subgroup rejection):

```javascript
import * as ed from '@noble/ed25519';

const ok = await ed.verifyAsync(signature, message, publicKey);
if (!ok) throw new Error('server_signature_invalid');
```

⚠️ Lock major version: `"@noble/ed25519": "^2"`. v1 имела разные defaults.

### 3.3. Argon2id — единые параметры

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

### 3.4. UI freeze — Web Worker mandatory

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

### 3.5. Zeroize

JS не даёт гарантий памяти. Best-effort:
- Хранить sensitive в `Uint8Array`, после использования `arr.fill(0)`
- Не передавать пароль как `String` в долгоживущие переменные (immutable, остаются в string pool)
- Custom error types которые не serialize secrets (`toJSON` возвращает `<REDACTED>`)
- Никогда `console.log(password)` или подобное в коде

---

## 4. Channel Binding (browser-mode)

WebCrypto **не предоставляет** TLS exporter. Любой WS-клиент в браузере не может вычислить `EXPORTER-ShamirDB-AUTH-v1`.

### 4.1. Resolution

Browser клиент шлёт `auth_init.binding_mode = 0x02` ("tls_no_export"). Сервер на browser endpoint (`/shamir/v1/browser`, см. TRANSPORT_WS §1) принимает это значение. На native endpoint — отказ.

В `auth_message`:
```
binding_mode = 0x02
tls_exporter_or_zeros = bytes(32) zeros
```

Это **явный** policy signal — не UA-detection. MITM не может switch'нуть browser клиента в стronger режим (нет API) или ослабить native клиента (server policy на endpoint).

### 4.2. Trade-off — honest assessment

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

### 4.3. Anti-downgrade в resumption

Browser sessions создают tickets с `binding_mode_at_auth = 0x02`. По SESSION_RESUMPTION §6.1:
- Browser ticket → native endpoint = upgrade ALLOWED
- Native (TLS exporter) ticket → browser endpoint = downgrade BLOCKED

То есть украденный browser ticket НЕ может быть escalated в native session.

---

## 5. CSP and XSS Defense

### 5.1. CSP (server-side, см. ADMIN_UI_HOSTING §5)

Strict CSP без `unsafe-inline`, БЕЗ `wasm-unsafe-eval`. WASM грузится через `instantiateStreaming(fetch(...))`.

### 5.2. JS Storage

**НИКОГДА не использовать:**
- `localStorage` / `sessionStorage` — для **любых** secrets (session_id, ticket, password, ключи)
- `IndexedDB` без encryption (too complex для v1)
- Cookies (см. ADMIN_UI_HOSTING §3)

**Использовать:**
- Memory-only state в JS variable closure-scope админ приложения
- Resumption ticket — **только в memory** (см. SESSION_RESUMPTION §11). Tab close = re-auth (~2с Argon2id, acceptable).

⚠️ **Изменение от предыдущей версии spec:** ticket НЕ хранится в sessionStorage. XSS = leak ticket = takeover был неприемлемый trade-off. Теперь tab close = full re-auth.

### 5.3. Bundle integrity

- Bundle path содержит SHA256 hash для cache busting + integrity
- Server отдаёт с правильным `Content-Type`
- WASM с `Content-Type: application/wasm`
- Subresource Integrity на `<script>` и `<link>`

### 5.4. Input sanitization

Все user-supplied data (включая username отображаемый в UI) — escape через template literal-style (React/Vue/Svelte автоматом). **Не использовать `innerHTML` / `dangerouslySetInnerHTML`**.

---

## 6. Connection Code (Pseudo-API)

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

## 7. Browser Compatibility

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

## 8. Mobile

8.1. **Battery / CPU:** Argon2id 2 секунды на mobile = заметно, но acceptable. **Один global default**, не adaptive (см. §3.3 — anti-fingerprinting trade-off).

8.2. **NAT rebinding:** мобильные сессии могут менять IP. Resumption tickets решают это **в пределах одной вкладки** (memory-only).

8.3. **WebView (in-app browsers):** ограничения. WASM работает, performance variable.

8.4. **PWA:** admin UI можно установить как PWA (manifest.json). Не меняет security model.

---

## 9. Test Checklist (release blockers)

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

## 10. Future (см. ROADMAP.md)

- WebAuthn second factor для admin
- WebTransport API когда TLS exporter станет доступен в browser
- Service Worker для offline UI shell
- Push notifications для session events
