/**
 * Write-operation wire types — type-only mirror of
 * `crates/shamir-query-types/src/write/`.
 *
 * Pure type declarations; the constructor / builder code that assembles these
 * shapes lives in `../../builders/write.ts`.
 *
 * Serde notes encoded here (so the builder emits the exact wire shape):
 *   - fields with `skip_serializing_if` are OPTIONAL (`?`) here — the builder
 *     omits them at their default;
 *   - fields with only `#[serde(default)]` (no skip) are ALWAYS present on
 *     the wire (e.g. `UpdateSelect.return_mode`);
 *   - `UpdateOp.where_clause` is `#[serde(rename = "where")]` — the wire key
 *     is `"where"`;
 *   - `UpdateReturnMode` uses `rename_all = "lowercase"`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { TableRefWire } from './query.js';
import type { Filter, FilterValue } from './filter.js';

// Re-export TableRefWire for the write builder.
export type { TableRefWire } from './query.js';

// ── Wire value ───────────────────────────────────────────────────────

/**
 * Recursive MessagePack-compatible wire value type. Represents any value
 * carried in write-operation fields (`InsertOp.values`, `UpdateOp.set`,
 * `SetOp.key`, `SetOp.value`) as decoded from the msgpack wire encoding.
 */
export type WireValue =
  | null
  | boolean
  | number
  | string
  | WireValue[]
  | { [key: string]: WireValue };

/**
 * Computed-expression forms (`$fn` / `$ref` / `$query` / `$expr` / `$cond` /
 * `$param`) admitted inside a write-operation value. These mirror the object
 * variants returned by the `filter.fn()` / `filter.ref()` / `filter.queryRef()`
 * / `filter.expr()` / `filter.cond()` / `filter.param()` constructors (now
 * typed to return the narrow per-variant shape).
 *
 * Defined as a structural shape — not `Extract<FilterValue, …>` — so that a
 * record mixing literals and expressions is assignable to `WriteValue`: the
 * full `FilterValue` union also carries `Uint8Array`, which would make every
 * field of such a record incompatible with the recursive record/object arm of
 * `WriteValue`.
 *
 * Mirrors the Rust invariant that `FilterValue` and `QueryValue` share the same
 * serde wire encoding — `write::Doc::set` accepts `impl Into<FilterValue>` for
 * both literals and computed expressions (see
 * `crates/shamir-query-builder/src/write/doc.rs`).
 */
export type ComputedExpr =
  | { $ref: (string | number)[] }
  | { $query: string; path?: string }
  | { $fn: string | { name: string; args?: FilterValue[] } }
  | { $expr: { op: string; args: FilterValue[] } }
  | { $cond: { if: Filter; then: FilterValue; else: FilterValue } }
  | { $param: string };

/**
 * Value accepted at a write-operation field position (`write.insert`,
 * `UpdateBuilder.set`, `write.upsert`). Extends `WireValue` with the computed
 * expression forms (`ComputedExpr`) so the idiomatic JS literal works without a
 * cast:
 *
 * ```ts
 * write.insert('events', { created_at: filter.fn('NOW'), total: filter.ref('price') })
 * ```
 *
 * `ComputedExpr` carries the `$`-tagged expression objects only (not the full
 * `FilterValue` union, which also includes `Uint8Array`); the `filter.*`
 * constructors return the same narrow variants, so a record mixing literals and
 * expressions type-checks cleanly against the recursive record/object arm.
 */
export type WriteValue =
  | null
  | boolean
  | number
  | string
  | WriteValue[]
  | ComputedExpr
  | { [key: string]: WriteValue };

// ── Update select types ──────────────────────────────────────────────

/**
 * Mode for returning records from an UPDATE operation.
 * `rename_all = "lowercase"`, default = `"changed"`.
 */
export type UpdateReturnMode = 'all' | 'changed' | 'unchanged';

