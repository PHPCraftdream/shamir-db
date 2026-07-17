בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# v0.10 Release-Readiness Audit — Functionality Sufficiency

**Date:** 2026-07-17 · **Method:** read-only source survey (no builds, no tests).
**Primary focus:** breadth of the built-in function library (`shamir-funclib`)
for filtering & value transformation. **Secondary:** query-language
completeness, client-SDK parity, operational tooling.

**Sources read in full:** every file in `crates/shamir-funclib/src/` (all 13
category modules + registry + resolver + agg), `crates/shamir-query-types/src/`
(filter/, read/, batch/, admin/), `crates/shamir-engine/src/query/filter/resolve.rs`,
`crates/shamir-engine/src/query/read/aggregate.rs`, `crates/shamir-server/src/`
(backup, observability, main), `crates/shamir-client-ts/src/` (builders/types),
`docs/guide-docs/guide/05-functions.md`, `07-operations.md`, and the prior
research in `docs/dev-artifacts/research/completeness-oql.md` and
`coverage-ts-query-builder.md`.

---

## Executive summary

The suspicion that the function library is "too thin" is **only partially
right**. The library is broader than a first glance suggests — **142
registered scalar functions across 11 folders plus 18 aggregates** (the
registry self-test asserts ≥ 130) — and its architecture is unusually good:
purity/determinism metadata per function, an explicit `trusted_pure` gate for
functional indexes, a lock-free user-override layer, and a WASM escape hatch
for anything custom. Strings (incl. a full 8-function regex family), math,
encoding, validation, and crypto are at or above what comparable
document-store engines ship.

The real deficits are **narrow but high-frequency**:

1. **Null-handling functions do not exist at all** — no `coalesce`,
   `if_null`, `nullif`. This is the single most-used transformation family in
   real-world filters/projections, and `fn_call.rs`'s own doc-comment
   *advertises* `{"$fn": {"name": "COALESCE", ...}}` as an example — a call
   that today returns `unknown_function`.
2. **Parameterised aggregates are unreachable from the wire** — `percentile`
   is hard-wired to p=0.5 and `string_agg` to sep="," because
   `SelectItem::AggregateFn` carries only a name and the executor calls
   `builtin_aggs().make(name)` with no params. The `distinct: bool` flag on
   `Aggregate`/`AggregateFn` is accepted on the wire but ignored by the
   grouped-aggregate executor.
3. **Datetime is UTC-epoch-only with RFC-3339-only formatting** — no custom
   format/parse patterns, no `add_months`/`add_years`, no timezone conversion.
4. **No generation functions** — no `uuid_v4()`, no `random()`; the docs'
   `{"$fn": "UUID"}` example is also fiction today.
5. **Docs/code drift** — `guide/05-functions.md` names functions that don't
   exist (`cast/to_str`, `datetime/format`, `datetime/parse`, `text/camel`,
   `text/snake`, `crypto/hash_sha256`, a `compare/` folder, a `value/`
   folder) while the real names differ (`cast/to_string`,
   `datetime/format_rfc3339`, `crypto/sha256`, `value_nav/`). For early
   adopters whose only discovery mechanism is the docs, this is functionally
   equivalent to the functions being missing.

Wider picture: the query language's "missing" JOIN/window/CTE/UNION are
**documented deliberate design** (object-native OQL, batch composition +
`$query` refs + ForEach + stored procedures instead — see
`completeness-oql.md` and roadmap PLAN §3). TS-client parity is strong (the
2026-06 parity campaigns closed all P0/P1 gaps; ValueCompare / `$cond` /
`$expr` / ForEach / batch limits incl. `max_execution_time_secs` are all
buildable from TS). Ops tooling has a genuinely solid observability/audit
story and an online storage-migration path; the thin spots are restore/PITR
and logical dump/load.

**Verdict: v0.10-ready after a ~1–2 week funclib top-up sprint** (see Part A
§3) plus the docs-drift fix. Nothing found is an architectural blocker.

---

## Part A — Function library breadth (primary)

### A.1 Complete inventory of registered functions

