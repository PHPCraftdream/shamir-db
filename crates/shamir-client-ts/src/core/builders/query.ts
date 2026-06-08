/**
 * `Query` — the OQL read fluent builder. The CODE that assembles a
 * {@link ReadQuery} wire object (declared in `../types/query.ts`). Mirrors
 * `crates/shamir-query-builder/src/query/` + `read_query.rs`.
 *
 * `.build()` returns a plain object ready to drop into a batch's `queries`
 * map. Defaults are omitted to match the server's serde defaults and the
 * shapes used by the existing e2e suite (e.g. a plain `SELECT *` omits
 * `select` entirely).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Filter, FieldPath } from '../types/filter.js';
import type {
  TableRefWire,
  Select,
  SelectItem,
  GroupBy,
  OrderBy,
  OrderByItem,
  OrderDirection,
  NullsOrder,
  Pagination,
  At,
  Temporal,
  ReadQuery,
} from '../types/query.js';
import { and } from './filter.js';
import { field as selectField } from './select.js';

/** Normalise a field spec (bare string or path array) to the wire form. */
function fp(field: string | string[]): FieldPath {
  return typeof field === 'string' ? [field] : field;
}

/** A point-in-time at an exact version. */
export function atVersion(version: number): At {
  return { version };
}

/** A point-in-time at an epoch-millis timestamp (resolved engine-side). */
export function atTimestamp(timestamp: number): At {
  return { timestamp };
}

type PaginationMode = 'none' | 'limitoffset' | 'page';

/** Fluent builder for an OQL `ReadQuery`. */
export class Query {
  private readonly from: TableRefWire;

  private selectItems: SelectItem[] | null = null;
  private selectDistinct = false;
  private whereFilter: Filter | null = null;
  private groupFields: FieldPath[] | null = null;
  private havingFilter: Filter | null = null;
  private orderItems: OrderByItem[] = [];

  private paginationMode: PaginationMode = 'none';
  private limitValue: number | null = null;
  private offsetValue = 0;
  private pageNumber = 1;
  private pageSize = 0;

  private countTotalFlag = false;
  private temporalValue: Temporal | null = null;
  private withVersionFlag = false;

  private constructor(from: TableRefWire) {
    this.from = from;
  }

  /** Query a table in the default repo ("main"). */
  static from(table: string): Query {
    return new Query(table);
  }

  /** Query a table in an explicit repo. */
  static withRepo(repo: string, table: string): Query {
    return new Query(repo === 'main' ? table : [repo, table]);
  }

  // ── Projection ─────────────────────────────────────────────────────

  /**
   * Set the projection. Accepts a list of field names/paths (shorthand for
   * `field` items) or explicit {@link SelectItem}s (aggregates, functions).
   */
  select(items: Array<string | string[]> | SelectItem[]): this {
    this.selectItems = items.map((it) =>
      typeof it === 'string' || Array.isArray(it) ? selectField(it) : it,
    );
    return this;
  }

  /** Explicit `SELECT *`. */
  selectAll(): this {
    this.selectItems = [{ type: 'all' }];
    return this;
  }

  /** Return distinct results. */
  distinct(on = true): this {
    this.selectDistinct = on;
    return this;
  }

  // ── WHERE ──────────────────────────────────────────────────────────

  /** Set the WHERE filter (replaces any previous one). */
  where(filter: Filter): this {
    this.whereFilter = filter;
    return this;
  }

  /** AND the given filter into the existing WHERE (smart-flattened). */
  andWhere(filter: Filter): this {
    this.whereFilter =
      this.whereFilter === null ? filter : and(this.whereFilter, filter);
    return this;
  }

  // ── GROUP BY / HAVING ──────────────────────────────────────────────

  /** GROUP BY the given fields (names or paths). */
  groupBy(...fields: Array<string | string[]>): this {
    this.groupFields = fields.map(fp);
    return this;
  }

  /** HAVING filter, applied after grouping. Requires `groupBy`. */
  having(filter: Filter): this {
    this.havingFilter = filter;
    return this;
  }

  // ── ORDER BY ───────────────────────────────────────────────────────

  /** Append one or more explicit ORDER BY items. */
  orderBy(items: OrderByItem | OrderByItem[]): this {
    if (Array.isArray(items)) this.orderItems.push(...items);
    else this.orderItems.push(items);
    return this;
  }

  /** Append an ascending sort on `field`. */
  orderByAsc(field: string | string[], nulls?: NullsOrder): this {
    return this.pushOrder(field, 'asc', nulls);
  }