/**
 * Configuration for selecting results from an UPDATE operation.
 *
 * `return_mode` is `#[serde(default)]` WITHOUT skip → **always emitted**
 * when an `UpdateSelect` is present (even at its default `"changed"`).
 * `fields` is `skip_serializing_if = "Option::is_none"` → omitted when absent.
 */
export interface UpdateSelect {
  return_mode: UpdateReturnMode;
  fields?: string[];
}

/**
 * Configuration for returning records from a DELETE operation.
 *
 * DELETE has no changed/unchanged mode — every matched row is removed —
 * so the only knob is an optional field projection. `fields` is
 * `skip_serializing_if = "Option::is_none"` → omitted when absent. The mere
 * presence of a `DeleteSelect` on a `DeleteOp` opts in to RETURNING.
 */
export interface DeleteSelect {
  fields?: string[];
}

/**
 * Optional projection over records returned from INSERT.
 *
 * INSERT always returns the inserted rows when the caller asks for results;
 * `InsertSelect` only carries an optional field projection. `fields` is
 * `skip_serializing_if = "Option::is_none"` → omitted when absent.
 */
export interface InsertSelect {
  fields?: string[];
}

// ── Write operations ─────────────────────────────────────────────────

/**
 * INSERT operation (`write/types.rs`). `insert_into` is a `TableRef`
 * (bare string for repo "main", or `[repo, table]` tuple). `values` is a
 * non-empty array of records.
 *
 * `select` is `skip_serializing_if = "Option::is_none"` → omitted unless
 * `opts.returningFields` was passed to `insert(...)`.
 */
export interface InsertOp {
  insert_into: TableRefWire;
  values: WireValue[];
  select?: InsertSelect;
  /**
   * Id-keyed msgpack-encoded record payloads for the smart-write (id-on-wire)
   * path (Stage 5-wire). Each element is ONE record's id-keyed storage msgpack
   * (field names replaced by their interned u64 ids). Set by
   * `executeWithTouch` on v2 servers for fully-literal records; records
   * carrying `$fn`/computed markers stay on `values`.
   *
   * Mirrors `write/types.rs::InsertOp.records_idmsgpack`
   * (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`, msgpack `bin`
   * via `serde_bytes`) — a PER-OP (per query-entry) field, NOT a batch-level
   * one. Omitted when empty.
   */
  records_idmsgpack?: Uint8Array[];
}

/**
 * UPDATE operation (`write/types.rs`).
 *   - `where` is `#[serde(rename = "where", skip_serializing_if = "Option::is_none")]`
 *     → omitted when no filter is set.
 *   - `set` is always present (required).
 *   - `select` is `skip_serializing_if = "Option::is_none"` → omitted unless
 *     `.returning()` was called.
 */
export interface UpdateOp {
  update: TableRefWire;
  where?: Filter;
  set: WireValue;
  select?: UpdateSelect;
  /** Optimistic-concurrency (CAS) version guard (FG-2). When set, the server
   * rejects the update with `version_conflict` unless every matched row is at
   * exactly this version (from `QueryResult.versions`). Omitted = disabled. */
  expected_version?: number;
}

/**
 * SET (upsert) operation (`write/types.rs`). Upserts by key: updates if the
 * key matches, inserts otherwise.
 */
export interface SetOp {
  set: TableRefWire;
  key: WireValue;
  value: WireValue;
}

/**
 * DELETE operation (`write/types.rs`).
 *   - `where` is `#[serde(rename = "where")]` — **required** (no skip),
 *     always present on the wire.
 *   - `select` is `#[serde(default, skip_serializing_if = "Option::is_none")]`
 *     → omitted unless `opts.returning` / `opts.returningFields` was passed
 *     to `del(...)`.
 */
export interface DeleteOp {
  delete_from: TableRefWire;
  where: Filter;
  select?: DeleteSelect;
  /** Optimistic-concurrency (CAS) version guard (FG-2). Same semantics as
   * `UpdateOp.expected_version`. */
  expected_version?: number;
}

/** Union of all write operation wire shapes. */
export type WriteOp = InsertOp | UpdateOp | SetOp | DeleteOp;
