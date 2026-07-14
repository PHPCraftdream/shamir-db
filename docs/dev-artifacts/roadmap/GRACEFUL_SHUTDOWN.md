בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Milestone: Graceful shutdown

**The linchpin.** Every run-mode (foreground, Windows service, Linux
systemd) funnels into ONE deterministic shutdown path — "one door out", the
mirror of Shomer's one door in. Prerequisite for `RUNTIME_MODES.md`.

## Why
A clean shutdown must: stop accepting work, let in-flight work finish (or
abort cleanly within a deadline), make all acked writes durable, and
**release storage file locks** — so the process can exit and be restarted
or re-opened immediately. (Bonus: this is the ROOT fix for the redb
"Database already open. Cannot acquire lock." reopen race we de-flaked with
`init_redb_retry` — a clean close releases the lock deterministically.)

## Invariant
- **No lost acked writes.** Anything the client got an ack for is durable
  after shutdown.
- **Bounded.** Shutdown completes within a deadline; past it, force-abort
  remaining in-flight work (and log what was dropped — no silent loss).
- **Idempotent + signal-safe.** Re-entrant shutdown signals are harmless.
- Behavior-preserving for normal operation (only adds the shutdown path).

## Slices (each = one agent delegation; zero-trust + green gate)

### GS-1 — ShutdownController (the signal/broadcast)
- A `ShutdownController` exposing: a `triggered()` future (resolves on the
  first trigger), a `trigger()` handle (programmatic), and a `subscribe()`
  for subsystems (tokio `broadcast`/`Notify`/`watch`). Sources wired by the
  caller: `ctrl_c`, unix `SIGTERM`, programmatic (service control handler).
- Where: `shamir-server` (or a small `shutdown` module). No behavior change
  yet (nothing subscribes).
- Tests: trigger resolves all subscribers once; re-trigger is a no-op.

### GS-2 — ServerLauncher.run_until(shutdown) — drain the front
- Stop accepting new connections (drop/close the TCP/WS listeners on
  trigger); track in-flight requests; await their completion up to the
  deadline, then force-abort.
- Where: `shamir-server` (the accept loop + ServerLauncher).
- Acceptance: a connected client mid-request is allowed to finish (within
  deadline); new connections after trigger are refused; returns after drain.
- Tests: in-process — start, connect, trigger shutdown, assert in-flight
  request completed and the loop returned within the deadline.

### GS-3 — Storage/engine clean close — flush + release locks
- `ShamirDb::shutdown()` (awaitable): finish/await in-flight commits + the
  background flushers (MemBuffer pump, any periodic tasks), `flush()` the
  WAL/stores, then DROP the backend handles so file locks (redb etc.) are
  released. Make it await all spawned background tasks (no detached task
  keeping the DB/lock alive).
- Where: `shamir-db` facade + `shamir-engine` (background tasks must be
  joinable — hold JoinHandles or use a shutdown token they observe).
- Acceptance: after `shutdown()`, re-opening the SAME redb path succeeds
  IMMEDIATELY (no lock error) → the `init_redb_retry` test helper can be
  REMOVED (its removal is the proof).
- Tests: open redb → write (acked) → `shutdown()` → reopen immediately
  (no retry) → the acked write is present; background flushers are observed
  to have stopped.

### GS-4 — End-to-end shutdown coverage
- Tests: graceful shutdown completes within the deadline; acked writes
  survive a shutdown→reopen; an in-flight tx either commits or rolls back
  cleanly (never half-applied); force-abort fires past the deadline and logs
  the dropped work. Wire into the existing in-process server↔client harness.
- Remove the `init_redb_retry` de-flake in `functions_lifecycle.rs` (now
  unnecessary) — keep it green without the retry.

## Acceptance for the milestone
Signal → clean exit within deadline; no lost acked writes; locks released
(immediate reopen, retry helper removed); all existing tests green
(`fmt --all --check`, `clippy --workspace --all-targets -D warnings`,
`test --workspace --lib`, `test --workspace --test '*'`). A bench is
optional (shutdown is cold-path); tests are the coverage.

## For agents
Order GS-1 → GS-2 → GS-3 → GS-4. Each is a `/crush` slice, behavior-
preserving except for adding the shutdown path, verified zero-trust + gate.
GS-3 is the meatiest (background-task joinability + lock release) and the
highest-value (fixes the reopen race at the root).
