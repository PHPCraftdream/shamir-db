/**
 * Select-item constructors — the CODE that builds the `SelectItem` wire
 * shapes declared in `../types/query.ts`. Mirrors the `Select` / `SelectItem`
 * surface of `crates/shamir-query-builder/src/select/`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  FieldPath,
  FilterValue,
} from '../types/filter.js';
import type {
  AggFunc,
  AggregateField,
  SelectItem,
} from '../types/query.js';

/** Normalise a field spec (bare string or path array) to the wire form. */
function fp(field: string | string[]): FieldPath {
  return typeof field === 'string' ? [field] : field;
}

/** Normalise an aggregate target: a field path, or `null` (`*`). */
function aggField(field: string | string[] | null): AggregateField {
  return field === null ? null : fp(field);
}

/** `SELECT *` — every field of the record. */
export function all(): SelectItem {
  return { type: 'all' };
}

/** Project a single field, with an optional alias. */
export function field(spec: string | string[], alias?: string): SelectItem {
  const item: SelectItem = { type: 'field', path: fp(spec) };
  if (alias !== undefined) item.alias = alias;
  return item;
}

/** `COUNT(*)` over the whole group/result. */
export function countAll(alias?: string): SelectItem {
  const item: SelectItem = { type: 'count_all' };
  if (alias !== undefined) item.alias = alias;
  return item;
}

/**
 * A fast-path aggregate (`count`/`sum`/`avg`/`min`/`max`). `field` is a path,
 * or `null` for `*`. `distinct` is always emitted on the wire.
 */
export function aggregate(
  func: AggFunc,
  field: string | string[] | null,
  opts: { alias?: string; distinct?: boolean } = {},
): SelectItem {
  const item: SelectItem = {
    type: 'aggregate',
    func,
    field: aggField(field),
    distinct: opts.distinct ?? false,
  };
  if (opts.alias !== undefined) item.alias = opts.alias;
  return item;
}

/** `COUNT(field)` (fast-path). */
export function count(
  field: string | string[] | null = null,
  opts: { alias?: string; distinct?: boolean } = {},
): SelectItem {
  return aggregate('count', field, opts);
}

/** `SUM(field)`. */
export function sum(
  field: string | string[],
  opts: { alias?: string; distinct?: boolean } = {},
): SelectItem {
  return aggregate('sum', field, opts);
}

/** `AVG(field)`. */
export function avg(
  field: string | string[],
  opts: { alias?: string; distinct?: boolean } = {},
): SelectItem {
  return aggregate('avg', field, opts);
}

/** `MIN(field)`. */
export function min(
  field: string | string[],
  opts: { alias?: string; distinct?: boolean } = {},
): SelectItem {
  return aggregate('min', field, opts);
}

/** `MAX(field)`. */
export function max(
  field: string | string[],
  opts: { alias?: string; distinct?: boolean } = {},
): SelectItem {
  return aggregate('max', field, opts);
}

/**
 * A library aggregate resolved by name through the funclib aggregate
 * registry (`median`, `mode`, `stddev`, `percentile`, `count_distinct`, …).
 * `distinct` is always emitted on the wire. `args` carries static literal
 * parameters for parameterised aggregates (e.g. `0.9` for `percentile`,
 * `";"` for `string_agg`) and is always emitted (defaults to `[]`).
 */
export function aggregateFn(
  name: string,
  field: string | string[] | null,
  opts: { alias?: string; distinct?: boolean; args?: FilterValue[] } = {},
): SelectItem {
  const item: SelectItem = {
    type: 'aggregate_fn',
    name,
    field: aggField(field),
    args: opts.args ?? [],
    distinct: opts.distinct ?? false,
  };
  if (opts.alias !== undefined) item.alias = opts.alias;
  return item;
}

/**
 * A scalar (row-level) function call in the projection, dispatched by name
 * through the funclib scalar registry (`strings/upper`, `math/abs`, …).
 * `args` is always emitted on the wire (defaults to `[]`).
 */
export function func(
  name: string,
  args: FilterValue[] = [],
  alias?: string,
): SelectItem {
  const item: SelectItem = { type: 'function', name, args };
  if (alias !== undefined) item.alias = alias;
  return item;
}

/** Aggregate namespace — every select constructor in one object. */
export const select = {
  all,
  field,
  countAll,
  aggregate,
  count,
  sum,
  avg,
  min,
  max,
  aggregateFn,
  func,
};
