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
