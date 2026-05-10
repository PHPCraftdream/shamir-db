# Browser support via WASM — analysis & implementation plan

Записка о том, как поддерживать браузеры через компиляцию `shamir-connect`
в WebAssembly. Покрывает размер бандла, performance, channel binding в
браузере, и поэтапный план интеграции.

## Контекст

Спека определяет два WS-эндпойнта (TRANSPORT_WS §2):

- `/shamir/v1` (native, `binding_mode = 0x01`) — TLS exporter channel
  binding. Используется native-клиентами (Rust/Go/Python).
- `/shamir/v1/browser` (browser, `binding_mode = 0x02`) — без exporter,
  потому что **браузер JS не имеет доступа к TLS exporter** (RFC 9266 не
  exposed в WebCrypto API).

Браузерному клиенту нужен крипто-движок: Argon2id, HMAC-SHA256,
SHA-256, AES-256-GCM, HKDF, Ed25519. Современный WebCrypto покрывает
часть (HMAC/SHA/AES, скоро Ed25519), но **не Argon2id**. WASM
неизбежен для KDF.

## Готовность кода

`shamir-connect` (client-side) — pure Rust без OS-зависимостей.
Используемые крейты компилируются в WASM as-is:

- `argon2` ✅
- `hmac`, `hkdf`, `sha2` ✅
- `ed25519-dalek` ✅
- `aes-gcm` ✅
- `subtle`, `zeroize` ✅
- `rand` (через `getrandom = { features = ["js"] }`) ✅
- `rmp-serde`, `serde`, `serde_bytes` ✅
- `precis-profiles` ✅ (Unicode tables — ~60 KB)

Никаких архитектурных правок не требуется — `shamir-connect-wasm`
будет тонкой `wasm-bindgen`-обёрткой над существующим API.

## Размер WASM-бандла (оценка)

| Компонент | Raw | Gzip |
|-----------|-----|------|
| `argon2` | ~30 KB | ~10 KB |
| `ed25519-dalek` | ~40 KB | ~14 KB |
| `aes-gcm` | ~20 KB | ~7 KB |
| `sha2` + `hmac` + `hkdf` | ~8 KB | ~3 KB |
| `rmp-serde` + `serde` | ~30 KB | ~10 KB |
| `precis-profiles` (Unicode tables) | ~60 KB | ~20 KB |
| `shamir-connect` логика | ~20 KB | ~7 KB |
| **Итого** | **~210 KB** | **~70 KB** |

Сравнение:
- `rustls` в WASM ≈ 500 KB raw — наш в 2.5× легче.
- Современные SPA возят 1-2 MB JS — 70 KB gzip незаметно.

Дополнительное снижение:
- `wasm-opt -Oz` (binaryen) — обычно −15-20%.
- `wee_alloc` или `dlmalloc` вместо std allocator — −10 KB.
- Dynamic load Argon2id отдельным chunk'ом, грузить при login — early
  page load не страдает.

## Производительность

| Операция | Native x86 | WASM Chrome (V8 + SIMD) | WASM Mobile Safari |
|----------|-----------|------------------------|--------------------|
| Argon2id (128 MB / 4 iter) | ~2 сек | ~3-5 сек | ~5-10 сек |
| HMAC-SHA256 (256B) | ~µs | ~5 µs | ~10 µs |
| Ed25519 verify_strict (200B) | ~50 µs | ~150 µs | ~300 µs |
| AES-256-GCM (256B) | ~3 µs | ~10 µs | ~25 µs |

### Главная боль: Argon2id на мобильном

5-10 сек на iPhone Safari блокирует UI и плохо для UX. Решения:

1. **Browser-specific kdf_params**: `memory_kb = 65536` (64 MB вместо
   128 MB). Время: ~2-3 сек на мобильном. Спека §3.7.2 floor = 19 MB,
   так что 64 MB всё ещё в 3× выше floor — security acceptable. Можно
   сделать per-listener config: `BROWSER_KDF_PARAMS` отдельно от
   `DEFAULT_KDF_PARAMS`.
2. **Web Worker**: запускать Argon2id в worker'е — UI остаётся
   responsive, можно показать прогресс-бар. Стандартный паттерн
   для wasm-bindgen.
3. **WebCrypto где можно**: SHA-256, AES-GCM, HMAC через WebCrypto
   API — нативная скорость (близко к native x86). Argon2id остаётся в
   WASM (не в WebCrypto). Гибридная архитектура.

### Per-request post-handshake

В steady state (после handshake):
- HMAC + SHA + ConstantTimeEq микросекунды × 2-3 = всё ещё <100 µs.
- Argon2id уже не вызывается.
- AES-GCM (для resumption ticket) — единицы µs.

Per-request CPU **невидимо** для пользователя.

