<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-types — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The reported allocation/traversal mechanisms are present. Their runtime importance is unmeasured, so medium/high latency conclusions are unsupported. Plan hoisting and authorization deduplication remove specific historical multipliers without removing all collection costs.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 8 | 1 | 1 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `BatchOp::deserialize` — triple codec round-trip + key clones + linear dispatch chain per op

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

QueryValue buffering, key cloning, re-encoding and typed decoding occur per op. The first stage may consume buffered serde Content rather than raw bytes, so triple codec round-trip is imprecise. Only planner benchmarking is registered; workload byte/latency estimates were not measured.

Evidence: [crates/shamir-query-types/src/batch/batch_op.rs:262](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L262); [crates/shamir-query-types/src/batch/batch_op.rs:266](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L266); [crates/shamir-query-types/src/batch/batch_op.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L277); [crates/shamir-query-types/Cargo.toml:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/Cargo.toml#L47).

<a id="review-2"></a>

### Claim 2 — `InsertedRecord::serialize` — per-record `Vec` collect + sort + base58, contradicting the "allocation-free" module claim

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each nonempty map serialization collects references into a Vec and sorts them; present ids allocate rendered text. This contradicts a broad allocation-free reading, not the narrower no-intermediate-map construction guarantee. Replication fanout serialization multipliers were not traced.

Evidence: [crates/shamir-query-types/src/write/inserted_record.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L1); [crates/shamir-query-types/src/write/inserted_record.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L11); [crates/shamir-query-types/src/write/inserted_record.rs:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L32); [crates/shamir-query-types/src/write/inserted_record.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/write/inserted_record.rs#L39).

<a id="review-3"></a>

### Claim 3 — Filter depth guard does not cover `FilterValue::Cond` nesting — unbounded deserialize-time recursion

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

The combined iterative checker now traverses Cond, Expr, FnCall and Array operands. The separate unlimited-decoder half was already false under rmp-serde 1.3.1, not fixed by this traversal change.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:321](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L321); [crates/shamir-query-types/src/filter/filter_enum.rs:342](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L342); [crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs#L256); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

Grouping/duplicate: [security-crypto.md#3](security-crypto.md#review-3). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — `FilterValue` — 13-variant `#[serde(untagged)]` enum: content buffering + ~6 failed map-shaped trials per marker value

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

serde_derive 1.0.228 generates ContentVisitor buffering followed by ordered ContentRefDeserializer attempts. There are thirteen variants, but zero preceding map-shaped marker variants for FieldRef and five for Param. A universal six failed map trials or sevenfold latency is unsupported. [Pinned derivation](https://docs.rs/crate/serde_derive/1.0.228/source/src/de/enum_untagged.rs).

Evidence: [crates/shamir-query-types/src/filter/filter_value.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L9); [crates/shamir-query-types/src/filter/filter_value.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L48); [crates/shamir-query-types/src/filter/filter_value.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_value.rs#L77); [Cargo.lock:3233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3233).

<a id="review-5"></a>

### Claim 5 — `QueryRecord::get_value_{i64,u64,bool}` — deep-clones the whole `Inserted` record per scalar lookup

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

i64/u64/bool Inserted arms call get_value_owned; that path clones the entire fields tree through as_value, then clones the chosen value. Direct borrowing as already used by get_value_str preserves semantics. Production hot-call frequency was not established.

Evidence: [crates/shamir-query-types/src/read/query_record.rs:192](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L192); [crates/shamir-query-types/src/read/query_record.rs:218](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L218); [crates/shamir-query-types/src/read/query_record.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L238); [crates/shamir-query-types/src/read/query_record.rs:246](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L246); [crates/shamir-query-types/src/read/query_record.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L277).

<a id="review-6"></a>

### Claim 6 — Batch planner — redundant alias-set clone and repeated String re-cloning through the plan

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

aliases duplicates query keys; alias_order, dependency/provenance maps and stages own additional strings. The current ForEach path plans once, and registered resolver-count tests discriminate reintroduced per-iteration validation. Draining provenance would be unsafe if it destroys the required edge metadata.

Evidence: [crates/shamir-query-types/src/batch/planner.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L163); [crates/shamir-query-types/src/batch/planner.rs:226](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L226); [crates/shamir-query-types/src/batch/planner.rs:816](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L816); [crates/shamir-engine/src/query/batch/query_runner.rs:882](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L882); [crates/shamir-engine/src/query/batch/tests/for_each_tests.rs:1960](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/tests/for_each_tests.rs#L1960).

<a id="review-7"></a>

### Claim 7 — Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access`

Status: `partially-fixed`. Current risk: `low`.

Prior-cycle decision: `partially-fixed`.

is_write, repo collection and access collection remain separate; is_write can short-circuit and is not always a full walk. Authorized::authorize deduplicates gate checks, while temporary paths/clones remain. Fixed-depth passes are linear, not inherently quadratic.

Evidence: [crates/shamir-query-types/src/batch/batch_op.rs:764](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L764); [crates/shamir-query-types/src/batch/query_entry.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/query_entry.rs#L102); [crates/shamir-query-types/src/batch/query_entry.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/query_entry.rs#L140); [crates/shamir-engine/src/query/batch/authorized.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L102).

<a id="review-8"></a>

### Claim 8 — `Pagination::eq` (`After`) — two msgpack encodes per equality comparison

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

After matching limit and after_id, both tuples are encoded into buffers. No hot production equality workflow was found. A structural replacement must implement wire equivalence, not ordinary Value equality; otherwise current documented semantics change.

Evidence: [crates/shamir-query-types/src/read/limit.rs:84](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/limit.rs#L84); [crates/shamir-query-types/src/read/limit.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/limit.rs#L123); [crates/shamir-query-types/src/read/limit.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/limit.rs#L130); [crates/shamir-types/src/types/value.rs:287](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L287).

<a id="review-9"></a>

### Claim 9 — Plan-time marker decode pays a msgpack round-trip per `$query`/`$fn`/`$cond`/`$expr` marker

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Recognized one/two-key marker maps still encode/decode to reuse FilterValue extraction. This preserves shared semantics but costs traversals/allocations. Current ForEach hoisting removes per-iteration planning; direct replacement must retain malformed-marker fallback and Cond validation.

Evidence: [crates/shamir-query-types/src/batch/planner.rs:382](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L382); [crates/shamir-query-types/src/batch/planner.rs:392](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L392); [crates/shamir-query-types/src/batch/planner.rs:424](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/planner.rs#L424); [crates/shamir-engine/src/query/batch/query_runner.rs:882](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L882).

<a id="review-10"></a>

### Claim 10 — Per-construction `"main"` String allocations

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

TableRef/default_repo helpers build owned Strings. No measured construction bottleneck establishes benefit from Cow/interning, and those replacements affect public field ownership and mutable use.

Evidence: [crates/shamir-query-types/src/table_ref.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/table_ref.rs#L14); [crates/shamir-query-types/src/table_ref.rs:21](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/table_ref.rs#L21); [crates/shamir-query-types/src/call/mod.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/call/mod.rs#L13); [crates/shamir-query-types/src/admin/types/index_ops.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/admin/types/index_ops.rs#L9).

## Evidence and recipe corrections

- Keep source-confirmed work at qualified Low/nit severity absent measurements; do not label structural allocation counts as proven latency incidents.
- The first BatchOp buffering traversal may read already buffered serde Content, not perform another raw MessagePack decode.
- Cached sorted permutations require invalidation because InsertedRecord.fields is publicly mutable; construction-only ordering is not enforced by the type.
- Draining provenance must not remove BatchPlan.edge_provenance; Rc&lt;str&gt; proposals must account for async Send/Sync consumers.
- A custom marker dispatcher must preserve strict binary/string/array semantics, large-integer normalization and ambiguous-marker policy.
- The ForEach count tests catch repeated table validation, not independently every planner invocation. No tests were executed.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-types — Performance & O(x→0)

## Summary

This is a pure-DTO crate, so its hot paths are wire (de)serialization, the batch planner, and result-row serialization — not per-row evaluation. The dominant theme-lens issue is `BatchOp::deserialize`: every op of every request pays a triple msgpack codec pass (QueryValue → re-encode → typed decode) plus a per-op key-`Vec<String>` clone and a ~75-probe linear `has()` dispatch chain — exactly the "repeated lookups / allocation in loops" pattern CLAUDE.md pillar 3 (O(x→0)) bans in helpers. Second tier: `InsertedRecord::serialize` allocates and sorts per record despite its "allocation-free hot path" doc claim; the filter depth guard does not bound `FilterValue::Cond` recursion (unbounded deserialize-time stack growth); `FilterValue`'s 13-variant untagged serde shape buffers and trial-decodes every marker value; and `QueryRecord`'s scalar accessors deep-clone whole `Inserted` rows per lookup. Bench coverage exists only for `BatchPlanner::plan` (`benches/batch_planner.rs`) — none of the paths found below are benched. The nested-batch exponential budget (max_queries^max_nesting_depth) is a known, documented trade-off (#666 comment in `planner.rs:109-117`) and is not re-litigated here.

## Findings

### 1. `BatchOp::deserialize` — triple codec round-trip + key clones + linear dispatch chain per op
- **File:** `src/batch/batch_op.rs:256-284` (round-trip at 262–277; `keys` clone 266–269; `has()` 270; dispatch chain 287–438)
- **Severity:** high
- **Issue:** Every batch op deserializes as: (1) buffer the whole op into a `QueryValue`, (2) `rmp_serde::to_vec_named(&qv)` — a full re-encode of the entire op payload into a fresh `Vec<u8>`, (3) `rmp_serde::from_slice` — a full re-decode into the typed op struct. That is 3 codec passes and ≥2 full-payload allocations per op, paid on every `Execute`/`TxExecute` request, and multiplied by nested `Batch`/`ForEach` bodies (each nested level's entries each pay it again). On top: `keys: Vec<String> = m.keys().cloned().collect()` clones every top-level key string per op, and `has = |k| keys.iter().any(...)` is a linear scan — with ~75 sequential probes ("set" is deliberately probed last, after every other discriminator), a late-chain op pays ~75×K string comparisons. The enclosing `DbRequest` is also internally-tagged (`#[serde(tag = "op")]`, `src/wire/db_message.rs:30`), which adds its own full-content buffering pass around all of this.
- **Failure scenario:** A 5 MB INSERT-heavy batch pays ~10 MB of avoidable encode/decode work plus thousands of extra allocations on the server's decode path per request; deep ForEach nesting multiplies it per level.
- **Suggested fix:** Borrow the keys (`m.keys().any(|s| s == k)` — no `Vec<String>` clone). Replace the 75-branch `has()` chain with a single pass over the map's keys matched against a static discriminator table (lazy-init `FxMap<&str, OpKind>` or a `match`). Longer term, feed each op struct's `Deserialize` directly from the already-decoded `QueryValue` via a Content-style bridge (or an externally-tagged op tag in a future query-lang version) to eliminate the msgpack re-encode. Add a bench for this path — `benches/` currently covers only the planner.

### 2. `InsertedRecord::serialize` — per-record `Vec` collect + sort + base58, contradicting the "allocation-free" module claim
- **File:** `src/write/inserted_record.rs:29-61` (pairs collect/sort at 39–40; `id.to_string()` at 32)
- **Severity:** medium
- **Issue:** For every returned row, serialization collects `Vec<(&String, &Value)>` of all fields and `sort_unstable_by_key`s them — O(F log F) comparisons plus one `Vec` allocation per record per serialization, plus a base58 `RecordId::to_string()` per record. A write returning N rows × F fields pays O(N·F log F) + 2N allocations per wire encode, and every re-serialization (replication fan-out to S subscribers re-encodes the same rows) pays it again. The module doc (`inserted_record.rs:1-12`) claims "Allocation-free write-result record for INSERT/UPSERT hot paths" — true for construction, false for serialization.
- **Failure scenario:** `INSERT … returning` 10k rows × 20 fields → 10k sorts + 20k allocations per response, ×S subscribers under replication.
- **Suggested fix:** Establish the sorted-key invariant once at construction (the engine builds these rows — sort the key order when the `WriteResult` is assembled), or cache a sorted key permutation alongside `fields`; keep per-serialize work a linear emit.

### 3. Filter depth guard does not cover `FilterValue::Cond` nesting — unbounded deserialize-time recursion
- **File:** `src/filter/filter_enum.rs:216-238` (`check_filter_depth` walks only `And`/`Or`/`Not`); `src/filter/filter_value.rs:71-74` + `src/filter/cond.rs:40-50` (mutual recursion `FilterValue::Cond → Cond.condition: Box<Filter> → Filter`)
- **Severity:** medium
- **Issue:** `check_filter_depth` never descends into a comparison variant's `value: FilterValue`, so a `$cond` chain threaded through values reports depth 1 regardless of true depth. More importantly, the guard can only run *after* deserialization, but `Filter`/`FilterValue` deserialization itself recurses Cond↔Filter↔FilterValue with no depth bound — each wire level costs ~40 bytes (`{"$cond":{"if":…`), so a modest payload builds tens of thousands of stack frames (untagged `FilterValue` additionally buffers a serde `Content` per level) and can overflow the decode thread's stack before `MAX_FILTER_DEPTH` is ever consulted. The doc at `filter_enum.rs:7-9` claims the cap prevents "stack overflow post-handshake", which it cannot for value-tree nesting.
- **Failure scenario:** A hostile client nests `$cond` values in a WHERE clause; the server's decoder overflows its stack during `BatchRequest` decode, before any limit check runs.
- **Suggested fix:** Enforce depth during deserialization (a custom checked deserializer for `FilterValue`/`Cond` threading a depth counter, erroring past `MAX_FILTER_DEPTH` — the crate already hand-routes binary via `de_binary_strict`, so the pattern exists), and/or extend `check_filter_depth` to recurse into `FilterValue::Cond/Expr/FnCall/Array` operands so the post-hoc check at least measures the real tree.

### 4. `FilterValue` — 13-variant `#[serde(untagged)]` enum: content buffering + ~6 failed map-shaped trials per marker value
- **File:** `src/filter/filter_value.rs:9-81` (same pattern repeated for `FnCall` `src/filter/fn_call.rs:22-33`, plus `GroupRef`/`ResourceRef`/`NumDto`/`SelectExprValue`/`AggregateField`)
- **Severity:** medium
- **Issue:** serde's untagged machinery buffers the whole value into `Content` and tries variants in declaration order. The marker variants (`FieldRef`, `QueryRef`, `FnCall`, `Expr`, `Cond`, `Param`) are declared last, so every `$query`/`$param`/`$fn` reference inside every WHERE / `when` / `set` / `bind` value pays full buffering plus ~6 failed struct-variant decode attempts over the buffered content. This is per-filter-value, per-request wire cost — a linear constant the O(x→0) pillar would rather not pay.
- **Suggested fix:** Replace untagged with a hand-written `Deserialize` that dispatches on the map's reserved key (`$query`/`$fn`/`$cond`/`$expr`/`$param`/`$ref`) the way `de_binary_strict` already hand-routes `Binary` — wire shape unchanged, single-pass decode. Literal variants already fail fast; the win is for marker values.

### 5. `QueryRecord::get_value_{i64,u64,bool}` — deep-clones the whole `Inserted` record per scalar lookup
- **File:** `src/read/query_record.rs:218-227` (`get_value_owned` → `as_value()` = `rec.fields.clone()`), `246-284`
- **Severity:** medium
- **Issue:** For `QueryRecord::Inserted`, each i64/u64/bool lookup routes through `get_value_owned` → `as_value()`, which makes a full deep clone of the record's `fields` `QueryValue`, then clones the one found value — work proportional to the *whole record* per scalar read. A caller reading k fields of n returned rows pays O(n·k·record_size) — a hidden near-quadratic in helpers. Inconsistent with `get_value_str` (lines 235–241), which borrows from `rec.fields` at zero cost; the cheap path exists one match-arm away.
- **Suggested fix:** Mirror `get_value_str`: `QueryRecord::Inserted(rec) => rec.fields.get(key).and_then(QueryValue::as_i64)` (likewise `as_u64`/`as_bool`).

### 6. Batch planner — redundant alias-set clone and repeated String re-cloning through the plan
- **File:** `src/batch/planner.rs:163-164` (`aliases` TSet + `alias_order` both `keys().cloned()`), `200-203` & `226` (deps inserted into `provenance`, then re-cloned into `deps`), `238-239` (`alias.clone()` per insert), `816-817` (`deps[k].len()` — second hash lookup per key), `857` (stages re-clone every alias)
- **Severity:** low
- **Issue:** `aliases` duplicates information `queries` already has — `queries.contains_key(dep)` answers the same validation with zero allocation. Each alias string ends up cloned ~4× per plan (aliases, alias_order, dependencies/edge_provenance keys, stages). Absolute cost is bounded by `max_queries` (50/level), but the planner re-runs per nested batch and per ForEach iteration (engine re-plans the body up to `max_iterations` = 1000 times), so the churn multiplies.
- **Suggested fix:** Drop the `aliases` set and use `queries.contains_key`; drain `provenance` keys into `deps` instead of re-cloning; iterate `deps.iter()` once when seeding `in_degree`; consider `Rc<str>`/`Box<str>` keys in `BatchPlan` if clones remain.

### 7. Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access`
- **File:** `src/batch/batch_op.rs:764,771` (`is_write` recursion over `Batch`/`ForEach` bodies); `src/batch/query_entry.rs:93-155` (`repos.insert(tr.repo.clone())` at 105; un-deduped access `Vec` at 127-134)
- **Severity:** low
- **Issue:** Each helper independently re-walks the entire op tree; `is_write` is invoked per-op by classification paths, so in the worst case (all-read nested batches at max fanout/depth, within the documented 50^4 budget) total visits approach the square of tree nodes. Also `collect_repos` clones the repo `String` per entry even when already present, and `collect_required_access` returns duplicates, so the engine's auth pre-check re-validates the same `(Action, ResourcePath)` repeatedly.
- **Suggested fix:** One fused classification walk computing (repos, required_access, has_write) in a single pass; `contains` check before insert for repos; dedup the access list.

### 8. `Pagination::eq` (`After`) — two msgpack encodes per equality comparison
- **File:** `src/read/limit.rs:123`, `131-133`
- **Severity:** low
- **Issue:** `key_bytes(k1) == key_bytes(k2)` allocates and fully serializes both seek tuples on every `==`. Harmless in tests; costly if `After` pagination ever lands in a cache key / request-dedup hot path.
- **Suggested fix:** Compare element-wise (equal-length short-circuit then a canonical `QueryValue` comparator), or compute the encoded form once at construction and store it.

### 9. Plan-time marker decode pays a msgpack round-trip per `$query`/`$fn`/`$cond`/`$expr` marker
- **File:** `src/batch/planner.rs:392-419` (`rmp_serde::to_vec_named(value)` + `from_slice::<FilterValue>` per marker map)
- **Severity:** low
- **Issue:** Each marker map found while walking write values is re-encoded to msgpack and re-decoded as a `FilterValue` (2 allocations + 2 codec passes) just to reuse `extract_deps_from_filter_value`. Multiplied by engine-side ForEach re-planning (per iteration, up to 1000).
- **Suggested fix:** Decode the marker directly from the `QueryValue` map (match on the reserved key, read `alias`/`path`/`args` fields) — O(marker size), no codec — or cache decoded markers within one plan pass.

### 10. Per-construction `"main"` String allocations
- **File:** `src/table_ref.rs:21` (`DEFAULT_REPO.to_string()`); `default_repo()` in `src/call/mod.rs:13`, `src/admin/types/table_ops.rs:9`, `index_ops.rs:9`, and siblings
- **Severity:** nit
- **Issue:** Every `TableRef::new` and every defaulted `repo` field allocates a fresh `"main"` `String` — one avoidable allocation per op on the request construction path.
- **Suggested fix:** `Cow<'static, str>` for the repo field, or a shared interned default; cosmetic unless op-construction throughput matters.

## Coverage note

Functional tests are extensive (~350 `#[test]`s across module `tests/` dirs, matching the repo's test-organization rules), but the only benchmark is `benches/batch_planner.rs` (planner only). The two hottest paths identified here — `BatchOp` deserialization (finding 1) and `InsertedRecord` serialization (finding 2) — have correctness round-trip tests but zero bench coverage, so their constants cannot regress visibly. Any fix for findings 1/2 should land with a `bench_scale_tool::Harness` bench first (baseline), per the repo's /opti workflow.

</details>
