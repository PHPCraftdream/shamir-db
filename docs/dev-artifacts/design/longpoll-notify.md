# LONGPOLL-NOTIFY — wake the replication long poll on a journal event

Status: design only (no production code in this change).
Scope: leader-side `Pull.wait_ms` in `crates/server/shamir-server`.

---

## 1. The current long-poll loop

All citations are to
`crates/server/shamir-server/src/engine/db_handler/handlers/repl_handler.rs`
(revision of this worktree) unless noted.

**File-header guarantees** (`repl_handler.rs:17-24`, "Long-poll (§5.1 / §5.6)"):

* `Pull.wait_ms` is a polite long-poll: when no events are immediately
  available, the handler re-reads the journal in a short-step loop
  (`POLL_STEP`) until the deadline expires or events appear.
* This is explicitly **"not a subscription"** (`repl_handler.rs:21-22`): the
  leader holds **no per-follower state** (`:22`), takes **no locks**, and
  **never blocks writers** (§5.6, `:22-23`).
* The loop is **deadline-bounded**: an empty tail always returns within
  `~wait_ms` (`:23-24`).

**The step constant** — `POLL_STEP = Duration::from_millis(50)`
(`repl_handler.rs:64-67`): "Short enough that the handler stays responsive
(new events appear within ~this interval), long enough to avoid a busy loop
when the tail is empty."

**The loop itself** (`handle_pull`, `repl_handler.rs:407-458`):

1. A first journal read runs unconditionally (`repl_handler.rs:381-405`):
   `self.db.read_changelog_for_replication(db, repo, from_version,
   effective_limit)`. Events, an error, or `unknown_repo` return
   immediately — the loop is entered **only** when the first read was empty
   AND `wait_ms` was supplied (`:407-410`).
2. The wait budget is clamped: `wait_ms.map(|ms| ms.min(30_000))` and must
   be `> 0` (`:411-412`).
3. `deadline = Instant::now() + ms` (`:413`). The loop body (`:414-457`):
   * if `now >= deadline`, break (`:419-421`);
   * otherwise sleep `remaining.min(POLL_STEP)` — never past the deadline
     (`:422-424`);
   * re-run the same replication journal read; empty → next step, error →
     error response (`:426-456`).

**Cost and latency.** With an empty tail and `wait_ms` ≤ 30 000 the handler
performs `⌈wait_ms / 50⌉` journal re-reads (N followers ⇒ N × 20 reads/s at
`wait_ms = 1000`), and a new event is observed up to 50 ms after it lands —
that is the sole reason for this design.

**What the read returns.** `read_changelog_for_replication`
(`crates/engine/shamir-db/src/shamir_db/shamir_db/catalog/changelog.rs:224-249`)
delegates to the repo's replication page read
(`crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/journal.rs:16-22`,
`read_changelog_replication_page`). Like every journal read, the returned
prefix is bounded by the **contiguous completion watermark**, not the last
committed version — "A stalled writer may return an older (possibly empty)
prefix" (`journal.rs:229-233`). This matters for §4: waking on *visibility*
is sufficient but not always sufficient for the *journal read* to see the
event yet.

---

## 2. Signals that already exist

### 2.1 Live changefeed broadcast of `ChangelogEvent`

* **Where it fires relative to publication: AFTER publish (visibility).**
  Commit path order, sync phase:
  `repo.drainer().wake()` at
  `crates/engine/shamir-engine/src/tx/commit/pipeline/phase/finalize.rs:100`,
  then `repo.broadcast_changefeed_event(changefeed_event)` at `:103` (a
  `#[cfg(test)]` hook sits between them at `:102`). The async-tail commit
  path does the same at
  `crates/engine/shamir-engine/src/tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs:134-135`
  (and wakes the drainer again at `:161`). In both paths the version is
  already visible when the broadcast fires (comment at `finalize.rs:88-99`:
  the drainer is woken "after the version is published (visibility)").
* **What it carries:** `Arc<ChangelogEvent>` (the same event the journal
  receives; the single Arc is shared by both tracks —
  `crates/storage/shamir-tx/src/changefeed/feed/core.rs:236-250`, broadcast
  at `:264-266`).
