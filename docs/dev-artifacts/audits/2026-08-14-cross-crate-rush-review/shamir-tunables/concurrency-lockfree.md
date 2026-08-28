# shamir-tunables -- Concurrency & lock-free invariants

## Summary

The crate is exemplary on the five-pillar checklist: it is a pure-`const` module plus one three-field atomic struct (`RuntimeTunables`) with zero locks, zero `.await`s, and no `scc`/`DashMap`/hash-map state at all (`Cargo.toml` declares no dependencies), so none of the banned hot-path primitives can exist here. Every atomic access is correctly `Ordering::Relaxed` (independent knobs, no cross-variable ordering promised) and the "zero-overhead read" contention model is documented inline, as the ideology requires. The substantive issues found are adjacent rather than pillar violations: the lock-free runtime-override primitive is dead scaffolding in production (every consumer still reads the compile-time consts), and its setters accept values that would create concurrency hazards (zero-permit semaphore, zero-interval spin) the day it is wired.

## Findings

### 1. `RuntimeTunables` override path has zero live consumers — all hot paths read the compile-time consts, so a runtime override is a silent no-op

- **File:line:** `crates/shamir-tunables/src/runtime.rs:18-22` (struct); evidence: `crates/shamir-server/src/server/server_handle.rs:93` (`pub tunables: Arc<RuntimeTunables>` — stored, never read), `crates/shamir-server/src/server/server_launcher.rs:900` (constructed) vs. `server_launcher.rs:958`, `:1008`, `:1105`, `:1210` and `crates/shamir-server/src/connection/handshake.rs:705-707` (all read `instance_defaults::*` consts directly).
- **Severity:** medium
- **Issue:** The crate's only concurrency primitive — the lock-free `RuntimeTunables` atomics — is constructed once in `shamir-server` and published on the public `ServerHandle::tunables` field, but no production call site ever invokes `io_frame_buffer_cap()`, `conn_max_in_flight()`, or `server_poll_interval()` (grep across the workspace: getter/setter calls exist only in `src/tests/runtime_tests.rs`). Every real consumer (per-connection semaphore sizing, housekeeping poll sleeps, frame-buffer capacities) hardcodes the `instance_defaults` consts. The struct's doc promise "Overrides are rare and just store a new atomic value, taking effect on the next read" (`runtime.rs:4-5,14`) is therefore true only for getters nothing calls: two sources of truth exist and the runtime one is already disconnected from all three wired behaviors.
- **Failure scenario:** An operator (or future test/bench) sets `set_conn_max_in_flight(8)` on `ServerHandle::tunables` — a public, `Arc`-shared, `&self`-settable API that invites exactly this — and the running server's semaphore size, poll interval, and buffer caps never change; observed behavior silently contradicts the configured value with no error.
- **Suggested fix:** Either wire the three hot paths through the getters (the `lib.rs:4-7` doc says promotion to the runtime cascade is a "later phase" — until then the scaffolding status should be explicit), or delete `RuntimeTunables` until that phase lands. If kept unwired, state it in the struct docs (e.g. "no consumer yet; overrides are currently inert") so nobody trusts an override, and add a workspace grep-able marker so the const call sites are findable when the promotion happens.

### 2. Setters accept concurrency-breaking values (0 / sub-millisecond) with no validation or documented floor

- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-58, 61-64, 73-75`; consumer semantics documented at `crates/shamir-tunables/src/lib.rs:41-48`.
- **Severity:** low (latent — only bites once finding 1 is fixed and consumers are wired)
- **Issue:** `set_conn_max_in_flight(0)` is stored verbatim; the documented consumers are "the per-connection semaphore (reader back-pressure) and the mpsc channel capacity to the writer task". A zero-permit semaphore makes every pipelined read `acquire()` forever (permanent per-connection hang — precisely the class of "deadlock" CLAUDE.md's test discipline treats as a bug), and a 0-capacity mpsc degrades to rendezvous semantics. `set_server_poll_interval` truncates via `as_millis() as u64`, so `Duration::from_micros(500)` silently becomes 0 ms; a housekeeping loop sleeping 0 ms busy-spins a tokio worker (livelock/starvation of co-scheduled tasks). The `Relaxed` ordering itself is correct here — the hazard is purely value-domain, not memory-ordering.
- **Failure scenario:** After wiring (finding 1), an operator applies `0` or a sub-millisecond interval at runtime; new connections deadlock on the semaphore, or a poll loop pins a worker thread and starves other tasks on the runtime.
- **Suggested fix:** Clamp or reject in the setters — a documented floor (e.g. `max(1)` for `conn_max_in_flight`, a minimum poll interval, or `debug_assert!` + `Result` return) — or make the getters return the clamped value so no consumer can observe a hazardous raw store. One-line inline comments naming the floor satisfy the repo's "justify inline" convention.

### 3. No test pins cross-thread visibility of an override; `reads_are_shared_ref` runs single-threaded

- **File:line:** `crates/shamir-tunables/src/tests/runtime_tests.rs:52-58`.
- **Severity:** nit
- **Issue:** The claim this crate exists to make is "Reads are a single atomic load (instant, cached, lock-free, non-blocking)" with overrides visible "on the next read" (`runtime.rs:1-6,13-16`). The suite covers defaults, set-then-read, and `Arc` callability, but never a store issued in one thread observed by a load in another. (A spawn-then-join pattern makes this deterministic — join establishes happens-before even for `Relaxed` — no flaky spin loop needed.) Coverage is otherwise appropriate for so small a crate, and the test layout conforms to the repo's `tests/` organization rules.
- **Suggested fix:** One test that spawns a writer thread storing a value, joins, then asserts the reader sees it, plus a compile-time `fn assert_send_sync<T: Send + Sync>()` for `RuntimeTunables`, so the lock-free sharing contract is pinned rather than incidental.

---

*Scope: only `crates/shamir-tunables/` (`Cargo.toml`, `src/lib.rs`, `src/runtime.rs`, `src/tests/`) plus consumer tracing in `shamir-server` to ground failure scenarios. No `scc::*::len()` call sites exist in the crate; no `Mutex`/`RwLock`/`parking_lot`, no `.await`, no hash-keyed structure — pillar 1/2/3/5 are satisfied trivially, pillar 4 (Fx hash) is not applicable.*
