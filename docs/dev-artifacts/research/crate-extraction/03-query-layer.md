# Crate-extraction research — query layer (03)

Scope: `shamir-query-types`, `shamir-query-builder`, `shamir-funclib`.
Question: is anything in these three crates worth extracting into a
standalone crate — for testability, or as a publishable crates.io package
(the way `bench-scale-tool` was extracted on 2026-07-07)?

Method: read of each crate's `Cargo.toml`, `src/` tree, module docs, and the
key implementation files (`batch/planner.rs`, `filter/*`, `registry.rs`,
`scalar_resolver.rs`, `canonical.rs`, `compare.rs`, builder `lib.rs`), plus a
dependency-footprint check against `shamir-types` / `shamir-collections`.

**Bottom line up front:** one genuine (but conditional) crates.io candidate —
`shamir-funclib` as a generic "scalar-function stdlib over a dynamic value
model" — and one honest *non*-candidate that looks tempting on paper: the
`BatchPlanner` DAG planner, which is far more entangled with shamir's wire
types than its name suggests. `shamir-query-builder` contains nothing
extractable, by design.

---

## 1. `shamir-query-types`

**What it is.** Pure DTOs for the SDBQL wire language: `Filter`,
`FilterValue`, `FilterExpr`/`Cond`/`FnCall` (`$expr`/`$cond`/`$fn` shapes),
`ReadQuery`, the write ops, `BatchRequest`/`BatchResponse`, `BatchOp` (50+
variants including all DDL/admin/auth/migration/validator ops), subscribe
types, HMAC-confirmation canonicalisation (`hmac.rs`, 480 LOC), and — lifted
in from `shamir-engine` — the `BatchPlanner` + reference-resolution helpers.
~15.4k LOC total (incl. tests); non-test core of the `batch/` module ≈ 2.6k
LOC, `filter/` ≈ 780 LOC. Deps: `shamir-types`, `shamir-collections`, serde /
rmp-serde / indexmap, optional hmac/sha2/zeroize.

### 1a. Filter / expression DSL (`filter/`) — NOT a candidate

The `Filter` enum (265 LOC), `FilterValue` (258 LOC), `FilterExpr` (75 LOC),
`Cond` (61 LOC) are *shapes only*. The actual evaluation — `compile_filter`,
`eval`, `$cond`/`$expr` resolution, `QueryRef` materialisation — lives in
`shamir-engine` (`src/query/filter/{eval,resolve}.rs`), where it is threaded
through `FilterContext`, the field-name interner, and `TableManager`. The
crate's own `lib.rs` doc comment is explicit about this split ("What's NOT
here: FilterContext, compile_filter, batch::executor…").

So there is no self-contained "filter DSL with evaluator" to extract: the
DTOs without the evaluator are just a serde vocabulary specific to shamir's
wire protocol (`$query` aliases, `FieldPath`, `Param`/`bind` scoping,
msgpack-shaped marker maps), and the evaluator cannot come along without
dragging the interner and engine runtime. A hypothetical
`filter-expr-dsl` crate would require re-implementing evaluation generically
— that is a new project, not an extraction.

Additional entanglement: `FilterValue ⇄ QueryValue` conversion relies on a
msgpack round-trip through `rmp-serde` because `FilterValue`'s wire encoding
*is* the reserved-key map convention (`{"$query": …}`) — the DSL's identity
is inseparable from shamir's msgpack wire format.

**Verdict: not extractable.** Too entangled with `shamir-types::QueryValue`,
the msgpack wire conventions, and an evaluator that lives in the engine.

### 1b. `batch::planner::BatchPlanner` — tempting, but NO

`batch/planner.rs` (737 LOC) plans a batch as a DAG: dependency extraction →
strict alias validation → cycle detection (white-gray-black DFS, borrow-based,
allocation-free happy path) → depth check → Kahn topological sort into
parallel stages with deterministic insertion-order tie-breaking.

