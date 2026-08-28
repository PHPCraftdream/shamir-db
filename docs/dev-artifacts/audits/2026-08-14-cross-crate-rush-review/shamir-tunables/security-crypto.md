# shamir-tunables -- Security & crypto boundary

## Summary

The crate has no security surface of its own: it is dependency-free (Cargo.toml declares no
dependencies at all), contains no `unsafe`, no I/O, no parsing, no secrets handling, no string
building, and no timing-sensitive comparisons — only plain `const`s and relaxed atomic
loads/stores. No auth/HMAC/TLS code lives here, so no critical or high findings exist. The two
findings below are hardening-level: an unvalidated public override API that can zero-out
security-relevant resource bounds once wired up, and a runtime layer that is dead in production,
so the documented "takes effect on next read" contract is currently false for every
security-relevant knob it mirrors (idle timeout, per-connection concurrency, poll interval).

## Findings

### 1. `RuntimeTunables` setters accept unvalidated values that can disable security-relevant resource bounds
- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-75` (all three setters), `crates/shamir-tunables/src/runtime.rs:61-64` (sub-ms truncation)
- **Severity:** low
- **Issue:** `set_conn_max_in_flight` and `set_server_poll_interval` store any value verbatim
  (`Ordering::Relaxed`), with no non-zero/lower-bound/upper-bound validation and no `Result`.
  These knobs are the resource-exhaustion defenses: `CONN_MAX_IN_FLIGHT` backs the per-connection
  semaphore and mpsc capacity, and `SERVER_POLL_INTERVAL` paces the server accept-error and
  housekeeping loops. `set_server_poll_interval` additionally truncates via
  `v.as_millis() as u64`, so any sub-millisecond duration (e.g. `Duration::from_micros(999)`)
  silently becomes `0`.
- **Failure scenario:** None today — no production caller exists. But the module doc
  (`lib.rs:5-7`) says a later phase "promotes selected knobs to a runtime cascade", and the
  `reads_are_shared_ref` test (`runtime_tests.rs:52-58`) shows the type is an interior-mutable
  `Arc` shareable by any code path. The moment a config/admin surface calls
  `set_conn_max_in_flight(0)`, every request on each new connection stalls permanently
  (semaphore with zero permits); `set_server_poll_interval(Duration::ZERO)` (or any sub-ms
  value) turns the accept-error/housekeeping loops into busy-spin CPU burn — a self-inflicted
  DoS from a misconfigured knob rather than a validated error.
- **Suggested fix:** Validate at the setter boundary: either return
  `Result<(), TunableRangeError>` for `0` (and a sane cap for `io_frame_buffer_cap`), or take
  `NonZeroUsize`/`NonZeroU64` and store saturating values. Round the poll interval up
  (`max(1, v.as_millis())`) so sub-ms inputs cannot degrade to a zero sleep. Add a tests/ case
  covering the zero and sub-ms inputs per the repo's TDD convention.

### 2. Runtime-override layer is dead code — every security-relevant consumer reads the consts, so the "override takes effect" contract is false
- **File:line:** `crates/shamir-tunables/src/runtime.rs:1-76` (claims); consumers at
  `crates/shamir-server/src/server/server_launcher.rs:958-959` and `:1008`,
  `crates/shamir-server/src/connection/handshake.rs:705-707`
- **Severity:** low
- **Issue:** `RuntimeTunables` is constructed (`server_launcher.rs:900`) and stored on
  `ServerHandle` (`server_handle.rs:93`, `pub tunables: Arc<RuntimeTunables>`), but a workspace
  grep finds zero production readers of any getter and zero callers of any setter — only
  `shamir-tunables`' own tests touch them. Meanwhile the security-relevant consumers bypass the
  runtime layer entirely: `CONN_MAX_IN_FLIGHT`/`CONN_IDLE_TIMEOUT` are read from
  `instance_defaults` consts at `server_launcher.rs:958-959` when building each
  `ConnectionContext`, `SERVER_POLL_INTERVAL` at `:1008`/`:1105`/`:1210`, and
  `IO_FRAME_BUFFER_CAP` at `handshake.rs:705-707`. `CONN_IDLE_TIMEOUT` is the control that
  closes abandoned *authenticated* connections (its doc explicitly frames it as a
  session-slot/socket-lifetime bound), and `CONN_MAX_IN_FLIGHT` is a per-connection
  resource-concurrency bound.
- **Failure scenario:** An operator (or future config surface) "tunes" the idle timeout or
  in-flight bound through `ServerHandle::tunables` and observes no effect — a security control
  that appears operable but is not, the same documented-vs-actual drift class the repo has
  called out elsewhere (CLAUDE.md F-9/#1076 revision). It also masks finding 1 until the wiring
  lands.
- **Suggested fix:** Either route the consumers above through
  `ServerHandle::tunables.<getter>()` (then finding 1's validation becomes load-bearing), or
  mark the runtime layer/docs explicitly as not-yet-wired so no one treats the knobs as live
  controls. Cheapest honest option today: a doc-comment note on `RuntimeTunables` naming the
  concrete consumers that must be switched before overrides mean anything.
