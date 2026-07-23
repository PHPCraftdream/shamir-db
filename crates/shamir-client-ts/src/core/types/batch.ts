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
import type { FilterValue, Filter } from './filter.js';
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

/**
 * A data-dependent for-each loop (Epic04, `{ "over": ..., "bind_row": ...,
 * "for_each": <BatchRequest> }`). Mirrors the server's `ForEachOp`.
 * `over` resolves to a list EXACTLY ONCE before the loop starts (it may be a
 * `$query` ref, an `$fn` call, or a literal array); the body is executed
 * once per element with the element bound to the parameter named
 * `bind_row`. The inner `BatchRequest` field is wire-keyed `for_each` (not
 * `batch`) to avoid colliding with `SubBatchOp`'s wire key.
 */
export interface ForEachOp {
  over: FilterValue;
  bind_row: string;
  for_each: BatchRequest;
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
  | ForEachOp
  | SubscribeOp
  | UnsubscribeOp;

// ── QueryEntry ──────────────────────────────────────────────────────

/**
 * One entry in the `queries` map. `#[serde(flatten)]` on the Rust side
 * means the op fields are spread at the same level as `return_result`,
 * `after`, and `when`. The builder omits `return_result` when `true`
 * (default), `after` when empty, and `when` when unset.
 *
 * `when` (Epic03/B, `#645`) is a conditional-execution guard: the op
 * executes iff `when` is absent, or present and evaluates to `true` via the
 * same `Filter` evaluation machinery WHERE clauses use. See
 * `docs/dev-artifacts/design/oql-03-conditional-execution-adr.md`.
 */
export type QueryEntry = BatchOpInput & {
  return_result?: boolean;
  after?: string[];
  when?: Filter;
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
  /**
   * Maximum `for_each` loop iterations (Epic04, #653). Rust-side
   * `#[serde(default = "default_max_iterations")]` (#662) means a `limits`
   * map omitting this field still deserializes, defaulting to `1000` — but
   * the TS builder always fills it explicitly (see `DEFAULT_LIMITS`).
   */
  max_iterations: number;
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
  /**
   * Conditional-execution status (Epic03/B, #645): `true` when this alias's
   * op did NOT run — either its own `when` evaluated `false`, or it was
   * cascade-skipped because a `DataFlow`/`Both`-provenance dependency was
   * itself skipped. Mirrors `query_result.rs::QueryResult.skipped`
   * (`#[serde(default, skip_serializing_if = "std::ops::Not::not")]`) —
   * omitted from the wire (and thus `undefined` here) when `false`, which
   * means the op executed normally.
   */
  skipped?: boolean;
  /**
   * Per-record version (FG-2), index-aligned with `records`. Present only
   * when the source `ReadQuery` set `with_version: true`. Each entry is the
   * canonical committed version of the corresponding record — use it as
   * `expected_version` on a subsequent UPDATE/DELETE for optimistic CAS.
   * Omitted when not requested or when the read path cannot structurally
   * attribute a version (aggregates, ORDER BY reordering, non-MVCC table).
   *
   * On the wire this is a bare `u64` per entry, so it round-trips as a
   * plain number/bigint — never a wrapped object (same rationale as
   * `CursorId`, see `types/cursor.ts`): `framing.ts`'s decoder
   * (`useBigInt64: true`) hands back a genuine `bigint` for any version
   * outside the safe-integer range, to avoid silently losing precision.
   */
  versions?: (number | bigint)[];
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

/**
 * Dependency-edge provenance tag (`EdgeKind`, `rename_all = "snake_case"`):
 * whether a batch DAG edge came from an explicit `after`, an auto-extracted
 * `$query` reference, or both. Mirrors
 * `shamir-query-types::batch::edge_kind::EdgeKind` (OQL Epic 01 / Phase A).
 */
export type EdgeKind = 'explicit' | 'data_flow' | 'both';

/** Batch response envelope. */
export interface BatchResponse {
  id: WireValue;
  results: Record<string, QueryResult>;
  execution_plan: string[][];
  /**
   * Dependency-edge provenance (for debugging): alias -> dep_alias ->
   * whether the edge came from an explicit `after`, an auto-extracted
   * `$query` reference, or both. Lets the client tell ordering-only edges
   * apart from real data-flow edges in `execution_plan`. Mirrors
   * `BatchResponse.edge_provenance` (`#[serde(default,
   * skip_serializing_if = "TMap::is_empty")]` — omitted when empty).
   */
  edge_provenance?: Record<string, Record<string, EdgeKind>>;
  execution_time_us: number;
  transaction?: TransactionInfo;
  /**
   * Per-repo interner deltas for ambient cache sync (Stage 5-wire, Part A).
   * Present only when the client advertised `interner_epochs` in the request.
   * Keyed by repo name.
   */
  interner_delta?: Record<string, WireInternerDelta>;
}
