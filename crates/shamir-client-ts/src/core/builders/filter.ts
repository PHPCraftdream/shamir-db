/**
 * Filter constructors — the CODE that builds the wire `Filter` shapes
 * declared in `../types/filter.ts`. Each function returns a plain object
 * that the Rust server deserialises into `Filter` via
 * `#[serde(tag = "op", rename_all = "snake_case")]`.
 *
 * Field paths accept a bare string ("age") or an explicit path array
 * (["address","city"]) and are normalised to the canonical array form.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  FieldPath,
  FilterValue,
  Filter,
  ComputedFilter,
  ExprOp,
  FnCall,
  FilterExprValue,
  CondValue,
  ValueCompareOp,
} from '../types/filter.js';

/** Normalise a field spec (bare string or path array) to the wire form. */
function fp(field: string | string[]): FieldPath {
  return typeof field === 'string' ? [field] : field;
}

// ── Comparison leaves ────────────────────────────────────────────────

/** `field == value` */
export function eq(field: string | string[], value: FilterValue): Filter {
  return { op: 'eq', field: fp(field), value };
}

/** `field != value` */
export function ne(field: string | string[], value: FilterValue): Filter {
  return { op: 'ne', field: fp(field), value };
}

/** `field > value` */
export function gt(field: string | string[], value: FilterValue): Filter {
  return { op: 'gt', field: fp(field), value };
}

/** `field >= value` */
export function gte(field: string | string[], value: FilterValue): Filter {
  return { op: 'gte', field: fp(field), value };
}

/** `field < value` */
export function lt(field: string | string[], value: FilterValue): Filter {
  return { op: 'lt', field: fp(field), value };
}

/** `field <= value` */
export function lte(field: string | string[], value: FilterValue): Filter {
  return { op: 'lte', field: fp(field), value };
}

// ── Value-vs-value comparison (#651) ─────────────────────────────────
//
// Unlike `eq`/`ne`/`gt`/`gte`/`lt`/`lte` above (which compare a RECORD
// FIELD against a value), these compare TWO independently-resolved
// `FilterValue`s with no record involved — the only comparison shape
// meaningful inside a `when` guard (`Batch.when`/`Batch.switch`), which has
// no per-row record to resolve a field path against. Typical usage
// compares two `$query` refs, e.g.
// `valueGte(queryRef('balance_check', '[0].balance'), 40)` for
// "run this op iff balance >= 40".

function valueCompare(
  left: FilterValue,
  cmp: ValueCompareOp,
  right: FilterValue,
): Filter {
  return { op: 'value_compare', left, cmp, right };
}

/** `left == right` (value-vs-value, no field/record involved). */
export function valueEq(left: FilterValue, right: FilterValue): Filter {
  return valueCompare(left, 'eq', right);
}

/** `left != right` (value-vs-value, no field/record involved). */
export function valueNe(left: FilterValue, right: FilterValue): Filter {
  return valueCompare(left, 'ne', right);
}

/** `left > right` (value-vs-value, no field/record involved). */
export function valueGt(left: FilterValue, right: FilterValue): Filter {
  return valueCompare(left, 'gt', right);
}

/** `left >= right` (value-vs-value, no field/record involved). */
export function valueGte(left: FilterValue, right: FilterValue): Filter {
  return valueCompare(left, 'gte', right);
}

/** `left < right` (value-vs-value, no field/record involved). */
export function valueLt(left: FilterValue, right: FilterValue): Filter {
  return valueCompare(left, 'lt', right);
}

/** `left <= right` (value-vs-value, no field/record involved). */
export function valueLte(left: FilterValue, right: FilterValue): Filter {
  return valueCompare(left, 'lte', right);
}

// ── Field equality shortcut ──────────────────────────────────────────

/**
 * Shortcut equality that serialises as `op: "field"` on the wire
 * (the Rust `Filter::FieldEq` / `#[serde(rename = "field")]` variant).
 */
