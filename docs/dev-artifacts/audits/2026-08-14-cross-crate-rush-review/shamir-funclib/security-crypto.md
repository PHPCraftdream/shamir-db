<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-funclib — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Authenticated scalar-expression availability risks remain in recursion, allocations, regex compilation, decimal reductions, and inline KDF execution. The value_nav overflow claim is false; length-dependent ct_eq is intentional.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 8 | 0 | 0 | 0 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `validate/is_json` hand-rolled parser recurses without a depth cap — stack-overflow abort from a ~20 KB query string

Status: `confirmed-open`. Current risk: `high`.

TextFormatParser still recursively descends arrays/objects without a depth counter. Its input is a string, so the transport's structural nesting limit does not bound this inner parser. A sufficiently deep evaluated input exhausts native stack; the quoted 20 KB threshold is unverified. Reachability requires an authorized operation that evaluates the scalar.

Evidence: [crates/shamir-funclib/src/validate.rs:87](../../../../../crates/shamir-funclib/src/validate.rs#L87); [crates/shamir-funclib/src/validate.rs:123](../../../../../crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/validate.rs:169](../../../../../crates/shamir-funclib/src/validate.rs#L169); [crates/shamir-funclib/src/validate.rs:351](../../../../../crates/shamir-funclib/src/validate.rs#L351); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Unbounded attacker-sized allocations → allocator abort in `strings/repeat`, `strings/pad_left`, `strings/pad_right`, `gen/random_bytes`

Status: `confirmed-open`. Current risk: `high`.

There are still no pre-allocation caps on these lengths. Query-controlled output amplification is independent of input message size. Allocation failure can abort; capacity overflow is generally an unwindable panic under this release profile and must not be described as automatically equivalent to abort.

Evidence: [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75); [crates/shamir-funclib/src/strings.rs:237](../../../../../crates/shamir-funclib/src/strings.rs#L237); [crates/shamir-funclib/src/strings.rs:406](../../../../../crates/shamir-funclib/src/strings.rs#L406); [Cargo.toml:88](../../../../../Cargo.toml#L88).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `validate/matches` recompiles the user-supplied regex on every call — per-row CPU amplification; ignores the crate's own caching convention

Status: `confirmed-open`. Current risk: `medium`.

Regex::new remains inside the per-call closure with no reuse. Repeated row evaluations rebuild the same pattern. This proves repeated compilation work, not a measured exhaustion threshold. Pinned regex also has compiled-size limits; its matching guarantees are not a universal O(haystack length) statement for all iterator APIs.

Evidence: [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342); [crates/shamir-funclib/src/strings.rs:417](../../../../../crates/shamir-funclib/src/strings.rs#L417); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [Cargo.lock:2844](../../../../../Cargo.lock#L2844).

Grouping/duplicate: `performance-hotpath.md#2`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Attacker-triggerable `rust_decimal` overflow panics in the numeric reductions — violates the "avoid panic / return Result" rule

Status: `confirmed-open`. Current risk: `medium`.

Sum/avg, variance arithmetic, range subtraction, and arrays reductions still use panicking operators. Pinned rust_decimal 1.40.0 confirms panic-on-overflow. Reachability includes native Decimal inputs and convertible query numeric values through array/generic aggregate routes; typed engine Sum/Avg are separate implementations. Variance depends on deviations/spread, not merely absolute magnitude.

Evidence: [crates/shamir-funclib/src/agg.rs:286](../../../../../crates/shamir-funclib/src/agg.rs#L286); [crates/shamir-funclib/src/agg.rs:322](../../../../../crates/shamir-funclib/src/agg.rs#L322); [crates/shamir-funclib/src/agg.rs:514](../../../../../crates/shamir-funclib/src/agg.rs#L514); [crates/shamir-funclib/src/agg.rs:852](../../../../../crates/shamir-funclib/src/agg.rs#L852); [crates/shamir-funclib/src/arrays.rs:276](../../../../../crates/shamir-funclib/src/arrays.rs#L276); [Cargo.lock:2979](../../../../../Cargo.lock#L2979); [crates/shamir-engine/src/query/read/aggregate.rs:889](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L889).

Grouping/duplicate: `error-handling-lifecycle.md#4`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — Plain `i64` arithmetic on extremes: debug-panic / wrong-in-release

Status: `confirmed-open`. Current risk: `medium`.

The age half remains open. The value_nav half is refuted: for a representable nonnegative list length and negative idx, len + idx lies within i64 range, including idx == i64::MIN, and correctly yields a miss. There is no overflow, wrapping, or release mis-navigation in the cited expression.

Evidence: [crates/shamir-funclib/src/datetime.rs:78](../../../../../crates/shamir-funclib/src/datetime.rs#L78); [crates/shamir-funclib/src/value_nav.rs:109](../../../../../crates/shamir-funclib/src/value_nav.rs#L109); [crates/shamir-funclib/src/value_nav.rs:111](../../../../../crates/shamir-funclib/src/value_nav.rs#L111).

Grouping/duplicate: `error-handling-lifecycle.md#5`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `crypto/ct_eq` length short-circuit leaks operand length (documented; acceptable for its intended MAC-tag use)

Status: `not-applicable`. Current risk: —.

The equal-length content comparison still uses subtle and length mismatch still returns immediately. Length is public for fixed-size MAC tags, so this is not a defect under the stated use. Variable-length, length-sensitive secrets would require a different contract; no such caller was established.

Evidence: [crates/shamir-funclib/src/crypto.rs:303](../../../../../crates/shamir-funclib/src/crypto.rs#L303); [crates/shamir-funclib/src/crypto.rs:305](../../../../../crates/shamir-funclib/src/crypto.rs#L305); [Cargo.lock:4000](../../../../../Cargo.lock#L4000).

<a id="review-7"></a>

### Claim 7 — `argon2id` blocking semaphore acquire runs inline on async runtime workers (documented residual risk — tracked, listed for completeness)

Status: `confirmed-open`. Current risk: `medium`.

Scalar query dispatch remains inline despite the separate offloaded named-function wrapper. Per-call dimensions and 16-permit admission still bound KDF concurrency. The statement 'no deadlock' must exclude the independently confirmed lost-wakeup hang.

Evidence: [crates/shamir-funclib/src/crypto.rs:102](../../../../../crates/shamir-funclib/src/crypto.rs#L102); [crates/shamir-funclib/src/crypto.rs:221](../../../../../crates/shamir-funclib/src/crypto.rs#L221); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-wasm-host/src/builtin.rs:63](../../../../../crates/shamir-wasm-host/src/builtin.rs#L63).

Grouping/duplicate: `concurrency-lockfree.md#3`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — `canonical.rs` key serialization swallows failure (`unwrap_or_default`) on an integrity-critical path

Status: `confirmed-open`. Current risk: `low`.

Error suppression remains. Ordinary query String keys do not exercise a fallible custom Serialize implementation, but the public generic canonical APIs already permit native custom keys that return serialization errors; this need not await a future code change. The asserted universal upstream nesting protection is also unsupported for directly constructed native Values.

Evidence: [crates/shamir-funclib/src/canonical.rs:187](../../../../../crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:199](../../../../../crates/shamir-funclib/src/canonical.rs#L199); [crates/shamir-funclib/src/canonical.rs:213](../../../../../crates/shamir-funclib/src/canonical.rs#L213); [crates/shamir-types/src/types/value.rs:33](../../../../../crates/shamir-types/src/types/value.rs#L33).

<a id="review-9"></a>

### Claim 9 — Robustness nits in parsers

Status: `confirmed-open`. Current risk: `nit`.

Both cited annotation/hardening opportunities remain. TextFormatParser maintains i <= input length, and eat is called only after a successful byte probe, so its current slice is safe. Pinned chrono proves midnight conversion infallible. Neither cited site establishes a current attacker-triggerable panic.

Evidence: [crates/shamir-funclib/src/validate.rs:115](../../../../../crates/shamir-funclib/src/validate.rs#L115); [crates/shamir-funclib/src/validate.rs:123](../../../../../crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/datetime.rs:153](../../../../../crates/shamir-funclib/src/datetime.rs#L153); [Cargo.lock:655](../../../../../Cargo.lock#L655).

## Corrections and qualified non-findings

- No separate Fix Plan exists; all Suggested fixes are assessed in their finding rows and SUMMARY plan.
- Downgrade universal/unauthenticated exploit wording to authorized scalar-expression availability exposure; precise stack byte/frame thresholds remain unverified.
- Registration into builtin_scalars is not itself proof of a direct WASM import for every scalar. The named-function registry preloads Argon2 only; other scalar exposure through guest-issued database queries depends on gateway authorization.
- The scalar field-rule bridge passes exactly one argument, so it can directly reach is_json but not repeat/pad or password-and-salt Argon2 calls. Query/computed-write paths establish those separate routes.
- SHA/HMAC/BLAKE3 KAT literals and error tests are registered; the Argon2 reference test uses the same Argon2 dependency, not an independent cryptographic implementation.
- Zero crate unsafe, rand 0.9.4 OsRng-seeded ThreadRng use, impure generation metadata, parameter ceilings, and RAII semaphore release are source-supported. The cap test's claimed deterministic saturation is not.
- encode/parse_json retains serde_json 1.0.149's default recursion limit. That does not establish a bound for the bespoke validator or arbitrary native Value constructors.
- Do not infer an actual WASM/FFI panic crossing or deployment-wide process abort from the decimal reduction findings; release unwinding ordinarily isolates such panics.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-funclib -- Security & crypto boundary

## Summary

This crate has no auth/SCRAM/TLS surface of its own — it is the crypto-*primitive* boundary (`sha256/sha512/sha3_256/blake3/hmac_sha256/ct_eq/argon2id`, plus the `crypto/canonical_hash` CAS integrity hash), and that crypto core is in good shape: no `unsafe` anywhere in the crate, `subtle`-based constant-time comparison, RFC 4231 / empty-string known-answer tests, an argon2id reference-comparison test, capped KDF params and a process-wide concurrency semaphore with a barrier-based regression test. The security findings are concentrated in the untrusted-input handling of the *sibling* scalar families that share the same query/guest-reachable dispatch path: two attacker-triggerable **process-abort** classes (unbounded recursion in `validate/is_json`; unbounded allocations in `strings/repeat`, `strings/pad_*`, `gen/random_bytes`) that bypass the workspace's deliberate `panic = "unwind"` isolation, one per-call regex-compilation CPU-amplification gap, and a family of `rust_decimal` overflow panics that violate the documented error-handling rule.

## Findings

### 1. `validate/is_json` hand-rolled parser recurses without a depth cap — stack-overflow abort from a ~20 KB query string
- **File:line:** `crates/shamir-funclib/src/validate.rs:123-191` (`TextFormatParser::value` → `object`/`array` → `value` mutual recursion), entry at `validate.rs:87` (`is_text_encoded_str`), registered at `validate.rs:350-356`.
- **Severity:** high
- **Issue:** The bespoke RFC 8259 validator recurses once per nesting level with no depth limit. `serde_json::from_str` (used by `encode/parse_json`) enforces a 128-level recursion limit; this parser was written to avoid the dependency and silently dropped that protection. Every frame is on the query-evaluation stack (2 MB default tokio worker), so roughly 5–10 k nesting levels exhaust it.
- **Failure scenario:** Any low-privileged query/filter/validator evaluating `validate/is_json('[[[[……]]]]')` with ~10–20 KB of `[` aborts the process with a stack overflow ("thread … has overflowed its stack"). A Rust stack overflow is **not** a catchable panic — the `panic = "unwind"` isolation the server architecture depends on (`Cargo.toml:66-88`) does not apply; every open connection dies.
- **Suggested fix:** Thread a depth counter through `TextFormatParser` (cap it at 128 to match `serde_json`'s default, or a named `const MAX_JSON_DEPTH`), returning `false` (invalid) when exceeded; add a regression test with 100 k nesting levels.

### 2. Unbounded attacker-sized allocations → allocator abort in `strings/repeat`, `strings/pad_left`, `strings/pad_right`, `gen/random_bytes`
- **File:line:** `crates/shamir-funclib/src/strings.rs:237` (`s.repeat(n as usize)` with `n` up to `i64::MAX`); `strings.rs:406` (`std::iter::repeat_n(ch, target - cur)` in `pad`, same unbounded `len`); `crates/shamir-funclib/src/gen.rs:75` (`vec![0u8; n as usize]`).
- **Severity:** high
- **Issue:** All three accept an attacker-chosen length with only a `>= 0` check and allocate it in one shot. Contrast `crypto/argon2id`, which caps every dimension (`A2_MAX_LENGTH = 256`, `A2_MAX_MEMORY_KB`) precisely because these scalars are query/guest-reachable — that cap discipline was not applied here.
- **Failure scenario:** `SELECT gen/random_bytes(1099511627776)` (1 TiB) or `strings/pad_left(x, 549755813888, '0')` → allocation far beyond available RAM → `handle_alloc_error` → **abort**. Like the stack overflow in finding 1, an abort bypasses the unwind-based per-connection isolation and kills the whole server. Sizes between `isize` capacity and RAM also yield `capacity overflow` panics per call (repeated → sustained resource burn even where the unwind boundary holds).
- **Suggested fix:** Impose per-call output caps (e.g. 64 MiB, mirroring `A2_MAX_MEMORY_KB`'s rationale) and return `ScalarError("out_of_range")` above the cap; a one-line `const` + range check per function.

### 3. `validate/matches` recompiles the user-supplied regex on every call — per-row CPU amplification; ignores the crate's own caching convention
- **File:line:** `crates/shamir-funclib/src/validate.rs:342` (`Regex::new(pat)` inside the closure) vs. `strings.rs:417-434`, which already implements the fix (`compile()` with a poison-tolerant `Mutex` cache bounded at 256 entries).
- **Severity:** medium
- **Issue:** `matches` is evaluated per row in filters/validators, so each row pays full NFA construction for the pattern. The module doc itself establishes the convention ("compiled once per call site via `LazyLock`" for the fixed patterns, `validate.rs:15-16`), and the matching regex family in `strings` caches by pattern — `matches` is the lone exception. Matching itself is ReDoS-safe (linear-time `regex` crate); the cost is the *compilation*, plus unbounded pattern-size parse work per call.
- **Failure scenario:** `WHERE validate/matches(col, '<multi-KB pattern>')` over a 1 M-row scan → 1 M regex compilations → sustained CPU exhaustion by a single cheap query, with zero cache reuse even for an identical constant pattern.
- **Suggested fix:** Promote `strings::compile` to a shared helper (or an `fn`-local `LazyLock<Mutex<TFxMap<String, Regex>>>` duplicate) and route `matches` through it, keeping the `bad_pattern` error code.

### 4. Attacker-triggerable `rust_decimal` overflow panics in the numeric reductions — violates the "avoid panic / return Result" rule
- **File:line:** `crates/shamir-funclib/src/agg.rs:286` (`SumAgg::accumulate`, `self.acc += to_dec(v)?`); `agg.rs:322` (`AvgAgg::accumulate`); `agg.rs:512-516` (`compute_variance` — `(*x - mean) * (*x - mean)` overflows `Decimal` for values roughly ≥ 1e15, i.e. `stddev`/`variance`); `agg.rs:852` (`RangeAgg::finalize`, `hi - lo`); `crates/shamir-funclib/src/arrays.rs:276-279` (`arrays/sum`, `arrays/avg`, `acc += …` / `acc /= …`).
- **Severity:** medium
- **Issue:** `rust_decimal`'s `std::ops` impls panic on overflow (verified upstream: `src/arithmetic_impls.rs` — `panic!("Addition overflowed")` etc.), and `Decimal::MAX` is only ~7.92e28, so two large stored values suffice. CLAUDE.md's error-handling section mandates `Result<T, E>` and forbids panics outside programmer-bug invariants; these are data-driven panics on untrusted values, one per accumulate call.
- **Failure scenario:** `SELECT agg/sum(big_col)` where rows hold values near `Decimal::MAX` → "Addition overflowed" panic inside the aggregate. The workspace's `panic = "unwind"` + per-request `JoinSet` boundary converts it to a dropped request (and it defeats `checked_*`-free code paths if any profile ever flips back to `abort`, or if the panic crosses a WASM/FFI boundary in the host). `stddev`/`variance` overflow even with modest (≈1e15) inputs because of the squaring step — very easy to hit with ordinary financial-curve data.
- **Suggested fix:** Switch these sites to `checked_add` / `checked_mul` / `checked_sub` and map `None` → `ScalarError("overflow")` (new stable code, consistent with the machine-codes-only convention).

### 5. Plain `i64` arithmetic on extremes: debug-panic / wrong-in-release
- **File:line:** `crates/shamir-funclib/src/datetime.rs:78` (`age`: `Utc::now().timestamp_millis() - then` with `then = i64::MIN`); `crates/shamir-funclib/src/value_nav.rs:110` (`navigate`: `len + idx` for negative index `idx = i64::MIN`).
- **Severity:** low
- **Issue:** Untreated `i64` subtraction/addition on attacker-chosen extremes. In release (overflow-checks off) `age` wraps silently to a nonsense value and `value_nav` wraps to a negative that happens to be caught by the `resolved < 0` miss-check (no crash, wrong-but-harmless); in debug/test builds (overflow-checks on) both panic.
- **Failure scenario:** `SELECT datetime/age(-9223372036854775808)` panics under the test gate (flaky red builds) and returns a garbage number in production.
- **Suggested fix:** `checked_sub` / `wrapping_add` + explicit `resolved < 0` handling (`len.checked_add(idx)` → miss on `None`).

### 6. `crypto/ct_eq` length short-circuit leaks operand length (documented; acceptable for its intended MAC-tag use)
- **File:line:** `crates/shamir-funclib/src/crypto.rs:305` (`lhs.len() == rhs.len() && bool::from(lhs.ct_eq(rhs))`).
- **Severity:** low
- **Issue:** The `&&` short-circuit returns before `subtle::ConstantTimeEq` runs when lengths differ, making the comparison constant-time in *content* but timing-visible in *length*. The code comment acknowledges this; for the intended use (comparing fixed-length HMAC tags, per the module's `hmac_sha256` pairing) tag length is public and the leak is harmless. Flagged for the record so a future caller doesn't use `ct_eq` on variable-length secrets whose length is itself sensitive. Everything else about the comparison path is correct (`Bin`-only via `arg_bytes`, genuine `subtle` CT compare).
- **Failure scenario:** None today. Hypothetical: `ct_eq(provided_token, stored_token)` with variable-length bearer tokens lets a local timing observer learn length equality.
- **Suggested fix:** None required; optionally document "length leaks by design — pad secrets to fixed length before comparing" on the function doc.

### 7. `argon2id` blocking semaphore acquire runs inline on async runtime workers (documented residual risk — tracked, listed for completeness)
- **File:line:** `crates/shamir-funclib/src/crypto.rs:102-110` (module-level "Inlining tension" note), `crypto.rs:221` (`SemaphorePermit::acquire` blocking on the condvar), called inline from `argon2id_fn`.
- **Severity:** low
- **Issue:** Scalar dispatch is inline on runtime workers (no `spawn_blocking` upstream), so up to `n_workers` tasks can park in `acquire()` stalling the reactor, and each admitted KDF (tens of ms) occupies a worker. The aggregate-memory DoS the cap addresses is real and fixed; this is the residual availability cost, explicitly documented in the module with the project-wide `spawn_blocking` refactor flagged as follow-up. The cap itself (16 permits, ≤ 64 MiB/call ⇒ ≤ 1 GiB worst case) and its barrier-saturated regression test (`crypto/tests/crypto_tests.rs:207`) are sound; the RAII permit releases on panic; the observability atomics are benign.
- **Failure scenario:** Many concurrent `argon2id()` calls → runtime workers queue on the condvar → event-loop latency spike for all connections until permits free. No deadlock (permit holders always make progress).
- **Suggested fix:** The module's own stated follow-up — route scalar dispatch (or at least `FnEntry`s flagged as CPU-bound) through `spawn_blocking` project-wide.

### 8. `canonical.rs` key serialization swallows failure (`unwrap_or_default`) on an integrity-critical path
- **File:line:** `crates/shamir-funclib/src/canonical.rs:187` (`rmp_serde::to_vec(key).unwrap_or_default()`), used by `encode` (the CAS `canonical_hash`, registered under `/crypto`).
- **Severity:** low
- **Issue:** If a key's `Serialize` impl ever failed, its encoding silently becomes the empty byte string — two distinct keys would then collide in the sorted entry list, producing an identical `canonical_hash` for different records, silently weakening the optimistic-CAS integrity protocol (hash equality currently *is* the change detector). Unreachable today for the actual key types (`String` and interned `u64` — msgpack encoding of either cannot fail), so this is a hardening item, not a live bug. Relatedly, `encode`/`compare` recurse over value nesting with no depth cap; they are safe only because the upstream codec/parser bounds nesting (`serde_json` at 128) — worth a comment pinning that assumption to the boundary that provides it.
- **Failure scenario:** Hypothetical only: a future key type with a fallible `Serialize` silently collapses distinct fields under the CAS hash.
- **Suggested fix:** Replace `unwrap_or_default()` with an explicit `ScalarError` (or `debug_assert!` + documented invariant comment); add a one-line comment naming the upstream depth bound.

### 9. Robustness nits in parsers
- **File:line:** `crates/shamir-funclib/src/validate.rs:115` (`self.b[self.i..]` slicing in `TextFormatParser::eat` relies on the call-site invariant `i < len` — true today since `eat` is only called after `get(self.i)` returns `Some`, but one new call-site away from a panic; use `self.b.get(self.i..).is_some_and(|s| s.starts_with(lit))`); `crates/shamir-funclib/src/datetime.rs:153` (`and_hms_opt(0, 0, 0).unwrap()` — always `Some`, but an invariant-unwrap in an untrusted-input parser).
- **Severity:** nit
- **Issue / Suggested fix:** As described — make the invariant local (`get`-based) or replace with an `expect` naming the invariant.

## Positive observations (no action)

- **Zero `unsafe`** in the entire crate (grep across `src/`, tests included: no matches).
- Crypto core tested against published vectors: SHA-256/512, SHA3-256, BLAKE3 empty-string KATs, HMAC-SHA256 RFC 4231 case 2 (`crypto_tests.rs:96-122`), argon2id bit-identity vs. an independent `Argon2` reference plus determinism, cap/arity/out-of-range errors.
- The argon2id aggregate-cap design (capped params + process-wide counting semaphore + RAII permit + peak-in-flight regression test) matches the audit trail it cites (§2b).
- `gen/uuid_v4`, `gen/random_bytes` use `rand::rng()` (OsRng-seeded thread CSPRNG) — appropriate randomness source; `gen` fns correctly registered `pure:false, deterministic:false`, so they can never back a functional index.
- Error handling elsewhere is disciplined: every extractor/coercion path returns machine-coded `ScalarError`s; `encode/parse_json` inherits `serde_json`'s 128-depth recursion limit; `datetime` pre-validates strftime patterns to avoid chrono's panicking `DelayedFormat`.

</details>
