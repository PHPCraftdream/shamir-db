<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-tunables — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The wire-free boundary remains clean. Wiring and setter-domain documentation remain open; mandatory knob symmetry and mandatory relocation of the correctly registered root tests are refuted.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 2 | 0 | 0 | 2 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `RuntimeTunables` is public and documented as effective, but unwired -- every consumer reads the compiled consts

Status: `confirmed-open`. Current risk: `medium`.

The public object still has no production getter consumers. Deferral exists in server documentation and the roadmap, but not alongside the crate's effective-override claims.

Evidence: [crates/shamir-tunables/src/runtime.rs:4](../../../../../crates/shamir-tunables/src/runtime.rs#L4); [crates/shamir-server/src/server/server_handle.rs:98](../../../../../crates/shamir-server/src/server/server_handle.rs#L98); [crates/shamir-server/src/server/server_launcher.rs:1048](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1048); [docs/dev-artifacts/roadmap/TUNABLES.md:179](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L179).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Setters accept out-of-domain values silently; millisecond truncation is undocumented

Status: `confirmed-open`. Current risk: `medium`.

All setters remain infallible and specify no domain or quantization policy. Zero/sub-ms conversion and large-duration narrowing are unchanged. The relevant issue is an undefined API policy, not that atomic stores themselves require Result.

Evidence: [crates/shamir-tunables/src/runtime.rs:55](../../../../../crates/shamir-tunables/src/runtime.rs#L55); [crates/shamir-tunables/src/runtime.rs:60](../../../../../crates/shamir-tunables/src/runtime.rs#L60); [crates/shamir-tunables/src/runtime.rs:63](../../../../../crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-tunables/src/runtime.rs:72](../../../../../crates/shamir-tunables/src/runtime.rs#L72).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Runtime knob selection is asymmetric within a single consumption site

Status: `refuted`. Current risk: —.

Different promotion status is real but does not violate a promised symmetric API. The roadmap positively specifies promotion only on genuine need and explicitly defers consumer wiring. No idle-timeout override is promised. Application timing must be designed if live wiring is chosen.

Evidence: [docs/dev-artifacts/roadmap/TUNABLES.md:113](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L113); [docs/dev-artifacts/roadmap/TUNABLES.md:172](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L172); [docs/dev-artifacts/roadmap/TUNABLES.md:179](../../../../../docs/dev-artifacts/roadmap/TUNABLES.md#L179); [crates/shamir-tunables/src/runtime.rs:18](../../../../../crates/shamir-tunables/src/runtime.rs#L18).

<a id="review-4"></a>

### Claim 4 — Test directory placement deviates from the per-module `tests/` convention

Status: `refuted`. Current risk: —.

These tests are owned and registered by the crate-root module, which has one tests directory and a manifest-only topic module. The rules do not require root-owned tests to be nested under the implementation file they exercise. Commit dd12593f explicitly migrated this crate into the present convention.

Evidence: [crates/shamir-tunables/src/lib.rs:11](../../../../../crates/shamir-tunables/src/lib.rs#L11); [crates/shamir-tunables/src/tests/mod.rs:1](../../../../../crates/shamir-tunables/src/tests/mod.rs#L1); [AGENTS.md:127](../../../../../AGENTS.md#L127); [CLAUDE.md:575](../../../../../CLAUDE.md#L575).

## Corrections and qualified non-findings

- Clean boundary claims remain valid: no serialization/query construction/dependencies; version 0.1.0-alpha.1 and publish=false are current (crates/shamir-tunables/Cargo.toml:3,4; Cargo.lock:3785).
- All 17 constants still have production uses, including HISTORY_SCAN_BATCH in crates/shamir-tx/src/mvcc_store/mvcc_history.rs:30 and the vector thresholds in crates/shamir-index/src/vector/vector_backend.rs:143.
- WAL decoupling remains valid: the caller supplies the threshold, and shamir-wal has no tunables dependency (crates/shamir-engine/src/repo/repo_instance.rs:825,830; crates/shamir-wal/src/segment_set.rs:88; crates/shamir-wal/Cargo.toml:9).
- Finding 3's per-listener boot snapshot observation is correct, but its semaphore is constructed per connection, not at listener boot (crates/shamir-server/src/server/server_launcher.rs:1023; crates/shamir-server/src/connection/request_loop.rs:154). Replacing only build_ctx's constant read would not enable post-launch changes for new connections.
- Idle-timeout promotion and test relocation are optional design/style choices, not necessary fixes for demonstrated defects.
- The suggested wire-up is not merely replacing three sites: there are now five accept-error backoff sites, and the shared object is currently created after listener tasks are launched.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
