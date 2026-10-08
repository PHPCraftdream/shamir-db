<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-types — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Signed-zero hashing, container materialization, epoch-adjacent classification, and macro parsing defects remain. Coverage claims need correction: upstream tests exercise scalar HAVING, owner escalation, and WasmCompiler permissions. for_each_field is quadratic but has no current engine caller.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 1 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `F64(0.0)` and `F64(-0.0)` compare equal but hash differently — Hash/Eq contract violated, and a test locks it in

Status: `confirmed-open`. Current risk: `high`.

PartialEq equates both zeros; Hash canonicalizes NaN only and hashes unequal zero bit patterns. The registered regression still asserts unequal hashes instead of testing equal-key membership/deduplication.

Evidence: [crates/shamir-types/src/types/value.rs:293](../../../../../crates/shamir-types/src/types/value.rs#L293); [crates/shamir-types/src/types/value.rs:697](../../../../../crates/shamir-types/src/types/value.rs#L697); [crates/shamir-types/src/types/tests/value_tests.rs:525](../../../../../crates/shamir-types/src/types/tests/value_tests.rs#L525); [crates/shamir-types/src/types/tests/mod.rs:8](../../../../../crates/shamir-types/src/types/tests/mod.rs#L8).

<a id="review-2"></a>

### Claim 2 — `HavingView::materialize_at` returns `Some(Null)` for containers, contradicting its own safety comment ("containers return None")

Status: `confirmed-open`. Current risk: `medium`.

The implementation still wraps container-to-Null conversion in Some, contradicting both its comment and the trait's owned-subtree contract. Contains' container fallback consequently receives Null and returns false; None would also return false there, so the original claimed difference between those two representations is not established for that caller.

Evidence: [crates/shamir-types/src/record_view/record_ref.rs:87](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L87); [crates/shamir-types/src/record_view/record_ref.rs:535](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L535); [crates/shamir-types/src/record_view/record_ref.rs:580](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L580); [crates/shamir-engine/src/query/filter/filter_node.rs:836](../../../../../crates/shamir-engine/src/query/filter/filter_node.rs#L836).

<a id="review-3"></a>

### Claim 3 — `RecordId::is_system()` misclassifies real records created within ~71 minutes after CUSTOM_EPOCH; the comment asserts this cannot happen

Status: `confirmed-open`. Current risk: `medium`.

Both timestamp constructors produce four leading zero bytes for relative times 0 through 2^32-1 microseconds; is_system accepts that prefix. Pre-epoch signed subtraction instead produces negative values, not zero. No production is_system caller was found; current misuse is an API risk, not demonstrated authorization bypass.

Evidence: [crates/shamir-types/src/types/record_id.rs:45](../../../../../crates/shamir-types/src/types/record_id.rs#L45); [crates/shamir-types/src/types/record_id.rs:67](../../../../../crates/shamir-types/src/types/record_id.rs#L67); [crates/shamir-types/src/types/record_id.rs:107](../../../../../crates/shamir-types/src/types/record_id.rs#L107); [crates/shamir-types/src/types/tests/record_id_tests.rs:131](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L131).

<a id="review-4"></a>

### Claim 4 — TDD gap: `HavingView` (a full public `RecordRef` impl, ~180 lines) has zero tests anywhere in the crate

Status: `confirmed-open`. Current risk: `low`.

No shamir-types test constructs HavingView or targets its container, nested-path, or unknown-key behavior. However, registered engine HAVING tests call apply_group_by, which constructs HavingView; blanket workspace-wide zero coverage is false.

Evidence: [crates/shamir-types/src/record_view/tests/mod.rs:9](../../../../../crates/shamir-types/src/record_view/tests/mod.rs#L9); [crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs:745](../../../../../crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs#L745); [crates/shamir-engine/src/query/read/tests/exec_tests.rs:323](../../../../../crates/shamir-engine/src/query/read/tests/exec_tests.rs#L323); [crates/shamir-engine/src/query/read/aggregate.rs:1308](../../../../../crates/shamir-engine/src/query/read/aggregate.rs#L1308); [crates/shamir-engine/src/query/read/tests/mod.rs:6](../../../../../crates/shamir-engine/src/query/read/tests/mod.rs#L6).

<a id="review-5"></a>

### Claim 5 — `RecordRef for RecordView::for_each_field` is O(fields²) — each field re-scans the whole map body

Status: `confirmed-open`. Current risk: `low`.

Every iterated field invokes materialize_at, restarting value_bytes_at at offset zero. Quadratic scanning is source-proven; current engine source has no for_each_field call, so the alleged current SELECT * regression is unsupported.

Evidence: [crates/shamir-types/src/record_view/record_ref.rs:338](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L338); [crates/shamir-types/src/record_view/lens.rs:1139](../../../../../crates/shamir-types/src/record_view/lens.rs#L1139); [crates/shamir-types/src/record_view/tests/record_ref_tests.rs:969](../../../../../crates/shamir-types/src/record_view/tests/record_ref_tests.rs#L969).

Grouping/duplicate: `performance-hotpath.md:2`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — `mpack!` rejects negative (or any multi-token) literals as OBJECT values — asymmetric with arrays/lists, uncovered by tests

Status: `confirmed-open`. Current risk: `low`.

Object scalar arms consume one token tree and have no unary-minus value arm; array accumulation forwards the complete negative token sequence. Existing negative tests are top-level, not negative object values. Arbitrary expressions require the documented @ escape; that broader restriction is intentional.

Evidence: [crates/shamir-types/src/macros/mpack.rs:94](../../../../../crates/shamir-types/src/macros/mpack.rs#L94); [crates/shamir-types/src/macros/mpack.rs:137](../../../../../crates/shamir-types/src/macros/mpack.rs#L137); [crates/shamir-types/src/macros/mpack.rs:249](../../../../../crates/shamir-types/src/macros/mpack.rs#L249); [crates/shamir-types/src/macros/tests/mpack_tests.rs:33](../../../../../crates/shamir-types/src/macros/tests/mpack_tests.rs#L33).

<a id="review-7"></a>

### Claim 7 — Untested public surface with documented contracts: `Interner::generation`, `ResourceMeta::owner_field`, `principal64_from_username`, `SecretString::into_inner`, `ResourcePath::WasmCompiler`

Status: `partially-fixed`. Current risk: `low`.

Direct types tests remain absent for these five symbols. Upstream registered tests discriminate missing versus explicit System ownership and WasmCompiler permission changes. generation is an allocation high-water mark, not a counter incremented on every successful touch: existing-name touches and touch_with_id filling lower-id holes can leave it unchanged. Username-hash tests would not prove collision resistance; into_inner handoff remains untested.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:157](../../../../../crates/shamir-types/src/core/interner/interner.rs#L157); [crates/shamir-types/src/core/interner/interner.rs:303](../../../../../crates/shamir-types/src/core/interner/interner.rs#L303); [crates/shamir-types/src/core/interner/interner.rs:449](../../../../../crates/shamir-types/src/core/interner/interner.rs#L449); [crates/shamir-types/src/tests/secret_tests.rs:1](../../../../../crates/shamir-types/src/tests/secret_tests.rs#L1); [crates/shamir-db/src/shamir_db/tests/enforcement_tests.rs:714](../../../../../crates/shamir-db/src/shamir_db/tests/enforcement_tests.rs#L714); [crates/shamir-db/src/shamir_db/tests/enforcement_tests.rs:762](../../../../../crates/shamir-db/src/shamir_db/tests/enforcement_tests.rs#L762); [crates/shamir-db/src/shamir_db/tests/wasm_compiler_permission_tests.rs:65](../../../../../crates/shamir-db/src/shamir_db/tests/wasm_compiler_permission_tests.rs#L65); [crates/shamir-db/src/shamir_db/tests/mod.rs:26](../../../../../crates/shamir-db/src/shamir_db/tests/mod.rs#L26).

<a id="review-8"></a>

### Claim 8 — `touch_ind` racing `touch_with_id` on the same id is guarded only by `debug_assert!` — silent in release

Status: `confirmed-open`. Current risk: `low`.

A failed OnceLock::set is still guarded only by debug_assert, while the forward mapping remains inserted. The public cross-API interleaving can cause permanent divergence. Deployment reachability is conditional; touch_with_id also has a live drainer caller, so describing all callers as startup recovery is inaccurate.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:169](../../../../../crates/shamir-types/src/core/interner/interner.rs#L169); [crates/shamir-types/src/core/interner/interner.rs:223](../../../../../crates/shamir-types/src/core/interner/interner.rs#L223); [crates/shamir-types/src/core/interner/interner.rs:449](../../../../../crates/shamir-types/src/core/interner/interner.rs#L449); [crates/shamir-engine/src/tx/drainer.rs:1116](../../../../../crates/shamir-engine/src/tx/drainer.rs#L1116).

<a id="review-9"></a>

### Claim 9 — Nit: vacuous self-comparison assertion in `from_ts_produces_unique_ids_with_same_timestamp`

Status: `confirmed-open`. Current risk: `nit`.

The assertion compares two calls to the same timestamp encoder and cannot distinguish a consistently wrong timestamp layout. Separate layout and uniqueness assertions provide real coverage; the whole test is not vacuous.

Evidence: [crates/shamir-types/src/types/tests/record_id_tests.rs:107](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L107); [crates/shamir-types/src/types/tests/record_id_tests.rs:114](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L114); [crates/shamir-types/src/types/tests/record_id_tests.rs:135](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L135).

<a id="review-10"></a>

### Claim 10 — Nit: duplicated public `CodecError` name, malformed doctests, dead error variant, misnamed error variant

Status: `confirmed-open`. Current risk: `nit`.

Both error enums remain; bincode examples contain stale paths and repeated derive text; PathNotFound has no production constructor; read_str_len still uses NonBinKey. Corrections: examples are unfenced prose, not executable doctests, and public string convenience methods discard the private NonBinKey error into None.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:8](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L8); [crates/shamir-types/src/codecs/basic/bincode.rs:26](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L26); [crates/shamir-types/src/codecs/error.rs:4](../../../../../crates/shamir-types/src/codecs/error.rs#L4); [crates/shamir-types/src/types/value_error.rs:21](../../../../../crates/shamir-types/src/types/value_error.rs#L21); [crates/shamir-types/src/types/tests/value_api_tests.rs:265](../../../../../crates/shamir-types/src/types/tests/value_api_tests.rs#L265); [crates/shamir-types/src/record_view/lens.rs:196](../../../../../crates/shamir-types/src/record_view/lens.rs#L196).

## Corrections and qualified non-findings

- The signed-zero fix must change the incorrect assertion, not necessarily delete its test.
- Signed i64::saturating_sub saturates only on integer overflow. Ordinary pre-epoch timestamps encode negatively; SUMMARY's purported source verification of clamping to zero is wrong.
- HavingView::new looks up existing ids with get_ind; it does not intern row keys. Unknown keys are omitted from its index.
- Returning None for existing containers would match the local comment but still not satisfy RecordRef's documented subtree-materialization contract; choose semantics against actual consumers.
- The cited upstream owner and WasmCompiler tests already existed before the review; their presence is not evidence of a later fix.
- Registered parity, merge, and depth tests have meaningful assertions, but their existence does not prove the universal malformed-input or byte-parity guarantees.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-types -- Correctness & TDD-coverage

## Summary
The crate is generally well-tested — the interner (#501 race regressions), the msgpack lens (byte-level parity, error/untrusted-input batteries), `merge_storage_bytes` byte-identity against a reference merge, and FG-1 u64 promotion are genuinely load-bearing Red/Green suites. However, this review found one real invariant violation enshrined by its own test (`F64(±0.0)` hashing vs `PartialEq`), a doc/code contradiction with silent falsification in `HavingView`'s container path, an epoch-boundary hole in `RecordId::is_system` that its own comment claims is impossible, and several public surfaces (`HavingView` entirely, plus `generation()`, `owner_field`, `principal64_from_username`, `SecretString::into_inner`, `WasmCompiler`) with zero tests despite documented behavioral contracts. Two structural invariants from CLAUDE.md are also strained: `RecordView::for_each_field` hides an O(N²) rescan, and the `mpack!` macro silently fails to compile for negative literals as object values (works in lists).

## Findings

### 1. `F64(0.0)` and `F64(-0.0)` compare equal but hash differently — Hash/Eq contract violated, and a test locks it in
- **File:line:** crates/shamir-types/src/types/value.rs:697-711 (Hash impl), :293-299 (PartialEq); src/types/tests/value_tests.rs:525-530
- **Severity:** high
- **Issue:** `Value::F64` `PartialEq` uses plain `f64` equality, so `Value::F64(0.0) == Value::F64(-0.0)` is `true`. The `Hash` impl hashes `f.to_bits()` for non-NaN floats; `0.0` hashes as `0x0000000000000000` and `-0.0` as `0x8000000000000000`. This breaks the `k1 == k2 ⇒ hash(k1) == hash(k2)` contract — the very contract the NaN branch (added for the audit 2026-07-06 `distinct()` regression) cites and fixes for NaN, while missing ±0.0. Worse, `test_f64_neg_zero_hash` **asserts the hashes differ**, turning the bug into codified behavior; any fix will have to delete the test (this is exactly a Red/Green inversion: the suite is green on wrong semantics).
- **Failure scenario:** `QueryValue::Set(TSet<Value<String>>)` keyed on floats (DISTINCT / IN-list dedup / set-membership): inserting `0.0` then `-0.0` yields two elements that compare `==`; membership lookup of one misses the other's bucket, so `Set([0.0]) == Set([-0.0])` evaluates `false` element-wise-equal sets and dedup counts ±0.0 twice.
- **Suggested fix:** Canonicalize ±0.0 in `Hash` exactly like NaN (e.g. hash `f + 0.0`'s bits, or map `-0.0 → 0.0` before `to_bits()`), and flip `test_f64_neg_zero_hash` to assert hash equality per the Eq contract.

### 2. `HavingView::materialize_at` returns `Some(Null)` for containers, contradicting its own safety comment ("containers return None")
- **File:line:** crates/shamir-types/src/record_view/record_ref.rs:514-539 (impl), :559-582 (`query_value_to_inner_value`)
- **Severity:** medium
- **Issue:** The comment block states containers return `None` so the filter's InSet/Contains gets the "no match" fallback, but the code unconditionally returns `Some(query_value_to_inner_value(val))`, which collapses `List`/`Set`/`Map` leaves to `InnerValue::Null`.
- **Failure scenario:** A HAVING predicate probing a container/aggregate output (`InSet`/`Contains` over an aggregate list result) receives `InnerValue::Null` instead of "no leaf": depending on how the node consumes it, the predicate silently evaluates as no-match-with-a-value rather than the intended skip, and can never be distinguished from a genuine Null aggregate — an observable correctness divergence hidden behind a comment claiming it cannot happen.
- **Suggested fix:** Either honor the comment (`None` for non-scalar leaves via a type check before conversion) or fix the comment and add a test pinning the chosen semantic.

### 3. `RecordId::is_system()` misclassifies real records created within ~71 minutes after CUSTOM_EPOCH; the comment asserts this cannot happen
- **File:line:** crates/shamir-types/src/types/record_id.rs:16-18 (SYSTEM_RECORD_PREFIX rationale), :41-54 (`from_ts`), :95-109 (`system`/`is_system`)
- **Severity:** medium
- **Issue:** `is_system()` tests "first 4 bytes zero". User ids put `(timestamp_micros − CUSTOM_EPOCH_MICROS).to_be_bytes()` there, whose top 4 bytes are zero for any relative time < 2³² µs ≈ 71.6 min after 2026-01-31T00:00Z (and all pre-epoch times saturate to negative → 0xFF…, different hazard but also outside the model). The constant's doc claims "a real timestamp will never be zero" — false at the epoch boundary. No test pins the boundary (all tests use "now", 6+ months past the window).
- **Failure scenario:** Any record minted in the first ~72 minutes after the custom epoch (cold-restore replaying old timestamps, clock set near the epoch, or a test using fixed wall-clock ≈ epoch) collides with the system-record id convention; downstream code branching on `is_system()` treats user data as system metadata.
- **Suggested fix:** Either reserve a distinguishing bit/pattern beyond "4 zero bytes" (e.g. require bytes[4] != 0 only when combined with a range check), or clamp/validate `from_ts` inputs to be > epoch + 2³² µs and document/test the boundary.
- **Residual note:** the deliberate 12-byte truncation collision of `system()` names ("123456789012-extra" == "123456789012", asserted by `test_system_record_id_logic`) has no name-collision guard at construction — fine if callers never pass colliding prefixes, but nothing enforces it.

### 4. TDD gap: `HavingView` (a full public `RecordRef` impl, ~180 lines) has zero tests anywhere in the crate
- **File:line:** crates/shamir-types/src/record_view/record_ref.rs:354-557; no matches for `HavingView|having` under src/**/tests/
- **Severity:** medium
- **Issue:** The grep across every `tests/` directory finds no test constructing a `HavingView`. Its distinctive behaviors are exactly where bugs hide: single-segment-only resolution (multi-segment paths silently return `None` = "predicate does not match"), key built by interning row keys at construction (unknown keys silently dropped from `key_index`), `for_each_field` materializing through `query_value_to_inner_value`, and finding #2 above. CLAUDE.md's Red/Green protocol presumes failing-first coverage for behaviors someone depends on; this impl shipped without any.
- **Failure scenario:** Future refactors of `key_index`/descend logic break HAVING evaluation with no test signal; the multi-segment `None` and unknown-key drop already define untested, possibly unintended semantics.
- **Suggested fix:** Add a `record_view/tests/having_view_tests.rs`: flat-row scalar_at hit/miss, unknown-key drop, multi-segment None, any_seq_elem List/Set paths, materialize_at scalar vs container (pinning the decision from finding #2), to_query_value clone identity.

### 5. `RecordRef for RecordView::for_each_field` is O(fields²) — each field re-scans the whole map body
- **File:line:** crates/shamir-types/src/record_view/record_ref.rs:338-346 (calls `materialize_at(&[key])` inside `fields()` iteration); lens.rs:1131-1168 (`value_bytes_at` restarts at `pos = 0` per call)
- **Severity:** medium
- **Issue:** For each top-level field, `fields()` advances a cursor cheaply, but the value bytes are then obtained via `materialize_at` → `value_bytes_at`, which does a fresh linear scan from offset 0 (read key, skip/match values) until the field is found. Total work is Σ i = O(N²) marker reads for an N-field record. This violates CLAUDE.md pillar 3 ("avoid hidden O(N)/O(N²) in helpers"); the crate even ships `FieldIndex` precisely to avoid repeated scans — unused here.
- **Failure scenario:** `SELECT *` / full projection on wide records (hundreds of fields) pays quadratic scanning per row on the engine's read path; no perf invariant or test guards it.
- **Suggested fix:** Build `self.index()` once, then decode each value from its indexed offset (mirror `FieldIndex::get`), or capture `(id, val_start..pos)` pairs during the single `fields()` walk.

### 6. `mpack!` rejects negative (or any multi-token) literals as OBJECT values — asymmetric with arrays/lists, uncovered by tests
- **File:line:** crates/shamir-types/src/macros/mpack.rs:249-261 (object `$value:tt` arms consume exactly ONE token tree); module doc :90-96 promises `-7` works; src/macros/tests/mpack_tests.rs has negatives only top-level/arrays
- **Severity:** low
- **Issue:** In object position, `"k": -7` tokenizes as two tokens (`-`, `7`); the single-tt value arms match neither, the muncher eventually accumulates `-`/`7` into the KEY state and the expansion ends with "no rules expected" — a compile error. Arrays work because their flush arm re-expands `mpack!($($elem)+)` into the dedicated `(- $n:literal)` arm. The doc sells negative-literal support without carving out objects; the test suite never exercises a negative (or float-expression) object value, so the asymmetry is invisible to CI.
- **Failure scenario:** A user writes `mpack!({"delta": -7})` following the doc and hits an inscrutable macro error (workaround exists via `@`, undocumented for this case).
- **Suggested fix:** Add object-value arms mirroring the array flush (accumulate tt run until `,`/end, re-expand into `mpack!(...)`), or extend the generic single-tt arm to `$($value:tt)+`; add tests: `{"profit": -7}`, `{-2.5}` nested lists/maps.

### 7. Untested public surface with documented contracts: `Interner::generation`, `ResourceMeta::owner_field`, `principal64_from_username`, `SecretString::into_inner`, `ResourcePath::WasmCompiler`
- **File:line:** interner.rs:303-305; access.rs:296-300, :59-66; secret.rs:40-43; access.rs:408/523/581 (WasmCompiler arms); grep shows definitions only — no test references
- **Severity:** low
- **Issue:** Each carries a behavioral promise that nothing pins:
  - `generation()` is the documented staleness signal for cache/filter compilation ("incremented on every successful touch") — zero tests, including that `touch_with_id` raising `current_id` bumps it.
  - `owner_field` exists specifically to distinguish "explicitly owned by System" from "field absent" for privilege-escalation decisions (its doc says `from_record`'s collapse is wrong for these callers) — security-relevant discrimination with no test.
  - `principal64_from_username` (used by production `access_control.rs` bridge sites until #559) — no determinism/distinctness test; FxHash-on-names collisions are silently possible and nothing documents observed bounds.
  - `SecretString::into_inner` ownership handoff (drop must NOT zeroize the taken buffer) — untested; also note the non-crypto feature build silently skips zeroization entirely (documented in Cargo.toml, untestable there).
  - `ResourcePath::WasmCompiler` parent/Display/permits flow is absent even from the exhaustive-looking `trace_access_transparent_for_all_variants` loop (which enumerates every other variant).
- **Failure scenario:** Any refactor (e.g. swapping the current_id bump order in `touch_with_id`, or changing `from_owner_id`) can silently invalidate cached-filter completeness or escalate/rename ownership decisions with green tests.
- **Suggested fix:** One small test file per area: generation bump cases (touch_ind/touch_with_id monotonicity), owner_field present-vs-absent-vs-null parity vs `from_record`, username-hash determinism + a fuzz-ish distinctness sanity check, into_inner validity after move, WasmCompiler added to the variant loops.

### 8. `touch_ind` racing `touch_with_id` on the same id is guarded only by `debug_assert!` — silent in release
- **File:line:** crates/shamir-types/src/core/interner/interner.rs:216-233 (`set_reverse_slot` debug_assert), :200-207 (documented out-of-scope hazard)
- **Severity:** low
- **Issue:** The single-writer lock serializes reverse writes among themselves, but cross-API exclusion (monotonic `touch_ind` vs WAL-recovery `touch_with_id`) rests on a recovery-model assumption enforced only under debug builds. In release, a lost race performs `OnceLock::set` failure → slot keeps the OTHER name while the forward map holds THIS name's id — a permanent forward/reverse divergence, silently. Tests cover each API separately, never the interaction (explicitly declared out of scope, hence low/residual).
- **Failure scenario:** Only reachable if recovery ever runs concurrently with live traffic (the exact condition the docs say cannot happen — the enforcement mechanism is the assumption itself).
- **Suggested fix:** Promote to a logged+Err (or swap to storing only-if-empty with a returned verdict) rather than `debug_assert!`, so a future caller that violates the model fails loudly instead of corrupting the namespace silently.

### 9. Nit: vacuous self-comparison assertion in `from_ts_produces_unique_ids_with_same_timestamp`
- **File:line:** crates/shamir-types/src/types/tests/record_id_tests.rs:107-113
- **Severity:** nit
- **Issue:** Asserts `id.as_bytes()[..8] == RecordId::from_ts(ts).as_bytes()[..8]` — re-encoding the same input and comparing encoders to themselves; always true regardless of layout bugs. (The subsequent shared-prefix loop does carry real weight.)
- **Suggested fix:** Compare against the hand-computed `relative.to_be_bytes()` like `from_ts_preserves_byte_layout` does, or drop the tautology.

### 10. Nit: duplicated public `CodecError` name, malformed doctests, dead error variant, misnamed error variant
- **File:line:** src/codecs/basic/bincode.rs:8-22 (second `CodecError{Serialize,Deserialize}` alongside `codecs::CodecError{Encode,Decode}`) with broken pseudo-doctests at :26-32/:43-50 referencing nonexistent `shamir_db::types::codec` (currently masked only because `[lib] doctest = false`); src/types/value_error.rs:19-24 (`PathNotFound` is produced by no API — `get_path` returns Option); src/record_view/lens.rs:196 (`read_str_len` reports errors as `NonBinKey`)
- **Severity:** nit
- **Issue:** Name-shadowed error enums invite wrong-type imports at call sites; the doctest snippets would fail the moment doctests are re-enabled; `PathNotFound` implies a path-based error surface that doesn't exist; `NonBinKey` fires for string reads too, misleading diagnostics.
- **Suggested fix:** Rename the bincode enum (e.g. `BincodeError`), fix/delete the broken examples, remove or start producing `PathNotFound`, add a `NotAStr` variant.

## Notes on scope
No cargo commands were executed; findings are from static reading of every `.rs` file under `crates/shamir-types/src/` plus Cargo.toml and all `tests/` directories. Concurrency/performance/build-gate themes were intentionally left to sibling reviewers except where they intersect correctness (finding #5) or TDD discipline (findings #4, #7, #9).

</details>
