# ShamirDB Roadmap

Future features beyond v1 spec. Не нормативные, не binding обещания.

> **Current state (2026-05-31):** the transactional layer is COMPLETE —
> SI + SSI + true serializability (phantom protection), single-batch and
> interactive multi-call, crash-safe, property-covered. See
> [`../PROJECT_STATE.md`](../PROJECT_STATE.md) for the full snapshot and
> [`NEXT_PHASES.md`](./NEXT_PHASES.md) for the post-Phase-A index. The items
> below are still-future (auth v1.1+, transports, PQ, cluster) plus the
> Database-Engine section (replication / sharding / backup tooling — not
> started). *(No "query-language v2": the query language is OQL by
> principle — see [`PLAN.md`](./PLAN.md) §3.)*

## Auth Protocol

### v1.1 (короткий горизонт)

- **HIBP-style breach check** при password set/change (online k-anonymity API ИЛИ offline static set)
- **Argon2id parameter auto-tuning** на старте сервера (benchmark под текущее железо)
- **Channel binding RFC 9266 формальное соответствие** — формальная attestation если interop требует
- **WebAuthn second factor** для admin operations (browser-native)
- **`expire_password_now` flag** в updateUser (требует field `password_must_change` в user record)
- **DPoP-like request signing** для session tokens — channel-bound MAC per request, защищает session_id от bearer-token theft
- **Privilege separation** — отдельный signer process для Ed25519 ops (RCE compartmentalization)

(Audit log HMAC chaining и Bootstrap token TTL configurable — перенесены в v1, см. IMPLEMENTATION_GUIDE.md §3.3 и AUTH_PROTOCOL.md §11.2.2.)

### v1.2

- **TRANSPORT_QUIC.md** — QUIC native binding. Переиспользует AUTH_PROTOCOL без изменений.
- **TRANSPORT_UDP.md** — UDP datagram binding (для embedded sensors / WireGuard-style overlay). Mandatory L1 (HMAC) per packet.
- **Unix socket transport** — file permissions = auth boundary, no SCRAM needed (отдельный mode).

### v2 (несовместимо)

- **Hybrid PQ identity:** Ed25519 + ML-DSA-65 (FIPS 204). Pin = `SHA256(ed25519_pub || mldsa_pub)`. Migration без breaking handshake (server поддерживает оба, клиент verify оба).
- **Hybrid PQ key exchange** в TLS (X25519+MLKEM768) — ждём rustls full support
- **FIPS profile:** alternative kdf=PBKDF2-HMAC-SHA256, signature=ECDSA P-256. Configurable.
- **Cluster mode:** shared SystemStore + sticky sessions OR distributed session store
- **OAuth/OIDC bridge** для SSO интеграций

## Database Engine (вне scope auth spec)

> **Query language: OQL — окончательно, никакого «v2».** Язык запросов —
> объектный (DTO: `Filter`/`ReadQuery`/`BatchRequest`), msgpack/JSON,
> строится типизированным билдером / `q!` / `filter!`. Текстового/SQL-
> фронтенда не будет **никогда** — это принцип, а не отсутствие парсера:
> «запрос-как-текст» порождает инъекции, парсер-баги/DoS, prepared-
> statement-церемонию и parse/plan-кеши; OQL делает их *структурно
> невозможными*. См. [`PLAN.md`](./PLAN.md) §3 (Resolved forks).

См. отдельные документы (TBD):
- Replication
- Sharding
- Backup tooling
- Migration tooling

---

Roadmap не binding — фичи могут переехать между версиями или быть отброшены.