* **Mechanism:** a per-repo `tokio::sync::broadcast` channel, capacity
  `BROADCAST_CAPACITY = 1024`
  (`crates/storage/shamir-tx/src/changefeed/feed/events.rs:96`), created
  once per repo at `core.rs:170`.
* **Late subscriber:** misses events emitted before `subscribe()`, but the
  feed is "always on" — the journal is fed on every non-empty commit
  regardless of subscribers
  (`crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/journal.rs:74-79`),
  so a late subscriber catches up via `read_changelog_from`
  (`journal.rs:212-221` documents the subscribe-then-read contract).
  Subscribers that fall behind the ring get `RecvError::Lagged` and are told
  to re-sync via the journal (`journal.rs:213-215`).
* **Cost for N followers:** O(1) per event for the *writer*
  (`broadcast::Sender::send` is a single lock-free push, "NEVER blocks and
  NEVER errors out to the caller" — `core.rs:236-239`); each of the N
  receivers gets a cloned Arc. Delivery to N subscribers is O(N) but cheap;
  the ring bounds memory at 1024 events.
* **Where the server could reach it:** the db-layer wrapper
  `subscribe_changelog`
  (`changelog.rs:120-159`) wraps the receiver in a revocable
  `ChangelogReceiver` and **takes `lock_replication_control()`**
  (`changelog.rs:137`) — an async lock held across registration. That is a
  lock on the subscribe path, which the replication handler must not
  acquire per poll (§4 invariant 2); the engine-level
  `RepoInstance::subscribe_changelog` (`journal.rs:216-221`) bypasses the
  revocation wrapper but is still a per-subscriber broadcast receiver —
  a per-follower artifact.

### 2.2 Bounded confirmed-read position

* `RepoChangefeed` keeps `confirmed_through: Arc<AtomicU64>` and
  `barrier_notify: Arc<Notify>` (`core.rs:104-105`), built at `:175-176`.
  `confirm_through(w, budget)` (`core.rs:272+`) enqueues a `Barrier(w)` to
  the journal writer and waits on `barrier_notify` until the durable prefix
  reaches `w` (timeout-bounded).
* **When it fires:** the journal writer notifies the barrier waiter after
  events are durably installed; it is a **pull-completion primitive**
  (`await_changelog_progress` uses it —
  `changelog.rs:294-308`), not a push to idle readers.
* **What it carries:** the confirmed version `u64` only.
* **Late subscriber:** yes — it is a level, not an edge: re-reading
  `confirmed_through` always yields the current value. Nothing is missed.
* **Cost for N followers:** zero marginal cost — it is one atomic the
  journal writer already maintains; waiters queue on a single shared
  `Notify`. This is the only existing signal whose fan-out is already
  O(1)-per-writer, O(waiters)-per-event with no per-follower state.

### 2.3 Journal append notification

* **There is none aimed at readers.** The append path is
  `journal_enqueue` → bounded `mpsc` → background `journal_writer_loop`
  (`core.rs:252-262`, writer at `:557+`); the only notification it emits is
  `barrier_notify` for a *specific* pending `confirm_through` waiter
  (§2.2). No "tail advanced" broadcast, no watch channel over the journal
  high-water mark exists today.

### 2.4 Drainer wake

* `DrainerWindow::wake()` (`crates/engine/shamir-engine/src/tx/drain/drainer/window.rs:117-120`)
  is `self.notify.notify_one()` on a bare `tokio::sync::Notify`; the commit
  path calls it after publishing (`finalize.rs:100`,
  `legacy_and_lockfree.rs:161`).
* **What it carries:** nothing (a pulse). **Late subscriber:** a bare
  `Notify::notify_one` stores at most one permit — a waiter that subscribes
  after the pulse but before the drainer loop consumes it may get the
  permit, otherwise it misses it; there is no replay.
* **Cost for N followers:** `notify_one` wakes exactly **one** waiter —
  unusable as a shared fan-out for N followers without a per-repo
  replacement. It exists for the drainer loop only; hijacking it would
  couple replication to drain scheduling.

### 2.5 Survey conclusion

The changefeed broadcast is the closest match (post-publication timing,
always-on journal) but is a per-subscriber `broadcast::Receiver` with a
bounded ring — attaching one to every long-poll request creates exactly the
per-follower state the file header forbids (`repl_handler.rs:22`), and the
db-layer subscribe path takes `lock_replication_control()`
(`changelog.rs:137`). The confirmed position (`confirmed_through` /
`barrier_notify`, §2.2) already implements the "shared, level-based,
O(1)-writer" shape — but it is private to `shamir-tx`'s feed and is only
notified at barrier completion, not on every event. The minimal mechanism
therefore adds one shared edge signal fed by the existing broadcast point.

---

## 3. Chosen mechanism: one shared `Notify` per repo, subscribed before the final re-read

### 3.1 The mechanism

Add **one `tokio::sync::Notify` per repo instance** (not per follower),
woken by the *existing* post-publication broadcast point, and replace the
fixed `sleep(remaining.min(POLL_STEP))` (`repl_handler.rs:422-424`) with:

```rust
// Subscribe FIRST (creates the waiter and arms any pending permit).
let wake = repo_notify.notified();          // NotifiedFuture — registration happens here
tokio::pin!(wake);
wake.as_mut().enable();                     // permit is captured, not lost

// THEN the final re-read of the tail (the initial read already happened).
if journal non-empty at re-read { return events; }

tokio::select! {
    _ = &mut wake => { /* loop: re-read the tail immediately */ }
    _ = tokio::time::sleep_until(deadline) => break,
}
```

Why `Notify` and not a `watch` of the confirmed version:

* A `watch` channel would require publishing a version number from the
  commit path. `confirmed_through` (§2.2) is a *durability* watermark,
  which lags visibility; a watch of *visibility* would need a new
  publisher hook in the commit pipeline. `Notify` can be hung off the
  broadcast call site with no version plumbing, no channel allocation per
  repo at commit time, and no lag/lost-value semantics to reason about —
  the waiter re-reads the journal for truth, the signal is only a hint.
* Spurious wakes are free: on wake the loop re-reads; if still empty it
  re-arms and re-enters `select!`. The deadline (`repl_handler.rs:413`)
  is recomputed once and never extended, so the §5.1 bound is preserved.

Plumbing (no wire, no storage changes):

* `RepoInstance` gains an `Arc<Notify>` created with the instance (same
  lifecycle as the drainer's `window.rs` notify — one field, no OnceCell).
* `broadcast_changefeed_event` (`journal.rs:194-198`), which is already
  called post-publication from both commit paths
  (`finalize.rs:103`, `legacy_and_lockfree.rs:135`), additionally does
  `self.tail_notify.notify_one()` — waiters on a bare `Notify` get
  `notify_one` semantics one-at-a-time, so use `notify_waiters()` (wakes
  all currently-armed waiters, O(waiters)) instead; it pairs correctly
  with the arm-then-enable pattern above.
* The db layer exposes `changelog_tail_notify(db, repo) ->
  Option<Arc<Notify>>` (pure lookup, no lock, mirroring
  `current_commit_version` at `changelog.rs:202-206`); the server handler
  fetches it once before entering the loop.

### 3.2 The lost-wakeup race and why it is closed

The race: event lands **between** the waiter's last empty journal read and
the moment the waiter starts listening. With `sleep` polling this cannot
lose an event (the next poll sees it); with a signal it can — unless the
waiter subscribes first.

Tokio's `Notify` gives exactly the needed primitive:

1. `Notify::notified()` returns a `NotifiedFuture`; **calling `notified()`
   registers the waiter**, and `enable()` on the pinned future is the
   documented arm point. If `notify_waiters()` fires at any moment after
   `notified()` was created and before `await`, the waker is stored and the
   future completes immediately — the event is not lost.
2. Order in the loop above: `notified()` + `enable()` (subscribe) happens
   **strictly before** the re-read of the tail. Two interleavings:
   * event lands **before** the re-read → the re-read itself returns it
     (no wake needed);
   * event lands **after** the (empty) re-read → the broadcast point has
     not fired yet or fires later; since the waiter is already armed, the
     `notify_waiters()` wake is captured by the armed future and
     `select!` returns immediately.
3. There is no third interleaving: the broadcast (`journal.rs:194-198`)
  happens after visibility in the same commit path, so any event the
  re-read *could* have missed was published after the re-read started,
  hence after the subscription. Subscription-before-read is the same
  discipline the changefeed itself documents
  (`core.rs:220-225`: "subscribe FIRST, then `read_from` the journal").

One subtlety, stated honestly: the **journal read** is bounded by the
completion watermark (§1), which can lag the visibility that fired the
wake. A wake can therefore arrive while the journal page is still empty.
This is harmless, not a lost wakeup: the deadline still bounds the wait,
and the loop's fallback (keep the 50 ms `POLL_STEP` as the select sleep
floor, i.e. `sleep(min(deadline - now, POLL_STEP))` on the timeout arm) re-reads
at the old cadence, so the event is delivered at worst as promptly as
today. Alternative accepted: on wake with an empty page, schedule one
`confirm_through`-style completion wait before re-reading; rejected for
this change as it drags the durability barrier onto the replication read
path (risk without a correctness need).

---

## 4. Invariants

1. **No per-follower leader state.** The only new state is one `Arc<Notify>`
   per *repo* (O(repos), created with the repo instance, never per
   connection). `Notify` internally keeps waker slots for armed waiters,
   which is runtime-managed wait registration, not leader bookkeeping:
   nothing names, counts, registers, times out or cleans up followers, and
   a dropped `NotifiedFuture` removes its slot automatically. The header's
   "not a subscription" (`repl_handler.rs:21-22`) stays true.
2. **No lock a writer can block on.** The commit path appends a single
   `notify_waiters()` to the existing broadcast call site
   (`journal.rs:194-198`); `Notify::notify_waiters` is non-blocking and
   lock-free for the caller. The handler's subscribe path takes no
   `lock_replication_control()` (contrast the db wrapper,
   `changelog.rs:137`) — the new db-layer accessor is a map lookup plus
   `Arc` clone. Writers are never on the waiter's critical path.
3. **The deadline still bounds every wait.** `select!` races the wake
   against a sleep to the *unchanged* `deadline` (`repl_handler.rs:413`);
   wake-and-still-empty loops re-enter the select with the same deadline,
   so worst-case latency for an empty tail is unchanged (~`wait_ms`), and
   worst-case latency for an event drops from 50 ms to the commit
   visibility time. The 30 000 ms clamp (`:411`) is untouched.
4. **No wire-protocol change.** `Pull`/`ReplResponse`, `wait_ms` semantics,
   and error codes are untouched; the follower cannot tell the mechanism
   changed. `CURRENT_REPL_PROTO_VER` is not bumped.
5. **Follower connecting while the leader is idle:** the handler does the
   first read (empty), arms the notify, and parks in `select!`. No events
   fire, no wakes arrive; at the deadline it breaks and returns an empty
   page — byte-identical behaviour to today's loop expiring. Idle-leader
   CPU cost falls from 20 re-reads/s per follower to ~0.
6. **Leader restart:** the `Notify` lives in the repo instance; on restart
   a fresh instance is built and any old waiter is gone with its process.
   A follower's outstanding long poll fails with the connection (as
   today — the leader keeps no follower state to clean up, which is why
   restart needs no grace logic). Followers resume by pulling from their
   last acknowledged version; the always-on journal (§2.1) guarantees no
   committed event is missing across the restart. If a repo is dropped and
   recreated mid-poll, the next lookup returns `None` and the handler
   returns `unknown_repo` via the existing error path (`repl_handler.rs:430-436`).

