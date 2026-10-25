# P0 stage C1 — p0-client-lifecycle

## Goal

Fix the open P0 review cards below at their root cause, with one regression test per card. Acceptance (from P0-FIX-PLAN.md): Reader exit closes every subscription channel and fails every pending request, including one registered during the drain.

Each card is a `### <ID>` section of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read the WHOLE card: trigger, consequence, direction, evidence). Its line numbers are from an older snapshot; the current evidence below (2026-10-24 revalidation at master b0e1ddf7) is authoritative for where the code lives now.

## Cards

### RV-CLIENT-93C26EBF — Reader exit never closes subscription channels: every live subscriber hangs forever on any disconnect

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-client/src/client/transport.rs:294; crates/shamir-client/src/client/subscription.rs:40
- Revalidation note: AGREE — OBSERVED: terminal reader cleanup drains only pending requests, while subscription handles retain the registry containing their channel senders.

### RV-CLIENT-B42B8576 — Reader-exit drain races pending registration → permanent hang of an in-flight request

- Status at HEAD: `confirmed-open`, risk `high`; evidence: crates/shamir-client/src/client/api.rs:338; crates/shamir-client/src/client/transport.rs:295
- Revalidation note: AGREE — OBSERVED: pending registration still lacks a closed recheck under the map lock, permitting insertion after terminal draining and an unbounded response wait when writing succeeds.

### RV-CLIENT-47644B74 — Concurrency-claim test gaps: stampede guard and drain race never exercised concurrently

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/tests/interner/interner_cache_tests.rs:194; crates/shamir-client/src/tests/client/demux_tests.rs:175
- Revalidation note: OBSERVED: dump coverage remains sequential and warm, and EOF coverage pre-registers waiters rather than exercising concurrent cold dumps or registration after terminal drain.

### RV-CLIENT-E38BE183 — Push-subscription lifecycle and closed-client paths have no error-path tests

- Status at HEAD: `confirmed-open`, risk `low`; evidence: crates/shamir-client/src/tests/client/demux_tests.rs:175; crates/shamir-client/src/tests/client/demux_tests.rs:276
- Revalidation note: OBSERVED: registered lifecycle tests still assert pending-request draining or individual push delivery, not receive closure, dead-client subscription registration or the production closed-roundtrip guard.

## BINDING DESIGN (two rush reviewers + @oh agree on the core; @oh's variant chosen where they differ)

Core: ONE closeable lifecycle state shared by request registration, subscription registration and teardown. Check-open and insert happen under the SAME lock that publishes the close cause and detaches all registries. A single idempotent `shutdown(cause)` runs on EVERY exit path of the connection (EOF, I/O error, reader task abort/cancel/panic via a drop guard, `Client::close()`, `Client` Drop) and: answers every pending request with `ConnectionClosed(cause)`, drops every subscription sender (and the early-push buffer) so each subscriber drains its already-queued items and then gets `None`. After shutdown, new registrations fail with the same cause and NOTHING is written to the wire.