export function fieldEq(field: string | string[], value: FilterValue): Filter {
  return { op: 'field', field: fp(field), value };
}

// ── Set membership ───────────────────────────────────────────────────

/** `field IN (values…)` */
export function in_(field: string | string[], values: FilterValue[]): Filter {
  return { op: 'in', field: fp(field), values };
}

/** `field NOT IN (values…)` */
export function notIn(field: string | string[], values: FilterValue[]): Filter {
  return { op: 'not_in', field: fp(field), values };
}

// ── Pattern matching ─────────────────────────────────────────────────

/** `field LIKE pattern` */
export function like(field: string | string[], pattern: string): Filter {
  return { op: 'like', field: fp(field), pattern };
}

/** Case-insensitive `LIKE` — serialises as `op: "i_like"`. */
export function ilike(field: string | string[], pattern: string): Filter {
  return { op: 'i_like', field: fp(field), pattern };
}

/** `field ~ pattern` (regex match) */
export function regex(field: string | string[], pattern: string): Filter {
  return { op: 'regex', field: fp(field), pattern };
}

// ── Null / existence ─────────────────────────────────────────────────

/** `field IS NULL` */
export function isNull(field: string | string[]): Filter {
  return { op: 'is_null', field: fp(field) };
}

/** `field IS NOT NULL` */
export function isNotNull(field: string | string[]): Filter {
  return { op: 'is_not_null', field: fp(field) };
}

/** Field exists in the record. */
export function exists(field: string | string[]): Filter {
  return { op: 'exists', field: fp(field) };
}

/** Field does not exist in the record. */
export function notExists(field: string | string[]): Filter {
  return { op: 'not_exists', field: fp(field) };
}

// ── Containment ──────────────────────────────────────────────────────

/** Array field contains `value`. */
export function contains(field: string | string[], value: FilterValue): Filter {
  return { op: 'contains', field: fp(field), value };
}

/** Array field contains any of `values`. */
export function containsAny(
  field: string | string[],
  values: FilterValue[],
): Filter {
  return { op: 'contains_any', field: fp(field), values };
}

/** Array field contains all of `values`. */
export function containsAll(
  field: string | string[],
  values: FilterValue[],
): Filter {
  return { op: 'contains_all', field: fp(field), values };
}

// ── Range ────────────────────────────────────────────────────────────

/** `from <= field <= to` */
export function between(
  field: string | string[],
  from: FilterValue,
  to: FilterValue,
): Filter {
  return { op: 'between', field: fp(field), from, to };
}

// ── Full-text search ─────────────────────────────────────────────────

/**
 * Full-text search.
 * @param mode "and" (all tokens must match) or "or" (any). Default: "and".
 */
export function fts(
  field: string | string[],
  query: string,
  mode: 'and' | 'or' = 'and',
): Filter {
  return { op: 'fts', field: fp(field), query, mode };
}

// ── Vector similarity ────────────────────────────────────────────────

/**
 * Per-query tuning options for `vectorSimilarity`.
 *
 * * `efSearch` — per-query HNSW exploration width. Higher = better recall,
 *   higher latency. Clamped server-side to `MAX_EF_SEARCH` (10_000).
 * * `oversample` — candidate-widening multiplier on the filtered-ANN path
 *   (default 2.0, clamped to ≥1.0 server-side); a bare vector_similarity
 *   accepts it but does not consume it.
 */
export interface VectorSimilarityOpts {
  efSearch?: number;
  oversample?: number;
}

/**
 * Mirrors `MAX_EF_SEARCH` in
 * `crates/shamir-index/src/vector/hnsw_adapter.rs:44`. The server silently
 * clamps `ef_search` above this value instead of rejecting it; we reject
 * explicitly on the client so the caller learns immediately rather than
 * getting a silently-degraded (clamped) search.
 */
