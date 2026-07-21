/**
 * Write-operation builders — the CODE that constructs the wire shapes
 * declared in `../types/write.ts`. Mirrors
 * `crates/shamir-query-types/src/write/types.rs`.
 *
 * Provides ergonomic constructors:
 *   - `insert(table, values, opts?)` → InsertOp
 *   - `update(table, opts?)` → UpdateBuilder (fluent `.where()`, `.set()`,
 *     `.returning()`, `.build()`)
 *   - `upsert(table, key, value, opts?)` → SetOp
 *   - `del(table, where, opts?)` → DeleteOp  (exported as `del` since
 *     `delete` is a reserved word)
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Filter } from '../types/filter.js';
import type {
  TableRefWire,
  WireValue,
  WriteValue,
  UpdateReturnMode,
  UpdateSelect,
  DeleteSelect,
  InsertSelect,
  InsertOp,
  UpdateOp,
  SetOp,
  DeleteOp,
} from '../types/write.js';

/** Build a TableRef wire value: bare string for default repo, else tuple. */
function tableRef(repo: string | undefined, table: string): TableRefWire {
  return !repo || repo === 'main' ? table : [repo, table];
}

// ── insert ───────────────────────────────────────────────────────────

/**
 * Build an `InsertOp`. Accepts a single record or an array; normalises to
 * an array internally.
 *
 * Each record is a `WriteValue` — plain literals OR computed expressions
 * (`filter.fn('NOW')`, `filter.ref('price')`, …) produced by the `filter.*`
 * constructors. This mirrors Rust `write::Doc::set(key, impl Into<FilterValue>)`.
 *
 * `opts.returningFields`, when provided, emits an `InsertSelect` projection
 * on the wire so each returned row carries only the named fields. Mirror of
 * `UpdateBuilder.returning(..., fields)` / `DeleteBuilder.returning(fields)`.
 */
export function insert(
  table: string,
  values: WriteValue | WriteValue[],
  opts?: { repo?: string; returningFields?: string[] },
): InsertOp {
  const rows = (Array.isArray(values) ? values : [values]) as WireValue[];
  const op: InsertOp = { insert_into: tableRef(opts?.repo, table), values: rows };
  if (opts?.returningFields !== undefined) {
    op.select = { fields: opts.returningFields };
  }
  return op;
}

// ── update (fluent builder) ──────────────────────────────────────────

/** Fluent builder for an `UpdateOp`. */
export class UpdateBuilder {
  private readonly tableRef: TableRefWire;
  private whereFilter: Filter | null = null;
  private setValue: WireValue | null = null;
  private selectValue: UpdateSelect | null = null;
  private expectedVersionValue: number | null = null;

  /** @internal Use `update()` to create an instance. */
  static create(tableRef: TableRefWire): UpdateBuilder {
    return new UpdateBuilder(tableRef);
  }

  private constructor(tableRef: TableRefWire) {
    this.tableRef = tableRef;
  }

  /** Set the WHERE filter (replaces any previous one). */
  where(filter: Filter): this {
    this.whereFilter = filter;
    return this;
  }

  /**
   * Set the fields to update. The record may mix plain literals and computed
   * expressions (`filter.fn('NOW')`, `filter.ref('subtotal')`, …) — mirrors
   * Rust `Doc::set`. Required before `.build()`.
   */
  set(obj: WriteValue): this {
    this.setValue = obj as WireValue;
    return this;
  }

  /**
   * Configure returning mode. When called, `select` is emitted on the wire
   * with `return_mode` always present (serde default = "changed").
   * `fields` is omitted unless provided.
   */
  returning(mode?: UpdateReturnMode, fields?: string[]): this {
    const select: UpdateSelect = {
      return_mode: mode ?? 'changed',
    };
    if (fields !== undefined) select.fields = fields;
    this.selectValue = select;
    return this;
  }

  /**
   * Set the optimistic-concurrency (CAS) version guard (FG-2). When set, the
   * server rejects the update with `version_conflict` unless every matched row
   * is currently at exactly this version. The version comes from
   * `QueryResult.versions` (read-side `.withVersion()`).
   */
  expectedVersion(version: number): this {
    this.expectedVersionValue = version;
    return this;
  }

  /** Assemble the wire `UpdateOp`. Throws if `.set()` was never called. */
  build(): UpdateOp {
    if (this.setValue === null) {
      throw new Error('update builder requires .set() before .build()');
    }
    const op: UpdateOp = {
      update: this.tableRef,
      set: this.setValue,
    };
    if (this.whereFilter !== null) op.where = this.whereFilter;
    if (this.selectValue !== null) op.select = this.selectValue;
    if (this.expectedVersionValue !== null)
      op.expected_version = this.expectedVersionValue;
    return op;
  }
}

/**
 * Start building an UPDATE operation. Returns a fluent `UpdateBuilder`.
 * Call `.where()`, `.set()`, `.returning()`, then `.build()`.
 */
export function update(
  table: string,
  opts?: { repo?: string },
): UpdateBuilder {
  return UpdateBuilder.create(tableRef(opts?.repo, table));
}

// ── upsert (set) ─────────────────────────────────────────────────────

/**
 * Build a `SetOp` (upsert by key). Both `key` and `value` accept `WriteValue` —
 * plain literals OR computed expressions (`filter.fn(...)`, `filter.queryRef(...)`,
 * …) — mirroring Rust `Doc::set`.
 */
export function upsert(
  table: string,
  key: WriteValue,
  value: WriteValue,
  opts?: { repo?: string },
): SetOp {
  return { set: tableRef(opts?.repo, table), key: key as WireValue, value: value as WireValue };
}

// ── delete ───────────────────────────────────────────────────────────

/**
 * Build a `DeleteOp`. Exported as `del` since `delete` is a reserved word
 * in JavaScript. `where` is required (no skip on the wire).
 *
 * `opts.returning` (boolean) or `opts.returningFields` (string[]) opts in
 * to RETURNING — emitted as a `DeleteSelect` on the wire. When
 * `returningFields` is given, each returned row is restricted to the named
 * fields; `returning: true` alone returns all fields. Mirror of
 * `UpdateBuilder.returning(mode, fields)`.
 */
export function del(
  table: string,
  where: Filter,
  opts?: {
    repo?: string;
    returning?: boolean;
    returningFields?: string[];
    expectedVersion?: number;
  },
): DeleteOp {
  const op: DeleteOp = { delete_from: tableRef(opts?.repo, table), where };
  if (opts?.returningFields !== undefined) {
    op.select = { fields: opts.returningFields };
  } else if (opts?.returning) {
    op.select = {};
  }
  if (opts?.expectedVersion !== undefined)
    op.expected_version = opts.expectedVersion;
  return op;
}

/** Aggregate namespace — every write constructor in one object. */
export const write = { insert, update, upsert, del };
