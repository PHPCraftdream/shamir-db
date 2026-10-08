<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-funclib — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Both aggregate dedup scans, repeated regex compilation, global cache serialization, and variance buffering remain. Allocation nits are present, but the proposed blanket stable-to-unstable sort equivalence is refuted.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `count_distinct` aggregator is O(N·C) -- the exact legacy pattern `arrays::distinct` was fixed to remove

Status: `confirmed-open`. Current risk: `medium`.

Each non-null input still scans seen using compare and clones unseen values. All-unique input performs N(N-1)/2 comparisons; comparison cost can additionally depend on value size. Engine AggregateFn reaches count_distinct, whereas typed Count{distinct:true} is rejected. Existing tests are small and there is no aggregate scale bench. Sort-based replacement cannot promise identical semantics until non-transitive compare equality is resolved.

Evidence: [crates/shamir-funclib/src/agg.rs:202](../../../../../crates/shamir-funclib/src/agg.rs#L202); [crates/shamir-funclib/src/agg/tests/agg_tests.rs:68](../../../../../crates/shamir-funclib/src/agg/tests/agg_tests.rs#L68); [crates/shamir-funclib/benches/distinct_arrays.rs:75](../../../../../crates/shamir-funclib/benches/distinct_arrays.rs#L75); [crates/shamir-engine/src/query/read/aggregate.rs:861](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L861); [crates/shamir-engine/src/query/read/aggregate.rs:889](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L889).

<a id="review-2"></a>

### Claim 2 — `validate/matches` recompiles the regex on every call

Status: `confirmed-open`. Current risk: `medium`.

Regex::new remains per invocation rather than in a reusable pattern cache. Per-row callers therefore repeat compilation structurally. No timings or global-allocator serialization measurements support the report's numeric latency claims.

Evidence: [crates/shamir-funclib/src/validate.rs:342](../../../../../crates/shamir-funclib/src/validate.rs#L342); [crates/shamir-engine/src/query/filter/resolve.rs:374](../../../../../crates/shamir-engine/src/query/filter/resolve.rs#L374); [crates/shamir-funclib/src/validate/tests/validate_tests.rs:164](../../../../../crates/shamir-funclib/src/validate/tests/validate_tests.rs#L164).

<a id="review-3"></a>

### Claim 3 — `DistinctWrapper` dedup is O(N·C) per wrapped aggregate

Status: `confirmed-open`. Current risk: `medium`.

The wrapper still scans its growing seen Vec on every non-null row; its 'bounded-cardinality cold path' assertion has no enforced bound. Sorted Vec insertion still has O(C) movement and requires a valid order, so the proposed binary-search change is not a complete asymptotic repair. Preserve order-sensitive inner-aggregate semantics.

Evidence: [crates/shamir-funclib/src/agg.rs:223](../../../../../crates/shamir-funclib/src/agg.rs#L223); [crates/shamir-funclib/src/agg.rs:245](../../../../../crates/shamir-funclib/src/agg.rs#L245); [crates/shamir-engine/src/query/read/aggregate.rs:996](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L996); [crates/shamir-funclib/src/agg/tests/agg_tests.rs:555](../../../../../crates/shamir-funclib/src/agg/tests/agg_tests.rs#L555).

<a id="review-4"></a>

### Claim 4 — `stddev`/`variance` buffer the entire column though an O(1)-state algorithm exists

Status: `confirmed-open`. Current risk: `medium`.

Both still retain Vec<Decimal> and compute two-pass variance at finalize, establishing O(N) scratch per instance. Welford is not implemented. Any streaming replacement must specify checked arithmetic and Decimal rounding/accuracy; adjusting test tolerance alone does not establish equivalence.

Evidence: [crates/shamir-funclib/src/agg.rs:448](../../../../../crates/shamir-funclib/src/agg.rs#L448); [crates/shamir-funclib/src/agg.rs:467](../../../../../crates/shamir-funclib/src/agg.rs#L467); [crates/shamir-funclib/src/agg.rs:484](../../../../../crates/shamir-funclib/src/agg.rs#L484); [crates/shamir-funclib/src/agg.rs:512](../../../../../crates/shamir-funclib/src/agg.rs#L512).

<a id="review-5"></a>

### Claim 5 — `gen/random_bytes` performs an attacker-controlled unbounded allocation

Status: `confirmed-open`. Current risk: `high`.

The argument still has no positive upper bound before vec allocation. This is scalar-expression output amplification, not merely unmeasured performance debt.

Evidence: [crates/shamir-funclib/src/gen.rs:71](../../../../../crates/shamir-funclib/src/gen.rs#L71); [crates/shamir-funclib/src/gen.rs:75](../../../../../crates/shamir-funclib/src/gen.rs#L75).

Grouping/duplicate: `error-handling-lifecycle.md#2`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `/strings` regex cache: process-global `std::sync::Mutex` on the per-row hot path

Status: `confirmed-open`. Current risk: `medium`.

The mutex, compilation under guard, and clear-all eviction remain. Serialization/churn are source-proven; throughput impact is unmeasured. scc read_sync would be shared-bucket-locked rather than lock-free.

Evidence: [crates/shamir-funclib/src/strings.rs:418](../../../../../crates/shamir-funclib/src/strings.rs#L418); [crates/shamir-funclib/src/strings.rs:427](../../../../../crates/shamir-funclib/src/strings.rs#L427); [crates/shamir-funclib/src/strings.rs:429](../../../../../crates/shamir-funclib/src/strings.rs#L429); [Cargo.lock:3123](../../../../../Cargo.lock#L3123).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — canonical `encode` re-serialises the reserved-key constant for every top-level map key

Status: `confirmed-open`. Current risk: `low`.

key_is_prev_hash still serializes _prev_hash separately for each top-level key. Hoisting would remove repeated allocation/encoding. The claim that this runs on every sequenced production write is not established: the located CAS validator is an integration-test implementation.

Evidence: [crates/shamir-funclib/src/canonical.rs:156](../../../../../crates/shamir-funclib/src/canonical.rs#L156); [crates/shamir-funclib/src/canonical.rs:193](../../../../../crates/shamir-funclib/src/canonical.rs#L193); [crates/shamir-db/tests/cas_sequenced_e2e.rs:53](../../../../../crates/shamir-db/tests/cas_sequenced_e2e.rs#L53); [crates/shamir-db/tests/cas_sequenced_e2e.rs:90](../../../../../crates/shamir-db/tests/cas_sequenced_e2e.rs#L90).

<a id="review-8"></a>

### Claim 8 — Stable sorts where the total order permits unstable (scratch allocation per call)

Status: `refuted`. Current risk: —.

Stable sort sites remain, but the claimed semantics-identical switch is false. compare equates observably different variants such as Int(5) and Dec(5), so unstable reordering can change array order and the selected representative for median/percentile/mode. compare is also non-transitive at precision boundaries. Scratch allocation is an optimization consideration, not proof that stability is unnecessary.

Evidence: [crates/shamir-funclib/src/arrays.rs:186](../../../../../crates/shamir-funclib/src/arrays.rs#L186); [crates/shamir-funclib/src/arrays.rs:199](../../../../../crates/shamir-funclib/src/arrays.rs#L199); [crates/shamir-funclib/src/agg.rs:435](../../../../../crates/shamir-funclib/src/agg.rs#L435); [crates/shamir-funclib/src/agg.rs:557](../../../../../crates/shamir-funclib/src/agg.rs#L557); [crates/shamir-funclib/src/agg.rs:792](../../../../../crates/shamir-funclib/src/agg.rs#L792); [crates/shamir-funclib/src/compare.rs:119](../../../../../crates/shamir-funclib/src/compare.rs#L119).

<a id="review-9"></a>

### Claim 9 — `cast/to_bool` allocates a lowercased copy of the input per call

Status: `confirmed-open`. Current risk: `nit`.

The string branch still calls trim().to_ascii_lowercase(); allocation-free literal comparisons are not implemented. Latency impact was not measured.

Evidence: [crates/shamir-funclib/src/cast.rs:177](../../../../../crates/shamir-funclib/src/cast.rs#L177).

<a id="review-10"></a>

### Claim 10 — `value_nav` int-step-into-map allocates a key `String` per step

Status: `confirmed-open`. Current risk: `nit`.

The numeric map-step branch still allocates idx.to_string(). A stack formatter or explicit accepted-cost comment remains optional optimization; no significant measured cost is established.

Evidence: [crates/shamir-funclib/src/value_nav.rs:120](../../../../../crates/shamir-funclib/src/value_nav.rs#L120).

## Corrections and qualified non-findings

- No separate Fix Plan exists; recommendations are assessed in the finding rows and SUMMARY plan.
- The array hash-set migration establishes expected/amortized linear membership work for fixed-cost keys, not unconditional O(N) for arbitrary nested values or adversarial hash distributions.
- distinct_large_unique_matches_naive checks 500-element output correctness without comparing against a naive implementation, counting operations, or asserting a timing bound. It cannot detect reintroduction of quadratic complexity.
- The distinct_arrays benchmark is registered and compares current versus naive implementations, but benches were not run and do not constitute an ordinary behavioral-test complexity gate.
- Shared root test suites and registry tests under math contradict the claimed universally faithful per-module layout.
- A populated scc read_sync path is not lock-free. The unpopulated resolver path remains cheap, but neither its cost nor all claimed performance effects were benchmarked.
- Resolve compare's ordering/equivalence contract before replacing scan dedup with sorting or binary search; do not adopt blanket unstable sorting.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-funclib -- Performance & O(x->0)

## Summary

The crate's hot paths are largely clean: both registries are Fx-hashed `TFxMap`s with O(1) lookups, `ScalarResolver::builtins_only()`'s empty-user-layer fast path is genuinely cheap (verified against scc 3.8: `get_sync` on an empty map is one hash probe; no `scc::*::len()` calls exist anywhere in the crate), and `arrays::distinct` was already migrated to an O(N) `new_fx_set_wc` dedup with an old-vs-new bench (`benches/distinct_arrays.rs`). The remaining asymptotic debt sits in the aggregate layer: `count_distinct` and `DistinctWrapper` still run the exact legacy O(N·C) linear-scan dedup that `arrays::distinct` was fixed to eliminate, and `stddev`/`variance` buffer the entire column where an O(1)-state streaming algorithm exists. One per-row predicate (`validate/matches`) recompiles its regex on every call, and `gen/random_bytes` performs an attacker-controlled unbounded allocation.

## Findings

### 1. `count_distinct` aggregator is O(N·C) -- the exact legacy pattern `arrays::distinct` was fixed to remove
- **File:line:** `crates/shamir-funclib/src/agg.rs:197-215` (scan at 202-205)
- **Severity:** high
- **Issue:** `CountDistinctAgg::accumulate` runs `self.seen.iter().any(|s| compare::compare(s, v) == Equal)` against a grow-only `Vec` for **every row** -- O(N·C) compares (C = distinct count) plus one clone per distinct value. The module's own contract states "the engine calls `Aggregator::accumulate` for every row" (agg.rs:5-7), so this is a per-row hot path. The repo already diagnosed this precise shape and fixed it for the array scalar (`src/arrays.rs:150-178`; `benches/distinct_arrays.rs` documents the O(N²) pathology and the O(N) replacement), but the aggregate-layer twin was left on the legacy scan. Note it cannot simply adopt `arrays::distinct`'s `QueryValue` Fx set unchanged: the aggregate's equality is `compare`-based (cross-type: `Int(5)` == `Dec(5.0)`), whereas `Hash`/`Eq`-based dedup counts them separately.
- **Failure scenario:** `SELECT count(DISTINCT user_id) FROM big_table` degrades quadratically with table growth -- a 1M-row all-distinct column costs ~5x10^11 compare calls, while memory grows linearly with cardinality (cloned `Vec<QueryValue>`). Query latency balloons with no error surfaced.
- **Suggested fix:** mirror `ModeAgg` (agg.rs:779-813): buffer rows in `accumulate`, then at `finalize` do one `sort_by(compare)` + adjacent run-length count -- O(N log N) with identical compare-equality semantics preserved. Add a `count_distinct` arm to a bench (the `distinct_arrays.rs` old/new side-by-side pattern) and a scale test so the regression is caught by the gate.

### 2. `validate/matches` recompiles the regex on every call
- **File:line:** `crates/shamir-funclib/src/validate.rs:336-348` (`Regex::new` at 342)
- **Severity:** high
- **Issue:** `matches` calls `Regex::new(pat)` per invocation. Compilation (NFA program build, many small allocations) is the expensive part, and `matches` is a predicate -- in a filter or CHECK constraint it executes once per row, so a scan of N rows pays N compilations of the *same* pattern. The crate already contains both correct patterns: `/strings`' regex family routes through a pattern-keyed cache (`strings.rs:417-434`), and `/validate`'s own fixed validators are `LazyLock<Regex>` statics (validate.rs:26-45). Only `matches` misses both.
- **Failure scenario:** `WHERE validate/matches(note, '^...$')` over 100k rows = 100k full regex compilations (typically microseconds to milliseconds each) -- CPU and allocator churn that dwarfs the actual matching and serializes on the global allocator.
- **Suggested fix:** promote `strings.rs`'s `compile()` into a shared `pub(crate)` helper (e.g. a small `regex_cache` module) and route `matches` through it. The only behavioural difference is the error code (`bad_pattern` vs `bad_regex`); the cache stores only successfully-compiled regexes, so the invalid-pattern path is unaffected.

### 3. `DistinctWrapper` dedup is O(N·C) per wrapped aggregate
- **File:line:** `crates/shamir-funclib/src/agg.rs:227-261` (accumulate 244-256; O(n^2) ack at 223-226)
- **Severity:** medium
- **Issue:** Same linear-scan dedup as finding 1. The doc comment acknowledges the O(n²) worst case and argues "aggregate dedup is a bounded-cardinality cold path, not a per-row hot path" -- but the wrapper wraps per-row aggregators (`sum(DISTINCT x)`, `string_agg(DISTINCT x, sep)`), so `accumulate` *is* per-row, and "bounded cardinality" is a property of the caller's data, not enforced by the code. Unlike `CountDistinctAgg`'s removal candidate, this is a documented-accepted-debt site; per CLAUDE.md pillar 3's own ack convention, the rationale should hold -- it does not for high-cardinality columns, and no bench/test pins the accepted cost.
- **Failure scenario:** `SELECT sum(DISTINCT order_id) ...` over a large high-cardinality column: per-row cost grows with the distinct set; latency degrades silently as data grows.
- **Suggested fix:** keep a sorted `seen: Vec<QueryValue>` and membership-test via `binary_search_by(|s| compare(s, v))` + ordered insert -- O(log C) compares per row (the memmove stays O(C) worst-case, but the compare cost that dominates today collapses). Alternatively keep the linear scan but document (and optionally enforce) a hard cardinality ceiling above which the aggregate errors.

### 4. `stddev`/`variance` buffer the entire column though an O(1)-state algorithm exists
- **File:line:** `crates/shamir-funclib/src/agg.rs:448-478` (StddevAgg), `484-517` (VarianceAgg + `compute_variance`)
- **Severity:** medium
- **Issue:** Both aggregators push a `Decimal` per non-null row and only reduce at `finalize` -- unbounded buffering that scales with input size, although population variance is computable in a single streaming pass (Welford: running mean + M2 + count, O(1) state). Median/percentile/mode/array_agg buffering is inherent to their algorithms; this one is not. At 16 B/row this is 16 MB of pure scratch per aggregate instance per 1M-row group, and the two-pass `compute_variance` re-reads the whole buffer again at finalize.
- **Failure scenario:** wide group-bys (many groups x large groups) hold one full-column copy per (group, stddev/variance) instance simultaneously -- memory blowup on aggregation-heavy dashboards, with no bound.
- **Suggested fix:** switch both to Welford's online algorithm (O(1) state: mean, M2, count). Flag the change in tests: results differ in the last decimals of Decimal rounding versus the buffered two-pass computation, so `agg_tests.rs` assertions on those aggregates need a tolerance or recompute.

### 5. `gen/random_bytes` performs an attacker-controlled unbounded allocation
- **File:line:** `crates/shamir-funclib/src/gen.rs:70-78` (`vec![0u8; n as usize]` at 75)
- **Severity:** medium
- **Issue:** `random_bytes(n)` allocates `n` bytes with `n: i64` accepted up to `i64::MAX` and no ceiling. The function is a registered, query-reachable scalar, so any caller (including a WASM guest or filter expression) can request an exabyte-scale allocation. The crate's own hardening precedent treats exactly this class of input seriously: `crypto.rs:52-62` caps `argon2id` per-call memory (`A2_MAX_MEMORY_KB`) with a documented rationale that "a single malicious call cannot pin 1 GiB"; `random_bytes` has no equivalent guard.
- **Failure scenario:** `SELECT gen/random_bytes(9223372036854775807)` -- a single query triggers allocation failure, which aborts the entire server process (Rust aborts on failed `vec!` allocation), not just the offending query.
- **Suggested fix:** clamp `n` to a per-call ceiling (e.g. 1 MiB, in the style of the argon2id bounds) and return `ScalarError("out_of_range")` above it.

### 6. `/strings` regex cache: process-global `std::sync::Mutex` on the per-row hot path
- **File:line:** `crates/shamir-funclib/src/strings.rs:414-434` (`Mutex` at 418, clear-all eviction at 429-431)
- **Severity:** medium
- **Issue:** every regex-family scalar call (`is_reg_match`, `reg_query`, `reg_replace`, ...) locks one process-global `Mutex<TFxMap<String, Regex>>` before lookup. CLAUDE.md pillar 1 bans `std::sync::Mutex` in hot paths unless justified inline with a contention model; the only comments here cover poison-tolerance, not contention. These functions are per-row filter predicates, so concurrent query threads serialize on this single lock even when they use unrelated patterns. Secondary: the 256-entry bound is enforced by `guard.clear()` -- the whole cache is wiped at once, so the following rows recompile every pattern in a thundering-herd burst. (Overlaps the concurrency lens; listed here for its throughput cost. `Regex` itself is Arc-backed, so the per-call clone is fine.)
- **Failure scenario:** N worker threads executing regex filters on different columns queue behind one mutex on every row; after 256 distinct patterns accumulate, all cached compilations are evicted together and the next batch recompiles them all.
- **Suggested fix:** replace with `scc::HashMap<String, Regex, THasher>` using lock-free `read_sync`/`get_sync` lookups (per the workspace concurrent-map table), or an `ArcSwap` snapshot of an Fx map. Keep a bound but make eviction incremental (or document the clear-all cliff with its rationale).

### 7. canonical `encode` re-serialises the reserved-key constant for every top-level map key
- **File:line:** `crates/shamir-funclib/src/canonical.rs:148-160` (check at 156), `190-196` (`key_is_prev_hash`)
- **Severity:** low
- **Issue:** in the `Value::Map` arm, `key_is_prev_hash(&key_bytes)` calls `rmp_serde::to_vec(PREV_HASH_FIELD)` **per key** -- a fresh allocation plus serialisation of a compile-time constant inside the loop. `canonical_hash` runs on every sequenced write (CAS protocol, per the module docs), so this is repeated per-record-write waste proportional to record width. The per-key `serialise_key` allocation is inherent (each key differs); the constant is not.
- **Failure scenario:** none functionally -- pure per-write CPU/allocation overhead.
- **Suggested fix:** hoist to `static PREV_HASH_KEY_BYTES: LazyLock<Vec<u8>>` (or compute it once before the loop) and compare slices in the loop.

### 8. Stable sorts where the total order permits unstable (scratch allocation per call)
- **File:line:** `crates/shamir-funclib/src/agg.rs:435` (median), `557` (percentile), `792` (mode); `crates/shamir-funclib/src/arrays.rs:186, 199` (sort/sort_desc)
- **Severity:** nit
- **Issue:** all five sites use `sort_by(compare::compare)`; the stable mergesort allocates a scratch buffer for non-small slices, while `compare` is documented as a *total* order (compare.rs:1-10), so `sort_unstable_by` is semantically identical and allocation-free. These fire on every call with the full input.
- **Suggested fix:** swap to `sort_unstable_by(compare)` at the five sites.

### 9. `cast/to_bool` allocates a lowercased copy of the input per call
- **File:line:** `crates/shamir-funclib/src/cast.rs:173`
- **Severity:** nit
- **Issue:** `s.trim().to_ascii_lowercase().as_str()` heap-allocates on every predicate call just to compare against four literals.
- **Suggested fix:** match on `s.trim()` using `eq_ignore_ascii_case("true")` / `("1")` etc. -- allocation-free.

### 10. `value_nav` int-step-into-map allocates a key `String` per step
- **File:line:** `crates/shamir-funclib/src/value_nav.rs:120`
- **Severity:** nit
- **Issue:** the back-compat numeric-key path allocates `idx.to_string()` per path step per row. Path depths are small, so impact is marginal today.
- **Suggested fix:** format into a fixed 20-byte stack buffer, or leave as-is with a one-line comment naming the accepted cost.

## Test-coverage note (theme-relevant)

The crate follows the workspace `tests/` layout faithfully (one directory per module, `mod.rs` manifests), and `arrays::distinct` has both a scale test (`arrays_tests.rs:254-296`) and an old-vs-new bench (`benches/distinct_arrays.rs`) pinning its O(N) behaviour. The aggregate dedup paths (findings 1 and 3) have neither a scale test nor a bench arm -- `agg_tests.rs` exercises them only at toy sizes -- so their O(N·C) cost is currently invisible to every gate in the repo.

</details>