Details (@oh):
- State: replace PendingMap/SubscriptionMap/EarlyBuffer by `Registry<K,V>{{ closed: Option<CloseCause>, map }}` guarded by the EXISTING `std::sync::Mutex` (no new lock kind; inline comment naming the contention model: O(1) insert/remove, never held across .await) — `pending: Mutex<Registry<u32, PendingSender>>`, `subs: Mutex<SubsInner{{closed, live, early}}>`, plus `closed_hint: AtomicBool` as a FAST PATH ONLY (not authoritative). `shutdown(c)`: lock pending, if already closed return; set closed, take the map; lock subs, set closed (first cause wins), clear live+early (drops PushSenders); set closed_hint; THEN resolve the taken waiters OUTSIDE the lock with `Err(ConnectionClosed(c.clone()))`.
- Error: `ClientError::ConnectionClosed(CloseCause)` with `#[non_exhaustive] #[derive(Clone)] enum CloseCause {{ PeerEof, Io(Arc<str>), LocalClose, ClientDropped, ReaderCancelled }}`; Display keeps the `connection closed` prefix. Update the few matches in transport.rs/api.rs/tests.
- Reader (transport.rs ~:185-303): install a `ShutdownGuard{{state, cause: ReaderCancelled}}` first; set `cause = PeerEof` / `Io(e)` when the loop ends; the guard's Drop calls `shutdown(cause)` (so abort and panic also close everything). Push routing looks up the live sender and falls back to the early buffer under the one `subs` lock.
- `roundtrip` (api.rs ~:310-365): fast check on closed_hint -> Err(cause); then under the `pending` lock: if closed -> Err(ConnectionClosed(cause)) else insert; then write; on write failure remove(rid): if already gone (drained) return the drained result via `rx.await`, else return the transport error. Timeout branch removal unchanged; a dropped receiver maps to `ConnectionClosed(cause_or(ReaderCancelled))`.
- `subscribe_push` (api.rs ~:67-95): ONE `subs` lock: if closed -> return an already-ended handle (drop tx); else insert sender and flush the early buffer under that same lock (also fixes the ordering window). `SubscriptionHandle::next() -> Option<PushEnvelope>` keeps its signature (queued items then None) and gains `termination() -> Option<CloseCause>`. DECISION (orchestrator): END-OF-STREAM + `termination()`, NOT a terminal error item (a terminal item could fail to enqueue on a full bounded channel and would change the stream type).
- `close()`: `shutdown(LocalClose)`, abort the reader, shut the write half. `Drop`: `shutdown(ClientDropped)`, abort.
- Test seam: move the three duplicated reader-spawn blocks (connect.rs ~:249-276, ~:435-461, ~:537-562) into one `Client::assemble(r, w, ..)`; add a `#[cfg(test)]` test-sink variant and `Client::for_test(r, w, hooks)` that goes through `assemble`. `#[cfg(test)] LifecycleHooks` with points `ReaderBeforeShutdown`, `AfterFastCheck`, `AfterRegister`: each a one-shot pair (`oneshot::Sender<()>` reached, `oneshot::Receiver<()>` release) consumed when fired, FIRED BY THE PRODUCTION CODE PATHS (roundtrip, reader guard, assemble). The test harness keeps every release Sender alive and calls it on every path (drop-guard).

Invariants: I1 under each registry lock `closed.is_some()` implies the map is empty; I2 every inserted PendingSender is resolved exactly once (response, drain, or its owner on timeout/write failure); I3 after shutdown no PushSender remains; I4 shutdown runs on all exit paths and is idempotent.

Test oracles (use two `tokio::io::simplex`/duplex pipes for separate directions; no sleeps; assert the receive side ENDS, never inspect registries directly):
T1 N=8 subscriptions and M=8 requests live, then EOF: every ping gets `ConnectionClosed(PeerEof)`; each handle yields its queued item then `None`; `termination()==Some(PeerEof)`.
T2a reader parked at ReaderBeforeShutdown (hook), a ping registered meanwhile, released: ping gets PeerEof from the drain.
T2b THE BUG: arm AfterFastCheck, start a ping, await `reached`; EOF and await the reader's completion through a test accessor; release; the ping must resolve to PeerEof and NOTHING may be read from the client->server pipe (`select! {{ biased; r = &mut ping => PeerEof, _ = server.read_frame() => panic!("written after close") }}`). Red on current master.
T3 call on a closed client: ping -> Err(PeerEof) with zero bytes written; `subscribe_push(7).next()` -> None, `termination()==Some(PeerEof)`.
T4 handle outlives the client: subscribe, push one event, drop(client): item, then None, then ClientDropped; the `close()` variant -> LocalClose. Red on current master.
T5 push error paths: (a) push CAP+1 events unconsumed then EOF: exactly CAP items then None; (b) early-buffered pushes then EOF then subscribe_push: early buffer empty, handle ends; (c) call `reader_task` directly, abort and await it: state closed with ReaderCancelled.
T6 (card 47644B74) concurrent cold callers of the stampede-guarded path parked at a hook before the fetch: exactly one fetch frame reaches the server.
Acceptance: only `ClientError::ConnectionClosed` changes in the public API; each registry lock carries an inline contention-model comment; no test touches a registry directly.

Reviewer A's text for reference:
## Verdict — approve a unified, closeable lifecycle registry; reject “drain the maps on EOF” as insufficient

**C1 must make closure a single irreversible state transition shared by request registration, subscription registration, and teardown.** Use the existing registry-lock approach, consolidated into one lifecycle state: check-open and insert under the **same lock** that publishes the close cause and detaches all registries. Subscribers should receive **one typed terminal error, then end**, without depending on free capacity in their bounded push channel.