The *generic kernel* — `detect_cycle` + `calculate_max_depth` +
`topological_sort` — is only ~200 LOC and operates on
`TMap<String, TSet<String>>`. Everything else is shamir-specific:

- `extract_dependencies` pattern-matches all `BatchOp` variants
  (Read/Update/Set/Delete/Insert/Call/Batch/ForEach + admin no-ops);
- `extract_deps_from_value` decodes reserved-key marker maps
  (`$query`/`$fn`/`$cond`/`$expr`) out of `QueryValue` via an rmp-serde
  round-trip — a wire-format-coupled trick (#641 fix);
- `when`-filter validation (#651: reject field-based comparisons inside
  `when`), `ForEach` virtual-op-unit DoS budgeting (#653), sub-batch nesting
  depth walk, `after` path-tail rejection — all OQL semantics, not graph
  theory.

**Case FOR extraction** (of the kernel, as e.g. `dag-stage-planner`): a small,
well-tested "group DAG nodes into maximally-parallel stages with deterministic
ordering, cycle diagnosis with the actual cycle path, and depth limits"
utility; shamir would consume it behind a `DependencySource` trait, improving
planner testability in isolation.

**Case AGAINST (wins):** the kernel is ~200 LOC of textbook algorithms already
served by `petgraph` (`toposort`, `tarjan_scc`) and the `topological-sort`
crate; the only novel bits (staging waves + insertion-order tie-break) are a
20-line delta anyone can write. The value of this file is 70% *dependency
extraction from shamir's own AST*, which cannot leave the crate. Extraction
would add a trait indirection and a publish burden for near-zero community
value, and the planner is already unit-testable in place (it has its own
bench, `benches/batch_planner.rs`, and a large `batch/tests/` suite).

**Verdict: do not extract.** If testability pressure grows, an *internal*
split of the pure graph functions into `batch/graph.rs` is enough — no new
crate.

### 1c. `hmac.rs` — NOT a candidate

Canonical HMAC-input byte layouts for shamir's destructive admin ops
(`drop_db`, `chown`, …) keyed off `session_id`. Wire-protocol-specific by
definition ("changing a layout here is a breaking protocol change"). Nothing
generic here.

---

## 2. `shamir-query-builder`

**What it is.** A fluent, CodeIgniter-Active-Record-style client-side builder
(~17.5k LOC incl. tests) whose *only* output is `shamir-query-types` wire
DTOs: `val/` (FilterValue constructors), `filter/`, `query/`, `select/`,
`write/`, `batch/` (typed `Handle` dependency references), `ddl/`,
`response/` extraction helpers, plus `doc!`/`vals!` macros and the
`filter!`/`q!` proc-macros (separate `shamir-query-builder-macros` crate).
Deps: `shamir-query-types`, `shamir-types`, `shamir-collections`, the macros
crate, serde/rmp-serde. WASM-friendly, no engine dependency.

**Assessment.** Every public method exists to construct a shamir wire DTO;
the crate's value is precisely its 1:1 coupling to `shamir-query-types`.
There is no algorithmic core, no generic sub-component, and no module that
would mean anything outside shamir. The proc-macro crate is likewise
shamir-shape-specific (it emits `::shamir_query_builder::…` paths). The
builder *pattern* (fluent API → DTO) is idiomatic Rust, not a reusable
artifact.

**Verdict: nothing to extract. Plainly.** The crate is *already* the product
of a good extraction (it was deliberately split from engine/client so it
compiles to WASM); further subdivision would only fragment it.

---

## 3. `shamir-funclib` — the one real candidate

**What it is.** The built-in scalar-function library (~8.4k LOC incl. tests,
~5.5k non-test): a `ScalarRegistry` mapping folder-qualified names
(`"math/abs"`, `"arrays/slice"`, `"text/levenshtein"`) to pure
`fn(&[QueryValue]) -> Result<QueryValue, ScalarError>`, plus 12 category
modules (math, strings, arrays, cast, datetime, value_nav, validate, encode,
object, text, crypto, canonical), an aggregate registry (`agg.rs`, 813 LOC),
a cross-type total order (`compare.rs`), and a lock-free 2-layer
user→builtin `ScalarResolver` (scc::HashMap user layer over a `&'static`
builtin registry).

Dependency footprint: **only two shamir crates** — `shamir-types` (for
`QueryValue = Value<String>`) and `shamir-collections` (for `TFxMap`/
`THasher`, itself a 63-LOC leaf over indexmap+rustc-hash). Everything else is
crates.io (rust_decimal, num-bigint, chrono, regex, strsim, base64, sha2/3,
blake3, argon2, scc, …). No engine, no wire, no interner dependency —
`lib.rs` states the ABI is deliberately string-keyed `QueryValue` so "no
interner needed".

### Candidate A (primary): `shamir-funclib` → `valuefn` / `scalar-funclib`
*(whole crate, generified or paired with a value-model leaf crate)*

- **Module path:** the entire crate (`crates/shamir-funclib`), minus nothing.
- **Scope:** ~5.5k LOC non-test + ~2.8k LOC tests. ~150 built-in functions
  with arity bounds, purity/determinism metadata, and the `trusted_pure`
  index-safety opt-in gate.
- **Dependency footprint:** pure crates.io except `QueryValue` and `TFxMap`.

**Case FOR.** This is the closest analogue to `bench-scale-tool` in the
workspace: a self-contained, generically useful library whose domain is "a
JSON-like dynamic value model", not "shamir". Anyone building a query engine,
rules engine, ETL tool, or expression evaluator over a `serde_json::Value`-
like type needs exactly this: a curated, audited set of pure scalar functions
(decimal-first math, Unicode-correct text ops, canonical order-independent
BLAKE3 hashing, deterministic crypto, encode/decode, datetime) with
machine-readable error codes, arity validation, folder-namespaced
registration, purity metadata for functional-index safety, and a lock-free
user-override layer. Nothing comparable exists on crates.io as a coherent
package. The design is clean, well-documented, and already treats itself as
an embeddable library ("the embedder must have audited…", "embedders can
register additional native scalars").

**Case AGAINST (and it is substantial).** The ABI is welded to
`QueryValue = Value<String>` — an 11-variant enum (with `Dec`/`Big`/`Set`,
custom serde, msgpack conventions like "Dec serialises as a string") that
lives inside `shamir-types`, a *heavy* crate (interner, DashMap, arc-swap,
codecs, rand, record IDs — 6.9k LOC). Publishing funclib therefore forces one
of two prerequisite moves, neither free:
1. **Extract the value model first** — split `Value<K>` (+ `ValueError`,
   ~740 LOC) out of `shamir-types` into a leaf crate (say `shamir-value`) and
   publish both. `Value<K>`'s only internal dep is `InternerKey` (for the
   `InnerValue` alias) and `TMap`/`TSet`; the alias could stay behind in
   shamir-types. This is a real refactor across ~20 dependent crates' `use`
   paths, and it exports shamir's serde quirks (Dec-as-string) as a public
   contract that then semver-freezes wire behaviour.
2. **Generify over a value trait** — rewrite `registry.rs`'s `arg_*`/`v_*`
   helpers and all 12 category modules against a `trait ScalarValue`
   abstraction. That is a rewrite, not an extraction, and the decimal-first
   conventions (`v_f64` returns `Dec`!) resist clean abstraction.

Also honest: the domain-specific stragglers (`canonical.rs`'s `_prev_hash`
CAS exclusion, `crypto`'s Argon2 spawn_blocking caveat, `agg.rs`'s coupling to
shamir's SELECT semantics) would need either feature-gating or staying behind.
And unlike `bench-scale-tool` (zero workspace deps at extraction time),
funclib sits *on* the value model, so extraction is a two-crate operation.

**Recommendation:** worth doing **only together with** a `Value<K>` leaf-crate
extraction, and only if the maintainer wants the maintenance/semver burden of
a public value model. As a pure-testability move it buys little — funclib is
already excellently isolated and unit-tested per category. Priority: medium;
genuine community value, non-trivial prerequisite.

### Candidate B (sub-component): `registry.rs` + `scalar_resolver.rs` →
`fn-registry` — NOT recommended alone

The registration/dispatch mechanism (folder-qualified names via `in_folder`
prefix scoping, `FnEntry` arity + purity + `trusted_pure` metadata,
`is_indexable()` gate, 2-layer lock-free resolver with static-builtin
fallback) is an elegant, reusable *pattern* — but stripped of the `arg_*`
value helpers (QueryValue-specific) it is ~250 LOC of "string → Arc<dyn Fn>
map with min/max arity and three bools". As a standalone crate that is a
20-minute reimplementation for anyone who needs it; the value is in the
*function library*, not the registry. Extract only as part of Candidate A.

### Candidate C (sub-component): `compare.rs` (166 LOC) and `canonical.rs`
(240 LOC) — too thin / too entangled

`compare.rs` (canonical cross-type total order with NaN-sorts-last and
cross-subtype numeric equality) and `canonical.rs` (order-independent,
type-tagged BLAKE3 content hash with sorted-by-serialised-key-bytes maps) are
both genuinely nice designs, but each is (a) under 250 LOC, (b) defined over
`Value<K>`, and (c) — for canonical — carries the shamir-specific
`_prev_hash` top-level exclusion and the "Dec/Big hash as T_STR for msgpack
round-trip invariance" rule, which only makes sense against shamir's codec.
They ride along inside Candidate A or stay put.

---

## Summary table

| Candidate | Source | Proposed crate | Non-test LOC | shamir-dep footprint | Verdict |
|---|---|---|---|---|---|
| Filter/expr DSL | `shamir-query-types/src/filter/` | — | ~780 (DTOs only; evaluator in engine) | QueryValue, msgpack wire conventions, engine evaluator | **No** — shapes without evaluator; evaluator inseparable from interner/engine |
| Batch DAG planner | `shamir-query-types/src/batch/planner.rs` | `dag-stage-planner` | 737 (generic kernel ~200) | Filter, FilterValue, QueryValue, BatchOp (50+ variants), rmp-serde markers | **No** — 70% is shamir-AST dependency extraction; kernel is petgraph-territory |
| Fluent builder | `shamir-query-builder` (any part) | — | ~9k | 1:1 with shamir wire DTOs by design | **No** — nothing generic exists |
| Scalar function stdlib | `shamir-funclib` (whole) | `valuefn` / `scalar-funclib` | ~5.5k | Only `QueryValue` + `TFxMap` (leaf-ish) | **Conditional YES** — requires prior `Value<K>` leaf-crate extraction; real community value |
| Fn registry/resolver | `funclib/{registry,scalar_resolver}.rs` | `fn-registry` | ~470 (~250 sans value helpers) | QueryValue in arg helpers | **No, not alone** — pattern too thin without the library |
| Total order / canonical hash | `funclib/{compare,canonical}.rs` | — | 166 / 240 | Value<K>; `_prev_hash`, msgpack Dec-as-str rules | **No** — too small, domain rules baked in |

## Cross-cutting observation

The single blocker recurring across every "almost" candidate in this layer is
that **`Value<K>` lives inside the heavy `shamir-types` crate** (interner,
codecs, DashMap, rand — 6.9k LOC). If the maintainer ever wants to publish
*anything* from the query layer, the enabling first move is extracting
`Value<K>` + `ValueError` (~740 LOC, deps: indexmap, rust_decimal, num-bigint,
serde, rmp-serde, bytes, shamir-collections) into a leaf `shamir-value` crate.
That refactor also stands on its own for testability/compile-time (query-layer
crates would stop rebuilding when the interner or codecs change). It was out
of scope for this report's three crates but is the recommended prerequisite
for Candidate A.
