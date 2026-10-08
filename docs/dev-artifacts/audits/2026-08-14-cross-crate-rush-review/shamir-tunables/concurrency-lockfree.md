<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-tunables — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Independent atomic storage is appropriate. Duration conversion and a narrow cross-thread regression-coverage gap remain; intentional absence of production consumers is not a concurrency defect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 2 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `RuntimeTunables` override path has zero live consumers — all hot paths read the compile-time consts, so a runtime override is a silent no-op

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The factual absence is confirmed, but the roadmap deliberately defers these consumers. Local state does change. No synchronization or publication obligation is broken.

Evidence: [docs/dev-artifacts/roadmap/TUNABLES.md:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/roadmap/TUNABLES.md#L180); [crates/shamir-server/src/server/server_handle.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_handle.rs#L98); [crates/shamir-tunables/src/runtime.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L57).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Setters accept concurrency-breaking values (0 / sub-millisecond) with no validation or documented floor

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The interval conversion is undocumented and lossy. Concurrency-breaking consequences are not established: max(1) protects both request primitives, and no runtime getter currently feeds the sleeps.

Evidence: [crates/shamir-tunables/src/runtime.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-server/src/connection/request_loop.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L153); [Cargo.lock:4196](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4196).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — No test pins cross-thread visibility of an override; `reads_are_shared_ref` runs single-threaded

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Arc cloning and both accesses occur on one thread. A writer joined before reading can guard synchronized sharing and Send + Sync compatibility; it cannot prove immediate cross-thread freshness without synchronization.

Evidence: [crates/shamir-tunables/src/tests/runtime_tests.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/tests/runtime_tests.rs#L52); [crates/shamir-tunables/src/runtime.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L18).

## Evidence and recipe corrections

- The no-lock/no-map/no-await observation is supported only for this crate, not all downstream consumers.
- Tokio 1.49.0 published source rejects bounded mpsc capacity zero and caps permits at usize::MAX &gt;&gt; 3: https://docs.rs/crate/tokio/1.49.0/source/src/sync/mpsc/bounded.rs and https://docs.rs/crate/tokio/1.49.0/source/src/sync/batch_semaphore.rs. Frozen pin: Cargo.lock:4196.
- Published Tokio 1.49.0 Sleep cooperative-budget polling and deadline rounding refute inevitable monopolization, not every possible high-load consequence of shortening an error backoff.
- Do not strengthen ordering merely to make next-read wording sound global. No payload-publication or multi-knob snapshot mechanism exists.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
