/**
 * Wire type model — the single home for ShamirDB's platform-agnostic DTO
 * types. This barrel re-exports only; every type lives in a sibling file
 * (filter.ts, query.ts, connection.ts). Constructor/builder CODE lives
 * under `../builders/`, kept apart from these declarations.
 *
 * PLATFORM-AGNOSTIC.
 */

export type {
  FieldPath,
  FilterValue,
  FnCall,
  Filter,
  ComputedFilter,
} from './filter.js';

export type {
  TableRefWire,
  AggFunc,
  AggregateField,
  SelectItem,
  Select,
  GroupBy,
  OrderDirection,
  NullsOrder,
  OrderByItem,
  OrderBy,
  Pagination,
  At,
  Temporal,
  ReadQuery,
} from './query.js';

export type { ConnectOptions } from './connection.js';
