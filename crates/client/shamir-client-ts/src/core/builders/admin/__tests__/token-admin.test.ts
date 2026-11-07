import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createApiToken, listApiTokens, revokeApiToken, revokeUserTokens } from '../index.js';
import { canonicalDbRequest, signCanonical } from '../../../protocol/hmac.js';
import { NodePlatform } from '../../../../platform/node.js';
import type { HmacSigner } from '../../../types/schema/ddl.js';

const fixture = JSON.parse(readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../../../../../../query/shamir-query-types/test-vectors/admin_hmac_v2.json'), 'utf8')) as {
  session_id_hex: string;
  vectors: Array<{ op_tag: string; op: string; fields: Record<string, unknown>; tag_hex: string }>;
};
const fromHex = (hex: string): Uint8Array => Uint8Array.from(hex.match(/.{2}/g) ?? [], byte => Number.parseInt(byte, 16));
const vec = (op: string) => {
  const found = fixture.vectors.find(vector => vector.op === op);
  if (!found) throw new Error(`missing ${op} vector`);
  return found;
};
const sessionId = fromHex(fixture.session_id_hex);
const signer: HmacSigner = {
  hmacTagHex: canonical => signCanonical(NodePlatform, sessionId, canonical),
};
const keys = (value: object): string[] => Object.keys(value).sort();

function fixtureOp(op: string): Uint8Array {
  const vector = vec(op);
  const f = vector.fields;
  switch (op) {
    case 'create_api_token': return canonicalDbRequest({
      op: 'create_api_token',
      label: f.label as string,
      ttl_secs: BigInt(f.ttl_secs as string),
      mode: f.mode as 'read_write',
      channels: f.channels as 'both',
      databases: [...f.databases as string[]],
      for_user: f.for_user as string,
    });
    case 'list_api_tokens': return canonicalDbRequest({
      op: 'list_api_tokens',
      for_user: f.for_user as string,
    });
    case 'revoke_api_token': return canonicalDbRequest({
      op: 'revoke_api_token',
      id: f.id as string,
    });
    case 'revoke_user_tokens': return canonicalDbRequest({
      op: 'revoke_user_tokens',
      user: f.user as null,
    });
    default: throw new Error(`unknown vector op: ${op}`);
  }
}

describe('API token admin builders', () => {
  it('createApiToken signs vector 0020 and emits only present fields', () => {
    const vector = vec('create_api_token');
    const spec = {
      label: vector.fields.label as string,
      ttl_secs: BigInt(vector.fields.ttl_secs as string),
      mode: vector.fields.mode as 'read_write',
      channels: vector.fields.channels as 'both',
      databases: [...vector.fields.databases as string[]],
      for_user: vector.fields.for_user as string,
    };
    const output = createApiToken(signer, spec);
    expect(output).toEqual({
      op: 'create_api_token',
      ...spec,
      hmac: vector.tag_hex,
    });
    expect(output.hmac).toBe(vector.tag_hex);
    expect(keys(output)).toEqual(['channels', 'databases', 'for_user', 'hmac', 'label', 'mode', 'op', 'ttl_secs']);
    expect(Object.values(output).some(value => value === undefined)).toBe(false);
    expect(canonicalDbRequest({
      op: 'create_api_token',
      label: spec.label,
      ttl_secs: spec.ttl_secs,
      mode: spec.mode,
      channels: spec.channels,
      databases: spec.databases,
      for_user: spec.for_user,
    })).toEqual(fixtureOp('create_api_token'));
    spec.databases.push('later');
    expect(output.databases).toEqual(['café-db', 'db']);
  });

  it('listApiTokens, revokeApiToken and revokeUserTokens sign their vectors', () => {
    const listVector = vec('list_api_tokens');
    const listed = listApiTokens(signer, {
      for_user: listVector.fields.for_user as string,
    });
    expect(listed).toEqual({
      op: 'list_api_tokens',
      for_user: 'bob',
      hmac: listVector.tag_hex,
    });
    expect(keys(listed)).toEqual(['for_user', 'hmac', 'op']);

    const revokeVector = vec('revoke_api_token');
    const revoked = revokeApiToken(signer, revokeVector.fields.id as string);
    expect(revoked).toEqual({
      op: 'revoke_api_token',
      id: revokeVector.fields.id,
      hmac: revokeVector.tag_hex,
    });
    expect(keys(revoked)).toEqual(['hmac', 'id', 'op']);

    const userVector = vec('revoke_user_tokens');
    const userRevoked = revokeUserTokens(signer);
    expect(userRevoked).toEqual({
      op: 'revoke_user_tokens',
      hmac: userVector.tag_hex,
    });
    expect(keys(userRevoked)).toEqual(['hmac', 'op']);
    expect(fixtureOp('list_api_tokens')).toEqual(canonicalDbRequest({
      op: 'list_api_tokens',
      for_user: 'bob',
    }));
    expect(fixtureOp('revoke_api_token')).toEqual(canonicalDbRequest({
      op: 'revoke_api_token',
      id: 'AAECAwQFBgcICQoLDA0ODw',
    }));
    expect(fixtureOp('revoke_user_tokens')).toEqual(canonicalDbRequest({
      op: 'revoke_user_tokens',
    }));
  });

  it('omits absent options, preserves raw usernames, and distinguishes empty databases', () => {
    const absent = createApiToken(signer, {
      label: 'x',
    });
    const empty = createApiToken(signer, {
      label: 'x',
      databases: [],
    });
    expect(keys(absent)).toEqual(['hmac', 'label', 'op']);
    expect(keys(listApiTokens(signer))).toEqual(['hmac', 'op']);
    expect(keys(revokeUserTokens(signer, {
      user: 'e\u0301',
    }))).toEqual(['hmac', 'op', 'user']);
    expect(revokeUserTokens(signer, {
      user: 'e\u0301',
    }).user).toBe('e\u0301');
    expect(absent.hmac).not.toBe(empty.hmac);
  });

  it('rejects invalid TTL and literals before invoking the signer', () => {
    const hmacTagHex = vi.fn((_canonical: Uint8Array): string => 'signed');
    const spySigner: HmacSigner = {
      hmacTagHex,
    };
    for (const ttl of [-1, 1.5, 2 ** 53, 2n ** 64n, -1n]) {
      expect(() => createApiToken(spySigner, {
        label: 'x',
        ttl_secs: ttl,
      })).toThrow(RangeError);
    }
    expect(() => createApiToken(spySigner, {
      label: 'x',
      mode: 'admin' as never,
    })).toThrow(TypeError);
    expect(() => createApiToken(spySigner, {
      label: 'x',
      channels: 'tcp' as never,
    })).toThrow(TypeError);
    expect(hmacTagHex).not.toHaveBeenCalled();
  });
});
