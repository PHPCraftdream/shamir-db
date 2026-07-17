# Performance Optimization Audit — 2026-07-17 Release Audit (07)

Read-only research pass over the hot paths of `shamir-engine` (query/filter/
projection), `shamir-index` (vector/HNSW/SQ8), `shamir-tx` (commit/MVCC),
`shamir-storage` (backend hot calls) and `shamir-server`/`shamir-connect`
(per-request dispatch). Method: Read/Grep only, no builds, no tests.

## Executive summary

The codebase is in visibly good shape: prior audits (§1.1/§1.2 storage, VR-7,
4(c)/4.1 vector, S4 lens-fed aggregation, #643 CondCache) have removed most of
the classic per-row costs, and this session's new code (`cond_cache.rs`,
`eval_context.rs`, `select_projection.rs`) does **not** reintroduce any banned
pattern — no unannotated `scc::*::len()` anywhere (all 20+ sites carry the
`O(N) ack` annotation and are off hot paths), no `RandomState` collections in
production code, and every `std::sync::Mutex`/`parking_lot` use is a documented
sanctioned exception.

The single biggest remaining systemic gap is the exact pattern #643 fixed for
`$cond`, applied one level wider: **`resolve_filter_query` interprets the raw
`FilterValue` AST per row**, so every dynamic operand (`$ref` field paths,
`$query` refs, literal strings inside `$fn`/`$expr` args) is re-interned,
re-parsed, re-allocated, or re-cloned **once per record** even though the tree
is static per query. The second cluster is per-candidate `Vec` clones in the
HNSW search loops. Both have clear, local fix directions.

## Findings ranked by (hotness × cost)

| # | Finding | Location | Hotness | Cost per unit | Severity |
|---|---------|----------|---------|---------------|----------|
| F1 | `FilterValue::FieldRef` re-interned + double-converted per row (no compiled value-IR) | `shamir-engine/src/query/filter/resolve.rs:182-191` | per row × per `$ref` operand | Vec alloc + N DashMap lookups + owned subtree materialise + convert | **High** |
| F2 | Record-independent operands (`$query`, `$param`) re-resolved per row: string path re-parse + deep `QueryValue` clone | `resolve.rs:192-195,535-557`; consumed per row from `filter_node.rs:301,327-328` | per row when `$query` in WHERE/projection | path parse + deep clone | **High** |
| F3 | HNSW search: per-candidate `Vec` clone of code/f32 vectors out of `scc` maps | `shamir-index/src/vector/hnsw_adapter.rs:1790,1841,1945,1952` | per candidate (overscan = 16k+64) per ANN query | heap alloc + memcpy of dim(-×4) bytes | **High** |
| F4 | Cosine metric: 2× O(dim) scalar norm recompute per graph hop; `approx_l2_sq`/norms not SIMD-ised | `shamir-index/src/vector/quantized_dist.rs:217-226,173-185`; `sq8.rs:295-325` | per graph hop per ANN query | 2× O(dim) scalar loops | **High** (vector workloads) |
| F5 | ForEach: per-iteration msgpack round-trip of results + full recompile of static loop body | `shamir-engine/src/query/batch/query_runner.rs:671-674` (+ recursion into `execute_batch_impl`) | per iteration (≤100 000) | full serialize+deserialize + re-plan/re-compile | **Med-High** |
| F6 | `In`-with-column-ref coercing set probe allocates `String`/`Vec` per row; `InSet` materialises owned value per row | `shamir-engine/src/query/filter/filter_node.rs:73-74,373-375` | per row | String/Vec alloc + owned convert | **Medium** |
| F7 | `search_quantized_bruteforce` clones the entire `vectors_u8` map per query | `hnsw_adapter.rs:1731-1735` | per query (small quantized index) | O(N·dim) copy + N Vec allocs | **Medium** |
| F8 | Fjall `set`: `contains_key` + `insert` = 2 LSM point lookups per write; per-op `spawn_blocking` on point reads | `shamir-storage/src/storage_fjall.rs:337-360,405-424` | per storage write / point read | +1 LSM lookup; task-spawn overhead | **Medium** |
| F9 | ORDER BY sort keys clone every string field per row | `shamir-engine/src/query/read/order.rs:180` | per row per string order key | String alloc + copy | **Low-Med** |
| F10 | `field_path` stored as `SmallVec<u64>` and re-wrapped to `SmallVec<InternerKey>` in every `matches()` arm | `filter_node.rs:294-295` (+ ~14 sibling arms); `select_projection.rs:113-115` | per node per row | stack-only rebuild (no heap) | **Low** |
| F11 | `resolve_write_value`: fast path deep-clones the whole doc + deep-eq compare; per-marker msgpack round-trip | `shamir-engine/src/query/batch/param_subst.rs:199-201,224-226`; `query_runner.rs:975,1219` | per write op (per iteration under ForEach) | deep clone + deep eq / serde round-trip | **Low** |

