/**
 * Typed `changes_since` request + lossless decode of the server payload.
 *
 * PLATFORM-AGNOSTIC.
 */

import { Batch, changesSince as changesSinceOp } from '../builders/index.js';
import { position } from '../visibility/positions.js';
import type {
  ChangesSinceEvent,
  ChangesSinceRecordChange,
  ChangesSinceResult,
} from './types.js';

/** Journal bytes arrive in TWO wire forms: `key` as MessagePack bin
 * (decoded to Uint8Array by the framing layer) and the optional `value`
 * as an ARRAY OF BYTE INTEGERS (the server's `serde_opt_bytes_compat`
 * emits a transparent `Some` of a slice, which is a fixarray on the
 * wire). Both are normalized to Uint8Array. */
function asBytes(v: unknown): Uint8Array {
  if (v instanceof Uint8Array) return v;
  if (
    Array.isArray(v) &&
    v.every((b) => typeof b === 'number' && Number.isInteger(b) && b >= 0 && b <= 255)
  ) {
    return Uint8Array.from(v as number[]);
  }
  throw new Error('malformed changes_since bytes field');
}

function decodeChange(raw: unknown): ChangesSinceRecordChange {
  if (raw === null || typeof raw !== 'object') {
    throw new Error('malformed changes_since record change');
  }
  const change = raw as Record<string, unknown>;
  return {
    table: String(change.table),
    key: asBytes(change.key),
    op: change.op === 'Delete' ? 'Delete' : 'Put',
    value: change.value == null ? null : asBytes(change.value),
  };
}

function decodeEvent(raw: unknown): ChangesSinceEvent {
  if (raw === null || typeof raw !== 'object') {
    throw new Error('malformed changes_since event');
  }
  const event = raw as Record<string, unknown>;
  return {
    repo: String(event.repo),
    commit_version: position(event.commit_version),
    tx_id: position(event.tx_id),
    actor: event.actor,
    timestamp_ns: position(event.timestamp_ns),
    changes: Array.isArray(event.changes) ? event.changes.map(decodeChange) : [],
  };
}

/** Decode the admin payload record (the server's msgpack round-trip of the
 * response map) into the typed result. Positions decode losslessly as
 * `bigint` via the framing decoder's `useBigInt64`. */
export function decodeChangesSince(payload: unknown): ChangesSinceResult {
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new Error('malformed changes_since payload');
  }
  const p = payload as Record<string, unknown>;
  return {
    changes_since: position(p.changes_since),
    events: Array.isArray(p.events) ? p.events.map(decodeEvent) : [],
    gap_at: p.gap_at == null ? null : position(p.gap_at),
    visible_through: p.visible_through == null ? null : position(p.visible_through),
  };
}

/** Execute a one-shot `changes_since` read built with the query builder
 * and return the typed result. `cursor` accepts `number | bigint`; values
 * above 2^53 must be passed as `bigint` to stay lossless on the
 * MessagePack path. Note: the server currently casts the sampled
 * positions through `i64`, so positions above `i64::MAX` are not
 * producible by the server today; the client decodes every u64 it DOES
 * send exactly (`bigint`). */
export async function requestChangesSince(
  client: { execute(db: string, batch: object): Promise<{ results: Record<string, { records: unknown[] }> }> },
  db: string,
  repo: string,
  cursor: number | bigint,
  limit?: number,
): Promise<ChangesSinceResult> {
  const op = changesSinceOp(cursor, { repo, limit });
  const batch = Batch.create('changes_since').add('changes_since', op).build();
  const response = await client.execute(db, batch);
  const result = response.results['changes_since'];
  if (result === undefined || result.records.length === 0) {
    throw new Error("server returned no payload for the changes_since op (alias 'changes_since')");
  }
  return decodeChangesSince(result.records[0]);
}
