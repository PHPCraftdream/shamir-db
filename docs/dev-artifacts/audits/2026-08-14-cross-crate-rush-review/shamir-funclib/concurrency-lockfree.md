<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-funclib — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The mutex cache, lost-notification schedule and inline KDF worker occupancy are source-established. Exact scc semantics contradict populated lock-free lookup claims; existing cap testing does not control the relevant interleavings.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 6 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Global `std::sync::Mutex` regex cache on the hot filter path — regex compiled while holding the lock

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Every hit locks one global mutex; misses compile while holding it. Inserting a 257th pattern clears the cache, causing subsequent churn. Serialization is proved, but throughput collapse and compile durations are unmeasured.

Evidence: [crates/shamir-funclib/src/strings.rs:418](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L418); [crates/shamir-funclib/src/strings.rs:423](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L423); [crates/shamir-funclib/src/strings.rs:427](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L427); [crates/shamir-funclib/src/strings.rs:429](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L429).

<a id="review-2"></a>

### Claim 2 — `CountingSemaphore` lost-wakeup: `release()` notifies without holding the mutex spanning the waiter's predicate check

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

After all holders finish, a delayed waiter can check zero, miss all releases before wait registration, then sleep with free permits and no future notifier. A controlled check-to-wait regression is required; the KDF overlap test does not force this schedule.

Evidence: [crates/shamir-funclib/src/crypto.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L139); [crates/shamir-funclib/src/crypto.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L140); [crates/shamir-funclib/src/crypto.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L145); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L207).

<a id="review-3"></a>

### Claim 3 — `argon2id` acquires a blocking semaphore inline on async runtime workers (documented residual risk, still open)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Filter and computed-write calls synchronously perform admission waits and KDF work. The named wrapper's spawn_blocking covers a different entry point. Occupied workers remain a risk even without saturation.

Evidence: [crates/shamir-funclib/src/crypto.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L221); [crates/shamir-engine/src/query/filter/resolve.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-engine/src/table/write_helpers.rs:314](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/write_helpers.rs#L314); [crates/shamir-wasm-host/src/builtin.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L63).

<a id="review-4"></a>

### Claim 4 — `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes with no locking"

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Published scc 3.8.4 get_sync calls optional_writer_sync and holds an exclusive bucket entry while cloning. read_sync uses a shared lock, so that substitution reduces exclusivity but does not fulfill lock-free wording.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L25); [crates/shamir-funclib/src/scalar_resolver.rs:46](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L46); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="review-5"></a>

