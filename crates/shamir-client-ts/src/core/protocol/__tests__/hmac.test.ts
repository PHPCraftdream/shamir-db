import { describe, it, expect } from 'vitest';
import { createHash, createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonicalBatchOp, canonicalDbRequest, deriveSessionHmacKey, signCanonical } from '../hmac.js';
import { ddl } from '../../builders/index.js';
import { NodePlatform } from '../../../../platform/node.js';
const bytes = (value: Uint8Array) => Array.from(value);
const vectorFixture = JSON.parse(readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../../../../shamir-query-types/test-vectors/admin_hmac_v2.json'), 'utf8')) as {
  session_id_hex: string;
  vectors: Array<{ op_tag: string; op: string; fields: Record<string, unknown>; fields_hex: string; canonical_hex: string; tag_hex: string }>;
};
const fromHex = (hex: string): Uint8Array => Uint8Array.from(hex.match(/.{2}/g) ?? [], byte => Number.parseInt(byte, 16));
const str = (f: Record<string, unknown>, k: string) => f[k] as string;
const optStr = (f: Record<string, unknown>, k: string) => f[k] === null ? undefined : f[k] as string;
const flag = (f: Record<string, unknown>, k: string) => f[k] === 1;
const uint = (f: Record<string, unknown>, k: string): bigint => BigInt(f[k] as string | number);
const optU64 = (f: Record<string, unknown>, k: string): bigint | undefined => f[k] === null ? undefined : uint(f, k);
const list = (f: Record<string, unknown>, k: string): string[] => f[k] as string[];

function vectorCanonical(v: (typeof vectorFixture.vectors)[number]): Uint8Array {
  const f = v.fields;
  const resource = (): import('../../types/access/admin.js').ResourceRef => {
    const parts = f.resource as string[];
    const at = (index: number): string => {
      const value = parts[index];
      if (value === undefined) throw new Error(`missing resource component ${index}`);
      return value;
    };
    switch (f.resource_kind) {
      case 'database': return { database: at(0) };
      case 'store': return { store: [at(0), at(1)] };
      case 'table': return { table: [at(0), at(1), at(2)] };
      case 'function': return { function: at(0) };
      case 'function_folder': return { function_folder: parts };
      case 'function_namespace': return { function_namespace: true };
      default: throw new Error(`unknown resource kind: ${f.resource_kind}`);
    }
  };
  const group = () => f.group_kind === 'name' ? { name: str(f, 'group') } : { id: BigInt(str(f, 'group')) };
  switch (v.op) {
    case 'drop_db': return canonicalBatchOp('', { drop_db: str(f, 'name'), cascade: flag(f, 'cascade'), if_exists: flag(f, 'if_exists') });
    case 'drop_repo': return canonicalBatchOp(str(f, 'db'), { drop_repo: str(f, 'name'), cascade: flag(f, 'cascade'), if_exists: flag(f, 'if_exists') });
    case 'drop_table': return canonicalBatchOp(str(f, 'db'), { drop_table: str(f, 'name'), repo: str(f, 'repo'), cascade: flag(f, 'cascade'), if_exists: flag(f, 'if_exists') });
    case 'drop_index': return canonicalBatchOp(str(f, 'db'), { drop_index: str(f, 'name'), repo: str(f, 'repo'), table: str(f, 'table'), if_exists: flag(f, 'if_exists') });
    case 'drop_user': return canonicalBatchOp('', { drop_user: str(f, 'name'), if_exists: flag(f, 'if_exists') });
    case 'start_migration': return canonicalBatchOp(str(f, 'db'), { start_migration: str(f, 'name'), repo: str(f, 'repo'), dst_repo: str(f, 'dst_repo'), dst_engine: str(f, 'dst_engine'), dst_path: optStr(f, 'dst_path') });
    case 'commit_migration': return canonicalBatchOp(str(f, 'db'), { commit_migration: str(f, 'name') });
    case 'rollback_migration': return canonicalBatchOp(str(f, 'db'), { rollback_migration: str(f, 'name') });
    case 'grant_role': return canonicalBatchOp('', { grant_role: str(f, 'role'), user: str(f, 'user') });
    case 'revoke_role': return canonicalBatchOp('', { revoke_role: str(f, 'role'), user: str(f, 'user') });
    case 'chmod': return canonicalBatchOp('', { chmod: resource(), mode: Number(f.mode) });
    case 'chown': return canonicalBatchOp('', { chown: resource(), owner: uint(f, 'owner') });
    case 'chgrp': return canonicalBatchOp('', { chgrp: resource(), group: f.group_id === null ? null : Number(f.group_id) });
    case 'create_user': return canonicalBatchOp('', { create_user: str(f, 'name'), password: 'excluded', roles: list(f, 'roles'), database: optStr(f, 'database') });
    case 'set_retention': return canonicalBatchOp(str(f, 'db'), { set_retention: str(f, 'name'), repo: str(f, 'repo'), retention: { max_age_secs: optU64(f, 'max_age_secs'), max_count: optU64(f, 'max_count'), min_count: optU64(f, 'min_count') } });
    case 'purge_history': return canonicalBatchOp(str(f, 'db'), { purge_history: str(f, 'name'), repo: str(f, 'repo'), scope: f.scope === 'older_than' ? { older_than: { timestamp: uint(f, 'timestamp') } } : { older_than_age: { age_secs: uint(f, 'age_secs') } } });
    case 'create_group': return canonicalBatchOp('', { create_group: str(f, 'name') });
    case 'drop_group': return canonicalBatchOp('', { drop_group: group(), if_exists: flag(f, 'if_exists') });
    case 'rename_group': return canonicalBatchOp('', { rename_group: group(), to: str(f, 'to') });
    case 'add_group_member': return canonicalBatchOp('', { add_group_member: group(), user: uint(f, 'user') });
    case 'remove_group_member': return canonicalBatchOp('', { remove_group_member: group(), user: uint(f, 'user') });
    case 'create_function': return canonicalBatchOp('', { create_function: str(f, 'name'), replace: flag(f, 'replace'), security: str(f, 'security'), visibility: optStr(f, 'visibility'), secret_grants: list(f, 'secret_grants'), net_grants: list(f, 'net_grants'), source: optStr(f, 'source'), wasm: optStr(f, 'wasm') });
    case 'create_validator': return canonicalBatchOp('', { create_validator: str(f, 'name'), replace: flag(f, 'replace'), source: optStr(f, 'source'), wasm: optStr(f, 'wasm') } as never);
    case 'drop_function': return canonicalBatchOp('', { drop_function: str(f, 'name'), if_exists: flag(f, 'if_exists') });
    case 'drop_validator': return canonicalBatchOp('', { drop_validator: str(f, 'name'), if_exists: flag(f, 'if_exists') });
    case 'drop_replication_profile': return canonicalBatchOp('', { drop_replication_profile: str(f, 'name') } as never);
    case 'drop_publication': return canonicalBatchOp('', { drop_publication: str(f, 'name') } as never);
    case 'drop_subscription': return canonicalBatchOp('', { drop_subscription: str(f, 'name') } as never);
    case 'set_superuser': return canonicalDbRequest({ op: 'set_superuser', user: str(f, 'user'), on: flag(f, 'on') });
    case 'set_replicator': return canonicalDbRequest({ op: 'set_replicator', user: str(f, 'user'), on: flag(f, 'on') });
    case 'create_scram_user': return canonicalDbRequest({ op: 'create_scram_user', name: str(f, 'name'), roles: list(f, 'roles') });
    default: throw new Error(`unknown vector op: ${v.op}`);
  }
}

