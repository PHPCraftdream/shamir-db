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

import type { WireValue } from './write.js';
import type { ReadQuery, ExplainPlan } from './query.js';
import type { InsertOp, UpdateOp, SetOp, DeleteOp } from './write.js';
import type { DdlOp } from './ddl.js';
import type { AdminOp } from './admin.js';
import type { CallOp } from './call.js';
import type { FilterValue } from './filter.js';
import type { SubscribeOp, UnsubscribeOp } from './subscribe.js';

// ── Sub-batch operation ─────────────────────────────────────────────

/**
 * A nested batch operation (`{ "batch": <BatchRequest>, "bind": { … } }`).
 * Mirrors the server's `SubBatchOp` variant accepted inside a batch entry.
 * `bind` maps parameter names to `FilterValue`s that the inner batch can
 * reference via `{ "$param": "name" }`.  Omitted when empty.
 */
export interface SubBatchOp {
  batch: BatchRequest;
  bind?: Record<string, FilterValue>;
}

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
  | CallOp
  | SubBatchOp
  | SubscribeOp
  | UnsubscribeOp;

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

/**
 * Per-request durability level (`batch_request.rs::durability`).
 *
 * - `'buffered'` (default) — ack after the in-memory MemBuffer.
 * - `'synced'` — flush durable backing of every touched repo before ack.
 * - `'async_index'` — ack after WAL fsync + data apply + MVCC publish; index
 *   posting apply / recovery markers / WAL cleanup / HNSW promote run on a
 *   background task. Only meaningful for `transactional: true` batches.
 */
export type DurabilityLevel = 'buffered' | 'synced' | 'async_index';

// ── BatchLimits ─────────────────────────────────────────────────────

/** Execution limits (security / DoS prevention). */
export interface BatchLimits {
  max_queries: number;
  max_dependency_depth: number;
  max_execution_time_secs: number;
  max_result_size: number;
  /** Maximum sub-batch nesting depth. 0 = no nesting allowed. */
  max_nesting_depth: number;
}

// ── BatchRequest ────────────────────────────────────────────────────

/**
 * Full batch request envelope. `id` and `queries` are always present.
 * All other fields are omitted at their default / unset state by the
 * builder.
 */
export interface BatchRequest {
  id: WireValue;
  name?: string;
  transactional?: true;
  isolation?: IsolationLevel;
  durability?: DurabilityLevel;
  queries: Record<string, QueryEntry>;
  return_all?: false;
  return_only?: string[];
  limits?: BatchLimits;
  /**
   * Per-repo interner epochs the client has cached (Stage 5-wire, Part A).
   *
   * Keyed by repo name; value is the client's current gap-free high-water
   * epoch for that repo's interner. The server attaches a per-repo
   * `WireInternerDelta` to the response for every entry here.
   *
   * Omitted when the registry has no warm entries (epoch = 0 for all repos).
   * Backward-compatible: absent → server sends no delta.
   */
  interner_epochs?: Record<string, number | bigint>;

  /**
   * Desired result encoding. `"id"` requests id-keyed result rows from the
   * server (v2+); the client de-interns them transparently.
   */
  result_encoding?: 'id' | 'name';
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
 *
 * `records` are msgpack-decoded objects (field → value); each field value
 * is a `WireValue`. A scalar/array answer from a stored function lands in
 * `value`, not here.
 */
export interface QueryResult {
  records: Array<Record<string, WireValue>>;
  stats?: QueryStats;
  pagination?: PaginationInfo;
  value?: WireValue;
  /**
   * EXPLAIN plan preview — present only when the source `ReadQuery` set
   * `explain: true`. Mirrors `query_result.rs::QueryResult.explain`
   * (`skip_serializing_if = "Option::is_none"`).
   */
  explain?: ExplainPlan;
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

/**
 * Per-repo interner delta returned by the server in a `BatchResponse`
 * (Stage 5-wire, Part A).
 *
 * Mirrors `shamir-query-types::batch::interner_delta::InternerDelta`.
 * `epoch` and `entries[*][0]` are u64 on the wire; @msgpack/msgpack
 * decodes them as `number` (safe range) or `bigint` (> MAX_SAFE_INTEGER).
 * Consumers normalise to `bigint` via the FieldMap helpers.
 */
export interface WireInternerDelta {
  epoch: number | bigint;
  /** `[id, name]` pairs — id-first, matching `interner_dump` shape. */
  entries: [number | bigint, string][];
}

/** Batch response envelope. */
export interface BatchResponse {
  id: WireValue;
  results: Record<string, QueryResult>;
  execution_plan: string[][];
  execution_time_us: number;
  transaction?: TransactionInfo;
  /**
   * Per-repo interner deltas for ambient cache sync (Stage 5-wire, Part A).
   * Present only when the client advertised `interner_epochs` in the request.
   * Keyed by repo name.
   */
  interner_delta?: Record<string, WireInternerDelta>;
}
