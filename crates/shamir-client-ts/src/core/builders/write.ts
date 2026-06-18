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
  UpdateReturnMode,
  UpdateSelect,
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
 */
export function insert(
  table: string,
  values: WireValue | WireValue[],
  opts?: { repo?: string },
): InsertOp {
  const rows = Array.isArray(values) ? values : [values];
  return { insert_into: tableRef(opts?.repo, table), values: rows };
}

// ── update (fluent builder) ──────────────────────────────────────────

/** Fluent builder for an `UpdateOp`. */
export class UpdateBuilder {
  private readonly tableRef: TableRefWire;
  private whereFilter: Filter | null = null;
  private setValue: WireValue | null = null;
  private selectValue: UpdateSelect | null = null;

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

  /** Set the fields to update (partial record). Required before `.build()`. */
  set(obj: WireValue): this {
    this.setValue = obj;
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

/** Build a `SetOp` (upsert by key). */
export function upsert(
  table: string,
  key: WireValue,
  value: WireValue,
  opts?: { repo?: string },
): SetOp {
  return { set: tableRef(opts?.repo, table), key, value };
}

// ── delete ───────────────────────────────────────────────────────────

/**
 * Build a `DeleteOp`. Exported as `del` since `delete` is a reserved word
 * in JavaScript. `where` is required (no skip on the wire).
 */
export function del(
  table: string,
  where: Filter,
  opts?: { repo?: string },
): DeleteOp {
  return { delete_from: tableRef(opts?.repo, table), where };
}

/** Aggregate namespace — every write constructor in one object. */
export const write = { insert, update, upsert, del };
