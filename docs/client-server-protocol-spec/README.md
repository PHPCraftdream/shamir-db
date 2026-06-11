# ShamirDB Protocol Specification

Спецификация transport-agnostic аутентификации и сессий. Один auth протокол, много транспортов.

## Принципы

1. **Простота** — каждый документ читается изолированно, ≤ 600 строк
2. **Универсальность** — auth не зависит от транспорта
3. **Security first** — все ревью-фиксы внутри (см. SECURITY_MODEL.md)
4. **Browser-friendly** — JS/WASM клиенты first-class

## Архитектура

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

## Документы

### Core (нормативные)
- **[AUTH_PROTOCOL.md](AUTH_PROTOCOL.md)** — handshake, key derivation, errors. Transport-agnostic.
- **[SESSION_RESUMPTION.md](SESSION_RESUMPTION.md)** — fast reconnect, anti-downgrade rules.
- **[SUBSCRIPTIONS.md](SUBSCRIPTIONS.md)** — live subscriptions v1.1: SubscribeOp/UnsubscribeOp, PushEnvelope, grant rejection codes, filter contract, early-buffer rule.

### Reference (informative)
- **[SECURITY_MODEL.md](SECURITY_MODEL.md)** — adversary model, threat coverage, non-guarantees.
- **[IMPLEMENTATION_GUIDE.md](IMPLEMENTATION_GUIDE.md)** — operational details (storage, observability, audit, recovery runbooks).

### Transport bindings
- **[TRANSPORT_TCP.md](TRANSPORT_TCP.md)** — TCP (TLS или plain loopback).
- **[TRANSPORT_WS.md](TRANSPORT_WS.md)** — WebSocket (wss; native + browser endpoints).
- **[ADMIN_UI_HOSTING.md](ADMIN_UI_HOSTING.md)** — static admin UI + Bearer REST.

### Clients
- **[CLIENT_BROWSER.md](CLIENT_BROWSER.md)** — browser SDK: WASM crypto, CSP, anti-XSS.

### Future (вне `docs/client-server-protocol-spec/`)
- **[../roadmap/ROADMAP.md](../roadmap/ROADMAP.md)** — v1.1+ planned features.

### Diagrams
- **[diagrams/](diagrams/)** — Mermaid sequence + state diagrams для всех flows. Renders на GitHub inline.

### Test vectors
- `test-vectors/auth_v1.json` — **release blocker** для v1 (см. AUTH_PROTOCOL §16).

## Версионирование

- `auth_init.version: u8` — major version **AUTH_PROTOCOL.md**. Единственная версия в handshake.
- Каждый документ имеет свою версию в header. Backward-compat = minor bump. Wire-breaking = major bump.
- Domain tags привязаны к version своего документа: `SHAMIR-TICKET-v2` может появиться без `SHAMIR-AUTH-v2`.
- Compatibility matrix — IMPLEMENTATION_GUIDE.md §9.

## Статус

**v1 — draft.** Ревью пройдены (3 итерации, 3 reviewer perspectives). Test vectors — TBD при имплементации.
