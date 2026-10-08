<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-types — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Codec passes, row sorting, scalar-accessor cloning, and planner allocations are structurally confirmed, but latency/multiplier estimates are not measured. Neighbor remediation removed per-iteration replanning and deduplicates authorization checks. The operand-depth omission is fixed.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 8 | 1 | 1 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `BatchOp::deserialize` — triple codec round-trip + key clones + linear dispatch chain per op

Status: `confirmed-open`. Current risk: `medium`.

QueryValue buffering, key String clones, full re-encode, and typed re-decode remain. Discriminator probes repeatedly scan cloned keys. The only registered crate bench still measures planning, not decoding. This proves allocation/traversal costs, not High latency or the numerical workload estimates.

Evidence: [crates/shamir-query-types/src/batch/batch_op.rs:262](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L262); [crates/shamir-query-types/src/batch/batch_op.rs:266](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L266); [crates/shamir-query-types/src/batch/batch_op.rs:277](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L277); [crates/shamir-query-types/src/batch/batch_op.rs:287](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L287); [crates/shamir-query-types/Cargo.toml:47](../../../../../crates/shamir-query-types/Cargo.toml#L47).

<a id="review-2"></a>

### Claim 2 — `InsertedRecord::serialize` — per-record `Vec` collect + sort + base58, contradicting the "allocation-free" module claim

Status: `confirmed-open`. Current risk: `medium`.

Map serialization still allocates a pairs Vec and sorts O(F log F); present ids also create base58 strings. No cached sorted order exists. The zero-intermediate-map construction claim is narrower than allocation-free serialization; replication fan-out multipliers were not traced.

Evidence: [crates/shamir-query-types/src/write/inserted_record.rs:1](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L1); [crates/shamir-query-types/src/write/inserted_record.rs:32](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L32); [crates/shamir-query-types/src/write/inserted_record.rs:39](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L39); [crates/shamir-query-types/src/write/inserted_record.rs:40](../../../../../crates/shamir-query-types/src/write/inserted_record.rs#L40).

<a id="review-3"></a>

### Claim 3 — Filter depth guard does not cover `FilterValue::Cond` nesting — unbounded deserialize-time recursion

Status: `fixed`. Current risk: —.

The combined iterative Filter/FilterValue depth checker now covers Cond conditions and branches with registered Array/Cond regressions. The separate unlimited-codec claim is refuted by pinned rmp-serde 1.3.1's container counter, not a later fix. Neither result establishes stack safety at every permitted codec depth on every target.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:321](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L321); [crates/shamir-query-types/src/filter/filter_enum.rs:342](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L342); [crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:256](../../../../../crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs#L256); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

Pinned dependency evidence: [rmp-serde 1.3.1, src/decode.rs:294](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/decode.rs:566](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs).

Grouping/duplicate: `security-crypto.md#1,#3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `FilterValue` — 13-variant `#[serde(untagged)]` enum: content buffering + ~6 failed map-shaped trials per marker value

Status: `confirmed-open`. Current risk: `low`.

The 13-variant untagged enum and ordered trial decoding remain. Cached serde confirms buffering/trials. Failed map-shaped trial count varies by selected variant, from zero preceding marker variants for FieldRef to five for Param; no sevenfold latency claim is proven.

Evidence: [crates/shamir-query-types/src/filter/filter_value.rs:9](../../../../../crates/shamir-query-types/src/filter/filter_value.rs#L9); [crates/shamir-query-types/src/filter/filter_value.rs:48](../../../../../crates/shamir-query-types/src/filter/filter_value.rs#L48); [crates/shamir-query-types/src/filter/filter_value.rs:77](../../../../../crates/shamir-query-types/src/filter/filter_value.rs#L77); [Cargo.lock:3233](../../../../../Cargo.lock#L3233).

<a id="review-5"></a>

### Claim 5 — `QueryRecord::get_value_{i64,u64,bool}` — deep-clones the whole `Inserted` record per scalar lookup

Status: `confirmed-open`. Current risk: `medium`.

All three Inserted scalar accessor arms still call get_value_owned, which calls as_value and clones all fields before cloning the selected value. The string accessor already borrows directly. The O(record-size) per lookup mechanism is unchanged.

Evidence: [crates/shamir-query-types/src/read/query_record.rs:192](../../../../../crates/shamir-query-types/src/read/query_record.rs#L192); [crates/shamir-query-types/src/read/query_record.rs:218](../../../../../crates/shamir-query-types/src/read/query_record.rs#L218); [crates/shamir-query-types/src/read/query_record.rs:246](../../../../../crates/shamir-query-types/src/read/query_record.rs#L246); [crates/shamir-query-types/src/read/query_record.rs:261](../../../../../crates/shamir-query-types/src/read/query_record.rs#L261); [crates/shamir-query-types/src/read/query_record.rs:277](../../../../../crates/shamir-query-types/src/read/query_record.rs#L277).

<a id="review-6"></a>

### Claim 6 — Batch planner — redundant alias-set clone and repeated String re-cloning through the plan

Status: `confirmed-open`. Current risk: `low`.

Redundant aliases/alias_order collections, provenance/dependency clones, and deps[k] lookups remain. The engine now plans each ForEach body once before its loop, so the report's per-iteration replanning multiplier is stale.

Evidence: [crates/shamir-query-types/src/batch/planner.rs:163](../../../../../crates/shamir-query-types/src/batch/planner.rs#L163); [crates/shamir-query-types/src/batch/planner.rs:226](../../../../../crates/shamir-query-types/src/batch/planner.rs#L226); [crates/shamir-query-types/src/batch/planner.rs:816](../../../../../crates/shamir-query-types/src/batch/planner.rs#L816); [crates/shamir-engine/src/query/batch/query_runner.rs:882](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L882).

<a id="review-7"></a>

### Claim 7 — Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access`

Status: `partially-fixed`. Current risk: `low`.

Separate recursive walkers and duplicate result/path allocations remain. Authorized::authorize now deduplicates requirements before gate.check, refuting repeated authorization-check cost. Three passes alone are linear; repeated nested-level scans are O(ND), not inherently quadratic at fixed depth.

Evidence: [crates/shamir-query-types/src/batch/batch_op.rs:764](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L764); [crates/shamir-query-types/src/batch/query_entry.rs:102](../../../../../crates/shamir-query-types/src/batch/query_entry.rs#L102); [crates/shamir-query-types/src/batch/query_entry.rs:140](../../../../../crates/shamir-query-types/src/batch/query_entry.rs#L140); [crates/shamir-engine/src/query/batch/authorized.rs:102](../../../../../crates/shamir-engine/src/query/batch/authorized.rs#L102).

<a id="review-8"></a>

### Claim 8 — `Pagination::eq` (`After`) — two msgpack encodes per equality comparison

Status: `confirmed-open`. Current risk: `low`.

When limit and after_id match, After equality still encodes both key tuples into newly allocated buffers. No production cache-key hot-path caller or measured latency was established.

Evidence: [crates/shamir-query-types/src/read/limit.rs:123](../../../../../crates/shamir-query-types/src/read/limit.rs#L123); [crates/shamir-query-types/src/read/limit.rs:130](../../../../../crates/shamir-query-types/src/read/limit.rs#L130).

<a id="review-9"></a>

### Claim 9 — Plan-time marker decode pays a msgpack round-trip per `$query`/`$fn`/`$cond`/`$expr` marker

Status: `confirmed-open`. Current risk: `low`.

Recognized marker maps still undergo to_vec_named/from_slice to reuse FilterValue extraction. The codec work remains, but ForEach body planning is now hoisted outside the iteration loop.

Evidence: [crates/shamir-query-types/src/batch/planner.rs:392](../../../../../crates/shamir-query-types/src/batch/planner.rs#L392); [crates/shamir-query-types/src/batch/planner.rs:395](../../../../../crates/shamir-query-types/src/batch/planner.rs#L395); [crates/shamir-engine/src/query/batch/query_runner.rs:882](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L882).

<a id="review-10"></a>

### Claim 10 — Per-construction `"main"` String allocations

Status: `confirmed-open`. Current risk: `nit`.

TableRef::new and default_repo helpers still allocate owned default strings. This is a source-confirmed construction cost, with no demonstrated throughput significance.

Evidence: [crates/shamir-query-types/src/table_ref.rs:21](../../../../../crates/shamir-query-types/src/table_ref.rs#L21); [crates/shamir-query-types/src/call/mod.rs:13](../../../../../crates/shamir-query-types/src/call/mod.rs#L13); [crates/shamir-query-types/src/admin/types/table_ops.rs:9](../../../../../crates/shamir-query-types/src/admin/types/table_ops.rs#L9); [crates/shamir-query-types/src/admin/types/index_ops.rs:9](../../../../../crates/shamir-query-types/src/admin/types/index_ops.rs#L9).

## Corrections and qualified non-findings

- Keep structural cost evidence separate from unmeasured latency, allocation-count estimates, and replication subscriber multipliers.
- ForEach body plan/validation is now performed once before iteration, not up to max_iterations times.
- Authorization consumers now deduplicate requirements before checking permissions.
- Three tree walks are not by themselves O(N²); fixed-depth repeated scans remain linear in N with a depth factor.
- Untagged marker trial counts depend on variant position; a universal approximately six failed map trials or sevenfold cost is not supported.
- Planner nesting remains bounded recursion, despite its iterative-worklist comments.

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