This is a design recommendation, not a verified implementation. All four requested cards, including their 2026-10-24 revalidation paragraphs, were read in full. No files were changed; no builds or tests were run.

## 1. Evidence and invariant

### OBSERVED — current implementation

- Reader exit sets an atomic flag and drains pending requests, but does not clear subscriptions or early pushes: `crates/shamir-client/src/client/transport.rs:294–302`.
- Subscription handles retain the registry containing their own senders: `crates/shamir-client/src/client/subscription.rs:36–40`. `next()` only awaits the bounded receiver: `crates/shamir-client/src/client/subscription.rs:53–55`.
- Requests check `closed` before serialization, then insert unconditionally under a different synchronization boundary: `crates/shamir-client/src/client/api.rs:310–342`.
- Push registration has no closed check and returns a handle unconditionally: `crates/shamir-client/src/client/api.rs:67–94`.
- Explicit close and Drop abort the reader; neither invokes terminal cleanup first: `crates/shamir-client/src/client/api.rs:371–387`.
- Reader failures lose their cause; `ConnectionClosed` is currently a unit variant: `crates/shamir-client/src/client/transport.rs:203–209`, `crates/shamir-client/src/error.rs:66–69`.

### INFERRED — consequences, not runtime reproductions

1. Keeping a live handle also keeps its sender registry alive. Once queued pushes are exhausted, transport failure alone cannot make `next()` return `None`.
2. A request can pass the initial check, pause, and insert after the reader's final drain. If writing succeeds, its default unbounded response wait has no remaining resolver.
3. Aborting the reader does not execute the ordinary epilogue following its read loop.

These agree with the revalidated findings at `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md:375`, `:413`, `:433`, and `:489`.

### Required invariant

> Every request or subscription registration linearizes either before close, in which case it is owned by normal completion or terminal cleanup, or after close, in which case registration fails with the stored close cause. No sender can be inserted into a closed registry. Closure happens once, never reopens, and cannot depend on network progress or consumer progress.

A response already completed before closure remains successful. A request timeout or caller cancellation that already removed its entry is not retroactively changed into a connection error.

## 2. One concrete lifecycle design

### State and registration

Replace the separate lifecycle ownership at `crates/shamir-client/src/client/connect.rs:54–63` with one shared registry state:

```text
Lifecycle {
    phase: Open | Closed(Arc<CloseCause>),
    pending: FxMap<RequestId, PendingSender>,
    subscriptions: FxMap<SubId, SubscriptionEntry>,
    early_pushes: FxMap<SubId, Vec<PushEnvelope>>,
}
```

Use **one existing synchronous registry mutex**, not a new independent closed-flag lock. All critical sections are synchronous; no I/O, `.await`, serialization, or user callback runs while held. This is a deliberately lock-based correctness design, **not a lock-free claim**. It consolidates the locks already present at `transport.rs:137` and `subscription.rs:28–33`; its contention justification must be explicit under the project's hot-path rules.

Against `crates/shamir-client/src/client/api.rs:338–343`:

```text
register_request(rid, sender):
    lock lifecycle
    if Closed(cause):
        return ConnectionClosed(cause)
    insert sender, rejecting an occupied rid rather than replacing it
    unlock
    emit request_registered(rid) hook
```

An optional early closed check can avoid serialization, but **only this locked check authorizes registration**.

Against `crates/shamir-client/src/client/api.rs:67–94`:

```text
subscribe_push(sub_id) -> Result<SubscriptionHandle, ClientError>:
    create bounded push channel and separate terminal channel
    lock lifecycle
    reject Closed(cause)
    reject an already-live sub_id without replacing its sender
    atomically register and transfer early pushes
    unlock
    emit subscription_registered(sub_id) hook
    return handle
```

Push lookup and early buffering at `transport.rs:217–245` must participate in the same registry ordering. Otherwise a push can miss registration and enter the early buffer after registration already flushed it. Do not hold the lock across asynchronous delivery.

### Close operation

Replace `crates/shamir-client/src/client/transport.rs:294–302` with one idempotent primitive:

```text
close_once(cause):
    lock lifecycle
    if Closed(existing):
        unlock
        return existing

    lifecycle.phase = Closed(shared cause)
    pending = take(lifecycle.pending)
    subscriptions = take(lifecycle.subscriptions)
    early_pushes = take(lifecycle.early_pushes)
    publish connection-terminal notification
    unlock

    for pending sender:
        send Err(ConnectionClosed(shared cause))

    for subscription entry:
        publish its terminal error through its independent terminal slot
        drop its push sender

    drop early_pushes
    emit cleanup_complete hook
    return shared cause
```

