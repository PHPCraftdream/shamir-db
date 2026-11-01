# Admin UI Hosting

HTTP-сервер для **раздачи статической admin SPA** + REST endpoints для управления **уже-выданной** сессией. **Не** primary auth transport. Auth — через WS или TCP (см. TRANSPORT_WS.md, TRANSPORT_TCP.md).

(Этот документ заменяет removed `TRANSPORT_HTTP.md`.)

> **Поправка Token API.** REST-набросок ниже (`POST /shamir/v1/query` и `/shamir/v1/admin/<command>` с `session_id` как Bearer) **заменён** для REST-доступа документом **TOKEN_API.md**: REST-клиенты используют API-токены (`POST /shamir/v1/request`), токен никогда не бывает администраторским. `session_id` как Bearer **не принимается** ни одним listener-ом. Административные REST-команды `/shamir/v1/admin/<command>` остаются **нереализованной спецификацией** и этой фичей не обслуживаются. Статическая раздача admin SPA (§4, §5) не меняется. Токен-listener (`kind: http`) не отдаёт `/admin/*`, `/shamir/v1/health`, `/shamir/v1/version` и метрики.

---

## 1. Endpoints

```
GET  /admin/                        — admin SPA index.html
GET  /admin/static/*                — bundle (JS/CSS/WASM)
GET  /shamir/v1/health              — liveness probe (no auth)
GET  /shamir/v1/version             — version, supported protocol versions (no auth)
GET  /admin/metrics                 — Prometheus metrics (Bearer admin session) — НЕ РЕАЛИЗОВАН: метрики отдаёт observability-listener (`observability.addr`, `/metrics`)
POST /shamir/v1/query               — REST query (Bearer session_id) — ЗАМЕНЁН: TOKEN_API.md §9
POST /shamir/v1/admin/<command>     — admin commands (Bearer admin session_id) — НЕ РЕАЛИЗОВАН
```

WS upgrade endpoints `/shamir/v1` и `/shamir/v1/browser` обслуживаются SCRAM-listener-ами `kind: ws` (см. TRANSPORT_WS.md), токен-listener `kind: http` их не обслуживает.

---

## 2. Auth Flow

> **Заменено для REST (TOKEN_API.md).** Описанный ниже поток «session_id как Bearer» не реализуется и не принимается. Остаётся как историческое описание admin-UI эскиза.

**HTTP не делает primary auth.** Клиент:
1. Получает session_id через WS-handshake (либо native либо browser endpoint).
2. Использует session_id как Bearer token для последующих REST вызовов.

Это унифицирует auth path: все сессии создаются одним flow (WS), HTTP только consumes их.

---

## 3. REST Request Format

> **Заменено (TOKEN_API.md §9).** Фактический REST-формат: `POST /shamir/v1/request`, `Authorization: Bearer shk1.<id>.<secret>`, статусы 401/403/400/413/415/429/503. Блок ниже не реализуется.

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

**Только msgpack** wire encoding (то же что и WS binary frames). Legacy text-encoding опция удалена в v1 — single encoding путь упрощает имплементацию и закрывает класс interop bugs. curl-friendliness достигается через CLI tool или `xxd`/`msgpack-cli` обёртки.

**Bearer не cookie:**
- Нет CSRF surface
- Browser admin UI хранит session_id в **memory only** (см. CLIENT_BROWSER.md §5)

---

## 4. Static Admin UI Delivery

### 4.1. Bundle structure

```
/admin/                          → index.html
/admin/static/main.<hash>.js     → app bundle
/admin/static/main.<hash>.css
/admin/static/argon2.<hash>.wasm → ~30 KB
```

URLs содержат content hash для cache busting + Subresource Integrity.

### 4.2. Server config

```toml
[admin_ui]
enabled = true                        # default false
addr = "0.0.0.0:7335"                 # отдельный listener
allowed_origins = ["https://admin.example.com"]   # для CORS preflight (не auth)
```

For same-origin browser login, keep the admin UI on its separate HTTPS listener and configure a reverse proxy to route `/shamir/v1/auth/begin`, `/shamir/v1/auth/finish`, and the TOKEN_API data paths actually used by the UI to the loopback `kind:http` listener; route SCRAM WebSocket paths only to their SCRAM listener. Configure the backend login listener according to TOKEN_LOGIN_HTTP §5.1. The edge terminates TLS, rejects HTTP, sanitizes/replaces XFF, preserves validated Host/Origin, and adds HSTS. Protect/isolate or authenticate the loopback backend; local processes can otherwise forge XFF or observe credentials. No CORS is added. Exempt the trusted proxy only from per-IP accept caps; global limits still apply.

### 4.3. Admin UI not used → endpoint disabled

Если admin UI не нужен — `enabled = false`, `/admin/*` возвращает 404.

---

## 5. Security Headers (mandatory для admin UI)

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

## 6. CORS

6.1. Admin UI fetches только same-origin → CORS не нужен.

6.2. REST `/shamir/v1/*` (admin-эскиз; токен-listener `kind: http` не выдаёт **никаких** CORS-заголовков, TOKEN_API.md §9):
- Default: same-origin only (нет CORS headers)
- Server config может разрешить ограниченные origins (см. §4.2)
- При configured origins — preflight для них только
- `Access-Control-Allow-Credentials: false` (используем Bearer не cookies)

---

## 7. TLS

7.1. **HTTPS only** для production. Plain HTTP — только loopback. **Исключение:** токен-listener `kind: http` с `tls: false` может по явному opt-in `allow_plaintext_remote` слушать не-loopback адрес; на нём принимаются только `read_only` токены, пока не задан `allow_plaintext_write` (TOKEN_API.md §11). Admin UI (`/admin/*`) этим исключением не пользуется.

7.2. TLS config — идентичен TRANSPORT_TCP §3 (TLS 1.3, no 0-RTT).

7.3. Browser обычно требует валидный CA cert. Решения:
- Self-signed с manual trust (dev / internal)
- Public CA cert (Let's Encrypt) — **не** меняет identity model: pin всё равно Ed25519 server key

---

## 8. Rate Limits

- `/shamir/v1/health`, `/shamir/v1/version`: 100/sec per IP
- `/admin/*`: 100/sec per IP (static delivery, не security-критично)
- `/shamir/v1/query`: per-session rate limit (server config) — заменено токен-REST, лимиты в TOKEN_API.md §5, §9
- `/shamir/v1/admin/*`: 10/sec per session — не реализован

---

## 9. Test Checklist

- GET /admin/ → CSP, HSTS headers presence
- GET /admin/static/argon2.wasm → Content-Type: application/wasm
- (заменено токен-REST, TOKEN_API.md §15) POST /shamir/v1/query без Bearer → 401
- (заменено) POST /shamir/v1/query с invalid Bearer → 401
- (заменено) POST /shamir/v1/query с expired session_id → 401, `session_expired`
- CSP violations при попытке inline script — browser blocks
- CORS preflight для allowed origin → headers present; для disallowed → no CORS headers
- WASM load via instantiateStreaming работает без `wasm-unsafe-eval`
- Subresource Integrity на bundle script tag
