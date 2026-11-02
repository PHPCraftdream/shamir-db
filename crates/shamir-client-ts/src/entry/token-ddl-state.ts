import type { DdlOpState } from '../core/types/write/batch.js';
import { TokenTransportError } from '../core/protocol/token/errors.js';

function malformed(): never { throw new TokenTransportError('protocol_error', 'Malformed DDL state'); }
function record(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value) || value instanceof Uint8Array) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
function timestamp(value: unknown): number {
  if (typeof value === 'bigint') {
    if (value < 0n || value > BigInt(Number.MAX_SAFE_INTEGER)) return malformed();
    return Number(value);
  }
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return malformed();
  return value;
}
export function normalizeTokenDdlState(value: unknown): DdlOpState {
  let kind: unknown;
  let fields: Record<string, unknown>;
  if (typeof value === 'string') {
    kind = value;
    fields = {};
  } else {
    if (!record(value)) return malformed();
    if (typeof value.kind === 'string') {
      kind = value.kind;
      fields = value;
    } else {
      const keys = Object.keys(value);
      if (keys.length !== 1) return malformed();
      kind = keys[0];
      const payload = value[keys[0] as string];
      if (!record(payload)) return malformed();
      fields = payload;
    }
  }
  switch (kind) {
    case 'in_progress': case 'unknown':
      if (Object.keys(fields).some((key) => key !== 'kind')) return malformed();
      return { kind };
    case 'succeeded':
      if (!Object.prototype.hasOwnProperty.call(fields, 'completed_at') || Object.keys(fields).some((key) => key !== 'kind' && key !== 'completed_at')) return malformed();
      return { kind, completed_at: timestamp(fields.completed_at) };
    case 'succeeded_via_crash_recovery':
      if (!Object.prototype.hasOwnProperty.call(fields, 'completed_at_restart') || Object.keys(fields).some((key) => key !== 'kind' && key !== 'completed_at_restart')) return malformed();
      return { kind, completed_at_restart: timestamp(fields.completed_at_restart) };
    case 'failed':
      if (typeof fields.detail !== 'string' || Object.keys(fields).some((key) => key !== 'kind' && key !== 'detail')) return malformed();
      return { kind, detail: fields.detail };
    default: return malformed();
  }
}
