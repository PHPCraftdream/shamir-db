# Live Subscriptions — Wire Format v1.1

> Status: v1.1 — normative. Document version is independent of `auth_init.version`.
>
> This document describes the **wire format** of S.H.A.M.I.R. live subscriptions: the
> `SubscribeOp` / `UnsubscribeOp` ops, the `PushEnvelope` push frames, the grant shape,
> and the semantics each side MUST observe. It is language-agnostic. For builder code
> that produces these shapes, see §14 References.

---

## 1. Overview

Subscriptions let a connected client receive server-initiated change notifications for
one or more tables in a single repository. They are issued as ordinary batch ops
(`SubscribeOp`) inside a `BatchRequest`, and the server answers with:

1. A normal `BatchResponse` carrying a **grant** (which assigns a `sub` id), and
2. An open-ended stream of **`PushEnvelope`** frames addressed by that `sub` id.

Push frames are distinguished from request/response traffic by their envelope key:

| Frame              | Discriminator key | Carries     |
|--------------------|-------------------|-------------|
| `RequestEnvelope`  | `rid`             | request id  |
| `ResponseEnvelope` | `rid`             | response id |
| `PushEnvelope`     | `push`            | push kind   |

Subscriptions are scoped to one connection. They terminate on `UnsubscribeOp`,
connection close, slow-consumer breach, or any bridge-side fatal error. Termination is
always announced by a `closed` push (best-effort).

---

## 2. Lifecycle

```
client                                            server
  │  Batch{ subscribe: SubscribeOp }                │
  │ ──────────────────────────────────────────────► │
  │                                                 │── activate bridge ─┐
  │                                                 │                    │
  │  BatchResponse{ results[alias].value.sub = N }  │                    │
  │ ◄────────────────────────────────────────────── │                    │
  │                                                 │                    │
  │  (any of the below MAY arrive before the        │                    │
  │   BatchResponse — see §11 Early-buffer rule)    │                    │
  │                                                 │                    │
  │  push=event   (×N, only if `initial: true`)     │                    │
  │ ◄─────────────────────────────────────────────  │ ◄── snapshot ──────┘
  │  push=ready   (only if `initial: true`)         │
  │ ◄─────────────────────────────────────────────  │
  │  push=event   (live changes, indefinitely)      │
  │ ◄─────────────────────────────────────────────  │
  │  push=gap         (optional, on lag/resume)     │
  │  push=slow_consumer (optional, on backpressure) │
  │  push=closed      (terminal, best-effort)       │
```

Ordering rules:

1. If `from_version` is set, any `gap` from journal-backfill is emitted **before** any
   backfilled events.
2. If `initial: true`, all snapshot `event` frames precede the single `ready` frame.
3. Live `event` frames begin only after the snapshot watermark is seeded (see §13).
4. `seq` is monotonic per subscription. It is only advanced on **successful** push, so
   the client never observes a hole from transient backpressure drops.
5. `closed` is emitted on bridge exit for any reason (server unsubscribe, slow consumer,
   stream end, internal error). It is best-effort and MAY be missed if the transport
   has already failed.

---

## 3. SubscribeOp

Wire discriminator: `"subscribe"` key on the op.

```json
{
  "subscribe": [
    { "table": "users", "events": "all" }
  ],
  "deliver": "records",
  "initial": false
}
```

| Field          | Type                        | Required | Default     | Description                                                                                       |
|----------------|-----------------------------|----------|-------------|---------------------------------------------------------------------------------------------------|
| `subscribe`    | array of `SubscriptionSource` | yes    | —           | One or more sources to watch. All MUST resolve to the same repository (§7).                       |
| `deliver`      | `DeliverMode`               | no       | `"records"` | How events are projected before being pushed (§5).                                                |
| `initial`      | `bool`                      | no       | `false`     | If `true`, server emits a snapshot of current matching records, followed by a single `ready`.     |
| `from_version` | `u64`                       | no       | absent      | Resume cursor. Server replays journal events strictly after `from_version`. See §13.              |

`initial: true` and `from_version` are not mutually exclusive: journal backfill (if any)
runs first, then the snapshot, then the `ready` frame, then live events.

---

## 4. SubscriptionSource

```json
{ "table": "orders",            "events": "put",    "filter": { "field": "status", "eq": "open" } }
{ "table": ["hot", "sessions"], "events": "delete"                                                  }
```

