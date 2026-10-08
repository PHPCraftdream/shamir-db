<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-types — api-wire-protocol revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Lossy variant encoding, decoder inconsistency, constructor collisions, error swallowing, and stale codec documentation remain. Several API-polish items are intentional contracts or incorrectly described rather than defects.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 14 | 0 | 0 | 1 | 0 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Wire format cannot represent Dec / Big / Set -- types silently degrade to Str / List on every encode

Status: `confirmed-open`. Current risk: `medium`.

All inspected serializers still flatten Dec/Big to strings and Set to arrays; decoders do not restore variants. Loss is deliberate and tested, and Big's wire rule is documented externally, but Value/README contract clarity and typed numeric comparisons remain incomplete. ScalarRef comparison lacks Str-to-Dec/Big arms; normal lit_u64 builders instead emit strings and can match string storage. Parent qualification: documented string/list projection is not itself an unexpected wire defect. Remaining documentation and direct typed-RHS comparison consistency must distinguish embedded values from the ordinary serialized protocol path.

Evidence: [crates/shamir-types/src/types/value.rs:72](../../../../../crates/shamir-types/src/types/value.rs#L72); [crates/shamir-types/src/types/value.rs:83](../../../../../crates/shamir-types/src/types/value.rs#L83); [crates/shamir-types/src/codecs/interned/messagepack.rs:954](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L954); [crates/shamir-types/src/record_view/scalar_ref.rs:192](../../../../../crates/shamir-types/src/record_view/scalar_ref.rs#L192); [crates/shamir-types/src/types/tests/value_tests.rs:420](../../../../../crates/shamir-types/src/types/tests/value_tests.rs#L420); [docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md:21](../../../../../docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md#L21).

<a id="review-2"></a>

### Claim 2 — Split-brain decode contract for msgpack u64 > i64::MAX (Big vs Str depending on decoder)

Status: `confirmed-open`. Current risk: `medium`.

ValueVisitor promotes raw uint64 to Big; custom decoder and lens emit decimal Str. From<usize> still wraps on 64-bit oversized input. Existing tests separately pin contradictory decoder outcomes. The normative wire document specifies Big promotion, so recommending Str everywhere would change the documented contract.

Evidence: [crates/shamir-types/src/types/value.rs:142](../../../../../crates/shamir-types/src/types/value.rs#L142); [crates/shamir-types/src/types/value.rs:660](../../../../../crates/shamir-types/src/types/value.rs#L660); [crates/shamir-types/src/codecs/interned/messagepack.rs:183](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L183); [crates/shamir-types/src/record_view/lens.rs:614](../../../../../crates/shamir-types/src/record_view/lens.rs#L614); [crates/shamir-types/src/types/tests/value_tests.rs:643](../../../../../crates/shamir-types/src/types/tests/value_tests.rs#L643); [crates/shamir-types/src/codecs/interned/tests/messagepack_tests.rs:562](../../../../../crates/shamir-types/src/codecs/interned/tests/messagepack_tests.rs#L562); [docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md:6](../../../../../docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md#L6).

<a id="review-3"></a>

### Claim 3 — RecordId::system() silently truncates names longer than 12 bytes -> durable-ID collisions

Status: `confirmed-open`. Current risk: `medium`.

The total constructor still truncates, and its registered test intentionally asserts aliasing. Truncation is documented, not undisclosed. Several production DDL sites have independently avoided long-name collisions; this does not fix the constructor. A <=12-byte check alone would not prevent trailing-NUL/padding aliases.

Evidence: [crates/shamir-types/src/types/record_id.rs:94](../../../../../crates/shamir-types/src/types/record_id.rs#L94); [crates/shamir-types/src/types/record_id.rs:100](../../../../../crates/shamir-types/src/types/record_id.rs#L100); [crates/shamir-types/src/types/tests/record_id_tests.rs:48](../../../../../crates/shamir-types/src/types/tests/record_id_tests.rs#L48); [crates/shamir-index/src/persistence.rs:529](../../../../../crates/shamir-index/src/persistence.rs#L529).

<a id="review-4"></a>

### Claim 4 — +/-0.0 violates the Hash/Eq contract the NaN fix explicitly established

Status: `confirmed-open`. Current risk: `high`.

IEEE zero equality and raw-bit non-NaN hashing still disagree; the wired test asserts divergent hashes.

Evidence: [crates/shamir-types/src/types/value.rs:293](../../../../../crates/shamir-types/src/types/value.rs#L293); [crates/shamir-types/src/types/value.rs:709](../../../../../crates/shamir-types/src/types/value.rs#L709); [crates/shamir-types/src/types/tests/value_tests.rs:525](../../../../../crates/shamir-types/src/types/tests/value_tests.rs#L525).

Grouping/duplicate: `correctness-tdd.md:1`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — RecordRef::to_query_value swallows de-intern errors into QueryValue::Null

Status: `confirmed-open`. Current risk: `medium`.

Both tree and lens trait implementations still use unwrap_or(Null), erasing missing-id failures. Current full-record projection and validator adapters consume this method. Swallowing is documented but remains indistinguishable from legal Null data.

Evidence: [crates/shamir-types/src/record_view/record_ref.rs:107](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L107); [crates/shamir-types/src/record_view/record_ref.rs:223](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L223); [crates/shamir-types/src/record_view/record_ref.rs:349](../../../../../crates/shamir-types/src/record_view/record_ref.rs#L349); [crates/shamir-engine/src/query/read/select_projection.rs:225](../../../../../crates/shamir-engine/src/query/read/select_projection.rs#L225); [crates/shamir-engine/src/validator/record_fields.rs:175](../../../../../crates/shamir-engine/src/validator/record_fields.rs#L175).

<a id="review-6"></a>

### Claim 6 — src/codecs/README.md documents APIs that no longer exist (and wrong semantics)

Status: `confirmed-open`. Current risk: `low`.

README still advertises removed InternedCodec/CodecFormat/legacy modules, wrong return types, a de-intern panic, and all-type round trips. Current exports and Result-returning implementation contradict it.

Evidence: [crates/shamir-types/src/codecs/README.md:13](../../../../../crates/shamir-types/src/codecs/README.md#L13); [crates/shamir-types/src/codecs/README.md:63](../../../../../crates/shamir-types/src/codecs/README.md#L63); [crates/shamir-types/src/codecs/README.md:290](../../../../../crates/shamir-types/src/codecs/README.md#L290); [crates/shamir-types/src/codecs/README.md:430](../../../../../crates/shamir-types/src/codecs/README.md#L430); [crates/shamir-types/src/codecs/interned/mod.rs:7](../../../../../crates/shamir-types/src/codecs/interned/mod.rs#L7); [crates/shamir-types/src/codecs/interned/common.rs:24](../../../../../crates/shamir-types/src/codecs/interned/common.rs#L24).

<a id="review-7"></a>

### Claim 7 — ResourcePath renders URIs (Display) but has no parser; rendering duplicated cross-crate

Status: `confirmed-open`. Current risk: `low`.

Display and HMAC's ResourceRef renderer remain separate, matching implementations; no ResourcePath parser is present. This is maintenance debt, not a current signing discrepancy. Both signing sides use canonical_resource_ref, and Display does not promise parsing or a round trip.

Evidence: [crates/shamir-types/src/access.rs:561](../../../../../crates/shamir-types/src/access.rs#L561); [crates/shamir-query-types/src/hmac.rs:179](../../../../../crates/shamir-query-types/src/hmac.rs#L179); [crates/shamir-query-types/src/hmac.rs:184](../../../../../crates/shamir-query-types/src/hmac.rs#L184); [crates/shamir-server/src/db_handler/admin.rs:697](../../../../../crates/shamir-server/src/db_handler/admin.rs#L697).

<a id="review-8"></a>

### Claim 8 — Two different `CodecError` enums under adjacent names; bincode's skips thiserror

Status: `confirmed-open`. Current risk: `low`.

Two distinct public enums remain and convenience functions return the basic bincode enum, not the adjacent codecs::CodecError. Wrong-type matching/propagation is normally a compile error, not silently missed runtime errors.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:8](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L8); [crates/shamir-types/src/codecs/error.rs:4](../../../../../crates/shamir-types/src/codecs/error.rs#L4); [crates/shamir-types/src/codecs/mod.rs:12](../../../../../crates/shamir-types/src/codecs/mod.rs#L12).

Grouping/duplicate: `error-handling-lifecycle.md:2`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — Stringly-typed error payloads on public interner APIs

Status: `confirmed-open`. Current risk: `low`.

touch_ind still returns a production-infallible Result with &'static str; touch_with_id returns formatted String errors. No inspected production caller substring-matches them, and a String error type does not allocate on successful calls.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:138](../../../../../crates/shamir-types/src/core/interner/interner.rs#L138); [crates/shamir-types/src/core/interner/interner.rs:348](../../../../../crates/shamir-types/src/core/interner/interner.rs#L348); [crates/shamir-engine/src/tx/recovery.rs:491](../../../../../crates/shamir-engine/src/tx/recovery.rs#L491).

Grouping/duplicate: `error-handling-lifecycle.md:3`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — ResourceMeta::inject_into silently no-ops on non-map records

Status: `confirmed-open`. Current risk: `low`.

The method returns unit and writes only inside the Map branch. Actual non-map privilege widening was not established; inspected normal callers construct maps. A fallible API would detect future misuse.

Evidence: [crates/shamir-types/src/access.rs:245](../../../../../crates/shamir-types/src/access.rs#L245); [crates/shamir-types/src/access.rs:304](../../../../../crates/shamir-types/src/access.rs#L304); [crates/shamir-db/src/shamir_db/system_store.rs:194](../../../../../crates/shamir-db/src/shamir_db/system_store.rs#L194).

<a id="review-11"></a>

### Claim 11 — Nits (API polish)

Status: `confirmed-open`. Current risk: `nit`.

All five components are evaluated separately below; the pre-epoch claim is refuted, random Default is intentional, and remaining items are documentation/API hygiene.

Evidence: [crates/shamir-types/src/types/record_id.rs:45](../../../../../crates/shamir-types/src/types/record_id.rs#L45); [crates/shamir-types/src/types/record_id.rs:127](../../../../../crates/shamir-types/src/types/record_id.rs#L127); [crates/shamir-types/src/macros/mpack.rs:291](../../../../../crates/shamir-types/src/macros/mpack.rs#L291).

<a id="review-11a"></a>

### Claim 11a — Default for RecordId generates a fresh random ID

Status: `not-applicable`. Current risk: —.

Default intentionally delegates to new. Copy/Hash imposes no nil-default requirement, and cloning an already-created id does not mint another id. Deprecating this trait behavior is an API-policy choice, not a proven fix.

Evidence: [crates/shamir-types/src/types/record_id.rs:20](../../../../../crates/shamir-types/src/types/record_id.rs#L20); [crates/shamir-types/src/types/record_id.rs:127](../../../../../crates/shamir-types/src/types/record_id.rs#L127).

<a id="review-11b"></a>

### Claim 11b — from_ts before CUSTOM_EPOCH produces system-prefixed IDs

Status: `refuted`. Current risk: —.

The operands are signed i64. Ordinary pre-epoch subtraction is negative and encodes with nonzero leading bytes; integer-underflow saturation produces i64::MIN, also not a zero prefix. The genuine post-epoch 2^32-microsecond hole is a different supported claim.

Evidence: [crates/shamir-types/src/types/record_id.rs:14](../../../../../crates/shamir-types/src/types/record_id.rs#L14); [crates/shamir-types/src/types/record_id.rs:45](../../../../../crates/shamir-types/src/types/record_id.rs#L45); [crates/shamir-types/src/types/record_id.rs:108](../../../../../crates/shamir-types/src/types/record_id.rs#L108).

<a id="review-11c"></a>

### Claim 11c — UserValue deprecated, twin QueryValue is not

Status: `confirmed-open`. Current risk: `nit`.

Both aliases still have identical underlying types but different documented purposes. The tests-only UserValue migration wording is overly broad; QueryValue remains an intentional name-keyed boundary type, not inherently forbidden production use.

Evidence: [crates/shamir-types/src/types/value.rs:19](../../../../../crates/shamir-types/src/types/value.rs#L19); [crates/shamir-types/src/types/value.rs:29](../../../../../crates/shamir-types/src/types/value.rs#L29); [crates/shamir-types/src/codecs/interned/codec.rs:137](../../../../../crates/shamir-types/src/codecs/interned/codec.rs#L137).

<a id="review-11d"></a>

### Claim 11d — bincode.rs malformed stale doctests

Status: `confirmed-open`. Current risk: `nit`.

Stale paths and duplicated derive text remain. They are unfenced rustdoc prose rather than runnable doctests, so enabling doctests alone would not execute these snippets.

Evidence: [crates/shamir-types/src/codecs/basic/bincode.rs:24](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L24); [crates/shamir-types/src/codecs/basic/bincode.rs:42](../../../../../crates/shamir-types/src/codecs/basic/bincode.rs#L42); [crates/shamir-types/Cargo.toml:89](../../../../../crates/shamir-types/Cargo.toml#L89).

Grouping/duplicate: `correctness-tdd.md:10`. This row is not another independent defect.

<a id="review-11e"></a>

### Claim 11e — `MpackIntoValue` documented as sealed but isn't

Status: `confirmed-open`. Current risk: `nit`.

The public doc-hidden trait has no private sealing supertrait, so downstream local types can implement it. Its non-API warning does not enforce the advertised seal.

Evidence: [crates/shamir-types/src/macros/mpack.rs:286](../../../../../crates/shamir-types/src/macros/mpack.rs#L286); [crates/shamir-types/src/macros/mpack.rs:291](../../../../../crates/shamir-types/src/macros/mpack.rs#L291).

## Corrections and qualified non-findings

- Big's flattening rule is already documented in NUMERIC_WIRE_SEMANTICS.md; describe incomplete public-type documentation rather than universally undocumented behavior.
- FilterValue has no direct Dec/Big literal variant. Exact OQL ::dec syntax/reachability was not established; source-supported mismatch is a persisted string field compared with an in-memory typed numeric RHS.
- Generic Value::from_bytes accepts id-keyed storage maps; msgpack_to_inner instead requires string keys. They are not interchangeable storage decoders.
- HMAC clients and servers share the same canonical_resource_ref implementation. Divergence from diagnostic Display is hypothetical maintenance risk, not established signature failure.
- The code does not support the pre-epoch clamp claim, success-path String allocation claim, or silent wrong-error-type matching claim.
- Wire/lens parity tests are scoped to fixtures and supported shapes; raw oversized uint64, malformed iteration, and header-derived allocation prevent a universal mechanically-sound guarantee.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-types -- API & wire-protocol design

## Summary

The wire layer is in good shape mechanically: the id-keyed MessagePack encoder/decoder pairs
(`InternedRef`, `QvInternedRef`, `msgpack_to_inner_zerocopy`) and the zero-copy `RecordView` lens are
byte-exact against each other, parity-tested, depth-capped, and untrusted-input safe. The main theme-level
weakness is that the public value model advertises 11 typed variants while the serialization contract only
carries ~7 shapes on the wire -- `Dec`/`Big` flatten to strings and `Set` flattens to lists on every encode,
a choice pinned by tests but undocumented at the API level and contradicted by the codecs README. Around
that sit a handful of interface footguns: split-brain u64>i64::MAX decode semantics between the query
visitor and the storage decoders, a silent-truncating `RecordId::system()`, an enshrined Hash/Eq violation
for -0.0, error-swallowing trait methods, and one stale README documenting APIs that no longer exist.
Builder-rule compliance is clean: the crate contains zero `serde_json` references; `mpack!` is the
sanctioned typed-literal constructor for `QueryValue` data (not a hand-assembled wire op).

## Findings

### 1. Wire format cannot represent Dec / Big / Set -- types silently degrade to Str / List on every encode
- **File:line:** crates/shamir-types/src/types/value.rs:72-73 (`Dec`/`Big` -> `serialize_str` in the generic
  `Serialize` impl); codecs/interned/messagepack.rs:375-376 and :954-955 (identical mapping in `InternedRef`
  and `QvInternedRef`); :389-398 and :965-971 (`Set` -> seq); decoders map str->`Str` (messagepack.rs:200-218)
  and seq->`List` (value.rs:187-196, lens.rs:519-533).
- **Severity:** high
- **Issue:** All three serializers share this contract, so no path can round-trip `Value::Dec`,
  `Value::Big`, or `Value::Set`: write -> persist -> read always yields `Str(decimal_string)` / `List`.
  The behavior is deliberate (tests pin it: value_tests.rs:419-496 asserts Dec/Big "become Str";
  interned/tests/messagepack_tests.rs:437-500 asserts Set->List), and kind.rs:19-24 acknowledges it --
  but the *public API* does not: `mpack!`'s `@` escape hatch explicitly invites constructing
  `Dec`/`Big` values (macros/mpack.rs:31-42), `query_value_to_inner` preserves them in memory, and the
  codecs README claims "MessagePack roundtrip for all types" with a type table omitting Dec/Big entirely.
- **Failure scenario:** A record written with field = `Dec(123.45)` stores `"123.45"`; after reload the
  stored scalar is `Str`. `scalar_ref_cmp` / `scalar_ref_cmp_qv` have NO `Str` <-> `Dec`/`Big` bridge
  (scalar_ref.rs:151-202 returns None), so `WHERE price = 123.45::dec` silently never matches a row that was
  written through exactly such a literal. A persisted `Set{3,1,2}` re-reads as `List[3,1,2]`: dedup/ordering
  semantics change shape silently, and `PartialEq` treats List vs Set as unequal, so pre/post-reload
  equality checks fail.
- **Suggested fix:** Tag the flattened variants on the wire (MessagePack ext codes for Dec/Big/Set -- ext is
  already collapsed to Bin on read so a versioned escape exists) or add a per-record schema byte. Failing
  that, minimum viable honesty: (a) correct codecs/README.md's round-trip claim and add Dec/Big/Set rows to
  the type table, (b) document the lossy rule on `Value` itself, (c) either add exact Str->Dec fallback arms
  to `scalar_ref_cmp(_qv)` or reject Dec/Big literals at the builder boundary.

### 2. Split-brain decode contract for msgpack u64 > i64::MAX (Big vs Str depending on decoder)
- **File:line:** types/value.rs:142-155 (serde visitor promotes to `Big(BigInt)` -- the "Unified u64
  contract, fix FG-1"); codecs/interned/messagepack.rs:183-190 (storage zerocopy decoder -> `Str`);
  record_view/lens.rs:610-620 (`uint_to_record_value` -> `RecordValue::Str`).
- **Severity:** medium
- **Issue:** Identical wire bytes (a raw msgpack uint above i64::MAX) decode as `InnerValue::Big` via the
  serde/`from_bytes` path but as `InnerValue::Str` via the storage/lens path. The lens doc ("mirrors the
  tree") is true only versus the zerocopy decoder, not the visitor; nothing reconciles them. Same file also
  still does the wrap-cast the FG-1 comment condemns: `From<usize> for Value<String>` uses `v as i64`
  (value.rs:660-664), inconsistent with `From<u64>` immediately above it.
- **Failure scenario:** A client/WASM guest emits a native uint > i64::MAX. Reading it back through
  `QueryValue::from_bytes` yields `Big(...)`; reading the same record from storage yields `Str("...")`.
  `Value::eq` cross-type is false, so the same logical field compares unequal across paths; downstream
  type-dispatch logic sees different discriminants for one input.
- **Suggested fix:** Pick one contract. Recommend matching the storage decoders (`Str`) everywhere since the
  encoder can never emit raw >i64::MAX ints anyway, then update `visit_u64` + the FG-1 comments to say why;
  align `From<usize>` with `From<u64>`; add a parity test pinning Big-vs-Str agreement across both decoders.

### 3. RecordId::system() silently truncates names longer than 12 bytes -> durable-ID collisions
- **File:line:** crates/shamir-types/src/types/record_id.rs:95-103 (truncation); :18/:107-109
  (`SYSTEM_RECORD_PREFIX` / `is_system`).
- **Severity:** medium
- **Issue:** System IDs are deterministic persistent metadata identifiers built from a name copied into 12
  bytes; anything longer is truncated with no signal (returns `Self`, not `Result`/`Option`). Two distinct
  system names sharing a 12-byte prefix alias to the same ID.
- **Failure scenario:** `RecordId::system("index_build_meta_v2")` vs `RecordId::system("index_build_meta_v3")`
  produce the identical 16-byte ID; catalogue/metadata writes under the second name land on the first
  identity. Nothing downstream can detect the collision because the API cannot report it.
- **Suggested fix:** Validate length at construction: return `Result<RecordId, RecordIdError>` (or panic as
  an invariant per house rules) when `name.len() > 12`; keep a convenience `system_truncating()` if callers
  genuinely rely on prefix aliasing. Add a test asserting distinct names never alias.

### 4. +/-0.0 violates the Hash/Eq contract the NaN fix explicitly established
- **File:line:** crates/shamir-types/src/types/value.rs:697-711 (Hash hashes raw bits except NaN
  canonicalization), :293-299 (PartialEq uses IEEE `==`, where `0.0 == -0.0`); pinned as expected behavior by
  src/types/tests/value_tests.rs:524-530.
- **Severity:** medium
- **Issue:** The NaN canonicalization comment states the invariant "`k1 == k2 => hash(k1) == hash(k2)`
  required by HashSet/HashMap (found via a distinct() dedup regression)". `+0.0 == -0.0` is true under IEEE
  comparison yet their bit patterns hash differently, violating the same invariant -- and the regression test
  enshrines it ("Different bit patterns -> different hashes").
- **Failure scenario:** `TSet<Value>` containing `F64(0.0)` reports `contains(F64(-0.0)) == false` (wrong
  bucket); inserting both zeros duplicates an element PartialEq calls equal. This is the same dedup-regression
  class as the fixed NaN bug, one rotation away.
- **Suggested fix:** Canonicalize `-0.0` to `+0.0` bits in `Hash` (one line next to the NaN arm): if
  `f.to_bits() == f64::NEG_ZERO_BITS { hash +0.0 }`. Flip the test to assert equal hashes and put both zeros
  in one set.

### 5. RecordRef::to_query_value swallows de-intern errors into QueryValue::Null
- **File:line:** crates/shamir-types/src/record_view/record_ref.rs:222-224 (:225 impl for InnerValue),
  :348-350 (impl for RecordView); trait doc :106-108 documents the swallow; related:
  `HavingView::materialize_at` fabricates `Some(InnerValue::Null)` for containers (:514-539) and
  `query_value_to_inner_value` maps containers -> Null (:567-582).
- **Severity:** medium
- **Issue:** The codec functions correctly return `Result`, but the public trait wrapper flattens a missing
  interner key (stale reverse-snapshot / genuine corruption) into an empty result. `Null` is also a legal
  data value, so failure is indistinguishable from a legitimately null record.
- **Failure scenario:** A cache-stale interner during failover makes every projected row render as
  `QueryValue::Null`; callers log/store empty rows instead of surfacing an error and retrying (the closure
  twin `record_view_deintern_with` explicitly designs FOR retry-on-cache-miss, making the trait-level
  swallowing inconsistent within the same module).
- **Suggested fix:** Change the trait method to `Result<QueryValue, CodecError>` (pre-1.0 crate, published =
  false), or add `try_to_query_value` alongside and deprecate the swallowing form.

### 6. src/codecs/README.md documents APIs that no longer exist (and wrong semantics)
- **File:line:** crates/shamir-types/src/codecs/README.md:13-25 (file tree listing `legacy_text.rs`,
  `legacy/tools.rs`), :63-101 (`InternedCodec` trait, `CodecFormat` enum), :225-300 (`text_to_inner`,
  "deintern_key ... Panics if key not found"), :332-344 (type table omits Dec/Big), :426-439 ("MessagePack
  roundtrip for all types").
- **Severity:** medium
- **Issue:** Actual surface (codecs/mod.rs, interned/mod.rs) has no `InternedCodec`/`CodecFormat`, no
  legacy_text files, no `legacy/tools.rs` or `TransformResult`; `deintern_key` returns
  `Result<_, CodecError>` (common.rs:24-28) rather than panicking. Interned codec doc (codec.rs:7-9)
  confirms these were removed. A live README inside src/ claiming phantom APIs and wrong panic semantics is
  interface drift future work will copy from.
- **Failure scenario:** A contributor implements an ACL/decode feature against `CodecFormat::LegacyText` or
  relies on `deintern_key` panicking on corruption; neither matches reality; review time wasted rediscovering
  the real API.
- **Suggested fix:** Rewrite the README around the current tree (`Codec<T>` + interned free functions +
  projection/validate_keys + merge_storage_bytes), delete the Legacy sections, include the Dec/Big/Set
  flattening table from Finding 1.

### 7. ResourcePath renders URIs (Display) but has no parser; rendering duplicated cross-crate
- **File:line:** crates/shamir-types/src/access.rs:561-588 (`db://`, `fn://`, `user://`, `group://` formats);
  duplicated independently in crates/shamir-query-types/src/hmac.rs:186-189 (`db://` rebuilt for HMAC
  canonical strings); no `FromStr for ResourcePath` exists anywhere (workspace grep: 0 hits).
- **Severity:** low
- **Issue:** One-way encoding, encoded twice. The HMAC signing format (a security surface) and the display
  format are maintained in two crates with no shared definition or parse round-trip.
- **Failure scenario:** An added variant or formatting tweak in one renderer desyncs signature computation
  from audit/error output; wire clients receiving `err.path` strings cannot reconstruct the typed path.
- **Suggested fix:** Move canonical encoding (+ a total `parse` if the grammar is closed) into shamir-types
  beside Display, and make hmac.rs delegate to it.

### 8. Two different `CodecError` enums under adjacent names; bincode's skips thiserror
- **File:line:** crates/shamir-types/src/codecs/error.rs:3-9 (thiserror `Encode/Decode`) vs
  codecs/basic/bincode.rs:6-22 (manual `Serialize/Deserialize`, own `Display`); re-exported side by side via
  codecs/mod.rs:12 and basic/mod.rs:4.
- **Severity:** low
- **Issue:** Callers of `basic::{to_bytes,from_bytes}` get a structurally different type than callers of the
  `Codec` trait despite the identical simple name, and the manual enum violates the house rule "thiserror for
  library error enums". (README openly documents the split without justifying it.)
- **Failure scenario:** A function generic over both codec styles needs two match arms per variant name;
  a caller pattern-matching `CodecError::Encode(..)` silently misses bincode failures styled
  `CodecError::Serialize(..)`.
- **Suggested fix:** Fold into the single thiserror enum (map Serialize->Encode, Deserialize->Decode) or rename
  the bincode one `BincodeError`.

### 9. Stringly-typed error payloads on public interner APIs
- **File:line:** crates/shamir-types/src/core/interner/interner.rs:138 (`touch_ind -> Result<_, &'static str>`),
  :348 (`touch_with_id -> Result<(), String>`, WAL-recovery-public API).
- **Severity:** low
- **Issue:** House style mandates thiserror enums for library errors; these force callers to match on message
  substrings and allocate Strings even on the success path's signature contract.
- **Failure scenario:** Recovery code distinguishing "name remap" from "id collision" branches on English text;
  message edits break recovery handling invisibly.
- **Suggested fix:** Small `InternerError { ReservedZero, NameRemap { .. }, IdCollision { .. }, Race { .. } }`
  thiserror enum; keep messages in its `#[error]`.

### 10. ResourceMeta::inject_into silently no-ops on non-map records
- **File:line:** crates/shamir-types/src/access.rs:245-261 (also :304-319 duplicate insert logic in
  `to_query_value`).
- **Severity:** low
- **Issue:** If `rec` is not a `Map`, `inject_into` returns `Ok(())`-shaped `()` having written nothing: ACL
  owner/group/mode fields vanish from the persisted catalogue record without any signal. The mutation has no
  way to be observed missing until a permission check reads absent defaults.
- **Failure scenario:** A caller passes a freshly-built non-map catalogue row (variant change upstream);
  resource silently persists open/System-owned instead of creator-owned; privilege decisions then run on wrong
  metadata.
- **Suggested fix:** Return `Result<(), ValueError>` (`NotAMap`) mirroring `Value::set_path`'s convention, and
  share one insertion helper between `inject_into`/`to_query_value`.

### 11. Nits (API polish)
- **Default for RecordId generates a fresh random ID** — record_id.rs:127-131. `Self::default()` on a
  `Copy`/hash-keyed ID type minting a new random identifier each call invites silent divergence
  (`..Default::default()` clones differently each time). Deprecate in favor of explicit `new()`/`nil()`.
  Severity: nit.
- **from_ts before CUSTOM_EPOCH produces system-prefixed IDs** — record_id.rs:41-53: `saturating_sub` clamps
  relative time to 0, so timestamps before 2026-01-31 (clock skew, imported data) yield leading zero bytes ->
  `is_system() == true` and colliding sort prefixes with real system records. Consider erroring or reserving a
  non-zero sub-epoch bias. Severity: nit-to-low.
- **UserValue deprecated, twin QueryValue is not** — value.rs:25-31. `QueryValue` is the identical alias used
  pervasively in production (access.rs, shamir-query-types wire structs), contradicting the UserValue note's
  "production should use InnerValue directly". Either un-deprecate the string-keyed family or state precisely
  which users must migrate. Severity: nit.
- **bincode.rs malformed stale doctests** — basic/bincode.rs:24-33, :42-51: tripled duplicated `# #[derive]`
  lines and nonexistent paths (`shamir_db::types::codec::{self,...}`); harmless today only because
  `doctest = false`. Severity: nit.
- **`MpackIntoValue` documented as sealed but isn't** — macros/mpack.rs:286-293: no private supertrait binds
  the seal; downstream impls would compile. Either add `: __Sealed` or drop the word. Severity: nit.

---

Test-coverage note (skimmed per brief): every module carries the mandated `tests/` directory with manifest-only
`mod.rs` (rule-compliant). Coverage is unusually strong where it matters for this theme -- lens/tree parity,
de-intern parity, merge_storage_bytes byte-identity, projection/validate keys -- and the wire tests honestly
pin the lossy Dec/Big/Set contracts (finding 1) rather than hiding them. Gaps: nothing exercises ±0.0 set/lookup
semantics (only the divergent hashes themselves are asserted), and there is no test that `RecordId::system`
distinct-name inputs stay collision-free.

</details>
