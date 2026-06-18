בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# LIVE SUBSCRIPTIONS — server-push changefeed to clients (#201)

**Status:** design doc (revision 2026-06-09). The client-facing
live-subscription layer: a long-lived connection that stays open and
receives pushed change events in real time, building on the existing
Phase 3b changefeed (hybrid live-push + durable journal).

Companion: [`TEMPORAL.md`](./TEMPORAL.md) (§2 — the `Temporal` selector
and `changes_since` resumable pull). This doc covers the *push* transport
and the client-demux story that `changes_since` alone cannot provide.

---

## §0 — Problem statement

Issue #201: "Продумать висячие бизнес-соединения (live-subscriptions /
server-push)." Today, a client that wants to react to data changes has
two options — both **pull**:

1.  Poll `changes_since(cursor, {repo, limit})` over a regular
    `DbRequest::Execute` batch.  Each call is a full request/response
    round-trip over the authenticated wire.
2.  Call `ShamirDb::subscribe_changelog` internally (engine API, not
    exposed over the wire).

Neither option keeps a connection open that pushes events to the client.
The server's wire protocol is strictly **request → response**: every
server-originated frame is a reply carrying the `rid` of a prior client
request. There is no mechanism for the server to send unsolicited frames.

**The gap:** a transport-level mechanism for server→client push frames,
a `DbRequest` verb to open/close subscriptions, and a client-side demux
that separates pushed events from regular request/response replies.

---

## §1 — Status quo (what already exists)

### 1.1 Changefeed emission (Phase 3b)

The engine emits `ChangelogEvent`s on every committed write — transactional
and non-transactional — through a per-repo `RepoChangefeed`
(`crates/shamir-tx/src/changefeed.rs`).

**When emitted:**

- **Transactional commits:** `crates/shamir-engine/src/tx/commit.rs:536`
  projects the write-set into a `ChangelogEvent` via `shamir_tx::project_event`
  and emits it after `gate.publish_committed`.

- **Non-transactional writes:** `crates/shamir-engine/src/table/write_exec.rs:257`
  calls `emit_nontx_changefeed(batch_version, changes)` after each direct
  write.

Both paths share the same per-repo version counter (`RepoTxGate`), so
`commit_version` is globally monotonic within a repo regardless of write
path (`changefeed_e2e.rs:431` tests this).

**Event shape** (`crates/shamir-tx/src/changefeed.rs:87`):

```
ChangelogEvent {
    repo: String,
    commit_version: u64,      // monotonic per repo
    tx_id: u64,               // 0 for non-tx writes
    actor: Actor,
    timestamp_ns: u64,
    changes: Vec<RecordChange> // { table, key, op: Put|Delete, value? }
}
```

### 1.2 Two emission tracks

Each `RepoChangefeed` (`crates/shamir-tx/src/changefeed.rs:166`) fans out
every event down two independent, non-blocking tracks:

| Track | Mechanism | Behaviour |
|-------|-----------|-----------|
| **Live push** | `tokio::sync::broadcast` (capacity 1024) | Instant delivery. Slow subscriber → `Lagged`. Zero subscribers → `Err`, silently ignored. |
| **Durable journal** | `tokio::sync::mpsc` (capacity 4096) → background writer → per-repo Store (`"__changelog__"`) | Append-only, keyed by `commit_version` BE. `try_send`; on overflow event is dropped with `log::warn!`. |

Emission (`RepoChangefeed::emit`, `changefeed.rs:286`) is non-blocking and
never fails the commit path.

### 1.3 CF-1 gap marker

When the journal writer's mpsc channel overflows, `RepoChangefeed::emit`
records the lowest dropped `commit_version` via an atomic CAS-min loop
(`changefeed.rs:186-190`, `first_gap_version: AtomicU64`). The
`read_from` method (`changefeed.rs:376`) returns `JournalRead { events,
gap_at: Option<u64> }` — `gap_at` is `Some(v)` when a known-dropped
version lies at or after the requested `from_version`.

### 1.4 `changes_since` wire op

`ChangesSinceOp` (`crates/shamir-query-types/src/admin/types.rs:554`) is
a one-shot admin batch op:

```msgpack
{ "changes_since": 0, "repo": "main", "limit": 1000 }
```

(wire form; clients build this via the query builder)