const MAX_EF_SEARCH = 10_000;

/**
 * Top-k nearest-neighbor vector similarity search.
 *
 * Pass an optional 4th `opts` argument to tune the per-query recall/latency
 * trade-off without rebuilding the whole filter:
 *
 *   const f = vectorSimilarity('emb', [1,0,0], 10, { efSearch: 400 });
 *
 * Omitting `opts` (or passing `{}`) yields the pre-V1.1 wire shape — no
 * `ef_search` / `oversample` keys emitted (both default to undefined →
 * skipped by `@msgpack/msgpack` and the Rust `skip_serializing_if`).
 *
 * `ef_search` is clamped server-side to `MAX_EF_SEARCH` (10_000).
 * `oversample` widens the candidate pool on the filtered-ANN path (#404).
 */
export function vectorSimilarity(
  field: string | string[],
  query: number[],
  k: number,
  opts?: VectorSimilarityOpts,
): Filter {
  if (k <= 0) {
    throw new Error(
      `vectorSimilarity: k must be > 0 (got ${k}) — the server would silently ` +
        'return 0 results, which is easy to mistake for "no matches"',
    );
  }
  if (opts?.efSearch !== undefined && opts.efSearch > MAX_EF_SEARCH) {
    throw new Error(
      `vectorSimilarity: ef_search (${opts.efSearch}) exceeds MAX_EF_SEARCH ` +
        `(${MAX_EF_SEARCH}) — the server would silently clamp it instead of ` +
        'rejecting it, which degrades recall without telling the caller',
    );
  }
  const f: Extract<Filter, { op: 'vector_similarity' }> = {
    op: 'vector_similarity',
    field: fp(field),
    query,
    k,
  };
  if (opts?.efSearch !== undefined) f.ef_search = opts.efSearch;
  if (opts?.oversample !== undefined) f.oversample = opts.oversample;
  return f;
}

/**
 * Chainable builder variant — returns a thin wrapper with `.efSearch(n)` /
 * `.oversample(f)` methods for the fluent-call style. Each method returns a
 * fresh builder (immutable). The final `.build()` yields the plain `Filter`.
 *
 *   const f = vs('emb', [1,0,0], 10).efSearch(400).oversample(2).build();
 */
export function vs(
  field: string | string[],
  query: number[],
  k: number,
): VectorSimilarityBuilder {
  return makeVsBuilder({ op: 'vector_similarity', field: fp(field), query, k });
}

/** Chainable builder for `vector_similarity` (fluent variant of [`vectorSimilarity`]). */
export interface VectorSimilarityBuilder {
  /** Set per-query HNSW `ef_search` (exploration width). */
  efSearch(ef: number): VectorSimilarityBuilder;
  /** Set per-query `oversample` (candidate widening on the filtered-ANN path). */
  oversample(f: number): VectorSimilarityBuilder;
  /** Finalize → plain `Filter`. */
  build(): Filter;
}

/**
 * Install the non-enumerable chain methods on a wire object and return it
 * as a builder. Each method returns a FRESH builder (immutable).
 */
function makeVsBuilder(
  wire: Extract<Filter, { op: 'vector_similarity' }>,
): VectorSimilarityBuilder {
  return {
    efSearch(ef: number): VectorSimilarityBuilder {
      return makeVsBuilder({ ...wire, ef_search: ef });
    },
    oversample(f: number): VectorSimilarityBuilder {
      return makeVsBuilder({ ...wire, oversample: f });
    },
    build(): Filter {
      return wire;
    },
  };
}

// ── Computed (functional index) ──────────────────────────────────────

/**
 * Comparison on a computed expression (functional index).
 * `exprOp`: "lower" | "upper" | "trim" | "length" | "substring" | "mod" …
 * `cmp`: "eq" | "lt" | "gt" | "lte" | "gte"
 */