## Channel binding в браузере

Сценарий выбора:

### Вариант A — `binding_mode = 0x02` (TlsNoExport)

Текущий спека-подход. WSS обеспечивает encryption + integrity на
транспорте; `tls_exporter_or_zeros = [0u8; 32]`.

**Защищает от:**
- Passive eavesdropping (WSS encrypts).
- Active MITM с подменой server cert при условии корректного pin (Ed25519
  pin detect).
- Replay захваченных handshakes (per-handshake nonce + timestamp).

**НЕ защищает от:**
- MITM с **compromise CA** или **HSTS bypass** + transparent relay через
  legitimate cert. Этот сценарий редкий (CA compromises detected via
  Certificate Transparency), но не невозможен.

Anti-downgrade matrix спеки §6.4 не даёт browser-выписанному ticket'у
resume в native-сессию — атакующий не может «апгрейднуть» свой weaker
binding до stronger.

**Для большинства production сценариев этого достаточно** (это и есть
классический WSS).

### Вариант B — WASM + Noise NK для full channel binding

Дополнительный слой поверх WSS:

- WASM реализует Noise NK pattern (snow крейт, ~50 KB).
- Inside WS BINARY messages: noise handshake → all subsequent messages
  encrypted+MAC'd.
- `noise_handshake_hash[..32]` ↔ `tls_exporter_or_zeros` маппинг.
- Новое значение `binding_mode = 0x03` (Noise).

**Compound defense:** даже если WSS взломан (compromise CA + transparent
MITM relay), inner Noise защищает. Атакующий должен сломать **И** TLS
**И** Noise — оба независимо.

WASM bundle increase: +~50 KB (snow + curve25519-dalek). Total ~120 KB
gzip — всё ещё легче rustls-in-WS.

**Стоимость**: новый crate `shamir-secure-channel`, новая spec section
§6.6 «Noise NK profile», ~3-4 недели работы.

## Архитектура

```
┌─ Browser ─────────────────────────────────────────────────┐
│                                                           │
│  ┌─ JS thin wrapper ──────────────────────────────────┐  │
│  │  - WebSocket API (wss://server/shamir/v1/browser)  │  │
│  │  - msgpack frame helpers                           │  │
│  │  - WebCrypto: SHA, HMAC, AES-GCM (где возможно)    │  │
│  │  - Web Worker для Argon2id (не блокирует UI)       │  │
│  └────────────────┬───────────────────────────────────┘  │
│                   │ wasm-bindgen API                      │
│  ┌─ shamir-connect-wasm (cdylib) ────────────────────┐   │
│  │  - HandshakeBuilder / ClientHandshake             │   │
│  │  - Argon2id, Ed25519, AES-GCM, HMAC primitives    │   │
│  │  - PRECIS UsernameCaseMapped                      │   │
│  │  - msgpack envelope encode/decode                 │   │
│  │  Бандл: ~210 KB raw, ~70 KB gzip                  │   │
│  └────────────────────────────────────────────────────┘  │
└──────────────────────┬───────────────────────────────────┘
                       │ wss://
                       │ binding_mode = 0x02
                       ▼
              ┌── ShamirDB Server ──┐
              │  shamir-connect     │
              │  (server-side, Rust)│
              └─────────────────────┘
```

### TypeScript API (target)

```typescript
import { HandshakeBuilder, init } from '@shamir/connect';

await init();  // load WASM

const hs = new HandshakeBuilder('alice', pinnedSha256Hex)
    .bindingMode('TlsNoExport')
    .build();

const ws = new WebSocket('wss://server.example.com/shamir/v1/browser');
ws.binaryType = 'arraybuffer';

ws.onopen = () => {
    ws.send(frame(hs.authInit()));
};

ws.onmessage = async (e) => {
    const msg = unframe(new Uint8Array(e.data));
    if (hs.state() === 'awaiting_challenge') {
        // Argon2id ~2 сек — в Web Worker:
        const proof = await runInWorker(() => hs.processChallenge(msg, password));
        ws.send(frame(proof));
    } else if (hs.state() === 'awaiting_auth_ok') {
        const sessionId = hs.processAuthOk(msg);
        // Авторизация прошла. Дальше — application requests.
        // Опционально: верификация identity_sig_previous если
        // auth_ok содержит rotation_in_progress (orphan recovery).
    }
};
```

## Поэтапный план

### Now (закрыть v1 release)