Execution (`crates/shamir-db/src/shamir_db/execute.rs:1993`) reads the
durable journal with `commit_version > cursor` (strictly after) and
returns:

```msgpack
{
    "changes_since": 0,
    "events": [ /* ChangelogEvent[] serialized as MessagePack */ ],
    "gap_at": null | <u64>
}
```

The TS client exposes `ddl.changesSince(cursor, {repo, limit})`
(`crates/shamir-client-ts/src/core/builders/ddl.ts:373`).

Usage example:

```ts
import { ddl, Batch } from '@shamir/client';

const resp = await Batch.create('poll')
  .add('c', ddl.changesSince(0, { repo: 'main', limit: 1000 }))
  .execute(client, 'my_app');

const { events, gap_at } = resp.results.c.records[0];
```

### 1.5 Engine API for live subscription

`ShamirDb::subscribe_changelog(db, repo)` (`shamir_db.rs:482`) returns a
`broadcast::Receiver<Arc<ChangelogEvent>>`. Used by the e2e tests
(`changefeed_e2e.rs`), but **not exposed over the wire** — no `DbRequest`
variant exists for it.

### 1.6 Wire protocol — strictly request/response

**Envelope** (`crates/shamir-connect/src/common/envelope.rs`):

```
Request:  { "sid": bytes(32), "rid": Optional<u32>, "req": <opaque> }
Response: { "rid": Optional<u32>, "res": <opaque> }
Error:    { "rid": Optional<u32>, "error": String }
```

Every server→client frame carries `rid` — it is a reply to a specific
request. There is no `rid`-less frame type (no unsolicited push).

**Request loop** (`crates/shamir-server/src/connection.rs:677-823`):
`read_frame_into` → `spawn_blocking(dispatch_request_view)` → write the
reply. The loop is purely sequential: read one, dispatch, write one,
repeat.

**DbRequest / DbResponse** (`crates/shamir-query-types/src/wire/db_message.rs`):
7 request variants (`Ping`, `Execute`, `CreateScramUser`, `TxBegin`,
`TxExecute`, `TxCommit`, `TxRollback`) and 8 response variants. No
subscription-related variant exists.

**TS client** (`crates/shamir-client-ts/src/core/client.ts`):
`sendDbRequest` serialises all calls over a single `sendQueue` promise
chain. The `WsFramer` (`framing.ts:26`) delivers frames FIFO; the
client matches replies strictly by arrival order, not by `rid`. Two
concurrent round-trips would cross-resolve. This serialisation is
incompatible with interleaved server pushes.

---

## §2 — The gap, precisely

| Already done | Missing for live client subscriptions |
|---|---|
| Per-repo changefeed emission (tx + non-tx) | Wire-level push frame type |
| Live broadcast channel (`broadcast::Receiver`) | `DbRequest` variant for subscribe/unsubscribe |
| Durable journal + `read_from` | Server-side subscription registry (maps `session_id → [repo subscriptions]`) |
| CF-1 gap marker (`first_gap_version`) | Session-loop integration (drain broadcast + write push frames alongside request/response replies) |
| `changes_since` batch op (pull) | Client-side demux (separate push channel from request/response channel) |
| `ShamirDb::subscribe_changelog` (engine API) | TS client subscription API (`client.subscribe(repo, opts?)`) |

The core architectural question: **how do server-pushed frames coexist
with the existing request/response session loop and the TS client's
FIFO framing?**

---

## §3 — Subscription model

### 3.1 What a client subscribes to

**MVP (P1):** a repo-wide subscription — every `ChangelogEvent` for the
named `{db, repo}` pair. This is the natural granularity because
`RepoChangefeed` is per-repo and `commit_version` is monotonic per repo.

**Later (P3):** table-level or filter-based subscriptions. The
underlying `broadcast::Receiver` already yields the full event with
`changes[].table`; a client-side or server-side filter can drop
irrelevant events. Adding a filter to the `Subscribe` request is a
wire-compatible addition (new optional field) — it does not change the
transport.

### 3.2 Cursor / resume story

A client that wants to resume after disconnect follows this protocol:

1.  **Reconnect** (SCRAM handshake or session-resume).
2.  **Subscribe** (`DbRequest::SubscribeChangelog { db, repo, from_version }`).
    - If `from_version = 0`: the server's subscription handler first
      calls `read_from(0, N)` on the durable journal to backfill the
      past, then bridges the live broadcast. Pushed events start from
      the oldest journaled event.
    - If `from_version > 0`: same, but the backfill starts at
      `from_version + 1`. The client sets this to the last
      `commit_version` it successfully processed.
3.  **Receive pushed events** (server→client push frames, see §4).
4.  **On gap signal** (`gap_at` field in a push frame): the client
    knows the journal has a hole. It must perform a full snapshot resync
    (read the current state of every table it cares about) and then
    resume with a fresh cursor.
5.  **On disconnect / reconnect**: repeat from step 1, setting
    `from_version` to the last processed `commit_version`.

The subscribe handler uses the established pattern from
`repo_instance.rs:493` and `shamir_db.rs:482`: subscribe to the live
broadcast first, then read the journal from `from_version` — the
journal/live overlap is de-duplicated by `commit_version` (described in
`changefeed.rs:259-264`).

---

## §4 — Wire protocol design

### 4.1 Two candidates compared

| Aspect | A: Subscribe + unsolicited push frames | B: Long-poll `changes_since` |
|--------|----------------------------------------|------------------------------|
| Latency | Immediate (push on commit) | One RTT per poll cycle |
| Wire change | New `DbRequest` variant + new response envelope type | None — reuse `Execute` |
| Server loop change | Significant — bidirectional multiplex | None |
| Client change | New demux channel | Just a polling loop |
| Backpressure | Server decides (buffer / gap-marker / disconnect) | Client-controlled poll rate |
| Connection utilisation | One WS connection carries both RPC + push | One WS per poll (or long-held request) |

**Recommendation: Candidate A** (subscribe + unsolicited push frames).

Rationale: Candidate B is a polling wrapper around `changes_since`, which
already works today with zero wire changes — a client can implement it
in user space. The *value* of #201 is true push with sub-RTT latency.
Candidate A delivers that; B does not. B remains available as a fallback
for clients that cannot handle push demux.

### 4.2 New `DbRequest` variants

```rust
// In shamir-query-types/src/wire/db_message.rs, added to DbRequest:
SubscribeChangelog {
    db: String,
    repo: String,
    from_version: u64,   // cursor: 0 = "from the beginning"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit_backfill: Option<u64>,  // cap on initial journal backfill
},
UnsubscribeChangelog {
    db: String,
    repo: String,
},
```

### 4.3 Response to `SubscribeChangelog`

The server replies immediately with an ack:

```rust
// Added to DbResponse:
SubscriptionOpened {
    db: String,
    repo: String,
    backfill_count: u64,   // how many journal events will follow as pushes
},
```

After the ack, the server starts pushing events. The client does **not**
send further requests to receive events — they arrive as unsolicited
frames.

### 4.4 Push frame — new envelope type

A push frame uses a dedicated envelope shape, distinct from
`ResponseEnvelope` so the client can demux without inspecting the
payload:

```
Push: { "push": "changelog_event", "sub": <subscription_id>, "seq": u64, "data": <ChangelogEvent msgpack> }
Gap:  { "push": "gap", "sub": <subscription_id>, "gap_at": u64 }
```

- `sub` is a server-assigned subscription id (returned in the ack or
  implicit — for the MVP with one subscription per session, it can be
  omitted).
- `seq` is a monotonically increasing per-subscription push counter for
  ordering verification.
- `data` carries the msgpack-serialised `ChangelogEvent` (same shape
  the journal stores, same shape `changes_since` returns).

A new Rust type in `shamir-connect/src/common/envelope.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushEnvelope {
    #[serde(rename = "push")]
    pub kind: String,                  // "changelog_event" | "gap"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(with = "serde_bytes", default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,        // msgpack ChangelogEvent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_at: Option<u64>,
}
```

The key insight: the client distinguishes `ResponseEnvelope` (has `rid`
and/or `res`), `ErrorEnvelope` (has `rid` and `error`), and
`PushEnvelope` (has `push`). A single `u8` discriminator at the msgpack
level (`"rid"` vs `"push"`) separates them — no magic byte needed.

### 4.5 Session loop change (server)

The current `request_loop` (`connection.rs:677`) is a sequential
read→dispatch→write cycle. It must become **bidirectional**: a
`tokio::select!` between:

1.  **Inbound frame** — client request (same as today).
2.  **Push channel** — `tokio::sync::mpsc::Receiver<PushEnvelope>` fed
    by a spawned per-connection subscription task.

Sketch:

```rust
async fn request_loop<F: Framer>(ctx, framer, frame_buf, write_scratch, sid) {
    let (push_tx, mut push_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let subscriptions = Arc::new(SubscriptionRegistry::new());

    loop {
        tokio::select! {
            // Branch 1: inbound client request (unchanged logic)
            read_result = framer.read_frame_into(MAX_FRAME_SIZE_DEFAULT, frame_buf) => { /* existing dispatch */ }

            // Branch 2: outbound push frame
            push_bytes = push_rx.recv(), if subscriptions.has_active() => {
                match push_bytes {
                    Some(bytes) => { let _ = framer.write_frame_into(&bytes, write_scratch).await; }
                    None => { /* push channel closed */ }
                }
            }
        }
    }
}
```

The subscription task (spawned inside the server on `SubscribeChangelog`)
calls `ShamirDb::subscribe_changelog(db, repo)` to get a
`broadcast::Receiver`, then loops `rx.recv() → serialise PushEnvelope →
push_tx.try_send()`. `try_send` honours the concurrency invariants
(CLAUDE.md §Concurrency): the commit-path never blocks, and a slow
consumer hitting the bounded push channel gets a gap marker instead of
backpressure.

### 4.6 TS client demux

The current TS client serialises all calls via `sendQueue`
(`client.ts:44`) and matches replies FIFO (`framing.ts:66`). Server
pushes would break this model — a push frame is not a reply to any
outstanding request.

**Solution:** extend `WsFramer` to support a separate push callback
channel. When a frame arrives that has a top-level `"push"` key, it is
routed to a `pushHandler` callback instead of being queued in the
`inbox` / resolving a `waiter`.

```typescript
// framing.ts — extended
class WsFramer {
    private pushHandler: ((push: Record<string, unknown>) => void) | null = null;

    onPush(handler: (push: Record<string, unknown>) => void): void {
        this.pushHandler = handler;
    }

    private onBinary(bytes: Uint8Array): void {
        // ... existing length-prefix validation ...
        const frame = body.slice();
        // Peek at the decoded msgpack to distinguish push vs response
        const preview = decode(frame) as Record<string, unknown>;
        if ('push' in preview) {
            this.pushHandler?.(preview);
            return;
        }
        // Existing FIFO path
        const waiter = this.waiters.shift();
        if (waiter) waiter.resolve(frame);
        else this.inbox.push(frame);
    }
}
```

The `ShamirClient` registers a push handler during construction. Pushed
events are routed to an `AsyncIterableIterator` or `EventEmitter` that
user code consumes. Regular request/response round-trips are unaffected.

**Performance note:** the extra `decode` peek is cheap (msgpack decode is
a header scan, not a full parse). If this ever matters, the server can
use a discriminating first byte (e.g. push frames start with a fixmap
key `0xa4` for `"push"` while responses start with `0xa3` for `"rid"`)
— but msgpack's self-describing nature makes the `decode` approach
correct and sufficient.

---

## §5 — Delivery semantics

### 5.1 Ordering

`commit_version` is monotonic per repo (`changefeed.rs:82-85`), and
events are emitted from the commit path in version order. The broadcast
channel preserves insertion order. Push frames arrive at the client in
the order the server writes them. **Guarantee: per-repo events arrive in
strictly increasing `commit_version` order.**

### 5.2 At-least-once

The live broadcast delivers at-least-once (no dedup). The durable
journal is an append-only log. On reconnect, the client resumes from its
last processed `commit_version` — duplicate delivery between the live
track and the journal backfill is de-duplicated client-side by skipping
events with `commit_version ≤ last_processed`.

### 5.3 Backpressure and slow consumers

Three tiers:

| Condition | Behaviour | Rationale |
|-----------|-----------|-----------|
| Push channel (`mpsc` to framer) full | Drop event, send `PushEnvelope { push: "gap", gap_at: V }` | Bounded memory (CLAUDE.md: lock-free, `try_send`). Gap marker lets the client resync from the journal. |
| Broadcast lag (`RecvError::Lagged`) | Inherent in `broadcast`; gap marker already surfaced by `first_gap_version` | Existing mechanism from Phase 3b. |
| Persistent slowness (push channel chronically full) | Server disconnects the session with a push frame `{ push: "slow_consumer" }` then closes the WS | Prevents unbounded memory growth. Client reconnects and resumes from its cursor. |