---

## 5. Required tests

All in `crates/server/shamir-server` (handler unit tests) and
`crates/engine/shamir-engine` (notify plumbing), gated `#[cfg(test)]`.
None may use real sleeps for the wake path — arm the notify and drive the
commit path; only the deadline test uses a (short) real timer.

1. **Read-vs-subscribe race (`lost_wakeup_race_closed`).** Arm a test hook
   so the commit fires *between* the waiter's empty journal read and the
   `select!` (seam: the existing `#[cfg(test)]`
   `fire_post_publish_pre_broadcast_test_hook` point, `journal.rs:201-209`,
   already placed exactly between wake/broadcast in
   `finalize.rs:102-103`). Assert the parked `Pull` returns the event
   without the 50 ms step and within the wait budget.
2. **Deadline expiry with no event (`deadline_bounds_idle_wait`).** `wait_ms`
   well under the test timeout, no commits: assert an empty page is
   returned at ~`wait_ms` (± scheduler slack) and that **zero** journal
   re-reads happened after the subscribe (count via the read path's test
   seam) — this pins the CPU win, not just correctness.
3. **Many waiters woken by one event (`one_event_wakes_all_waiters`).**
   Park K (e.g. 16) concurrent `Pull` handlers on one repo, commit once,
   assert all K return the event and none needed a poll-step timeout
   (each completed well under `POLL_STEP`). Pins `notify_waiters()`
   (not `notify_one`, which would wake one and strand K−1 until their
   deadlines).