- [v1 #9 follow-up] **`WsListenerProfile` enforcement** в
  `shamir-transport-ws` (по образцу `shamir-transport-tcp::listener`):
  - `Wss` (default) — отказывает в принятии non-TLS потоков.
  - `WssBrowser` — TLS обязателен + Origin check.
  - `PlainWsLoopback` — ws:// разрешён только на 127.0.0.0/8 / ::1.
- Документировать в spec TRANSPORT_WS: «`ws://` requires loopback or
  inner secure channel» (как plain TCP в §2.2).

### v1.1 — WASM browser SDK (~1-2 недели)

- [ ] Создать crate `crates/shamir-connect-wasm` (cdylib).
- [ ] `wasm-bindgen` API: `HandshakeBuilder`, `ClientHandshake`,
  `verify_rotation_in_progress`, util-функции (encode/decode envelope,
  msgpack helpers).
- [ ] Browser-profile `KdfParams` (64 MB / 4 iter): добавить в
  `common::kdf_params` константу `BROWSER_KDF_PARAMS`. Spec §3.7
  расширить: per-listener kdf_params allowed iff above floor.
- [ ] Web Worker integration sample (один файл worker.js,
  postMessage-based API).
- [ ] Hybrid с WebCrypto где возможно: thin JS layer вокруг WASM
  использует `crypto.subtle.digest` / `.encrypt` / `.sign` для
  одноразовых SHA / AES / HMAC операций; Argon2id остаётся WASM-only.
- [ ] TypeScript .d.ts через `wasm-bindgen --typescript`.
- [ ] Browser integration tests: `wasm-bindgen-test` (Chrome/Firefox
  headless) + Playwright (Safari).
- [ ] CI: `wasm-pack build` + `wasm-opt -Oz` для production.
- [ ] npm publish как `@shamir/connect`. Subresource Integrity hash в
  install instructions.

Бандл-ожидания после `wasm-opt -Oz`:
- Raw: ~150 KB
- Gzip: ~50 KB
- Brotli: ~40 KB

### v2 — Noise NK compound defense (~3-4 недели; опционально)

Только если есть конкретный use-case (compromise-CA threat model,
zero-trust network, embedded scenarios без TLS termination):

- [ ] Crate `crates/shamir-secure-channel` — Noise NK реализация поверх
  байт-стрима. Использовать `snow` + `curve25519-dalek`.
- [ ] WASM-сборка `shamir-secure-channel-wasm`. +50 KB gzip.
- [ ] Spec section §6.6 — «Noise NK profile»: byte layout handshake
  сообщений, mapping `noise_handshake_hash[..32] → tls_exporter_or_zeros`,
  `binding_mode = 0x03`.
- [ ] Diagram 13 — sequence Noise NK поверх WS / TCP.
- [ ] Anti-downgrade matrix update §6.4: Noise (strength=2) ≥ TLS-no-export
  (strength=1) ≤ TLS-exporter (strength=2). Resume policy.
- [ ] Server-side Noise dispatcher (после WS upgrade, до SCRAM).
- [ ] Integration tests: WSS+Noise compound, plain-TCP+Noise embedded.

### Decision points

1. **Делать WASM crate сейчас (v1.1)?** Я бы делал. Это естественный
   следующий шаг, ~1-2 недели работы, стандартизованный wasm-pack
   pipeline, добавляет браузерный target без изменения протокола.

2. **Делать Noise NK сейчас?** Я бы откладывал до v2. Нужны
   конкретные user stories («наш регулятор требует zero-trust
   network», «embedded device без TLS», «защищаемся от compromised
   CA»), а пока WSS + Ed25519 pin для подавляющего большинства
   сценариев достаточно.

## Open questions

- [ ] Profile enforcement в spec: добавить отдельный binding_mode для
  Noise или оставить 0x03 как «inner secure channel» обобщённое?
- [ ] Browser kdf_params как config флаг сервера или client signal в
  auth_init? (Server-driven via `challenge.kdf_params` уже работает —
  достаточно просто хранить per-user другой kdf_params для
  browser-зарегистрированных юзеров.)
- [ ] Resumption ticket в браузере — localStorage или
  sessionStorage? Spec говорит «memory-only для browser» (CLIENT_BROWSER
  §5.2). Нужно подтвердить и зафиксировать в WASM SDK API.
- [ ] Subresource Integrity для npm пакета — auto-publish workflow с
  reproducible builds?

## References

- docs/protocol/AUTH_PROTOCOL.md §3.7 — KDF parameters
- docs/protocol/TRANSPORT_WS.md — WebSocket binding (browser endpoint)
- docs/protocol/CLIENT_BROWSER.md — browser-specific guidance (memory-only ticket
  storage, etc.)
- [Noise Protocol Framework](https://noiseprotocol.org/)
- [`snow` crate](https://docs.rs/snow/) — production Noise impl
- [`wasm-bindgen` book](https://rustwasm.github.io/wasm-bindgen/)
- [RFC 9266](https://www.rfc-editor.org/rfc/rfc9266.html) — TLS exporter
  channel binding (the thing we lose in browser)