---

## F1 — `resolve_filter_query` has no compiled IR: `$ref` paths re-interned per row (the un-generalised #643)

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\filter\resolve.rs:182-191`

```rust
FilterValue::FieldRef { path } => {
    let keys = intern_field_path(path, ctx.interner)?;          // Vec<u64> heap alloc + 1 DashMap lookup PER SEGMENT
    let ipath: SmallVec<[InternerKey; 4]> =
        keys.iter().map(|&id| InternerKey::new(id)).collect();  // second buffer
    record.materialize_at(&ipath)                                // owned InnerValue subtree (clones Str/containers)
        .and_then(|iv| inner_value_to_query_value(&iv, ctx.interner).ok())  // second conversion pass
}
```

`intern_field_path` (`resolve.rs:46-53`) allocates a `Vec` and performs one
`Interner::get_ind` — a DashMap shard lookup (`shamir-types/src/core/interner/interner.rs:287-291`)
— per path segment, **per record**. The compiled `FilterNode` tree already
solved this for top-level field paths (pre-interned `CompactPath` at
`compile_filter` time), but every `FilterValue` that reaches the interpreter
dynamically bypasses it:

- `$fn` args in projections — `SELECT upper(name)` re-interns `name` on every row (`select_projection.rs:124-127` → `resolve_filter_query`);
- `$expr` operands (`resolve.rs:285-288`), `$cond` `then`/`or_else` branches (`resolve.rs:242-244`);
- non-literal RHS of `Compare`/`Between`/`Contains`/`In` (`filter_node.rs:301,459,503,644`);
- write-value resolution under ForEach (`param_subst.rs:249`).

Literal strings/binaries inside these trees are also cloned per row
(`resolve.rs:180-181` — `s.clone()` / `b.clone()`), because `pre_resolved`
constant-folding exists only at the top level of `Compare`/`In`/`Between`
nodes, not inside `FnCall`/`Expr`/`Cond` argument trees.

**Cost:** for a scan of N rows with one `$fn($ref)` projection: N × (1 Vec
alloc + segments × DashMap lookup + 1 owned subtree materialise + 1 tree
conversion + literal clones). This is the same shape #643 just fixed for
`compile_filter`-per-row, one layer down.

**Fix direction:** compile `FilterValue` into a per-query value-IR (mirroring
`FilterNode`): pre-interned `CompactPath` for `FieldRef`, `pre_resolved:
Option<QueryValue>` for literal leaves, pre-compiled child nodes for
`FnCall`/`Expr`/`Cond`. Lower-effort interim (exact CondCache precedent): a
pointer-keyed `TMap<usize, CompactPath>` populated by a `prescan` (extend
`prescan_cond_cache`, which already walks every `FilterValue` shape) and
threaded through `FilterContext` next to `cond_cache`.

## F2 — `$query`/`$param` operands are record-independent but re-resolved per row (path re-parse + deep clone)

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\filter\resolve.rs:192-195` and `resolve_query_ref_value` at `resolve.rs:535-557`.

A `FilterValue::QueryRef` in a WHERE clause (e.g. `age > @stats[0].avg`) is
resolved on **every row** via `FilterNode::Compare`'s dynamic arm
(`filter_node.rs:301`) even though `ctx.resolved_refs` is fixed for the whole
scan. Each per-row resolution does:

- `alias.strip_prefix('@')` + map lookup (cheap),
- **string path parsing per row** — `path.find(']')`, `usize::parse`, prefix
  strips (`resolve.rs:546-556`, `resolve_query_value_path:598-630`),
- **deep clone of the target `QueryValue`** (`.clone()` /
  `.into_owned()` at `resolve.rs:538,553,556`) — arbitrarily large if the
  referenced value is a Map/List.

`$param` (`resolve.rs:211`) likewise clones per row. The `In` node already
demonstrates the fix in-tree: `ref_column_sets` (`filter_node.rs:207,407-427`)
hoists column-ref resolution into a `OnceLock` populated on the first row.

