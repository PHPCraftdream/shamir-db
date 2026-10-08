<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-funclib — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Authorized evaluated scalars expose native recursion, output amplification and synchronous CPU work. Exact dependency evidence supports the narrow crypto and admission guarantees, not universal stack safety, deterministic saturation or all-deployment exploit claims.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 7 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `validate/is_json` hand-rolled parser recurses without a depth cap — stack-overflow abort from a ~20 KB query string

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Arbitrary nesting inside an evaluated string drives native mutual recursion without a counter. Transport nesting checks do not inspect that text. Availability risk is established; the title's 20-KB crash threshold and unauthenticated exposure are not.

Evidence: [crates/shamir-funclib/src/validate.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/validate.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L178); [crates/shamir-engine/src/query/filter/resolve.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L374).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Unbounded attacker-sized allocations → allocator abort in `strings/repeat`, `strings/pad_left`, `strings/pad_right`, `gen/random_bytes`

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Small authorized expressions can request uncapped results before response enforcement. Allocation failure may abort; capacity overflow can instead unwind. No exact platform crash behavior was executed.

Evidence: [crates/shamir-funclib/src/gen.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/gen.rs#L75); [crates/shamir-funclib/src/strings.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L237); [crates/shamir-funclib/src/strings.rs:406](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/strings.rs#L406); [Cargo.toml:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.toml#L88).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — `validate/matches` recompiles the user-supplied regex on every call — per-row CPU amplification; ignores the crate's own caching convention

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Repeated row evaluation invokes Regex::new repeatedly. Published regex 1.12.3 includes size protection but does not eliminate compilation work. This is qualified CPU amplification, not measured exhaustion or universal linear matching.

Evidence: [crates/shamir-funclib/src/validate.rs:342](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L342); [crates/shamir-engine/src/query/filter/resolve.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L374); [Cargo.lock:2844](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2844).

Grouping/duplicate: [performance-hotpath.md#2](performance-hotpath.md#review-2). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Attacker-triggerable `rust_decimal` overflow panics in the numeric reductions — violates the "avoid panic / return Result" rule

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Published rust_decimal 1.40.0 unchecked operators panic. Generic variance over Int(-10^15), Int(10^15) squares deviations beyond Decimal range; native MAX sums also fail. Typed engine Sum/Avg use separate paths, and no actual FFI panic crossing was established.

Evidence: [crates/shamir-funclib/src/agg.rs:286](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L286); [crates/shamir-funclib/src/agg.rs:515](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/agg.rs#L515); [crates/shamir-funclib/src/arrays.rs:276](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/arrays.rs#L276); [crates/shamir-engine/src/query/read/aggregate.rs:889](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/aggregate.rs#L889); [Cargo.lock:2979](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2979).

Grouping/duplicate: [error-handling-lifecycle.md#4](error-handling-lifecycle.md#review-4). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — Plain `i64` arithmetic on extremes: debug-panic / wrong-in-release

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

age retains an overflowing subtraction for i64::MIN. The bundled value_nav allegation is refuted: with nonnegative representable len and negative idx, len+idx stays inside i64 range and a negative result correctly misses.

Evidence: [crates/shamir-funclib/src/datetime.rs:78](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/datetime.rs#L78); [crates/shamir-funclib/src/value_nav.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/value_nav.rs#L109); [crates/shamir-funclib/src/value_nav.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/value_nav.rs#L110).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — `crypto/ct_eq` length short-circuit leaks operand length (documented; acceptable for its intended MAC-tag use)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Equal-length content comparison uses subtle 2.6.1, whose published slice contract itself short-circuits unequal lengths. Public fixed-size tag length is not a secret; no stronger required contract or sensitive-length caller was found.

Evidence: [crates/shamir-funclib/src/crypto.rs:303](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L303); [crates/shamir-funclib/src/crypto.rs:305](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L305); [Cargo.lock:4000](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4000).

<a id="review-7"></a>

### Claim 7 — `argon2id` blocking semaphore acquire runs inline on async runtime workers (documented residual risk — tracked, listed for completeness)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Expression dispatch executes the KDF and possible Condvar wait synchronously. The named wrapper offloads, but does not intercept expression calls. The cap limits admitted backing buffers; it does not establish worker liveness.

Evidence: [crates/shamir-funclib/src/crypto.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/crypto.rs#L221); [crates/shamir-engine/src/query/filter/resolve.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-wasm-host/src/builtin.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/builtin.rs#L63).

Grouping/duplicate: [concurrency-lockfree.md#3](concurrency-lockfree.md#review-3). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — `canonical.rs` key serialization swallows failure (`unwrap_or_default`) on an integrity-critical path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Already-public generic APIs accept native fallible Serialize implementations. A failed key becomes empty bytes, permitting distinct native maps to share encodings. Published rmp-serde 1.3.1 propagates such failures before funclib suppresses them; ordinary query String keys are unaffected.

Evidence: [crates/shamir-funclib/src/canonical.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/canonical.rs#L199); [crates/shamir-funclib/src/canonical.rs:213](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/canonical.rs#L213); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

<a id="review-9"></a>

### Claim 9 — Robustness nits in parsers

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Parser cursor updates preserve i&lt;=len and eat follows a successful byte probe; its slice is safe. chrono 0.4.43 guarantees midnight conversion. Defensive spelling/annotations are optional, not outstanding attacker-triggered failures.

Evidence: [crates/shamir-funclib/src/validate.rs:115](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L115); [crates/shamir-funclib/src/validate.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/datetime.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/datetime.rs#L153); [Cargo.lock:655](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L655).

## Evidence and recipe corrections

- The authorization boundary is execute_as/Authorized::authorize; builtin registration alone proves neither unauthenticated access nor a direct WASM import.
- The single-argument field-rule bridge reaches is_json but cannot directly satisfy repeat/pad/Argon2 arity.
- Argon2 reference testing uses the same argon2 dependency; it validates wrapper defaults and determinism, not an independent cryptographic implementation.
- Fixed-length ct_eq is supported; universal constant time across lengths is not.
- Parser invariant nits are not-applicable under the current contract rather than confirmed-open defects.

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
