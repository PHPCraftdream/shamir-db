/**
 * Batch wire types — type-only mirror of
 * `crates/shamir-query-types/src/batch/types.rs` and
 * `crates/shamir-query-types/src/read/query_result.rs`.
 *
 * Pure type declarations; the `Batch` fluent builder that constructs a
 * {@link BatchRequest} lives in `../../builders/batch.ts`.
 *
 * Serde notes encoded here (so the builder emits the exact wire shape):
 *   - `QueryEntry.op` uses `#[serde(flatten)]` → the op fields are
 *     spread directly; `return_result` (default `true`) is omitted when
 *     true, emitted as `false` only; `after` (default `[]`) is
 *     skip-if-empty.
 *   - `BatchRequest.transactional` is `#[serde(default = false)]` →
 *     omitted when false.
 *   - `BatchRequest.return_all` is `#[serde(default = true)]` → omitted
 *     when true (default).
 *   - `BatchRequest.name`, `isolation`, `durability`, `return_only`,
 *     `limits` are `skip_serializing_if = "Option::is_none"` → omitted
 *     when unset.
 *   - `BatchRequest.limits` has a Rust-side default; the TS builder
 *     omits the field entirely unless the caller explicitly sets it.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Json } from './write.js';
import type { ReadQuery } from './query.js';
import type { InsertOp, UpdateOp, SetOp, DeleteOp } from './write.js';
import type { DdlOp } from './ddl.js';
import type { AdminOp } from './admin.js';
import type { CallOp } from './call.js';

// ── Batch operation input ───────────────────────────────────────────

/** Union of all wire-operation shapes accepted by a batch entry. */
export type BatchOpInput =
  | ReadQuery
  | InsertOp
  | UpdateOp
  | SetOp
  | DeleteOp
  | DdlOp
  | AdminOp
  | CallOp;

// ── QueryEntry ──────────────────────────────────────────────────────

/**
 * One entry in the `queries` map. `#[serde(flatten)]` on the Rust side
 * means the op fields are spread at the same level as `return_result`
 * and `after`. The builder omits `return_result` when `true` (default)
 * and `after` when empty.
 */
export type QueryEntry = BatchOpInput & {
  return_result?: boolean;
  after?: string[];
};

// ── Isolation / Durability ──────────────────────────────────────────

/** Isolation level for transactional batches. */
export type IsolationLevel = 'snapshot' | 'serializable';

/** Per-request durability level. */
export type DurabilityLevel = 'buffered' | 'synced';

// ── BatchLimits ─────────────────────────────────────────────────────

/** Execution limits (security / DoS prevention). */
export interface BatchLimits {
  max_queries: number;
  max_dependency_depth: number;
  max_execution_time_secs: number;
  max_result_size: number;
}

// ── BatchRequest ────────────────────────────────────────────────────

/**
 * Full batch request envelope. `id` and `queries` are always present.
 * All other fields are omitted at their default / unset state by the
 * builder.
 */
export interface BatchRequest {
  id: Json;
  name?: string;
  transactional?: true;
  isolation?: IsolationLevel;
  durability?: DurabilityLevel;
  queries: Record<string, QueryEntry>;
  return_all?: false;
  return_only?: string[];
  limits?: BatchLimits;
}

// ── Response types ──────────────────────────────────────────────────

/** Query execution statistics. */
export interface QueryStats {
  index_used: string | null;
  records_scanned: number;
  records_returned: number;
  execution_time_us: number;
}

/** Pagination metadata (present when pagination was used). */
export interface PaginationInfo {
  total_count?: number;
  total_pages?: number;
  current_page?: number;
  page_size?: number;
  has_next: boolean;
  has_prev: boolean;
}

/**
 * Query result — every batch entry (read / write / DDL / admin) comes
 * back as a `QueryResult`.
 */
export interface QueryResult {
  records: Json[];
  stats?: QueryStats;
  pagination?: PaginationInfo;
  value?: Json;
}

/** Transaction metadata (present on transactional batches). */
export interface TransactionInfo {
  tx_id: number;
  status: 'committed' | 'aborted';
  reason?: string;
  snapshot_version?: number;
  commit_version?: number;
  materialized: boolean;
}

/** Batch response envelope. */
export interface BatchResponse {
  id: Json;
  results: Record<string, QueryResult>;
  execution_plan: string[][];
  execution_time_us: number;
  transaction?: TransactionInfo;
}
