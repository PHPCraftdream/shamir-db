# shamir-tunables -- Correctness & TDD-coverage

## Summary

The crate is small and internally sound: the `store_defaults`/`instance_defaults` consts are consumed exactly as documented (verified per-consumer: `SLOW_CONSUMER_THRESHOLD` fires on the 100th failure via `>=`, the subscription cap rejects at `active >= cap`, `MAX_UNDRAINED_VERSIONS` really brakes to `high/2` at `commit.rs:489`, `CONN_MAX_IN_FLIGHT` really feeds both the semaphore and the mpsc cap), and the manual `Default` impl + `defaults_equal_consts` test correctly guard against a would-be `#[derive(Default)]` regression to zeros. The substantive problems are in the runtime half: `RuntimeTunables` ships a public override API whose setters are silently ineffective (no consumer in the workspace ever reads the struct), the setters accept degenerate values (`0`, sub-millisecond durations) that consumers are already forced to defensively clamp elsewhere, and one documented env override (`SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD`) does not exist anywhere in the codebase. Test organization follows CLAUDE.md's `tests/` layout faithfully, but per the Red/Green discipline the suite never exercises the degenerate-input or override-takes-effect paths, so these gaps shipped green.

## Findings

### 1. Runtime override API is unwired — all three setters are silent no-ops for real behavior
- **File:line:** `crates/shamir-tunables/src/runtime.rs:36-76`; cross-refs `crates/shamir-server/src/server/server_launcher.rs:900,958-959,1008,1105,1210`, `crates/shamir-server/src/connection/handshake.rs:705,707`, `crates/shamir-server/src/server/server_handle.rs:89-93`
- **Severity:** high
- **Issue:** `runtime.rs` promises "Overrides are rare and just store a new atomic value, taking effect on the next read" and "Instance-level runtime-overridable tunables". In the workspace the only `RuntimeTunables` instance is constructed at `server_launcher.rs:900` (`Arc::new(RuntimeTunables::new())`), stored on the `pub` field `ServerHandle::tunables`, and its accessors (`io_frame_buffer_cap()` / `server_poll_interval()` / `conn_max_in_flight()`) have **zero** non-test call sites — grep confirms every production read goes straight to the compile-time consts (`instance_defaults::CONN_MAX_IN_FLIGHT`, `CONN_IDLE_TIMEOUT`, `SERVER_POLL_INTERVAL` at launcher 958/959/1008/1105/1210; `IO_FRAME_BUFFER_CAP` at handshake 705/707). `server_handle.rs:91-92` admits "Consumer wiring ... is deferred to a follow-up slice", but the crate-side setter API and the `pub` field are already shipped and look functional.
- **Failure scenario:** a caller (test, SDK consumer, future ops hook) does `server.tunables.set_conn_max_in_flight(8)`; the setter compiles, the getter dutifully returns 8, and actual server behavior stays at the const `32` — a silent, undetectable configuration divergence with no error or warning. TDD angle: the crate's entire test suite exercises exactly this unwired surface, and no test anywhere in the workspace asserts any consumer honors a runtime override — green tests over dead plumbing.
- **Suggested fix:** either land the wiring (pass the shared `Arc<RuntimeTunables>` into `ConnectionContext` and the three poll loops and replace the const reads — a small, mechanical diff), or until then mark the setters `#[doc(hidden)]` with a "not yet consumed by any call-site; overrides are no-ops" warning (or remove them), so the API cannot be mistaken for functional. Also reconcile `lib.rs:4-7` ("Today these are plain `const`s ... a later phase promotes") with `runtime.rs`'s present-tense claims.

