<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-tunables — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

There is no internal resource-lifecycle failure. Duration loss is current; allocation/concurrency panics described for future wiring are compatibility constraints rather than present setter failures.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 2 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Infallible setters accept values that arm downstream panics / zero-permit deadlocks

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The usize setters store all accepted values exactly and have no documented restricted domain. Production does not consume them. Future Vec/Semaphore consumers would require an upper policy, but the actual request loop already floors zero; arbitrary storage alone is not a current panic or fallible-operation violation.

Evidence: [crates/shamir-tunables/src/runtime.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L56); [crates/shamir-tunables/src/runtime.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L73); [crates/shamir-server/src/connection/request_loop.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L153); [docs/dev-artifacts/roadmap/TUNABLES.md:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/roadmap/TUNABLES.md#L179).

<a id="review-2"></a>

### Claim 2 — `set_server_poll_interval` silently truncates/wraps and accepts a hot-spin value

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The lossy conversion is deterministic and undocumented. Exactly 2^64 milliseconds wraps to zero; sub-millisecond input also loses precision. Duration::MAX becomes u64::MAX milliseconds. No hot-spin execution follows without new consumer wiring and sustained accept errors.

Evidence: [crates/shamir-tunables/src/runtime.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L61); [crates/shamir-tunables/src/runtime.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L63); [crates/shamir-server/src/server/server_launcher.rs:1093](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1093).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `RuntimeTunables` is dead plumbing — runtime override path has zero readers

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Intentional deferred foundation state has no unbalanced acquisition/release or required asynchronous cleanup. Its lack of server readers is documented at the public owner.

Evidence: [crates/shamir-server/src/server/server_handle.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_handle.rs#L98); [docs/dev-artifacts/roadmap/TUNABLES.md:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/roadmap/TUNABLES.md#L175).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — No boundary/error-path tests for the runtime setters

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The tests omit interval precision/range boundaries and repeat updates. Such boundary tests are possible now; the current API has no existing error-return path to cover.

Evidence: [crates/shamir-tunables/src/tests/runtime_tests.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/tests/runtime_tests.rs#L35); [crates/shamir-tunables/src/runtime.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tunables/src/runtime.rs#L61).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

## Evidence and recipe corrections

- Retain the correction that defined narrowing is not undefined behavior.
- Future wiring must respect published Tokio 1.49.0's usize::MAX &gt;&gt; 3 permit limit; a lower floor or NonZero type alone is insufficient. Source: https://docs.rs/crate/tokio/1.49.0/source/src/sync/batch_semaphore.rs; frozen pin Cargo.lock:4196.
- Do not mandate a thiserror dependency or Result merely for infallible stores. Checked conversion can justify a fallible API, but signature changes require deliberate compatibility decisions.
- Saturation to u64::MAX milliseconds fixes modulo wrapping, not necessarily a suitable operational maximum backoff. Choose an operational policy before any wiring.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