The phase change and registry detachment are one critical section. Notification delivery and destruction of potentially large detached buffers happen outside it. No terminal send awaits channel capacity.

**Ordering proof:** if registration wins the lock, cleanup detaches its entry. If close wins, registration observes `Closed` and cannot insert. There is no third outcome and no need to repeat draining.

### Every reader exit, including cancellation

At `crates/shamir-client/src/client/transport.rs:185–201`:

- Return an explicit cause from the read loop; do not just `break` and discard it.
- Run `close_once(cause)` for peer-close, read/framing failure, and cooperative cancellation.
- Own a synchronous drop guard that invokes `close_once(ReaderCancelled)` if the reader future is aborted or dropped before normal cleanup.
- Construct that guard **before spawning and move it into the future**. Constructing it only inside the async body misses cancellation before the first poll.

Apply the same construction to all three spawn sites: `crates/shamir-client/src/client/connect.rs:255`, `:440`, and `:541`.

At `crates/shamir-client/src/client/api.rs:371–387`, explicit close and client Drop must invoke `close_once(LocalClose/ClientDropped)` **before** aborting the reader. Transport shutdown is subsequent best-effort cleanup, not a prerequisite for notifying consumers. Repeated close attempts preserve the first cause.

### A drained oneshot is not enough: unblock the write phase

**OBSERVED:** a request awaits the write mutex and `write_frame` before awaiting its response receiver: `crates/shamir-client/src/client/api.rs:348–361`.

Therefore requests must observe terminal notification while waiting for the writer **and while writing**. Otherwise closure can resolve their oneshots while the actual public calls remain stuck in write acquisition or I/O.

```text
register request
race connection-terminal against writer acquisition
recheck terminal state after acquiring writer
race connection-terminal against write_frame
on successful write: await response/terminal result
on transport write failure: close_once(WriteFailure)
on exit/cancellation: remove this request's entry through a registration guard
```

Cancelling a partially completed framed write makes that connection unusable; it must not be reused for later requests. Closure is irreversible, and writes must not be retried automatically.

## 3. Error and subscription contract

### Requests

Change `crates/shamir-client/src/error.rs:66–69` to a typed equivalent of:

```text
ClientError::ConnectionClosed { cause: Arc<CloseCause> }
```

`CloseCause` should distinguish:

- peer close sentinel;
- read failure, retaining the underlying error and its kind;
- framing violation, retaining relevant framing information;
- write failure;
- explicit local close;
- client Drop;
- reader cancellation.

Preserve the actual error rather than only its formatted string. The first successful close transition owns the cause; other waiters and subsequent calls receive that same cause.

Framing currently represents the zero-length sentinel separately, while `read_exact` propagates EOF/read errors: `crates/shamir-transport-tcp/src/framing.rs:100–110`. Do not fabricate a “clean EOF” classification that the current framing interface cannot distinguish from truncation.

Server request errors remain request errors, not automatic connection closure: current routing is at `crates/shamir-client/src/client/transport.rs:257–264`.

### Subscribers: terminal error, then end

Change the contract at `crates/shamir-client/src/client/subscription.rs:53–55` to:

```text
next() -> Option<Result<PushEnvelope, ClientError>>
```

For connection closure:

1. Yield already-admitted buffered pushes in order.
2. Yield exactly one `Err(ConnectionClosed { cause })`.
3. Return `None` on every subsequent call.

Use a **separate terminal oneshot/slot**, not `try_send(Err(...))` into the bounded push queue. A full queue must not drop the only indication of failure, and a stalled subscriber must not block reader cleanup. `next()` must remember when the terminal item has been emitted.

Do not invent a server `PushKind::Closed` frame for local transport failure. That wire kind means server-initiated subscription closure: `crates/shamir-connect/src/common/messages/push_envelope.rs:17–18`. Its existing meaning is distinct from loss of the connection.

### New calls after closure

- Request methods return the stored typed closure error without registering or writing.
- `subscribe_push` returns `Err(ConnectionClosed { cause })`, not a dead-on-arrival handle.
- Existing handles still receive their terminal item even if `Client` has already been dropped.
- Metadata accessors can remain available; they do not perform connection operations.

