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
import type { Filter } from './filter.js';

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

// ── Write operations ─────────────────────────────────────────────────

/**
 * INSERT operation (`write/types.rs`). `insert_into` is a `TableRef`
 * (bare string for repo "main", or `[repo, table]` tuple). `values` is a
 * non-empty array of records.
 */
export interface InsertOp {
  insert_into: TableRefWire;
  values: WireValue[];
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
 * `where` is `#[serde(rename = "where")]` — **required** (no skip), always
 * present on the wire.
 */
export interface DeleteOp {
  delete_from: TableRefWire;
  where: Filter;
}

/** Union of all write operation wire shapes. */
export type WriteOp = InsertOp | UpdateOp | SetOp | DeleteOp;