export function computed(
  exprOp: string,
  field: string | string[],
  cmp: string,
  value: FilterValue,
  exprArgs?: FilterValue[],
): Filter {
  const f: ComputedFilter = {
    op: 'computed',
    expr_op: exprOp,
    field: fp(field),
    cmp,
    value,
  };
  if (exprArgs !== undefined) f.expr_args = exprArgs;
  return f;
}

// ── Batch parameter reference ────────────────────────────────────────

/**
 * Reference to a named parameter bound at the outer batch level.
 * Produces `{ "$param": name }` on the wire — matches the server's
 * `FilterValue::Param` variant.
 */
export function param(name: string): { $param: string } {
  return { $param: name };
}

// ── Value references ────────────────────────────────────────────────

/**
 * Reference to another query's result in the same batch.
 * `alias` is the `@alias` string, e.g. `'@users'`.
 * `path` is optional — when provided it extracts a scalar (`'[0].id'`)
 * or a column (`'[].id'`) from the upstream result.
 */
export function queryRef(
  alias: string,
  path?: string,
): { $query: string; path?: string } {
  const v: { $query: string; path?: string } = { $query: alias };
  if (path !== undefined) v.path = path;
  return v;
}

/**
 * Reference to another field in the same document.
 * A bare string is normalised to a 1-element path array.
 */
export function ref(field: string | string[]): { $ref: FieldPath } {
  return { $ref: fp(field) };
}

// ── Function / Expression / Conditional value constructors ────────────

/**
 * System function call (`$fn`).
 *
 * When `args` is omitted or empty the wire form is the bare-string Simple
 * variant (`{ "$fn": "NOW" }`), matching `FnCall::Simple` in Rust. Otherwise
 * the Complex variant is emitted (`{ "$fn": { "name": ..., "args": [...] } }`).
 */
export function fn(name: string, args?: FilterValue[]): { $fn: FnCall } {
  if (!args || args.length === 0) {
    return { $fn: name };
  }
  return { $fn: { name, args } };
}

/**
 * Expression (`$expr`) — arithmetic, string, logic, comparison.
 *
 * Mirrors `FilterExpr { op, args }` in `filter_expr.rs`.
 */
export function expr(op: ExprOp, args: FilterValue[]): { $expr: FilterExprValue } {
  return { $expr: { op, args } };
}

/**
 * Conditional (`$cond`) — ternary operator.
 *
 * Mirrors `Cond { if, then, else }` in `cond.rs`.
 */
export function cond(
  ifFilter: Filter,
  then: FilterValue,
  orElse: FilterValue,
): { $cond: CondValue } {
  return { $cond: { if: ifFilter, then, else: orElse } };
}

/**
 * Switch-case sugar over {@link cond} — folds an ordered list of
 * `[condition, value]` cases plus a `default` into a right-associated chain
 * of nested `$cond`s, so 4+ branches don't require hand-nested parens.
 *
 * Cases are evaluated in order: the first `condition` that holds wins; if
 * none hold, `default` is the result.
 *
 * ```ts
 * switchCase(
 *   [
 *     [gte('score', 100), 'vip'],
 *     [gte('score', 50), 'regular'],
 *   ],
 *   'newbie',
 * )
 * // == cond(gte('score', 100), 'vip', cond(gte('score', 50), 'regular', 'newbie'))
 * ```
 */
export function switchCase(
  cases: [Filter, FilterValue][],
  defaultValue: FilterValue,
): FilterValue {
  return cases.reduceRight<FilterValue>(
    (acc, [ifFilter, then]) => cond(ifFilter, then, acc),
    defaultValue,
  );
}

// ── Binary / u64 literal helpers ────────────────────────────────────

/**
 * Binary literal constructor. Mirrors Rust `bin()` (`filter_value.rs:75`)
 * → `FilterValue::Binary`.
 *
 * Sugar-normaliser: `number[]` is converted to `new Uint8Array(bytes)`;
 * a `Uint8Array` passes through unchanged. The returned `Uint8Array` is
 * directly valid as a `FilterValue` (wire: `Binary`).
 */