**Fix direction:** extend the existing `pre_resolved` mechanism to any
record-independent `FilterValue` (a `FilterValue` subtree with no
`FieldRef` inside is invariant across the scan) — either at compile time when
a `FilterContext` is available, or lazily via the `OnceLock` pattern already
used for `ref_column_sets`. This removes both the per-row parse and the
per-row deep clone.

## F3 — HNSW search loops clone each candidate's vector out of the scc map

**Location:** `D:\dev\rust\shamir-db\crates\shamir-index\src\vector\hnsw_adapter.rs`

- `search_quantized_graph` (line 1790): `self.vectors_u8.read_async(&n.d_id, |_, c| c.clone())` — clones a `Vec<u8>` of `dim` bytes for **each of the overscan = 16k+64 candidates** per query, only to call `ctx.score(&codes)` and drop it.
- `search_cofilter_quantized` (line 1841): same pattern.
- `search_prefilter` (lines 1945, 1952): same, for up to `PRE_FILTER_MAX_CANDIDATES = 4096` candidates — the f32 arm clones `Vec<f32>` (4·dim bytes each; at dim=768 that is ~12 MB of transient allocation per pre-filter query at the cap).

**Fix direction:** compute the score **inside the read closure** and return the
`f32`: `read_async(&n.d_id, |_, c| ctx.score(c))` /
`read_async(&internal, |_, v| dist.eval(query, v))`. `RescoreCtx::score` and
`ShamirDist::eval` take `&[u8]`/`&[f32]` and are synchronous — no reason to
own the buffer. Zero-risk, mechanical change; eliminates one heap
alloc + memcpy per candidate per query.

Related (line 1828): `allow_set.clone()` per co-filter query copies the whole
allow-set to satisfy `spawn_blocking`'s `'static` bound — acceptable while the
set is small, but it scales with candidate count; an `Arc<TFxSet>` built once
by the caller would remove it.

## F4 — SQ8 Cosine: two O(dim) scalar norm passes per graph hop; L2/norm loops not SIMD-ised

**Location:** `D:\dev\rust\shamir-db\crates\shamir-index\src\vector\quantized_dist.rs:217-226` (Cosine arm), `:173-185` (`dequant_norm_sq`), and `sq8.rs:295-325` (`approx_l2_sq`).

The module's own VR-7 (#429) analysis documents that the Cosine arm of
`ShamirDistU8::eval` recomputes `dequant_norm_sq` for **both** operands on
every HNSW hop, measured at ~3.5× slower than L2 (243 µs vs 69 µs per
256-candidate pool, dim=128) — and the recommended fixes (option 2:
query-norm hoist; option 3: pointer-keyed norm cache) are **still
unimplemented**. This is the single largest known, quantified, unfixed cost in
the vector path.

Additionally, task #614 gave `approx_dot` a dedicated SIMD kernel
(`weighted_bilinear_f32`), but its siblings on the same per-hop path remain
scalar loops: `approx_l2_sq` (the entire L2 metric eval, `sq8.rs:316-323`) and
`dequant_norm_sq` (both copies: `quantized_dist.rs:180-184` and the
`RescoreCtx` one at `:391-396`), plus `RescoreCtx::fused_dot` (`:376-380`).
These are exactly the shapes the existing AVX2/NEON dispatch in
`vector/simd.rs` was built for.

**Fix direction:** (a) land VR-7 option 2 (compute the query code's norm once
per search in `search_quantized_graph` — safe because the graph clone lives
per search) or option 3 with an eviction cap; (b) add
`weighted_sq_diff_u8` / `dequant_norm_sq` SIMD kernels next to
`weighted_bilinear_f32` and route `approx_l2_sq` + both `dequant_norm_sq`
copies through them.

## F5 — ForEach: per-iteration msgpack round-trip + full recompile of a static body

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\batch\query_runner.rs:671-674`

```rust
let value = rmp_serde::to_vec_named(&inner_results)
    .ok()
    .and_then(|b| rmp_serde::from_slice::<QueryValue>(&b).ok())
    .unwrap_or(QueryValue::Null);
```

Each ForEach iteration (server ceiling: `ABSOLUTE_MAX_FOR_EACH_ITERATIONS =
100_000`, line 34) serialises the **entire** iteration result map to msgpack
and parses it back, purely as an in-memory `TMap<String, QueryResult>` →
`QueryValue` type conversion. That is O(result size) encode + decode + full
re-allocation of every string/record, twice, per iteration.

