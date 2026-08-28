# shamir-tunables -- API & wire-protocol design

## Summary

A zero-dependency crate (no serde at all) holding two compile-time const modules plus one runtime-overridable struct, `RuntimeTunables` (relaxed atomic loads/stores). Wire-protocol exposure is nil by design -- nothing is serialized, no wire formats are defined, and the builder-only query-construction rule is trivially satisfied (no `serde_json` / `json!` / `from_value` anywhere under `src/`), so there are no serialization or versioning findings. The interface-quality problem is concentrated in the runtime half: the public `RuntimeTunables` API is fully unwired (every production site still reads the compiled consts), its setters accept out-of-domain values silently (0, sub-millisecond), and the deferral is documented only in the consumer crate, not here.

## Findings

### 1. `RuntimeTunables` is public and documented as effective, but unwired -- every consumer reads the compiled consts
- **File:line:** `crates/shamir-tunables/src/runtime.rs:36-76` (whole impl); evidence in `crates/shamir-server/src/server/server_launcher.rs:900, 958-959, 1008` and `crates/shamir-server/src/connection/handshake.rs:704-707`
- **Severity:** medium
- **Issue:** No production code in the workspace calls `io_frame_buffer_cap()`, `server_poll_interval()`, `conn_max_in_flight()` or any setter -- the only callers are this crate's own tests (`src/tests/runtime_tests.rs`). `server_launcher.rs:900` constructs `Arc::new(RuntimeTunables::new())` and carries it on the `pub` `ServerHandle::tunables` field, yet `build_ctx` snapshots the consts `CONN_MAX_IN_FLIGHT` / `CONN_IDLE_TIMEOUT` (`server_launcher.rs:958-959`), all three accept loops sleep on the const `SERVER_POLL_INTERVAL` (`:1008, :1105, :1210`), and the handshake preallocates from the const `IO_FRAME_BUFFER_CAP` (`handshake.rs:704-707`). The deferral is documented only in the *consumer* (`server_handle.rs:89-93`, "wiring ... deferred to a follow-up slice"), while this crate's own docs (`runtime.rs:1-16`, `lib.rs:4-7`: "an untouched instance behaves exactly as the consts", overrides "take effect on the next read") imply a *touched* instance does change.
- **Failure scenario:** An operator (or future admin surface built on the `pub` `ServerHandle::tunables` field) calls `set_conn_max_in_flight(8)`; the getter confirms `8`; the server keeps admitting 32 pipelined requests per connection. Silent divergence between API state and behavior -- the worst failure mode for a knob API.
- **Suggested fix:** Either wire the three knobs (accept-loop sleeps read `tunables.server_poll_interval()` per iteration; `ConnectionContext` construction takes the `Arc` and reads per accepted connection), or until wired repeat the deferral note from `server_handle.rs` inside `runtime.rs`'s docs and mark the setters `#[doc(hidden)]` / experimental so nobody builds on them prematurely.

### 2. Setters accept out-of-domain values silently; millisecond truncation is undocumented
- **File:line:** `crates/shamir-tunables/src/runtime.rs:56-75` (esp. `:61-64`)
- **Severity:** medium
- **Issue:** `set_server_poll_interval` stores `v.as_millis() as u64`: a sub-millisecond duration (`Duration::from_micros(100)`) silently becomes 0 ms, and `Duration::ZERO` is accepted outright. Once the knob is wired, that turns the accept-error backoff (`server_launcher.rs:1008`) into a 0-ms busy-spin. Likewise `set_conn_max_in_flight(0)` and `set_io_frame_buffer_cap(0)` are accepted; a zero-permit connection semaphore would stall every pipelined request forever. Setters return `()` and document no valid domain, which sidesteps the project's error-handling rule (`Result<T, E>` for fallible ops) by silently coercing invalid input instead of rejecting it.
- **Failure scenario:** Wiring lands as-is; a config path or test passes `Duration::from_millis(0)` (or sub-ms) as a "disable backoff" shortcut; the accept loop spins at 100% CPU on its error path with no error surfaced.
- **Suggested fix:** Return `Result<(), TunableError>` (or clamp per a documented policy) rejecting `0` / sub-ms for the poll interval and `0` for the semaphore/in-flight cap -- or at minimum document the valid domain (whole milliseconds, >= 1) on each setter's doc comment.

### 3. Runtime knob selection is asymmetric within a single consumption site
- **File:line:** `crates/shamir-tunables/src/runtime.rs:18-22` vs `instance_defaults::CONN_IDLE_TIMEOUT` (`crates/shamir-tunables/src/lib.rs:50-55`)
- **Severity:** low
- **Issue:** `RuntimeTunables` promotes `conn_max_in_flight` but not `conn_idle_timeout`, although the sole consumer (`build_ctx`, `server_launcher.rs:958-959`) reads both side by side and both are instance-level defaults. When wiring lands, idle timeout stays compile-time-only while its sibling becomes runtime-tunable -- an arbitrary split from an API consumer's perspective. Related semantics gap worth closing at the same time: the context/semaphore are snapshotted once per listener at boot, so "override takes effect on the next read" needs a defined rule ("applies to connections accepted after the override") for `conn_max_in_flight` to be honest.
- **Suggested fix:** Promote `conn_idle_timeout` (millis in an `AtomicU64`, mirroring the existing pattern) alongside `conn_max_in_flight`, or document the criteria by which knobs are selected for the runtime cascade.

### 4. Test directory placement deviates from the per-module `tests/` convention
- **File:line:** `crates/shamir-tunables/src/tests/runtime_tests.rs` (manifest `src/tests/mod.rs`)
- **Severity:** nit
- **Issue:** CLAUDE.md prescribes one `tests/` directory per module (e.g. `src/types/tests/`); the `runtime` module's tests live in a crate-root `src/tests/` instead. The layout is otherwise compliant -- manifest-only `mod.rs`, one topic-split file, wired via `#[cfg(test)] mod tests;` in `lib.rs`, no inline test blocks -- and with a single testable module it is harmless today, but it will fragment as knobs get promoted. Test coverage itself is appropriate for what the API currently does: a defaults==consts drift guard (`defaults_equal_consts`), per-knob set/read round-trips, and `Arc` shareability. Coverage cannot extend to integration because none exists (see finding 1).
- **Suggested fix:** Move to `src/runtime/tests/` on the next touch, or amend the convention to sanction crate-root `src/tests/` for single-module crates.

### Verified clean for this theme (no findings)

- **Builder-only query construction:** no `serde_json`, `json!`, `from_value`, or `to_value` anywhere under `src/`; the crate constructs no queries, filters, batches, or wire ops. Compliant.
- **Serialization/versioning:** no serde dependency (`Cargo.toml` has zero dependencies); nothing crosses the wire from this crate; version `0.1.0-alpha.1`, `publish = false`. Nothing to get wrong.
- **Dead public surface:** every const has at least one live consumer -- `SLOW_CONSUMER_THRESHOLD` (`shamir-server/src/subscriptions/push.rs:101`), `JOURNAL_BACKFILL_LIMIT` (`shamir-server/src/subscriptions/bridge.rs:229`), `VECTOR_SNAPSHOT_DELTA_THRESHOLD` / `VECTOR_COMPACTION_*` (`shamir-index/src/vector/vector_backend.rs:143-147`), the rest per engine/tx/index/storage call sites.
- **API boundary discipline:** `WAL_SEGMENT_MAX_BYTES`'s doc (`lib.rs:113-115`) explicitly records that `shamir-wal` takes the bound as a parameter to avoid a dependency on this crate -- good decoupling, keep it.
