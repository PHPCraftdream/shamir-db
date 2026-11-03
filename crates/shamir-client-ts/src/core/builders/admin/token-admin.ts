import { canonicalDbRequest } from '../../protocol/hmac.js';
import type { HmacSigner } from '../../types/schema/ddl.js';
import type {
  ApiTokenChannels,
  ApiTokenMode,
  CreateApiTokenOp,
  CreateApiTokenSpec,
  ListApiTokensOp,
  RevokeApiTokenOp,
  RevokeUserTokensOp,
} from '../../types/access/token-admin.js';

const MODES: readonly ApiTokenMode[] = ['read_only', 'read_write'];
const CHANNELS: readonly ApiTokenChannels[] = ['rest', 'ws', 'both'];

function optional<T extends object, K extends string, V>(target: T, key: K, value: V | null | undefined): T & Partial<Record<K, V>> {
  if (value != null) Object.assign(target, { [key]: value });
  return target as T & Partial<Record<K, V>>;
}

function validateTtl(value: number | bigint | undefined): void {
  if (value == null) return;
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value < 0) throw new RangeError('ttl_secs must be a safe non-negative integer');
    return;
  }
  if (typeof value !== 'bigint' || value < 0n || value > 0xffffffffffffffffn) throw new RangeError('ttl_secs out of range');
}

function validateLiteral<T extends string>(value: T | undefined, allowed: readonly T[], name: string): void {
  if (value !== undefined && !allowed.includes(value)) throw new TypeError(`invalid ${name}`);
}

export function createApiToken(signer: HmacSigner, spec: CreateApiTokenSpec): CreateApiTokenOp {
  validateTtl(spec.ttl_secs);
  validateLiteral(spec.mode, MODES, 'mode');
  validateLiteral(spec.channels, CHANNELS, 'channels');
  const op: CreateApiTokenOp = {
    op: 'create_api_token',
    label: spec.label,
    ...(spec.ttl_secs == null ? {} : { ttl_secs: spec.ttl_secs }),
    ...(spec.mode == null ? {} : { mode: spec.mode }),
    ...(spec.channels == null ? {} : { channels: spec.channels }),
    ...(spec.databases == null ? {} : { databases: [...spec.databases] }),
    ...(spec.for_user == null ? {} : { for_user: spec.for_user }),
    hmac: '',
  };
  const canonical = canonicalDbRequest(op);
  return { ...op, hmac: signer.hmacTagHex(canonical) };
}

export function listApiTokens(signer: HmacSigner, opts?: { for_user?: string }): ListApiTokensOp {
  const op = optional({ op: 'list_api_tokens' as const }, 'for_user', opts?.for_user);
  const canonical = canonicalDbRequest(op);
  return { ...op, hmac: signer.hmacTagHex(canonical) };
}

export function revokeApiToken(signer: HmacSigner, id: string): RevokeApiTokenOp {
  const op = { op: 'revoke_api_token' as const, id };
  return { ...op, hmac: signer.hmacTagHex(canonicalDbRequest(op)) };
}

export function revokeUserTokens(signer: HmacSigner, opts?: { user?: string }): RevokeUserTokensOp {
  const op = optional({ op: 'revoke_user_tokens' as const }, 'user', opts?.user);
  const canonical = canonicalDbRequest(op);
  return { ...op, hmac: signer.hmacTagHex(canonical) };
}
