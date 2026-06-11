/**
 * Subscribe wire types — type-only mirror of
 * `crates/shamir-query-types/src/subscribe/`.
 *
 * Pure type declarations; the constructor code that assembles these
 * shapes lives in `../../builders/subscribe.ts`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Filter, FilterValue } from './filter.js';
import type { BatchRequest } from './batch.js';
import type { CallOp } from './call.js';
import type { TableRefWire } from './query.js';

// ── EventMask ───────────────────────────────────────────────────────

/** Which mutation events to subscribe to. Mirrors `EventMask` (serde `rename_all = "snake_case"`). */
export type EventMask = 'all' | 'put' | 'delete';

// ── SubscriptionSource ──────────────────────────────────────────────

/**
 * One table + filter + event combination to watch.
 * `table` uses the same wire format as `ReadQuery.from` — bare string
 * for repo "main", or `[repo, table]` tuple (custom serde on `TableRef`).
 */
export interface SubscriptionSource {
  table: TableRefWire;
  filter?: Filter;
  events?: EventMask;
}

// ── DeliverMode ─────────────────────────────────────────────────────

/**
 * Externally-tagged enum (serde default). Unit variants serialize as
 * bare strings; newtype variants as `{ "variant": data }`.
 */
export type DeliverMode =
  | 'records'
  | 'keys'
  | { batch: { batch: BatchRequest; bind?: Record<string, FilterValue> } }
  | { call: CallOp };

// ── SubscribeOp ─────────────────────────────────────────────────────

/** Wire shape for a subscribe batch operation. */
export interface SubscribeOp {
  subscribe: SubscriptionSource[];
  deliver?: DeliverMode;
  initial?: boolean;
  from_version?: number;
}

// ── UnsubscribeOp ───────────────────────────────────────────────────

/** Wire shape for an unsubscribe batch operation. */
export interface UnsubscribeOp {
  unsubscribe: number;
}

// ── Push envelope (server → client) ────────────────────────────────

/** Kind of push message from the server. */
export type PushKind = 'event' | 'gap' | 'slow_consumer' | 'ready' | 'closed';

/** Server push envelope for a subscription. */
export interface PushEnvelope {
  push: PushKind;
  sub: number;
  seq: number;
  data?: Uint8Array;
  gap_at?: number;
}