The `subscribe_push` and `next()` signature changes are intentional public API changes, not implementation details.

## 4. Deterministic CI oracles

Provide a post-handshake injected transport constructor sharing the **real initialization and reader-launch path**. The current reader is generic, but `WriteSink` only supports TLS and IPC: `crates/shamir-client/src/client/transport.rs:35–38`. A test-only injected writer must still exercise real `roundtrip`, registration, framing, and teardown.

Use independently controlled fake read/write halves. In particular, the writer must be able to continue accepting writes after read EOF; otherwise write failure can accidentally hide the registration race.

| Scenario | Deterministic sequence | Required assertions |
|---|---|---|
| **Disconnect with N subscriptions and M requests** | Register handles through `subscribe_push`; launch real calls with response timeout disabled; await production `request_registered` hooks; admit known pushes; inject reader failure. | All M calls finish with the injected typed cause; every handle yields its admitted pushes, one terminal error, then persistent `None`; registry and early buffer are empty; client remains alive during assertions. |
| **Close wins the registration boundary** | Park a request after its optimistic open check but before locked registration. Inject EOF. Reader performs close/detach, then parks at an **after-detach, before-notification** hook. Release request registration while reader remains parked. | Request rejects immediately with the stored cause; no insertion and no write; after reader release, all earlier requests terminate. This reproduces the old dangerous ordering without relying on writer failure. |
| **Registration wins the boundary** | Park reader at a **before-close-transition** hook, outside the lock. Register a real request and await its registration hook; release reader. | The request is included in cleanup and receives the close cause; nothing remains registered. |
| **Closed-client call** | Inject failure and await real cleanup completion; then call `ping` and `subscribe_push`. | Exact closure variant/cause; no new registry entries; fake writer invocation count unchanged. |
| **Push error path with a full channel** | Fill a real handle's bounded queue; await production push-admission hooks; inject read error without consuming. | Cleanup completes before consuming; queued pushes remain ordered; terminal error is not lost to fullness; then end. |
| **Blocked writer / cancellation** | Fake writer acknowledges entering `Pending`; inject reader failure. Separately abort a reader after start, and drop its future before first poll. | Public request finishes without writer release; live handles terminate; cancellation guard closes in both reader-cancellation cases. |
| **Explicit close / Drop** | Keep subscription handles alive independently of the client; close or drop client. | Terminal cause is local close/Drop, exactly once; handles do not retain live senders indefinitely. |

Also inject a real server error envelope for an in-flight subscription-creation request: assert the request error, no successful handle creation, and no accidental connection-wide failure.

### Hook discipline

Hooks must be emitted by the actual production control flow:

- registration acknowledgment **after successful insertion**;
- push acknowledgment **after actual admission**;
- drain-boundary acknowledgment **after the real close transition and detachment**;
- cleanup completion **after terminal notifications and sender disposal**.

Test-only hooks may pause outside locks; they must not implement an alternate lifecycle algorithm. A test that fires its own “registered” hook, directly inserts a sender, sets `closed`, or manually clears a registry proves the test harness—not the public client.

Use oneshot release/acknowledgment pairs, no sleeps or yield-count scheduling. CI timeouts are failure watchdogs, never evidence that the desired interleaving occurred.

## 5. Acceptance decision and card disposition

**Accept C1 only when:**

- One close transition excludes both kinds of subsequent registration.
- EOF, read/framing error, write failure, cancellation, explicit close, and Drop converge on that transition.
- Requests blocked before response-await also terminate.
- Full push queues cannot suppress terminal errors or block cleanup.
- Boundary tests exercise both linearization orders through real methods.
- CI registers and executes the new oracles; merely adding test files is insufficient.

The existing EOF test pre-registers map entries and only asserts request errors: `crates/shamir-client/src/tests/client/demux_tests.rs:175–206`. Existing push delivery coverage receives one push but never asserts terminal delivery or end: `:276–309`. Neither establishes the required lifecycle invariant.

**Do not close all of RV-CLIENT-47644B74 on C1 evidence alone.** Its concurrent cold interner-dump obligation is separate; the cited current test is explicitly warm and sequential at `crates/shamir-client/src/tests/interner/interner_cache_tests.rs:194–196`. C1 can discharge its drain-race portion, not its stampede-testing portion.

**Files changed:** none. **Builds/tests run:** none; verification remains GitHub CI only.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
