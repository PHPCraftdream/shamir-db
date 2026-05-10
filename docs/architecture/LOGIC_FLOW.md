# Logic Flow

A view from above of how a single request travels through ShamirDB —
which crate owns which slice of work, where the boundaries are, and
which background streams run alongside the main current.

For storage-engine internals, types, and indexes see
[ARCHITECTURE.md](./ARCHITECTURE.md). For the wire-level contract
between client and server see
[../client-server-protocol-spec/](../client-server-protocol-spec/).

---

## The forward path

**A drop arrives from outside** — bytes on a socket, still meaningless.
`shamir-transport-tcp` or `shamir-transport-ws` catch them in the TLS
cup; from there a TLS exporter is pulled — the thin channel-binding
thread that later layers will tug on for verification. This is the
first membrane.

**Through the membrane — `shamir-connect`.** Here is the handshake:
`auth_init → challenge → client_proof → auth_ok`. SCRAM-Argon2id
kneads the password down to a `stored_key`; Ed25519 signs the server's
own identity. The output is a `Session`. Sessions live in the
`SessionStore`, each with permissions, expiry, and a family-counter
for possible resumption tickets. Authentication and storage are not
yet "the database" — they are the contract of the connection.

**`shamir-server` is the dispatcher.** It bridges *connection* and
*database*. `dispatch_request_view` first checks §7.5 — has the
session been revoked (`tickets_invalid_before_ns`)? If alive, it
unwraps `RequestEnvelope` → `DbRequest`. The single coarse permission
check happens here: `is_admin? → require superuser`. After that it
hands the batch to `ShamirDb::execute`.

**`shamir-db` is the facade.** It does not run queries itself; it
wires the pieces together: `DbTableResolver` (db/repo/table → concrete
`TableManager`), `ShamirAdminExecutor` (DDL and auth-ops via
`SystemStore`), and lets the batch executor pick up from there.

**`shamir-engine` is where queries come alive.** `BatchRequest` is
parsed by the planner: queries with `{"$query": "@alias[].field"}`
references are ordered by their dependency graph; independent groups
run in parallel. Each operation dives deeper:

- *Read* → `ReadQuery + FilterContext` → `TableManager.read` →
  `IndexManager` picks a plan → collects `InternerKey`-encoded
  `InnerValue`s → resolves them back to strings via the interner →
  yields `Vec<Value>`.
- *Write* (Insert / Set / Update / Delete) → checks unique indexes
  *before* the write → writes through the `Store` → updates indexes
  *after* → returns the affected count.
- *Admin* (CreateDb / Repo / Table / Index) → `SystemStore` (itself
  a `DbInstance` backed by its own redb file) + `RepoManager` brings
  up a new backend.
- *Auth* (CreateUser / Role) → set/delete on the system `users` /
  `roles` tables.

**`shamir-storage` is the solid floor.** The `Store` trait moves
buckets of bytes: `insert / get / set / remove / iter_stream`.
Underneath it are seven implementations (Sled, Redb, Fjall, Nebari,
Persy, Canopy, InMemory + a Cached wrapper), each behind a Cargo
feature. There are no types, no queries here — only key/value.

**`shamir-types` is the foundation.** The Value model, JSON /
MessagePack codecs, `RecordId` (a 16-byte timestamped ULID), and the
`Interner` (`String → u64` for memory economy). Nothing above it
violates its abstractions, and it knows nothing of storage or
sessions.

**The ebb.** `BatchResponse` (records + stats + plan + transaction
info) → `DbResponse::Batch` → msgpack → `ResponseEnvelope` → frame
→ TLS → back to the client.

---

## Background streams

Running alongside the main current:

- **Scheduler** ticks: GC of stale sessions, rotation of `ticket_key`,
  eviction of expired lockout entries.
- **AuditAppender** accumulates events and flushes an HMAC-chained
  block into `audit_log.redb` at most every 5 seconds.
- **Argon2Semaphore** caps concurrent KDF computations — without it
  CPU DoS is trivial.
- **RateLimit / Backoff / Lockout** — three layers of brute-force
  defence: per-subnet sliding window, per-`(subnet, user_hash)`
  exponential backoff, durable lockout after 50 failures per hour.

---

## The shape

The project is a cone. The narrow neck on top is the wire (TLS,
frame, envelope). Going down it widens: handshake → session →
dispatch → batch → plans → many operations → many tables → many
backends. At the bottom is a `DashMap` or a redb file — a concrete
place where the bytes physically sit.

Each crate is a horizontal slice of the cone. Lower layers do not
know about higher ones; higher layers reach into lower ones only
through narrow traits (`Store`, `RequestHandler`, `RepoFactory`).
That is why feature flags work: a storage backend can be turned off
without anything above it breaking.

What is striking: **no layer does another layer's work.**
`shamir-connect` does not touch the database (it keeps only a
`UserDirectory` trait; the implementation lives in `shamir-server`).
`shamir-server` does not parse SQL (the batch is structured JSON,
parsed by `shamir-engine`). `shamir-engine` does not know what a
session is — it receives `SessionPermissions` from outside. The
boundaries stay clean.

---

## Notes on a few design choices

- `SystemStore` uses the same query operations (`SetOp`, `DeleteOp`,
  `ReadQuery`) as user databases — a deliberate recursion of the
  abstraction. The system store is itself a `DbInstance` with a single
  repo `system` and five tables (`databases`, `repositories`,
  `settings`, `users`, `roles`). The bootstrap chicken-and-egg is
  resolved in `ShamirDb::init()`, which opens the system store
  directly before the regular API is available. The reuse keeps the
  read/write path single — the alternative would be a parallel
  specialized binary path for system metadata.
- *Resumption tickets* are a system-within-a-system. The complexity is
  not accidental: each piece (rotating `ticket_key`, family-counter,
  durable `consumed_counters`, anti-downgrade matrix, TLS-exporter
  AAD, 24 h max chain age) defends against a distinct threat. See
  `docs/client-server-protocol-spec/SESSION_RESUMPTION.md` for the
  full mapping.
- All three layers — DTOs (`shamir-query-types`), pure planning, and
  runtime execution — are now properly separated. The batch planner
  and `$query` reference parser live in `shamir-query-types::batch`
  next to the DTOs; only the executor (which actually drives a
  `TableManager`) stays in `shamir-engine::query::batch::executor`.

---

## Silence in the code

Well-named code carries almost no comments. Where a comment exists,
it points at an invariant that is not visible from the surface — a
library quirk worked around, a constraint enforced elsewhere, a race
that *would* happen if you reordered two lines. Big docstrings are
rare. The structure is meant to be read directly, layer by layer.