4. **No per-follower registry (`no_per_follower_state`).** Structural
   assertion: after K concurrent long polls on the same repo, the repo
   instance exposes exactly one `Notify` (assert pointer equality /
   `Arc::strong_count` within a small bound), and the server holds no map
   keyed by session/node id (grep-level test or a compile-time API shape
   test). Cheap guard against a future "registry" refactor regressing the
   header invariant.
5. **Idle-leader connect (`idle_leader_behaviour`).** Covered by (2) with a
   follower that connects *after* the leader has been idle — asserts the
   first read + armed wait path and the empty-page response shape.

### Flake diagnostics (mandatory per CLAUDE.md)

Tests (1) and (3) are timing-adjacent and must be registered with
`shamir-test-diag`'s `FlakeGuard`
(`crates/devtools/shamir-test-diag/src/guard.rs:64`, `new` `:82`,
`with_watchdog` `:89` for the parked-handler tests that could hang past the
runner's kill; `mark` `:129` / `note` `:138` for timeline points):

* `FlakeGuard::with_watchdog("repl_lost_wakeup", 5s)`; `mark` at
  "initial-read-empty", "subscribed", "commit-published", "woke",
  "returned"; `note` the observed wait budget consumed and the number of
  journal re-reads. `shamir-test-diag` stays a `[dev-dependencies]` of the
  test crates only (repo rule; design in
  `docs/dev-artifacts/design/flake-failure-logs.md`).
* Evidence prints **only on failure** (guard `Drop` while panicking /
  watchdog fire); a green run adds no output, includes no secrets, and
  changes no assertion or timing budget.

---

## 6. Implementation plan

| Step | Files | Content | Risk |
|---|---|---|---|
| 1 | `crates/engine/shamir-engine/src/repo/instance/repo_instance/*` (core struct + `lifecycle/journal.rs`) | Add `tail_notify: Arc<Notify>` field, accessor; `broadcast_changefeed_event` (`journal.rs:194-198`) also `notify_waiters()`. | Low — additive; the call site already runs on both commit paths. |
| 2 | `crates/engine/shamir-db/src/shamir_db/shamir_db/catalog/changelog.rs` | `changelog_tail_notify(db, repo) -> Option<Arc<Notify>>` (lock-free lookup, style of `current_commit_version`, `changelog.rs:202-206`). | Low. |
| 3 | `crates/server/shamir-server/src/engine/db_handler/handlers/repl_handler.rs` | Replace `sleep(remaining.min(POLL_STEP))` (`:422-424`) with the arm-subscribe-read-`select!` loop of §3.1; keep deadline/clamp untouched. Keep `POLL_STEP` as the select timeout granularity fallback. | Medium — the only behavioural change; the race-proof of §3.2 must survive review line-by-line. |
| 4 | server + engine `#[cfg(test)]` tests (§5) + `shamir-test-diag` dev-deps | Tests 1-5 with FlakeGuard instrumentation. | Low-medium; test seam (1) reuses `journal_publish_hook`. |

Not changed: wire types (`shamir-query-types`), storage/`shamir-tx`,
`subscribe_changelog`, the drainer, quarantine gating.

Risk summary: the single substantive risk is a subtle wakeup bug in step 3
(missed arm, `notify_one` used instead of `notify_waiters`, or subscribe
placed after the re-read). The design's defence is the §3.2 ordering rule
plus test 1, which exercises exactly that interleave via the existing
post-publish/pre-broadcast hook.

**Review needed:**

1. **Concurrency review by the replication/tx owner** (the §3.2 race proof
   and the `Notify` arm/enable discipline; `finalize.rs` vs
   `legacy_and_lockfree.rs` call-site parity). This is the blocking review.
2. **Server/API review** that the header invariant "not a subscription"
   still reads true and that no `lock_replication_control` appears on the
   poll path.
3. Standard PR gates (fmt/clippy/layout scoped to the touched crates;
   `./scripts/test.sh @server @engine` in CI) per CLAUDE.md.

Open questions for the owner:

* Should the wake fire from `broadcast_changefeed_event` (visibility) or
  from a journal-completion point? This design chooses visibility + 50 ms
  fallback (§3.2 subtlety); a completion-point wake is stricter but adds a
  barrier wait to the read path.
* Repo-scoped vs db-scoped `Notify`: this design is repo-scoped (matches
  the pull's (db, repo) key); a db-scoped one would trade spurious wakes
  for fewer channels.
* Whether `subscribe_changelog`'s `lock_replication_control()`
  (`changelog.rs:137`) should eventually shrink too — out of scope here.

---

## 7. Review notes (orchestrator, before any implementation)

The mechanism of sections 3 and 4 (one shared `Notify` per repo, subscribe before the final re-read, `notify_waiters()`,
deadline unchanged) is sound. The race proof of 3.2 is correct for tokio's `Notify`: a future created by `notified()` is
guaranteed the wake of a later `notify_waiters()` even before it is polled, and `enable()` makes the arm explicit. Five
points must be settled before the implementation is written:

1. **The fallback poll contradicts the cost claim.** Section 3.2 and step 3 of section 6 keep `POLL_STEP = 50 ms` as the
   timeout arm of the `select!`, which is today's 20 re-reads per second per follower; section 4.5 and test 2 claim "about
   zero" idle cost and "zero re-reads after the subscribe". Pick one. Recommendation: the fallback is COARSE (a new
   constant, for example 1 s, never longer than the remaining deadline), and a wake that finds the page still empty (the
   completion watermark lags visibility, section 1) triggers a short bounded burst of quick re-reads (for example 5, 10,
   20 ms) before the loop parks again. Idle cost is then about one read per second per follower and event latency is the
   visibility time plus the watermark lag. Test 2 then asserts "at most the deadline divided by the fallback", not zero.
2. **Hang the `Notify` on `RepoChangefeed`, not on `RepoInstance`.** The changefeed is the per-repo shared object that
   already owns the broadcast point (`core.rs:264-266`), lives in `shamir-tx`, and is not touched by the R2c stages that
   are rewriting `RepoInstance` into the `RepoCore` shell (tasks 1883, 1886, 1897). Firing `notify_waiters()` inside
   its broadcast keeps step 1 of section 6 out of the files those stages own and lets this change land at any time.
3. **Journal-append parity.** The mechanism is only as complete as the set of paths that call the broadcast. The design
   names the two live commit paths. Before implementing, enumerate EVERY path that appends to the journal (replicated
   apply on a follower that serves a downstream, seed install, restore, migration, the gap markers
   `mark_changefeed_gap` and `record_pending_changefeed_gap`) and say for each whether it broadcasts. A path that appends
   without broadcasting delays its event to the coarse fallback of point 1; that is acceptable only if it is stated.
4. **Test 4 is brittle.** `Arc::strong_count` within "a small bound" will flake with unrelated clones. Assert the API shape
   instead: the accessor returns the one shared handle (pointer equality of two calls) and the handler keeps no map keyed
   by session or node.
5. **Open questions of section 6, with defaults.** Wake at visibility (not at the journal-completion point), repo-scoped
   `Notify`, and `lock_replication_control()` out of scope. Implementation waits for the owner to confirm these three; if
   confirmed as recommended it is small and independent of the R2 chain.