### 5.4 Gap recovery

When the client receives a gap marker (`gap_at = V`):

1.  The journal has a hole starting at version `V`.
2.  The client cannot trust incremental journal reads from before `V`.
3.  **Recovery path:** perform a full snapshot resync (read the current
    state of subscribed tables), note the current `commit_version`, then
    subscribe from that version + 1.

This is the same recovery semantics the existing `JournalRead::gap_at`
(`changefeed.rs:199`) describes.

### 5.5 Concurrency invariants (CLAUDE.md compliance)

| Invariant | How this design satisfies it |
|-----------|------------------------------|
| No `std::sync::Mutex` / `RwLock` in hot paths | Subscription registry uses `scc::HashMap`; push channel is `tokio::sync::mpsc` (bounded, `try_send`). |
| `broadcast::Sender::send` never blocks | Confirmed (`changefeed.rs:12`). |
| `tokio::task::spawn_blocking` for CPU-bound work | Push-frame serialisation is msgpack encode (µs-scale) — stays on the async task. Journal reads already use `spawn_blocking` in the engine layer. |
| No blocking on the commit path | Emission is `broadcast::send` + `mpsc::try_send` (confirmed `changefeed.rs:286-327`). Subscription fan-out is the same broadcast channel — zero additional commit-path cost. |

---

## §6 — Phased plan

### P1 — Server push-frame transport + subscribe/unsubscribe ops

**Goal:** the server can accept a `SubscribeChangelog` request, send an
ack, and push `ChangelogEvent`s as unsolicited `PushEnvelope` frames
over the existing WS/TCP connection.

**Crates / files touched:**

| File | Change |
|------|--------|
| `shamir-query-types/src/wire/db_message.rs` | Add `SubscribeChangelog`, `UnsubscribeChangelog` to `DbRequest`; `SubscriptionOpened` to `DbResponse`. |
| `shamir-connect/src/common/envelope.rs` | Add `PushEnvelope` struct. |
| `shamir-server/src/connection.rs` | Refactor `request_loop` into `tokio::select!` between inbound requests and push channel. Add per-session `SubscriptionRegistry`. |
| `shamir-server/src/db_handler.rs` | Handle the two new `DbRequest` variants: subscribe (call `ShamirDb::subscribe_changelog`, spawn a bridge task that feeds into the push channel) and unsubscribe (drop the broadcast receiver). |
| `shamir-server/src/subscription.rs` | New file: `SubscriptionRegistry` (tracks active subs per session, maps `(db, repo) → broadcast::Receiver + push_task_handle`). |

**Testability:** integration test over WS (real `WsFramer`): subscribe,
insert a row, receive the push frame with the expected fields, unsubscribe,
insert another row, confirm no push.

### P2 — Client demux + TS subscription API

**Goal:** the TS client can open a subscription and consume pushed events
via an `AsyncIterableIterator`.

**Files touched:**

| File | Change |
|------|--------|
| `shamir-client-ts/src/core/framing.ts` | Add push-frame detection in `onBinary`; route to `pushHandler` callback instead of inbox. |
| `shamir-client-ts/src/core/client.ts` | Add `subscribeChangelog(db, repo, fromVersion?)` method returning an `AsyncIterableIterator<ChangelogEvent>`. Wire up the `pushHandler` to feed the iterator. |
| `shamir-client-ts/src/core/types/subscription.ts` | New file: `ChangelogEvent`, `SubscriptionEvent` (push | gap) types. |

**Testability:** mock-server test that pushes frames and verifies the
iterator yields them in order.

### P3 — Journal backfill on subscribe

**Goal:** `SubscribeChangelog { from_version: V }` backfills from the
durable journal before switching to live push.

**Files touched:**

| File | Change |
|------|--------|
| `shamir-server/src/db_handler.rs` | Subscribe handler: call `read_changelog_from(V + 1, N)`, push each journal event as a regular `PushEnvelope`, then bridge the live broadcast. |
| `shamir-server/src/subscription.rs` | Backfill state machine (pending → live). |