| Field    | Type           | Required | Default | Description                                                  |
|----------|----------------|----------|---------|--------------------------------------------------------------|
| `table`  | `TableRef`     | yes      | —       | Either `"name"` (repo = `"main"`) or `["repo", "name"]`.     |
| `filter` | `Filter`       | no       | absent  | Server-side predicate. Evaluated only on `Put`; see §10.     |
| `events` | `EventMask`    | no       | `"all"` | `"all"` \| `"put"` \| `"delete"`.                            |

Multi-source rules:

- All sources MUST share the same repository. Mixed repos → the grant is rejected with
  `multi_repo_subscriptions_not_supported` (§7).
- The filter and event mask are evaluated **per source independently**. An event is
  delivered if *any* source matches.
- Duplicate `(table, mask)` entries are allowed; effective behavior is OR.

---

## 5. DeliverMode

Externally tagged. Exactly one of:

| Wire form                                | Variant   | Pushed payload (`data` field of PushEnvelope)                                  |
|------------------------------------------|-----------|--------------------------------------------------------------------------------|
| `"records"`                              | Records   | UTF-8 JSON `{ table, op, key, commit_version, value? }`. `value` only on Put.  |
| `"keys"`                                 | Keys      | UTF-8 JSON `{ table, op, key, commit_version }`. No value, both ops.           |
| `{ "batch": SubBatchOp }`                | Batch     | MessagePack-encoded `BatchResponse` of the reactive sub-batch.                 |
| `{ "call": CallOp }`                     | Call      | MessagePack-encoded `BatchResponse` wrapping the call result.                  |

Examples:

```json
"deliver": "records"
```

```json
"deliver": { "batch": { "batch": { /* BatchRequest */ }, "bind": { "$now": 1700000000 } } }
```

```json
"deliver": { "call": { "fn": "audit.log", "args": { "event": "$event.op" } } }
```

Exactly **one** `DeliverMode` per `SubscribeOp` — it applies to every source.

For `batch` and `call`, the following bindings are injected into the bind map of the
wrapper batch before execution, so the sub-batch / function can reference them:

| Binding                    | Type     | Source                              |
|----------------------------|----------|-------------------------------------|
| `$event.table`             | string   | the changed table                   |
| `$event.op`                | string   | `"put"` \| `"delete"`               |
| `$event.key`               | string   | hex-encoded raw key bytes           |
| `$event.commit_version`    | i64      | the commit version of the change    |

User-supplied bindings on the sub-batch take precedence only when their keys do not
collide with `$event.*`; collisions are overwritten by the injected values.

---

## 6. Grant

The server answers a `SubscribeOp` with a normal `BatchResponse`. The query result for
the subscribe alias has `value` set to a grant object:

```json
{
  "rid": 42,
  "results": {
    "my_sub": {
      "records": [],
      "value": {
        "subscription_grant": true,
        "sources_count": 1,
        "sub": 7
      }
    }
  }
}
```

| Field                 | Type   | Description                                                                |
|-----------------------|--------|----------------------------------------------------------------------------|
| `subscription_grant`  | bool   | Always `true` on success. Discriminates from regular query results.        |
| `sources_count`       | u64    | Number of sources in the original `SubscribeOp`.                           |
| `sub`                 | u64    | **Server-assigned routing id.** All subsequent `PushEnvelope` frames carry this `sub`. |

The `sub` id is unique per connection. It is the only identifier the server uses to
address push frames; the client's alias is not echoed.

`UnsubscribeOp` returns the same envelope shape with:

```json
{ "unsubscribe_grant": true, "sub_id": 7 }
```

---

## 7. Grant rejections

Grant validation runs synchronously during batch execution, before any bridge is
started. A rejected grant produces a `QueryError` (no `sub` is assigned, no pushes are
emitted).

| Code                                       | Condition                                                                                                |
|--------------------------------------------|----------------------------------------------------------------------------------------------------------|
| `multi_repo_subscriptions_not_supported`   | The set of distinct `table.repo` values across `subscribe[]` has cardinality > 1.                        |
| `table_not_found`                          | Any source's `table` does not resolve in its repo.                                                       |
| `subscription_filter_unsupported_operator` | The filter on any source contains a variant not in the supported list below.                             |

**Supported filter variants** (others → rejected):

