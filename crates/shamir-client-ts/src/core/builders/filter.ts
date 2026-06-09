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

/** Top-k nearest-neighbor vector similarity search. */
export function vectorSimilarity(
  field: string | string[],
  query: number[],
  k: number,
): Filter {
  return { op: 'vector_similarity', field: fp(field), query, k };
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
};