### Claim 5 — The module carrying the crate's concurrency claims has no tests

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No resolver-specific suite is wired in funclib. Registered external successful-dispatch tests do not establish occupied-name replacement, builtin shadowing, identity or controlled registration/lookup behavior.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L145); [crates/shamir-funclib/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/tests/mod.rs#L1); [crates/shamir-engine/src/query/read/tests/select_projection_tests.rs:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/tests/select_projection_tests.rs#L283).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — `A2_IN_FLIGHT` observability counter is not panic-safe (permit itself is)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Unwind between manual increment/decrement leaves telemetry inflated, while permit Drop releases admission. Returned errors decrement before propagation. No presently input-triggerable KDF unwind was established.

Evidence: [crates/shamir-funclib/src/crypto.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L178); [crates/shamir-funclib/src/crypto.rs:225](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L225); [crates/shamir-funclib/src/crypto.rs:230](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L230).

Grouping/duplicate: [error-handling-lifecycle.md#7](error-handling-lifecycle.md#review-7). This is not an additional independent defect.

## Evidence and recipe corrections

- TFxMap registries are std::HashMap, not IndexMap; cardinality remains O(1).
- The sibling Argon2Semaphore has the same notifier shape but retries through wait_timeout capped at one second; do not infer funclib's indefinite missed-notification hang for that sibling.
- Acquiring the notification mutex unconditionally on release affects every release, not only contended releases.
- A start barrier does not make peak==cap deterministic or guarantee that removing admission makes peak exceed cap.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-funclib -- Concurrency & lock-free invariants

## Summary

The crate is mostly pure scalar functions and is pillar-clean where it matters structurally: registries are single-threaded `TFxMap` (Fx) built once and shared as `&'static` via `OnceLock`, the user-scalar layer is `scc::HashMap<String, FnEntry, THasher>`, the Argon2 gate's fast path is a lock-free CAS, and the scc `len()` O(N) ban is respected (`ScalarRegistry::len` is IndexMap O(1); `UserScalarLayer::is_empty` maps to scc's early-exit `has_entry`, not the banned `len()`). Three real deviations exist: a process-global `std::sync::Mutex` regex cache on the hot filter path that compiles `Regex::new` *while holding the lock* (pillar-1 violation, no contention-model comment), a lost-wakeup race in the hand-rolled condvar semaphore (notify without holding the mutex whose guard spans the waiter's predicate check), and the documented-but-unresolved blocking `acquire()` inline on async runtime workers. The `ScalarResolver`/`UserScalarLayer` concurrency claims are untested.

## Findings

### 1. Global `std::sync::Mutex` regex cache on the hot filter path — regex compiled while holding the lock
- **File:line:** `crates/shamir-funclib/src/strings.rs:417-434` (cache at 418, lock at 423, `Regex::new` at 427)
- **Severity:** high
- **Issue:** `compile()` is called by all 8 regex scalar functions (`is_reg_match`, `reg_query`, `reg_query_all`, `reg_captures`, `reg_replace`, `reg_split`, `reg_count`, `reg_find_index` — lines 262-367), which run per-row inside filter expressions. Every call takes a **process-global `std::sync::Mutex<TFxMap<String, Regex>>`** (pillars 1 and 5 violation), and on a cache miss `Regex::new(pat)` — expensive and driven by a user-supplied pattern string — executes **while the global guard is held**. There is no inline comment naming a contention model, which CLAUDE.md requires for every hot-path `std::sync::Mutex`. Secondary: the eviction policy is `if guard.len() >= 256 { guard.clear() }` (lines 429-431), so at 257 alternating patterns the whole cache is wiped and the next 256 calls all recompile — under the same global lock.
- **Failure scenario:** one connection runs `reg_query(col, '<pathologically complex pattern>')`; its compile takes milliseconds to seconds *inside the global critical section*. Every regex scalar call from every other connection/query/WASM guest stalls behind it; throughput collapses to serialized regex access, and the cache-thrash case makes recompiles a steady-state cost, not a warm-up cost.
- **Suggested fix:** replace the `OnceLock<Mutex<TFxMap>>` with `scc::HashMap<String, Regex, THasher>` (the same primitive and pattern as `UserScalarLayer` next door): `read_sync` on hit, drop the guard, then `insert_sync` the freshly compiled `Regex` — compilation outside any container lock. For call sites with literal, fixed patterns, follow `validate.rs`'s `LazyLock<Regex>` statics instead of a shared cache. Consider replacing clear-on-full with a simple epoch/FIFO eviction to stop recompile churn.

### 2. `CountingSemaphore` lost-wakeup: `release()` notifies without holding the mutex spanning the waiter's predicate check
- **File:line:** `crates/shamir-funclib/src/crypto.rs:132-148` (`acquire` 132-143, `release` 145-148), predicate `try_take` at 153-163
- **Severity:** medium
- **Issue:** The waiter holds `self.notify.0`'s mutex across `try_take` and hands it off inside `cvar.wait` — but `try_take` is lock-free on the atomic, and `release()` performs `available.fetch_add(1, Release); notify_one()` **without ever acquiring that mutex**. std `Condvar` notifications are not remembered, so the canonical interleaving loses the wakeup: permits exhausted (available=0) → waiter W takes the mutex, `try_take` fails → releaser R does `fetch_add` (available=1) then `notify_one()` (zero registered waiters → no-op) → W calls `cvar.wait` and sleeps with a permit sitting free. The mutex only helps if the notifier holds it while notifying; here it does not, so the check-then-wind-to-sleep window is unguarded.
- **Failure scenario:** under saturation a caller parks despite a free permit. Under continued traffic a later `release()` re-wakes it (transient latency spike); on a quiescent system the parked thread sleeps indefinitely — and per the crate's own documented dispatch model (crypto.rs:102-110) that thread is a tokio runtime worker, so a runtime worker is removed from the pool with no timeout and no way to observe it.
- **Suggested fix:** hold the mutex across the notify: in `release()`, take `let _g = self.notify.0.lock();` before `notify_one()` (then the fast path stays lock-free, and the lock is only touched on release-under-contention). Alternatively restructure so the predicate is checked and the notify happen under the same mutex (`wait_while`). Same audit should be applied to `shamir_connect`'s `Argon2Semaphore`, which this deliberately mirrors (out of scope here). The `argon2id_concurrency_cap_bounds_parallel_calls` test cannot catch this (it exercises the fast path + a clean queue), so a targeted stress/loom-style test is warranted with the fix.

### 3. `argon2id` acquires a blocking semaphore inline on async runtime workers (documented residual risk, still open)
- **File:line:** `crates/shamir-funclib/src/crypto.rs:102-110` (tension note), 111-112 (gate), 221 (blocking `SemaphorePermit::acquire` inside the pure-`fn` scalar body)
- **Severity:** medium
- **Issue:** The scalar contract is a synchronous `fn`, and the crate's own doc records that the engine dispatches it **inline on runtime workers** (`filter/resolve.rs`, `table/write_helpers.rs`, `validator/schema/field_rule.rs` — no `spawn_blocking`). So `acquire()` can park a worker on the condvar (pillar 2: CPU-bound work must cross to `spawn_blocking`). The cap bounds memory but cannot bound the liveness cost: at 16 permits and multiple runtimes/blocking-pool threads in play, several workers can be parked simultaneously waiting for KDF calls that must themselves be scheduled to finish and release.
- **Failure scenario:** mixed workload where blocking-pool threads hold all 16 permits while connection-query workers pile up inside `acquire()`; each parked worker is a lost runtime thread for the duration, degrading every concurrent query on the runtime, not just the Argon2 ones.
- **Suggested fix:** the code already names the correct remediation ("moving scalar dispatch onto `spawn_blocking` project-wide is a larger refactor flagged as follow-up") — track and land it; until then this stays a consciously accepted debt, but it should not silently expire. A smaller in-crate mitigation: a `try_acquire` + caller-visible `throttled` error code so query engines can defer instead of parking a worker.

### 4. `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes with no locking"
- **File:line:** `crates/shamir-funclib/src/scalar_resolver.rs:45-47` (claim at 25-26 and 111)
- **Severity:** low
- **Issue:** scc's `get_sync` returns an `OccupiedEntry`, which takes the **bucket's write lock** (scc's own docs: "use `read_sync` if read-only access is sufficient"); the lookup then clones `FnEntry` under that exclusive lock. The module doc ("Lookups are read-only hash probes with no locking", "Fast path: user layer (lock-free read...)") is factually wrong for any non-empty user layer, and per-key concurrent `ScalarResolver::call`s hashing to the same bucket serialize. The common `builtins_only()` path is unaffected: the shared empty layer has a null bucket array, so the miss exits before locking — the doc's real fast-path argument survives, but the stated mechanism ("lock-free read") does not.
- **Failure scenario:** a DB registers several user scalars; parallel filter evaluations dispatching frequently-hit user scalars contend on bucket writer locks, measurably worse than the documented zero-contention model, and inconsistent with the pillar-1 story the module advertises.
- **Suggested fix:** one-line change: `self.fns.read_sync(name, |_, v| v.clone())`. The `FnEntry` clone is cheap (one `Arc` bump + POD fields), so no other change is needed.

### 5. The module carrying the crate's concurrency claims has no tests
- **File:line:** `crates/shamir-funclib/src/scalar_resolver.rs` (no `tests/` directory; no references anywhere else in the crate's test tree)
- **Severity:** low
- **Issue:** `UserScalarLayer`/`ScalarResolver` — the only scc-backed hot-path structure in the crate, with explicit concurrency claims (empty-layer fast path, user-shadows-builtin priority, lock-free registration) — has zero dedicated tests; nothing exercises concurrent `register`/`call` or the two-layer fallback. By contrast the Argon2 cap is well covered (`crypto/tests/crypto_tests.rs:206-281`, barrier-hardened per audit G4/#528 — good).
- **Failure scenario:** a refactor of the resolver (e.g. the finding-4 fix) silently breaks shadowing priority or the empty-layer fast path with no red test.
- **Suggested fix:** add `src/scalar_resolver/tests/` per the test-organisation convention: shadowing, unknown-function, arity parity with `ScalarRegistry::call`, `builtins_only()` sharing, and a multi-threaded register/call smoke test.

### 6. `A2_IN_FLIGHT` observability counter is not panic-safe (permit itself is)
- **File:line:** `crates/shamir-funclib/src/crypto.rs:225-230`
- **Severity:** nit
- **Issue:** `A2_IN_FLIGHT.fetch_add` / `fetch_sub` bracket `hash_password_into` manually; a panic in between permanently inflates the in-flight counter. The semaphore permit is correctly RAII (`SemaphorePermit` drops on unwind), so no permit leaks — this only skews the observability pair used by the cap test (which resets the counters, masking drift).
- **Failure scenario:** none production-visible today (pure observability).
- **Suggested fix:** a tiny RAII guard for the counter pair, or move both counters into `SemaphorePermit`'s acquire/drop. (The `fetch_max(prev + 1)` peak logic itself is sound: each `fetch_add` returns a unique prior value, so the peak always reflects a real instantaneous in-flight count.)

## Verified non-issues (for the record)
- `ScalarRegistry`/`AggRegistry` use `TFxMap` (IndexMap + Fx); their `len()`/`is_empty()` are O(1) IndexMap calls, used only in tests — the scc `len()` ban does not apply, and no `#[allow]`-worthy O(N) call exists.
- `UserScalarLayer::is_empty` maps to scc's `!has_entry` (early-exit bucket scan, immediate `true` on the shared empty layer) — not the banned O(N) `scc::*::len()`; no `AtomicUsize` mirror needed at current usage.
- No lock is held across `.await` anywhere (the crate contains no `async fn`/`.await`); no `parking_lot`, no `RwLock`, no `unsafe`, no `static mut`.
- Fx pillar holds: `scc::HashMap::with_hasher(THasher::default())` (scalar_resolver.rs:35), `TFxMap` registries, `new_fx_set_wc`/`new_map_wc` in arrays/object; `rand::rng()` (thread-local) in `gen.rs` is thread-safe and correctly registered as impure/non-indexable.

</details>