**Testability:** integration test: insert 5 rows (no subscriber),
subscribe with `from_version: 0`, receive all 5 journal events then a
live event for a 6th insert.

### P4 — Gap recovery and slow-consumer handling

**Goal:** server sends gap markers; client exposes gap events; persistent
slow consumers get disconnected.

**Files touched:**

| File | Change |
|------|--------|
| `shamir-server/src/subscription.rs` | When `push_tx.try_send` fails (channel full), serialise and send a gap marker instead. Track consecutive failures; disconnect after threshold. |
| `shamir-client-ts/src/core/client.ts` | The subscription iterator yields `{ type: "gap", gap_at: V }` events. Client code decides how to react. |

**Testability:** integration test with a deliberately slow consumer
(sleep in the push handler) verifying the gap marker arrives and the
event stream continues.

### P5 (future) — Filtered subscriptions

**Goal:** `SubscribeChangelog` accepts an optional filter
(`tables: ["users", "orders"]`) and only matching events are pushed.

**Wire change:** additive optional field on `DbRequest::SubscribeChangelog`.
No envelope change. Filter applied server-side before `push_tx.try_send`.

### P6 (future) — Rust native client subscription

The `shamir-client` Rust crate already imports `DbRequest`/`DbResponse`
(`crates/shamir-client/src/lib.rs:40`). Adding subscription support
mirrors the TS client: demux push frames from request/response in the
client's framing layer.

---

## §7 — Open questions

1.  **Filter granularity.** MVP ships repo-wide subscriptions. Per-table
    filtering is a natural extension (P5) but the filter language
    (table list? WHERE predicate? record-key prefix?) needs a product
    decision.

2.  **Retention interaction.** When `purge_history` removes old journal
    entries, a client that subscribed with `from_version` pointing into
    the purged range gets an incomplete backfill. Should the server
    detect this and send a `gap` marker? Or is it the client's
    responsibility to track the retention window?

3.  **Auth / ACL on subscriptions.** `changes_since` currently requires
    `Action::Read` on the repo's store resource
    (`execute.rs:1996-2001`). Subscriptions should enforce the same
    check at subscribe time and re-check periodically (what if the
    user's role is revoked while subscribed?).

4.  **Multiple subscriptions per session.** The MVP supports one
    subscription per session (simplifies `sub` routing). Multi-sub
    requires a `sub_id` field and per-subscriber routing in the push
    handler — a straightforward extension once the single-sub case is
    proven.

5.  **TCP+TLS transport.** The push mechanism works identically over
    `TcpFramer` (full-duplex: read and write halves are independent).
    However, TCP clients are the native Rust client, not the TS client.
    Priority is WS (TS client); TCP support is P6.

6.  **Push-frame serialisation format.** This design uses msgpack
    (consistent with the rest of the wire). A future optimisation could
    use a lighter framing (e.g. the raw `ChangelogEvent` bytes without
    an outer `PushEnvelope` wrapper, discriminated by a 1-byte tag)
    if profiling shows the wrapper overhead matters.

---

## §8 — Wire layout summary

All shapes below are the **wire protocol design** for Phase 1–2. They are
not yet exposed through the TS client; once implemented, P2 will add
`client.subscribeChangelog(db, repo, fromVersion?)` returning an
`AsyncIterableIterator<ChangelogEvent>`.

### New DbRequest variants (wire form; clients build this via the query builder)

```msgpack
{ "op": "subscribe_changelog", "db": "mydb", "repo": "main", "from_version": 0, "limit_backfill": 1000 }
{ "op": "unsubscribe_changelog", "db": "mydb", "repo": "main" }
```

### New DbResponse variant (wire form; clients build this via the query builder)

```msgpack
{ "kind": "subscription_opened", "db": "mydb", "repo": "main", "backfill_count": 42 }
```

### Push frame (unsolicited, server → client) (wire form; clients build this via the query builder)

```msgpack
{ "push": "changelog_event", "sub": 1, "seq": 7, "data": <msgpack ChangelogEvent> }
{ "push": "gap", "sub": 1, "gap_at": 1042 }
{ "push": "slow_consumer" }
```

### Unsubscribe ack (regular response) (wire form; clients build this via the query builder)

```msgpack
{ "kind": "subscription_closed", "db": "mydb", "repo": "main" }
```
