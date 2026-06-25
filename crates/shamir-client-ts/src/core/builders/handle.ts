/**
 * `Handle` / `RowRef` — typed accessors for `$query` result references inside a
 * batch. Mirrors `crates/shamir-query-builder/src/batch/handle.rs`.
 *
 * These classes build `$query` `FilterValue`s through the existing
 * {@link queryRef} constructor — they do NOT duplicate the wire-path format.
 *
 * `Batch.handle(alias)` returns a `Handle`; chaining on `Batch.add()` is
 * unaffected (it still returns `this`).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { FilterValue } from '../types/filter.js';
import { queryRef } from './filter.js';

/** Normalise a field spec to the dotted-path string used after `[]`. */
function dotted(field: string | string[]): string {
  return typeof field === 'string' ? field : field.join('.');
}

/**
 * Typed handle to a batch query result identified by `alias`.
 *
 * - `.column(field)` → `$query` path `"[].field"` (all rows, one column);
 *   nested paths produce `"[].a.b"`.
 * - `.row(i)` → a {@link RowRef} for row `i`.
 * - `.first()` → shorthand for `.row(0)`.
 * - `.all()` → a bare `$query` ref with no path (the whole result set).
 */
export class Handle {
  constructor(private readonly alias: string) {}

  /** Reference every row's `field` value → `{ $query, path: "[].field" }`. */
  column(field: string | string[]): FilterValue {
    return queryRef(this.alias, `[].${dotted(field)}`);
  }

  /** Reference row `index` → a {@link RowRef}. */
  row(index: number): RowRef {
    return new RowRef(this.alias, index);
  }

  /** Shorthand for `.row(0)`. */
  first(): RowRef {
    return this.row(0);
  }

  /** Reference the whole result set (no path) → `{ $query }`. */
  all(): FilterValue {
    return queryRef(this.alias);
  }
}

/**
 * Typed reference to a single row `index` of a batch query result.
 *
 * - `.field(f)` → `{ $query, path: "[i].field" }`;
 * - `.get()` → `{ $query, path: "[i]" }` (the whole row object).
 */
export class RowRef {
  constructor(
    private readonly alias: string,
    private readonly index: number,
  ) {}

  /** Reference `field` within this row → `{ $query, path: "[i].field" }`. */
  field(field: string | string[]): FilterValue {
    return queryRef(this.alias, `[${this.index}].${dotted(field)}`);
  }

  /** Reference the whole row object → `{ $query, path: "[i]" }`. */
  get(): FilterValue {
    return queryRef(this.alias, `[${this.index}]`);
  }
}