### 2. Setters accept degenerate values unvalidated; millisecond quantization silently truncates to zero
- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-58` (`set_io_frame_buffer_cap`), `61-64` (`set_server_poll_interval`), `73-75` (`set_conn_max_in_flight`); truncation also on the Default path at `:29`
- **Severity:** medium
- **Issue:** `set_conn_max_in_flight(0)`, `set_io_frame_buffer_cap(0)` and `set_server_poll_interval(Duration::ZERO)` all store verbatim. The wired-path consumer already needs a defensive clamp — `request_loop.rs:153` does `ctx.max_in_flight.max(1)` — because `Semaphore::new(0)` + `mpsc::channel(0)` would block every request on the connection forever. Additionally `server_poll_interval` is stored as `v.as_millis() as u64`: `Duration::from_micros(900)` becomes `Duration::ZERO`, and absurd `Duration` values wrap on the `u128 → u64` cast. `SERVER_POLL_INTERVAL = 0` would turn the three housekeeping loops (`server_launcher.rs:1008/1105/1210`) into hot spins once wired.
- **Failure scenario:** latent until finding 1's wiring lands; then a single bad override (a `0` from a misparsed config, a sub-ms duration) wedges new connections or burns a core, with no validation error pointing at the setter.
- **Suggested fix:** clamp and document the policy inside the setters (e.g. `v.max(1)` for the two `usize` knobs; `Duration::from_millis(v.as_millis().min(u64::MAX as u128).max(1))` for the interval), then pin the policy with red/green tests (see finding 4).

### 3. Phantom env override documented: `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` is read nowhere
- **File:line:** `crates/shamir-tunables/src/lib.rs:149` (doc of `VECTOR_SNAPSHOT_DELTA_THRESHOLD`: "`SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` overrides at startup.")
- **Severity:** medium
- **Issue:** a workspace-wide search finds this identifier only in this doc comment. The only consumer, `shamir-index/src/vector/vector_backend.rs:143`, initializes from the const with no environment read anywhere on the path. The documented override mechanism is fiction, which contradicts this crate's stated role as the single truthful home for knob documentation.
- **Failure scenario:** an operator sets the env var at startup expecting to bound the restart-replay / orphan-chunk footprint; it is silently ignored and capacity planning built on the override is wrong.
- **Suggested fix:** either implement the startup env read at the consumer (and keep this line as its doc anchor) or delete the sentence; if planned-not-built, say "planned" explicitly.

### 4. TDD coverage gaps on the runtime surface: degenerate inputs, truncation, and override-effect never tested
- **File:line:** `crates/shamir-tunables/src/tests/runtime_tests.rs` (whole file)
- **Severity:** low
- **Issue:** the five tests cover defaults-equal-consts and happy-path set/get. `defaults_equal_consts` is genuinely load-bearing (it is what catches a future `#[derive(Default)]`, whose atomic defaults would be zeros) — but per CLAUDE.md's Red/Green/Refactor the edge cases that drove findings 1-2 were never written first: no test sets `0` / `Duration::ZERO` / sub-ms durations (which would have forced the clamp/truncation policy decision in Red), no double-overwrite test, and — the real vacuity — no test anywhere ties an override to observable consumer behavior (currently impossible, since no consumer reads the struct; that is finding 1's root). The current lossy `as_millis()` behavior is thus neither documented as contract nor pinned by a test.
- **Failure scenario:** the suite stays green while the override contract drifts; a future wiring change can silently alter truncation/clamping semantics with no failing test.
- **Suggested fix:** after findings 1-2: add tests asserting the clamp policy for `0`/sub-ms inputs and overwrite-twice semantics; add one shamir-server integration test asserting that lowering `tunables.server_poll_interval()` changes observed poll cadence (the test that would have caught finding 1 in Red).

### 5. `lib.rs` header doc stale relative to shipped `runtime.rs`
- **File:line:** `crates/shamir-tunables/src/lib.rs:3-7`
- **Severity:** nit
- **Issue:** "Today these are plain `const`s (change = edit here + rebuild + benchmark via /opti); a later phase promotes selected knobs to a runtime cascade" — the promotion already partially exists (`runtime.rs`, 3 knobs). Harmless alone, but combined with finding 1 it gives contradictory impressions of what is live.
- **Suggested fix:** one sentence: consts are authoritative today; the `RuntimeTunables` overrides exist but are not yet consumed (see finding 1).