Second, the loop recurses into `run_nested_body_in_outer_tx` /
`execute_batch_impl` (lines 620-666) per iteration, which re-plans and
re-compiles the identical body: `compile_filter` for each WHERE,
`SelectProjection::new` (including `prescan_cond_cache`), `pre_intern_select_keys`,
planner probes — all invariant across iterations except the injected
`$param`. This is the request brief's "another compile_filter-per-row-style
pattern": #643 removed per-row recompiles inside one query; ForEach still pays
per-iteration recompiles of a whole batch body.

**Fix direction:** (a) write a direct `QueryResult`/results-map → `QueryValue`
conversion (all shapes are known; no serde needed); (b) longer-term, hoist a
compiled-body cache (compiled filters + projections keyed by op index) out of
the iteration loop and thread it through the recursion, invalidated by nothing
(the body is immutable for the loop's lifetime).

## F6 — `In` filter per-row allocations in the membership probes

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\filter\filter_node.rs`

- `set_contains_coercing` (lines 73-74): `ScalarRef::Str(s) => set.contains(&QueryValue::Str(s.to_string()))` and `Bin(b) => ... b.to_vec()` — a heap `String`/`Vec` allocated **per row** just to probe the hash set, on the column-query-ref `In` path (called from line 452). The O(1) set probe was the point of the optimisation; the per-probe alloc gives some of it back.
- `InSet::matches` (lines 373-375): uses `record.materialize_at(...)` (owned `InnerValue`, clones the leaf — `String` alloc for str fields) + `inner_value_to_query_value` per row, where the sibling `In` node uses the borrow-based `scalar_at` → `ScalarRef`. The all-literal fast path is thus *more* allocation-heavy per row than the slow path's field access.

**Fix direction:** for `InSet`, switch to `scalar_at` + a probe helper (the
existing `set_contains_coercing` semantics discussion at lines 443-450 notes
the coercion divergence — unifying on the coercing probe fixes both); for the
`Str`/`Bin` probe alloc, use `hashbrown`'s raw-entry / `equivalent`-style
lookup so a `&str` can probe a `TSet<QueryValue>` without an owned key
(e.g. an internal `QueryValueRef<'a>` with matching `Hash`).

## F7 — `search_quantized_bruteforce` snapshots the whole codes map per query

**Location:** `D:\dev\rust\shamir-db\crates\shamir-index\src\vector\hnsw_adapter.rs:1731-1735`

```rust
let mut pairs: Vec<(usize, Vec<u8>)> = Vec::with_capacity(256);
self.vectors_u8.iter_sync(|i, c| { pairs.push((*i, c.clone())); true });
```

Every brute-force quantized query clones **every** stored code vector
(O(N·dim) bytes + N `Vec` allocs) before scoring. The `RescoreCtx` is built
before the loop and `score(&[u8])` borrows — the clone exists only to escape
the visitor.

**Fix direction:** build `ctx` first and score inside `iter_sync`, collecting
`(internal, f32)` pairs (12 bytes each) instead of `(internal, Vec<u8>)`;
`deleted`/`rid_map` checks already run after the snapshot and can stay in the
second (async) pass over the small score list.

## F8 — Storage: fjall `set` double point-lookup; per-op `spawn_blocking`

**Location:** `D:\dev\rust\shamir-db\crates\shamir-storage\src\storage_fjall.rs:337-360` (set), `:405-424` (get)

Already flagged in-code (§1.2, audit 2026-07-06): `set` performs
`contains_key` + `insert` — doubling the LSM point-lookup cost of **every
durable write** — solely to honour the `Store::set → bool` ("was created")
contract, which the engine layer mostly re-derives itself. The in-code note
names the proper fix: a flag-free fast-path variant on `Store`
(`set_unchecked` / `set_no_flag`), routed from callers that ignore the bool.
This is a trait-level change; listing it here so the release audit tracks it
as an approved follow-up rather than re-discovering it.

Secondary: every point `get`/`set` pays a `tokio::task::spawn_blocking`
round-trip (task alloc + queue + wake, ~1-5 µs) on top of a microsecond-scale
LSM memtable read. `get_many`-style callers already batch; remaining
single-record hot callers (e.g. MVCC `resolve_read` history probes,
`mvcc_store/mod.rs:668-671`) inherit the per-op overhead. Fix direction:
batch adjacent point reads where call sites allow, or route sub-microsecond
reads through a dedicated small blocking pool / inline-if-memtable-hit path if
fjall exposes one.

## F9 — ORDER BY clones every string sort key

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\read\order.rs:180`

`QvSortKey::from_query_value` does `QueryValue::Str(s) =>
QvSortKey::Str(s.clone())` — one `String` alloc per row per string ORDER BY
column, in both `apply_order_by_qv` (phase 1, line 27-30) and the top-K heap
(line 116). The owned repr exists only because `Dec`/`Big` need a
`to_string()` canonical form. A `Cow<'a, str>` (borrowed for `Str`, owned for
`Dec`/`Big`) removes N allocs per sort; the records outlive the key vector in
both call shapes.