describe('canonical v2 encoder', () => {
  it('rv_query_types_c7aec31f_matches_known_answer_vectors_for_every_tag', () => {
    expect(vectorFixture.vectors).toHaveLength(31);
    const tags = new Set<number>();
    const sessionId = fromHex(vectorFixture.session_id_hex);
    const key = createHash('sha256').update('shamir-db hmac key v1\0').update(sessionId).digest();
    for (const vector of vectorFixture.vectors) {
      const tag = Number.parseInt(vector.op_tag, 16);
      expect(tag).toBeGreaterThanOrEqual(1);
      expect(tag).toBeLessThanOrEqual(31);
      expect(tags.has(tag)).toBe(false);
      tags.add(tag);
      const canonical = vectorCanonical(vector);
      const prefixAndTag = fromHex(`7368616d69722d646220686d61632063616e6f6e6963616c20763200${vector.op_tag}`);
      expect(bytes(canonical.slice(0, prefixAndTag.length))).toEqual(bytes(prefixAndTag));
      expect(bytes(canonical.slice(prefixAndTag.length))).toEqual(bytes(fromHex(vector.fields_hex)));
      expect(bytes(canonical)).toEqual(bytes(fromHex(vector.canonical_hex)));
      const expectedTag = createHmac('sha256', key).update(canonical).digest('hex');
      expect(expectedTag).toBe(vector.tag_hex);
      expect(signCanonical(NodePlatform, sessionId, canonical)).toBe(vector.tag_hex);
    }
    expect([...tags].sort((a, b) => a - b)).toEqual(Array.from({ length: 31 }, (_, i) => i + 1));
  });

  it('rv_query_types_f1dc8e9a_builder_hmac_uses_fixture_known_answer_tag', () => {
    const vector = vectorFixture.vectors.find(v => v.op === 'drop_db');
    if (!vector) throw new Error('drop_db vector missing');
    const sessionId = fromHex(vectorFixture.session_id_hex);
    const canonicalOp = { drop_db: str(vector.fields, 'name'), cascade: flag(vector.fields, 'cascade'), if_exists: flag(vector.fields, 'if_exists') };
    const canonical = canonicalBatchOp('', canonicalOp);
    expect(bytes(canonical)).toEqual(bytes(fromHex(vector.canonical_hex)));
    expect(signCanonical(NodePlatform, sessionId, canonical)).toBe(vector.tag_hex);
    const signer = { hmacTagHex: (canonicalBytes: Uint8Array) => signCanonical(NodePlatform, sessionId, canonicalBytes) };
    const signedOp = ddl.dropDb(signer, str(vector.fields, 'name'), { cascade: flag(vector.fields, 'cascade'), if_exists: flag(vector.fields, 'if_exists') });
    expect(signedOp.hmac).toBe(vector.tag_hex);
  });

  it('rv_query_types_c7aec31f_preserves_group_ids_above_js_safe_integer', () => {
    const exactId = 9_007_199_254_740_993n;
    const canonical = canonicalBatchOp('', { drop_group: { id: exactId }, if_exists: false });
    expect(Array.from(canonical.slice(-9))).toEqual([0, 0x20, 0, 0, 0, 0, 0, 1, 0]);
  });
  it('rv_query_types_c7aec31f_separates_resource_variants_and_list_boundaries', () => {
    const fn = canonicalBatchOp('', { chmod: { function: '' }, mode: 0 });
    const namespace = canonicalBatchOp('', { chmod: { function_namespace: true }, mode: 0 });
    const oneFolder = canonicalBatchOp('', { chmod: { function_folder: ['a/b'] }, mode: 0 });
    const twoFolders = canonicalBatchOp('', { chmod: { function_folder: ['a', 'b'] }, mode: 0 });
    expect(bytes(fn)).not.toEqual(bytes(namespace));
    expect(bytes(oneFolder)).not.toEqual(bytes(twoFolders));
  });
  it('binds alias-sensitive flags, names, and UTF-8 byte lengths', () => {
    const op = { drop_db: 'é', cascade: false, if_exists: false };
    const first = canonicalBatchOp('', op);
    expect(bytes(canonicalBatchOp('', { ...op, cascade: true }))).not.toEqual(bytes(first));
    expect(bytes(canonicalBatchOp('', { ...op, drop_db: 'e' }))).not.toEqual(bytes(first));
  });
  it('encodes top-level SCRAM name/roles, excludes password, defaults roles', () => {
    const request = canonicalDbRequest({ op: 'create_scram_user', name: 'é', password: 'x' });
    expect(bytes(request)).toEqual(bytes(canonicalDbRequest({ op: 'create_scram_user', name: 'é', roles: [] })));
    expect(bytes(request)).not.toEqual(bytes(canonicalDbRequest({ op: 'create_scram_user', name: 'e', roles: [] })));
  });
  it('rejects unsafe u64 numbers', () => {
    expect(() => canonicalBatchOp('', { chown: { database: 'd' }, owner: Number.MAX_SAFE_INTEGER + 1 } as never)).toThrow(RangeError);
  });
  it('accepts u16 mode boundaries and rejects values outside the range', () => {
    for (const mode of [0, 65535]) {
      expect(() => canonicalBatchOp('', { chmod: { database: 'd' }, mode })).not.toThrow();
    }
    for (const mode of [-1, 65536, 1.5]) {
      expect(() => canonicalBatchOp('', { chmod: { database: 'd' }, mode })).toThrow(RangeError);
    }
  });
  it('encodes unicode resource strings with UTF-8 byte lengths', () => {
    const canonical = canonicalBatchOp('', { chmod: { database: 'é' }, mode: 0 });
    const prefix = new TextEncoder().encode('shamir-db hmac canonical v2\0');
    expect(Array.from(canonical.slice(prefix.length + 3, prefix.length + 7))).toEqual([0, 0, 0, 2]);
    expect(Array.from(canonical.slice(prefix.length + 7, prefix.length + 9))).toEqual([0xc3, 0xa9]);
  });
  it('keeps function source digest tied to exact UTF-8 string bytes', () => {
    const a = canonicalBatchOp('', { create_function: 'f', source: 'é', replace: true, secret_grants: [], net_grants: [] } as never);
    const b = canonicalBatchOp('', { create_function: 'f', source: 'e', replace: true, secret_grants: [], net_grants: [] } as never);
    expect(bytes(a)).not.toEqual(bytes(b));
  });
});

describe('HMAC key and tag', () => {
  it('uses unchanged session key derivation and node crypto tag', () => {
    const sid = new Uint8Array(32).fill(1);
    const canonical = canonicalBatchOp('db', { drop_table: 'users', repo: 'main', cascade: false, if_exists: false });
    const key = createHash('sha256').update('shamir-db hmac key v1\0').update(sid).digest();
    expect(bytes(deriveSessionHmacKey(NodePlatform, sid))).toEqual(Array.from(key));
    const expected = createHmac('sha256', key).update(canonical).digest('hex');
    expect(signCanonical(NodePlatform, sid, canonical)).toBe(expected);
  });
});
