/**
 * Read-query wire types — type-only mirror of
 * `crates/shamir-query-types/src/read/`.
 *
 * Pure type declarations; the `Query` fluent builder that constructs a
 * {@link ReadQuery} lives in `../../builders/query.ts`.
 *
 * Serde notes encoded here (so the builder emits the exact wire shape):
 *   - fields with `skip_serializing_if` are OPTIONAL (`?`) here — the
 *     builder omits them at their default;
 *   - fields with only `#[serde(default)]` (no skip) are ALWAYS present on
 *     the wire (e.g. `Select.distinct`, `OrderByItem.direction`).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { FieldPath, FilterValue, Filter } from './filter.js';
import type { WireValue } from './write.js';

// ── TableRef ─────────────────────────────────────────────────────────

/**
 * `from` target. A bare string means repo "main"; a `[repo, table]` tuple
 * names an explicit repo (`table_ref.rs` custom serde).
 */
export type TableRefWire = string | [string, string];

// ── Aggregation ──────────────────────────────────────────────────────

/** Fast-path aggregate function (`agg.rs`, `rename_all = "lowercase"`). */
export type AggFunc = 'count' | 'sum' | 'avg' | 'min' | 'max';

/**
 * Aggregation target (`AggregateField`, `#[serde(untagged)]`): a field path,
 * or `null` for `*` (the `All` unit variant serialises as null).
 */
export type AggregateField = FieldPath | null;

// ── Select ───────────────────────────────────────────────────────────

/**
 * One projection item (`SelectItem`, `#[serde(tag = "type",
 * rename_all = "snake_case")]`). `distinct` on Aggregate/AggregateFn and
 * `args` on Function are `#[serde(default)]` WITHOUT skip → always emitted.
 */
export type SelectItem =
  | { type: 'all' }
  | { type: 'field'; path: FieldPath; alias?: string }
  | {
      type: 'aggregate';
      func: AggFunc;
      field: AggregateField;
      alias?: string;
      distinct: boolean;
    }
  | { type: 'count_all'; alias?: string }
  | {
      type: 'aggregate_fn';
      name: string;
      field: AggregateField;
      args: FilterValue[];
      alias?: string;
      distinct: boolean;
    }
  | { type: 'function'; name: string; args: FilterValue[]; alias?: string }
  /**
   * Computed SELECT expression. Accepted by this type and the wire/parser,
   * but F-26 (#819): the server currently REJECTS any query whose `select`
   * contains this variant with a typed `select_expression_not_supported`
   * error at execution time (no evaluator implemented yet) — it is never
   * silently dropped from the projected result. Kept in the type for a
   * future real implementation; a client constructing one today gets a
   * clear rejection, not missing data.
   */
  | { type: 'expr'; expr: unknown; alias?: string };

/** Projection set (`Select`). `distinct` is always present on the wire. */
export interface Select {
  items: SelectItem[];
  distinct: boolean;
}

// ── Group by ─────────────────────────────────────────────────────────

/** GROUP BY clause (`group_by.rs`). `fields` is an array of field paths. */
export interface GroupBy {
  fields: FieldPath[];
  having?: Filter;
}

// ── Order by ─────────────────────────────────────────────────────────

/** Sort direction (`OrderDirection`, lowercase). Default: "asc". */
export type OrderDirection = 'asc' | 'desc';

/** NULL ordering (`NullsOrder`, lowercase). */
export type NullsOrder = 'first' | 'last';

/**
 * One ORDER BY term. `direction` is `#[serde(default)]` without skip → it is
 * always emitted (matches the e2e wire shape `{field, direction}`).
 */
export interface OrderByItem {
  field: FieldPath;
  direction: OrderDirection;
  nulls?: NullsOrder;
}

/** ORDER BY clause (`order_by.rs`). */
export interface OrderBy {
  items: OrderByItem[];
}

// ── Pagination ───────────────────────────────────────────────────────

/**
 * Pagination (`Pagination`, `#[serde(tag = "mode")]` — note: NO
 * rename_all, so the discriminant keeps PascalCase variant names). The
 * `None` variant is skip-serialized, so it never appears on the wire.
 * `offset` is `#[serde(default)]` without skip → always present.
 * `After` is keyset/seek pagination: `key` is always present (an array
 * of {@link WireValue}); `limit` is skip-if-none → omitted when unset.
 * `after_id` (task #537) is an optional record-id tie-breaker — the base58
 * `_id` string of the last row the client received on the previous page.
 * When present, the server resumes STRICTLY past that specific row, so rows
 * tied on the same ORDER BY value across a page boundary are not silently
 * dropped. Omitted (the default) → today's backward-compatible behavior.
 */
export type Pagination =
  | { mode: 'LimitOffset'; limit?: number; offset: number }
  | { mode: 'Page'; page: number; page_size: number }
  | { mode: 'After'; key: WireValue[]; limit?: number; after_id?: string };

// ── Temporal ─────────────────────────────────────────────────────────

/**
 * A point in time (`At`, `rename_all = "snake_case"`, externally tagged):
 * `{ version }` is exact and cheap; `{ timestamp }` (epoch-millis) is
 * resolved to a version by the engine.
 */
export type At = { version: number } | { timestamp: number };

/**
 * Temporal selector (`Temporal`, `#[serde(tag = "kind",
 * rename_all = "snake_case")]`). `Latest` is skip-serialized — the builder
 * omits the whole `temporal` field for a present-time read. In `history`,
 * `order` is `#[serde(default)]` without skip → always present.
 */
export type Temporal =
  | { kind: 'as_of'; at: At }
  | {
      kind: 'history';
      from?: At;
      to?: At;
      limit?: number;
      order: OrderDirection;
    };

// ── ReadQuery ────────────────────────────────────────────────────────

/**
 * A complete read query (`read_query.rs`). Optional (`?`) fields are
 * skip-serialized at their Rust default and are omitted by the builder.
 * `select` defaults to `Select::all()` server-side, so the builder omits it
 * for a plain `SELECT *` (matching the e2e wire shape).
 */
export interface ReadQuery {
  from: TableRefWire;
  select?: Select;
  where?: Filter;
  group_by?: GroupBy;
  order_by?: OrderBy;
  pagination?: Pagination;
  count_total?: boolean;
  temporal?: Temporal;
  with_version?: boolean;
  /**
   * EXPLAIN / dry-run: run only the planner (index selection, plan type)
   * and return a plan preview WITHOUT materialising any rows. Mirrors
   * `read_query.rs::explain` (`#[serde(default, skip_serializing_if = is_false)]`
   * → omitted at its `false` default; emitted only when `true`). The result
   * lands in {@link QueryResult.explain}.
   */
  explain?: boolean;
}

// ── EXPLAIN plan preview ─────────────────────────────────────────────

/**
 * Plan type chosen by the read planner (`query_result.rs::PlanType`,
 * externally tagged — the Rust enum has NO `rename_all`, so the wire
 * discriminant keeps the PascalCase variant names verbatim).
 */
export type PlanType =
  | 'KeysetSeek'
  | 'OrderLimitFast'
  | 'Index2'
  | 'IndexScan'
  | 'SortedIndexScan'
  | 'AndRangeIndexScan'
  | 'CounterShortcut'
  | 'MinMaxIndex'
  | 'FullScan';

/**
 * EXPLAIN plan preview — present on {@link QueryResult.explain} only when the
 * query set `explain: true`. Mirrors `query_result.rs::ExplainPlan`;
 * `index_used` / `estimated_rows` are skip-if-none.
 */
export interface ExplainPlan {
  plan_type: PlanType;
  index_used?: string;
  estimated_rows?: number;
}
