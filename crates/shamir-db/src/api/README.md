# `api/` — legacy MessagePack command DTOs

This module is a thin, vestigial layer left over from an earlier client/server
prototype. It contains three serializable types used only by its own
round-trip tests:

- `Command` — `Put { key, value } | Get { key } | Del { key } | Execute { func, args }`
- `Request { request_id: u64, command: Command }`
- `Response { request_id: u64, result: Result<Option<UserValue>, String> }`

`UserValue` is `#[deprecated]` (see `shamir-types::types::value`); these types
exist only for backward-compatible round-trip checks of the original `Command`
shape.

## Where the real network surface lives

The production client/server protocol is **not** in this module. It lives in
the dedicated workspace crates:

- `shamir-server` — TCP/WS listeners, framing, connection handling, observability
- `shamir-connect` — SCRAM-Argon2id auth handshake, sessions, identity
- `shamir-transport-tcp`, `shamir-transport-ws`, `shamir-transport-udp` — wire transports

Inside the database, the canonical entry point is the **Batch API**:
`ShamirDb::execute(db_name, &BatchRequest) -> BatchResponse`. It covers reads,
writes, DDL, and auth ops as a single JSON/MessagePack-shaped surface, with
cross-query references via `{"$query": "@alias[].field"}`. See
[`shamir-engine::query::batch`](../../../../shamir-engine/src/query/batch/README.md).

This `api/` module does **not** wrap the Batch API and is not used by any
transport. New code should depend on `shamir-server` + `shamir-engine::query`
instead.
