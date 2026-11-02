import { decode, encode } from '../framing.js';
import type { PushKind } from '../../types/stream/subscribe.js';
import { TokenTransportError } from './errors.js';
export interface TokenBinding { readonly sid: Uint8Array; readonly expiresAtNs: bigint; }
export interface TokenPush { readonly push: 'event' | 'ready' | 'gap' | 'slow_consumer' | 'closed'; readonly sub: number; readonly seq: number; readonly data?: Uint8Array; readonly gap_at?: number; }
export interface TokenRequest {
  readonly op: string;
  readonly db?: string;
  readonly [key: string]: unknown;
}
export function encodeBrowserTokenFrame(token: string): Uint8Array {
  if (typeof token !== 'string' || token.length !== 71 || !/^shk1\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}$/.test(token)) throw new TokenTransportError('invalid_request', 'Invalid token frame');
  const frame = encode({ token });
  if (frame.byteLength > 256) throw new TokenTransportError('invalid_request', 'Invalid token frame');
  return frame;
}
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value) && !(value instanceof Uint8Array); }
export function decodeTokenBinding(bytes: Uint8Array): TokenBinding {
  try {
    const value: unknown = decode(bytes);
    if (!record(value) || Object.keys(value).sort().join(',') !== 'expires_at_ns,ok,sid' || value.ok !== true || !(value.sid instanceof Uint8Array) || value.sid.byteLength !== 32 || value.sid.every((b) => b === 0)) throw new Error();
    const expires = value.expires_at_ns;
    if (!(typeof expires === 'bigint' && expires > 0n) && !(typeof expires === 'number' && Number.isSafeInteger(expires) && expires > 0)) throw new Error();
    return Object.freeze({ sid: value.sid.slice(), expiresAtNs: BigInt(expires) });
  } catch { throw new TokenTransportError('protocol_error', 'Invalid token binding'); }
}
export function encodeTokenRequest(sid: Uint8Array, rid: number, request: object): Uint8Array {
  if (sid.byteLength !== 32 || !Number.isInteger(rid) || rid < 0 || rid > 0xffff_ffff || !request || typeof request !== 'object' || Array.isArray(request)) throw new TokenTransportError('invalid_request', 'Invalid token request');
  const outer = { sid, rid, req: encode(request) };
  return encode(outer);
}
export function decodeTokenFrame(bytes: Uint8Array): Record<string, unknown> {
  try { const value: unknown = decode(bytes); if (!record(value)) throw new Error(); return value; }
  catch { throw new TokenTransportError('protocol_error', 'Invalid token frame'); }
}
export function decodeTokenPush(frame: Record<string, unknown>): TokenPush | undefined {
  if (!('push' in frame)) return undefined;
  if (typeof frame.push !== 'string' || !['event','ready','gap','slow_consumer','closed'].includes(frame.push) || !Number.isSafeInteger(frame.sub) || (frame.sub as number) < 0 || !Number.isSafeInteger(frame.seq) || (frame.seq as number) < 0) throw new TokenTransportError('protocol_error', 'Invalid push frame');
  if (frame.data !== undefined && !(frame.data instanceof Uint8Array)) throw new TokenTransportError('protocol_error', 'Invalid push frame');
  if (frame.gap_at !== undefined && (!Number.isSafeInteger(frame.gap_at) || (frame.gap_at as number) < 0)) throw new TokenTransportError('protocol_error', 'Invalid push frame');
  return { push: frame.push as PushKind, sub: frame.sub as number, seq: frame.seq as number, ...(frame.data ? { data: frame.data } : {}), ...(frame.gap_at === undefined ? {} : { gap_at: frame.gap_at as number }) };
}
export function decodeTokenPayload(bytes: Uint8Array): unknown { try { return decode(bytes); } catch { throw new TokenTransportError('protocol_error', 'Invalid token payload'); } }
export function encodeTokenPayload(value: unknown): Uint8Array { return encode(value); }
