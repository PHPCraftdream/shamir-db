# shamir-tunables -- Error handling & resource lifecycle

## Summary

The crate is a near-trivial home for compile-time consts plus a three-atomics
`RuntimeTunables` struct; it contains no `unwrap`/`expect`/`panic!`, holds no
OS or task resources (so error-path cleanup is trivially N/A), and follows the
workspace test layout (`tests/` dir, manifest-only `mod.rs`). The theme's real
exposure is API-shaped: the runtime setters are infallible yet accept values
that arm downstream panics, deadlocks, or busy-spins (latent today, because
`RuntimeTunables` is plumbed into `ServerHandle` but nothing reads it yet —
all server call-sites still read the compile-time consts). Secondary gap: the
test suite covers only happy-path set/read; no boundary/error-path cases
exist, which is possible precisely because the API has no error paths at all.

## Findings

### 1. Infallible setters accept values that arm downstream panics / zero-permit deadlocks
- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-58` (`set_io_frame_buffer_cap`), `crates/shamir-tunables/src/runtime.rs:73-75` (`set_conn_max_in_flight`)
- **Severity:** medium
- **Issue:** Both setters take an unbounded `usize` and store it with no validation, no documented valid range, and no `Result` — contra the CLAUDE.md error-handling pillar ("Return `Result<T, E>`"; panics are for programmer bugs). The values' only known consumers are allocation/concurrency primitives: `io_frame_buffer_cap` feeds `Vec::with_capacity(...)` (`crates/shamir-server/src/connection/handshake.rs:704-707`, where `usize::MAX` is a hard capacity-overflow panic), and `conn_max_in_flight` feeds a per-connection `Semaphore::new(cap)` + `mpsc::channel(cap)` (`crates/shamir-server/src/connection/request_loop.rs:153-155`, where `0` permits would stall every request).
- **Failure scenario:** Latent today: every server call-site reads the compile-time consts, and `request_loop.rs:153` defensively clamps `ctx.max_in_flight.max(1)` — but only for the const-fed path. The moment the server switches to the already-plumbed `ServerHandle.tunables` reads (`crates/shamir-server/src/server/server_handle.rs:93`), `set_io_frame_buffer_cap(usize::MAX)` (or any value exceeding addressable memory) panics the per-connection task inside `with_capacity`, and the `≥ 1` invariant for `conn_max_in_flight` survives only if each new consumer remembers to re-derive the `.max(1)` clamp that currently lives in the wrong crate.
- **Suggested fix:** Make the setters honest per the house rules: either return `Result<(), TunablesError>` (a small `thiserror` enum — the crate currently has none), or validate-and-clamp at the boundary (`max(1)` here, plus a sane upper ceiling for the buffer cap) so the invariant is owned by the type, not re-derived per consumer. At minimum, document the accepted range on each setter.

### 2. `set_server_poll_interval` silently truncates/wraps and accepts a hot-spin value
- **File:line:** `crates/shamir-tunables/src/runtime.rs:61-64`
- **Severity:** medium
- **Issue:** The setter stores `v.as_millis() as u64` — a truncating `u128 → u64` `as` cast with no error path or doc note. Millisecond quantization is silent (sub-ms durations become `0`), and `Duration::MAX.as_millis()` (~1.8e22) exceeds `u64::MAX` by ~1000×, so the cast wraps to an arbitrary garbage interval. `Duration::ZERO` is also accepted, storing `0`.
- **Failure scenario:** When wired to the poll loops that will consume it (today they sleep on the const: `crates/shamir-server/src/server/server_launcher.rs:1008,1105,1210` — e.g. the TCP accept-error backoff), a wrapped `Duration::MAX` yields a randomly small retry interval instead of "very long", and a stored `0` turns the backoff/poll sleeps into a hot spin (busy CPU in a loop whose whole purpose is to *not* burn CPU after accept errors).
- **Suggested fix:** Saturate instead of truncate (`u64::try_from(v.as_millis()).unwrap_or(u64::MAX)`), reject or floor `Duration::ZERO` / sub-ms input (return `Result` or clamp to `1 ms`), and document the millisecond precision on the setter. Given the knob's backoff role, floor at `1 ms` and cap at a sane ceiling.

### 3. `RuntimeTunables` is dead plumbing — runtime override path has zero readers
- **File:line:** `crates/shamir-tunables/src/runtime.rs:36-76`; `crates/shamir-server/src/server/server_handle.rs:93`; `crates/shamir-server/src/server/server_launcher.rs:900`
- **Severity:** low
- **Issue:** `ServerLauncher` constructs `Arc::new(RuntimeTunables::new())` into `ServerHandle.tunables`, but no code anywhere in the workspace calls any getter or setter on it (grep: sole references are the field decl and the constructor). Every tunable consumer still reads the compile-time `instance_defaults` consts directly.
- **Failure scenario:** Not a runtime failure — a lifecycle/hygiene one. The struct's doc promises "overrides … taking effect on the next read", but no read path exists, so the validation gaps in findings 1–2 stay invisible until someone flips the call-sites over; at that point behavior silently diverges from every doc comment and test (which only pin defaults), and the setters' first live callers are also their first testers.
- **Suggested fix:** Either wire the reads (replace the const reads at `server_launcher.rs:958-959/1008/1105/1210` and `handshake.rs:704-707` with `tunables.*()`), or, if the cascade phase is still distant, remove the `tunables` field until then. If kept as scaffolding, mark it as such in the doc so reviewers know the override path is unexercised.

### 4. No boundary/error-path tests for the runtime setters
- **File:line:** `crates/shamir-tunables/src/tests/runtime_tests.rs:7-58`
- **Severity:** low
- **Issue:** The five tests cover only happy paths: defaults-equal-consts, one normal set/read per knob, and Arc sharing. There are no tests for `set_conn_max_in_flight(0)`, `set_io_frame_buffer_cap(0)` / huge values, `set_server_poll_interval(Duration::ZERO)` / sub-ms inputs / `Duration::MAX` (the truncating-cast wrap in finding 2), i.e. none of the cases where the current API's behavior is actually undefined. This is a direct consequence of the API having no error paths — the "missing error-path tests" are un-writable until findings 1–2 give the setters defined behavior.
- **Failure scenario:** When validation (or wiring) lands, none of these edge cases are pinned; a refactor of the storage representation (e.g. millis→micros, or `AtomicU64`→`AtomicUsize`) can silently change truncation/wrap behavior with a green suite.
- **Suggested fix:** After (or alongside) findings 1–2, add boundary tests: rejected/clamped zero and overflow inputs per knob, sub-ms interval → documented floor, `Duration::MAX` → saturation, and keep `defaults_equal_consts` as the drift guard it already is.

### Positive observations (no action)
- No `unwrap`/`expect`/`panic!`/`todo!` in `src/`; the only asserts are in tests — panic-avoidance pillar clean for what the crate contains.
- No `anyhow`/`Box<dyn Error>` leakage; no error enum is warranted yet (nothing fallible is actually performed — `Atomic*::store` is infallible).
- No resources held (no files, locks, tasks, sockets), so there is no error-path cleanup surface to audit; `Drop` needs are nil.
- Test organization matches CLAUDE.md exactly: `src/tests/mod.rs` is a manifest-only re-export, `lib.rs` wires `#[cfg(test)] mod tests;`, imports are at file top.
