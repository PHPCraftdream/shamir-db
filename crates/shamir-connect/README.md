# shamir-connect

ShamirDB connection protocol library — auth handshake + session management.

Implements the spec at `../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md`.

## Modules

- **`common`** — shared types, canonical `auth_message` builder, crypto primitive wrappers, error types. Used by both `client` and `server`.
- **`client`** (feature: `client`) — SCRAM client computation, server identity verification, pin checking.
- **`server`** (feature: `server`) — SCRAM verification, fake-blob anti-enumeration, identity signing, session management.

## Features

- `default = ["client", "server"]` — both modules
- `client` — only client SDK (smaller binary, no server-only deps)
- `server` — only server SDK

## Usage

```toml
[dependencies]
shamir-connect = "0.1"                     # client + server
shamir-connect = { version = "0.1", default-features = false, features = ["client"] }  # client-only
```

## Status

**v0.1 alpha** — TDD-developed against spec v1 (frozen). The full handshake,
session lifecycle, password rotation, and resumption tickets are all
implemented end-to-end with integration coverage in `tests/`.

Implemented surface (see source layout under `src/`):

- **common/** — canonical `auth_message` builder, crypto wrappers,
  fake-blob anti-enumeration, password / username (PRECIS) normalization,
  envelope, KDF params, identity, rotation primitives, latency helpers
- **client/** — bootstrap, SCRAM handshake, password change, key rotation
- **server/** — bootstrap, handshake, SCRAM verify, session manager, resume
  tickets, password change, key rotation, audit chain, lockout / rate limit /
  Argon2 semaphore, durable counters, admin ops, dispatch

Integration tests under `tests/` exercise full round-trips for bootstrap,
handshake, resume, rotation, password change, session lifecycle, admin ops,
log redaction, and PRECIS username handling.