export function bin(bytes: Uint8Array | number[]): Uint8Array {
  return bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
}

/**
 * u64 filter literal. Mirrors Rust `lit_u64` (`filter_value.rs`).
 *
 * Unified u64 contract (FG-1): values representable in `i64`
 * (`<= 9223372036854775807`) stay a `number` (unchanged, msgpack-safe
 * integer). Values above `i64::MAX` become their EXACT decimal `string` —
 * matching how Rust `Value::Big` / `QueryValue::Big` serialises on the wire
 * (`serializer.serialize_str(&b.to_string())`) and how Rust
 * `FilterValue::String(v.to_string())` represents the same overflow case.
 *
 * JS strings are exact for arbitrary-precision decimal text, so no `bigint`
 * is needed on this side for correctness (the value only needs a wire
 * representation here, never arithmetic). The old behaviour wrapped via
 * `Number(v)`, silently losing precision above `2^53` AND sign-flipping
 * above `i64::MAX`; that silent data corruption is gone.
 *
 * NO runtime range checks are added — no throw.
 */
export function litU64(v: bigint | number): number | string {
  if (typeof v === 'bigint') {
    if (v <= 9223372036854775807n) {
      return Number(v);
    }
    return v.toString();
  }
  // number input: in the safe i64 range it passes through unchanged; a
  // `number` already above i64::MAX (only reachable via lossy callers) is
  // emitted as its exact decimal text to stay lossless.
  if (v <= 9223372036854775807) {
    return v;
  }
  // Above i64::MAX the number is emitted as its exact decimal string (no
  // further-precision `Number()` round-trip).
  return v.toString();
}

// ── Logical combinators ──────────────────────────────────────────────

/**
 * AND — with smart flattening: if `a` is already an `and` filter, the new
 * filter is appended to its `filters` array rather than nesting (mirrors
 * `FilterExt::and`).
 */
export function and(a: Filter, b: Filter): Filter;
export function and(filters: Filter[]): Filter;
export function and(aOrFilters: Filter | Filter[], b?: Filter): Filter {
  if (Array.isArray(aOrFilters)) {
    return { op: 'and', filters: aOrFilters };
  }
  const a = aOrFilters;
  if (a.op === 'and') {
    return { op: 'and', filters: [...a.filters, b!] };
  }
  return { op: 'and', filters: [a, b!] };
}

/**
 * OR — with smart flattening: if `a` is already an `or` filter, the new
 * filter is appended to its `filters` array rather than nesting.
 */
export function or(a: Filter, b: Filter): Filter;
export function or(filters: Filter[]): Filter;
export function or(aOrFilters: Filter | Filter[], b?: Filter): Filter {
  if (Array.isArray(aOrFilters)) {
    return { op: 'or', filters: aOrFilters };
  }
  const a = aOrFilters;
  if (a.op === 'or') {
    return { op: 'or', filters: [...a.filters, b!] };
  }
  return { op: 'or', filters: [a, b!] };
}

/** Negate a filter. */
export function not(filter: Filter): Filter {
  return { op: 'not', filter };
}

/** Aggregate namespace — every filter constructor in one object. */
export const filter = {
  eq,
  ne,
  gt,
  gte,
  lt,
  lte,
  valueEq,
  valueNe,
  valueGt,
  valueGte,
  valueLt,
  valueLte,
  fieldEq,
  in_,
  notIn,
  like,
  ilike,
  regex,
  isNull,
  isNotNull,
  exists,
  notExists,
  contains,
  containsAny,
  containsAll,
  between,
  fts,
  vectorSimilarity,
  computed,
  and,
  or,
  not,
  queryRef,
  ref,
  param,
  fn,
  expr,
  cond,
  switchCase,
  bin,
  litU64,
};
