# shamir-connect

ShamirDB connection protocol library — auth handshake + session management.

Implements the spec at `../../spec/AUTH_PROTOCOL.md`.

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

**v0.1 alpha** — under active TDD development. Tracks spec v1 (frozen).

Implementation order (per AGENTS.md):
1. Foundation: types, canonical auth_message, crypto wrappers
2. Test vectors loading + verification
3. Common: HKDF anti-enumeration, password normalization
4. Client: SCRAM derivation, mutual auth verify, pin check
5. Server: SCRAM verify, identity signing, session lifecycle
6. Resumption ticket: AEAD encrypt/decrypt, family counter
7. Integration tests: full handshake round-trip
