/**
 * Subscribe builder — constructs wire `SubscribeOp` / `UnsubscribeOp`
 * shapes from a user-friendly config object.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { Filter, FilterValue } from '../types/filter.js';
import type { BatchRequest } from '../types/batch.js';
import type { CallOp } from '../types/call.js';
import type { TableRefWire } from '../types/query.js';
import type {
  EventMask,
  SubscriptionSource,
  DeliverMode,
  SubscribeOp,
  UnsubscribeOp,
} from '../types/subscribe.js';
import { filter } from './filter.js';
import { Batch } from './batch.js';

// ── Public config types ─────────────────────────────────────────────

/** User-facing event name. `'any'` maps to wire `'all'`. */
export type EventName = 'put' | 'delete' | 'any';

/** User-friendly subscription source config. */
export interface SubscribeSource {
  /** Repository name. */
  store: string;
  /** Table name. */
  table: string;
  /** Filter — callback receiving the filter namespace, or a literal Filter. */
  where?: Filter | ((f: typeof filter) => Filter);
  /** Events to watch. Defaults to `['any']`. */
  on?: EventName[];
  /** Simple deliver mode (mutually exclusive with `handle`/`call`). */
  deliver?: 'records' | 'keys';
  /** Callback that builds a sub-batch for DeliverMode.Batch (mutually exclusive with `deliver`/`call`). */
  handle?: (b: Batch) => Batch;
  /**
   * Stored-function call to invoke on delivery (DeliverMode.Call).
   * Build with the {@link call} constructor; mutually exclusive with
   * `deliver`/`handle`.
   */
  call?: CallOp;
  /** Parameter bindings for the sub-batch (only meaningful with `handle`). */
  bind?: Record<string, FilterValue>;
}

/** Options for the subscribe builder. */
export interface SubscribeOpts {
  initial?: boolean;
  fromVersion?: number;
}

// ── Helpers ─────────────────────────────────────────────────────────

function resolveEventMask(on?: EventName[]): EventMask | undefined {
  if (!on || on.length === 0) return undefined;
  if (on.length > 1 || on.includes('any')) return 'all';
  return on[0] === 'any' ? 'all' : on[0];
}

function resolveFilter(w?: Filter | ((f: typeof filter) => Filter)): Filter | undefined {
  if (w === undefined) return undefined;
  return typeof w === 'function' ? w(filter) : w;
}

function resolveTableRef(store: string, table: string): TableRefWire {
  return store === 'main' ? table : [store, table];
}

function resolveDeliverMode(src: SubscribeSource): DeliverMode | undefined {
  // Enforce mutual exclusion of deliver modes within a single source.
  const active: string[] = [];
  if (src.handle) active.push('handle');
  if (src.call) active.push('call');
  if (src.deliver) active.push(`deliver:'${src.deliver}'`);
  if (active.length > 1) {
    throw new Error(
      `subscribe: mutually exclusive deliver options on one source — at most one of { deliver, handle, call } may be set (got: ${active.join(', ')})`,
    );
  }

  if (src.handle) {
    const inner = src.handle(Batch.create());
    const req: BatchRequest = inner.build();
    const sub: { batch: BatchRequest; bind?: Record<string, FilterValue> } = { batch: req };
    if (src.bind && Object.keys(src.bind).length > 0) sub.bind = src.bind;
    return { batch: sub };
  }
  if (src.call) {
    return { call: src.call };
  }
  if (src.deliver === 'keys') return 'keys';
  if (src.deliver === 'records') return 'records';
  return undefined;
}

// ── Builder functions ───────────────────────────────────────────────

/**
 * Build a wire `SubscribeOp` from user-friendly source config(s).
 * Returns a `SubscribeOp` ready to be passed to `batch.add()`.
 */
export function subscribe(
  sources: SubscribeSource | SubscribeSource[],
  opts?: SubscribeOpts,
): SubscribeOp {
  const arr = Array.isArray(sources) ? sources : [sources];

  const wireSources: SubscriptionSource[] = arr.map((src) => {
    const ws: SubscriptionSource = {
      table: resolveTableRef(src.store, src.table),
    };
    const f = resolveFilter(src.where);
    if (f) ws.filter = f;
    const ev = resolveEventMask(src.on);
    if (ev) ws.events = ev;
    return ws;
  });

  // Server-side `deliver` is op-level: one DeliverMode for the whole
  // SubscribeOp. If sources disagree, user intent is ambiguous — throw
  // rather than silently picking the first.
  const resolvedModes: DeliverMode[] = [];
  for (const src of arr) {
    const m = resolveDeliverMode(src);
    if (m !== undefined) resolvedModes.push(m);
  }
  let deliver: DeliverMode | undefined;
  if (resolvedModes.length > 0) {
    const first = resolvedModes[0];
    const firstKey = JSON.stringify(first);
    for (let i = 1; i < resolvedModes.length; i++) {
      if (JSON.stringify(resolvedModes[i]) !== firstKey) {
        throw new Error(
          'subscribe: conflicting deliver/handle/call across sources — all sources in one subscription must agree on delivery mode',
        );
      }
    }
    deliver = first;
  }

  const op: SubscribeOp = { subscribe: wireSources };
  if (deliver) op.deliver = deliver;
  if (opts?.initial) op.initial = true;
  if (opts?.fromVersion !== undefined) op.from_version = opts.fromVersion;

  return op;
}

/**
 * Build a wire `UnsubscribeOp`.
 * Returns `{ unsubscribe: subId }` ready to be passed to `batch.add()`.
 */
export function unsubscribeOp(subId: number): UnsubscribeOp {
  return { unsubscribe: subId };
}