```
Eq, Ne, Gt, Gte, Lt, Lte,
In, NotIn,
IsNull, IsNotNull, Exists, NotExists,
And, Or, Not
```

Unsupported (currently rejected): `like`, `ilike`, `regex`, `contains`, `contains_any`,
`contains_all`, `between`, `field_eq`, `fts`, `vector_similarity`, `computed`.

`And` / `Or` / `Not` are walked recursively; an unsupported variant anywhere in the
tree produces the rejection with the offending variant name in the error message.

---

## 8. PushEnvelope

Server-initiated frame. Always carries the `"push"` key (no `"rid"`).

```json
{
  "push": "event",
  "sub": 7,
  "seq": 12,
  "data": "<msgpack-or-json-bytes>",
  "gap_at": null
}
```

| Field    | Type            | Required           | Default | Description                                                                |
|----------|-----------------|--------------------|---------|----------------------------------------------------------------------------|
| `push`   | `PushKind`      | yes                | —       | One of `event`, `ready`, `gap`, `slow_consumer`, `closed` (§9).            |
| `sub`    | u64             | yes                | —       | Subscription id from the grant.                                            |
| `seq`    | u64             | yes                | —       | Per-sub monotonic sequence. Advanced only on successful push.              |
| `data`   | bytes (opaque)  | only on `event`    | absent  | Payload bytes — encoding depends on `DeliverMode` (§5).                    |
| `gap_at` | u64             | only on `gap`      | absent  | Lower bound of the gap window (see §9, §13). MAY be absent on lag gaps.    |

The envelope itself is serialized as MessagePack on the wire. `data` is an opaque
byte string; decoding is per-`DeliverMode`.

---

## 9. PushKind semantics

| Kind            | Meaning for the consumer                                                                                              |
|-----------------|-----------------------------------------------------------------------------------------------------------------------|
| `event`         | A change has been delivered. `data` carries the payload per the negotiated `DeliverMode`.                             |
| `ready`         | The initial snapshot is complete. Emitted **only** when the grant was issued with `initial: true`. Live events follow. |
| `gap`           | Events between `gap_at` (or "unknown" if absent) and the current cursor were missed. Consumer SHOULD reconcile out-of-band. |
| `slow_consumer` | The bridge dropped events because the per-connection push channel was full for `SLOW_CONSUMER_THRESHOLD` consecutive attempts (default `100`). The bridge will emit `closed` next. |
| `closed`        | Terminal. The subscription is gone server-side; no further pushes will arrive. Consumer SHOULD release local state.   |

`gap` is emitted in two situations:

1. **Resume backfill**: the requested `from_version` is older than the journal can
   serve (older than `JOURNAL_BACKFILL_LIMIT` events back). `gap_at = from_version`.
2. **Live lag**: the changefeed broadcast lagged this consumer. `gap_at` is absent.

`slow_consumer` is followed by a best-effort `closed`. The client MUST treat the
subscription as dead after either.

---

## 10. Filter evaluation contract

| Op type   | Filter behavior                                                                                  |
|-----------|--------------------------------------------------------------------------------------------------|
| `Put`     | Filter is evaluated against the **de-interned, string-keyed** record value.                      |
| `Delete`  | Filter is ignored. Only `EventMask` gates the event.                                              |

> Internally the changefeed ships record values as MessagePack with `u64` interned map
> keys. The server resolves these to string keys via the table's interner before
> running the filter. **Clients always see string-keyed JSON in payloads**; the
> on-wire representation never leaks interned ids.

Failure modes (all **fail-closed** — the event is dropped, not delivered):

- Decode failure of the changefeed value bytes (corrupt MessagePack, interner miss).
- A filter variant outside the supported list of §7 reaches the evaluator (should be
  impossible — grant validation rejects these — but the evaluator MUST return `false`
  defensively).

Filter semantics for the supported variants follow the standard `Filter` definitions
in `shamir-query-types`; numeric vs. string ordering uses `partial_cmp` on `f64` for
numbers and lexicographic on strings, with mismatched types yielding `None` (i.e. the
comparison fails).

---

## 11. Early-buffer rule

**Pushes MAY arrive before the `BatchResponse` carrying the grant.** The server
activates the bridge before the response is written to the wire, so under heavy
push activity the first `event` frames can race ahead of the `sub` assignment.

Clients MUST buffer push frames whose `sub` id is not yet registered locally, up to
**256 frames per unknown sub**. When the buffer is full, the **newest** frame is
dropped (the older buffered frames are retained — they represent the earliest state).

