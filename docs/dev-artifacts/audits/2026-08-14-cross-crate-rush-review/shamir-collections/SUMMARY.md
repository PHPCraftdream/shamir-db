<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-collections — SUMMARY revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

No original item is source-proven fixed. Ten distinct coverage/documentation/ergonomics observations remain open; the specific HashDoS amplification claim is unverified despite confirmed client-controlled deterministic hashing. Remove the unsupported runtime-High framing and correct algorithm, removal, capacity and test-oracle explanations.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 19 | 17 | 0 | 0 | 0 | 1 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1-1"></a>

### Claim 1.1 — Entirely untested crate: every documented behavioral contract has zero regression protection

Status: `confirmed-open`. Current risk: `medium`.

No local suite or registration exists. Direct contract coverage is missing, but existing downstream tests and unchanged thin constructors do not justify a demonstrated runtime High.

Evidence: [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16); [scripts/test.sh:104](../../../../../scripts/test.sh#L104); [crates/shamir-types/src/types/tests/value_tests.rs:52](../../../../../crates/shamir-types/src/types/tests/value_tests.rs#L52).

Grouping/duplicate: `correctness-tdd.md#1`. This row is not another independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — Unverifiable "~15-20% faster" performance claim stated as fact in stable rustdoc

Status: `confirmed-open`. Current risk: `low`.

The number remains without supporting comparative benchmark evidence; historical commit prose is not a measurement.

Evidence: [crates/shamir-collections/src/lib.rs:41](../../../../../crates/shamir-collections/src/lib.rs#L41); [crates/shamir-collections/src/lib.rs:45](../../../../../crates/shamir-collections/src/lib.rs#L45); [docs/dev-artifacts/audits/shamir-collections.md:38](../../../../../docs/dev-artifacts/audits/shamir-collections.md#L38).

Grouping/duplicate: `correctness-tdd.md#2`. This row is not another independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — Redundant `use std::cmp::Eq` import

Status: `confirmed-open`. Current risk: `nit`.

Eq remains redundantly imported; Hash is not in the prelude.

Evidence: [crates/shamir-collections/src/lib.rs:13](../../../../../crates/shamir-collections/src/lib.rs#L13); [crates/shamir-collections/src/lib.rs:15](../../../../../crates/shamir-collections/src/lib.rs#L15).

Grouping/duplicate: `correctness-tdd.md#3`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — concurrency-lockfree

Status: `not-applicable`. Current risk: —.

The clean local synchronization assessment stands: no locks, awaits or concurrent state and all helpers select THasher. Capacity construction is not universally O(1).

Evidence: [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:30](../../../../../crates/shamir-collections/src/lib.rs#L30); [crates/shamir-collections/Cargo.toml:9](../../../../../crates/shamir-collections/Cargo.toml#L9); [clippy.toml:39](../../../../../clippy.toml#L39).

<a id="review-3-1"></a>

### Claim 3.1 — Unseeded FxHasher exported as THE workspace hasher is fed client-controlled string keys downstream — precomputable HashDoS amplifier

Status: `unverified`. Current risk: `medium` (provisional; not a confirmed defect).

Authenticated request decoding still builds zero-seeded Fx maps from client names before query limits/permissions. The pinned algorithm contradicts the claimed multiply-xor mechanism, and practical collision amplification was not demonstrated.

Evidence: [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [Cargo.lock:3008](../../../../../Cargo.lock#L3008); [crates/shamir-server/src/db_handler/handler.rs:343](../../../../../crates/shamir-server/src/db_handler/handler.rs#L343); [crates/shamir-server/src/db_handler/handler.rs:502](../../../../../crates/shamir-server/src/db_handler/handler.rs#L502); [crates/shamir-connect/src/server/dispatch.rs:131](../../../../../crates/shamir-connect/src/server/dispatch.rs#L131); [crates/shamir-engine/src/query/batch/batch_execute.rs:869](../../../../../crates/shamir-engine/src/query/batch/batch_execute.rs#L869).

Grouping/duplicate: `security-crypto.md#1`. This row is not another independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Crate-wide `#![allow(clippy::disallowed_types)]`: right to exist, wrong scope, no local justification

Status: `confirmed-open`. Current risk: `nit`.

The sanctioned broad allow and missing local explanation remain. This is future lint containment, not evidence of a present security/runtime defect.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [clippy.toml:39](../../../../../clippy.toml#L39).

Grouping/duplicate: `security-crypto.md#2`. This row is not another independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — `TMap`/`TSet` docs omit the O(N) order-preserving-removal asymmetry; 100+ consumer sites pick a removal strategy blind

Status: `confirmed-open`. Current risk: `medium`.

The alias documentation is unchanged and the cited MVCC flush still repeatedly shift-removes from TMap. Worst-case aggregate shifting is superlinear; production latency was not measured.

Evidence: [crates/shamir-collections/src/lib.rs:19](../../../../../crates/shamir-collections/src/lib.rs#L19); [crates/shamir-collections/src/lib.rs:22](../../../../../crates/shamir-collections/src/lib.rs#L22); [crates/shamir-tx/src/mvcc_store/version_entry.rs:42](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L42); [crates/shamir-tx/src/mvcc_store/version_entry.rs:124](../../../../../crates/shamir-tx/src/mvcc_store/version_entry.rs#L124).

Grouping/duplicate: `performance-hotpath.md#1`. This row is not another independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — the "~15-20% faster" bench claim

Status: `confirmed-open`. Current risk: `nit`.

Same unsupported quantitative rustdoc; no independent defect or supporting comparative run was found.

Evidence: [crates/shamir-collections/src/lib.rs:41](../../../../../crates/shamir-collections/src/lib.rs#L41); [crates/shamir-collections/src/lib.rs:45](../../../../../crates/shamir-collections/src/lib.rs#L45).

Grouping/duplicate: `SUMMARY.md#1.2`. This row is not another independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — No documented serialization/wire contract for `TMap`-backed protocol fields

Status: `confirmed-open`. Current risk: `medium`.

Derived DTO maps retain insertion-order-dependent planning and duplicate coalescing without a leaf contract. Canonical serialization is not required merely to checksum transmitted bytes.

Evidence: [crates/shamir-collections/src/lib.rs:1](../../../../../crates/shamir-collections/src/lib.rs#L1); [crates/shamir-query-types/src/batch/batch_request.rs:87](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L87); [crates/shamir-query-types/src/batch/sub_batch_op.rs:15](../../../../../crates/shamir-query-types/src/batch/sub_batch_op.rs#L15); [crates/shamir-query-types/src/batch/planner.rs:164](../../../../../crates/shamir-query-types/src/batch/planner.rs#L164); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:904](../../../../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L904).

Grouping/duplicate: `api-wire-protocol.md#1`. This row is not another independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — Public API mostly undocumented; `_wc` naming cryptic; doctests disabled

Status: `confirmed-open`. Current risk: `low`.

THasher, constructor docs and a doctest rationale remain absent; naming is unchanged. No functional failure is established.

Evidence: [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16).

Grouping/duplicate: `api-wire-protocol.md#2`. This row is not another independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — Constructor surface is partially redundant and inconsistently adopted

Status: `confirmed-open`. Current risk: `low`.

Multiple valid idioms remain without canonical documentation. The claimed custom-hasher TMap::with_capacity shortcut is not available in pinned IndexMap.

Evidence: [crates/shamir-collections/src/lib.rs:30](../../../../../crates/shamir-collections/src/lib.rs#L30); [Cargo.lock:1783](../../../../../Cargo.lock#L1783); [crates/shamir-query-builder/src/batch/batch.rs:56](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L56); [crates/shamir-types/src/record_view/lens.rs:1064](../../../../../crates/shamir-types/src/record_view/lens.rs#L1064).

Grouping/duplicate: `api-wire-protocol.md#3`. This row is not another independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Half the API missing from the shared façade re-export

Status: `confirmed-open`. Current risk: `low`.

The seven-item façade still omits six Fx items. Collections already exports thirteen public items; this is optional shamir-types façade consistency work.

Evidence: [crates/shamir-types/src/types/common.rs:5](../../../../../crates/shamir-types/src/types/common.rs#L5); [crates/shamir-types/src/record_view/lens.rs:33](../../../../../crates/shamir-types/src/record_view/lens.rs#L33); [crates/shamir-types/src/record_view/lens.rs:34](../../../../../crates/shamir-types/src/record_view/lens.rs#L34); [crates/shamir-collections/src/lib.rs:49](../../../../../crates/shamir-collections/src/lib.rs#L49).

Grouping/duplicate: `api-wire-protocol.md#4`. This row is not another independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — zero in-crate tests, including no serde/ordering pinning

Status: `confirmed-open`. Current risk: `low`.

Same missing local suite. Downstream map equality and deterministic ordered iteration are insufficient oracles for order and builder identity.

Evidence: [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-query-types/src/batch/tests/batch_types_tests.rs:687](../../../../../crates/shamir-query-types/src/batch/tests/batch_types_tests.rs#L687).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — blanket allow without justification comment

Status: `confirmed-open`. Current risk: `nit`.

The attribute still lacks local explanation, although clippy.toml explicitly sanctions it.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [clippy.toml:39](../../../../../clippy.toml#L39).

Grouping/duplicate: `SUMMARY.md#3.2`. This row is not another independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — Infallible capacity constructors can only abort the process; no fallible counterpart exists

Status: `confirmed-open`. Current risk: `low`.

Infallible undocumented allocation remains. Only-abort is false for capacity overflow, and the public raw decoder already forwards attacker-declared counts; remote production reachability remains unverified.

Evidence: [crates/shamir-collections/src/lib.rs:30](../../../../../crates/shamir-collections/src/lib.rs#L30); [crates/shamir-collections/src/lib.rs:54](../../../../../crates/shamir-collections/src/lib.rs#L54); [Cargo.toml:88](../../../../../Cargo.toml#L88); [crates/shamir-types/src/codecs/interned/messagepack.rs:251](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L251); [crates/shamir-types/src/codecs/interned/messagepack.rs:318](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L318).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — zero tests, no error-path surface to test

Status: `confirmed-open`. Current risk: `low`.

No local suite exists. There is no returned error path, but capacity failure is delegated rather than nonexistent.

Evidence: [crates/shamir-collections/src/lib.rs:29](../../../../../crates/shamir-collections/src/lib.rs#L29); [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — no tests anywhere in the pillar-4 anchor crate

Status: `confirmed-open`. Current risk: `low`.

Same local coverage gap; no existing-test layout violation is present.

Evidence: [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — redundant prelude import `std::cmp::Eq`

Status: `confirmed-open`. Current risk: `nit`.

The redundant import remains; speculative future lint failures were not verified.

Evidence: [crates/shamir-collections/src/lib.rs:13](../../../../../crates/shamir-collections/src/lib.rs#L13).

Grouping/duplicate: `SUMMARY.md#1.3`. This row is not another independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — doc/comment coverage inconsistent within lib.rs

Status: `confirmed-open`. Current risk: `nit`.

Both missing public rustdoc and the unexplained local allow remain. Its allow facet separately duplicates 3.2.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25).

Grouping/duplicate: `SUMMARY.md#5.2`. This row is not another independent defect.

## Current fix-plan state

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 10 | 0 | 0 | 0 | 0 | 0 |

A source-fixed item closes only its stated mechanism. Partial items retain the obligations named below; proposed fixes must obey the corrections and current contracts, not merely copy the historical recipe.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`. Current risk: —.

No pinning suite was added. Correct the proposed remove oracle, assert builder identity directly, and compare explicit key sequences for serde order; equal maps/shared-reference encoders do not establish it.

Evidence: [crates/shamir-collections/src/lib.rs:63](../../../../../crates/shamir-collections/src/lib.rs#L63); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16); [crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs:344](../../../../../crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs#L344); [crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs:361](../../../../../crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs#L361).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`. Current risk: —.

Both proposed documentation contracts remain absent. Distinguish TMap map semantics from TSet sequence semantics and require duplicate rejection before coalescing; do not imply canonical bytes are necessary for ordinary checksums.

Evidence: [crates/shamir-collections/src/lib.rs:1](../../../../../crates/shamir-collections/src/lib.rs#L1); [crates/shamir-collections/src/lib.rs:19](../../../../../crates/shamir-collections/src/lib.rs#L19); [crates/shamir-collections/src/lib.rs:22](../../../../../crates/shamir-collections/src/lib.rs#L22); [crates/shamir-query-types/src/batch/batch_request.rs:87](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L87).

<a id="plan-p1-3"></a>

### Plan P1.3 — P1.3

Status: `confirmed-open`. Current risk: —.

No keyed ingress builder or revised trust premise is present. Practical exploit severity needs validation against the pinned algorithm; interning alone is insufficient because raw names are hashed during ID assignment.

Evidence: [CLAUDE.md:345](../../../../../CLAUDE.md#L345); [CLAUDE.md:351](../../../../../CLAUDE.md#L351); [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-server/src/db_handler/handler.rs:343](../../../../../crates/shamir-server/src/db_handler/handler.rs#L343); [crates/shamir-tx/src/layered_interner.rs:95](../../../../../crates/shamir-tx/src/layered_interner.rs#L95).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`. Current risk: —.

The allow remains crate-wide and unexplained locally; clippy.toml still describes the same sanctioned blanket site. Optional containment work remains.

Evidence: [crates/shamir-collections/src/lib.rs:9](../../../../../crates/shamir-collections/src/lib.rs#L9); [clippy.toml:39](../../../../../clippy.toml#L39).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`. Current risk: —.

THasher and constructor rustdoc remain absent; doctests remain disabled without a local rationale. A capacity-name rename is optional breaking-window work, not required for runtime safety.

Evidence: [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:25](../../../../../crates/shamir-collections/src/lib.rs#L25); [crates/shamir-collections/Cargo.toml:16](../../../../../crates/shamir-collections/Cargo.toml#L16).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`. Current risk: —.

Allocation contracts remain undocumented. Correct panic-versus-OOM wording; the claimed no-untrusted-bound-caller condition is false at the public raw decoder, so docs alone do not bound its declared-count allocation.

Evidence: [crates/shamir-collections/src/lib.rs:29](../../../../../crates/shamir-collections/src/lib.rs#L29); [Cargo.toml:88](../../../../../Cargo.toml#L88); [crates/shamir-types/src/codecs/interned/messagepack.rs:251](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L251); [crates/shamir-types/src/codecs/interned/messagepack.rs:318](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L318).

<a id="plan-p2-7"></a>

### Plan P2.7 — P2.7

Status: `confirmed-open`. Current risk: —.

No canonical construction idiom is documented. Keep _wc helpers or spell with_capacity_and_hasher explicitly; the proposed TMap::with_capacity fallback is not equivalent or available.

Evidence: [crates/shamir-collections/src/lib.rs:1](../../../../../crates/shamir-collections/src/lib.rs#L1); [crates/shamir-collections/src/lib.rs:30](../../../../../crates/shamir-collections/src/lib.rs#L30); [crates/shamir-query-builder/src/batch/batch.rs:56](../../../../../crates/shamir-query-builder/src/batch/batch.rs#L56); [crates/shamir-types/src/record_view/lens.rs:1064](../../../../../crates/shamir-types/src/record_view/lens.rs#L1064).

<a id="plan-p2-8"></a>

### Plan P2.8 — P2.8

Status: `confirmed-open`. Current risk: —.

The partial façade remains unchanged. Completing or retiring it is a shamir-types ergonomics decision; the complete collection surface contains thirteen items.

Evidence: [crates/shamir-types/src/types/common.rs:5](../../../../../crates/shamir-types/src/types/common.rs#L5); [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-collections/src/lib.rs:61](../../../../../crates/shamir-collections/src/lib.rs#L61).

<a id="plan-p2-9"></a>

### Plan P2.9 — P2.9

Status: `confirmed-open`. Current risk: —.

Neither alias's quantitative wording was softened and no supporting comparative benchmark was found. No benchmark was executed in this revalidation.

Evidence: [crates/shamir-collections/src/lib.rs:41](../../../../../crates/shamir-collections/src/lib.rs#L41); [crates/shamir-collections/src/lib.rs:45](../../../../../crates/shamir-collections/src/lib.rs#L45); [docs/dev-artifacts/audits/shamir-collections.md:38](../../../../../docs/dev-artifacts/audits/shamir-collections.md#L38).

<a id="plan-p2-10"></a>

### Plan P2.10 — P2.10

Status: `confirmed-open`. Current risk: —.

The redundant Eq import remains; retain the necessary Hash import.

Evidence: [crates/shamir-collections/src/lib.rs:13](../../../../../crates/shamir-collections/src/lib.rs#L13); [crates/shamir-collections/src/lib.rs:15](../../../../../crates/shamir-collections/src/lib.rs#L15).

## Corrections and qualified non-findings

- Retain every original row and its duplicate relationships, but recompute severity/status counts; the old High count and health ranking are not current evidence.
- No item is fixed merely because extraction, lint enforcement, dependency migration or historical gate commits were completed.
- Eight constant-time constructors and one allocation are incorrect blanket guarantees: nonzero ordered-family capacity construction performs O(capacity) work and allocates two backing structures.
- Correct remove to deprecated swap-removal; only shift_remove/shift_take preserve relative order.
- Correct Hash-is-in-prelude, twelve-public-items, top-level-bind and old batch/batch.rs assertions.
- Separate the confirmed deterministic/client-key boundary from unverified practical HashDoS amplification; replace the incorrect pinned-algorithm explanation.
- Qualify the threat model with authentication, rate gating, the frame ceiling, application decoding before query-count checks, and restricted peer-IP control.
- Interning is not a complete HashDoS fix unless its own raw-name hashing boundary is protected.
- Distinguish capacity-overflow unwind from OOM abort. The current raw decoder already violates the all-materialized-capacities premise, but no remote production path was established.
- Existing @types tests are reachable through shamir-types registrations. Their equality and shared-encoder oracles do not independently prove collection builder identity or order.
- The absence of canonical application maps does not prevent checksumming exact transmitted bytes; protocol authentication has its own canonical representation.
- The no-local-locks/no-unsafe/no-I/O/no-query-construction observations remain valid. They do not establish transitive allocation, input-trust or dependency-security guarantees.
- There are no standalone Fix Plan sections in the seven lens reports; their suggested-fix qualifications are covered by the corresponding finding corrections and all ten SUMMARY plan rows.

## Current follow-up order

1. Clarify wire ordering and duplicate handling, with duplicate rejection before map coalescing.
2. Document removal costs and evaluate the remaining repeated MVCC shift_remove path while preserving its output-order contract.
3. Add direct hasher, constructor-capacity, insertion/removal/dedup and serde-order guards with discriminating oracles.
4. Resolve the authenticated-client hashing threat model against rustc-hash 2.1.2; do not claim a demonstrated collision attack or treat interning alone as a complete mitigation.
5. Document allocation failure accurately and bound or fallibly reserve declared counts when the public raw decoder receives untrusted bytes.
6. Remove or substantiate the 15–20% claim, then address public rustdoc, optional façade/lint containment and the Eq nit.

## Coverage and limitations

- All eight documents were read completely; all 18 original findings, SUMMARY finding entries, relevant non-findings and all ten Fix Plan items were reviewed. No TASK_GROUPS document exists for this module.
- Static inspection only: no files changed, child agents launched, git mutations, builds, tests, benchmarks or reproductions performed.
- Pinned indexmap 2.14.0, rustc-hash 2.1.2 and hashbrown 0.17.1 implementations were inspected. Dependency-cache paths are deliberately omitted from evidence.
- Practical collision construction and resource amplification against rustc-hash 2.1.2 were not demonstrated; the specific HashDoS exploit claim remains unverified.
- No supporting committed benchmark was found for the 15–20% lookup claim; actual latency differences remain unverified.
- Exact standard-library source for the repository-pinned Rust 1.94.0 was unavailable. Allocation-failure distinctions are supported by the pinned IndexMap/hashbrown path, not execution of the std-backed constructors.
- No in-tree production caller of the public raw MessagePack decoder was found; its attacker-declared preallocation is source-proven, but remote exploit reachability is unverified.

## Reviewed document inventory

- [correctness-tdd.md](./correctness-tdd.md) — 3 claim decisions; 0 explicit plan items.
- [concurrency-lockfree.md](./concurrency-lockfree.md) — 0 claim decisions; 0 explicit plan items.
- [security-crypto.md](./security-crypto.md) — 2 claim decisions; 0 explicit plan items.
- [performance-hotpath.md](./performance-hotpath.md) — 2 claim decisions; 0 explicit plan items.
- [api-wire-protocol.md](./api-wire-protocol.md) — 6 claim decisions; 0 explicit plan items.
- [error-handling-lifecycle.md](./error-handling-lifecycle.md) — 2 claim decisions; 0 explicit plan items.
- [style-claude-md.md](./style-claude-md.md) — 3 claim decisions; 0 explicit plan items.
- [SUMMARY.md](./SUMMARY.md) — 19 claim decisions; 10 explicit plan items.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-collections — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-collections/` — the workspace's foundational leaf: `THasher`
(`BuildHasherDefault<FxHasher>`, ideology pillar 4) plus four public collection aliases
(`TMap`/`TSet` over `IndexMap`, `TFxMap`/`TFxSet` over `std::collections`) and eight O(1)
constructor fns, consumed by essentially every other crate in the workspace. The entire
crate is two files (`Cargo.toml` + a 63-line `src/lib.rs`); there is no `tests/`, no
`benches/`, no submodule tree.

Review basis: the seven 2026-08-14 lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and synthesized read-only (no build/test/lint
commands; no source files modified). Structure/tone calibrated against the two completed
exemplars, `shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`;
workspace-level context (rank #21, "solid with isolated gaps", 18 lens-tagged findings)
taken from the top-level `SUMMARY.md`. Spot-checks against the actual source
(`src/lib.rs`, `Cargo.toml`, `clippy.toml:36-44`) confirmed every cited file:line.

## Executive summary

The crate is structurally clean — zero locks, zero `unsafe`, zero panic sites, no
fallible API, Fx hashing pinned at the type level on every exported structure, and the
crate-level `#![allow(clippy::disallowed_types)]` is the allow-site explicitly sanctioned
verbatim by `clippy.toml` — but it ships **zero tests** while anchoring pillar 4 for all
23 crates, so the two guarantees everything downstream leans on (Fx hasher identity,
insertion-order iteration) have no regression net; a refactor that silently breaks either
still passes `./scripts/test.sh @types` green because the scope runs zero assertions for
this crate. Fix first: (1) add the missing pinning test suite (the crate's only high),
(2) document the two load-bearing contracts consumers already rely on blindly — the wire
semantics of `TMap`-backed DTO fields and the O(N) order-preserving-removal cost —
(3) close the HashDoS premise gap: the unseeded, non-keyed `THasher` minted here is fed
client-controlled string keys at demonstrable downstream sites, breaking pillar 4's
"trusted inputs only" assumption.

---

## 1. correctness-tdd

### 1.1 — high — Entirely untested crate: every documented behavioral contract has zero regression protection
*(primary; also flagged by api-wire-protocol [5.5], error-handling-lifecycle [6.2], and style-claude-md [7.1] — one defect, four lenses)*
- **File:line:** `crates/shamir-collections/src/lib.rs:1-63` (whole crate — verified: no
  `src/tests/`, no integration `tests/`, no `benches/`); `Cargo.toml:16`
  (`[lib] doctest = false`).
- **Issue:** CLAUDE.md makes TDD normative ("write a failing `#[tokio::test]` first") and
  prescribes the `src/tests/` manifest layout; none of that process left an artefact here,
  and `doctest = false` disables even doc-level checks. The following load-bearing
  contracts are pinned by nothing:
  - `THasher = BuildHasherDefault<FxHasher>` (lib.rs:17) is the ideological anchor cited
    by `clippy.toml`'s disallowed-types ban and consumed workspace-wide (shamir-engine,
    shamir-tx, shamir-index, shamir-server, shamir-storage, shamir-db). No failing-if-swapped
    guard exists — a refactor back to `RandomState` compiles and passes `@types` silently.
  - `TMap`/`TSet` insertion-order iteration (lib.rs:3, 19-23 doc promise), relied upon by
    `shamir-query-types/src/batch/planner.rs` topological sort and `batch_execute.rs`'s
    order-preserving `TMap<String, QueryResult>` accumulation — including the sharp edges
    between order-preserving O(n) `remove`/`shift_remove` and order-scrambling O(1)
    `swap_remove` (see 4.1), and dedup keeping first-insert position.
  - `_wc` constructors reserve ≥ requested capacity (lib.rs:29-38, 53-63).
  - serde round-trip preserves entry order (indexmap `serde` feature deliberately enabled,
    `Cargo.toml:10`; wire DTOs in `shamir-query-types` serialize these maps — see 5.1).
  The error-handling lens adds the lifecycle framing: a drift here surfaces as
  nondeterministic-iteration bugs *in other crates*, far from the cause and misattributed
  to engine/query logic.
- **Failure scenario:** an indexmap upgrade or an in-place refactor changes ordering
  semantics or hasher identity; batch-plan determinism and result/response ordering degrade
  silently across dependent crates while `./scripts/test.sh @types` reports green for this
  crate — CI has no signal, and the regression surfaces only as flaky downstream behavior.
- **Suggested fix:** add `src/tests/mod.rs` (manifest-only, per convention) wired via
  `#[cfg(test)] mod tests;`, with small pure-sync topic files: `hasher_tests.rs` (assert the
  builder type is Fx; two identically-keyed `TFxMap`s hash-insert consistently),
  `tmap_order_tests.rs` (insertion order after overwrite / `remove` / `shift_remove`;
  document `swap_remove` scrambling), `tset_tests.rs` (dedup keeps first-insert position),
  `capacity_tests.rs` (`_wc` len==0 / capacity ≥ n), and `serde_roundtrip_tests.rs`
  (order preserved through JSON/msgpack). Sub-second, fully inside the existing `@types`
  scope. Red-first per CLAUDE.md where a fix is involved.

### 1.2 — low — Unverifiable "~15-20% faster" performance claim stated as fact in stable rustdoc
*(primary; also flagged by performance-hotpath [4.2, rated nit there] — one defect, two lenses)*
- **File:line:** `crates/shamir-collections/src/lib.rs:41-42, 45-46`.
- **Issue:** "~15-20% faster than TMap/TSet" is presented as an established measurement,
  but it originates as an *expected* effect in a planning doc
  (`docs/dev-artifacts/audits/shamir-collections.md:16` — "Ожидаемый эффект −15-20% lookup",
  probability rated Низкая). The crate has no `[[bench]]` target and no committed harness
  run anywhere substantiates the number; CLAUDE.md requires perf conclusions come from real
  runs through `bench_scale_tool`. Directionally plausible (IndexMap pays a double-structure
  lookup + index indirection vs std's flat table), so this is a credibility issue, not a
  correctness one.
- **Failure scenario:** none at runtime; future contributors treat the figure as
  benchmarked truth and churn (or decline to fix) hot-path call sites on an unmeasured basis.
- **Suggested fix:** either (a) soften to "expected faster: see
  docs/dev-artifacts/audits/shamir-collections.md (#2)", or (b) measure once with
  `benches/fx_vs_index_lookup.rs` via `bench_scale_tool::Harness` (isolated bench target
  dir per CLAUDE.md) and cite the run in the doc comment.

### 1.3 — nit — Redundant `use std::cmp::Eq` import
*(primary; also flagged by style-claude-md [7.2] and mentioned within api-wire-protocol's test finding)*
- **File:line:** `crates/shamir-collections/src/lib.rs:13`.
- **Issue:** `Eq` (and `Hash`, imported separately) are both in the prelude;
  `std::cmp::Eq` adds noise with no scoping benefit. Imports are correctly top-of-file —
  just superfluous. The style lens adds: redundant-prelude imports can flip into
  `unused_imports` gate noise on a toolchain upgrade.
- **Suggested fix:** delete line 13; keep the plain `Eq`/`Hash` names in the generic bounds.

## 2. concurrency-lockfree

**Clean — no findings for this theme.** The crate contains zero locks (no
`std::sync::Mutex`/`RwLock`, no `parking_lot` — neither imported nor in
`[dependencies]`, which lists only `indexmap` and `rustc-hash`), zero `.await`s, and zero
`scc`/`dashmap` usages, so none of the lens failure modes (hot-path lock, lock across
`.await`, O(N) `scc::*::len()` without ack) can occur. Against the five pillars it is
actively compliant rather than merely silent: `THasher` pins every exported structure at
the type level (`RandomState` is unreachable through this API) and at every constructor
(`with_hasher(THasher::default())` / `with_capacity_and_hasher` — the explicitly-hashed
forms `clippy.toml` whitelists), while all helpers are O(1)/O(capacity) preallocations.
Two items checked and cleared: the crate-level allow (lib.rs:9) is the sanctioned
exception, not lint-masking (see 3.2 for its scoping nit); and the absence of
`scc`/`DashMap` constructors from this crate matches its documented dependency-light-leaf
design intent, not a pillar gap.

## 3. security-crypto

No local crypto/injection surface: no auth, HMAC/SCRAM/TLS, `unsafe` (grep-verified zero
hits), parsing, path assembly, or I/O. Dependency hygiene is good (indexmap 2.14.0,
rustc-hash 2.1.2 with the in-manifest RUSTSEC-2025-0057 rationale comment,
`Cargo.toml:11-12`). The theme exposure is *exported* rather than coded:

### 3.1 — medium — Unseeded FxHasher exported as THE workspace hasher is fed client-controlled string keys downstream — precomputable HashDoS amplifier
- **File:line:** `crates/shamir-collections/src/lib.rs:17` (`pub type THasher =
  BuildHasherDefault<FxHasher>`); consumer sites violating the trusted-input premise:
  `crates/shamir-engine/src/query/batch/batch_execute.rs:130,357` (`params: &TMap<String,
  QueryValue>`), `:350,435,463` (`queries: &TMap<String, QueryEntry>`), `:712`
  (`TFxSet<String>` over result names); `crates/shamir-query-builder/src/batch/batch.rs:33`;
  `crates/shamir-tx/src/tx_context.rs:207` (`interner_overlay: scc::HashMap<String, u64,
  THasher>` keyed by raw field-name strings); `crates/shamir-server/src/conn_limiter.rs:140`
  (`DashMap<IpAddr, AtomicUsize, THasher>`).
- **Issue:** `BuildHasherDefault<FxHasher>` always seeds state at zero and FxHash is a
  non-keyed multiply-xor construction with no final avalanche — its 64-bit outputs are
  trivially collidable offline (classic HashDoS family; craft-once, reuse-forever, because
  unlike `RandomState` the seed never changes across processes or restarts). CLAUDE.md
  pillar 4 trades that protection away on the premise *"we don't accept untrusted hash
  inputs here"* — a premise that is factually broken at the sites above: query **params**
  and **alias/result names** in batch requests are deserialized from client payloads and
  become `String` keys of `TMap`/`TFxSet` built on `THasher`. This crate cannot enforce the
  boundary — it is where the primitive originates, which is why the finding is reported here.
- **Failure scenario:** an attacker submits a batch whose parameter names form an FxHash
  collision set (precomputed once, valid against every deployment and restart). All keys
  collapse into one hash bucket; insertion and lookup degrade from O(1) toward O(N²) per
  request. Repeating with modestly growing N turns each connection into disproportionate
  CPU load on the engine's batch-execute hot path — a cheap, persistent resource-exhaustion
  vector against a multi-connection server.
- **Suggested fix:** do not roll back pillar 4 globally; close the boundary: (a) map
  client-supplied strings to interned `u64` ids (an interner already exists server-side)
  *before* they become hash-map keys, or (b) use a seeded/keyed builder (`RandomState`,
  random-seeded ahash) at exactly the few client-string-keyed sites, each carrying the
  sanctioned inline justification comment; and (c) amend CLAUDE.md pillar 4 and this
  crate's docs to name *which* upstream inputs are considered trusted, so the assumption
  stops being silently inherited by new call sites. A request-shape cap on param-count is
  a complementary mitigation, not a substitute.

### 3.2 — low — Crate-wide `#![allow(clippy::disallowed_types)]`: right to exist, wrong scope, no local justification
*(primary; also flagged by api-wire-protocol [5.6, rated nit there] and — as its
allow-comment aspect — by style-claude-md [7.3] — one defect, three lenses)*
- **File:line:** `crates/shamir-collections/src/lib.rs:9`; sanction text at
  `clippy.toml:36-44` ("The ONE sanctioned allow-site …").
- **Issue:** the attribute is per-convention, not a violation — but its scoping is
  crate-wide while the need is item-local (only lines 11-63 touch the banned
  `std::collections::{HashMap, HashSet}`). Any *future* code added to this crate also
  silently escapes the deny-level ban — e.g. a stray helper reaching for
  `std::collections::hash_map::RandomState` would produce zero diagnostics in exactly the
  crate least supervised by consumer review. And a reader standing in this file sees an
  unexplained blanket suppression; the repo's annotation culture (`#[allow(...)] // <why>`)
  points at an inline justification.
- **Failure scenario:** none today; a future lint-bypassing addition to this file compiles
  silently where the rest of the workspace is denied.
- **Suggested fix:** narrow the allow to the items that require it (per-item
  `#[allow(clippy::disallowed_types)]` on the two alias groups and their constructors) so
  the rest of the crate keeps the deny-by-default posture, add the one-line why pointing at
  `clippy.toml`'s section, and update the "ONE sanctioned allow-site" wording to match.
  Cosmetic effort, durable lint assurance.

## 4. performance-hotpath

Structurally clean against the O(x→0) pillar: eight O(1) constructors, no loops, no
locks, no buffering in the crate itself; `_wc` variants exist for pre-reservation. The
substantive gap is interface documentation at the single canonical definition point:

### 4.1 — medium — `TMap`/`TSet` docs omit the O(N) order-preserving-removal asymmetry; 100+ consumer sites pick a removal strategy blind
- **File:line:** `crates/shamir-collections/src/lib.rs:19-23` (alias doc comments);
  live impact surface: `crates/shamir-tx/src/mvcc_store/version_entry.rs:124`.
- **Issue:** the aliases are documented only as "Ordered map/set that maintains insertion
  order for predictable iteration." In IndexMap, order-preserving removals
  (`shift_remove`/`shift_take`) are **O(N)** — they memmove every subsequent entry down
  *and* decrement its stored index — while `swap_remove`/`swap_take` are O(1) but scramble
  order. This asymptotic asymmetry is invisible at every `use shamir_collections::…` site;
  these doc comments are the one place the cost could be documented once for all consumers.
  This is exactly pillar 3's "avoid hidden O(N)/O(N²) in helpers" trap: the alias looks
  like a drop-in map, so an author reaches for `.shift_remove()` expecting O(1).
- **Failure scenario (real call site, this workspace):** `OverlayWinners = TMap<Bytes,
  (u64, Bytes)>` (`version_entry.rs:42`) backs the streaming CURRENT-scan group-by;
  `flush_group` calls `overlay.shift_remove(&key)` once per history group matched
  (`version_entry.rs:124`). With N overlay winners and K matched groups the cost is
  Σ O(N−i) ≈ O(N·K); removing entries near the front of the insertion order on a large
  pending-write window makes the merge super-linear. The caller-side fix is owned by
  shamir-tx, but the root interface knowledge belongs here.
- **Suggested fix:** two sentences on `TMap`/`TSet` at lib.rs:19-23, e.g.:
  "Order-preserving removal (`shift_remove`/`shift_take`) is O(n): it shifts all later
  entries. On hot paths prefer `swap_remove`/`swap_take` (O(1), changes iteration order)
  or drain/bulk-build instead of per-element shifts." Zero runtime change.

### 4.2 — *(primary: same as 1.2)* — the "~15-20% faster" bench claim
- (Full write-up at 1.2. Listed here because this lens filed it independently [as a nit]:
  an unverifiable number steers hot-path authors by authority rather than measurement;
  this lens's addendum is that a `benches/tmap_vs_tfx_lookup.rs` harness would also settle
  which of the two alias families genuinely belongs on which hot path.)

## 5. api-wire-protocol

Compliant on query-construction: no JSON assembly, no hand-built filters, no
builder-bypass surface. The main theme gap sits at the seam with `shamir-query-types`:

### 5.1 — medium — No documented serialization/wire contract for `TMap`-backed protocol fields
- **File:line:** `crates/shamir-collections/src/lib.rs:20` (+ `Cargo.toml:10`);
  consumer truth: `shamir-query-types/src/batch/batch.rs:33,38` (`queries: TMap<String,
  QueryEntry>`, `interner_epochs: TMap<String, u64>`), derived wire structs in
  `shamir-query-types/src/wire/db_message.rs:29,280` carrying these maps into every
  client/server message.
- **Issue:** the crate owns the abstraction and enables indexmap's `serde` feature, yet
  documents nothing about what that means on the wire: whether insertion order is part of
  the contract, that order survives round-trip *only* through order-preserving
  formats/serializers, or that duplicate keys in an untrusted payload silently coalesce
  (last value wins, first position retained) instead of being rejected.
- **Failure scenario:** a non-Rust guest/WASM host or proxy that round-trips requests
  through an unordered map representation (or canonicalizing JSON/MessagePack tooling)
  reorders entries; alias-keyed batch semantics whose execution depends on insertion
  sequence change silently, and two ops sharing an alias in one decoded request merge
  instead of erroring. "Checksums everywhere" (goal 4) cannot be extended over such
  payloads because byte-level canonical form is undefined for these fields.
- **Suggested fix:** add a crate-level doc section defining the wire semantics of
  `TMap`/`TSet` when serialized: insertion order is carried by order-preserving formats
  only; duplicate-key behavior is last-wins and MUST be validated upstream; no
  cross-language canonical form. If alias uniqueness/order carries semantic weight in
  `BatchRequest`, recommend `Vec<(K, V)>` pairs for those specific DTO fields rather than
  a hash map.

### 5.2 — low — Public API mostly undocumented; `_wc` naming cryptic; doctests disabled
*(primary; also flagged by style-claude-md [7.3, as its doc-coverage nit] — one defect, two lenses)*
- **File:line:** `crates/shamir-collections/src/lib.rs:17-63` (+ `Cargo.toml:16`).
- **Issue:** `THasher` — the single most-relied-upon export (workspace pillar 4; imported
  by shamir-tx, shamir-engine, shamir-index, shamir-server, shamir-db) — has zero rustdoc;
  the DOS-protection-vs-speed rationale lives only in CLAUDE.md. The eight constructor
  functions have no doc comments (spot-checked), and names like `new_map_wc` require
  guessing ("with capacity"). `[lib] doctest = false` guarantees even future examples
  would not be compile-checked. For a crate whose entire product is its public interface,
  bare signatures are thin documentation.
- **Failure scenario:** none functional; discoverability/misuse cost (e.g. a contributor
  reaching for `std::collections::HashMap::new()` habits instead of the blessed
  constructors; IDE hover on the flagship export shows nothing).
- **Suggested fix:** `///` docs on `THasher` (rationale + pillar-4 pointer), each
  constructor, and rename `_wc` → a `with_capacity` suffix spelling at the next natural
  breaking window; re-enable doctests or state why they stay off.

### 5.3 — low — Constructor surface is partially redundant and inconsistently adopted
- **File:line:** `crates/shamir-collections/src/lib.rs:25-63`.
- **Issue:** all eight free functions duplicate paths already available on the exported
  types: `THasher` satisfies `BuildHasher + Default`, so `TMap::<K,V>::default()`,
  `TMap::with_capacity(n)`, `TFxSet::<T>::default()` etc. work identically — the ctors add
  ergonomics only, not safety (the aliases already pin the hasher). Workspace usage shows
  three coexisting idioms for identical construction: `new_map()`
  (`shamir-query-builder/src/batch/batch.rs:56`), `TMap::default()` (planner tests; engine
  `p1059_online_create_index_tests.rs:117`), and the fully spelled
  `indexmap::IndexMap<String, QueryValue, shamir_collections::THasher>` bypassing the alias
  entirely (`shamir-engine/src/query/read/aggregate.rs:925`), plus
  `TFxMap::with_capacity_and_hasher(n, THasher::default())`
  (`shamir-types/src/record_view/lens.rs:1064`).
- **Failure scenario:** none at runtime; API-discoverability fragmentation makes the ctor
  set look authoritative while real code bypasses it (and vice versa), and future edits
  have no single idiom to conform to.
- **Suggested fix:** declare one canonical idiom in the crate-level doc. Either keep the
  ctors as the blessed form (then fix the bypass call sites and document that
  `Default`/`with_capacity` are equivalent fallbacks) or drop the duplicate fns in favor
  of `.default()`/`.with_capacity()`. Right moment: whenever this crate's API next changes.

### 5.4 — low — Half the API missing from the shared façade re-export
- **File:line:** `crates/shamir-collections/src/lib.rs:43-62` vs
  `crates/shamir-types/src/types/common.rs:5` (shared blame with `shamir-types`; root
  cause here, since this crate should expose all twelve items as one coherent group).
- **Issue:** `shamir-types::types::common` presents itself as the façade but re-exports
  only `{new_map, new_map_wc, new_set, new_set_wc, TMap, TSet, THasher}` — omitting
  `TFxMap`, `TFxSet`, and their four constructors. Files therefore need twin imports in
  one header, e.g. `shamir-types/src/record_view/lens.rs:33-34` (`crate::types::common::
  THasher` **and** `shamir_collections::TFxMap`) and `codecs/interned/messagepack.rs:14+22`.
- **Failure scenario:** none; perpetual import friction and inconsistent lint/config drift
  risk if the two sources ever diverge (e.g. a hasher swap done in one path only).
- **Suggested fix:** make `common.rs` re-export the full set (all 12 items) or stop
  maintaining the partial façade and standardize on direct `shamir_collections::*` imports.

### 5.5 — *(primary: same as 1.1)* — zero in-crate tests, including no serde/ordering pinning
- (Full write-up at 1.1. This lens's addendum, folded in: the serde round-trip test should
  also assert the *documented duplicate-key behavior* once 5.1 documents it, and the
  hasher-wiring test should be observable via deterministic iteration of equal-priority
  keys.)

### 5.6 — *(primary: same as 3.2)* — blanket allow without justification comment
- (Full write-up at 3.2; this lens filed the same item at nit severity.)

## 6. error-handling-lifecycle

Vacuously clean on most of this theme: no `Result`-bearing (i.e. fallible) API, no error
enum to design, no I/O, no locks, no `Drop`-managed state, no resources acquired; static
scan found zero explicit panic sites (`unwrap`/`expect`/`panic!`/`assert`/`todo!`) and no
`anyhow`/`Box<dyn Error>` leakage. The honest residue:

### 6.1 — low — Infallible capacity constructors can only abort the process; no fallible counterpart exists
- **File:line:** `crates/shamir-collections/src/lib.rs:29-31, 37-39, 53-55, 61-63`
  (`new_map_wc`, `new_set_wc`, `new_fx_map_wc`, `new_fx_set_wc`).
- **Issue:** all four `_wc` constructors take a caller-supplied `capacity: usize` and call
  `IndexMap::with_capacity_and_hasher` / `HashMap::with_capacity_and_hasher`, which on
  allocation failure (or integer overflow in the capacity computation) invoke
  `handle_alloc_error` — **panic/abort**, not `Result`. CLAUDE.md's rule ("Return
  `Result<T, E>`. Avoid `panic!` outside invariant violations") cannot be satisfied by
  these fns under a hostile-capacity scenario. Mitigating context, verified by
  workspace-wide grep of all ~100 call sites: every current capacity argument is a literal
  (0-10) or `.len()` of an already-materialized in-memory collection, so real OOM at these
  sites implies the process was already over-committed; no call site passes an
  untrusted/user-derived number directly.
- **Failure scenario:** a future caller derives `capacity` from an untrusted bound (client
  batch size hint, advertised manifest count, config knob) without pre-clamping — e.g.
  `new_fx_set_wc(manifest.files.len())` where `files.len()` comes from a parsed,
  attacker-influenced backup manifest before validation → process abort instead of a
  recoverable error surfaced to the operator.
- **Suggested fix:** either (a) document the infallible-allocation contract on each `_wc`
  doc-comment ("panics via alloc failure, like `std`; pass clamped bounds") — acceptable
  alone given the call-site audit — or (b) add `try_new_*_wc` variants returning
  `Result<T, TryReserveError>` and note which is intended for untrusted-bound callers.

### 6.2 — *(primary: same as 1.1)* — zero tests, no error-path surface to test
- (Full write-up at 1.1. This lens's verdict: there are no error paths, so no *error-path*
  tests are missing — but the missing-tests judgment stands at low severity because the fix
  is trivial and the blast radius workspace-wide.)

## 7. style-claude-md

Essentially conformant: no `mod.rs` files exist (rule vacuously satisfied), all `use`
statements at file top, no inline `#[cfg(test)] mod tests` blocks, and the single
`lib.rs` legitimately qualifies as one closely-coupled group under the "one file = one
primary export" exception (a strict sibling-file split would contradict the 64-line
reality of the leaf — recorded as a judgment call, not filed). All three of this lens's
findings dedupe into primaries above:

### 7.1 — *(primary: same as 1.1)* — no tests anywhere in the pillar-4 anchor crate
- (Full write-up at 1.1; this lens's layout framing: zero tests violates no test-*layout*
  rule — those govern where tests live once they exist — but the crate has no executable
  statement of its two core guarantees, Fx hashing and shared host/guest insertion order.)

### 7.2 — *(primary: same as 1.3)* — redundant prelude import `std::cmp::Eq`
- (Full write-up at 1.3.)

### 7.3 — *(primary: same as 5.2, with its allow-comment aspect folded into 3.2)* — doc/comment coverage inconsistent within lib.rs
- (Full write-up at 5.2 for the missing `THasher`/constructor rustdoc and at 3.2 for the
  unexplained line-9 allow; this lens filed the combination as one nit, counted once here.)

---

## Finding counts

| Severity | Lens-tagged findings | Distinct defects after dedup | Dedup groups |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 1 | 1 | 1.1 (×4 lenses: correctness, api, error, style) |
| medium | 3 | 3 | 3.1 (HashDoS) · 4.1 (O(N) removal doc) · 5.1 (wire contract) |
| low | 9 | 6 | 1.2 (×2: correctness, perf) · 3.2 (×3: security, api, style) · 5.2 (×2: api, style) · 5.3 · 5.4 · 6.1 |
| nit | 5 | 1 | 1.3 (×2 standalone: correctness, style; also a sub-item of api's 5.5) |
| **total** | **18** | **11** | 7 duplicate flags folded |

Deduplicated defect census: **0 critical, 1 high, 3 medium, 6 low, 1 nit = 11 distinct
defects** (18 lens-tagged findings). The lens-tagged totals match the workspace
`SUMMARY.md` per-crate row for shamir-collections (0 / 1 / 3 / 9 / 5 = 18) and its
Health-Scorecard verdict (*"solid with isolated gaps — zero tests, but on the pillar-4
anchor whose failures propagate workspace-wide"*, rank #21 — with the summary's own
blast-radius caveat that low totals at the dependency-graph base surface in *other*
crates' behavior).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Add the missing pinning test suite.** `src/tests/mod.rs` manifest + topic files
   (hasher identity, insertion order across insert/overwrite/`remove`/`shift_remove`/
   `swap_remove`, set dedup keeps first position, `_wc` capacity ≥ n, serde round-trip
   order preservation), run inside the existing `@types` scope. Closes **1.1** (and with
   it **5.5, 6.2, 7.1**) — the crate's only high; today `@types` executes zero assertions
   for this crate while the workspace's determinism guarantees hang on this file.
2. **Document the two contracts consumers already rely on** (rustdoc-only, zero runtime
   risk, at the single canonical definition point): (a) the wire/serialization semantics
   of `TMap`/`TSet`-backed DTO fields — order carried only by order-preserving formats,
   duplicate keys last-wins and must be validated upstream, no cross-language canonical
   form, `Vec<(K,V)>` recommended where alias order is semantic (closes **5.1**); (b) the
   O(N) `shift_remove`/O(1) `swap_remove` asymmetry on `TMap`/`TSet` (closes **4.1**).

**P1 — soon**
3. **Close the HashDoS premise gap (3.1):** intern client-supplied strings to `u64` ids
   before they become hash-map keys (or use a seeded/keyed builder at exactly the
   client-string-keyed sites), and amend CLAUDE.md pillar 4 + this crate's docs to name
   which upstream inputs are trusted. The code changes mostly live in consumer crates
   (shamir-engine/shamir-tx/shamir-server own their call sites), but this crate owns the
   primitive and the premise.
4. **Narrow the blanket allow + add its why (3.2, 7.3-partial):** per-item
   `#[allow(clippy::disallowed_types)]` on the alias groups/constructors, one-line
   justification comment on the attr, `clippy.toml` wording updated.
5. **Document the public API (5.2, 7.3):** `THasher` rationale doc, doc lines on the eight
   constructors, decide doctest policy (`doctest = false` → on, or documented rationale).
6. **Document the `_wc` infallible-allocation contract (6.1):** one doc line per `_wc` fn
   ("panics via alloc failure, like `std`; pass clamped bounds"); `try_new_*_wc` variants
   only if an untrusted-bound caller actually appears.

**P2 — backlog**
7. **Declare one canonical construction idiom (5.3):** blessed ctors vs `Default`/
   `with_capacity`, fix or accept the bypass call sites; natural moment is the next API
   change (together with any `_wc` rename from item 5).
8. **Complete or retire the `shamir-types` façade re-export (5.4)** — shared item with a
   shamir-types/api owner; this crate should expose all twelve public items as one group
   so neither split is load-bearing.
9. **Soften or measure the "~15-20%" claim (1.2):** a one-line wording change now, or a
   `benches/fx_vs_index_lookup.rs` run via `bench_scale_tool::Harness` with the isolated
   bench target dir.
10. **Style residue:** delete the redundant `use std::cmp::Eq;` (closes **1.3, 7.2**).

</details>
