/**
 * Filter wire types — type-only mirror of
 * `crates/shamir-query-types/src/filter/`.
 *
 * These are pure TypeScript type declarations (no runtime code). The
 * constructor functions that BUILD these shapes live in
 * `../../builders/filter.ts`. Types and code are kept apart.
 *
 * PLATFORM-AGNOSTIC.
 */

// ── FieldPath ────────────────────────────────────────────────────────

/**
 * A document path expressed as an array of field-name segments.
 * The Rust `de_field_path` helper also accepts a bare string for a
 * top-level field, but the canonical serialised form is always an array.
 */
export type FieldPath = string[];

// ── FilterValue ──────────────────────────────────────────────────────

/** System function call (`$fn`) — `FnCall` in `fn_call.rs` (untagged). */
export type FnCall = string | { name: string; args?: FilterValue[] };

/** Expression operator for `$expr`. Mirrors `FilterExprOp` (serde `rename_all = "lowercase"`). */
export type ExprOp =
  | 'add' | 'sub' | 'mul' | 'div' | 'mod' | 'neg'
  | 'concat' | 'lower' | 'upper' | 'trim' | 'length'
  | 'and' | 'or' | 'not'
  | 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte';

/** Expression value (`$expr`) — mirrors `FilterExpr` in `filter_expr.rs`. */
export interface FilterExprValue {
  op: ExprOp;
  args: FilterValue[];
}

/** Conditional value (`$cond`) — mirrors `Cond` in `cond.rs`. */
export interface CondValue {
  if: Filter;
  then: FilterValue;
  else: FilterValue;
}

/**
 * Scalar / composite value accepted in filter positions.
 * Mirrors `FilterValue` in `filter_value.rs` (`#[serde(untagged)]`):
 * Null / Bool / Int / Float / String / Binary / Array / FieldRef /
 * QueryRef / FnCall / Expr / Cond.
 */
export type FilterValue =
  | null
  | boolean
  | number
  | string
  | Uint8Array
  | FilterValue[]
  | { $ref: FieldPath }
  | { $query: string; path?: string }
  | { $fn: FnCall }
  | { $expr: FilterExprValue }
  | { $cond: CondValue }
  | { $param: string };

// ── Filter ───────────────────────────────────────────────────────────

/**
 * Discriminated union of all filter variants. The `op` tag drives Rust
 * deserialization via `#[serde(tag = "op", rename_all = "snake_case")]`
 * in `filter_enum.rs`.
 */
export type Filter =
  | { op: 'eq'; field: FieldPath; value: FilterValue }
  | { op: 'ne'; field: FieldPath; value: FilterValue }
  | { op: 'gt'; field: FieldPath; value: FilterValue }
  | { op: 'gte'; field: FieldPath; value: FilterValue }
  | { op: 'lt'; field: FieldPath; value: FilterValue }
  | { op: 'lte'; field: FieldPath; value: FilterValue }
  | { op: 'like'; field: FieldPath; pattern: string }
  | { op: 'i_like'; field: FieldPath; pattern: string }
  | { op: 'regex'; field: FieldPath; pattern: string }
  | { op: 'is_null'; field: FieldPath }
  | { op: 'is_not_null'; field: FieldPath }
  | { op: 'in'; field: FieldPath; values: FilterValue[] }
  | { op: 'not_in'; field: FieldPath; values: FilterValue[] }
  | { op: 'contains'; field: FieldPath; value: FilterValue }
  | { op: 'contains_any'; field: FieldPath; values: FilterValue[] }
  | { op: 'contains_all'; field: FieldPath; values: FilterValue[] }
  | { op: 'between'; field: FieldPath; from: FilterValue; to: FilterValue }
  | { op: 'exists'; field: FieldPath }
  | { op: 'not_exists'; field: FieldPath }
  | { op: 'and'; filters: Filter[] }
  | { op: 'or'; filters: Filter[] }
  | { op: 'not'; filter: Filter }
  | { op: 'field'; field: FieldPath; value: FilterValue }
  | {
      op: 'fts';
      field: FieldPath;
      query: string;
      /** "and" (all tokens must match) or "or" (any). Default: "and". */
      mode?: string;
    }
  | {
      op: 'vector_similarity';
      field: FieldPath;
      query: number[];
      k: number;
      /** V1.1: per-query HNSW exploration width. Higher = better recall, higher latency. */
      ef_search?: number;
      /**
       * V1.1: candidate-widening multiplier. Accepted on the wire but
       * NOT yet consumed server-side (reserved for P3 #404).
       */
      oversample?: number;
    }
  | {
      op: 'computed';
      expr_op: string;
      field: FieldPath;
      expr_args?: FilterValue[];
      cmp: string;
      value: FilterValue;
    };

/** The `computed` filter variant, narrowed (used by the builder). */
export type ComputedFilter = Extract<Filter, { op: 'computed' }>;