All scalar names are **folder-qualified** at dispatch time
(`register_builtins()` wires each module via `in_folder`), i.e. a query calls
`strings/lower`, `math/abs`, `datetime/now`. Aggregates use **plain** names.
Error handling is machine-code-only (`type_mismatch`, `out_of_range`, …).

| Folder | Count | Functions (dispatch names) |
|---|---|---|
| `math/` | 17 | `abs ceil floor round(x[,dp]) trunc sign neg pow sqrt exp ln log(x[,base]) mod clamp min(n-ary) max(n-ary) between` |
| `strings/` | 27 | `lower upper trim ltrim rtrim length byte_length substring concat replace split starts_with ends_with contains index_of repeat reverse pad_left pad_right` + regex family `is_reg_match reg_query reg_query_all reg_captures reg_replace reg_split reg_count reg_find_index` (compiled-regex cache, bounded at 256) |
| `arrays/` | 15 | `length get slice contains index_of first last flatten distinct sort join sum min max avg` |
| `cast/` | 8 | `to_int to_float to_dec to_string to_bool parse_int parse_float try_cast(v, type_name)` |
| `datetime/` | 23 | `now age` (impure) · `to_epoch_s to_epoch_ms from_epoch_s from_epoch_ms parse_rfc3339 format_rfc3339 year month day hour minute second weekday is_weekend add_secs add_days diff_secs start_of_day start_of_week start_of_month truncate(unit)` — canonical timestamp is epoch-millis UTC `i64` |
| `value_nav/` | 5 | `get_path array_length keys type_of exists` |
| `validate/` | 12 | `is_email is_url is_uuid is_ipv4 is_ipv6 is_phone luhn in_range matches is_json is_empty len_between` |
| `encode/` | 12 | `base64_enc base64_dec base64url_enc base64url_dec hex_enc hex_dec base32_enc base32_dec url_encode url_decode html_escape json_escape` |
| `object/` | 8 | `keys values entries has_key get_path merge pick omit` |
| `text/` | 7 | `normalize_nfc normalize_nfkc slugify levenshtein jaro_winkler word_count truncate_ellipsis` |
| `crypto/` | 8 | `sha256 sha512 sha3_256 blake3 hmac_sha256 ct_eq argon2id` (semaphore-gated, cap 16) + `canonical_hash` (order-independent record hash, CAS protocol) |
| **Scalars total** | **142** | (`register_builtins_tests.rs` asserts ≥ 130) |

**Aggregates** (`agg.rs`, 18): `count count_distinct sum avg min max median
stddev variance percentile first last string_agg array_agg bool_and bool_or
mode range`. SQL null-skipping semantics; decimal-first numerics; sensible
empty-input conventions documented in the module header.

Adjacent expression surfaces that overlap the funclib role:

- **`$expr`** (`FilterExprOp`, evaluated in `resolve.rs::eval_filter_expr`):
  `add sub mul div mod neg concat lower upper trim length and or not
  eq ne gt gte lt lte` — covers arithmetic/logic that funclib deliberately
  does not register (per the `oql-02-expr-fate-adr`).