| Reference client    | Cap   | Drop policy                    | Diagnostic on drop                                   |
|---------------------|-------|--------------------------------|------------------------------------------------------|
| `shamir-client` (Rust) | 256 | drop NEW                       | `tracing::debug!`                                    |
| `shamir-client-ts`  | 256   | drop NEW                       | `console.warn`                                       |

Once the client receives the grant and registers the `sub`, buffered frames MUST be
flushed in FIFO order before any subsequently-arriving frame is dispatched.

If the buffer is never claimed (the grant never arrives — e.g. the response was
errored out) the entries SHOULD be evicted on connection close.

---

## 12. UnsubscribeOp

Wire discriminator: `"unsubscribe"` key on the op.

```json
{ "unsubscribe": 7 }
```

| Field         | Type | Required | Description                          |
|---------------|------|----------|--------------------------------------|
| `unsubscribe` | u64  | yes      | The `sub` id assigned by the grant.  |

Semantics:

- **Idempotent.** Unsubscribing an unknown or already-removed `sub` returns the same
  `unsubscribe_grant: true` envelope; it is never an error.
- **Trailing frames.** Any frames already in-flight (or already in the early buffer)
  MAY still arrive at the client after `UnsubscribeOp` is sent. Clients MUST tolerate
  this.
- **Terminal marker.** The definitive end of the subscription stream is the `closed`
  push (which the bridge emits on its way out). Frames arriving *after* `closed` for
  the same `sub` are protocol violations.

---

## 13. `from_version`

`from_version` is a resume cursor expressed as a repository commit version.

- The server replays journal events with `commit_version > from_version`, up to
  `JOURNAL_BACKFILL_LIMIT` (default `10_000`).
- If the requested `from_version` is older than the journal's oldest retained event,
  the server emits a single `gap` push with `gap_at = from_version` **before** any
  replayed events.
- During backfill, the server seeds an internal per-repo watermark equal to the
  highest replayed `commit_version`. Live events with `commit_version <= watermark`
  are dropped to prevent duplicates with replayed events.
- If `initial: true` is also set, the snapshot runs **after** backfill, and on its
  completion the watermark is bumped to `current_commit_version` to prevent
  duplicates between snapshot rows and the live tail. The `ready` push follows.

> The combination `from_version + initial: true` is permitted but unusual: the client
> receives backfill events, then the present-state snapshot, then `ready`, then live.
> Most clients want one or the other.

---

## 14. References

Rust types (wire DTOs):

- `crates/shamir-query-types/src/subscribe/subscribe_op.rs` — `SubscribeOp`
- `crates/shamir-query-types/src/subscribe/source.rs` — `SubscriptionSource`
- `crates/shamir-query-types/src/subscribe/deliver_mode.rs` — `DeliverMode`
- `crates/shamir-query-types/src/subscribe/event_mask.rs` — `EventMask`
- `crates/shamir-query-types/src/subscribe/unsubscribe_op.rs` — `UnsubscribeOp`
- `crates/shamir-query-types/src/table_ref.rs` — `TableRef`
- `crates/shamir-connect/src/common/push_envelope.rs` — `PushEnvelope`, `PushKind`

Server implementation:

- `crates/shamir-engine/src/query/batch/query_runner.rs` — grant validation,
  `find_unsupported_subscription_filter`.
- `crates/shamir-server/src/db_handler/subscribe_handler.rs` — `activate_subscriptions`,
  `sub` assignment.
- `crates/shamir-server/src/subscriptions/bridge.rs` — `bridge_task`, filter
  evaluation, watermarking, slow-consumer detection.

Tunables:

- `crates/shamir-tunables/src/lib.rs` — `SLOW_CONSUMER_THRESHOLD` (default `100`),
  `JOURNAL_BACKFILL_LIMIT` (default `10_000`).

Reference client builders (produce the shapes above — see "Query construction —
builder only" in `CLAUDE.md`):

- Rust: `crates/shamir-query-builder` (Batch / subscribe builder).
- TypeScript: `crates/shamir-client-ts/src/core/subscription-router.ts` (push routing
  + early buffer) and the typed query builder in the same crate.
- Rust client early buffer: `crates/shamir-client/src/subscription.rs`
  (`EARLY_BUFFER_CAP`).
