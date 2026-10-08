<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The consolidated mechanisms remain largely unchanged at frozen HEAD. Scope/marker mismatch, alias-state loss, macro leakage and deep-input panics are actionable; two previously open rows describe intentional or optional API choices rather than defects. No quantitative release-blocking conclusion is established.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 39 | 34 | 0 | 0 | 1 | 0 | 4 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — `Batch::try_build` falsely rejects nested `sub_batch`/`for_each` batches whose inner entries have internal `$query` refs

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Inner a→b references are falsely checked against outer aliases. Production plans inner bodies separately; inspected SDK callers use build, limiting the demonstrated high-impact claim.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-types/src/batch/planner.rs:308](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L308); [crates/shamir-sdk/src/db.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L139).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — `Batch::after` and `Batch::when` silently no-op when the handle's alias is not registered

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Missing target mutations are discarded before validation; foreign equal aliases instead alter local operations.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1003); [crates/shamir-query-builder/src/batch/batch.rs:1025](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1025); [crates/shamir-query-builder/src/batch/handle.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/handle.rs#L11).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — `collect_query_refs` uses the loose pre-#641 marker rule, diverging from the planner's exact marker-map convention

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A literal map with $query plus an ordinary sibling produces a false alias error locally but no marker dependency in the planner.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1122](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1122); [crates/shamir-query-types/src/batch/planner.rs:385](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L385).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — `try_build` does not validate `return_only` aliases

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unknown selections pass local validation and executor retention silently removes unmatched results; this is a diagnostic gap.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:912](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L912); [crates/shamir-engine/src/query/batch/batch_execute.rs:868](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/batch_execute.rs#L868).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — `switch` with zero cases emits `when: Not{Or{[]}}`; empty-`Or` truth value is unpinned and untested

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Not(Or([])) currently evaluates true; the missing test is for default execution, not an unresolved truth value.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1077](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1077); [crates/shamir-engine/src/query/filter/filter_node.rs:664](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/filter_node.rs#L664).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — No findings — clean by construction

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Exclusive owned mutations introduce no local concurrency protocol. This does not prove fallback correctness or adversarial constant-time hashing.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L27); [crates/shamir-query-builder/src/write/doc.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/doc.rs#L19); [crates/shamir-collections/src/lib.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L18).

<a id="review-3-1"></a>

### Claim 3.1 — Plaintext password's zeroize-on-drop is silently disabled by this crate's own dependency profile

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

query-types enables types defaults and crypto through an unconditional dependency; the disabling premise is false before any later fix.

Evidence: [crates/shamir-query-types/Cargo.toml:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/Cargo.toml#L11); [crates/shamir-types/Cargo.toml:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/Cargo.toml#L53); [crates/shamir-types/src/secret.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L67).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Builder holds the plaintext password in a bare `String` until `build()`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Abandoning the unbuilt private-field builder bypasses SecretString Drop; direct downstream logging and indefinite residue are not proven.

Evidence: [crates/shamir-query-builder/src/ddl/auth.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L19); [crates/shamir-query-builder/src/ddl/auth.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L52).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — `Doc::set` does a full msgpack round-trip per field — the crate's per-row hot loop

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The literal converter is unused, establishing avoidable work but no measured medium-impact bottleneck.

Evidence: [crates/shamir-query-builder/src/write/doc.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/doc.rs#L43); [crates/shamir-query-types/src/filter/filter_value.rs:322](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L322).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — `rows_as` deserializes via a per-record encode+decode round-trip instead of one batched pass

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Repeated buffers are visible; raw-record batching changes Inserted/IdBytes projection semantics and measured harmful impact is absent.

Evidence: [crates/shamir-query-builder/src/response/batch_response_ext.rs:186](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L186); [crates/shamir-query-types/src/read/query_record.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L189).

Grouping/duplicate: [performance-hotpath.md#2](performance-hotpath.md#review-2). This is not an additional independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No typed arm exists; Subscribe optimization must distinguish delivery scope and every delivery variant.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-types/src/subscribe/deliver_mode.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/subscribe/deliver_mode.rs#L10).

Grouping/duplicate: [performance-hotpath.md#3](performance-hotpath.md#review-3). This is not an additional independent defect.

<a id="review-4-4"></a>

### Claim 4.4 — `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

SDK execution encodes the cloned request; no consuming alternative exists, but exact peak-memory multiplication is unmeasured.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:886](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L886); [crates/shamir-sdk/src/db.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L139).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-4-5"></a>

### Claim 4.5 — `Batch::switch` guard construction is O(K²) deep clones and O(K²) emitted filter nodes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each guard includes prior condition clones; cost is condition-size-weighted and undocumented, not measured latency harm.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1065](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1065).

Grouping/duplicate: [performance-hotpath.md#5](performance-hotpath.md#review-5). This is not an additional independent defect.

<a id="review-4-6"></a>

### Claim 4.6 — `to_request_via_msgpack` is a build+encode+decode triple on a public API

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Build/encode/decode is explicitly documented diagnostic functionality. Public availability does not make the decode a performance defect; panic handling is separate.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:872](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L872); [crates/shamir-query-builder/src/batch/batch.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L878).

Grouping/duplicate: [performance-hotpath.md#6](performance-hotpath.md#review-6). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — `try_build` over-validates nested batch bodies against the outer alias namespace

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Whole-op fallback traverses inner bodies whose production dependencies belong to an independent namespace.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-types/src/batch/planner.rs:308](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L308).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — `Handle` has no batch identity; post-hoc mutators cannot detect misuse

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Alias-only handles cannot distinguish equal-name foreign operations; lookup misses lose mutation intent.

Evidence: [crates/shamir-query-builder/src/batch/handle.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/handle.rs#L11); [crates/shamir-query-builder/src/batch/batch.rs:1025](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1025).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — Re-registering an alias silently replaces the earlier op — a database operation vanishes

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Unconditional IndexMap insertion replaces the previous value and fresh QueryEntry fields erase its guard and ordering state.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1096](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1096); [crates/shamir-query-builder/src/batch/batch.rs:1106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1106).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — `subscribe!` / `bind!` macros hardcode foreign crate paths, breaking the crate's dependency-hiding contract

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Builder-only consumers cannot resolve hardcoded query-types/collections expansion paths; internal macro tests supply them.

Evidence: [crates/shamir-query-builder/src/macros/mod.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L63); [crates/shamir-query-builder/src/macros/mod.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L100); [crates/shamir-query-builder/src/macros/mod.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L162).

Grouping/duplicate: [api-wire-protocol.md#4](api-wire-protocol.md#review-4). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — `return_only` bypasses the construction-time validation story

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unknown names are not checked, although explicit return_only is a valid override of silent flags.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:912](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L912); [crates/shamir-engine/src/query/batch/batch_execute.rs:868](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/batch_execute.rs#L868).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — `Query` pagination setters silently clobber each other

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Cross-family reset is real but setters promise setting values, not conflict rejection. Remaining debt is explanatory documentation and mixed-chain coverage.

Evidence: [crates/shamir-query-builder/src/query/query.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/query/query.rs#L154); [crates/shamir-query-builder/src/query/query.rs:182](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/query/query.rs#L182).

Grouping/duplicate: [api-wire-protocol.md#6](api-wire-protocol.md#review-6). This is not an additional independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — Public API leaks `rmp_serde` error types; decode errors are re-labeled as encode errors

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The public signatures expose codec enums and to_query_value folds decode errors into encode Syntax. Compatible-release breakage is not established.

Evidence: [crates/shamir-query-builder/src/wire/mod.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L31); [crates/shamir-query-builder/src/wire/mod.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L37); [crates/shamir-query-builder/src/response/batch_response_ext.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L30).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-5-8"></a>

### Claim 5.8 — public panicking API contradicts the #1083 precedent

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Locally owned deep id lists reach the pinned decoder's active container limit and the helper converts failure to panic.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L878); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-5-9"></a>

### Claim 5.9 — `lit_u64`'s decimal-`String` encoding for `u64 > i64::MAX` is only specified for equality

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Big/String Compare is already exact; decimal String/String ordering is lexical and scalar-only range/set paths differ.

Evidence: [crates/shamir-engine/src/query/filter/resolve.rs:161](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L161); [crates/shamir-engine/src/query/filter/resolve.rs:201](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L201); [crates/shamir-engine/src/query/filter/filter_node.rs:952](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/filter_node.rs#L952).

Grouping/duplicate: [api-wire-protocol.md#9](api-wire-protocol.md#review-9). This is not an additional independent defect.

<a id="review-5-10"></a>

### Claim 5.10 — doc examples name APIs that do not exist

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

query_ref and mpak remain stale references; the former is only an implementation comment.

Evidence: [crates/shamir-query-builder/src/filter/leaf.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/filter/leaf.rs#L83); [crates/shamir-query-builder/src/write/insert.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/insert.rs#L44).

Grouping/duplicate: [SUMMARY.md#7.6](SUMMARY.md#review-7-6). This is not an additional independent defect.

<a id="review-5-11"></a>

### Claim 5.11 — `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The same unqualified names are exported by val and select; module qualification works and wire behavior is unaffected.

Evidence: [crates/shamir-query-builder/src/val/expr.rs:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/val/expr.rs#L15); [crates/shamir-query-builder/src/select/select_item.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/select/select_item.rs#L42); [crates/shamir-query-builder/src/select/select_item.rs:76](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/select/select_item.rs#L76).

Grouping/duplicate: [api-wire-protocol.md#11](api-wire-protocol.md#review-11). This is not an additional independent defect.

<a id="review-5-12"></a>

### Claim 5.12 — `Batch`'s ~40 named DDL/DML methods do not constrain the op family

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The actual generic API forwards the supplied variant unchanged; no enforced-family contract or failure caused by the method name was found.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:355](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L355); [crates/shamir-query-builder/src/batch/into_batch_op.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/into_batch_op.rs#L13).

Grouping/duplicate: [api-wire-protocol.md#12](api-wire-protocol.md#review-12). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — `Batch::to_request_via_msgpack` panics on codec error — public API, contradicts the crate's own #1083 rationale

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Deep QueryValue id containers exceed exact rmp-serde 1.3.1's active array/map budget; serialization has no active depth counter and decode failure reaches expect.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L878); [crates/shamir-types/src/types/value.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L77); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — a misuse path `try_build` structurally cannot see

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The absent-target branch changes no stored state and records no error, leaving nothing for later validation.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1003); [crates/shamir-query-builder/src/batch/batch.rs:1025](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1025).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — Fallible-build pattern not applied to builders with self-documented required fields

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Implementation, conditional HMAC, priority, field-type and scope requirements remain unchecked locally; server rejection and embedded trust must be distinguished.

Evidence: [crates/shamir-query-builder/src/ddl/function.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/function.rs#L95); [crates/shamir-query-builder/src/ddl/validator.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/validator.rs#L162); [crates/shamir-query-builder/src/ddl/schema.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/schema.rs#L377); [crates/shamir-query-builder/src/ddl/replication.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/replication.rs#L56).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — Five error enums hand-roll `Display` + `std::error::Error` despite the workspace thiserror rule

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Five manual exhaustive implementations depart from thiserror policy but preserve current error messages and ResponseError source chaining.

Evidence: [crates/shamir-query-builder/src/batch/build_error.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/build_error.rs#L45); [crates/shamir-query-builder/src/ddl/create_index_build_error.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/create_index_build_error.rs#L131); [crates/shamir-query-builder/src/response/batch_response_ext.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L63).

Grouping/duplicate: [error-handling-lifecycle.md#4](error-handling-lifecycle.md#review-4). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — `Doc::set` `.expect()`s the `FilterValue` -&gt; `QueryValue` msgpack round-trip in a public setter

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Locally constructed nested FilterValue arrays exceed the QueryValue decoder budget and reach the public setter's expect.

Evidence: [crates/shamir-query-builder/src/write/doc.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/doc.rs#L47); [crates/shamir-query-types/src/filter/filter_value.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L45); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-6-6"></a>

### Claim 6.6 — Guarded `unwrap()`/`expect()` cluster in `TryFrom<&CreateIndex>` is sound but could be total

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Some-type, positive-dimension and single-field checks dominate every reported unwrap/expect; no reachable invariant failure was identified.

Evidence: [crates/shamir-query-builder/src/ddl/create_index.rs:759](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/create_index.rs#L759); [crates/shamir-query-builder/src/ddl/create_index.rs:782](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/create_index.rs#L782); [crates/shamir-query-builder/src/ddl/create_index.rs:826](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/create_index.rs#L826).

Grouping/duplicate: [error-handling-lifecycle.md#6](error-handling-lifecycle.md#review-6). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — `ToWire` trait + blanket impl live directly in `wire/mod.rs`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Trait methods and blanket implementation remain in the manifest file; the issue is structural only.

Evidence: [crates/shamir-query-builder/src/wire/mod.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L24).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — All four `macro_rules!` definitions live inline in `macros/mod.rs`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

All four macro definitions remain without a declared manifest-only-rule exception.

Evidence: [crates/shamir-query-builder/src/macros/mod.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L25); [crates/shamir-query-builder/src/macros/mod.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L86).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Schema/access-control bundle independent exports; smaller coupled families require policy judgment rather than automatic splitting.

Evidence: [crates/shamir-query-builder/src/ddl/schema.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/schema.rs#L36); [crates/shamir-query-builder/src/ddl/schema.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/schema.rs#L408); [crates/shamir-query-builder/src/ddl/access_control.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/access_control.rs#L77).

Grouping/duplicate: [style-claude-md.md#3](style-claude-md.md#review-3). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — Imports not at top: one production-code site + a pervasive function-local `use` pattern in tests

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A redundant production import and test-local imports remain without documented collision exceptions; behavior is unaffected.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1123); [crates/shamir-query-builder/src/batch/tests/after_tests.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/tests/after_tests.rs#L24).

Grouping/duplicate: [style-claude-md.md#4](style-claude-md.md#review-4). This is not an additional independent defect.

<a id="review-7-5"></a>

### Claim 7.5 — `cursor` module's tests use a bare `tests.rs` instead of the documented `tests/` directory + manifest

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

cursor/tests.rs is correctly registered but differs from the prescribed manifest-directory layout.

Evidence: [crates/shamir-query-builder/src/cursor.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/cursor.rs#L81); [crates/shamir-query-builder/src/cursor/tests.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/cursor/tests.rs#L1).

Grouping/duplicate: [style-claude-md.md#5](style-claude-md.md#review-5). This is not an additional independent defect.

<a id="review-7-6"></a>

### Claim 7.6 — Documentation drift: nonexistent APIs referenced in docs, stale module list

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

query_ref, removed panicking conversion commentary, mpak!, and omitted wire/bind/subscribe inventory all remain. They require scoped documentary corrections.

Evidence: [crates/shamir-query-builder/src/filter/leaf.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/filter/leaf.rs#L83); [crates/shamir-query-builder/src/batch/batch.rs:248](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L248); [crates/shamir-query-builder/src/write/insert.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/insert.rs#L44); [crates/shamir-query-builder/src/lib.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/lib.rs#L33).

<a id="review-7-7"></a>

### Claim 7.7 — Duplicate re-exports: the same items re-exported in both a sibling file and its `mod.rs`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Sibling and manifest exports identify the same types. Cleanup must retain imports needed inside the implementation.

Evidence: [crates/shamir-query-builder/src/select/select_item.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/select/select_item.rs#L8); [crates/shamir-query-builder/src/select/mod.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/select/mod.rs#L12); [crates/shamir-query-builder/src/write/update.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/update.rs#L10).

Grouping/duplicate: [style-claude-md.md#6](style-claude-md.md#review-6). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 14 | 14 | 0 | 0 | 0 | 0 | 0 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Scope-specific Batch/ForEach arms and exact marker recognition are absent. Mirror bind/over extraction without descending inner bodies; if recursively validating inner requests, use their own namespace. Preserve nested expression-marker recursion and add valid-inner/invalid-outer/literal-map controls.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1122](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1122); [crates/shamir-query-builder/src/batch/batch.rs:1333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-types/src/batch/planner.rs:308](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L308).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Mutator errors and duplicate protection are absent. Diagnose before overwrite or retain collision intent. debug_assert is insufficient in release; auto-renaming changes result aliases and is not a transparent fix. Decide foreign-handle and cloned-batch identity semantics explicitly.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1003](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1003); [crates/shamir-query-builder/src/batch/batch.rs:1096](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1096); [crates/shamir-query-builder/src/batch/handle.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/handle.rs#L11).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Doc::set still uses codecs for every value. Literal conversion is a plausible structural optimization, not a proved speedup; preserve binary and expression-marker encoding. The borrowed converter clones some values and does not itself establish bounded recursion.

Evidence: [crates/shamir-query-builder/src/write/doc.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/doc.rs#L43); [crates/shamir-query-types/src/filter/filter_value.rs:322](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L322).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Immediate credential wrapping remains unapplied. The feature-restoration component is refuted by the existing transitive default edge; do not add heavy features on that premise. Preserve transparent password wire encoding and caller ownership expectations.

Evidence: [crates/shamir-query-builder/src/ddl/auth.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L19); [crates/shamir-query-types/Cargo.toml:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/Cargo.toml#L11); [crates/shamir-types/src/secret.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L54).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No fallible roundtrip helper exists. Introduce phase-correct error propagation or a documented enforced construction bound. doc(hidden) does not prevent panic; cfg(test) removal breaks sibling callers. The diagnostic decode itself is intentional, not a performance defect.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L878); [crates/shamir-query-builder/src/wire/mod.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L37); [crates/shamir-server/src/replication/tests/supervisor_tests.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/tests/supervisor_tests.rs#L129).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Membership validation is absent. Keep it opt-in and preserve explicit selection of silent operations, since return_only overrides return_result flags.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:912](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L912); [crates/shamir-engine/src/query/batch/batch_execute.rs:868](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/batch_execute.rs#L868).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The listed DDL validation additions are absent. Prefer opt-in try_build equivalents rather than silently changing infallible API signatures. Mirror each actual contract independently; source currently takes precedence over wasm when both exist, so rejecting both would be a separate behavior change. Local HMAC presence cannot replace session verification.

Evidence: [crates/shamir-query-builder/src/ddl/function.rs:95](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/function.rs#L95); [crates/shamir-query-builder/src/ddl/validator.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/validator.rs#L48); [crates/shamir-query-builder/src/ddl/schema.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/schema.rs#L377); [crates/shamir-db/src/shamir_db/execute/admin_function.rs:65](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_function.rs#L65); [crates/shamir-server/src/db_handler/admin.rs:740](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L740).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

rows_as remains per-record. Reject the historical raw Vec&lt;QueryRecord&gt; recipe: serialize equivalent as_value projections and preserve missing-alias, empty-list, first-error and depth-boundary behavior. Add Inserted and IdBytes controls before treating batching as equivalent.

Evidence: [crates/shamir-query-builder/src/response/batch_response_ext.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L90); [crates/shamir-query-builder/src/response/batch_response_ext.rs:186](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L186); [crates/shamir-query-types/src/read/query_record.rs:189](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L189).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Foreign expansion paths remain. Route every generated path through stable $crate helpers/re-exports, including internal event arms and delivery payloads. A downstream consumer with only the builder dependency is the discriminating fixture.

Evidence: [crates/shamir-query-builder/src/macros/mod.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L63); [crates/shamir-query-builder/src/macros/mod.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L87); [crates/shamir-query-builder/src/macros/mod.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L162).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No phase-distinguishing crate error was introduced. Coordinate with the new fallible helper rather than repeating decode-as-encode mapping. Preserve useful typed source chaining; API wrapping is a compatibility decision, not an automatic reliability fix.

Evidence: [crates/shamir-query-builder/src/wire/mod.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L31); [crates/shamir-query-builder/src/wire/mod.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L37); [crates/shamir-query-builder/src/response/batch_response_ext.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/response/batch_response_ext.rs#L63).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

All structural components remain. Moves must preserve public exports, macro lexical wiring and test registration; removing duplicate pub uses requires replacement ordinary imports. Closely coupled family exceptions are policy choices.

Evidence: [crates/shamir-query-builder/src/wire/mod.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/wire/mod.rs#L24); [crates/shamir-query-builder/src/macros/mod.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/macros/mod.rs#L25); [crates/shamir-query-builder/src/ddl/schema.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/schema.rs#L36); [crates/shamir-query-builder/src/cursor.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/cursor.rs#L81); [crates/shamir-query-builder/src/select/select_item.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/select/select_item.rs#L8).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

All four documentary components remain. Correct qref arity and examples, describe current fallible conversion types, fix mpack spelling, and update the module/macro inventory without unrelated edits.

Evidence: [crates/shamir-query-builder/src/filter/leaf.rs:83](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/filter/leaf.rs#L83); [crates/shamir-query-builder/src/batch/batch.rs:248](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L248); [crates/shamir-query-builder/src/write/insert.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/insert.rs#L44); [crates/shamir-query-builder/src/lib.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/lib.rs#L33).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Consuming construction, Call/Subscribe typed paths and switch cost documentation remain absent. Retain reusable borrowed build; inspect every Subscribe delivery scope. An O(K) value-expression rewrite is not proved equivalent to conditional operation branches.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:886](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L886); [crates/shamir-query-builder/src/batch/batch.rs:1065](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1065); [crates/shamir-query-builder/src/batch/batch.rs:1333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1333); [crates/shamir-query-types/src/subscribe/deliver_mode.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/subscribe/deliver_mode.rs#L10).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Boundary documentation/tests and fallible Doc handling remain outstanding. Preserve valid empty-switch behavior and explicit pagination replacement unless policy changes. Numeric guarantees need storage/operator-specific controls. Name constraints, glob renames and guarded unwrap cleanup are optional, not runtime fixes. Doc panic reachability is now source-supported.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1077](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1077); [crates/shamir-query-builder/src/query/query.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/query/query.rs#L154); [crates/shamir-engine/src/query/filter/resolve.rs:161](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L161); [crates/shamir-query-builder/src/write/doc.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/write/doc.rs#L47); [crates/shamir-query-builder/src/ddl/create_index.rs:838](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/create_index.rs#L838).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Accepted SELECT/HAVING query references are not supplied by the production dependency path

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

Register known alias a and a read b whose only a reference is a SELECT function argument or HAVING operand. Builder try_build accepts it, but BatchPlanner's Read arm extracts only WHERE references. build_resolved_refs exposes results only for DataFlow/Both edges, so HAVING receives no a result unless another genuine reference independently supplies that edge. SELECT has an additional deterministic failure: SelectProjection evaluates with its own permanently empty reference map, so even an added dependency cannot supply a. An identity-like SELECT expression over a.first().field('x') therefore resolves to Null; a HAVING comparison against that reference can wrongly remove groups. Explicit after is not a data grant and is insufficient. Existing builder tests only reject nonexistent aliases; a discriminating integration oracle must use a known non-null producer result through real batch planning/execution and assert the projected value/group retention.

Evidence: [crates/shamir-query-builder/src/batch/batch.rs:1271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1271); [crates/shamir-query-builder/src/batch/batch.rs:1290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/batch.rs#L1290); [crates/shamir-query-builder/src/batch/tests/batch_tests.rs:363](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/tests/batch_tests.rs#L363); [crates/shamir-query-builder/src/batch/tests/batch_tests.rs:392](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/batch/tests/batch_tests.rs#L392); [crates/shamir-query-types/src/batch/planner.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L277); [crates/shamir-engine/src/query/batch/query_runner.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L54); [crates/shamir-engine/src/query/read/select_projection.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/select_projection.rs#L207); [crates/shamir-engine/src/query/read/select_projection.rs:242](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/select_projection.rs#L242); [crates/shamir-engine/src/query/read/select_projection.rs:273](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/select_projection.rs#L273); [crates/shamir-engine/src/query/read/aggregate.rs:1304](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/read/aggregate.rs#L1304); [crates/shamir-engine/src/query/filter/resolve.rs:363](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L363).

## Evidence and recipe corrections

- The 81 current claim rows are decisions including duplicates, not a unique-defect count; the historical 32-defect census and 'not shippable' conclusion are not independently established.
- Current high nested-scope severity is reduced to medium; Doc/rows_as medium performance severity is reduced to low absent workload evidence. Pagination remains only nit-level documentation debt.
- Current 4.6 is not applicable as a performance defect because codec roundtripping is the helper's documented purpose. Current 5.12 is not applicable as a runtime defect because broad operation forwarding is the actual API and execution follows its variant.
- Overview and P2.14 still contain unverified-panic wording after the parent supplied a witness. Exact archive inspection independently supports both public deep-input panic mechanisms, without measured target thresholds.
- #1083 removed expects only from fallback validation; #1093 added five typed paths. Their source changes predate the report and do not fix scope, marker, mutator, macro or public-helper defects. Historical green claims were not accepted as proof.
- The proposed design's claim that DTO-only layering prevents wire divergence is false; local validation and downstream execution can disagree. SELECT/HAVING reference handling supplies an additional concrete example.
- Scope-aware alias identity, phase-correct errors, response projection preservation and release-visible rejection must be maintained; debug assertions, automatic renaming and hiding helpers do not establish those guarantees.

## Module scope and limitations

Coverage: 8 assigned documents, 81 current claim rows, 14 plan rows, 0 pre-existing observation rows; 1 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [correctness-tdd.md](correctness-tdd.md); [concurrency-lockfree.md](concurrency-lockfree.md); [security-crypto.md](security-crypto.md); [performance-hotpath.md](performance-hotpath.md); [api-wire-protocol.md](api-wire-protocol.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [style-claude-md.md](style-claude-md.md); [SUMMARY.md](SUMMARY.md).

- All eight assigned documents, current ledgers, parent refinements, corrections, and collapsed historical claims and recipes were read. There are no assigned NEW.N observations or TASK_GROUPS.md.
- Inspection only: no files written, agents launched, programs reproduced, builds or tests executed. Test registration and discriminating assertions do not establish passing runs.
- HEAD remained the required commit. Unrelated working-tree changes, including Cargo.lock and later toolchain/CI changes, were observed. Dependency pins and toolchain metadata were taken from frozen HEAD; inspected builder sources and assigned reports had no working-tree changes.
- Actual CARGO_HOME and known registry source/cache locations were inspected. Exact rmp-serde 1.3.1, indexmap 2.14.0 and zeroize 1.8.2 published archives were read through stdout without extraction. The rmp-serde archive SHA-256 matched frozen Cargo.lock.
- No measured latency, allocation count, RSS, stack-failure threshold, browser-memory retention duration, or WASM compilation result is claimed.
- This is bounded claim revalidation using semantic-conformance and oracle checks, not an exhaustive engine, authorization, supply-chain, or Rust-category audit.

## Guarantee checks

- **The builder constructs existing wire DTOs without an engine or runtime dependency.** — `supported`. The public construction paths produce query-types DTOs. This layering does not prove the design document's stronger assertion that wire or execution divergence is impossible. Reference: crates/shamir-query-builder/src/lib.rs:4; crates/shamir-query-builder/Cargo.toml:15.
- **Nested Batch and ForEach bodies have their own alias namespaces.** — `diverges`. Production planning isolates inner bodies, but builder fallback validation collects their references and checks them against outer aliases. Reference: crates/shamir-query-types/src/batch/planner.rs:308; crates/shamir-engine/src/query/batch/query_runner.rs:680.
- **Write-value dependency discovery follows exact reserved-marker shapes.** — `diverges`. The builder recognizes any string-valued $query key, including literal maps with ordinary sibling keys. Reference: crates/shamir-query-types/src/batch/planner.rs:385.
- **Typed handles identify operations registered in this batch.** — `diverges`. Handles carry only alias strings. Missing-target mutations disappear; equal-alias foreign handles affect local operations; replacement preserves alias lookup while changing the operation. Reference: crates/shamir-query-builder/src/batch/handle.rs:11; docs/dev-artifacts/roadmap/QUERY_BUILDER.md:246.
- **SecretString zeroization is disabled by the builder dependency profile.** — `diverges`. The unconditional query-types-to-types dependency enables types' default crypto feature. Exact zeroize 1.8.2 published src/lib.rs uses volatile clearing and a compiler fence for slices: https://docs.rs/crate/zeroize/1.8.2/source/src/lib.rs . This counter-evidence predates the audit. Reference: crates/shamir-query-builder/Cargo.toml:17; crates/shamir-query-types/Cargo.toml:11; crates/shamir-types/Cargo.toml:53.
- **Public codec roundtrips are infallible for every locally constructible value.** — `diverges`. Exact rmp-serde 1.3.1 published encode.rs has no active container-depth enforcement, while decode.rs initializes depth to 1024 and counts arrays/maps through any_inner. Deep FilterValue::Array values and Batch.id QueryValue::List values therefore provide decode-error-to-expect witnesses, subject to sufficient stack resources. Sources: https://docs.rs/crate/rmp-serde/1.3.1/source/src/encode.rs and https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs . This is not a universal bound on every enum/newtype dispatch, typed walker, clone, or drop. Reference: crates/shamir-query-builder/src/write/doc.rs:43; crates/shamir-query-builder/src/batch/batch.rs:878; Cargo.lock:2949.
- **Accepted SELECT/HAVING query references participate in production dependency resolution.** — `diverges`. Builder validation accepts known aliases in these positions, but the production planner's Read arm scans only WHERE. SELECT additionally evaluates against an explicitly empty reference map. Reference: crates/shamir-query-builder/src/batch/batch.rs:1271; crates/shamir-query-types/src/batch/planner.rs:277; crates/shamir-engine/src/query/read/select_projection.rs:242.
- **lit_u64 preserves magnitude and supports uniformly numeric filtering.** — `diverges`. Magnitude preservation is supported. Big/String Compare ordering is exact; String/String ordering remains lexical, and Between/In/InSet lack Compare's materialization fallback. A uniform numeric-operator guarantee is unsupported. Reference: crates/shamir-query-builder/src/val/filter_value.rs:62; docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md:69.
- **rows_as batching can directly serialize Vec&lt;QueryRecord&gt; without changing behavior.** — `diverges`. Current extraction uses as_value: Direct borrows its value, Inserted exposes fields without injected identity, and IdBytes becomes Null. Raw QueryRecord serialization uses different representations. Reference: crates/shamir-query-builder/src/response/batch_response_ext.rs:90; crates/shamir-query-types/src/read/query_record.rs:189.
- **Positive try_* tests prove equivalence with the non-try wire path.** — `diverges`. The shared helper runs the same try path twice and compares decoded values. It proves repeatability, not cross-path equivalence; deterministic payload omission could pass. Reference: crates/shamir-query-builder/src/batch/tests/try_into_batch_op_tests.rs:22.
- **The reported behavioral tests are registered and selected by the documented runner.** — `supported`. Relevant unit modules are wired, including cursor/tests.rs. Top-level integration targets require full/integration selection; doctests are disabled. Registration does not prove runtime execution or successful results. Reference: crates/shamir-query-builder/src/lib.rs:49; crates/shamir-query-builder/src/batch/tests/mod.rs:1; scripts/test.sh:177; crates/shamir-query-builder/Cargo.toml:13.
- **All modules follow manifest-only files, header imports, and multiline test-literal conventions.** — `diverges`. Wire and macros mod.rs contain implementations; family files bundle independent builders; local imports and single-line mpack literals remain. Cursor tests are registered but use the alternate single-file layout. Reference: AGENTS.md:109; CLAUDE.md:508; crates/shamir-query-builder/src/wire/mod.rs:24.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- security-crypto#1 and SUMMARY#3.1 remain refuted: the unconditional query-types-to-types default-feature edge is positive, pre-existing counter-evidence.
- The parent deep-input conclusions for error-handling#1/#5 are independently confirmed from checksum-verified rmp-serde 1.3.1 published bytes. The active bound applies to the traced array/map decoding paths, not universally to every dispatch or local recursive operation.
- correctness#1, api#1 and SUMMARY#1.1/#5.1 remain confirmed-open, but severity changes high→medium: the proved effect is opt-in local false rejection, and inspected SDK execution uses build.
- performance#1/#2 and SUMMARY#4.1/#4.2 remain confirmed-open, but severity changes medium→low because redundant work is established without measured harmful workload impact.
- performance#6 and SUMMARY#4.6 change confirmed-open→not-applicable: the extra decode is explicitly promised diagnostic roundtrip behavior, not accidental work.
- api#12 and SUMMARY#5.12 change confirmed-open→not-applicable: actual IntoBatchOp forwarding preserves the supplied wire variant; no family-enforcement contract or execution failure caused by the convenience name was found.
- api#6 and SUMMARY#5.6 remain confirmed-open only as nit-level documentation debt, not a demonstrated semantic failure: setters replace state and no conflict-rejection promise was identified.
- The current overview's broad coverage assurance needs qualification: positive try_* helper tests compare the same path twice, and SELECT/HAVING builder validation tests do not exercise the production reference seam.
- No assigned source-fixed labels exist. The inspected #1083/#1093 history does not close residual claims, and no post-report builder changes were identified.

## Current follow-up order

1. Prevent silent operation/guard/ordering loss from duplicate aliases and missing-target mutations; decide foreign-handle identity explicitly.
2. Restore scope-correct and exact-marker validation with valid-inner, invalid-bind/over and literal-marker regression oracles.
3. Resolve the newly identified SELECT/HAVING reference mismatch at the production planner/context seam; builder-only tests cannot establish this feature.
4. Repair exported macro paths and validate a consumer whose sole domain dependency is shamir-query-builder.
5. Make public codec roundtrips fallible or enforce a documented local construction bound, with actual deep-input error-propagation controls.
6. Wrap CreateUser passwords immediately; retain the refutation of feature-disabled zeroization.
7. Add opt-in return selection/DDL diagnostics and storage/operator-specific numeric guarantees.
8. Optimize Doc conversion, row extraction and request ownership only with semantic-preservation controls and workload evidence; keep structural cleanup and optional API redesign separate.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-query-builder/` — the pure, synchronous, WASM-lean client-side query
builder: typed fluent constructors over `shamir-query-types` wire DTOs (no engine, no runtime,
no I/O; "query construction — builder only" is this crate *as a rule*).

Review basis: the seven 2026-08-14 lens reports under
`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-query-builder/`
(`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`, `performance-hotpath.md`,
`api-wire-protocol.md`, `error-handling-lifecycle.md`, `style-claude-md.md`), synthesized and
deduplicated in the format calibrated against the exemplar syntheses for
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. Read-only pass — no
build/test/lint commands; a handful of file:line refs were spot-checked against the working tree
(all verified; no new defects were found during synthesis, so nothing below is marked
"added during synthesis").

## Executive summary

The crate is one of the most disciplined in the workspace — builder-only construction with zero
injection surface, five typed error families with near-exhaustive error-path tests, per-module
`tests/` directories, and a concurrency lens that found literally nothing to flag (pure
single-owner builder, no shared state). But it is not shippable as-is: (1) **`Batch::try_build`
false-rejects every nested `sub_batch`/`for_each` batch whose inner entries reference each
other** — the crate's headline dependency feature — contradicting the planner's documented
scoping rule and pushing users onto the unvalidated `build()`; (2) **`Doc::set`, the central
write primitive, pays a full msgpack encode+decode round-trip per field** even though a typed
converter already exists in `shamir-query-types`; (3) **the `Batch` alias model loses state
silently**: `after`/`when` no-op on an unregistered handle and re-registering an alias silently
discards the previous op — conditional-execution guards and DDL→DML ordering edges can vanish
with no error, and `try_build` structurally cannot see either loss. Fix those three (plus the
medium-severity zeroize-on-drop gap that silently disables credential hygiene exactly in the
crate's headline WASM deployment) before anything else ships from this crate.

---

## 1. correctness-tdd

### 1.1 — high — `Batch::try_build` falsely rejects nested `sub_batch`/`for_each` batches whose inner entries have internal `$query` refs *(also flagged: api-wire-protocol #1 → 5.1 — one defect, two lenses)*
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1333-1345` (fallback arm of
  `collect_op_query_refs`), walker `collect_query_refs` at `:1122-1140`, driver `try_build` at
  `:913-931`; contrast `crates/shamir-query-types/src/batch/planner.rs:308-322`.
- Issue: `BatchOp::Batch` and `BatchOp::ForEach` are not on the typed fast path, so they fall to
  the "unconditionally-correct" msgpack round-trip, which serializes the ENTIRE inner
  `BatchRequest` and collects every `"$query"` key in it. Every collected ref is then checked
  against the **outer** batch's alias set. The planner states the opposite contract verbatim:
  "outer deps come exclusively from `bind` values. Do NOT descend into the inner batch's queries —
  those are planned recursively at execution time" (`planner.rs:308-315`, same for `ForEach` at
  316-322). Inner aliases are a separate scope (that is exactly why `bind`/`param` exists), yet
  `try_build` checks every inner ref against the outer namespace — the validator and the executor
  disagree on scoping.
- Failure scenario: build an inner batch where entry `b` reads entry `a` via
  `a.first().field("id")`, embed it via `outer.sub_batch("proc", inner.build(), bind)` (or
  `outer.for_each(...)`), then call `outer.try_build()` → `Err(BuildError::UnknownAlias { alias:
  "a", referenced_by: "proc" })` for a batch `inner.build()` produces fine and the engine plans
  and executes correctly. Callers hit by this will drop to the unvalidated `build()`, losing *all*
  validation (including the checks that are correct).
- TDD gap: `sub_batch_tests.rs` and `for_each_tests.rs` never call `try_build` at all, let alone
  with inner-to-inner refs; only `bind`/`over`/`CallOp` ref paths are covered (`call_tests.rs:158`,
  `batch_tests.rs`).
- Suggested fix: classify `BatchOp::Batch(sub)` and `BatchOp::ForEach(fe)` explicitly (per the
  crate's own exhaustive-match convention): collect refs only from `sub.bind` values / `fe.over`
  against outer aliases, skip the inner batch body (its own `try_build` is its validator;
  optionally validate the inner request against its own alias set). Red/green test: nested batch
  with internal ref must pass `try_build`; a `bind` value referencing an unknown outer alias must
  still fail.

### 1.2 — medium — `Batch::after` and `Batch::when` silently no-op when the handle's alias is not registered *(also flagged: api-wire-protocol #2 → 5.2, error-handling-lifecycle #2 → 6.2 — one defect, three lenses)*
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1003-1008` (`after`), `:1025-1030`
  (`when`).
- Issue: both post-hoc mutators are `if let Some(entry) = self.queries.get_mut(...) { ... }` with
  no else. A `Handle` from a *different* `Batch` instance (trivially possible — both take
  `&Handle`, the compiler cannot tell), or one whose alias was never registered / later replaced
  (see 5.3), makes the call a silent no-op: no ordering edge is written, no guard is attached.
  `try_build` cannot catch this because nothing was ever recorded — its `after`-list validation
  (`batch.rs:934-957`) has nothing to fire on. The doc for `after` (`batch.rs:993-998`) already
  concedes the two-`&Handle` transposition footgun; cross-batch handles are worse.
- Failure scenario: `b.when(&h, guard)` with `h` from another batch → the op ships with
  `when: None` and executes **unconditionally** — a conditional-execution safety primitive
  silently disabled. `b.after(&rows, &mk)` dropped → the documented primary use (DDL→DML
  ordering) is gone → the insert can run before `create_table`, surfacing as a remote batch error
  far from the bug site while the local `try_build()` reports `Ok`.
- TDD gap: `after_tests.rs` / `when_tests.rs` cover only the happy path plus manually-injected bad
  `after` strings; no test exercises an unknown/unregistered handle.
- Suggested fix: make the mutators fallible (`Result`/`bool` "was the alias found") or at minimum
  `debug_assert!(self.queries.contains_key(...))` plus a doc line naming the no-op semantics; the
  internal `switch()` callers (`batch.rs:1071,1079`) always pass freshly-registered handles, so
  the churn is contained. Add negative tests for both.

### 1.3 — medium — `collect_query_refs` uses the loose pre-#641 marker rule, diverging from the planner's exact marker-map convention
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1122-1140` vs
  `crates/shamir-query-types/src/batch/planner.rs:385-391`.
- Issue: the builder treats ANY map containing a `"$query"` string key as a ref; the planner
  (since #641) only treats len-1 maps with reserved keys (`$query`/`$fn`/`$cond`/`$expr`) or the
  exact 2-key `{"$query","path"}` shape as markers, everything else being literal data. The
  builder's header says it "mirrors planner.rs logic" — it mirrors the pre-#641 logic the planner
  explicitly fixed.
- Failure scenario: user data stored via `Doc::set_value` / `mpack!` that happens to contain
  `{"$query": "...", "other": ...}` (a field literally named `$query` with extra keys) is data to
  the server but a ref to `try_build` → spurious `UnknownAlias` rejection of a batch the engine
  accepts.
- Suggested fix: port the marker-map rule (`map.len()` 1-or-`{"$query","path"}`-2) into
  `collect_query_refs`; add a test with a non-marker map containing a `"$query"` key that must be
  ignored. Same walker as 1.1 — natural to land in the same PR.

### 1.4 — low — `try_build` does not validate `return_only` aliases *(also flagged: api-wire-protocol #5 → 5.5 — one defect, two lenses)*
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:912-987` (validates `$query`,
  `after`, `when` refs; `return_only`, set at `:124-131`, is untouched).
- Issue: `return_only(["typo_alias"])` passes validation and the server returns a silently
  reduced/empty result set — the same "typo'd alias" class `try_build` exists to catch, with the
  alias set already in hand.
- Failure scenario: a typo'd `return_only` entry silently narrows the response (or is rejected
  only server-side after a full round trip), defeating the "find out at construction time"
  guarantee the crate documents for its other validation passes.
- Suggested fix: in `try_build`, check every `return_only` entry against `self.queries` (new
  `BuildError::UnknownReturnAlias` variant; and, if the planner requires it, that the entry is
  non-silent). Test both directions.

### 1.5 — low — `switch` with zero cases emits `when: Not{Or{[]}}`; empty-`Or` truth value is unpinned and untested
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1077`
  (`let default_guard = not(or(seen_conditions));`).
- Issue: `switch(vec![], default)` is accepted and produces a guard whose evaluation is an engine
  convention (vacuous OR). Depending on that convention the default branch runs always or never —
  and `when_tests.rs` covers only 1, 2, and 4-case shapes, so nothing pins the degenerate input
  the builder permits. (Cost dimension of the same method is 4.5; distinct defect.)
- Failure scenario: `switch(vec![], default)` ships a guard whose semantics differ by engine
  convention; a refactor of the vacuous-OR rule silently flips the default branch's execution.
- Suggested fix: either reject empty `cases` (a switch with only a default needs no guard at all
  — or emit `when: None`), or add a test documenting the intended empty-`Or` semantics.

## 2. concurrency-lockfree

**No findings — clean by construction.** The crate is a pure, single-owner fluent builder: no
locks, atomics, shared/global state, or async surface at all (zero matches for
`Mutex`/`RwLock`/`parking_lot`/`arc_swap`/`scc`/`dashmap`/`Atomic*`/`OnceLock`/`thread_local`/
`Arc`/channels/`unsafe`/`async`/`.await`/`tokio` across `src/` and `tests/`), so the lock-free
and async-I/O pillars are satisfied vacuously. Pillar 4 (Fx hash) is fully honored — every
hash-keyed structure routes through `shamir_collections::{TMap, TSet, new_map}` (`Batch::queries`
/ `Batch::interner_epochs`, `Doc::fields`, the `bind!` macro); the only `std::collections` uses
are `BTreeMap` in two test fixtures (comparison-ordered, test-only, outside pillar 4's scope).
Pillar 3 holds: `try_build` validation uses O(1) lookups, the msgpack-fallback O(payload) walk is
a documented opt-in trade-off (perf aspects → 4.3), and `switch`'s O(K²) output equals the wire
data its semantics require (→ 4.5). The absence of concurrency-specific tests is appropriate.

## 3. security-crypto

### 3.1 — medium — Plaintext password's zeroize-on-drop is silently disabled by this crate's own dependency profile
- File:line: `crates/shamir-query-builder/src/ddl/auth.rs:52` (with
  `crates/shamir-query-builder/Cargo.toml:17,20`; `crates/shamir-types/src/secret.rs:67-75`).
- Issue: `CreateUser::build()` wraps the plaintext password into `SecretString`, whose whole point
  per its doc is "Drop that zeroizes the heap buffer before freeing it". That `Drop` impl is
  `#[cfg(feature = "crypto")]`, and this crate declares `shamir-types = { path =
  "../shamir-types", default-features = false }` (and `shamir-query-types` likewise), which turns
  `crypto` off. `shamir-types`' Cargo.toml confirms guest/WASM builds consume it exactly this way
  and "skip zeroize-on-drop". So in the crate's stated headline deployment (lib.rs: "compiles to
  WASM for browser clients") the password buffer is freed WITHOUT zeroization; Debug-redaction
  remains the only protection. In a native workspace build, feature unification re-enables
  `crypto` via other crates — the guarantee's presence is an accident of the surrounding
  dependency graph, invisible at this call site.
- Failure scenario: a browser/WASM client built from this crate calls `create_user(...)`; the
  password String's heap buffer is deallocated un-wiped and persists in JS-heap/`memory` dumps
  (browser heap snapshots, devtools memory inspection) indefinitely after the request is sent —
  a class of exposure zeroize exists to shrink.
- Suggested fix: depend on `shamir-types` with `features = ["crypto"]` (or add a minimal `secret`
  feature on shamir-types that pulls only `zeroize` and use it). Zeroize is a tiny, no_std-friendly
  dependency; the WASM-lean argument does not justify dropping the guarantee precisely where the
  crate handles credentials. Alternatively gate `SecretString`'s definition (not just its `Drop`)
  so a build without `crypto` cannot silently construct one and believe it is protected.

### 3.2 — low — Builder holds the plaintext password in a bare `String` until `build()`
- File:line: `crates/shamir-query-builder/src/ddl/auth.rs:8-25, 49-57`.
- Issue: `CreateUser.password: String` keeps the cleartext in an unprotected `String` for the
  entire builder lifetime; only at `build()` is it moved into `SecretString`. If the builder is
  dropped without `build()` (error path, early return), the plaintext is never zeroized even when
  the `crypto` feature IS enabled; and until the move, the value is an ordinary `String` any
  intermediate code can `clone()`/log. (Credit where due: no `#[derive(Debug)]` on any auth
  builder, so no accidental `{:?}` leak from this crate itself, and `SecretString::from(String)`
  takes ownership without copying.)
- Failure scenario: `let b = create_user("alice", pw); if !ready { return Err(..) }` — `b` drops
  with the password un-wiped even in a fully-featured native build.
- Suggested fix: wrap at the boundary: `create_user(name, password: impl Into<SecretString>)` (or
  store `SecretString` in the constructor immediately), shrinking the plaintext-as-bare-`String`
  window to the caller's own expression.

### Positive notes (kept for calibration parity)
Zero `unsafe` in the crate; no secret/tag comparisons client-side (no timing surface); no string
interpolation anywhere (`format!` appears once, `batch.rs:943`, only to echo a user alias into a
`BuildError`); the one byte-slicing site (`batch.rs:1361`) is boundary-safe; response extraction
(`batch_response_ext.rs`) is fully `Result`-based on malformed server data; the remaining
`unwrap`/`expect` sites are guarded invariants. The `canonical_*` doc pointers (e.g. auth.rs:42
"never the password") are good hygiene — keep them.

## 4. performance-hotpath

### 4.1 — high — `Doc::set` does a full msgpack round-trip per field — the crate's per-row hot loop
- File:line: `crates/shamir-query-builder/src/write/doc.rs:43-53` (amplified by the `doc!` macro,
  `src/macros/mod.rs:25-32`, which calls `.set` once per key).
- Issue: Every `.set(key, value)` call executes `rmp_serde::to_vec_named(&fv)` (heap `Vec`
  allocation + full serialization of the value tree) followed by `rmp_serde::from_slice` (full
  decode into a fresh `QueryValue` tree) — two codec passes and at least two allocations **per
  field**, discarded immediately. Bulk insert construction (`insert(t).row(doc().set(...).set(...))`
  for N rows x F fields) therefore pays N*F double codec passes plus 2*N*F transient allocations,
  dominating the CPU cost of building the request (far exceeding the single `to_msgpack` encode
  that actually ships it). This is precisely the "allocation in loops / per-row instead of
  batched+amortized" pattern pillar 3 (O(x→0)) bans, on the crate's central write-path primitive —
  and it hits WASM guests hardest, where `shamir-sdk`'s `db.execute` path funnels all builder
  traffic.
- Failure scenario: No functional failure — pure waste. A client streaming many small inserts (or
  a browser WASM guest building large batches) burns single-digit-x CPU and allocator traffic it
  never needed; `Doc::set` shows up as the hottest builder frame instead of the wire encode.
- Suggested fix: Use the already-exported typed converter
  `shamir_query_types::filter::filter_value_to_query_value(&fv)`
  (`shamir-query-types/src/filter/filter_value.rs:322`, symmetry-tested in
  `filter_value_conv_tests.rs`): scalars and nested `Array`s convert directly with zero codec
  involvement; only when it returns `None` (the expression variants `FieldRef`/`QueryRef`/
  `FnCall`/`Expr`/`Cond`/`Param`) fall back to the existing msgpack round-trip. Alternatively add
  a total, exhaustive `FilterValue -> QueryValue` mapping in `shamir-types` under the same
  "new variant must compile-error here" convention `batch.rs` already uses for
  `collect_filter_refs`. (A typed fast path also shrinks the panic surface of 6.5 — different
  defect, shared remediation.)

### 4.2 — medium — `rows_as` deserializes via a per-record encode+decode round-trip instead of one batched pass
- File:line: `crates/shamir-query-builder/src/response/batch_response_ext.rs:90-107` (helper
  `deserialize_record`), applied per row at `:186-189`.
- Issue: `rows_as<T>` maps `deserialize_record` over every record: each record is individually
  serialized to msgpack (fresh `Vec` allocation) and decoded into `T`. For an alias with R records
  that is 2R codec passes, R intermediate byte-buffer allocations, and R decoder setups, where the
  whole job is two passes over the same bytes. Pillar 3 explicitly prefers "batched + amortized
  over per-row"; this is a textbook per-row loop with hidden O(N) allocation churn that scales
  with result-set size (large `SELECT` pages, cursor pages via `fetch_next`).
- Failure scenario: A read returning thousands of rows makes typed extraction
  (`get_as`/`rows_as`) a measurable client-side bottleneck — 2x the decode work plus per-row
  allocator pressure, again amplified inside WASM guests.
- Suggested fix: Encode once, decode once: `let bytes = rmp_serde::to_vec_named(&qr.records)?;`
  then `rmp_serde::from_slice::<Vec<T>>(&bytes)` — `Vec<QueryRecord>` is `Serialize`, the wire
  encoding is identical, error semantics (first failing row surfaces the same `Deserialize` error)
  are preserved, and cost drops to 2 passes + 1 allocation total. Keep single-record `row_as`
  as-is.

### 4.3 — low — `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1308-1345` (fallback arm at
  `:1333-1345`). Same mechanism as 1.1 — but 1.1 is a correctness defect (false rejection); this
  is the cost defect, which survives any scoping fix.
- Issue: The #1093 typed fast path covers `Read`/`Insert`/`Update`/`Set`/`Delete`; every other
  variant — deliberately, per the well-documented module rationale — falls back to `to_vec_named`
  + `from_slice` into a `QueryValue` tree just to walk for `"$query"` keys. But
  `BatchOp::Call(CallOp { params: Vec<FilterValue>, .. })` carries exactly the shape
  `collect_filter_value_refs` (`batch.rs:1228`) already walks exhaustively, and `Subscribe`'s
  `SubscriptionSource.filter: Option<Filter>` / `DeliverMode::Batch(SubBatchOp { bind: TMap<String,
  FilterValue>, .. })` are likewise closed typed shapes. `Call` params routinely embed large
  literals (vector embeddings: dim*4 bytes each), so each `try_build` re-encodes + re-decodes
  potentially kilobytes of params the typed walker could inspect directly.
- Failure scenario: None (documented deferral, tracked in #1093); per-request validation
  overhead, not a correctness or unbounded-growth issue — hence low.
- Suggested fix: Extend the fast path to `BatchOp::Call` (walk `params` via
  `collect_filter_value_refs`) and `BatchOp::Subscribe` (source filters + `DeliverMode::Batch`
  bind values), keeping the unconditionally-correct msgpack fallback for the genuinely un-audited
  admin/DDL variants. Update the #1093 audit note accordingly.

### 4.4 — low — `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:886-900`
  (`queries: self.queries.clone()` etc.); reached per request from `to_msgpack` (`:868-870`),
  `shamir-sdk/src/db.rs:139-141`, `shamir-client/src/interner_cache_ops.rs:201,233,272`,
  `shamir-client/src/client.rs:402`.
- Issue: `build(&self)` deep-clones the whole accumulated state — the `queries` `TMap` (every op,
  doc, filter tree, all `$query` payload values), `return_only`, `interner_epochs` — so every send
  pays one full tree copy of its own payload *in addition to* the wire encode. A consuming variant
  would move the fields with zero copies.
- Failure scenario: None functional; doubles transient peak memory per request and adds a full
  deep copy per send — noticeable for large batches and for WASM guests with tight heaps.
- Suggested fix: Add `pub fn into_request(mut self) -> BatchRequest` that moves each field (no
  clones) and route the encode path through it (`into_msgpack(self)`, or callers do
  `let req = b.into_request()`); keep `build(&self)` for callers that legitimately reuse the
  `Batch` (tests, retry loops).

### 4.5 — low — `Batch::switch` guard construction is O(K²) deep clones and O(K²) emitted filter nodes
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1062-1083` (fold at `:1066-1069`).
- Issue: For case *i*, the guard folds `.iter().cloned()` over **all** prior conditions, so K cases
  produce sum(i-1) = O(K²) full filter-tree deep copies at build time, and the complementary
  `when` filters emitted on the wire themselves total O(K²) nodes (guard i embeds i-1 negated
  prior conditions). With the K=2..5 the sugar targets this is negligible, but nothing documents a
  ceiling: a 100-case switch silently clones ~5,000 condition trees and bloats the request
  proportionally. (Semantics of the degenerate K=0 input is 1.5 — distinct defect.)
- Failure scenario: Only pathological use (many cases) — super-linear build cost and request size
  growth; no correctness risk.
- Suggested fix: Document a practical case ceiling on `switch` (matching the "builder-only sugar"
  ADR framing), or for large K emit an O(K) first-match shape (nested `$cond` chain via
  `val::switch_case`, or an ordered `when`-evaluation convention) instead of per-case negation
  conjunctions. At minimum note the O(K²) in the doc comment.

### 4.6 — nit — `to_request_via_msgpack` is a build+encode+decode triple on a public API
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:878-881`. Same function as the 6.1
  panic cluster (which absorbs correctness-tdd #7 and api-wire #8) — kept distinct because the
  redundant-work defect is inherent to the function's design and survives any panic fix.
- Issue: `build()` deep clone (4.4) + full msgpack encode + full decode, ~3x the work of
  `to_msgpack`. The doc scopes it to tests ("notably tests"), but it is `pub` and
  indistinguishable from a production entry point at the call site; nothing prevents per-request
  adoption.
- Failure scenario: A caller using it as their send path triples per-request builder/codec cost.
- Suggested fix: `#[doc(hidden)]` / "test-only, do not ship" doc banner, or rename to make intent
  unmistakable (e.g. `round_trip_via_msgpack_for_tests`). Fold into 6.1's fix.

## 5. api-wire-protocol

### 5.1 — high — *(primary: same as 1.1)* — `try_build` over-validates nested batch bodies against the outer alias namespace
- (Full write-up at 1.1. Listed here because it is the lens-defining drift: the validator and the
  executor disagree on scoping — the wire protocol's nested-batch semantics are contradicted by
  the client-side validation pass, which converts the crate's headline `sub_batch`/`for_each`
  feature into a reason to abandon validation entirely.)

### 5.2 — medium — *(primary: same as 1.2)* — `Handle` has no batch identity; post-hoc mutators cannot detect misuse
- (Full write-up at 1.2. API-design root cause: `Handle` is just a `String` alias with no batch
  identity, so `after`/`when` cannot distinguish "this batch's handle" from "any string" — the
  silent no-op is the observable symptom.)

### 5.3 — medium — Re-registering an alias silently replaces the earlier op — a database operation vanishes
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:1096-1109` (`add_entry_after` uses
  `TMap::insert`, which overwrites). Distinct defect from 1.2 (unconditional overwrite vs lookup
  miss), though correctness-tdd's finding 2 names it as a related silent-loss class.
- Issue: Aliases are the result-key namespace, and `add_entry_after` `insert`s unconditionally.
  Registering the same alias twice silently discards the first `BatchOp` — including a write or
  destructive DDL — and also wipes any `after`/`when` state attached to it (the replacement entry
  starts with `after: []`, `when: None`), silently invalidating handles previously returned for
  that alias. Not even `try_build` detects the collision.
- Failure scenario: `b.insert("row", ins_a); b.insert("row", ins_b);` ships a batch containing
  only `ins_b`; `ins_a` is never executed and no error, warning, or result entry ever mentions it.
  In loops that reuse a literal alias, this silently drops all but the last iteration's op.
- Suggested fix: Reject duplicate aliases in `add_entry_after` (typed `BuildError::DuplicateAlias`,
  surfaced via `try_build` at minimum; a debug_assert in the infallible path), or rename-with-
  suffix and return the surviving `Handle`.

### 5.4 — medium — `subscribe!` / `bind!` macros hardcode foreign crate paths, breaking the crate's dependency-hiding contract
- File:line: `crates/shamir-query-builder/src/macros/mod.rs:63-69` (`bind!` emits
  `shamir_collections::new_map()`), `:85-189` (`subscribe!` emits
  `shamir_query_types::subscribe::EventMask`, `shamir_query_types::TableRef::with_repo`,
  `shamir_query_types::batch::SubBatchOp`).
- Issue: `lib.rs:66-79` re-exports the DTOs explicitly "so a downstream guest (the SDK) can name
  them without depending on shamir-query-types directly" (WASM-lean footprint is a stated design
  goal, `lib.rs:21-23`). The `#[macro_export]` macros contradict this: `macro_rules!` hygiene
  resolves the hardcoded `shamir_query_types::…` / `shamir_collections::…` paths **in the
  downstream crate**, so any consumer of `subscribe!`/`bind!` needs direct dependencies on both
  crates at compatible versions. Other macros in the same file correctly route through `$crate::`
  (`doc!`, `vals!`), so the fix pattern already exists in-file.
- Failure scenario: The WASM/SDK guest that depends only on `shamir-query-builder` cannot compile
  `subscribe!` or `bind!` (unresolved crate paths); a workspace-internal user is masked from the
  problem until the guest build breaks.
- Suggested fix: Route every emitted path through `$crate::` re-exports (`$crate::val::...`, plus
  new `pub use` re-exports for `TableRef` and `SubBatchOp` — or move bind-map construction behind
  a `$crate::batch::bind_map(...)` helper).

### 5.5 — low — *(primary: same as 1.4)* — `return_only` bypasses the construction-time validation story
- (Full write-up at 1.4.)

### 5.6 — low — `Query` pagination setters silently clobber each other
- File:line: `crates/shamir-query-builder/src/query/query.rs:154-188`
  (`limit`/`offset`/`page`), `:207-231` (`after`/`after_with_id`).
- Issue: `limit`/`offset` convert any non-`LimitOffset` pagination to `LimitOffset` (silently
  discarding a prior `page(n, size)`), and `page`/`after`/`after_with_id` replace the variant
  wholesale. The crate already solved exactly this bug class for `CreateIndex`
  (`check_conflicting_state` + `try_build`, `create_index.rs:81-160`), so the asymmetric leniency
  here is an inconsistency, not a principled choice.
- Failure scenario: `.page(3, 20).limit(5)` silently becomes `LimitOffset { limit: 5, offset: 0 }`
  — page 1, 5 rows — with no error; a keyset `.after(..)` call silently erases an earlier
  `.page(..)`.
- Suggested fix: Either track "pagination already set" and reject the second family via a
  `try_build`-style check (mirroring `CreateIndexBuildError`), or document the last-write-wins rule
  on each method.

### 5.7 — low — Public API leaks `rmp_serde` error types; decode errors are re-labeled as encode errors
- File:line: `crates/shamir-query-builder/src/wire/mod.rs:31-44` (`to_query_value`/`to_msgpack`
  return `rmp_serde::encode::Error`), `wire/mod.rs:37` (decode failure mapped to
  `encode::Error::Syntax`), `src/batch/batch.rs:868` (`Batch::to_msgpack`),
  `src/response/batch_response_ext.rs:15-37` (`ResponseError::Deserialize` carries
  `rmp_serde::decode::Error`).
- Issue: The crate's public error surface directly embeds a third-party crate's error enums,
  coupling the semver of this alpha crate to rmp-serde's (a future codec major bump becomes an API
  break). Additionally, `to_query_value` reports a *decode* failure as an encode error variant —
  documented in-line, but callers matching on the error cannot distinguish the phases.
- Failure scenario: Upgrading `rmp-serde` (even to a compatible-looking minor with error-type
  changes) breaks downstream `match` arms; a decode failure in `to_query_value` is misdiagnosed as
  an encoding bug by naive handlers.
- Suggested fix: Wrap codec errors in a crate-owned error enum
  (`WireError::{Encode(String), Decode(String)}`), following the crate's own
  `SerializationFailed { reason: String }` precedent in `BuildError`. Natural companion to 6.1's
  `try_` conversion.

### 5.8 — low — *(primary: same as 6.1)* — public panicking API contradicts the #1083 precedent
- (Full write-up at 6.1, which carries the merged severity from this file's low rating plus
  error-handling-lifecycle's medium.)

### 5.9 — low — `lit_u64`'s decimal-`String` encoding for `u64 > i64::MAX` is only specified for equality
- File:line: `crates/shamir-query-builder/src/val/filter_value.rs:62-82`.
- Issue: The unified-u64 contract documents that the `Str(decimal)` representation matches stored
  `Big` values via "the engine's cross-type comparison layer (`Big`↔`Str` **equality**)". For
  range/set operators (`gt`/`lt`/`between`/`in_`) built with the same value, no cross-type
  ordering guarantee is stated, and the builder cannot enforce one.
- Failure scenario: `Query::from("t").where_gt("id", lit_u64(u64::MAX))` compiles and ships
  `{"field": ["id"], "value": "<decimal string>"}`; if the engine's ordering layer only
  special-cases equality, the filter silently matches nothing (or errors per-op), and the wire
  encoding offers the server no way to know the string was intended numerically.
- Suggested fix: Either verify/extend the engine contract for ordering comparisons and broaden the
  doc, or add a typed `FilterValue::Big`-style constructor once the wire DTO grows one, so the
  numeric intent survives serialization.

### 5.10 — nit — *(primary: same as 7.6)* — doc examples name APIs that do not exist
- (Full write-up at 7.6, the merged doc-drift finding.)

### 5.11 — nit — `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users
- File:line: `crates/shamir-query-builder/src/val/filter_value.rs:116` vs
  `src/select/select_item.rs:42`; `src/val/expr.rs:15` vs `src/select/select_item.rs:76`.
- Issue: Both modules are designed for `use …::*` consumption (their own module docs say so), yet
  they export same-named functions with different signatures. The crate has an established rename
  precedent for exactly this (`and_expr`/`or_expr`/`not_expr` in `val/expr.rs:80-100`, `negate` in
  `FilterExt`), so these two pairs are inconsistent with it.
- Failure scenario: `use shamir_query_builder::{val::*, select::*};` makes bare
  `func(...)`/`expr(...)` calls an ambiguity error; users discover it only after writing call
  sites.
- Suggested fix: Rename the select-side constructors (e.g. `select_fn`/`select_expr` aliases, or
  keep `func` only behind the module path), matching the `*_expr` precedent.

### 5.12 — nit — `Batch`'s ~40 named DDL/DML methods do not constrain the op family
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:318-655` (all delegate to
  `op: impl IntoBatchOp`).
- Issue: Every specialized method (`create_table`, `insert`, `delete`, …) accepts any
  `IntoBatchOp`, so `b.create_table("t", insert("users").row(...))` compiles and ships a
  `BatchOp::Insert` under a create-table alias. The names provide documentation value only; no
  type- or validation-time check ties the method name to the op variant (`op()` is the
  *documented* escape hatch, which implies the named methods promise more than they enforce).
- Failure scenario: Swapped/mis-paired arguments compile cleanly and fail only at server
  execution, after a round trip — the exact failure mode the crate's `try_build` passes exist to
  prevent.
- Suggested fix: If enforcement is desired, wrap each family in a marker newtype implementing
  `IntoBatchOp` and make the named methods generic over the marker; otherwise document the methods
  as aliases of `op()` so the lack of enforcement is explicit.

*Scope note (api lens, carried forward): every terminal produces exactly a wire DTO; the msgpack
named-fields codec is exercised end-to-end (`wire_tests`, `to_request_via_msgpack_tests`,
byte-identical `create_index_matrix.json` cross-language fixtures); the infallible `build()` /
validating `try_build()` split is applied consistently across `Batch`, `Query`, and `CreateIndex`
with three separate, well-documented error families; all modules have `tests/` dirs; no raw
`json!`/`Value` query assembly exists in the crate.*

## 6. error-handling-lifecycle

### 6.1 — medium — `Batch::to_request_via_msgpack` panics on codec error — public API, contradicts the crate's own #1083 rationale *(also flagged: correctness-tdd #7 [nit], api-wire-protocol #8 [low]; perf lens adds the cost dimension separately → 4.6)*
- File:line: `crates/shamir-query-builder/src/batch/batch.rs:878-881` (doc at 872-877; `to_msgpack`
  at 868-870).
- Issue: `to_request_via_msgpack` is a public, non-test method that does
  `self.to_msgpack().expect("msgpack encode")` then `rmp_serde::from_slice(&bytes).expect("msgpack
  decode")`. Its doc asserts "the builder always produces a serialisable request" — but the
  crate's own `BuildError::SerializationFailed` doc (`build_error.rs:29-42`) says an entry can
  hold "a value msgpack cannot represent", which is exactly why `try_build` was converted from
  `.expect` to a typed error in #1083. The two docs cannot both be right: if encoding is truly
  infallible, `to_msgpack` should not return `Result`; if it is not, this method converts a
  `Result` into a panic for no gain. rmp-serde currently encodes non-finite f64 as-is, so the
  panic is likely unreachable today — precisely the "looked safe to the original author" reasoning
  the #1083 test comment (`batch_tests.rs:417-428`) dismantles. On the WASM target a panic traps
  the whole guest, not just a task.
- Failure scenario: a future `QueryValue`/`BatchRequest` field (or a new `BatchOp` payload) makes
  `rmp_serde::to_vec_named` fail for some client-constructed value (`Batch::id(impl
  Into<QueryValue>)`, `Doc::set_value`, `Insert::row` all accept arbitrary client `QueryValue`s);
  the caller's `let req = b.to_request_via_msgpack()` panics at the validation call site instead
  of producing an `Err`.
- Suggested fix: add `try_to_request_via_msgpack(&self) -> Result<BatchRequest,
  rmp_serde::encode::Error>` (mapping the decode error into `Error::Syntax`, exactly as
  `wire/mod.rs:31-38` already does for `to_query_value`) and either move the panicking convenience
  behind `#[cfg(test)]`/`doc(hidden)` or keep it as a thin documented wrapper over the `try_`
  form (this also closes 4.6's scoping concern). If the panicking variant stays public, reconcile
  its doc with `BuildError::SerializationFailed`'s.

### 6.2 — medium — *(primary: same as 1.2)* — a misuse path `try_build` structurally cannot see
- (Full write-up at 1.2. Lifecycle framing: the silent no-op violates the crate's own discipline
  of surfacing programmer misuse as a typed error — `ConflictingBuilderState`, `MissingWhereClause`,
  `AfterPathIgnored` — and it is the only error-path behavior with zero test coverage.)

### 6.3 — low — Fallible-build pattern not applied to builders with self-documented required fields *(also flagged: correctness-tdd #6, security-crypto #3 — one root-cause class, three lenses; all rated low)*
- File:line: `crates/shamir-query-builder/src/ddl/function.rs:95-107` (build; contract doc at
  86-92), `crates/shamir-query-builder/src/ddl/validator.rs:48-55` (`CreateValidator::build`) and
  `:162-166` (`BindValidator::priority`: "must be in `[1000, 9999]`", accepts any `u16`),
  `crates/shamir-query-builder/src/ddl/schema.rs:377-386` (`FieldBuilder::build` accepts an empty
  `ty`), `crates/shamir-query-builder/src/ddl/replication.rs:49-53` (`ReplScopeBuilder::table` —
  "requires `repo`", accepts `table` without `repo`); HMAC surface: `ddl/auth.rs:49-57,103-109,151-157,198-205`,
  `ddl/access_control.rs` (all builders), `ddl/function.rs:95-107`, also `ddl/drop_*.rs` /
  `ddl/replication.rs` (same pattern).
- Issue: the crate established the principle that a builder whose DTO requires a field must reject
  its absence at construction time ("so a caller finds out at *construction* time, not after a
  full round trip through the server" — `create_index_build_error.rs:8-12`; `builder_error.rs:1-12`).
  Multiple builders violate it with contracts documented in prose but unenforced in code, and
  every HMAC-gated op stores `hmac: Option<String>` initialized to `None`, finalizing happily
  without `.hmac(...)` — including `CreateFunction`, whose own doc says the tag is "Required IFF
  `security == "definer"` or `secret_grants` is non-empty", and `GrantRole`, documented as "the
  single most dangerous op in the system". There is no typestate, no `debug_assert!`, no
  `try_build` check for any of these; doc language like "HMAC-gated" overstates what the builder
  enforces — the actual gate is 100% server-side rejection after a network round-trip. (The
  security lens's remediation angle: enforce conditionally-required tags with a typed error or a
  two-type state machine, or reword the rustdocs to "HMAC-authorized server-side; optional here".)
- Failure scenario: `create_function("f").security("definer").build()` (no `.hmac(...)`) compiles
  and flows to the server, which rejects it at DDL-execution time — the exact late-failure round
  trip `CreateIndexBuildError` was created to eliminate. Same shape for `BindValidator::priority`
  out of range, `ReplScopeBuilder::table` without `repo`, `FieldBuilder` with empty `ty`, and a
  caller forgetting `.hmac(...)` on any gated op.
- Suggested fix: extend `BuilderError` with the missing variants (`MissingImplementation`,
  `HmacRequired`, `PriorityOutOfRange`, `MissingFieldType`) and convert these `build()`s to
  `Result` like their `write/` siblings, adding `TryIntoBatchOp` impls + `Batch::try_op` coverage
  (the plumbing already exists); mirror the server checks client-side per the crate's own pattern
  (`CreateIndex::try_build`, `Query::try_build`). At minimum reword the HMAC rustdocs so callers
  do not infer client enforcement.

### 6.4 — low — Five error enums hand-roll `Display` + `std::error::Error` despite the workspace thiserror rule
- File:line: `crates/shamir-query-builder/src/batch/build_error.rs:45-75`,
  `crates/shamir-query-builder/src/write/builder_error.rs:45-79`,
  `crates/shamir-query-builder/src/query/query_build_error.rs:40-68`,
  `crates/shamir-query-builder/src/ddl/create_index_build_error.rs:131-240`,
  `crates/shamir-query-builder/src/response/batch_response_ext.rs:39-70`.
- Issue: CLAUDE.md's error-handling section is normative: "`thiserror` for library error enums
  (with `#[from]` where natural)". Thirteen sibling crates depend on `thiserror = "2.0"`; this
  crate hand-implements `Display`/`Error` for all five enums (~250 lines of boilerplate, and new
  variants must remember both the match arm and the Display arm). `ResponseError::Deserialize
  { source, .. }` even hand-writes `source()` (`batch_response_ext.rs:63-70`) — the textbook
  `#[source]` case. The impls are correct today (all variants covered; no drift found), so this is
  convention drift and maintenance cost, not a defect.
- Failure scenario: a future variant added without its `Display` arm is a compile error
  (exhaustive match) — so the real cost is only boilerplate and review surface, which is why this
  is low.
- Suggested fix: a standalone `chore` task (per the "style sweeps get their own commit" rule)
  migrating the five enums to `thiserror::Error` with `#[error("...")]` attributes; no behavior
  change expected and the `PartialEq`/`Clone` derives are unaffected.

### 6.5 — low — `Doc::set` `.expect()`s the `FilterValue` -> `QueryValue` msgpack round-trip in a public setter
- File:line: `crates/shamir-query-builder/src/write/doc.rs:47-50`. Same function as 4.1 —
  distinct defect: 4.1 is the redundant cost; this is the panic-on-hypothetical-codec-failure
  (4.1's typed fast path also shrinks this surface).
- Issue: two `.expect("... is infallible")` calls convert a hypothetical codec failure into a
  panic in the crate's most-used value builder. The invariant genuinely holds today (msgpack
  encodes every `FilterValue` shape, including non-finite f64, and `QueryValue` decodes any valid
  msgpack), and the comment says so — but this is the exact same "no way to construct a failing
  value today" assumption that #1083 (`batch_tests.rs:417-428`) shows does not age: one new
  `FilterValue` variant with a non-round-trippable payload turns every `doc().set(...)` into a
  WASM-trapping panic.
- Failure scenario: `FilterValue` gains a variant whose serde shape `QueryValue` cannot decode (or
  a codec regression on a float edge); `doc().set("k", v)` panics inside a client builder instead
  of surfacing an error.
- Suggested fix: keep the fast path but map the encode error into a `QueryValue`-decode
  `Error::Syntax` and either make `Doc::set` return `Result<Self, rmp_serde::encode::Error>`
  (matching the `Update`/`Upsert`/`Delete` fallible-build precedent, with `doc!` macro
  `.expect`-ing on top so ergonomics are unchanged) or leave it panicking with an explicit
  `debug_assert` + doc-note referencing the #1083 rationale.

### 6.6 — nit — Guarded `unwrap()`/`expect()` cluster in `TryFrom<&CreateIndex>` is sound but could be total
- File:line: `crates/shamir-query-builder/src/ddl/create_index.rs:772, 778, 815`
  (`itype.unwrap()`), `create_index.rs:838-839` (double `.expect("vector_dim checked Some &
  > 0")`), `create_index.rs:862` (`.expect("sorted index checked to have exactly one field")`).
- Issue: all five sites are guarded invariants (`non_btree` = `matches!(itype, Some(...))` for the
  unwraps; check 4 at line 782 for the dimension; check at line 826 for the single field), so per
  CLAUDE.md they are sanctioned, and the comments name the checks that establish them. Two
  cosmetic observations: lines 838-839 duplicate the same expect message around
  `NonZeroU32::new(...)` when a single `.expect` on the constructor result suffices, and the three
  `itype.unwrap()` sites could bind locals to remove the operator entirely. No behavior change
  implied.
- Failure scenario: none under current code; the guards and construction sites are additionally
  pinned by `index_spec_tests.rs` and the `create_index_matrix.json` fixture.
- Suggested fix: optional tidy-up only — collapse the double expect, and consider a small
  `fn expect_itype(...) -> &str` helper (or `IndexSpec`-carrying type enum) so the invariant lives
  in one place rather than five comments.

*Error-path coverage note (carried forward): all `BuilderError`/`QueryBuildError`/
`ResponseError`/`CreateIndexBuildError` variants are asserted in `src/*/tests/` with happy-path
siblings; `BuildError`'s `UnknownAlias`/`SelfReference`/`AfterPathIgnored` are triggered
end-to-end and `SerializationFailed` is honestly documented as untriggerable through valid builder
inputs (`Display`-only test — an acceptable, explicit gap). The only zero-coverage error-path
behavior is 1.2's silent no-op, which is itself the finding. No resource-cleanup tests are needed:
the crate holds no resources across any fallible boundary.*

## 7. style-claude-md

### 7.1 — medium — `ToWire` trait + blanket impl live directly in `wire/mod.rs`
- File:line: `src/wire/mod.rs:24-48`.
- Issue: CLAUDE.md: "mod.rs files contain re-exports only. Types and logic live in sibling
  files." `wire/mod.rs` instead defines the public `ToWire` trait (two provided methods with real
  logic — the msgpack round-trip in `to_query_value`) plus the blanket `impl<T: Serialize +
  ?Sized> ToWire for T {}`. The module has no sibling implementation file at all; it is the only
  mod.rs in the crate that carries runtime logic.
- Failure scenario: none functional. Structural debt: grep/`git blame` for the trait points at a
  manifest file; anyone extending `wire` (the module already has a `tests/` dir) has no sibling
  file to extend and the documented layout stops matching reality.
- Suggested fix: move the trait + blanket impl verbatim to `src/wire/to_wire.rs` (keeping the
  module doc on the new file) and reduce `wire/mod.rs` to module docs + `mod to_wire;` + `pub use
  to_wire::*;` + `#[cfg(test)] mod tests;`. Zero public-API change.

### 7.2 — medium — All four `macro_rules!` definitions live inline in `macros/mod.rs`
- File:line: `src/macros/mod.rs:24-32` (`doc!`), `:46-51` (`vals!`), `:62-69` (`bind!`),
  `:85-189` (`subscribe!`).
- Issue: same "mod.rs = re-exports only" rule; `macros/mod.rs` is ~190 lines of definitions and
  zero re-exports. `subscribe!` alone spans five match arms. Under one-file-one-export this would
  be four sibling files (or at least `subscribe.rs` separate from the small ones).
- Failure scenario: none functional (`#[macro_export]` macros are crate-root-visible regardless of
  file). Diff-atomicity suffers: a tweak to `subscribe!`'s `deliver:` grammar and a tweak to
  `doc!` land in the same file's blame.
- Suggested fix: split into `macros/doc.rs`, `macros/vals.rs`, `macros/bind.rs`,
  `macros/subscribe.rs`; keep `macros/mod.rs` as `mod`/`pub(crate) use` wiring (verify the
  `#[macro_use] pub mod macros;` ordering in `lib.rs:53-54` still compiles identically).
  Mitigating factor acknowledged: declarative macros are the most conventional mod.rs tenant, so
  if the team prefers, document a macro exception in CLAUDE.md instead of migrating — but today
  the rule as written is violated.

### 7.3 — medium — `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders
- File:line: `src/ddl/access_control.rs` (9 public builder types + 9 ctors); `src/ddl/schema.rs`
  (5 builders + the `field()` DSL, 595 lines, 5 distinct wire ops); `src/ddl/validator.rs` (5
  builders); `src/ddl/auth.rs` (4 builders); `src/ddl/replication.rs` (3 builders + 6 free fns);
  `src/ddl/list.rs` (4 builders + 3 free fns); `src/ddl/migration.rs` (3 builders + 1 fn);
  `src/ddl/buffer_config.rs` (3 builders); `src/ddl/retention.rs` (3 builders);
  `src/ddl/function.rs` (2 builders + 3 free fns) — versus ~15 one-op-per-file siblings
  (`create_db.rs`, `drop_db.rs`, `rename_db.rs`, `create_repo.rs`, `create_table.rs`,
  `drop_table.rs`, `rename_table.rs`, `create_index.rs`, `drop_index.rs`, `rename_index.rs`,
  `describe_table.rs`, `create_index_build_error.rs`, `tokenizer.rs`, `metric.rs`,
  `quantization.rs`, …).
- Issue: CLAUDE.md: "One file = one primary export … If a file defines multiple unrelated public
  types, split them into separate files. This keeps diffs atomic and `git blame` meaningful." The
  module's own dominant pattern is one op per file, which makes the family files the anomaly
  rather than an alternative convention. The rule's "closely-coupled group" carve-out plausibly
  covers the smallest cases (`list.rs` builders all feed the single `ListOp` enum;
  `buffer_config.rs` is one DTO family), but it does not cover `schema.rs` (Set/Add/Remove/Get
  schema ops + a field-rule DSL are four independent op families) or `access_control.rs`
  (chmod/chown/chgrp/groups are unrelated op families).
- Failure scenario: `schema.rs` shows repeated phase-marked accretion (Phase B / C2 / C3 / ②.2a /
  ③.2c comments) — exactly the churn pattern the rule exists to prevent; blame for `FieldBuilder`
  and `GetTableSchemaBuilder` interleaves in one file, and unrelated-schema-task diffs are not
  atomic.
- Suggested fix: at minimum split `schema.rs` (`field.rs`, `set_table_schema.rs`,
  `add_schema_rule.rs`, `remove_schema_rule.rs`, `get_table_schema.rs`) and `access_control.rs`
  (`chmod.rs`, `chown.rs`, `chgrp.rs`, `group_*.rs`, `access_tree.rs`); secondarily `auth.rs` and
  `validator.rs`. Pure mechanical moves — `ddl/mod.rs` already glob-re-exports each sibling (`pub
  use <file>::*;`), so the public API is unchanged. Keep the small single-DTO family files
  (`list.rs`, `buffer_config.rs`, `retention.rs`) as-is, or note the "closely-coupled op family"
  carve-out in CLAUDE.md so the layout is a documented decision rather than drift.

### 7.4 — low — Imports not at top: one production-code site + a pervasive function-local `use` pattern in tests
- File:line: `src/batch/batch.rs:1123` (`use shamir_types::types::value::QueryValue;` inside `fn
  collect_query_refs` — redundant: `QueryValue` is already imported at `batch.rs:10`). Test
  files: `batch/tests/batch_tests.rs:448,460,532,565,576,591,606`;
  `batch/tests/after_tests.rs:24,25,140,193,194`; `batch/tests/when_tests.rs:35,36,59`;
  `batch/tests/call_tests.rs:139`; `batch/tests/sub_batch_tests.rs:142`;
  `macros/tests/q_macro_tests.rs:298,474,576,607,608,620,629,638,654,655,669,670`;
  `select/tests/select_tests.rs:123,157,191`; `write/tests/write_tests.rs:386`;
  `query/tests/query_tests.rs:1036`; `filter/tests/filter_tests.rs:197`;
  `ddl/tests/schema_ddl_tests.rs:551`; `ddl/tests/replication_ddl_tests.rs:268`.
- Issue: CLAUDE.md: "All `use` statements live in the file header … never inside a function or
  block body," with three narrow exceptions (`use super::*;` in a test mod; collision-documented
  single-method trait imports; macro/cfg-gated bodies). None of these sites qualifies: the
  `batch.rs` site duplicates the file-header import; the test sites are per-`#[test]`-function
  imports (several, e.g. `use crate::wire::ToWire;`, are the "trait imported solely to call one
  method" shape but lack the required naming-collision justification comment, and hoisting would
  collide with nothing).
- Failure scenario: none functional. Cost is consistency: the rule as written is absolute, so
  every new test written in the local style deepens the drift, and a future mechanical enforcement
  (or a contributor following CLAUDE.md literally) will flag the whole body of tests at once.
- Suggested fix: delete the `batch.rs:1123` import (the header import already covers it) and hoist
  the test-file `use`s into each file's header import block (few and disjoint per file). A
  dedicated `style:` commit per the CLAUDE.md style-sweep rule.

### 7.5 — low — `cursor` module's tests use a bare `tests.rs` instead of the documented `tests/` directory + manifest
- File:line: `src/cursor.rs:81-82` (`#[cfg(test)] mod tests;`) + `src/cursor/tests.rs` (single
  file).
- Issue: CLAUDE.md's test-organisation layout is "one `tests/` directory per module" with a
  manifest-only `tests/mod.rs`. Every other module in the crate follows it (`query/tests/mod.rs`,
  `wire/tests/mod.rs`, `batch/tests/mod.rs`, `ddl/tests/mod.rs`, …); `cursor` is the sole module
  using the degenerate `cursor.rs` + `cursor/tests.rs` form. Wiring itself
  (`#[cfg(test)] mod tests;`) is fine.
- Failure scenario: none. Discoverability/consistency cost only: tooling or habits tuned to
  `tests/mod.rs` manifests miss cursor's tests.
- Suggested fix: either migrate to `src/cursor/tests/mod.rs` (`pub mod cursor_tests;`) +
  `src/cursor/tests/cursor_tests.rs` (move the file verbatim), or — since it is a single-topic
  file — add a one-line note to CLAUDE.md's test-organisation section blessing the single-file
  degenerate case so the layout is intentional.

### 7.6 — nit — Documentation drift: nonexistent APIs referenced in docs, stale module list *(also flagged: correctness-tdd #8, api-wire-protocol #10 — one root-cause class, three lenses; all nit)*
- File:line: `src/filter/leaf.rs:83-84` (references `val::query_ref(...)`, which does not exist —
  the constructors are `val::qref`/`qref_all` at `val/filter_value.rs:166-182`);
  `src/batch/batch.rs:248-255` (fallible mirrors described as avoiding panics "inside
  `IntoBatchOp`", but `crate::write::Update`/`Upsert`/`Delete` no longer implement `IntoBatchOp`
  at all — only `TryIntoBatchOp`); `src/write/insert.rs:44` (doc says "e.g. from `mpak!({...})`" —
  elsewhere consistently `mpack!`, e.g. `src/write/mod.rs:28,44`, `src/write/upsert.rs:49`);
  `src/lib.rs:33-42` (module list omits the `wire` module entirely, and describes `macros` as only
  "`doc!` / `vals!` declarative macros; `filter!` / `q!` proc-macro re-exports" while
  `macros/mod.rs` also defines `bind!` and `subscribe!`).
- Issue: documentation drift that misdirects the next maintainer: users copy the `value_gte`
  snippet and hit a compile error; the `mpak!` reference sends readers searching for a macro that
  does not exist; the IntoBatchOp doc describes removed impls; a newcomer consulting the crate doc
  misses `wire::ToWire`, `bind!`, and `subscribe!`.
- Failure scenario: purely documentary — compile errors and dead-end searches for the next
  maintainer/user.
- Suggested fix: comment-only fixes: `qref` in the leaf.rs snippet; reword the
  `IntoBatchOp`/`TryIntoBatchOp` split at `batch.rs:248-255`; `mpak!` → `mpack!` at
  `insert.rs:44`; add `wire` + `bind!`/`subscribe!` to the lib.rs inventory.

### 7.7 — nit — Duplicate re-exports: the same items re-exported in both a sibling file and its `mod.rs`
- File:line: `src/select/select_item.rs:8` + `src/select/mod.rs:12` (`AggFunc`,
  `AggregateField`); `src/write/update.rs:10` + `src/write/mod.rs:69` (`UpdateReturnMode`).
- Issue: the sibling-file `pub use` is already scooped up by the mod.rs glob (`pub use
  select_item::*;`), making the explicit duplicate in `mod.rs` redundant — and re-exports per the
  convention belong in exactly one place (the mod.rs manifest), not in implementation files.
  Contrast the correct pattern in the same crate: `val/filter_value.rs:7` (`FnCall`) is
  re-exported only via the sibling.
- Failure scenario: none (both paths resolve to the same item, so no ambiguity error). Minor
  reader confusion about where the re-export is authored.
- Suggested fix: delete the sibling-file `pub use` in `select_item.rs:8` and `update.rs:10`,
  keeping the commented re-exports in `select/mod.rs:12` and `write/mod.rs:69`. Public API
  unchanged.

*Style conformance carried forward: not a single inline `#[cfg(test)] mod tests { ... }` block in
`src/`; topic-split test files; JSON/`mpack!` literals multi-line and indented; no raw
`json!`/`serde_json` in `src/` (builder-only rule trivially satisfied).*

---

## Finding counts

Raw lens-tagged total across the 7 files: **42** (matches the workspace SUMMARY.md's
per-crate row: 0 crit / 3 high / 12 med / 18 low / 9 nit).

| Severity | Lens-tagged findings | Distinct defects | Finding numbers (dedup groups count once) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 3 | 2 | 1.1 + 5.1 (try_build false-reject — one defect, two lenses), 4.1 (Doc::set per-field round-trip) |
| medium | 12 | 10 | 1.2 + 5.2 + 6.2 (after/when silent no-op — one defect, three lenses), 1.3, 3.1, 4.2, 5.3, 5.4, 6.1, 7.1, 7.2, 7.3 |
| low | 18 | 14 | 1.4 + 5.5 (return_only — two lenses), 1.5, 3.2, 4.3, 4.4, 4.5, 5.6, 5.7, 5.9, 6.3 (also absorbs correctness-tdd #6 + security-crypto #3), 6.4, 6.5, 7.4, 7.5 |
| nit | 9 | 6 | 4.6, 5.11, 5.12, 6.6, 7.6 (also absorbs correctness-tdd #8 + api-wire #10), 7.7 |
| **total** | **42** | **32** | |

Cross-severity note: the 6.1 group absorbs api-wire #8 (low-tagged) and correctness-tdd #7
(nit-tagged) into a medium — the error-handling lens carried the strongest severity and fullest
analysis for that defect — which is why the low and nit "distinct" columns don't sum from the
lens-tagged column row-wise.

Deduplicated defect census: **0 critical, 2 high, 10 medium, 14 low, 6 nit = 32 distinct
defects** (42 lens-tagged findings). Dedup groups: 1.1/5.1 · 1.2/5.2/6.2 · 1.4/5.5 ·
6.1/(correctness #7)/(api #8) · 6.3/(correctness #6)/(security #3) · 7.6/(correctness #8)/(api #10).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Fix `try_build`'s nested-batch scoping and the marker rule in the same walker.** Typed arms
   for `BatchOp::Batch`/`BatchOp::ForEach` in `collect_op_query_refs` that collect only `bind`/`over`
   refs against outer aliases and skip the inner batch body (mirroring `planner.rs:308-322`); port
   the #641 marker-map rule into `collect_query_refs`. Red tests first per CLAUDE.md TDD: nested
   batch with internal ref must pass `try_build`; non-marker map with a `"$query"` key must be
   ignored; `try_build` × `sub_batch`/`for_each` coverage (currently zero). Closes **1.1, 5.1,
   1.3** — removes the main force pushing users off the validated path.
2. **Make the `Batch` alias model lossless.** Fallible `after`/`when` (or `debug_assert!` + doc)
   on unregistered aliases; reject duplicate aliases in `add_entry_after` (or rename-with-suffix);
   negative tests for both. Closes **1.2, 5.2, 6.2, 5.3** — ordering edges, conditional-execution
   guards, and whole ops currently vanish silently with `try_build` reporting `Ok`.
3. **Give `Doc::set` the typed fast path.** Route through
   `shamir_query_types::filter::filter_value_to_query_value` with the msgpack round-trip only for
   expression variants. Closes **4.1** (the crate's per-row hot loop) and shrinks 6.5's panic
   surface as a side effect.

**P1 — soon**
4. **Restore the zeroize guarantee where credentials are handled.** Depend on `shamir-types` with
   `crypto` (or a minimal `secret` feature pulling only `zeroize`); optionally gate `SecretString`'s
   definition so an unprotected build can't silently construct one; wrap the password at the
   `create_user` boundary. Closes **3.1, 3.2**.
5. **`try_to_request_via_msgpack` + de-publish the panicking triple.** Add the `try_` form (decode
   error → `Error::Syntax`, per `wire/mod.rs` precedent), gate/rename the panicking variant, and
   reconcile its doc with `BuildError::SerializationFailed`. Closes **6.1** (with correctness #7 +
   api #8 absorbed) and **4.6**.
6. **Validate `return_only` in `try_build`.** New `BuildError::UnknownReturnAlias`; test both
   directions. Closes **1.4, 5.5**.
7. **Enforce the documented DDL contracts.** `BuilderError` variants
   (`MissingImplementation`, `HmacRequired`, `PriorityOutOfRange`, `MissingFieldType`) + fallible
   `build()`s for `CreateFunction`/`CreateValidator`/`BindValidator`/`FieldBuilder`/
   `ReplScopeBuilder`; reword the "HMAC-gated" rustdocs to state the advisory-only truth. Closes
   **6.3** (with correctness #6 + security #3 absorbed).
8. **Batched `rows_as`.** One `to_vec_named` over `Vec<QueryRecord>`, one `from_slice::<Vec<T>>`.
   Closes **4.2**.
9. **Route `subscribe!`/`bind!` through `$crate::`.** Closes **5.4** — restores the
   dependency-hiding contract for WASM guests.
10. **Wrap codec errors in a crate-owned enum** (`WireError::{Encode, Decode}`) instead of leaking
    `rmp_serde` types; fixes the decode-as-encode mislabel. Closes **5.7**; lands naturally with
    item 5.

**P2 — backlog**
11. **Structural style sweep (own commits per CLAUDE.md):** move `ToWire` to `wire/to_wire.rs`
    (7.1); split `macros/mod.rs` into four sibling files or document the macro exception (7.2);
    split `ddl/schema.rs` + `ddl/access_control.rs` (then `auth.rs`, `validator.rs`) or document
    the family-file carve-out (7.3); hoist the ~38 function-local imports (7.4); migrate
    `cursor/tests.rs` to the `tests/` layout or bless the degenerate case in CLAUDE.md (7.5);
    drop the duplicate sibling re-exports (7.7). Closes **7.1-7.5, 7.7**.
12. **Doc-drift bundle (comments-only):** `val::qref` snippet, `IntoBatchOp`/`TryIntoBatchOp`
    reword, `mpak!` → `mpack!`, lib.rs module inventory. Closes **7.6** (with correctness #8 +
    api #10 absorbed).
13. **Perf backlog:** `into_request(mut self)` consuming variant for the send path (4.4); extend
    the #1093 typed fast path to `Call`/`Subscribe` (4.3); document a `switch` case ceiling or the
    O(K²) cost (4.5). Closes **4.3, 4.4, 4.5**.
14. **API-semantics backlog:** pin or reject `switch(vec![], default)`'s empty-`Or` semantics
    (1.5); pagination last-write-wins or `try_build` conflict check (5.6); verify/broaden the
    `lit_u64` ordering contract or add a `Big` constructor (5.9); rename the `select::func`/
    `select::expr` collisions (5.11); document the named batch methods as `op()` aliases or add
    marker newtypes (5.12); collapse the `CreateIndex` double-expect/unwrap cluster (6.6); decide
    `Doc::set`'s final `Result`-vs-documented-invariant posture (6.5); optional `thiserror`
    migration for the five enums (6.4). Closes **1.5, 5.6, 5.9, 5.11, 5.12, 6.4, 6.5, 6.6**.

</details>