- **`$cond`** ternary (with the #643 per-query compile cache) provides
  CASE-WHEN-like conditional selection.
- **`$query` / `$param` / `$ref`** cover cross-query and cross-field
  references; `Filter::Computed` exposes functional-index-accelerated
  `lower/upper/trim/length/substring/mod` comparisons.

### A.2 Gap table vs. a "reasonably complete" DBMS function library

Legend: ✅ present · 🟡 partial/awkward · ❌ absent.

| Capability family | Status | Detail |
|---|---|---|
| **Null handling as functions** | ❌ | No `coalesce`, `if_null`, `nullif` anywhere (grep across all crates: only filter ops `IsNull`/`IsNotNull` and validator internals). `$cond` can emulate `coalesce(a,b)` but only clumsily (needs an explicit `is_null` condition — which also doesn't exist as a scalar; nearest is `validate/is_empty`). The silent-miss semantics of unresolvable values make `coalesce` *more* important here than in SQL. `fn_call.rs` doc-comment advertises `COALESCE`. |
| String basics (trim/pad/replace/split/concat/case) | ✅ | Full set incl. char-vs-byte length distinction. |
| Regex (match/extract/replace/split/count) | ✅ | 8 functions, ReDoS-safe engine, pattern cache. |
| String formatting/interpolation (`format`/sprintf) | ❌ | Only `concat`. No template/number-formatting function (`format_int`, thousands separators, `%05d`-style). |
| Case-transform extras (`capitalize`, `title_case`, `camel`, `snake`) | ❌ | Docs advertise `text/camel`, `text/snake`; neither exists. `slugify` exists. |
| Math (round/floor/ceil/pow/sqrt/log/mod/clamp) | ✅ | Decimal-first exact ops + f64 transcendentals; `round(x, dp)` supported. Missing only trig (rare in DB workloads) and `log2`. |
| Type casting (`to_int/to_string/to_bool/parse_*`) | ✅ | Solid; `try_cast` dispatches by type name. Doc drift: `to_str` vs. real `to_string`. |
| Datetime component extraction | ✅ | `year…second weekday is_weekend`. Missing: `day_of_year`, `iso_week`, `quarter`, `days_in_month`, `end_of_*`. |
| Datetime arithmetic | 🟡 | `add_secs`/`add_days`/`diff_secs` only. **No `add_months`/`add_years`, no `diff_days`** (users will hand-roll `diff_secs/86400` and get calendar bugs). |
| Datetime format/parse | 🟡 | RFC-3339 only. **No strftime-style `format(ts, pattern)` / `parse(s, pattern)`** — the single most common datetime need in reporting projections. Docs advertise `datetime/format`/`datetime/parse`; they don't exist. |
| Timezone support | ❌ | Everything is UTC. No `to_timezone(ts, "Europe/…")` for display-side grouping (`start_of_day` in a local tz is unrepresentable). |
| Array accessors/slicing | ✅ | `get slice first last index_of contains length`. |
| Array set-ops / restructuring | 🟡 | `flatten distinct join` exist. **Missing: `reverse`, `concat` (two arrays), `union`, `intersect`, `difference`, `zip`, `chunk`.** |
| Array `sort` | 🟡 | **Numeric-only** (coerces every element via `arg_dec`; a string array → `type_mismatch`). Cross-type `compare()` already exists in the crate and is used by `min`/`max` — sort simply wasn't wired to it. Surprising failure for early adopters. |
| Higher-order array ops (map/filter/reduce with lambda) | ❌ (by design) | The scalar ABI is `fn(&[QueryValue])` — no closure surface. WASM user scalars are the documented escape hatch; ForEach covers the batch-level case. Acceptable for v0.10; document the recipe. |
| JSON/document ops | 🟡 | Navigation (`value_nav/get_path`, `object/get_path`, `keys/values/entries/has_key`), reshaping (`merge/pick/omit`) are good. **Missing: `parse_json` (Str → Map/List) and `to_json` (value → Str)** — `validate/is_json` validates but cannot *parse*. Also missing immutable `set_path`/`remove_path` and `deep_merge` (current `merge` is shallow) and `from_entries`. |
| Conditional helpers | ✅ | `$cond` ternary + `math/clamp` + n-ary `min`/`max` (= LEAST/GREATEST) + `between`. |
| Hashing | ✅ | sha256/512, sha3, blake3, hmac, argon2id (DoS-gated), canonical record hash, `ct_eq`. |
| Encoding | ✅ | base64/base64url/base32/hex both directions, url encode/decode, html/json escape. |
| **Generation (uuid/random)** | ❌ | No `uuid_v4()`, `random()`, `random_bytes(n)`. The registry already models impure/non-deterministic entries (`datetime/now` sets `pure:false`), so there is no architectural obstacle. Docs advertise `{"$fn": "UUID"}`. |
| Aggregates beyond count/sum/avg/min/max | ✅ | median, mode, stddev, variance, percentile, count_distinct, string_agg, array_agg, bool_and/or, first/last, range — **above** typical NoSQL baseline. |
| **Aggregate parameterisation on the wire** | ❌ | `agg.rs` exposes `percentile(p)` / `string_agg(sep)` factories, but `SelectItem::AggregateFn { name, field, alias, distinct }` has no params field and `aggregate.rs:654` calls `builtin_aggs().make(name)` — so only p=0.5 and sep="," are reachable by clients. |
| Aggregate DISTINCT modifier | 🟡 | `count_distinct` works (own name). The generic `distinct: bool` on `Aggregate`/`AggregateFn` is deserialized but **ignored** by the grouped executor (`aggregate.rs:619,640` destructure with `..`) — `sum`/`avg`/`string_agg` DISTINCT silently degrade to non-distinct. Should be honored or rejected, never ignored. |
| Filtered aggregates (`FILTER (WHERE …)`) | ❌ | Not expressible per-aggregate; whole-query `where` + separate batch entries is the workaround. Low priority. |
| min_by/max_by (arg-extremum), top_k | ❌ | Common in analytics; medium priority. |

### A.3 Prioritized additions recommended for v0.10

**P0 — ship before tagging v0.10** (each is small, pure, pattern-copyable
from an existing module):

| # | Function(s) | Namespace | Rationale |
|---|---|---|---|
| 1 | `coalesce(v…)` (n-ary, first non-null), `if_null(v, default)`, `nullif(a, b)`, `is_null(v)→Bool` | new `null/` folder (or `value_nav/`) | The most-used transformation family in any filter/projection language; already advertised in `fn_call.rs` docs; `$cond` emulation is impossible without `is_null` anyway. |
| 2 | Wire params for `percentile`/`string_agg` (add optional `args: Vec<FilterValue>` to `SelectItem::AggregateFn`) + honor-or-reject the `distinct` flag | `agg` | The engine-side factories already exist; the wire is the only missing piece. Silent-wrong-results (ignored `distinct`, silently-median `percentile`) are trust-killers for a DB. |
| 3 | `datetime/format(ts, pattern)` + `datetime/parse(s, pattern)` (chrono `format`/`parse_from_str`) | `datetime/` | #1 datetime need for any report/projection; chrono is already a dependency; docs already promise them. |
| 4 | `uuid_v4()` (and optionally `random()`, `random_bytes(n)`) as `pure:false` entries | new `gen/` folder | Client-generated IDs are the current workaround; server-side default-value generation (computed DEFAULT) can't mint IDs today. Registry purity model already supports impure fns. |
| 5 | Fix `arrays/sort` to use cross-type `compare()` (keep numeric fast path), add `arrays/sort_desc` | `arrays/` | Sorting a string array is a day-one operation; current `type_mismatch` will read as a bug. |
| 6 | `parse_json(s)` / `to_json(v)` | `encode/` or `cast/` | Document DB without a JSON-string bridge is awkward at integration boundaries (webhook payloads stored as strings, etc.). A serde bridge already exists in the codebase. |

**P1 — strongly recommended, same release train if capacity allows:**

| # | Function(s) | Namespace | Rationale |
|---|---|---|---|
| 7 | `add_months`, `add_years`, `diff_days`, `end_of_day/month`, `quarter`, `iso_week` | `datetime/` | Calendar arithmetic users otherwise hand-roll incorrectly. |
| 8 | `arrays/reverse`, `arrays/concat`, `arrays/union`, `arrays/intersect`, `arrays/difference` | `arrays/` | Standard collection algebra; `distinct` already established the FxSet pattern. |
| 9 | `object/set_path(m, path, v)`, `object/remove_path`, `object/deep_merge` | `object/` | Rounds out immutable document reshaping (merge is shallow-only today). |
| 10 | `strings/capitalize`, `strings/format` (positional `{}` interpolation), `text/camel`, `text/snake` | `strings/`, `text/` | Display-layer basics; two of these are already (falsely) documented. |
| 11 | `min_by(field)/max_by(field)` aggregates | `agg` | Frequent "row with max value" pattern; avoids a second query. |
| 12 | `to_timezone(ts, tz_name)` + tz-aware `start_of_day` | `datetime/` | Needs `chrono-tz` (new dep) — largest item here; can slip to v0.11 if dep-budget is tight, but flag it in release notes as a known limitation. |

**P2 — nice-to-have / v0.11:** trig + `log2`, `crc32`/`sha1` (compat),
`strings/translate`, `arrays/zip`/`chunk`, filtered aggregates,
`corr`/`covar`, `top_k`.

**Deliberately NOT recommended:** lambda-based `map`/`filter`/`reduce`
scalars (the pure `fn(&[QueryValue])` ABI has no closure surface; WASM user
scalars + ForEach are the sanctioned answer — document the recipe instead).

---

## Part B — Wider release-readiness survey (secondary)

### B.1 Query-language completeness

Current model (confirmed in source): **object-native OQL** — MessagePack
DTOs, no textual language. Read pipeline: index/full scan → WHERE (22 filter
ops + `Fts` + `VectorSimilarity` + `Computed` functional-index ops) →
GROUP BY/HAVING → SELECT (fields, fast-path aggregates, `AggregateFn`
funclib aggregates, `Function` row-level scalars) → DISTINCT → ORDER BY →
pagination (incl. keyset) — plus temporal `as-of` reads, `with_version`,
`explain`. Composition happens at the **batch** level: named entries, DAG
planning over `$query` refs, `when` guards (ValueCompare), nested sub-batches
with `$param` binding, `ForEach` loops, `Call` stored procedures,
subscriptions, and interactive transactions.

**JOIN/window/CTE/UNION absence is a documented design decision**, not a
gap: `completeness-oql.md` (status section, 2026-06-26) marks H1 JOIN, H2
window, H4 set-ops, H5 correlated subqueries, M1 CTE as "intentionally out of
scope — object-native by design," replaced by batch composition + stored
procedures. I found no reason to reopen this for v0.10; the batch DAG +
`$query` column refs (`[].field`) genuinely cover the lookup-join and
semi-join patterns early adopters will need first.

Real (small) gaps already tracked in that doc and confirmed still present:
- `SelectItem::Expression` is parsed but **not executed** (`exec.rs` rejects /
  `aggregate.rs:663` no-op) — fine as a deferral, but the wire type's
  existence invites silent no-ops from hand-built clients. Consider rejecting
  it with an explicit error code at parse time for v0.10.
- FTS has no ranking/score/highlight surface.
- Generated columns are write-time only (computed DEFAULT + `created_at`/
  `updated_at` stamping in `apply_transforms`); no always-recompute or
  virtual columns.
- Wire `AggFunc` fast path is Count/Sum/Avg/Min/Max; everything else rides
  `AggregateFn` (works, but see the parameterisation gap in Part A).

### B.2 Client SDK parity

- **TS client (`crates/shamir-client-ts`)** — parity is good. Verified
  present: `ValueCompare` (filter builder + types + e2e `when` test),
  `$fn`/`$expr`/`$cond`/`$param`/`$query` in `types/filter.ts`,
  `select.aggregateFn(...)` and `select.func(...)` (funclib aggregates and
  row-level scalars), `forEach(...)` in the batch builder,
  `max_execution_time_secs` in batch limits (default 30) with client-side
  deadline handling in `client.ts`, subscribe/replication/admin/DDL builder
  families. The 2026-06 parity campaigns (per `coverage-ts-query-builder.md`)
  closed all P0/P1 gaps; the one deliberate hold-back is a `SelectExpr`
  builder (correct — the engine doesn't execute it).
- **Rust builder (`shamir-query-builder`)** — `ValueCompare` and
  `aggregate_fn` present (`filter/leaf.rs`, `select/select_item.rs`).
- **Residual parity risk** is *discoverability*, not wire coverage: nothing
  in any client can enumerate the built-in scalar/aggregate catalogue
  (`ScalarRegistry::names()` exists server-side but has no wire exposure I
  could find), so users depend on `guide/05-functions.md` — which names
  ~10 functions that don't exist and mis-names folders (see exec summary
  point 5). Either fix the doc table to match the registry exactly, or add a
  `list_builtin_functions` introspection op (or both).

### B.3 Operational tooling

Present and credible:
- **Backup:** `shamir-server backup --to <dir>` one-shot snapshot copy of
  `data_dir` (`backup.rs`, wired in `main.rs`); documented in
  `guide/07-operations.md` §8.
- **Observability:** dedicated HTTP endpoint (`observability.rs`) with
  `/healthz`, `/readyz`, `/metrics` (Prometheus: process metrics + auth/tx/
  SSI-abort/GC counters), `/info`; loopback-by-default; Grafana/alert
  examples in `deploy/`.
- **Service lifecycle:** `service install` for systemd (Type=notify)/Windows
  SCM/launchd/rc.d; graceful drain with 30 s deadline; single-instance file
  lock.
- **Logs/audit:** slow-query WARN threshold, non-blocking batched file
  logging, 14 per-namespace levels with SIGHUP live-reload; HMAC-chained
  audit log with size rotation and retention.
- **Migration:** online storage-engine migration (`start/commit/rollback/
  status` + HMAC), declarative schema ops (`SetTableSchema`,
  `Add/RemoveSchemaRule`, `DescribeTable`), history retention/purge,
  replication pub/sub DDL + `ReplicationStatus`.

Conspicuously thin for production:
1. **No restore path** — backup exists, restore is implicitly "point
   `data_dir` at the copy"; undocumented and unverified. No PITR / WAL
   archiving between snapshots (RPO = snapshot interval).
2. **No logical dump/load (import/export)** — no way to export a table as
   portable data or seed one from a file; also the only cross-version escape
   hatch if on-disk format changes pre-1.0. (Only `InternerDumpOp` exists,
   which is interner introspection, not data export.)
3. **No schema-migration runner** — schema DDL ops exist but there is no
   versioned-migration story (a `migrations/` runner or version stamp);
   fine for v0.10, worth a roadmap note.

### B.4 Untracked-work note

The working tree contains uncommitted changes (`cond_cache.rs`, filter/
resolve/select_projection edits — the #643 `$cond` cache) and a stray
`fix643_test.log` at repo root; the repo's own discipline rules say stray
debug files must not ship in a release tag.

---

## Verdict: is v0.10 release-ready?

**Yes, conditionally** — the architecture, query surface, security posture,
and ops story are past the bar for an early-adopter release, and the "thin
funclib" worry is mostly a targeted-gaps problem, not a breadth problem. The
conditions are the P0 list below; all are small relative to what's already
built.

### Top 5 blockers/recommendations, ranked

1. **Null-handling scalars** (`coalesce` / `if_null` / `nullif` / `is_null`)
   — highest usage-frequency gap in the whole product; already promised by
   the `$fn` docs. (Part A §3 #1)
2. **Stop silently mis-aggregating:** wire-level params for `percentile`/
   `string_agg` and honor-or-reject the ignored `distinct` flag on
   `Aggregate`/`AggregateFn`. Correctness-of-results trust issue, not a
   feature request. (Part A §3 #2)
3. **Docs/introspection alignment for the function catalogue** — fix the
   ~10 phantom names in `guide/05-functions.md` and/or expose a
   `list_builtin_functions` op; every funclib investment is invisible if
   users can't discover real names (`strings/lower`, not `text/camel`).
   (Part B §2)
4. **Datetime custom format/parse + calendar arithmetic**
   (`format(ts, pattern)`, `parse(s, pattern)`, `add_months`, `diff_days`)
   — the most common projection/reporting need not currently expressible.
   (Part A §3 #3, #7)
5. **`uuid_v4()` generation + `arrays/sort` cross-type fix + `parse_json`**
   — three small items that each remove a "wait, it can't do *that*?"
   moment in a new user's first hour. (Part A §3 #4–#6)

Post-v0.10 roadmap candidates (not blockers): timezone support, restore/PITR
+ logical dump/load, FTS ranking, full generated columns, min_by/max_by.