  /** Append a descending sort on `field`. */
  orderByDesc(field: string | string[], nulls?: NullsOrder): this {
    return this.pushOrder(field, 'desc', nulls);
  }

  private pushOrder(
    field: string | string[],
    direction: OrderDirection,
    nulls?: NullsOrder,
  ): this {
    const item: OrderByItem = { field: fp(field), direction };
    if (nulls !== undefined) item.nulls = nulls;
    this.orderItems.push(item);
    return this;
  }

  // ── Pagination ─────────────────────────────────────────────────────

  /** LIMIT n (switches to limit/offset pagination). */
  limit(n: number): this {
    this.paginationMode = 'limitoffset';
    this.limitValue = n;
    return this;
  }

  /** OFFSET n (switches to limit/offset pagination). */
  offset(n: number): this {
    this.paginationMode = 'limitoffset';
    this.offsetValue = n;
    return this;
  }

  /** Page-based pagination: 1-based `page` of `pageSize` records. */
  page(page: number, pageSize: number): this {
    this.paginationMode = 'page';
    this.pageNumber = page;
    this.pageSize = pageSize;
    return this;
  }

  /** Compute and return the total matching count (expensive). */
  countTotal(on = true): this {
    this.countTotalFlag = on;
    return this;
  }

  // ── Temporal ───────────────────────────────────────────────────────

  /** Read as of an exact version. */
  asOfVersion(version: number): this {
    this.temporalValue = { kind: 'as_of', at: atVersion(version) };
    return this;
  }

  /** Read as of an epoch-millis timestamp (resolved engine-side). */
  asOfTimestamp(timestamp: number): this {
    this.temporalValue = { kind: 'as_of', at: atTimestamp(timestamp) };
    return this;
  }

  /** Read as of an explicit {@link At} point. */
  asOf(at: At): this {
    this.temporalValue = { kind: 'as_of', at };
    return this;
  }

  /**
   * Range read over history. `from`/`to` bound the window (either may be
   * omitted for an open bound); `limit` caps the version count; `order`
   * defaults to "asc" (oldest → newest).
   */
  history(
    opts: { from?: At; to?: At; limit?: number; order?: OrderDirection } = {},
  ): this {
    const t: Extract<Temporal, { kind: 'history' }> = {
      kind: 'history',
      order: opts.order ?? 'asc',
    };
    if (opts.from !== undefined) t.from = opts.from;
    if (opts.to !== undefined) t.to = opts.to;
    if (opts.limit !== undefined) t.limit = opts.limit;
    this.temporalValue = t;
    return this;
  }

  /** Include each record's version in the result (for as_of cursors / CAS). */
  withVersion(on = true): this {
    this.withVersionFlag = on;
    return this;
  }

  // ── Build ──────────────────────────────────────────────────────────

  /** Assemble the wire {@link ReadQuery}. */
  build(): ReadQuery {
    if (this.havingFilter !== null && this.groupFields === null) {
      throw new Error('having() requires groupBy()');
    }

    const q: ReadQuery = { from: this.from };

    // select: omit a plain `SELECT *` (server defaults to all()).
    if (this.selectItems !== null || this.selectDistinct) {
      const select: Select = {
        items: this.selectItems ?? [{ type: 'all' }],
        distinct: this.selectDistinct,
      };
      q.select = select;
    }

    if (this.whereFilter !== null) q.where = this.whereFilter;

    if (this.groupFields !== null) {
      const group: GroupBy = { fields: this.groupFields };
      if (this.havingFilter !== null) group.having = this.havingFilter;
      q.group_by = group;
    }

    if (this.orderItems.length > 0) {
      const order: OrderBy = { items: this.orderItems };
      q.order_by = order;
    }

    const pagination = this.buildPagination();
    if (pagination !== null) q.pagination = pagination;

    if (this.countTotalFlag) q.count_total = true;
    if (this.temporalValue !== null) q.temporal = this.temporalValue;
    if (this.withVersionFlag) q.with_version = true;

    return q;
  }

  private buildPagination(): Pagination | null {
    switch (this.paginationMode) {
      case 'page':
        return {
          mode: 'Page',
          page: this.pageNumber,
          page_size: this.pageSize,
        };
      case 'limitoffset': {
        const p: Pagination = { mode: 'LimitOffset', offset: this.offsetValue };
        if (this.limitValue !== null) p.limit = this.limitValue;
        return p;
      }
      default:
        return null;
    }
  }
}
