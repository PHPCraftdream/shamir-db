<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-funclib — SUMMARY revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

All 64 consolidated numbered rows and all 21 Fix Plan entries were revalidated. The core availability/correctness mechanisms remain open, with important refutations and narrowed guarantees; the old 47-defect census is not a current verified-open count.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 64 | 60 | 0 | 0 | 2 | 0 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1-1"></a>

### Claim 1.1 — Query-reachable unbounded allocations/panics in `random_bytes`/`repeat`/`pad`

Status: `confirmed-open`. Current risk: `high`.

Positive lengths remain uncapped before allocating; checked byte ceilings and boundary tests are absent.

Evidence: [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75); [crates/shamir-funclib/src/strings.rs:237](../../../../../crates/shamir-funclib/src/strings.rs#L237); [crates/shamir-funclib/src/strings.rs:406](../../../../../crates/shamir-funclib/src/strings.rs#L406).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — `datetime/parse` date-only fallback: invariant-`unwrap` on a user-parsed date

Status: `confirmed-open`. Current risk: `nit`.

Only the invariant annotation remains open. Pinned chrono constructs midnight for every valid NaiveDate; the -262144 panic allegation is refuted. The synthesis's since-0.4.32 historical attribution was not independently verified and is unnecessary to this proof.

Evidence: [crates/shamir-funclib/src/datetime.rs:153](../../../../../crates/shamir-funclib/src/datetime.rs#L153); [Cargo.lock:655](../../../../../Cargo.lock#L655).

<a id="review-1-3"></a>

### Claim 1.3 — F64→i64 range check is off by one ulp: silently saturates instead of `out_of_range`

Status: `confirmed-open`. Current risk: `low`.

Both inclusive float-rounded upper bounds still admit 2^63; no F64 boundary regression is present.

Evidence: [crates/shamir-funclib/src/registry.rs:218](../../../../../crates/shamir-funclib/src/registry.rs#L218); [crates/shamir-funclib/src/cast.rs:121](../../../../../crates/shamir-funclib/src/cast.rs#L121).

Grouping/duplicate: `correctness-tdd.md#3`. This row is not another independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — `datetime/age`: unchecked subtraction, inconsistent with sibling `diff_secs`

Status: `confirmed-open`. Current risk: `medium`.

Unchecked subtraction and truncating division remain, unlike checked/floored diff_secs.

Evidence: [crates/shamir-funclib/src/datetime.rs:78](../../../../../crates/shamir-funclib/src/datetime.rs#L78); [crates/shamir-funclib/src/datetime.rs:264](../../../../../crates/shamir-funclib/src/datetime.rs#L264).

Grouping/duplicate: `SUMMARY.md#6.5`. This row is not another independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — Two coexisting equality semantics: `==` (variant-strict) vs `compare` (value-strict)

Status: `confirmed-open`. Current risk: `medium`.

The cross-type policy split remains; equal signed zeros also receive different Hash bit inputs, a concrete Hash/Eq violation beyond policy ambiguity.

Evidence: [crates/shamir-funclib/src/arrays.rs:85](../../../../../crates/shamir-funclib/src/arrays.rs#L85); [crates/shamir-funclib/src/null.rs:70](../../../../../crates/shamir-funclib/src/null.rs#L70); [crates/shamir-types/src/types/value.rs:293](../../../../../crates/shamir-types/src/types/value.rs#L293); [crates/shamir-types/src/types/value.rs:707](../../../../../crates/shamir-types/src/types/value.rs#L707).

Grouping/duplicate: `correctness-tdd.md#5`. This row is not another independent defect.

<a id="review-1-6"></a>

### Claim 1.6 — `compare`'s documented "total (transitive)" order is not guaranteed on the lossy fallback paths

Status: `confirmed-open`. Current risk: `medium`.

Int(2^53) and Int(2^53+1) order exactly but both equal F64(2^53) through fallback. Exact Dec/Big alone or documentation softening does not repair the ordering contract.

Evidence: [crates/shamir-funclib/src/compare.rs:115](../../../../../crates/shamir-funclib/src/compare.rs#L115); [crates/shamir-funclib/src/compare.rs:132](../../../../../crates/shamir-funclib/src/compare.rs#L132).

Grouping/duplicate: `correctness-tdd.md#6`. This row is not another independent defect.

<a id="review-1-7"></a>

### Claim 1.7 — `scalar_resolver.rs` has zero in-crate tests

Status: `confirmed-open`. Current risk: `low`.

Dedicated resolver suite and wiring remain absent.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:145](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L145); [crates/shamir-funclib/src/tests/mod.rs:1](../../../../../crates/shamir-funclib/src/tests/mod.rs#L1).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — `strings/substring` doc comment contradicts implemented (and tested) behavior

Status: `confirmed-open`. Current risk: `nit`.

The comment still promises an error; the registered past-end test still expects empty string.

Evidence: [crates/shamir-funclib/src/strings.rs:130](../../../../../crates/shamir-funclib/src/strings.rs#L130); [crates/shamir-funclib/src/tests/strings_tests.rs:73](../../../../../crates/shamir-funclib/src/tests/strings_tests.rs#L73).

Grouping/duplicate: `correctness-tdd.md#8`. This row is not another independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — Residual correctness nits (bundle)

Status: `confirmed-open`. Current risk: `nit`.

Both residual bullets remain: dead BoolAnd/BoolOr any fields and missing type_of assertions for F64/Dec/Big/Bin/Set. The five redirected bullets were also checked under their primary rows.

Evidence: [crates/shamir-funclib/src/agg.rs:724](../../../../../crates/shamir-funclib/src/agg.rs#L724); [crates/shamir-funclib/src/agg.rs:760](../../../../../crates/shamir-funclib/src/agg.rs#L760); [crates/shamir-funclib/src/tests/value_nav_tests.rs:143](../../../../../crates/shamir-funclib/src/tests/value_nav_tests.rs#L143).

<a id="review-2-1"></a>

### Claim 2.1 — Global `std::sync::Mutex` regex cache on the hot filter path — regex compiled *while holding the lock*

Status: `confirmed-open`. Current risk: `medium`.

Global locking, compile-under-guard, and clear-all-at-256 eviction remain. Throughput magnitudes are unmeasured.

Evidence: [crates/shamir-funclib/src/strings.rs:418](../../../../../crates/shamir-funclib/src/strings.rs#L418); [crates/shamir-funclib/src/strings.rs:427](../../../../../crates/shamir-funclib/src/strings.rs#L427); [crates/shamir-funclib/src/strings.rs:429](../../../../../crates/shamir-funclib/src/strings.rs#L429).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-2-2"></a>

### Claim 2.2 — `CountingSemaphore` lost wakeup: `release()` notifies without holding the mutex spanning the waiter's predicate check

Status: `confirmed-open`. Current risk: `high`.

The predicate-check-to-wait window is still not synchronized with release notification, allowing indefinite sleep despite a free permit.

Evidence: [crates/shamir-funclib/src/crypto.rs:140](../../../../../crates/shamir-funclib/src/crypto.rs#L140); [crates/shamir-funclib/src/crypto.rs:145](../../../../../crates/shamir-funclib/src/crypto.rs#L145).

Grouping/duplicate: `concurrency-lockfree.md#2`. This row is not another independent defect.

<a id="review-2-3"></a>

### Claim 2.3 — `argon2id` acquires a blocking semaphore inline on async runtime workers (documented residual risk, still open)

Status: `confirmed-open`. Current risk: `medium`.

Query/computed-write scalar paths remain synchronous; the named async Argon2 wrapper's offload does not cover them.

Evidence: [crates/shamir-funclib/src/crypto.rs:221](../../../../../crates/shamir-funclib/src/crypto.rs#L221); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-engine/src/table/write_helpers.rs:314](../../../../../crates/shamir-engine/src/table/write_helpers.rs#L314); [crates/shamir-wasm-host/src/builtin.rs:63](../../../../../crates/shamir-wasm-host/src/builtin.rs#L63).

Grouping/duplicate: `concurrency-lockfree.md#3`. This row is not another independent defect.

<a id="review-2-4"></a>

### Claim 2.4 — `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes with no locking"

Status: `confirmed-open`. Current risk: `low`.

Exclusive get_sync remains. Pinned scc read_sync is shared-locked, not lock-free.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:46](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L46); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

Grouping/duplicate: `concurrency-lockfree.md#4`. This row is not another independent defect.

<a id="review-2-5"></a>

### Claim 2.5 — The module carrying the concurrency claims has no tests

Status: `confirmed-open`. Current risk: `low`.

No dedicated concurrent resolver tests or contract suite are registered.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:145](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L145); [crates/shamir-funclib/src/tests/mod.rs:1](../../../../../crates/shamir-funclib/src/tests/mod.rs#L1).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-2-6"></a>

### Claim 2.6 — `A2_IN_FLIGHT` observability counter is not panic-safe

Status: `confirmed-open`. Current risk: `nit`.

Manual add/sub still leaks on unwind during hashing, not on ordinary returned errors; permit Drop remains safe.

Evidence: [crates/shamir-funclib/src/crypto.rs:225](../../../../../crates/shamir-funclib/src/crypto.rs#L225); [crates/shamir-funclib/src/crypto.rs:230](../../../../../crates/shamir-funclib/src/crypto.rs#L230).

Grouping/duplicate: `SUMMARY.md#6.7`. This row is not another independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — `validate/is_json` recurses without a depth cap

Status: `confirmed-open`. Current risk: `high`.

Unbounded parser recursion remains; exact stack-overflow size and unauthenticated exposure are unverified.

Evidence: [crates/shamir-funclib/src/validate.rs:123](../../../../../crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/validate.rs:178](../../../../../crates/shamir-funclib/src/validate.rs#L178).

Grouping/duplicate: `SUMMARY.md#6.1`. This row is not another independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Unbounded attacker-sized allocations → allocator abort

Status: `confirmed-open`. Current risk: `high`.

All cited output-amplification paths remain uncapped before allocation.

Evidence: [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75); [crates/shamir-funclib/src/strings.rs:237](../../../../../crates/shamir-funclib/src/strings.rs#L237); [crates/shamir-funclib/src/strings.rs:406](../../../../../crates/shamir-funclib/src/strings.rs#L406).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — `validate/matches` recompiles the user-supplied regex on every call

Status: `confirmed-open`. Current risk: `medium`.

Regex::new still runs per scalar invocation; no shared successful-pattern cache is used.

Evidence: [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342).

Grouping/duplicate: `SUMMARY.md#4.2`. This row is not another independent defect.

<a id="review-3-4"></a>

### Claim 3.4 — Attacker-triggerable `rust_decimal` overflow panics in the numeric reductions

Status: `confirmed-open`. Current risk: `medium`.

Panicking Decimal arithmetic remains in the cited reductions; native/generic/array routes must be distinguished from engine fast Sum/Avg.

Evidence: [crates/shamir-funclib/src/agg.rs:286](../../../../../crates/shamir-funclib/src/agg.rs#L286); [crates/shamir-funclib/src/agg.rs:514](../../../../../crates/shamir-funclib/src/agg.rs#L514); [crates/shamir-funclib/src/arrays.rs:276](../../../../../crates/shamir-funclib/src/arrays.rs#L276); [Cargo.lock:2979](../../../../../Cargo.lock#L2979).

Grouping/duplicate: `SUMMARY.md#6.4`. This row is not another independent defect.

<a id="review-3-5"></a>

### Claim 3.5 — `value_nav`: plain `i64` addition on extremes (the sibling of 6.5's `age` defect)

Status: `refuted`. Current risk: —.

For nonnegative representable len and negative idx, len + idx cannot overflow i64. i64::MIN remains negative after adding any feasible list length and correctly returns a miss; no debug panic or release mis-navigation follows.

Evidence: [crates/shamir-funclib/src/value_nav.rs:109](../../../../../crates/shamir-funclib/src/value_nav.rs#L109); [crates/shamir-funclib/src/value_nav.rs:111](../../../../../crates/shamir-funclib/src/value_nav.rs#L111).

<a id="review-3-6"></a>

### Claim 3.6 — `crypto/ct_eq` length short-circuit leaks operand length (documented; acceptable for its intended MAC-tag use)

Status: `not-applicable`. Current risk: —.

Intentional public-length behavior for fixed-size tags; content comparison remains subtle-based. No length-sensitive-secret caller was established.

Evidence: [crates/shamir-funclib/src/crypto.rs:303](../../../../../crates/shamir-funclib/src/crypto.rs#L303); [crates/shamir-funclib/src/crypto.rs:305](../../../../../crates/shamir-funclib/src/crypto.rs#L305).

Grouping/duplicate: `security-crypto.md#6`. This row is not another independent defect.

<a id="review-3-7"></a>

### Claim 3.7 — `argon2id` blocking semaphore acquire inline on runtime workers

Status: `confirmed-open`. Current risk: `medium`.

Inline query scalar execution remains and the named-function offload is a different entry point.

Evidence: [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-funclib/src/crypto.rs:221](../../../../../crates/shamir-funclib/src/crypto.rs#L221).

Grouping/duplicate: `SUMMARY.md#2.3`. This row is not another independent defect.

<a id="review-3-8"></a>

### Claim 3.8 — `canonical.rs` key serialization swallows failure (`unwrap_or_default`) on an integrity-critical path

Status: `confirmed-open`. Current risk: `low`.

Suppression remains reachable for public native custom Serialize keys, though ordinary String-keyed queries do not supply such implementations. Direct native Values also lack the asserted universal codec-depth bound.

Evidence: [crates/shamir-funclib/src/canonical.rs:187](../../../../../crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:199](../../../../../crates/shamir-funclib/src/canonical.rs#L199); [crates/shamir-types/src/types/value.rs:33](../../../../../crates/shamir-types/src/types/value.rs#L33).

Grouping/duplicate: `security-crypto.md#8`. This row is not another independent defect.

<a id="review-3-9"></a>

### Claim 3.9 — Robustness nits in parsers (bundle)

Status: `confirmed-open`. Current risk: `nit`.

eat slicing and midnight unwrap remain, but current cursor invariants and pinned chrono make them safe. Only local annotation/defensive-expression cleanup remains.

Evidence: [crates/shamir-funclib/src/validate.rs:115](../../../../../crates/shamir-funclib/src/validate.rs#L115); [crates/shamir-funclib/src/datetime.rs:153](../../../../../crates/shamir-funclib/src/datetime.rs#L153); [Cargo.lock:655](../../../../../Cargo.lock#L655).

Grouping/duplicate: `security-crypto.md#9`. This row is not another independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — `count_distinct` aggregator is O(N·C) — the exact legacy pattern `arrays::distinct` was fixed to remove

Status: `confirmed-open`. Current risk: `medium`.

The per-row scan remains. N(N-1)/2 comparisons for unique input are structural, not a benchmark result; sort replacement first requires lawful comparison semantics.

Evidence: [crates/shamir-funclib/src/agg.rs:202](../../../../../crates/shamir-funclib/src/agg.rs#L202); [crates/shamir-engine/src/query/read/aggregate.rs:889](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L889).

Grouping/duplicate: `performance-hotpath.md#1`. This row is not another independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — `validate/matches` recompiles the regex on every call

Status: `confirmed-open`. Current risk: `medium`.

Per-invocation compilation remains; numerical latency and allocator-contention claims are unverified.

Evidence: [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342).

Grouping/duplicate: `performance-hotpath.md#2`. This row is not another independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — `DistinctWrapper` dedup is O(N·C) per wrapped aggregate

Status: `confirmed-open`. Current risk: `medium`.

seen remains linearly searched with no enforced cardinality bound; sorted Vec insertion alone would retain linear movement cost.

Evidence: [crates/shamir-funclib/src/agg.rs:223](../../../../../crates/shamir-funclib/src/agg.rs#L223); [crates/shamir-funclib/src/agg.rs:245](../../../../../crates/shamir-funclib/src/agg.rs#L245); [crates/shamir-engine/src/query/read/aggregate.rs:996](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L996).

Grouping/duplicate: `performance-hotpath.md#3`. This row is not another independent defect.

<a id="review-4-4"></a>

### Claim 4.4 — `stddev`/`variance` buffer the entire column though an O(1)-state algorithm exists

Status: `confirmed-open`. Current risk: `medium`.

Both instances retain all non-null Decimal inputs. No streaming state or rounding/overflow contract is implemented.

Evidence: [crates/shamir-funclib/src/agg.rs:448](../../../../../crates/shamir-funclib/src/agg.rs#L448); [crates/shamir-funclib/src/agg.rs:484](../../../../../crates/shamir-funclib/src/agg.rs#L484); [crates/shamir-funclib/src/agg.rs:512](../../../../../crates/shamir-funclib/src/agg.rs#L512).

Grouping/duplicate: `performance-hotpath.md#4`. This row is not another independent defect.

<a id="review-4-5"></a>

### Claim 4.5 — `gen/random_bytes` attacker-controlled unbounded allocation

Status: `confirmed-open`. Current risk: `high`.

The allocation is still guarded only against negative lengths.

Evidence: [crates/shamir-funclib/src/gen.rs:72](../../../../../crates/shamir-funclib/src/gen.rs#L72); [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-4-6"></a>

### Claim 4.6 — `/strings` regex cache: process-global `Mutex` on the per-row hot path

Status: `confirmed-open`. Current risk: `medium`.

Global locking, compile-under-lock, and clear-all eviction are unchanged.

Evidence: [crates/shamir-funclib/src/strings.rs:418](../../../../../crates/shamir-funclib/src/strings.rs#L418); [crates/shamir-funclib/src/strings.rs:427](../../../../../crates/shamir-funclib/src/strings.rs#L427).

Grouping/duplicate: `SUMMARY.md#2.1`. This row is not another independent defect.

<a id="review-4-7"></a>

### Claim 4.7 — canonical `encode` re-serialises the reserved-key constant for every top-level map key

Status: `confirmed-open`. Current risk: `low`.

Per-key constant serialization remains. Automatic use on every production write is not supported by located callers.

Evidence: [crates/shamir-funclib/src/canonical.rs:156](../../../../../crates/shamir-funclib/src/canonical.rs#L156); [crates/shamir-funclib/src/canonical.rs:193](../../../../../crates/shamir-funclib/src/canonical.rs#L193); [crates/shamir-db/tests/cas_sequenced_e2e.rs:90](../../../../../crates/shamir-db/tests/cas_sequenced_e2e.rs#L90).

Grouping/duplicate: `performance-hotpath.md#7`. This row is not another independent defect.

<a id="review-4-8"></a>

### Claim 4.8 — Stable sorts where the total order permits unstable (scratch allocation per call)

Status: `refuted`. Current risk: —.

Equal comparison results include distinguishable value variants, making stable tie order and selected representatives observable; compare is additionally non-transitive. The proposed blanket substitution is not semantically identical.

Evidence: [crates/shamir-funclib/src/arrays.rs:186](../../../../../crates/shamir-funclib/src/arrays.rs#L186); [crates/shamir-funclib/src/agg.rs:435](../../../../../crates/shamir-funclib/src/agg.rs#L435); [crates/shamir-funclib/src/compare.rs:119](../../../../../crates/shamir-funclib/src/compare.rs#L119).

Grouping/duplicate: `performance-hotpath.md#8`. This row is not another independent defect.

<a id="review-4-9"></a>

### Claim 4.9 — `cast/to_bool` allocates a lowercased copy of the input per call

Status: `confirmed-open`. Current risk: `nit`.

The allocating lowercase conversion remains.

Evidence: [crates/shamir-funclib/src/cast.rs:177](../../../../../crates/shamir-funclib/src/cast.rs#L177).

Grouping/duplicate: `performance-hotpath.md#9`. This row is not another independent defect.

<a id="review-4-10"></a>

### Claim 4.10 — `value_nav` int-step-into-map allocates a key `String` per step

Status: `confirmed-open`. Current risk: `nit`.

idx.to_string remains in the numeric map navigation branch.

Evidence: [crates/shamir-funclib/src/value_nav.rs:120](../../../../../crates/shamir-funclib/src/value_nav.rs#L120).

Grouping/duplicate: `performance-hotpath.md#10`. This row is not another independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — Query-reachable scalars allocate unbounded memory

Status: `confirmed-open`. Current risk: `high`.

Registration and evaluated query/write reachability remain; all cited result-size parameters are still uncapped.

Evidence: [crates/shamir-funclib/src/lib.rs:53](../../../../../crates/shamir-funclib/src/lib.rs#L53); [crates/shamir-funclib/src/lib.rs:60](../../../../../crates/shamir-funclib/src/lib.rs#L60); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75).

Grouping/duplicate: `SUMMARY.md#6.2`. This row is not another independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — `canonical_hash` key ordering is msgpack-dependent but documented as name order; byte format has no version tag

Status: `confirmed-open`. Current risk: `low`.

Length-tagged key encoding, inaccurate raw-name ordering docs, and an unversioned canonical byte stream remain. No actual migration or cross-language mismatch was reproduced.

Evidence: [crates/shamir-funclib/src/canonical.rs:28](../../../../../crates/shamir-funclib/src/canonical.rs#L28); [crates/shamir-funclib/src/canonical.rs:187](../../../../../crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:203](../../../../../crates/shamir-funclib/src/canonical.rs#L203); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

Grouping/duplicate: `api-wire-protocol.md#2`. This row is not another independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — Machine error-code vocabulary is free-form and inconsistent across categories

Status: `confirmed-open`. Current risk: `low`.

No catalog exists and regex synonyms remain; client UX impact and the need to merge conversion/extraction codes are not established.

Evidence: [crates/shamir-funclib/src/registry.rs:20](../../../../../crates/shamir-funclib/src/registry.rs#L20); [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342); [crates/shamir-funclib/src/strings.rs:427](../../../../../crates/shamir-funclib/src/strings.rs#L427).

Grouping/duplicate: `SUMMARY.md#6.6`. This row is not another independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Module docs advertise plain unqualified names; the wire protocol dispatches folder-qualified names

Status: `confirmed-open`. Current risk: `low`.

Headers and stubs wording remain stale. Plain-name category tests validly exercise direct registration; production wiring coverage is only partial.

Evidence: [crates/shamir-funclib/src/math.rs:4](../../../../../crates/shamir-funclib/src/math.rs#L4); [crates/shamir-funclib/src/lib.rs:12](../../../../../crates/shamir-funclib/src/lib.rs#L12); [crates/shamir-funclib/src/lib.rs:51](../../../../../crates/shamir-funclib/src/lib.rs#L51); [crates/shamir-funclib/src/tests/register_builtins_tests.rs:24](../../../../../crates/shamir-funclib/src/tests/register_builtins_tests.rs#L24).

Grouping/duplicate: `api-wire-protocol.md#4`. This row is not another independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — Same public name `get_path` in two folders with opposite miss semantics

Status: `confirmed-open`. Current risk: `nit`.

Distinct qualified functions retain deliberately documented error-versus-Null contracts; only optional cross-reference/alignment remains, not a proved correctness violation.

Evidence: [crates/shamir-funclib/src/object.rs:10](../../../../../crates/shamir-funclib/src/object.rs#L10); [crates/shamir-funclib/src/value_nav.rs:11](../../../../../crates/shamir-funclib/src/value_nav.rs#L11); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374).

Grouping/duplicate: `api-wire-protocol.md#5`. This row is not another independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — `trusted_pure` gate: pub fields make the "explicit opt-in" convention-only; docs claim indexability the gate forbids

Status: `confirmed-open`. Current risk: `low`.

Public metadata and misleading builtin indexability docs remain, while engine eligibility checks still enforce all three flags. This is not an untrusted wire privilege bypass; specialized builtin expression indexes are separate.

Evidence: [crates/shamir-funclib/src/registry.rs:54](../../../../../crates/shamir-funclib/src/registry.rs#L54); [crates/shamir-funclib/src/arrays.rs:28](../../../../../crates/shamir-funclib/src/arrays.rs#L28); [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:249](../../../../../crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L249); [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:262](../../../../../crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L262).

Grouping/duplicate: `api-wire-protocol.md#6`. This row is not another independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — `f64 → i64` extraction accepts values above `i64::MAX` due to a float-rounded bound

Status: `confirmed-open`. Current risk: `low`.

The duplicated inclusive upper-bound defect remains.

Evidence: [crates/shamir-funclib/src/registry.rs:218](../../../../../crates/shamir-funclib/src/registry.rs#L218); [crates/shamir-funclib/src/cast.rs:121](../../../../../crates/shamir-funclib/src/cast.rs#L121).

Grouping/duplicate: `SUMMARY.md#1.3`. This row is not another independent defect.

<a id="review-5-8"></a>

### Claim 5.8 — `ScalarError` has no structured detail slot

Status: `confirmed-open`. Current risk: `nit`.

Only code: String is present; structured detail is an optional API enhancement under the existing code-only contract.

Evidence: [crates/shamir-funclib/src/registry.rs:20](../../../../../crates/shamir-funclib/src/registry.rs#L20).

Grouping/duplicate: `api-wire-protocol.md#8`. This row is not another independent defect.

<a id="review-5-9"></a>

### Claim 5.9 — `ScalarRegistry::register` collision policy undocumented

Status: `confirmed-open`. Current risk: `nit`.

Last-wins insertion remains undocumented in ScalarRegistry; AggRegistry explicitly documents it.

Evidence: [crates/shamir-funclib/src/registry.rs:126](../../../../../crates/shamir-funclib/src/registry.rs#L126); [crates/shamir-funclib/src/registry.rs:135](../../../../../crates/shamir-funclib/src/registry.rs#L135); [crates/shamir-funclib/src/agg.rs:72](../../../../../crates/shamir-funclib/src/agg.rs#L72).

Grouping/duplicate: `api-wire-protocol.md#9`. This row is not another independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — `validate/is_json` hand-rolled parser recurses without a depth limit — query-reachable stack overflow aborts the process

Status: `confirmed-open`. Current risk: `high`.

Unbounded recursive parsing remains and registered tests are shallow. Sufficient nested string content can exhaust stack outside ScalarError/unwind isolation. Exact thresholds and universal low-privilege/all-deployment claims were not proven.

Evidence: [crates/shamir-funclib/src/validate.rs:123](../../../../../crates/shamir-funclib/src/validate.rs#L123); [crates/shamir-funclib/src/validate.rs:178](../../../../../crates/shamir-funclib/src/validate.rs#L178); [crates/shamir-funclib/src/validate/tests/validate_tests.rs:183](../../../../../crates/shamir-funclib/src/validate/tests/validate_tests.rs#L183); [Cargo.toml:88](../../../../../Cargo.toml#L88).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Unbounded-allocation scalar paths: `strings/repeat`, `strings/pad_left`/`pad_right`, `gen/random_bytes`

Status: `confirmed-open`. Current risk: `high`.

No pre-allocation ceiling or checked output-byte calculation exists in any cited member; no cap-boundary tests are registered.

Evidence: [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75); [crates/shamir-funclib/src/strings.rs:237](../../../../../crates/shamir-funclib/src/strings.rs#L237); [crates/shamir-funclib/src/strings.rs:406](../../../../../crates/shamir-funclib/src/strings.rs#L406).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — `UserScalarLayer::register` discards scc's `Err` — documented "(or replace)" silently never replaces

Status: `confirmed-open`. Current risk: `medium`.

Occupied-key insert still leaves the old native function and discards the new entry. Pinned scc confirms the mechanism; WASM function replacement uses a separate registry.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:39](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L39); [crates/shamir-funclib/src/scalar_resolver.rs:41](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L41); [Cargo.lock:3123](../../../../../Cargo.lock#L3123); [crates/shamir-wasm-host/src/registry.rs:50](../../../../../crates/shamir-wasm-host/src/registry.rs#L50).

Grouping/duplicate: `error-handling-lifecycle.md#3`. This row is not another independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — `rust_decimal` arithmetic panics on overflow in agg and array reductions

Status: `confirmed-open`. Current risk: `medium`.

Unchecked arithmetic sites remain and pinned Decimal operators panic on overflow. Existing tests do not cover MAX pairs or wide deviations.

Evidence: [crates/shamir-funclib/src/agg.rs:286](../../../../../crates/shamir-funclib/src/agg.rs#L286); [crates/shamir-funclib/src/agg.rs:514](../../../../../crates/shamir-funclib/src/agg.rs#L514); [crates/shamir-funclib/src/agg.rs:852](../../../../../crates/shamir-funclib/src/agg.rs#L852); [crates/shamir-funclib/src/arrays.rs:276](../../../../../crates/shamir-funclib/src/arrays.rs#L276); [Cargo.lock:2979](../../../../../Cargo.lock#L2979).

Grouping/duplicate: `error-handling-lifecycle.md#4`. This row is not another independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — `datetime/age` performs unchecked `i64` subtraction — inconsistent with the file's own checked discipline

Status: `confirmed-open`. Current risk: `medium`.

Unchecked age subtraction and non-floor division remain; the sibling diff_secs fix did not fix age.

Evidence: [crates/shamir-funclib/src/datetime.rs:78](../../../../../crates/shamir-funclib/src/datetime.rs#L78); [crates/shamir-funclib/src/datetime.rs:264](../../../../../crates/shamir-funclib/src/datetime.rs#L264).

Grouping/duplicate: `error-handling-lifecycle.md#5`. This row is not another independent defect.

<a id="review-6-6"></a>

### Claim 6.6 — `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context

Status: `confirmed-open`. Current risk: `low`.

Free-form allocating string codes, regex synonyms, and absent detail remain. The struct itself satisfies Error/Display; concrete frontend breakage is unverified.

Evidence: [crates/shamir-funclib/src/registry.rs:20](../../../../../crates/shamir-funclib/src/registry.rs#L20); [crates/shamir-funclib/src/strings.rs:427](../../../../../crates/shamir-funclib/src/strings.rs#L427); [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342).

Grouping/duplicate: `error-handling-lifecycle.md#6`. This row is not another independent defect.

<a id="review-6-7"></a>

### Claim 6.7 — Argon2id semaphore: poison-propagating `expect`s, and the in-flight counter leaks on panic

Status: `confirmed-open`. Current risk: `nit`.

Poison propagation and manual telemetry bracketing remain conditional hardening concerns. No normal KDF error leaks the counter or permit.

Evidence: [crates/shamir-funclib/src/crypto.rs:139](../../../../../crates/shamir-funclib/src/crypto.rs#L139); [crates/shamir-funclib/src/crypto.rs:141](../../../../../crates/shamir-funclib/src/crypto.rs#L141); [crates/shamir-funclib/src/crypto.rs:225](../../../../../crates/shamir-funclib/src/crypto.rs#L225); [crates/shamir-funclib/src/crypto.rs:230](../../../../../crates/shamir-funclib/src/crypto.rs#L230).

Grouping/duplicate: `error-handling-lifecycle.md#7`. This row is not another independent defect.

<a id="review-6-8"></a>

### Claim 6.8 — Canonical-hash path silently degrades serialization failures to empty key bytes

Status: `confirmed-open`. Current risk: `low`.

Generic key serialization still converts errors into empty bytes; native custom-key callers are the supported reachability scope.

Evidence: [crates/shamir-funclib/src/canonical.rs:187](../../../../../crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:199](../../../../../crates/shamir-funclib/src/canonical.rs#L199).

Grouping/duplicate: `SUMMARY.md#3.8`. This row is not another independent defect.

<a id="review-6-9"></a>

### Claim 6.9 — `ScalarResolver` / `UserScalarLayer` have zero tests — their error paths are unverified

Status: `confirmed-open`. Current risk: `low`.

No dedicated resolver error/collision/concurrency suite exists.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:135](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L135); [crates/shamir-funclib/src/tests/mod.rs:1](../../../../../crates/shamir-funclib/src/tests/mod.rs#L1).

Grouping/duplicate: `SUMMARY.md#7.1`. This row is not another independent defect.

<a id="review-6-10"></a>

### Claim 6.10 — Unannotated panicky/fallible primitives in library code (bundle)

Status: `confirmed-open`. Current risk: `nit`.

Midnight unwrap, Decimal-to-f64 NaN fallback, and invariant arrays unreachable remain annotation/readability opportunities. Pinned dependencies prove the first two failure branches unreachable for valid inputs; the cited agg unreachable is absent.

Evidence: [crates/shamir-funclib/src/datetime.rs:153](../../../../../crates/shamir-funclib/src/datetime.rs#L153); [crates/shamir-funclib/src/agg.rs:472](../../../../../crates/shamir-funclib/src/agg.rs#L472); [crates/shamir-funclib/src/arrays.rs:268](../../../../../crates/shamir-funclib/src/arrays.rs#L268); [Cargo.lock:655](../../../../../Cargo.lock#L655); [Cargo.lock:2979](../../../../../Cargo.lock#L2979).

Grouping/duplicate: `error-handling-lifecycle.md#10`. This row is not another independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — `scalar_resolver` is the only real module with no tests and no `tests/` dir

Status: `confirmed-open`. Current risk: `low`.

Resolver-specific tests remain absent despite registered external successful-dispatch tests. Registry also lacks its own test directory but has tests under math.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:145](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L145); [crates/shamir-funclib/src/math/tests/mod.rs:2](../../../../../crates/shamir-funclib/src/math/tests/mod.rs#L2); [crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs:51](../../../../../crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs#L51).

Grouping/duplicate: `style-claude-md.md#1`. This row is not another independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — Stale crate-level doc claims all non-math categories are "stubs"

Status: `confirmed-open`. Current risk: `nit`.

The stale front-door documentation remains.

Evidence: [crates/shamir-funclib/src/lib.rs:12](../../../../../crates/shamir-funclib/src/lib.rs#L12).

Grouping/duplicate: `style-claude-md.md#2`. This row is not another independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — Tests for five flat modules consolidated in `src/tests/` instead of per-module `tests/` dirs

Status: `confirmed-open`. Current risk: `nit`.

Four category suites remain root-shared but reachable. Builtin-registration tests are correctly rooted, not a fifth misplaced category suite.

Evidence: [crates/shamir-funclib/src/tests/mod.rs:1](../../../../../crates/shamir-funclib/src/tests/mod.rs#L1); [crates/shamir-funclib/src/lib.rs:84](../../../../../crates/shamir-funclib/src/lib.rs#L84).

Grouping/duplicate: `style-claude-md.md#3`. This row is not another independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — `registry` contract tests nested under `math/tests/`

Status: `confirmed-open`. Current risk: `nit`.

Placement remains unchanged; math's external test registration reaches the suite.

Evidence: [crates/shamir-funclib/src/math/tests/mod.rs:2](../../../../../crates/shamir-funclib/src/math/tests/mod.rs#L2); [crates/shamir-funclib/src/math/tests/registry_tests.rs:1](../../../../../crates/shamir-funclib/src/math/tests/registry_tests.rs#L1).

Grouping/duplicate: `style-claude-md.md#4`. This row is not another independent defect.

<a id="review-7-5"></a>

### Claim 7.5 — `use` statements inside function bodies

Status: `confirmed-open`. Current risk: `nit`.

All four cited local-import sites remain; cleanup is style-only.

Evidence: [crates/shamir-funclib/src/lib.rs:72](../../../../../crates/shamir-funclib/src/lib.rs#L72); [crates/shamir-funclib/src/scalar_resolver.rs:89](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L89); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:208](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L208); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:288](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L288).

Grouping/duplicate: `style-claude-md.md#5`. This row is not another independent defect.

<a id="review-7-6"></a>

### Claim 7.6 — Doc/code drift: `random()` documented as returning `F64`, actually `Dec`

Status: `confirmed-open`. Current risk: `nit`.

Header still says F64, while v_f64 stores Dec and the test permits either.

Evidence: [crates/shamir-funclib/src/gen.rs:15](../../../../../crates/shamir-funclib/src/gen.rs#L15); [crates/shamir-funclib/src/registry.rs:307](../../../../../crates/shamir-funclib/src/registry.rs#L307); [crates/shamir-funclib/src/gen/tests/gen_tests.rs:78](../../../../../crates/shamir-funclib/src/gen/tests/gen_tests.rs#L78).

Grouping/duplicate: `style-claude-md.md#6`. This row is not another independent defect.

<a id="review-7-7"></a>

### Claim 7.7 — Doc drift: encode.rs header lists `str_escape_chars` as a registered function name

Status: `confirmed-open`. Current risk: `nit`.

Catalog still names str_escape_chars; registration uses json_escape.

Evidence: [crates/shamir-funclib/src/encode.rs:5](../../../../../crates/shamir-funclib/src/encode.rs#L5); [crates/shamir-funclib/src/encode.rs:139](../../../../../crates/shamir-funclib/src/encode.rs#L139).

Grouping/duplicate: `style-claude-md.md#7`. This row is not another independent defect.

<a id="review-7-8"></a>

### Claim 7.8 — `scalar_resolver` doc references `builtin_scalars()` without a path

Status: `confirmed-open`. Current risk: `nit`.

The prose alias remains unqualified; local static_builtin and wasm-host alias are distinct named entry points.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:3](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L3); [crates/shamir-funclib/src/lib.rs:71](../../../../../crates/shamir-funclib/src/lib.rs#L71); [crates/shamir-wasm-host/src/scalar.rs:20](../../../../../crates/shamir-wasm-host/src/scalar.rs#L20).

Grouping/duplicate: `style-claude-md.md#8`. This row is not another independent defect.

<a id="review-7-9"></a>

### Claim 7.9 — Category-header phrasing drift about folder qualification

Status: `confirmed-open`. Current risk: `nit`.

Production qualification remains omitted in eleven category headers.

Evidence: [crates/shamir-funclib/src/math.rs:4](../../../../../crates/shamir-funclib/src/math.rs#L4); [crates/shamir-funclib/src/crypto.rs:3](../../../../../crates/shamir-funclib/src/crypto.rs#L3); [crates/shamir-funclib/src/lib.rs:51](../../../../../crates/shamir-funclib/src/lib.rs#L51).

Grouping/duplicate: `SUMMARY.md#5.4`. This row is not another independent defect.

<a id="review-7-10"></a>

### Claim 7.10 — Single-line JSON literals in tests

Status: `confirmed-open`. Current risk: `nit`.

Both cited one-line parser/codec fixture strings remain.

Evidence: [crates/shamir-funclib/src/tests/encode_tests.rs:157](../../../../../crates/shamir-funclib/src/tests/encode_tests.rs#L157); [crates/shamir-funclib/src/validate/tests/validate_tests.rs:186](../../../../../crates/shamir-funclib/src/validate/tests/validate_tests.rs#L186).

Grouping/duplicate: `style-claude-md.md#10`. This row is not another independent defect.

<a id="review-7-11"></a>

### Claim 7.11 — `registry.rs` and `agg.rs` stretch "one file = one primary export"

Status: `not-applicable`. Current risk: —.

Both are closely-coupled groups allowed by the stated rule. Splitting helpers is optional organization, not a required defect fix.

Evidence: [crates/shamir-funclib/src/registry.rs:39](../../../../../crates/shamir-funclib/src/registry.rs#L39); [crates/shamir-funclib/src/agg.rs:45](../../../../../crates/shamir-funclib/src/agg.rs#L45); [CLAUDE.md:492](../../../../../CLAUDE.md#L492).

Grouping/duplicate: `style-claude-md.md#11`. This row is not another independent defect.

## Current fix-plan state

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 21 | 21 | 0 | 0 | 0 | 0 | 0 |

A source-fixed item closes only its stated mechanism. Partial items retain the obligations named below; proposed fixes must obey the corrections and current contracts, not merely copy the historical recipe.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`. Current risk: —.

Neither parser depth cap nor allocation ceilings have landed. Add deterministic boundary tests; use checked output-byte calculations rather than char limits alone. The documented exact crash-size thresholds are not acceptance criteria.

Evidence: [crates/shamir-funclib/src/validate.rs:98](../../../../../crates/shamir-funclib/src/validate.rs#L98); [crates/shamir-funclib/src/validate.rs:178](../../../../../crates/shamir-funclib/src/validate.rs#L178); [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75); [crates/shamir-funclib/src/strings.rs:406](../../../../../crates/shamir-funclib/src/strings.rs#L406).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`. Current risk: —.

Same-name native replacement, resolver contract tests, and read_sync migration remain absent. Use an atomic occupied/vacant entry operation rather than remove-then-insert; read_sync is shared-locked, not lock-free.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:41](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L41); [crates/shamir-funclib/src/scalar_resolver.rs:46](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L46); [crates/shamir-funclib/src/scalar_resolver.rs:145](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L145); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`. Current risk: —.

Notifier still does not acquire the wait mutex and no targeted lost-wakeup test exists. Synchronize notification with check-to-wait; unconditional release locking would affect every release, not just contended ones. The suggested sibling check also finds the same notification shape.

Evidence: [crates/shamir-funclib/src/crypto.rs:140](../../../../../crates/shamir-funclib/src/crypto.rs#L140); [crates/shamir-funclib/src/crypto.rs:145](../../../../../crates/shamir-funclib/src/crypto.rs#L145); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:207](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L207); [crates/shamir-connect/src/server/argon2_semaphore.rs:113](../../../../../crates/shamir-connect/src/server/argon2_semaphore.rs#L113).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`. Current risk: —.

Global cache mutex, compile-under-lock, whole-cache eviction, and uncached validate/matches remain. A bounded shared helper must compile outside cache guards and consciously preserve or migrate error codes. scc HashMap alone does not give lock-free lookup.

Evidence: [crates/shamir-funclib/src/strings.rs:418](../../../../../crates/shamir-funclib/src/strings.rs#L418); [crates/shamir-funclib/src/strings.rs:427](../../../../../crates/shamir-funclib/src/strings.rs#L427); [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`. Current risk: —.

Both aggregate scan dedup paths remain and aggregate scale coverage is absent. The proposed sorting/binary-search replacements require resolving compare's non-transitive equality first; sorted Vec insertion retains O(C) movement, and order-sensitive inner aggregates must retain their semantics.

Evidence: [crates/shamir-funclib/src/agg.rs:202](../../../../../crates/shamir-funclib/src/agg.rs#L202); [crates/shamir-funclib/src/agg.rs:245](../../../../../crates/shamir-funclib/src/agg.rs#L245); [crates/shamir-funclib/src/compare.rs:132](../../../../../crates/shamir-funclib/src/compare.rs#L132); [crates/shamir-funclib/benches/distinct_arrays.rs:75](../../../../../crates/shamir-funclib/benches/distinct_arrays.rs#L75).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`. Current risk: —.

Decimal checked arithmetic and age checked/floored arithmetic remain absent, with no relevant extreme-value regressions. Remove the value_nav checked_add requirement: its claimed overflow is refuted. Select a stable overflow-code contract rather than assuming a new code is compatible.

Evidence: [crates/shamir-funclib/src/agg.rs:286](../../../../../crates/shamir-funclib/src/agg.rs#L286); [crates/shamir-funclib/src/agg.rs:514](../../../../../crates/shamir-funclib/src/agg.rs#L514); [crates/shamir-funclib/src/arrays.rs:276](../../../../../crates/shamir-funclib/src/arrays.rs#L276); [crates/shamir-funclib/src/datetime.rs:78](../../../../../crates/shamir-funclib/src/datetime.rs#L78); [crates/shamir-funclib/src/value_nav.rs:109](../../../../../crates/shamir-funclib/src/value_nav.rs#L109).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`. Current risk: —.

No Welford/streaming implementation exists. A replacement needs a defined Decimal rounding/accuracy and checked-overflow contract, not merely relaxed test tolerances.

Evidence: [crates/shamir-funclib/src/agg.rs:448](../../../../../crates/shamir-funclib/src/agg.rs#L448); [crates/shamir-funclib/src/agg.rs:484](../../../../../crates/shamir-funclib/src/agg.rs#L484); [crates/shamir-funclib/src/agg.rs:512](../../../../../crates/shamir-funclib/src/agg.rs#L512).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`. Current risk: —.

No code catalog, all-literals-known test, synonym migration, or detail slot exists. Cataloging is supported; automatic code deduplication and detail addition require compatibility decisions and are not proven runtime fixes.

Evidence: [crates/shamir-funclib/src/registry.rs:20](../../../../../crates/shamir-funclib/src/registry.rs#L20); [crates/shamir-funclib/src/validate/tests/validate_tests.rs:178](../../../../../crates/shamir-funclib/src/validate/tests/validate_tests.rs#L178); [crates/shamir-funclib/src/tests/strings_tests.rs:241](../../../../../crates/shamir-funclib/src/tests/strings_tests.rs#L241).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`. Current risk: —.

Ordering docs, unversioned bytes, generic error suppression, and per-key constant serialization remain. Prefer documenting/fixing a frozen current format or an explicit migration; a prefix/sort change silently alters hashes. Do not add a false universal codec-depth guarantee, and debug_assert alone is not release-safe error handling.

Evidence: [crates/shamir-funclib/src/canonical.rs:28](../../../../../crates/shamir-funclib/src/canonical.rs#L28); [crates/shamir-funclib/src/canonical.rs:187](../../../../../crates/shamir-funclib/src/canonical.rs#L187); [crates/shamir-funclib/src/canonical.rs:193](../../../../../crates/shamir-funclib/src/canonical.rs#L193); [crates/shamir-funclib/src/canonical.rs:203](../../../../../crates/shamir-funclib/src/canonical.rs#L203); [crates/shamir-types/src/types/value.rs:33](../../../../../crates/shamir-types/src/types/value.rs#L33).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`. Current risk: —.

Eleven headers and the stubs sentence remain unchanged; behavioral suites mostly register directly. Keep valid direct-registration tests and add production-qualified coverage rather than asserting those plain names are invalid APIs.

Evidence: [crates/shamir-funclib/src/math.rs:4](../../../../../crates/shamir-funclib/src/math.rs#L4); [crates/shamir-funclib/src/lib.rs:12](../../../../../crates/shamir-funclib/src/lib.rs#L12); [crates/shamir-funclib/src/math/tests/math_tests.rs:9](../../../../../crates/shamir-funclib/src/math/tests/math_tests.rs#L9); [crates/shamir-funclib/src/tests/register_builtins_tests.rs:24](../../../../../crates/shamir-funclib/src/tests/register_builtins_tests.rs#L24).

<a id="plan-p1-11"></a>

### Plan P1.11 — P1.11

Status: `confirmed-open`. Current risk: —.

Query/computed-write scalar execution remains inline; neither try-acquire throttling nor worker offload covers these routes. Named Argon2idFunction is already offloaded, but does not close the scalar finding. Preserve synchronous callers while choosing an async dispatch/admission contract.

Evidence: [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-engine/src/table/write_helpers.rs:314](../../../../../crates/shamir-engine/src/table/write_helpers.rs#L314); [crates/shamir-funclib/src/crypto.rs:221](../../../../../crates/shamir-funclib/src/crypto.rs#L221); [crates/shamir-wasm-host/src/builtin.rs:63](../../../../../crates/shamir-wasm-host/src/builtin.rs#L63).

<a id="plan-p1-12"></a>

### Plan P1.12 — P1.12

Status: `confirmed-open`. Current risk: —.

The exclusive float upper-bound fix and dedicated F64(2^63) tests remain absent. A float roundtrip alone is not sufficient to reject the saturated upper edge.

Evidence: [crates/shamir-funclib/src/registry.rs:218](../../../../../crates/shamir-funclib/src/registry.rs#L218); [crates/shamir-funclib/src/cast.rs:121](../../../../../crates/shamir-funclib/src/cast.rs#L121); [crates/shamir-funclib/src/cast/tests/cast_tests.rs:21](../../../../../crates/shamir-funclib/src/cast/tests/cast_tests.rs#L21).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`. Current risk: —.

Cross-module equality policy remains split and signed-zero Hash/Eq incompatibility remains unfixed. Prioritize the concrete hash invariant; choose and test any broader cross-type policy separately.

Evidence: [crates/shamir-funclib/src/arrays.rs:85](../../../../../crates/shamir-funclib/src/arrays.rs#L85); [crates/shamir-funclib/src/arrays.rs:170](../../../../../crates/shamir-funclib/src/arrays.rs#L170); [crates/shamir-types/src/types/value.rs:293](../../../../../crates/shamir-types/src/types/value.rs#L293); [crates/shamir-types/src/types/value.rs:707](../../../../../crates/shamir-types/src/types/value.rs#L707).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`. Current risk: —.

No lawful numeric comparator or boundary-triple test exists. Documentation softening and exact Dec/Big alone cannot close this item because Int/F64 already violates transitivity; ordering-dependent algorithms need a valid comparator.

Evidence: [crates/shamir-funclib/src/compare.rs:3](../../../../../crates/shamir-funclib/src/compare.rs#L3); [crates/shamir-funclib/src/compare.rs:115](../../../../../crates/shamir-funclib/src/compare.rs#L115); [crates/shamir-funclib/src/compare.rs:132](../../../../../crates/shamir-funclib/src/compare.rs#L132); [crates/shamir-funclib/src/compare/tests/compare_tests.rs:342](../../../../../crates/shamir-funclib/src/compare/tests/compare_tests.rs#L342).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`. Current risk: —.

The distinct get_path contracts remain, and neither cross-reference nor rename/alignment has landed. Their individual behavior is already documented and tested; changing it is an optional API decision, not a required correctness fix.

Evidence: [crates/shamir-funclib/src/object.rs:10](../../../../../crates/shamir-funclib/src/object.rs#L10); [crates/shamir-funclib/src/value_nav.rs:11](../../../../../crates/shamir-funclib/src/value_nav.rs#L11); [crates/shamir-funclib/src/tests/object_tests.rs:100](../../../../../crates/shamir-funclib/src/tests/object_tests.rs#L100); [crates/shamir-funclib/src/tests/value_nav_tests.rs:57](../../../../../crates/shamir-funclib/src/tests/value_nav_tests.rs#L57).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`. Current risk: —.

Fields remain public and builtin indexability docs remain misleading. Metadata privacy is optional API hardening within the already-trusted native embedder boundary; correcting docs is the supported requirement. The engine eligibility check remains intact.

Evidence: [crates/shamir-funclib/src/registry.rs:54](../../../../../crates/shamir-funclib/src/registry.rs#L54); [crates/shamir-funclib/src/arrays.rs:28](../../../../../crates/shamir-funclib/src/arrays.rs#L28); [crates/shamir-funclib/src/cast.rs:18](../../../../../crates/shamir-funclib/src/cast.rs#L18); [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:262](../../../../../crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L262).

<a id="plan-p2-17"></a>

### Plan P2.17 — P2.17

Status: `confirmed-open`. Current risk: —.

Resolver get_sync migration remains open. ct_eq already comments on unequal-length behavior and is correct for public fixed-size tag lengths; its stronger length-sensitive-secret note is optional, not an open security defect.

Evidence: [crates/shamir-funclib/src/scalar_resolver.rs:46](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L46); [crates/shamir-funclib/src/crypto.rs:303](../../../../../crates/shamir-funclib/src/crypto.rs#L303); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `confirmed-open`. Current risk: —.

Poison-tolerant handling, telemetry RAII, defensive eat spelling, midnight annotation, and NaN-fallback cleanup have not landed. These remain conditional/optional hardening: pinned chrono and Decimal prove the cited conversion failures unreachable, and the current parser cursor invariant is safe.

Evidence: [crates/shamir-funclib/src/crypto.rs:139](../../../../../crates/shamir-funclib/src/crypto.rs#L139); [crates/shamir-funclib/src/crypto.rs:225](../../../../../crates/shamir-funclib/src/crypto.rs#L225); [crates/shamir-funclib/src/validate.rs:115](../../../../../crates/shamir-funclib/src/validate.rs#L115); [crates/shamir-funclib/src/datetime.rs:153](../../../../../crates/shamir-funclib/src/datetime.rs#L153); [crates/shamir-funclib/src/agg.rs:472](../../../../../crates/shamir-funclib/src/agg.rs#L472).

<a id="plan-p2-19"></a>

### Plan P2.19 — P2.19

Status: `confirmed-open`. Current risk: —.

Four category test moves, registry test relocation, and all four cited import hoists remain undone. Existing suites are wired; these changes alone do not add missing behavioral coverage.

Evidence: [crates/shamir-funclib/src/tests/mod.rs:1](../../../../../crates/shamir-funclib/src/tests/mod.rs#L1); [crates/shamir-funclib/src/math/tests/mod.rs:2](../../../../../crates/shamir-funclib/src/math/tests/mod.rs#L2); [crates/shamir-funclib/src/lib.rs:72](../../../../../crates/shamir-funclib/src/lib.rs#L72); [crates/shamir-funclib/src/scalar_resolver.rs:89](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L89); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:208](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L208); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:288](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L288).

<a id="plan-p2-20"></a>

### Plan P2.20 — P2.20

Status: `confirmed-open`. Current risk: —.

to_bool and numeric-key allocation optimizations remain absent. Remove the blanket unstable-sort prescription: equal-but-distinguishable values make stable order observable, and compare is not total. A stack formatter must accommodate the sign of i64::MIN.

Evidence: [crates/shamir-funclib/src/cast.rs:177](../../../../../crates/shamir-funclib/src/cast.rs#L177); [crates/shamir-funclib/src/value_nav.rs:120](../../../../../crates/shamir-funclib/src/value_nav.rs#L120); [crates/shamir-funclib/src/arrays.rs:186](../../../../../crates/shamir-funclib/src/arrays.rs#L186); [crates/shamir-funclib/src/agg.rs:435](../../../../../crates/shamir-funclib/src/agg.rs#L435); [crates/shamir-funclib/src/compare.rs:132](../../../../../crates/shamir-funclib/src/compare.rs#L132).

<a id="plan-p2-21"></a>

### Plan P2.21 — P2.21

Status: `confirmed-open`. Current risk: —.

All named doc/literal/collision/detail nits remain. The optional helper split is not a rule violation. The redirected 1.9 bundle also requires separately considering dead bool any fields and omitted type_of variant tests; doc edits alone would not close it.

Evidence: [crates/shamir-funclib/src/strings.rs:130](../../../../../crates/shamir-funclib/src/strings.rs#L130); [crates/shamir-funclib/src/gen.rs:15](../../../../../crates/shamir-funclib/src/gen.rs#L15); [crates/shamir-funclib/src/encode.rs:5](../../../../../crates/shamir-funclib/src/encode.rs#L5); [crates/shamir-funclib/src/scalar_resolver.rs:3](../../../../../crates/shamir-funclib/src/scalar_resolver.rs#L3); [crates/shamir-funclib/src/registry.rs:126](../../../../../crates/shamir-funclib/src/registry.rs#L126); [crates/shamir-funclib/src/tests/encode_tests.rs:157](../../../../../crates/shamir-funclib/src/tests/encode_tests.rs#L157); [crates/shamir-funclib/src/agg.rs:724](../../../../../crates/shamir-funclib/src/agg.rs#L724); [crates/shamir-funclib/src/tests/value_nav_tests.rs:143](../../../../../crates/shamir-funclib/src/tests/value_nav_tests.rs#L143).

## Corrections and qualified non-findings

- No numbered finding or plan item was omitted. The seven lens reports have no separate Fix Plan sections; their Suggested fixes are assessed in matching rows, with consolidated action coverage here.
- The executive statement that all headline defects kill the whole process is overbroad: stack exhaustion/allocation failure can abort, but capacity-overflow and arithmetic panics normally unwind under the configured release profile.
- The stack example conflates 100000–1000000 opening brackets with 10–20 KB of text. Exact crash thresholds must be removed or explicitly marked unverified.
- Positive source guarantees retained: no funclib unsafe/async/await; Fx-backed collections; no banned scc len calls; generation impurity metadata; KDF parameter/admission caps and RAII permit release; named Argon2 wrapper offload; registered primitive KATs; selected canonical order/Dec/Big roundtrip tests; exact Int/Big and NaN regression mechanisms; external test manifests; bench-scale harness and doctest=false.
- Positive guarantees narrowed: scc HashMap populated reads/inserts are not lock-free; read_sync uses shared locks; barrier release does not prove KDF overlap; the Argon2 reference uses the same dependency rather than an independent implementation; per-function and per-directory coverage is not universal.
- Arrays distinct has expected hash-based membership complexity, but the 500-element test is an output oracle, not a complexity gate. No aggregate scale benchmark exists.
- Canonical tests cover selected contracts, not frozen cross-language byte vectors, every variant's storage roundtrip, or arbitrary native input depth. Automatic per-production-write CAS hashing is not established by the test-local CasValidator.
- The original 64 lens-tagged numbered rows are a document census. The 47 distinct-defect count includes refuted claims, optional enhancements, and bundled items and must not be reused as the current open-defect count.
- The stated historical chrono range change since 0.4.32 remains unverified; current pinned source independently proves the midnight invariant.
- Do not count duplicate rows as separate current defects or count optional style/API ergonomics as runtime High issues.

## Additional observations from revalidation

These were found while validating the original claims/remediations. They are separate from the original-row counts; cross-module repeats are not extra unique defects.

<a id="new-1"></a>

### Observation NEW.1 — Argon2 concurrency-cap test still relies on scheduling-dependent saturation after the claimed deterministic-flake fixes

Status: `confirmed-open`. Current risk: `low`.

The start barrier synchronizes readiness, not execution inside the permit-held region. No post-acquire latch retains permits until all admitted callers enter. A valid schedule can run callers successively and produce peak < 16, failing the registered peak == cap assertion without a production cap violation. History d7e07eb5 added the barrier; 2a17f71c only increased KDF work after the same under-saturation recurred. That widens a probabilistic overlap window rather than proving determinism. This qualifies the previous reviews' claimed completed test hardening; no current flake was reproduced.

Evidence: [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:229](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L229); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:245](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L245); [crates/shamir-funclib/src/crypto/tests/crypto_tests.rs:279](../../../../../crates/shamir-funclib/src/crypto/tests/crypto_tests.rs#L279); [crates/shamir-funclib/src/crypto.rs:225](../../../../../crates/shamir-funclib/src/crypto.rs#L225).

## Current follow-up order

1. P0: bound inner JSON-parser depth and scalar output bytes before allocation; add deterministic rejection/boundary oracles.
2. P0: synchronize semaphore release with the check-to-wait window and add a controlled lost-wakeup regression.
3. P1: honor native user-scalar replacement with specified concurrent semantics; add dedicated resolver shadowing, collision, arity, fallback, identity, and concurrency tests.
4. P1: repair numeric comparison transitivity and signed-zero Hash/Eq compatibility before adopting ordered aggregate dedup; independently reject F64(2^63) integer conversion.
5. P1: use checked Decimal reductions and checked age subtraction with an explicit rounding/error-code contract.
6. P1: remove runtime-worker KDF blocking on scalar-expression routes; retain the already-offloaded named wrapper.
7. P1: share bounded regex compilation outside global guards; then address aggregate dedup complexity and variance buffering with explicit semantic/accuracy contracts.
8. P2: freeze/document canonical encoding or design an explicit compatible version migration; propagate custom-key serialization errors and hoist reserved-key encoding.
9. P2: replace scheduling-dependent Argon2 saturation assumptions with controlled synchronization; retain meaningful upper-bound and wakeup oracles.
10. P2: resolve error-code/documentation contracts and remaining layout/import/allocation nits without blanket unstable sorting, unnecessary value_nav overflow changes, or mandatory closely-coupled-file splitting.

## Coverage and limitations

- Source-only revalidation: no builds, tests, benchmarks, reproductions, edits, worktrees, or child agents.
- All eight assigned Markdown files were read completely; no TASK_GROUPS.md exists in this directory.
- Allocation and recursion mechanisms are established structurally; exact crash thresholds, timings, throughput changes, and exploit demonstrations remain unverified.
- Security reachability is established through evaluated scalar expressions. It requires permission to execute the relevant query/write or an applicable validator; unauthenticated reachability and every deployment configuration were not established.
- Lockfile-pinned chrono 0.4.43, scc 3.8.4, rust_decimal 1.40.0, and rmp-serde 1.3.1 source archives were read without extraction to disk. Dependency evidence below cites repository lockfile pins rather than private cache paths.
- Final HEAD remained the requested commit, with no working-tree differences observed.

## Reviewed document inventory

- [correctness-tdd.md](./correctness-tdd.md) — 9 claim decisions; 0 explicit plan items.
- [concurrency-lockfree.md](./concurrency-lockfree.md) — 6 claim decisions; 0 explicit plan items.
- [security-crypto.md](./security-crypto.md) — 9 claim decisions; 0 explicit plan items.
- [performance-hotpath.md](./performance-hotpath.md) — 10 claim decisions; 0 explicit plan items.
- [api-wire-protocol.md](./api-wire-protocol.md) — 9 claim decisions; 0 explicit plan items.
- [error-handling-lifecycle.md](./error-handling-lifecycle.md) — 10 claim decisions; 0 explicit plan items.
- [style-claude-md.md](./style-claude-md.md) — 11 claim decisions; 0 explicit plan items.
- [SUMMARY.md](./SUMMARY.md) — 64 claim decisions; 21 explicit plan items.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-funclib — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate review)

Crate: `crates/shamir-funclib/` — ShamirDB's built-in scalar/aggregate function library: one
`ScalarRegistry`/`ScalarResolver` dispatch table over `fn(&[QueryValue]) -> ScalarResult`,
13 category modules + the aggregate layer + the canonical-hash CAS codec, folder-qualified
on the wire (`math/abs`), consumed by shamir-engine filters, schema validator field rules,
and WASM guests.

Review basis: the seven 2026-08-14 lens reports for this crate under
`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-funclib/` —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and synthesized read-only (no builds, tests, or lint
runs; no source modified). Structure/tone/rigor calibrated on the two completed exemplar
syntheses, `shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. The
workspace-wide `SUMMARY.md` row (1 crit / 8 high / 17 med / 23 low / 15 nit = 64) is the
pre-dedup lens-tagged census and is reproduced here as the raw count. Key file:line
references were spot-checked against source during synthesis; the one disagreement between
lens files (reachability of the `datetime.rs:153` `unwrap`, findings 1.2 vs 3.9) was
resolved against the pinned chrono version and is annotated inline.

## Executive summary

This crate carries a **CRITICAL**, and it is one of the workspace's top-3 headline issues:
`validate/is_json` is a hand-rolled recursive-descent JSON validator with **no depth cap**
(`validate.rs:123-191`), so one low-privilege query containing ~10–20 KB of nested `[` is a
hard stack-overflow **abort** — not a catchable panic — that kills the whole process and
defeats the `panic = "unwind"` per-connection isolation the architecture depends on. It is
the worst member of a four-strong "one authenticated session kills the server" family: the
same dispatch path also carries **uncapped allocations** in `gen/random_bytes`,
`strings/repeat`, and `strings/pad_*` (`gen.rs:75`, `strings.rs:237,406`), any of which
aborts the process via allocator OOM/capacity overflow from a single query. Fix the cap
family, the silently-broken `UserScalarLayer::register` replace contract (scc `Err`
swallowed — `CREATE OR REPLACE FUNCTION` keeps the old body), and the lost-wakeup in the
Argon2 semaphore before anything else ships from this crate; the remaining 47-defect
census (5 high, 11 medium, 16 low, 14 nit after dedup) is a P1/P2 program dominated by
hot-path quadratics (`count_distinct` O(N·C), per-row regex recompiles), `rust_decimal`
panic-on-overflow reductions, and doc drift. The crypto core itself (SHA/HMAC/BLAKE3/
argon2id) is in genuinely good shape — the findings concentrate on the *sibling* untrusted
input paths that share its dispatch.

---

## 1. correctness-tdd

### 1.1 — medium — *(primary: same as 6.2)* — Query-reachable unbounded allocations/panics in `random_bytes`/`repeat`/`pad`
- Correctness-lens filing of the unbounded-allocation family (`gen.rs:75`, `strings.rs:237`,
  `strings.rs:406`): only `n < 0` is rejected; `str::repeat` capacity-overflows or
  allocation-aborts, neither of which is a `ScalarError`, so the crash escapes the
  `fn(&[QueryValue]) -> ScalarResult` contract. Full write-up and fix at **6.2** (filed
  high by three of the five lenses that flagged it).

### 1.2 — medium — `datetime/parse` date-only fallback: invariant-`unwrap` on a user-parsed date
- File:line: `crates/shamir-funclib/src/datetime.rs:150-155` (the `unwrap` at `:153`).
- Issue: the fallback path does `NaiveDate::parse_from_str(s, pattern).map(|d|
  d.and_hms_opt(0, 0, 0).unwrap())` — an unchecked `unwrap` on untrusted input, violating
  the "panic only for programmer bugs" rule. **Resolution note (synthesis):** the two lens
  files disagreed on reachability — correctness filed it medium with a `-262144-01-01`
  panic scenario, security filed the same site a nit ("always Some"). `Cargo.lock` pins
  chrono **0.4.43**; since chrono 0.4.32 `NaiveDate`'s range is narrowed so midnight is
  representable for every valid date, so the `unwrap` is **currently infallible** — but the
  guarantee is version-coupled and undocumented, and nothing local pins it.
- Failure scenario: none at the pinned version; the panic risk returns if chrono's range
  semantics change or the date handling is reworked — with no test in the crate that would
  notice.
- Suggested fix: `and_hms_opt(0,0,0).ok_or_else(|| ScalarError::new("out_of_range"))?` (or
  at minimum an `expect` naming the chrono-range invariant, plus a boundary test). The
  module already models the right discipline one function over (`validate_pattern` +
  `format_with_malformed_pattern_returns_err_not_panic`); this site got no such treatment.

### 1.3 — low — F64→i64 range check is off by one ulp: silently saturates instead of `out_of_range`
- File:line: `crates/shamir-funclib/src/registry.rs:217-223` (`arg_i64`); same logic
  duplicated in `crates/shamir-funclib/src/cast.rs:120-126` (`cast_to_int`).
  *(Also flagged api-wire-protocol #7.)*
- Issue: the guard `*f <= i64::MAX as f64` accepts `f = 2^63`, because `i64::MAX as f64`
  rounds *up* to exactly 2^63; the saturating `*f as i64` then yields `i64::MAX` — a wrong
  value — where the honest answer is an error. The `Dec` arm right above does the honest
  thing (`to_i64()` → `None` → error), so the two numeric paths disagree at the same
  boundary.
- Failure scenario: `cast/to_int(F64(9223372036854775808.0))` returns
  `9223372036854775807` instead of `out_of_range`/`cast_failed` — a silent wrong-value
  conversion on a public conversion API and on every category funnelling through `arg_i64`.
  No suite tests the `i64::MAX as f64` edge today (though `compare_tests` covers the
  sibling Int↔Big precision bug).
- Suggested fix: bound with `*f < 9_223_372_036_854_775_808.0` (or a roundtrip check /
  route through the exact `Decimal` path); add the boundary test.

### 1.4 — low — *(primary: same as 6.5)* — `datetime/age`: unchecked subtraction, inconsistent with sibling `diff_secs`
- Correctness-lens filing of the `age` defect (`datetime.rs:78`): unchecked `i64`
  subtraction (panics in debug / wraps in release) and truncating division where `diff_secs`
  (`:266`) uses `checked_sub` + `div_floor` and has a dedicated overflow test
  (`datetime_tests.rs:177`). Full write-up at **6.5** (filed medium there).

### 1.5 — low — Two coexisting equality semantics: `==` (variant-strict) vs `compare` (value-strict)
- File:line: `crates/shamir-funclib/src/arrays.rs:85,99,170` (`contains`/`index_of`/
  `distinct`) vs `crates/shamir-funclib/src/null.rs:70` (`nullif`), `agg.rs:205`
  (`count_distinct`), `compare.rs:59-64`.
- Issue: the arrays family uses `QueryValue`'s variant-strict `PartialEq`/`Hash`
  (`Int(5) != Dec(5.0) != F64(5.0)`); everything else in the crate uses `compare`, under
  which those pairs are `Equal`. The ±0.0 case diverges three ways: `compare` says Equal,
  `Value::hash` (`shamir-types/src/types/value.rs:697-710`) canonicalizes NaN but *not*
  −0.0 (so `arrays/distinct([0.0, -0.0])` keeps both), while `canonical_hash` normalizes
  −0.0 (`canonical.rs:95-96`).
- Failure scenario: `arrays/contains([Dec(5.0)], Int(5))` → `false`, while
  `null/nullif(Int(5), Dec(5.0))` → `Null` and `count_distinct` counts the pair as 1 — the
  same "equality" answered three different ways; a CHECK constraint and a filter over the
  same data can disagree.
- Suggested fix: route `contains`/`index_of`/`distinct` through compare-equality (matching
  `count_distinct`) or document the split as deliberate and add cross-module parity tests;
  separately canonicalize −0.0 in `Value::hash` (shamir-types side, manifests here).

### 1.6 — low — `compare`'s documented "total (transitive)" order is not guaranteed on the lossy fallback paths
- File:line: `crates/shamir-funclib/src/compare.rs:3-6` (doc), `:101-102,124-127` (f64
  fallback).
- Issue: paths mixing exact comparisons (Int/Dec, Int/Big, Dec/Dec, Big/Big) with the
  lossy f64 conversion (Dec↔Big, anything×F64) can make `Equal` non-transitive: two
  adjacent huge `Dec`s compare `Less` exactly yet both compare `Equal` to a `Big` whose f64
  rounding collapses them. `compare_tests.rs` asserts transitivity only where it holds
  (Sets/Maps and a matrix not straddling the rounding boundary).
- Failure scenario: canonicalization guarantees phrased on top of `compare`
  (`compare_sets` doc) can mis-group near-boundary numeric sets; "equal" runs are not
  interchangeable across paths.
- Suggested fix: soften the top-level doc to "total up to documented f64-lossy
  approximation" (the `compare_numeric` doc already flags lossiness — the module doc
  overclaims), or implement an exact Dec↔Big path; add a boundary triple test.

### 1.7 — low — *(primary: same as 7.1)* — `scalar_resolver.rs` has zero in-crate tests
- Correctness-lens filing of the untested-resolver defect (whole module; no `tests/` dir).
  Four of seven lenses flagged it (correctness, concurrency, error-handling, style — the
  style lens at medium). Full write-up at **7.1**.

### 1.8 — low — `strings/substring` doc comment contradicts implemented (and tested) behavior
- File:line: `crates/shamir-funclib/src/strings.rs:129-130` (doc) vs
  `crates/shamir-funclib/src/tests/strings_tests.rs:73-81` (test pins the behavior).
- Issue: the comment says "Out-of-range start/negative args -> out_of_range", but the code
  only rejects negatives; `start` past the char count silently yields `""` — and the test
  *pins* that. Inconsistent with sibling `arrays/get`, which errors `out_of_range` past the
  end.
- Failure scenario: none at runtime — but the comment is the contract the next editor will
  "fix" the code toward (or vice versa), and the two modules diverge for the same
  conceptual mistake.
- Suggested fix: correct the comment to match the tested behavior (or change behavior +
  test to error, matching `arrays/get`), and state the cross-module indexing rule once.

### 1.9 — nit — Residual correctness nits (bundle)
- **Dead `any` field** on `BoolAndAgg`/`BoolOrAgg` kept alive by `let _ = self.any;`
  (`agg.rs:724,760`) — delete or use it (e.g. to distinguish empty input if
  SQL-NULL-on-empty is ever adopted).
- **`value_nav type_of` tests** cover only int/string/list/map/bool/null; the
  `f64`/`dec`/`big`/`bytes`/`set` variant names are unasserted.
- The bundle's other bullets were deduplicated into their primary findings: registry-test
  location → 7.4; flat-module test layout → 7.3; `validate/matches` cache/error-code drift
  → 6.6; module-header folder-prefix drift → 5.4; `canonical.rs` `serialise_key` → 3.8.

## 2. concurrency-lockfree

Structurally pillar-clean where it matters: registries are single-threaded `TFxMap` (Fx)
built once and shared as `&'static` via `OnceLock`; the user-scalar layer is
`scc::HashMap<String, FnEntry, THasher>`; the Argon2 gate's fast path is a lock-free CAS;
no scc O(N) `len()` anywhere (`ScalarRegistry::len` is IndexMap O(1);
`UserScalarLayer::is_empty` maps to early-exit `has_entry`). No lock held across `.await`
(the crate has no async), no `parking_lot`, no `RwLock`, no `unsafe`, no `static mut`.
Three real deviations below.

### 2.1 — high — Global `std::sync::Mutex` regex cache on the hot filter path — regex compiled *while holding the lock*
- File:line: `crates/shamir-funclib/src/strings.rs:417-434` (cache at `:418`, lock at
  `:423`, `Regex::new` at `:427`). *(Also flagged performance-hotpath #6, at medium, for
  the throughput cost.)*
- Issue: `compile()` backs all 8 regex scalars (`is_reg_match`, `reg_query`,
  `reg_query_all`, `reg_captures`, `reg_replace`, `reg_split`, `reg_count`,
  `reg_find_index` — `:262-367`), which run per-row inside filter expressions. Every call
  takes a **process-global `std::sync::Mutex<TFxMap<String, Regex>>`** (pillars 1 and 5
  violation), and on a cache miss `Regex::new(pat)` — expensive and driven by a
  user-supplied pattern — executes **inside the global critical section**. No inline
  contention-model comment, which CLAUDE.md requires for every hot-path `std::sync::Mutex`
  (the only comments cover poison-tolerance). Secondary: eviction is
  `if guard.len() >= 256 { guard.clear() }` (`:429-431`) — at 257 alternating patterns the
  whole cache wipes and the next 256 calls all recompile, under the same lock.
- Failure scenario: one connection runs `reg_query(col, '<pathologically complex
  pattern>')`; its compile takes ms–s *inside the global critical section*. Every regex
  scalar call from every connection/query/guest stalls behind it; throughput collapses to
  serialized regex access, and cache thrash makes recompiles a steady-state cost.
- Suggested fix: replace with `scc::HashMap<String, Regex, THasher>` (same primitive and
  pattern as `UserScalarLayer` next door): `read_sync` on hit, drop the guard, then
  `insert_sync` the freshly compiled `Regex` — compilation outside any container lock.
  Fixed literal patterns should be `LazyLock<Regex>` statics per `validate.rs`'s own
  convention. Make eviction incremental (or document the clear-all cliff). The same edit
  is the vehicle for fixing 4.2 (`validate/matches` recompiles).

### 2.2 — medium — `CountingSemaphore` lost wakeup: `release()` notifies without holding the mutex spanning the waiter's predicate check
- File:line: `crates/shamir-funclib/src/crypto.rs:132-148` (`acquire` `:132-143`,
  `release` `:145-148`), predicate `try_take` `:153-163`.
- Issue: the waiter holds `notify.0`'s mutex across `try_take` and hands it off inside
  `cvar.wait` — but `try_take` is lock-free on the atomic and `release()` performs
  `available.fetch_add(1, Release); notify_one()` **without ever acquiring that mutex**.
  std `Condvar` notifications are not remembered, so the canonical interleaving loses the
  wakeup: permits exhausted → W takes the mutex, `try_take` fails → R does `fetch_add`
  (available=1) then `notify_one()` (zero registered waiters → no-op) → W calls
  `cvar.wait` and sleeps with a permit sitting free. The mutex only helps if the notifier
  holds it while notifying.
- Failure scenario: under saturation a caller parks despite a free permit. Under continued
  traffic a later `release()` re-wakes it (transient latency spike); on a quiescent system
  the parked thread sleeps indefinitely — and per the crate's own dispatch model
  (`crypto.rs:102-110`) that thread is a tokio runtime worker, removed from the pool with
  no timeout and no way to observe it.
- Suggested fix: hold the mutex across the notify (`let _g = self.notify.0.lock();` before
  `notify_one()` in `release()`; the fast path stays lock-free), or restructure so
  predicate check and notify share the mutex (`wait_while`). The
  `argon2id_concurrency_cap_bounds_parallel_calls` test cannot catch this (fast path +
  clean queue only) — a targeted stress/loom-style test is warranted with the fix. Apply
  the same audit to `shamir_connect`'s mirroring `Argon2Semaphore`.

### 2.3 — medium — `argon2id` acquires a blocking semaphore inline on async runtime workers (documented residual risk, still open)
- File:line: `crates/shamir-funclib/src/crypto.rs:102-110` (tension note), `:111-112`
  (gate), `:221` (blocking `SemaphorePermit::acquire` inside the pure-`fn` scalar body).
  *(Also flagged security-crypto #7, at low, as the residual availability cost.)*
- Issue: the scalar contract is a synchronous `fn`, and the crate's own doc records that
  the engine dispatches it **inline on runtime workers** (`filter/resolve.rs`,
  `table/write_helpers.rs`, `validator/schema/field_rule.rs` — no `spawn_blocking`). So
  `acquire()` can park a worker on the condvar (pillar 2). The cap bounds memory but not
  the liveness cost: several workers can park simultaneously waiting for KDF calls that
  must themselves be scheduled to finish and release.
- Failure scenario: mixed workload where blocking-pool threads hold all 16 permits while
  connection-query workers pile up inside `acquire()`; each parked worker is a lost
  runtime thread for the duration, degrading every concurrent query on the runtime, not
  just the Argon2 ones. No deadlock (permit holders always make progress).
- Suggested fix: the module already names the remediation ("moving scalar dispatch onto
  `spawn_blocking` project-wide is a larger refactor flagged as follow-up") — track and
  land it; until then this stays consciously accepted debt but should not silently expire.
  Smaller in-crate mitigation: `try_acquire` + a caller-visible `throttled` error code so
  engines can defer instead of parking a worker.

### 2.4 — low — `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes with no locking"
- File:line: `crates/shamir-funclib/src/scalar_resolver.rs:45-47` (claims at `:25-26` and
  `:111`).
- Issue: scc's `get_sync` returns an `OccupiedEntry` holding the **bucket write lock**
  (scc's own docs: "use `read_sync` if read-only access is sufficient"); the lookup clones
  `FnEntry` under that exclusive lock. Per-key concurrent `ScalarResolver::call`s hashing
  to the same bucket serialize. The common `builtins_only()` path is unaffected (empty
  layer → null bucket array → miss before locking), so the fast-path *argument* survives
  while the stated mechanism ("lock-free read") is wrong.
- Failure scenario: a DB with several user scalars sees parallel filter evaluations
  contend on bucket writer locks for frequently-hit user functions — measurably worse
  than the documented zero-contention model.
- Suggested fix: one line — `self.fns.read_sync(name, |_, v| v.clone())`. The `FnEntry`
  clone is cheap (one `Arc` bump + POD fields).

### 2.5 — low — *(primary: same as 7.1)* — The module carrying the concurrency claims has no tests
- Concurrency-lens filing of the untested-resolver defect: nothing exercises concurrent
  `register`/`call` or the two-layer fallback, so the finding-2.4 fix or a shadowing
  refactor could regress silently. (By contrast the Argon2 cap is well covered,
  `crypto/tests/crypto_tests.rs:206-281`, barrier-hardened.) Full write-up at **7.1**.

### 2.6 — nit — *(primary: same as 6.7)* — `A2_IN_FLIGHT` observability counter is not panic-safe
- Concurrency-lens filing (nit) of the counter half of 6.7: `fetch_add`/`fetch_sub`
  bracket `hash_password_into` manually; a panic in between permanently inflates the
  in-flight counter (the permit itself is correctly RAII). Full write-up at **6.7**.

## 3. security-crypto

No auth/SCRAM/TLS surface here — this is the crypto-*primitive* boundary
(`sha256/sha512/sha3_256/blake3/hmac_sha256/ct_eq/argon2id` plus the `canonical_hash` CAS
integrity hash), and that core is in good shape: zero `unsafe` crate-wide, `subtle`
constant-time comparison, RFC 4231 / empty-string KATs, an argon2id
reference-identity test, capped KDF params and a process-wide concurrency semaphore with a
barrier-based regression test. The security findings are concentrated in the untrusted
input handling of the *sibling* scalar families sharing the same query/guest-reachable
dispatch path.

### 3.1 — high — *(primary: same as 6.1 — the crate's CRITICAL)* — `validate/is_json` recurses without a depth cap
- Security-lens filing (high) of the critical defect: `TextFormatParser::value` →
  `object`/`array` → `value` mutual recursion (`validate.rs:123-191`, entry `:87`,
  registered `:350-356`) with no depth limit — the bespoke parser silently dropped
  `serde_json`'s 128-level protection. Full write-up at **6.1**.

### 3.2 — high — *(primary: same as 6.2)* — Unbounded attacker-sized allocations → allocator abort
- Security-lens filing (high) of the unbounded-allocation family (`strings.rs:237,406`,
  `gen.rs:75`): the class `crypto.rs` capped for `argon2id` (`A2_MAX_LENGTH = 256`,
  `crypto.rs:59-62`) was never applied here; aborts bypass unwind-based per-connection
  isolation and kill the whole server. Full write-up at **6.2**.

### 3.3 — medium — *(primary: same as 4.2)* — `validate/matches` recompiles the user-supplied regex on every call
- Security-lens filing (medium, CPU-amplification framing): per-row NFA construction of an
  attacker-pattern is sustained CPU exhaustion by one cheap query; matching itself is
  ReDoS-safe (linear-time `regex` crate), the cost is compilation plus unbounded
  pattern-size parse work per call, with zero cache reuse even for an identical constant
  pattern. Full write-up at **4.2**; the `bad_pattern`-vs-`bad_regex` code drift aspect is
  covered at **6.6**.

### 3.4 — medium — *(primary: same as 6.4)* — Attacker-triggerable `rust_decimal` overflow panics in the numeric reductions
- Security-lens filing (medium) of the decimal-panic family: rust_decimal's ops panic on
  overflow (verified upstream, `src/arithmetic_impls.rs`), data-driven, one per accumulate
  call; `stddev`/`variance` overflow even with ≈1e15 inputs due to the squaring step. Full
  write-up at **6.4**.

### 3.5 — low — `value_nav`: plain `i64` addition on extremes (the sibling of 6.5's `age` defect)
- File:line: `crates/shamir-funclib/src/value_nav.rs:110` (`navigate`: `len + idx` for
  negative index `idx = i64::MIN`).
- Issue: untreated `i64` addition on attacker-chosen extremes (security #5 bundled this
  with `datetime/age`; the `age` half is deduplicated into **6.5**). In release,
  `value_nav` wraps to a negative that happens to be caught by the `resolved < 0`
  miss-check (no crash, wrong-but-harmless); in debug/test builds (overflow-checks on) it
  panics.
- Failure scenario: a path navigation with `idx = i64::MIN` panics under the test gate
  (flaky red builds); silently mis-navigates in release.
- Suggested fix: `len.checked_add(idx)` → miss on `None`.

### 3.6 — low — `crypto/ct_eq` length short-circuit leaks operand length (documented; acceptable for its intended MAC-tag use)
- File:line: `crates/shamir-funclib/src/crypto.rs:305` (`lhs.len() == rhs.len() &&
  bool::from(lhs.ct_eq(rhs))`).
- Issue: the `&&` short-circuit makes the comparison constant-time in *content* but
  timing-visible in *length*. The code comment acknowledges this; for the intended use
  (fixed-length HMAC tags) tag length is public and the leak is harmless. Flagged for the
  record so a future caller doesn't use `ct_eq` on variable-length secrets whose length is
  itself sensitive. Everything else about the path is correct (`Bin`-only via `arg_bytes`,
  genuine `subtle` CT compare).
- Failure scenario: none today. Hypothetical: `ct_eq(provided_token, stored_token)` with
  variable-length bearer tokens lets a local timing observer learn length equality.
- Suggested fix: none required; optionally document "length leaks by design — pad secrets
  to fixed length before comparing".

### 3.7 — low — *(primary: same as 2.3)* — `argon2id` blocking semaphore acquire inline on runtime workers
- Security-lens filing (low) of the documented residual risk: the cap (16 permits, ≤64
  MiB/call ⇒ ≤1 GiB worst case) and its barrier-saturated regression test are sound; the
  RAII permit releases on panic; this is the residual availability cost with the
  project-wide `spawn_blocking` refactor flagged as follow-up. Full write-up at **2.3**.

### 3.8 — low — `canonical.rs` key serialization swallows failure (`unwrap_or_default`) on an integrity-critical path
- File:line: `crates/shamir-funclib/src/canonical.rs:187` (`serialise_key`:
  `rmp_serde::to_vec(key).unwrap_or_default()`); `:192-196` (`key_is_prev_hash`:
  `unwrap_or(false)`). *(Also flagged error-handling-lifecycle #8 and a correctness nit
  bullet.)*
- Issue: a key that fails to serialise silently becomes the empty byte string — distinct
  keys would then collide in the sorted entry list, producing an identical `canonical_hash`
  for different records, silently weakening the optimistic-CAS integrity protocol (hash
  equality currently *is* the change detector; the module doc promises "any change to data
  changes the hash"). Unreachable today for the actual key types (`String`/interned `u64`
  serialise infallibly) — hardening, not a live bug. Related: `encode`/`compare` recurse
  over value nesting with no depth cap; they are safe only because the upstream
  codec/parser bounds nesting (`serde_json` at 128) — worth a comment pinning that
  assumption to the boundary that provides it.
- Failure scenario: hypothetical — a future key type with a fallible `Serialize` silently
  collapses distinct fields under the CAS hash.
- Suggested fix: return `Result<Vec<u8>, ScalarError>` from `serialise_key` (or
  `debug_assert!` + documented invariant); mirror the decision in `key_is_prev_hash`; add
  the upstream-depth-bound comment.

### 3.9 — nit — Robustness nits in parsers (bundle)
- **`TextFormatParser::eat`** (`validate.rs:115`): `self.b[self.i..]` slicing relies on the
  call-site invariant `i < len` (true today since `eat` is only called after `get(self.i)`
  returns `Some`, but one new call-site away from a panic) — make it local:
  `self.b.get(self.i..).is_some_and(|s| s.starts_with(lit))`.
- **`datetime.rs:153` `and_hms_opt(0,0,0).unwrap()`** — security filed this site as a nit
  ("always Some"); deduplicated into **1.2** (which carries the synthesis resolution: the
  pinned chrono 0.4.43 makes it currently infallible; harden anyway).

## 4. performance-hotpath

Hot paths largely clean: both registries are Fx-hashed `TFxMap`s with O(1) lookups;
`ScalarResolver::builtins_only()`'s empty-user-layer fast path is genuinely cheap (verified
against scc 3.8: one hash probe; no `scc::*::len()` calls anywhere); `arrays::distinct` was
already migrated to an O(N) `new_fx_set_wc` dedup with an old-vs-new bench
(`benches/distinct_arrays.rs`). The remaining asymptotic debt sits in the aggregate layer.

### 4.1 — high — `count_distinct` aggregator is O(N·C) — the exact legacy pattern `arrays::distinct` was fixed to remove
- File:line: `crates/shamir-funclib/src/agg.rs:197-215` (scan at `:202-205`).
- Issue: `CountDistinctAgg::accumulate` runs `self.seen.iter().any(|s| compare::compare(s,
  v) == Equal)` against a grow-only `Vec` for **every row** — O(N·C) compares (C = distinct
  count) plus one clone per distinct value. The module contract states "the engine calls
  `Aggregator::accumulate` for every row" (`agg.rs:5-7`), so this is a per-row hot path.
  The repo already diagnosed this precise shape and fixed it for the array scalar
  (`arrays.rs:150-178`; `benches/distinct_arrays.rs`), but the aggregate-layer twin was
  left on the legacy scan. It cannot adopt `arrays::distinct`'s `QueryValue` Fx set
  unchanged: the aggregate's equality is `compare`-based (cross-type), whereas
  `Hash`/`Eq`-based dedup counts `Int(5)` and `Dec(5.0)` separately.
- Failure scenario: `SELECT count(DISTINCT user_id) FROM big_table` degrades quadratically
  with table growth — a 1M-row all-distinct column costs ~5×10¹¹ compare calls; memory
  grows linearly with cardinality; latency balloons with no error surfaced.
- Suggested fix: mirror `ModeAgg` (`agg.rs:779-813`): buffer rows in `accumulate`, then at
  `finalize` one `sort_by(compare)` + adjacent run-length count — O(N log N) with identical
  compare-equality semantics. Add a `count_distinct` bench arm (the `distinct_arrays.rs`
  old/new pattern) and a scale test so the gate catches the regression. (Today neither the
  dedup paths in 4.1 nor 4.3 have any scale test or bench — `agg_tests.rs` exercises them
  only at toy sizes, so their cost is invisible to every gate in the repo.)

### 4.2 — high — `validate/matches` recompiles the regex on every call
- File:line: `crates/shamir-funclib/src/validate.rs:336-348` (`Regex::new` at `:342`).
  *(Also flagged security-crypto #3, at medium, as per-row CPU amplification.)*
- Issue: `matches` calls `Regex::new(pat)` per invocation. Compilation (NFA program build,
  many small allocations) is the expensive part, and `matches` is a predicate — in a
  filter or CHECK constraint it executes once per row, so a scan of N rows pays N
  compilations of the *same* pattern. The crate already contains both correct patterns:
  `/strings`' regex family routes through a pattern-keyed cache (`strings.rs:417-434`) and
  `/validate`'s own fixed validators are `LazyLock<Regex>` statics (`validate.rs:26-45`).
  Only `matches` misses both.
- Failure scenario: `WHERE validate/matches(note, '<multi-KB pattern>')` over 100k rows =
  100k full regex compilations (µs–ms each) — CPU and allocator churn that dwarfs the
  actual matching and serializes on the global allocator; sustained CPU exhaustion by a
  single cheap query.
- Suggested fix: promote `strings::compile` to a shared `pub(crate)` helper (small
  `regex_cache` module) and route `matches` through it — landing the 2.1 cache fix at the
  same time. The only behavioural difference is the error code (`bad_pattern` vs
  `bad_regex`, see 6.6); the cache stores only successfully-compiled regexes, so the
  invalid-pattern path is unaffected.

### 4.3 — medium — `DistinctWrapper` dedup is O(N·C) per wrapped aggregate
- File:line: `crates/shamir-funclib/src/agg.rs:227-261` (accumulate `:244-256`; O(n²) ack
  at `:223-226`).
- Issue: same linear-scan dedup as 4.1. The doc comment acknowledges the O(n²) worst case
  and argues "aggregate dedup is a bounded-cardinality cold path, not a per-row hot path" —
  but the wrapper wraps per-row aggregators (`sum(DISTINCT x)`,
  `string_agg(DISTINCT x, sep)`), so `accumulate` *is* per-row, and "bounded cardinality"
  is a property of the caller's data, not enforced by the code. Documented-accepted-debt
  site, but the rationale does not hold for high-cardinality columns and no bench/test
  pins the accepted cost.
- Failure scenario: `SELECT sum(DISTINCT order_id) ...` over a large high-cardinality
  column: per-row cost grows with the distinct set; latency degrades silently as data
  grows.
- Suggested fix: keep a sorted `seen: Vec<QueryValue>` and membership-test via
  `binary_search_by(|s| compare(s, v))` + ordered insert — O(log C) compares per row — or
  keep the linear scan but document (and optionally enforce) a hard cardinality ceiling
  above which the aggregate errors.

### 4.4 — medium — `stddev`/`variance` buffer the entire column though an O(1)-state algorithm exists
- File:line: `crates/shamir-funclib/src/agg.rs:448-478` (`StddevAgg`), `:484-517`
  (`VarianceAgg` + `compute_variance`).
- Issue: both push a `Decimal` per non-null row and only reduce at `finalize` — unbounded
  buffering that scales with input size, although population variance is computable in a
  single streaming pass (Welford: running mean + M2 + count). Median/percentile/mode/
  array_agg buffering is inherent to their algorithms; this one is not. At 16 B/row that is
  16 MB of pure scratch per aggregate instance per 1M-row group, and the two-pass
  `compute_variance` re-reads the whole buffer again at finalize.
- Failure scenario: wide group-bys (many groups × large groups) hold one full-column copy
  per (group, stddev/variance) instance simultaneously — memory blowup on
  aggregation-heavy dashboards, with no bound.
- Suggested fix: switch both to Welford's online algorithm. Flag in tests: last-decimal
  Decimal rounding differs from the buffered two-pass, so `agg_tests.rs` assertions need a
  tolerance or recompute.

### 4.5 — medium — *(primary: same as 6.2)* — `gen/random_bytes` attacker-controlled unbounded allocation
- Performance-lens filing (medium) of the unbounded-allocation family (`gen.rs:70-78`):
  the crate's own hardening precedent (`A2_MAX_MEMORY_KB`, "a single malicious call cannot
  pin 1 GiB") treats exactly this class seriously; `random_bytes` has no equivalent. Full
  write-up at **6.2**.

### 4.6 — medium — *(primary: same as 2.1)* — `/strings` regex cache: process-global `Mutex` on the per-row hot path
- Performance-lens filing (medium, throughput framing): N worker threads executing regex
  filters on different columns queue behind one mutex on every row; after 256 distinct
  patterns, all cached compilations are evicted together (thundering-herd recompiles).
  (`Regex` itself is Arc-backed, so the per-call clone is fine.) Full write-up at **2.1**.

### 4.7 — low — canonical `encode` re-serialises the reserved-key constant for every top-level map key
- File:line: `crates/shamir-funclib/src/canonical.rs:148-160` (check at `:156`),
  `:190-196` (`key_is_prev_hash`).
- Issue: in the `Value::Map` arm, `key_is_prev_hash(&key_bytes)` calls
  `rmp_serde::to_vec(PREV_HASH_FIELD)` **per key** — a fresh allocation plus serialisation
  of a compile-time constant inside the loop. `canonical_hash` runs on every sequenced
  write (CAS protocol), so this is repeated per-record-write waste proportional to record
  width. The per-key `serialise_key` allocation is inherent (each key differs); the
  constant is not.
- Failure scenario: none functionally — pure per-write CPU/allocation overhead.
- Suggested fix: hoist to `static PREV_HASH_KEY_BYTES: LazyLock<Vec<u8>>` (or compute once
  before the loop) and compare slices.

### 4.8 — nit — Stable sorts where the total order permits unstable (scratch allocation per call)
- File:line: `crates/shamir-funclib/src/agg.rs:435` (median), `:557` (percentile), `:792`
  (mode); `crates/shamir-funclib/src/arrays.rs:186,199` (sort/sort_desc).
- Issue: all five sites use `sort_by(compare::compare)`; the stable mergesort allocates a
  scratch buffer for non-small slices, while `compare` is documented as a *total* order, so
  `sort_unstable_by` is semantically identical and allocation-free. These fire on every
  call with the full input.
- Suggested fix: swap to `sort_unstable_by(compare)` at the five sites.

### 4.9 — nit — `cast/to_bool` allocates a lowercased copy of the input per call
- File:line: `crates/shamir-funclib/src/cast.rs:173`.
- Issue: `s.trim().to_ascii_lowercase().as_str()` heap-allocates on every predicate call
  just to compare against four literals.
- Suggested fix: match on `s.trim()` via `eq_ignore_ascii_case("true")` / `("1")` etc. —
  allocation-free.

### 4.10 — nit — `value_nav` int-step-into-map allocates a key `String` per step
- File:line: `crates/shamir-funclib/src/value_nav.rs:120`.
- Issue: the back-compat numeric-key path allocates `idx.to_string()` per path step per
  row. Path depths are small, so impact is marginal today.
- Suggested fix: format into a fixed 20-byte stack buffer, or leave as-is with a one-line
  comment naming the accepted cost.

## 5. api-wire-protocol

The public interface is coherent: one dispatch table over
`fn(&[QueryValue]) -> ScalarResult`, folder-qualified wire names, code-only machine-error
contract, explicit purity/trust metadata (`FnEntry`) consumed by the engine's
functional-index gate. Builder-only query-construction rule: **compliant** — the crate
constructs no queries; all `serde_json`/`rmp-serde` use is value-codec work, and the bench
uses `bench_scale_tool::Harness` per convention.

### 5.1 — high — *(primary: same as 6.2)* — Query-reachable scalars allocate unbounded memory
- API-lens filing (high) of the unbounded-allocation family (`gen.rs:70-77`,
  `strings.rs:228-242,389-411`), with the reachability evidence: all three are registered
  into `register_builtins` (`lib.rs:53,60`) and dispatched from filters
  (`shamir-engine/src/query/filter/resolve.rs:368`), schema field rules
  (`field_rule.rs:404`), and WASM guests (`shamir-wasm-host::builtin_scalars`).
  `SELECT gen/random_bytes(9223372036854775807)` (or `strings/repeat('a', 10^18)`)
  attempts a ~10¹⁸-byte allocation; Rust allocation failure aborts the process. Full
  write-up at **6.2**.

### 5.2 — medium — `canonical_hash` key ordering is msgpack-dependent but documented as name order; byte format has no version tag
- File:line: `crates/shamir-funclib/src/canonical.rs:26-37` (module doc), `:180-188`
  (`serialise_key` comment), `:161` (sort), `:199-219` (public API).
- Issue: the module doc says for string keys the sorted bytes "are the UTF-8 key name", and
  the inline comment claims "string keys order exactly as their names do". False:
  `rmp_serde::to_vec(key)` prepends a length tag (fixstr `0xa0|len` for len < 32; `0xd9`
  str8 above), so ordering is (length-class, length, bytes) — e.g. `"b"` encodes to
  `[0xa1,'b']` and sorts *before* `"aa"` (`[0xa2,'a','a']`) although `"aa" < "b"` by name.
  The hash stays deterministic and insertion-order independent (well covered by
  `canonical/tests`), but (a) the documented contract is wrong, and (b) any cross-language
  reimplementation of the CAS hash must replicate rmp-serde's exact encoding, which is
  specified nowhere. Additionally `canonical_bytes`/`canonical_hash` emit no magic/version
  prefix, and the format is implicitly coupled to codec behavior (Dec/Big hashed as
  `T_STR` because `Serialize` emits `to_string()`, `canonical.rs:52-58,102-116`; verified
  true today in `shamir-types/src/types/value.rs:72-73`).
- Failure scenario: a non-Rust client (or a future codec change — reactivating the
  reserved `0x04/0x05` tags or changing Dec serialization) computes hashes by name-sorting
  or with changed tags; stored `_prev_hash` chains then mismatch for logically identical
  records, and there is no version field to detect or migrate the format change.
- Suggested fix: (a) sort string keys by raw UTF-8 bytes (matching the documented
  contract), keeping msgpack encoding only for non-string keys — this changes hash
  outputs, so pair with (b); (b) prefix the canonical byte stream with a 1-byte format
  version and document the encoding as frozen.

### 5.3 — medium — *(primary: same as 6.6)* — Machine error-code vocabulary is free-form and inconsistent across categories
- API-lens filing (medium, wire-contract framing): the frontend localises by code
  (`registry.rs:8-9`), making the code set part of the wire contract, yet it exists only
  as scattered string literals with no single catalogue. Full write-up at **6.6** (which
  also covers the thiserror-shape and no-payload aspects).

### 5.4 — medium — Module docs advertise plain unqualified names; the wire protocol dispatches folder-qualified names
- File:line: `arrays.rs:1-6`, `cast.rs:3`, `crypto.rs:3-4`, `datetime.rs:4-8`,
  `encode.rs:3-6`, `math.rs:4-6`, `object.rs:3-4`, `strings.rs:6-10`, `text.rs:6-8`,
  `validate.rs:3-5`, `value_nav.rs:4-5` (all say "plain names, no folder prefix");
  `lib.rs:12-13` (stale "remaining categories are stubs"); correct in `gen.rs:3-4` and
  `null.rs:3-4`. *(Also flagged style-claude-md #9 as a nit and a correctness nit bullet.)*
- Issue: `register_builtins` (`lib.rs:49-66`) folder-qualifies every category, and the
  production wire contract uses `math/abs`-style names (confirmed in
  `shamir-query-types/src/read/select.rs:102`, the TS client builders, and
  `docs/guide-docs/guide/05-functions.md`). Eleven of thirteen category headers still claim
  plain names — an embedder reading `math.rs` would call `"abs"` and get
  `"unknown_function"`. Additionally, the per-category behavioural suites register modules
  *without* a folder and assert plain names (`math/tests/registry_tests.rs:21-25`,
  `tests/encode_tests.rs:6-10`, etc.), i.e. they exercise names that do not exist in the
  production registry; only `tests/register_builtins_tests.rs` (one sample per category)
  and the gen/null/canonical wiring tests cover the qualified spellings.
- Failure scenario: docs steer embedders into dead names; a future regression in
  `register`/`in_folder` prefixing for one category would not be caught by that category's
  behavioural suite.
- Suggested fix: update the 11 headers to folder-qualified names, delete the stale "stubs"
  sentence in `lib.rs` (see 7.2), and build each category's test registry via
  `in_folder("<cat>", <mod>::register)` so behavioural tests cover the production
  spelling.

### 5.5 — low — Same public name `get_path` in two folders with opposite miss semantics
- File:line: `crates/shamir-funclib/src/object.rs:94-121` vs
  `crates/shamir-funclib/src/value_nav.rs:26-37,98-140`.
- Issue: `object/get_path` errors `"missing_key"` on any miss and accepts only `Str`
  steps; `value_nav/get_path` returns `Null` on any miss, accepts `Int`/`Str` steps (with
  negative indexing), and errors `"type_mismatch"` only on a malformed step. `lib.rs:5-8`
  documents the folder mechanism as a collision fix but not that the colliding names are
  semantically different functions.
- Failure scenario: a query author picks the wrong namesake; the miss surfaces as a
  swallowed `"missing_key"` (the engine's `.ok()` silent-miss path at `resolve.rs:368`)
  instead of the expected `Null`, or vice versa — filters silently misbehave.
- Suggested fix: align the miss semantics (both Null or both error), or rename one;
  document the divergence at both definition sites regardless.

### 5.6 — low — `trusted_pure` gate: pub fields make the "explicit opt-in" convention-only; docs claim indexability the gate forbids
- File:line: `crates/shamir-funclib/src/registry.rs:54-65,94-104`; `arrays.rs:28`;
  `cast.rs:18-19`; consumer check at
  `shamir-engine/src/table/table_manager_index_mgmt.rs:250-262`.
- Issue: all `FnEntry` fields — including `trusted_pure` — are `pub`, so the documented
  "set via `.trusted_pure()`" vouch workflow is bypassable by struct literal; the gate's
  enforcement is only that `register_builtins` never vouches. Separately, the `arrays.rs`/
  `cast.rs` headers say their functions "(indexable)" / "may back a functional index", but
  every built-in is registered via `FnEntry::pure` (`trusted_pure = false`) and the engine
  rejects non-vouched entries — `is_indexable()` is false for the entire built-in library,
  and functional indexes are documented (engine side) as user-scalar-only.
- Failure scenario: an embedder follows the module doc, tries to back an index with
  `cast/to_int`, and gets a rejection whose message ("Call .trusted_pure() …") contradicts
  the doc they just read.
- Suggested fix: make the metadata fields private with the builder as sole setter (or at
  least `trusted_pure`), and reword the two headers to "pure + deterministic;
  functional-index use requires an explicit `.trusted_pure()` vouch".

### 5.7 — low — *(primary: same as 1.3)* — `f64 → i64` extraction accepts values above `i64::MAX` due to a float-rounded bound
- API-lens filing (low) of the one-ulp defect (`registry.rs:217-223`, duplicated
  `cast.rs:120-126`): `cast/to_int(9223372036854775808.0)` returns
  `Int(9223372036854775807)` instead of `"cast_failed"` — a silent wrong-value conversion
  on a public conversion API. Full write-up at **1.3**.

### 5.8 — nit — `ScalarError` has no structured detail slot
- File:line: `crates/shamir-funclib/src/registry.rs:17-29`.
- Issue: the code-only design is deliberate (localisation by code), but errors cannot carry
  *machine-safe* detail — which argument index failed, expected type, `[min,max]` arity —
  so clients cannot distinguish "arg 2 was bad" from "arg 0 was bad". Detail is not human
  text and would not violate the stated no-human-text contract.
- Suggested fix: add `pub detail: Option<ScalarErrorDetail>` (enum: `Arity { min, max }`,
  `ArgType { index, expected }`, …) while keeping `code` and `Display` unchanged.

### 5.9 — nit — `ScalarRegistry::register` collision policy undocumented
- File:line: `crates/shamir-funclib/src/registry.rs:126-136` vs `agg.rs:72-75`.
- Issue: `AggRegistry::register` documents "last-wins on collision";
  `ScalarRegistry::register` inserts silently with no stated policy. Given this registry
  just migrated away from plain-name collisions (#118), the overwrite policy should be
  explicit (note it genuinely *is* last-wins — IndexMap insert overwrites — unlike the
  scc-backed `UserScalarLayer`, see 6.3).
- Suggested fix: one doc line ("duplicate names: last-wins") plus a debug log on overwrite.

## 6. error-handling-lifecycle

`Result`/`ScalarError` discipline is largely exemplary for the documented
machine-code-only error model: `?` propagation uniform, panics rare and mostly
invariant-annotated, datetime defensively pre-validates strftime patterns to dodge a chrono
panic, the argon2id permit is proper RAII, and per-function error-code assertions
(`unwrap_err().code`) are pervasive across every `tests/` dir. The gaps are at the extremes
of the input space — and one of them is the crate's CRITICAL.

### 6.1 — **CRITICAL** — `validate/is_json` hand-rolled parser recurses without a depth limit — query-reachable stack overflow aborts the process
- File:line: `crates/shamir-funclib/src/validate.rs:123-191` (`TextFormatParser::value`
  `:123` → `object` `:136` / `array` `:169` → `value` mutual recursion), entry
  `is_text_encoded_str` at `:87`, registered at `:349-356`. *(Same root filed [HIGH] in
  security-crypto #1.)*
- Issue: `is_text_encoded_str` is a recursive-descent RFC 8259 validator with **no
  recursion-depth cap**: one stack frame per nesting level, bounded only by input length.
  Every other JSON path in the crate uses `serde_json` (default 128-level recursion limit,
  see `encode.rs:160` `parse_json`); this bespoke parser was written to avoid the
  dependency and silently dropped that protection (verified in source during synthesis: no
  depth counter exists anywhere in the parser).
- Failure scenario: `SELECT validate/is_json('[[[[[...')` with ~10⁵–10⁶ `[` characters
  (one unprivileged scalar call, ~10–20 KB of query text) exhausts the thread stack. A
  stack overflow is a hard `abort` — `catch_unwind` cannot intercept it, so the
  panic-isolation boundary documented in the root `Cargo.toml` (F-68 cluster C, #895: "one
  connection's panic can't take down another's") does not apply; the whole database
  process dies, every connection with it. Trivially repeatable (restart, one query, dead
  again) — a sustained remote DoS prosecuted by the cheapest principal in the system, live
  on every deployment shape (single-node included). Tests only cover shallow inputs
  (`validate/tests/validate_tests.rs:186-195`), so nothing in CI will surface it before a
  user does.
- Suggested fix: thread a `depth: usize` through `TextFormatParser` (increment in
  `value()`, return `false` past a sane limit such as serde_json's 128 — a named `const
  MAX_JSON_DEPTH`), and add a TDD red test with deep nesting (~100k levels) asserting
  `is_json` returns `false` (not a crash) for depth ≫ 128.

### 6.2 — high — Unbounded-allocation scalar paths: `strings/repeat`, `strings/pad_left`/`pad_right`, `gen/random_bytes`
- File:line: `crates/shamir-funclib/src/strings.rs:229-242` (`s.repeat(n as usize)` at
  `:237`), `strings.rs:406` (`std::iter::repeat_n(ch, target - cur)` in `pad`),
  `crates/shamir-funclib/src/gen.rs:70-78` (`vec![0u8; n as usize]` at `:75`).
  *(Same root filed high in security-crypto #2 and api-wire-protocol #1; medium in
  correctness #1 and performance #5 — five lenses, one defect.)*
- Issue: each takes an attacker-chosen `i64` length, rejects only negatives, and allocates
  it in one shot. Huge-but-valid values hit `String::repeat`'s `capacity overflow` panic or
  an allocator OOM-abort. All three are registered into `register_builtins`
  (`lib.rs:53,60`) and dispatched from filters, schema field rules, and WASM guests. The
  crate's own crypto module caps the analogous input (`A2_MAX_LENGTH = 256`,
  `A2_MAX_MEMORY_KB`, `crypto.rs:48-76`, with the documented rationale "a single malicious
  call cannot pin 1 GiB") precisely because these scalars are query/guest-reachable — the
  omission is an inconsistency, not a policy.
- Failure scenario: `SELECT gen/random_bytes(9223372036854775807)` (or
  `strings/repeat('a', 100000000000)`, or `strings/pad_left('x', 10^12, 'a')`) →
  allocation far beyond available RAM → `handle_alloc_error` → **abort**. Like the stack
  overflow in 6.1, an abort bypasses the unwind-based per-connection isolation and kills
  the whole server; sizes between `isize` capacity and RAM yield `capacity overflow`
  panics per call (repeated → sustained resource burn even where the unwind boundary
  holds). This family is what makes 6.1 a four-strong "one low-priv query kills the
  server" class rather than a single bug.
- Suggested fix: impose a per-call result-size ceiling per function (e.g. `random_bytes`
  ≤ 1 MiB, `repeat`/`pad` output ≤ ~2²⁰ chars — mirroring the argon2id style: `const
  MAX_RESULT_BYTES: i64`), enforced with `return Err(ScalarError::new("out_of_range"))`
  **before** allocating in all three paths; add cap-boundary tests (`n = i64::MAX` and
  just-above-cap) next to `random_bytes_negative_is_error` in `gen/tests/gen_tests.rs` and
  `tests/strings_tests.rs`.

### 6.3 — high — `UserScalarLayer::register` discards scc's `Err` — documented "(or replace)" silently never replaces
- File:line: `crates/shamir-funclib/src/scalar_resolver.rs:39-42` (verified in source:
  `let _ = self.fns.insert_sync(name.into(), entry);` under a doc saying "Register (or
  replace)").
- Issue: scc's `insert_sync` returns `Err((K, V))` when the key already exists — it does
  **not** overwrite — and the `let _` swallows it. The workspace's own code proves the
  semantics: `shamir-engine/src/repo/repo_instance.rs:1724` matches `if let Err((_,
  attempted)) = ...insert_sync(...)`, and `shamir-tx/src/tx_context.rs:731-733` explicitly
  comments that the `Err` on duplicate is being discarded (there deliberately idempotent —
  here silently wrong). Contrast `ScalarRegistry::register` (`registry.rs:129-136`,
  IndexMap-backed, genuinely last-wins) and `AggRegistry::register` (`agg.rs:72-75`,
  "last-wins on collision") — the registries disagree.
- Failure scenario: an embedder re-registers a user scalar to redefine it
  (`CREATE OR REPLACE FUNCTION`-style flow); scc returns `Err`, the `let _` eats it, and
  the **old** implementation keeps executing with no error, no log, no test failure (there
  are no tests — see 7.1).
- Suggested fix: honour the documented contract — on `Err((k, v))` fall through to
  `update_sync(&k, |_, e| *e = v)` (or `remove_sync` + re-`insert_sync`); if replace is
  not intended, fix the doc and comment the discard like `tx_context.rs` does. Either way,
  add a re-registration test pinning the chosen semantics (part of the 7.1 test set).

### 6.4 — medium — `rust_decimal` arithmetic panics on overflow in agg and array reductions
- File:line: `crates/shamir-funclib/src/agg.rs:286` (`SumAgg`: `self.acc += to_dec(v)?`),
  `agg.rs:322` (`AvgAgg`), `agg.rs:512-517` (`compute_variance`: `sum::<Decimal>()`,
  `(*x - mean) * (*x - mean)`, `sum_sq / n`), `agg.rs:852` (`RangeAgg`: `hi - lo`),
  `crates/shamir-funclib/src/arrays.rs:274-279` (`reduce` Sum/Avg: `acc +=` / `acc /=`).
  *(Also flagged security-crypto #4.)*
- Issue: rust_decimal's `Add`/`Sub`/`Mul`/`Div` impls are
  `checked_*(...).expect(...)` — they **panic** on overflow (independent of
  `overflow-checks` profile settings; verified upstream). `Decimal::MAX` is only ~7.92e28,
  so two large stored values suffice; these paths feed user-supplied `Dec` values straight
  into unchecked operators, so the panic escapes a pure scalar/`Aggregator` that returns
  `Result` — violating the crate's own `ScalarError` contract and differing from
  `math/pow`/`exp`, which carefully map non-finite results to `"domain"`.
- Failure scenario: `SELECT agg/sum(big_col)` where rows hold values near `Decimal::MAX` →
  "Addition overflowed" panic inside the aggregate; `stddev`/`variance` overflow even with
  modest (≈1e15) inputs because of the squaring step — easy to hit with ordinary
  financial-curve data. Unwinding contains it at the connection boundary (`panic =
  "unwind"`), but it defeats any profile that flips back to `abort` and is a trap if the
  panic ever crosses a WASM/FFI boundary.
- Suggested fix: switch these six sites to `checked_add`/`checked_sub`/`checked_mul`/
  `checked_div` (or `try_sum`) and map `None` to a stable error code (e.g.
  `ScalarError::new("overflow")`, consistent with the machine-codes-only convention); add
  an overflow regression test (e.g. `sum` of `[Dec(Decimal::MAX), Dec(Decimal::MAX)]`
  asserting the code).

### 6.5 — medium — `datetime/age` performs unchecked `i64` subtraction — inconsistent with the file's own checked discipline
- File:line: `crates/shamir-funclib/src/datetime.rs:76-79` (verified in source:
  `Ok(v_int((Utc::now().timestamp_millis() - then) / 1000))`). *(Also flagged
  correctness #4 (low) and the `age` half of security-crypto #5.)*
- Issue: `then` is an unvalidated `i64`; `now_ms - i64::MIN` overflows. Every sibling in
  the same file is checked (`from_epoch_s` `checked_mul`, `add_secs`/`add_days`
  `checked_add_signed`, `diff_secs` `checked_sub`), and
  `datetime/tests/datetime_tests.rs:177-188` explicitly asserts `diff_secs` "must return
  out_of_range, not panic" — `age` is the lone miss and has no such test. The `/1000` also
  truncates toward zero where the crate's documented convention is floor division
  (`div_floor`, `:312`), so `age` of a future timestamp rounds differently (−1 vs −2 s)
  than `diff_secs`.
- Failure scenario: `SELECT datetime/age(-9223372036854775808)` → debug builds (all lib
  tests) panic on overflow; release builds silently wrap, returning a garbage "age".
- Suggested fix: reuse `diff_secs`' shape: `now_ms.checked_sub(then).ok_or_else(||
  ScalarError::new("out_of_range"))?` then route through `div_floor` for the /1000; add
  the mirrored non-panic test.

### 6.6 — medium — `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context
- File:line: `crates/shamir-funclib/src/registry.rs:19-37` (struct); e.g.
  `strings.rs:427` `"bad_regex"` vs `validate.rs:342` `"bad_pattern"` for the same
  user-regex-compilation failure; `crypto.rs:212/229/289` `"bad_params"`/`"compute"`/
  `"bad_key"` appear in no module-doc error list. *(Also flagged api-wire-protocol #3,
  where the frontend-localisation framing lives, and a correctness nit bullet — which also
  notes `validate/matches` compiles per call while `strings` caches: two cache policies
  and two machine codes for one operation.)*
- Issue: per CLAUDE.md ("thiserror for library error enums") the natural shape would be a
  closed enum of codes; instead `ScalarError` is a `String`-holding struct and every site
  hand-types its code as an inline literal. The registry's own docs promise a stable,
  frontend-localisable code set, but the set is nowhere enumerated, codes for the *same*
  failure class have already drifted, and errors carry no payload (no arg index /
  offending type — see 5.8), so the frontend can only ever print a generic sentence. Every
  error allocates a fresh `String` on a library hot path where a `&'static str` would not.
- Failure scenario: a frontend keyed on `"bad_regex"` silently falls back to a generic
  error for `validate/matches` failures; telemetry/UX keyed to one code silently misses its
  twin; a typo in a future code (`"tyoe_mismatch"`) compiles and ships undetected.
- Suggested fix: keep the wire shape (code string) but back it with a
  `#[non_exhaustive] pub enum ScalarErrorCode` + thiserror `Display`, or at minimum a
  `pub mod codes` of `&'static str` constants used at every call site; dedupe
  `bad_regex`/`bad_pattern` consciously; document the full set in the registry module
  docs; add a registry-level test asserting every `ScalarError::new(...)` literal in the
  crate is a known code.

### 6.7 — low — Argon2id semaphore: poison-propagating `expect`s, and the in-flight counter leaks on panic
- File:line: `crates/shamir-funclib/src/crypto.rs:139` (`lock.lock().expect("semaphore
  mutex poisoned")`), `:141` (`cvar.wait(guard).expect(...)`), `:225-230` (manual
  `fetch_add`/`fetch_sub` pair). *(The counter half also flagged concurrency-lens as a
  nit.)*
- Issue: (a) the semaphore's critical section contains no panicking code, so poisoning is
  near-unreachable — but if it ever poisons, every subsequent `argon2id()` call on every
  connection panics forever, contradicting the poison-tolerant idiom the crate itself uses
  ten metres away (`strings.rs:418-423` `cache.lock().unwrap_or_else(|e| e.into_inner())`
  with an explanatory comment). (b) `A2_IN_FLIGHT.fetch_sub(1)` sits *after* the fallible
  KDF call without a guard: if `hash_password_into` ever panics (or the code between
  add/sub is later extended), the counter leaks upward permanently; the RAII permit is
  fine, the counter is not. (The `fetch_max(prev + 1)` peak logic itself is sound.)
- Failure scenario: mostly theoretical; (b) is a latent tripwire for future edits inside
  the add/sub window.
- Suggested fix: (a) adopt the `unwrap_or_else(|e| e.into_inner())` pattern with the same
  rationale comment; (b) fold the counter into the existing `SemaphorePermit` RAII (inc in
  `acquire`, dec in `Drop`) so it shares the permit's panic-safety.

### 6.8 — low — *(primary: same as 3.8)* — Canonical-hash path silently degrades serialization failures to empty key bytes
- Error-lens filing (low) of the `serialise_key` `unwrap_or_default()` hardening item
  (`canonical.rs:180-188`): latent CAS-integrity break if the generic is ever instantiated
  with a fallible `Serialize` key. Full write-up at **3.8** (security framing — the CAS
  hash is the change detector).

### 6.9 — low — *(primary: same as 7.1)* — `ScalarResolver` / `UserScalarLayer` have zero tests — their error paths are unverified
- Error-lens filing (low) of the untested-resolver defect: `unknown_function` from the
  fallback layer, shadowing priority, arity gating via `dispatch_entry`, and (directly
  enabling 6.3) re-registration over an existing name are all unverified — the
  replace-vs-insert contract bug shipped precisely because no test could go red. Full
  write-up at **7.1**.

### 6.10 — nit — Unannotated panicky/fallible primitives in library code (bundle)
- **`agg.rs:472`** (`StddevAgg::finalize`): `variance.to_f64().unwrap_or(f64::NAN)` masks a
  `Decimal→f64` conversion failure into NaN, which only *later* surfaces as
  `"out_of_range"` from `from_f64_retain` — correct outcome, needlessly obscured failure
  chain; map the `None` to `"out_of_range"` directly. The `unreachable!()`s in the
  reduce dispatch (`agg.rs:268`, `arrays.rs:268`) are invariant-shaped but unannotated.
- The bundle's datetime `.unwrap()` bullet is deduplicated into **1.2** (with the chrono
  0.4.43 resolution); suggested treatment there: `expect("0:0:0 is valid for any
  NaiveDate")` at minimum.

## 7. style-claude-md

Strong conformance: all 13 `mod.rs` files (every one under a `tests/` dir) are
re-export-only manifests; zero inline `#[cfg(test)] mod tests { ... }` blocks; 12 category
modules + `lib.rs` correctly carry `#[cfg(test)] mod tests;`; the bench uses the mandated
`bench_scale_tool::Harness` (no Criterion) and `Cargo.toml` sets `doctest = false`;
module doc comments are unusually thorough (crypto.rs's semaphore documentation is
exemplary); no `anyhow`, no `panic!`/`unwrap` on library paths beyond the invariant sites
already covered above.

### 7.1 — medium — `scalar_resolver` is the only real module with no tests and no `tests/` dir
- File:line: `crates/shamir-funclib/src/scalar_resolver.rs:1-145` (no `#[cfg(test)] mod
  tests;`; verified during synthesis: no `src/scalar_resolver/tests/` exists; zero matches
  for `ScalarResolver`/`UserScalarLayer` anywhere under `src/**/tests/`).
  *(Also flagged correctness #7, concurrency #5, and error-handling #9 — four lenses, one
  defect.)*
- Issue: every other functional module (agg, arrays, canonical, cast, compare, crypto,
  datetime, gen, math, null, text, validate, plus the lib root) follows the documented
  "one `tests/` directory per module" layout with topic-split coverage.
  `scalar_resolver.rs` — a public module consumed directly by shamir-engine/shamir-index/
  shamir-db on the hot filter path (2-layer user→builtin dispatch, `builtins_only()`
  OnceLock sharing) — has neither wiring nor tests in its home crate. Its core contract
  (user layer shadows builtins, fallback on miss, `unknown_function`, arity parity with
  `ScalarRegistry::call`, `get()` precedence) is exercised only indirectly via
  shamir-engine test fixtures (`resolver_with_user_scalar` helpers), not here — engine
  red tests assert engine behavior, not resolver semantics (nothing anywhere asserts a
  user scalar whose name collides with a builtin wins, or that `register` replaces).
- Failure scenario: a regression in dispatch precedence or in the shared `EMPTY` OnceLock
  (or the 6.3/2.4 fixes regressing) would not be caught by `./scripts/test.sh -p
  shamir-funclib`; the TDD protocol has no in-crate anchor for this module.
- Suggested fix: add `src/scalar_resolver/tests/scalar_resolver_tests.rs` (+ `mod.rs`
  manifest), wired via `#[cfg(test)] mod tests;`: user-shadows-builtin, builtin fallback,
  `unknown_function`, arity parity vs `ScalarRegistry::call`, `get()` precedence,
  `register`-replace semantics (pinning 6.3's chosen contract), `builtins_only()` sharing
  one `UserScalarLayer`/registry instance, and a multi-threaded register/call smoke test.

### 7.2 — low — Stale crate-level doc claims all non-math categories are "stubs"
- File:line: `crates/shamir-funclib/src/lib.rs:12-13` (verified in source).
- Issue: "`[`math`]` is the fully-implemented reference; the remaining categories are
  stubs to be populated by their owning agents." All remaining categories are fully
  implemented — 130+ registered functions across 13 categories, with per-category test
  suites (the `>= 130` assertion in `src/tests/register_builtins_tests.rs:17` confirms).
- Failure scenario: a reader trusting the crate's front-door doc may conclude categories
  are unimplemented, duplicate work, or distrust the module docs; comment-discipline cuts
  both ways — drifted comments mislead.
- Suggested fix: replace the sentence with the current state, e.g. "all categories are
  implemented; each module's header documents its function catalogue."

### 7.3 — low — Tests for five flat modules consolidated in `src/tests/` instead of per-module `tests/` dirs
- File:line: `src/tests/mod.rs:1-5`; host modules `src/encode.rs` (ends `:215`),
  `src/object.rs` (`:189`), `src/strings.rs` (`:435`), `src/value_nav.rs` (`:140`) carry no
  `#[cfg(test)] mod tests;`. *(Also a correctness nit bullet.)*
- Issue: CLAUDE.md mandates "one `tests/` directory per module", and the crate itself
  demonstrates the pattern 12 times. But `encode_tests.rs`, `object_tests.rs`,
  `strings_tests.rs`, and `value_nav_tests.rs` live in the crate-root `src/tests/` dir,
  and their host modules have no local test wiring — the association exists only through
  `lib.rs`'s root `mod tests`.
- Failure scenario: module tests are not discoverable next to the module they cover; a
  future split of `strings.rs` (e.g. extracting the regex family into a file per
  one-file-one-export) will not carry its tests along.
- Suggested fix: move each file to `<module>/tests/<module>_tests.rs` with a manifest-only
  `mod.rs`, and add `#[cfg(test)] mod tests;` to the four host modules.
  `register_builtins_tests.rs` legitimately stays under `src/tests/` — it tests the crate
  root.

### 7.4 — low — `registry` contract tests nested under `math/tests/`
- File:line: `src/math/tests/registry_tests.rs:1` (wired via `src/math/tests/mod.rs:2`).
  *(Also a correctness nit bullet.)*
- Issue: the file tests `crate::registry` (dispatch, arity, unknown-function, all `arg_*`
  extractors, all `v_*` constructors) — not the `/math` category — while `registry.rs`
  itself has no `#[cfg(test)] mod tests;`. Per the documented layout, these belong to the
  registry module's own `tests/` dir.
- Failure scenario: test history is mis-attributed: `git log -- src/registry*` shows no
  tests; a refactor of `math.rs` looks test-covered for registry behaviour it does not
  own; the "at least one test per registered function" pairing breaks.
- Suggested fix: move to `src/registry/tests/registry_tests.rs`, wire from `registry.rs`,
  drop the entry from `math/tests/mod.rs`.

### 7.5 — low — `use` statements inside function bodies
- File:line: `src/lib.rs:72` (`use std::sync::OnceLock;` inside `static_builtin()`),
  `src/scalar_resolver.rs:89` (inside `builtins_only()` — verified in source),
  `src/crypto/tests/crypto_tests.rs:208-209` and `:288`.
- Issue: CLAUDE.md: "All `use` statements live in the file header … never inside a
  function or block body," with three documented exceptions; none applies here. Hoisting
  is trivially safe in all four cases.
- Failure scenario: none functional; drift from a written rule, idiom inconsistent with
  the rest of the crate.
- Suggested fix: hoist `OnceLock` to the headers of `lib.rs` and `scalar_resolver.rs`;
  hoist the std/argon2 imports to the top of `crypto_tests.rs`.

### 7.6 — nit — Doc/code drift: `random()` documented as returning `F64`, actually `Dec`
- File:line: `src/gen.rs:15` (vs `src/registry.rs:303-310`, `v_f64` → `QueryValue::Dec`).
- Issue: the header says "`random()` takes 0 args and returns an `F64` in `[0.0, 1.0)`";
  the implementation routes through `v_f64`, which intentionally stores `QueryValue::Dec`
  (decimal-first value model), so the type a caller observes is `dec`.
- Suggested fix: correct the doc to "returns a `Dec` in [0, 1) (via `v_f64`)".

### 7.7 — nit — Doc drift: encode.rs header lists `str_escape_chars` as a registered function name
- File:line: `src/encode.rs:4-6` (registered name is `json_escape`, line `:139`).
- Issue: the header catalogue names `html_escape str_escape_chars` as registered
  functions, but `register()` registers `json_escape` for the `str_escape_chars`
  implementation (the conventions bullet at `:14` gets this right, contradicting the
  header above it).
- Suggested fix: update the header list to `… html_escape json_escape to_json parse_json`.

### 7.8 — nit — `scalar_resolver` doc references `builtin_scalars()` without a path
- File:line: `src/scalar_resolver.rs:3`.
- Issue: the backticked `builtin_scalars()` resolves to nothing in this crate — the
  in-crate function is `crate::static_builtin()`; a `pub fn builtin_scalars()` exists only
  in `shamir-wasm-host` (`crates/shamir-wasm-host/src/scalar.rs:20`).
- Suggested fix: reference `crate::static_builtin()` and note the embedder-facing alias,
  e.g. "`static_builtin()` (published to embedders as
  `shamir_wasm_host::builtin_scalars()`)".

### 7.9 — nit — *(primary: same as 5.4)* — Category-header phrasing drift about folder qualification
- Style-lens filing (nit) of the plain-name-headers defect: nine category headers say
  "plain names, no folder prefix" (true only at `register()` time) where gen/null
  document the qualification correctly. Full write-up at **5.4** (filed medium there for
  the doc-steers-embedders + test-coverage consequences).

### 7.10 — nit — Single-line JSON literals in tests
- File:line: `src/tests/encode_tests.rs:157`, `src/validate/tests/validate_tests.rs:186`.
- Issue: discipline rule: "In tests, JSON literals are always multi-line and indented for
  readability." Both sites use one-line raw strings. Payloads are short enough to read
  inline, but they contradict the letter of the rule.
- Suggested fix: reformat as multi-line indented literals, or amend the rule to exempt
  short inline payloads.

### 7.11 — nit — `registry.rs` and `agg.rs` stretch "one file = one primary export"
- File:line: `src/registry.rs:20-325` (`ScalarError`, `ScalarResult`, `ScalarFn`,
  `FnEntry`, `ScalarRegistry` + 16 public `arg_*`/`v_*` free fns), `src/agg.rs:45-856`
  (`Aggregator`, `AggFactory`, `AggRegistry`, `DistinctWrapper`, `percentile`,
  `string_agg` + ~18 private aggregator impls).
- Issue: CLAUDE.md allows a "closely-coupled group", and both qualify — one ABI + one
  registration path each; agg's per-aggregator structs are private. Still, the shared
  extractor/constructor half of `registry.rs` is a coherent standalone unit consumed by
  every category, and `registry_tests.rs` (7.4) already treats it as a distinct topic.
- Failure scenario: none today; file breadth makes blame and diffs noisier as categories
  grow.
- Suggested fix (optional): move the `arg_*`/`v_*` helpers into a sibling file
  (e.g. `registry/args.rs`) re-exported from `registry` so existing
  `use crate::registry::{…}` sites are unchanged; leave `agg.rs` as-is.

---

## Positive observations (carried from the lens files; no action)

- **Zero `unsafe`** anywhere in the crate (grep across `src/`, tests included).
- Crypto core tested against published vectors: SHA-256/512, SHA3-256, BLAKE3 KATs,
  HMAC-SHA256 RFC 4231 case 2 (`crypto_tests.rs:96-122`), argon2id bit-identity vs an
  independent reference + determinism, capped params (`A2_MAX_LENGTH`/`A2_MAX_MEMORY_KB`)
  and a process-wide counting semaphore with a barrier-hardened peak-in-flight regression
  test — the audit-trail hardening is real.
- `gen/uuid_v4`/`random_bytes` use `rand::rng()` (OsRng-seeded thread CSPRNG); `gen` fns
  correctly registered `pure:false, deterministic:false`, so they can never back a
  functional index.
- Regression tests that name the bug they pin exist for real past bugs: NaN Hash/Eq dedup,
  Int-vs-Big precision at `i64::MAX`, map/set canonicalization, chrono strftime panic
  conversion — plus an anti-vacuous `peak == cap` assertion in the argon2id cap test.
- scc pillar hygiene is real: `with_hasher(THasher::default())`, no banned O(N) `len()`,
  `is_empty` → early-exit `has_entry`; Fx pillar holds in arrays/object
  (`new_fx_set_wc`/`new_map_wc`).
- Canonical-hash serializer unusually well tested for order-independence and msgpack
  round-trip invariance (`canonical/tests/canonical_tests.rs:25-241`); agg empty-input
  table has ~60 tests; every module except `scalar_resolver` has a `tests/` dir per the
  layout convention.
- Builder-only rule compliant: the crate constructs no queries; bench uses
  `bench_scale_tool::Harness`.

## Finding counts

| Severity | Lens-tagged filings (as in the 7 files / workspace SUMMARY) | Distinct defects (after dedup) |
|---|---|---|
| critical | 1 | 1 |
| high | 8 | 5 |
| medium | 17 | 11 |
| low | 23 | 16 |
| nit | 15 | 14 |
| **total** | **64** | **47** |

Deduplicated defect census: **1 critical, 5 high, 11 medium, 16 low, 14 nit = 47 distinct
defects** (64 lens-tagged filings). Dedup groups (defects flagged by more than one lens,
listed once under the primary lens):

- **6.1 = 3.1** — `is_json` unbounded recursion (critical; security filed the same root
  high).
- **6.2 = 1.1 = 3.2 = 4.5 = 5.1** — unbounded allocations in `random_bytes`/`repeat`/
  `pad` (one defect, five lenses; high in three filings, medium in two).
- **4.2 = 3.3** — `validate/matches` per-call regex recompile (high; security filed the
  same root medium).
- **6.3** — `UserScalarLayer::register` scc-Err swallow (high, single filing).
- **2.1 = 4.6** — global `Mutex` regex cache, compile-under-lock (high; perf filed the
  same root medium).
- **4.1** — `count_distinct` O(N·C) (high, single filing).
- **7.1 = 1.7 = 2.5 = 6.9** — `scalar_resolver` untested (medium; three lenses filed it
  low).
- **6.4 = 3.4** — `rust_decimal` overflow panics (medium, both filings).
- **6.5 = 1.4** (+ the `age` half of security #5) — `datetime/age` unchecked subtraction
  (medium; correctness filed the same root low; security #5's other half is 3.5).
- **6.6 = 5.3** (+ a correctness nit bullet) — stringly error-code taxonomy
  drift (medium, both filings).
- **2.3 = 3.7** — argon2id blocking acquire on runtime workers (medium; security filed
  the same root low).
- **4.3** — `DistinctWrapper` O(N·C) (medium, single filing) · **4.4** — stddev/variance
  column buffering (medium, single filing) · **5.2** — canonical_hash ordering doc +
  no version tag (medium, single filing) · **5.4 = 7.9** (+ a correctness nit bullet) —
  plain-name module docs vs folder-qualified wire names (medium) · **1.2** — the
  `datetime.rs:153` invariant-unwrap (medium as filed; see the chrono 0.4.43 resolution
  note there; security #9 and error #10 filed the same site as nit bullets).
- Low/nit-level dedup groups: **1.3 = 5.7** (f64→i64 one-ulp) · **3.8 = 6.8** (+ a
  correctness nit bullet) (`serialise_key` swallow) · **6.7 = 2.6** (semaphore poison
  expects + in-flight counter) · **7.3 = 7.4** (each + a correctness nit bullet) (test
  layout).

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Cap the "one query kills the server" family at the scalar boundary.** (a) Thread a
   `MAX_JSON_DEPTH` counter through `TextFormatParser` (cap 128, return `false` beyond) in
   `validate.rs`; red test with ~100k nesting asserting `is_json` → `false`. (b) Add
   per-call output caps (`random_bytes` ≤ 1 MiB, `repeat`/`pad` ≤ ~2²⁰ chars) returning
   `"out_of_range"` **before** allocating, mirroring the `A2_MAX_*` pattern; boundary
   tests at `n = i64::MAX` and just-above-cap. Closes **6.1** (the CRITICAL, = 3.1) and
   **6.2** (= 1.1 / 3.2 / 4.5 / 5.1) — the workspace's headline issue and its
   three-flagged sibling in one program.
2. **Fix `UserScalarLayer::register` to actually replace** (`update_sync` on scc's
   `Err`), or fix the doc and comment the deliberate discard — plus the resolver test set
   (shadowing, fallback, `unknown_function`, arity parity, re-registration, concurrent
   register/call). Closes **6.3** and **7.1** (= 1.7 / 2.5 / 6.9); also pins **2.4**'s
   fix if `get_sync` → `read_sync` lands with it.
3. **Fix the semaphore lost wakeup**: hold `notify.0`'s mutex across `notify_one()` in
   `release()` (fast path stays CAS), and add a stress/loom-style test that the cap test's
   fast path can't cover. Closes **2.2** — a hang-class bug on a tokio worker, which the
   workspace rules classify as a defect, not a tuning issue.
4. **Replace the global regex cache with `scc::HashMap<String, Regex, THasher>`**
   (compile outside any container lock; incremental eviction) and route `validate/matches`
   through the shared helper. Closes **2.1** (= 4.6) and **4.2** (= 3.3) with one edit,
   and removes the "compiled while holding a process-global lock on the per-row filter
   path" exposure.

**P1 — soon**

5. **Kill the aggregate-layer quadratics**: `count_distinct` via buffer-then-
   `sort_by(compare)` + run-length (mirror `ModeAgg`); `DistinctWrapper` via sorted
   `seen` + `binary_search_by` membership (or a documented/enforced cardinality ceiling).
   Add bench arms + scale tests (today the O(N·C) cost is invisible to every gate).
   Closes **4.1**, **4.3**.
6. **`checked_*` arithmetic at all six decimal/i64 reduction sites** (sum/avg/variance/
   range aggregates, arrays sum/avg → `"overflow"`; `age` via `checked_sub` + `div_floor`
   → `"out_of_range"`; `value_nav` via `checked_add` → miss) with non-panic regression
   tests (`i64::MIN`, `Decimal::MAX` pairs). Closes **6.4** (= 3.4), **6.5** (= 1.4),
   **3.5**.
7. **`stddev`/`variance` → Welford** (O(1) state), with test-tolerance updates for
   last-decimal rounding. Closes **4.4**.
8. **Error-code catalogue**: `pub mod codes` (or `#[non_exhaustive] enum`), dedupe
   `bad_regex`/`bad_pattern`, document the set, add the every-literal-is-known test.
   Closes **6.6** (= 5.3); **5.8** (structured detail slot) can land in the same edit.
9. **Canonical-hash contract**: fix the key-ordering doc or sort string keys by raw
   UTF-8 (breaking — pair with a 1-byte format-version prefix + frozen-encoding doc);
   harden `serialise_key` (`Result`/`debug_assert!` instead of `unwrap_or_default`);
   hoist the `PREV_HASH_FIELD` constant out of the per-key loop; add the
   upstream-depth-bound comment. Closes **5.2**, **3.8** (= 6.8), **4.7**.
10. **Doc/wire-name alignment**: folder-qualified names in the 11 category headers,
    delete the stale "stubs" sentence in `lib.rs`, and build per-category test registries
    via `in_folder(...)` so suites exercise production spellings. Closes **5.4** (=
    7.9), **7.2**.
11. **Argon2id dispatch follow-up**: track and land the project-wide `spawn_blocking`
    (or at minimum `try_acquire` + `throttled` error) so worker parking is bounded.
    Closes **2.3** (= 3.7).
12. **f64→i64 boundary**: `*f < 9_223_372_036_854_775_808.0` (or the exact Decimal path)
    + boundary test. Closes **1.3** (= 5.7).

**P2 — backlog**

13. Equality semantics: route arrays family through compare-equality or document the
    split + parity tests; canonicalize −0.0 in `Value::hash` (shamir-types side).
    Closes **1.5**.
14. Soften `compare`'s total-order doc (or implement exact Dec↔Big) + boundary triple
    test. Closes **1.6**.
15. `get_path` miss-semantics alignment/renaming + divergence docs. Closes **5.5**.
16. `FnEntry` metadata privacy (builder-only setters) + indexability doc rewording.
    Closes **5.6**.
17. `ct_eq` length-leak doc note; `get_sync` → `read_sync` in `UserScalarLayer::get` (if
    not landed with P0-2). Closes **3.6**, **2.4**.
18. Semaphore poison-tolerant locks + RAII in-flight counter; `validate.rs::eat`
    get-based slicing; `and_hms_opt` `ok_or_else`/`expect` (currently infallible per the
    chrono 0.4.43 pin — hardening only); agg NaN-masking cleanup.
    Closes **6.7** (= 2.6), **3.9**, **1.2**, **6.10**.
19. Test-layout moves: flat-module tests to per-module `tests/`, registry tests out of
    `math/tests/`, hoist the four in-body `use`s. Closes **7.3**, **7.4**, **7.5**.
20. Perf nits: `sort_unstable_by` at five sites; allocation-free `to_bool`; stack-buffer
    key formatting in `value_nav`. Closes **4.8**, **4.9**, **4.10**.
21. Doc nits: `substring` comment, `random()` returns `Dec`, encode.rs header
    `json_escape`, `scalar_resolver` `builtin_scalars()` reference, `ScalarRegistry`
    collision policy, `ScalarError` detail slot (if not landed with P1-8), multi-line
    JSON literals, optional `registry/args.rs` split.
    Closes **1.8**, **7.6**, **7.7**, **7.8**, **5.9**, **5.8**, **7.10**, **7.11**,
    and the **1.9** bundle.

</details>