## F10 — `CompactPath` re-wrapped into `SmallVec<InternerKey>` on every `matches()`

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\filter\filter_node.rs:294-295` (and the same 2-line preamble in ~14 other arms: 352-353, 357-358, 367-368, 394-395, 474-475, 487-488, 528-529, 557-558, 575-576, 603-604, 634-635, 670-671, 675-676, 685-686), plus `select_projection.rs:113-115`.

Every compiled node stores `field_path: SmallVec<[u64; 4]>` and rebuilds a
`SmallVec<[InternerKey; 4]>` per row per node. It is stack-only (no heap for
≤4 segments) so the cost is small, but it is pure busy-work executed at the
highest frequency in the engine. Since `InternerKey` is a `u64` newtype, store
`CompactPath = SmallVec<[InternerKey; 4]>` at compile time (single-site type
change in `intern_field_path_compact`, `resolve.rs:60-70`) and pass
`&field_path` directly.

## F11 — Write-value marker resolution: fast-path deep clone + per-marker serde round-trip

**Location:** `D:\dev\rust\shamir-db\crates\shamir-engine\src\query\batch\param_subst.rs:199-201, 224-226`; callers `query_runner.rs:972-975, 1213-1219`

- Fast path (`param_subst.rs:200`): `return Ok(value.clone())` deep-clones the whole write document even when nothing needs substitution; the Update/Set callers then run a **deep equality compare** (`subst_set == op.set`, `query_runner.rs:975`) to detect the no-op. A `Cow<'_, QueryValue>` return (Borrowed on the fast path) removes both the clone and the compare.
- Marker resolution (`:224-226`): each `$query`/`$fn`/`$cond`/`$expr` marker is converted `QueryValue → msgpack bytes → FilterValue` per occurrence, per op — and per iteration under ForEach. A pointer-keyed decoded-marker cache (CondCache precedent) or a direct `QueryValue → FilterValue` structural conversion would remove the serde round-trip.

Per-op rather than per-row, so ranked low — but it multiplies under F5's
iteration loop.

---

## Verified-clean (audited, no action)

- **#643 CondCache (this session's new code)** — `cond_cache.rs` prescan runs once at query-compile time; pointer-identity keying is used only for lookup (soft-miss on clone); `FilterContext` default `None` keeps all other callers on the old path. No banned pattern introduced.
- **`scc::*::len()`** — every call site across the workspace carries `#[allow(clippy::disallowed_methods)]` with an `O(N) ack` naming a cold context (snapshot seed, per-tx sizing, telemetry). The two commit-path sites (`shamir-engine/src/tx/commit.rs:171,816`) are bounded by the tx's own footprint, not table size. `HnswAdapter::len()` is an atomic mirror (`hnsw_adapter.rs:2760-2767`), correctly O(1).
- **Hashers** — no `HashMap::new()`/`HashSet::new()` (RandomState) in production code; the five `BTreeMap::new()` hits are cold (DDL/launcher/drain-batch grouping).
- **Locks** — all `std::sync::Mutex`/`parking_lot` uses are annotated sanctioned exceptions (WAL segment file handle, interner reverse-write, per-session rate bucket, admin/audit); none on per-row paths.
- **FTS brute-force** (`filter_node.rs:680-737`, `fts.rs`) — bitmask AND-mode, ASCII fast-path folding, no per-record lowercase alloc; already optimal.
- **Aggregation** (`aggregate.rs`) — lens-fed `ScalarRef` accumulators, owned state only at §5b boundaries; per-group (not per-row) fallbacks.
- **Dispatch** (`shamir-connect/src/server/dispatch.rs`) — zero-copy envelope view path exists and is the documented transport hot path.
- **MVCC read** (`shamir-tx/src/mvcc_store/mod.rs:653-706`) — overlay-first probe, single-log design, no per-read clones beyond the returned `Bytes` (refcounted).
- **Fjall `get`** — zero-copy `Slice → Bytes` (bytes_1 feature) already landed.
