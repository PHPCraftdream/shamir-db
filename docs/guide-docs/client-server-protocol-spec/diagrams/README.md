# ShamirDB Protocol Diagrams

Visual reference for ShamirDB Auth Protocol v1. Diagrams в Mermaid syntax — рендерятся inline на GitHub, могут быть экспортированы в SVG/PNG через `mermaid-cli`.

## Navigation

### Auth flows (sequence diagrams)
- [01 — Initial Auth (full SCRAM)](01-initial-auth.md) — основной auth flow с channel binding и Ed25519 identity verify
- [02 — Resumption](02-resumption.md) — fast reconnect через ticket с anti-downgrade и family counter
- [03 — Bootstrap](03-bootstrap.md) — создание первого admin (out-of-band pin mandatory)
- [05 — Identity Rotation](05-identity-rotation.md) — broadcast active sessions + orphan recovery
- [06 — Update User](06-update-user.md) — admin command с persist barrier и per-request validity

### Lifecycle & architecture
- [10 — Session Lifecycle](10-session-lifecycle.md) — state diagram (pre-auth → active → eviction)
- [11 — Component Overview](11-component-overview.md) — high-level архитектура клиент/сервер/transports

### Decision tables
- [12 — Anti-Downgrade Matrix](12-anti-downgrade-matrix.md) — binding_strength правила, cross-transport scenarios

## Conventions

- **Actors:** `Client` (C), `Server` (S), `TLS` layer, `SystemStore` (DB)
- **Messages:** `->>` request, `-->>` response, `-x` connection close
- **Notes:** internal computation steps (Argon2id, HKDF, verify)
- **Colors:** `rect rgb()` для grouping security-critical sections

## Building

```bash
# Render single diagram to SVG
npx @mermaid-js/mermaid-cli -i 01-initial-auth.md -o 01-initial-auth.svg

# Or use Mermaid Live Editor: https://mermaid.live
```

## Sync с спекой

Каждая диаграмма ссылается на конкретные секции спеки в комментариях. При изменении wire format / state machine — диаграмма должна обновляться синхронно. CI gate (TBD): assert что все sequence diagrams ссылаются на existing sections.
