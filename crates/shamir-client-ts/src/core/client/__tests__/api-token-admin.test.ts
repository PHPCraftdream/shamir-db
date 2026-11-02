import { afterEach, describe, expect, it } from 'vitest';
import { inspect } from 'node:util';
import type { ResumeOptions } from '../../types/connection.js';
import type { Platform, Socket } from '../../support/platform.js';
import { encode, decode } from '../../protocol/framing.js';
import { NodePlatform } from '../../../platform/node.js';
import { ShamirClient } from '../client.js';
import { ApiToken } from '../../../entry/api-token.js';
import { ShamirDbError, TOKEN_ADMIN_ERROR_CODES, isTokenAdminError, type TokenAdminErrorCode } from '../../support/errors.js';
import { canonicalDbRequest, signCanonical } from '../../protocol/hmac.js';

class FakeSocket implements Socket {
  readonly sent: Uint8Array[] = [];
  private messageHandler: (data: Uint8Array) => void = () => {};
  private closeHandler: (err?: Error) => void = () => {};
  private pending: Uint8Array[] = [];
  private handlerRegistered = false;
  private closed = false;
  private sendWaiters: Array<() => void> = [];
  send(data: Uint8Array): void {
    if (this.closed) throw new Error('connection closed');
    this.sent.push(data);
    for (const resolve of this.sendWaiters.splice(0)) resolve();
  }
  onMessage(handler: (data: Uint8Array) => void): void {
    this.messageHandler = handler;
    this.handlerRegistered = true;
    for (const frame of this.pending.splice(0)) this.messageHandler(frame);
  }
  onClose(handler: (err?: Error) => void): void { this.closeHandler = handler; }
  close(): Promise<void> { this.closed = true; this.closeHandler(); return Promise.resolve(); }
  waitForSend(): Promise<void> {
    return new Promise(resolve => {
      this.sendWaiters.push(resolve);
    });
  }
  pushFrame(body: Uint8Array): void {
    const frame = new Uint8Array(4 + body.length);
    const size = body.length;
    frame[0] = size >>> 24; frame[1] = size >>> 16; frame[2] = size >>> 8; frame[3] = size;
    frame.set(body, 4);
    if (this.handlerRegistered) this.messageHandler(frame); else this.pending.push(frame);
  }
}

function platformWithSocket(): { platform: Platform; socket: FakeSocket } {
  const socket = new FakeSocket();
  const platform: Platform = {
    ...NodePlatform,
    openSocket: async () => socket,
  };
  return { platform, socket };
}
function resumeOkFrame(): Uint8Array {
  return encode([
    new Uint8Array(32).fill(0x07),
    1_800_000_000_000_000_000n,
    new Uint8Array(0),
    0n,
    4,
  ]);
}
function decodeRequest(frame: Uint8Array): { rid: number; req: Record<string, unknown> } {
  const envelope = decode(frame.subarray(4)) as Record<string, unknown>;
  return { rid: envelope.rid as number, req: decode(envelope.req as Uint8Array) as Record<string, unknown> };
}
function pushReply(socket: FakeSocket, rid: number, response: Record<string, unknown>): void {
  socket.pushFrame(encode({ rid, res: encode(response) }));
}

const clients: ShamirClient[] = [];
afterEach(async () => {
  while (clients.length > 0) {
    const client = clients.pop();
    if (client) await client.close();
  }
});

async function makeClient(): Promise<{ client: ShamirClient; socket: FakeSocket }> {
  const { platform, socket } = platformWithSocket();
  socket.pushFrame(resumeOkFrame());
  const opts: ResumeOptions = {
    host: 'localhost',
    port: 443,
    ticket: new Uint8Array([1]),
    serverPubKey: new Uint8Array(32),
  };
  const client = await ShamirClient.resume(platform, opts);
  clients.push(client);
  return { client, socket };
}
async function captureRequest<T>(socket: FakeSocket, call: () => Promise<T>): Promise<{ result: Promise<T>; req: Record<string, unknown>; rid: number }> {
  const previous = socket.sent.length;
  const sent = socket.waitForSend();
  const result = call();
  await sent;
  expect(socket.sent).toHaveLength(previous + 1);
  const { rid, req } = decodeRequest(socket.sent[previous]!);
  return { result, req, rid };
}
function rejection(promise: Promise<unknown>): Promise<Error> {
  return promise.then(
    () => { throw new Error('expected rejection'); },
    (error: unknown) => {
      expect(error).toBeInstanceOf(Error);
      return error as Error;
    },
  );
}

function expectSignature(req: Record<string, unknown>, op: Parameters<typeof canonicalDbRequest>[0]): void {
  const canonical = canonicalDbRequest(op);
  expect(req.hmac).toBe(signCanonical(NodePlatform, new Uint8Array(32).fill(0x07), canonical));
}
const tokenSecret =
  'shk1.AAECAwQFBgcICQoLDA0ODw.AAECAwQFBgcICQoLDA0ODwAAAAAAAAAAAAAAAAAAAAA';
const tokenId = tokenSecret.slice(5, 27);
const createdResponse = (overrides: Record<string, unknown> = {}): Record<string, unknown> => ({
  kind: 'api_token_created',
  id: tokenId,
  token: tokenSecret,
  label: 'robot',
  owner: 'e\u0301',
  mode: 'read_only',
  channels: 'rest',
  databases: null,
  created_at_ns: 9_007_199_254_740_993n,
  expires_at_ns: 2_000_000_000_000_000_000n,
  ...overrides,
});

describe('ShamirClient.apiTokens', () => {
  it('creates with exact wire keys, session HMAC, typed result and redaction', async () => {
    const { client, socket } = await makeClient();
    const operation = await captureRequest(socket, () => client.apiTokens.create({
      label: 'robot',
      ttl_secs: 9_007_199_254_740_993n,
      mode: 'read_write',
      channels: 'both',
      databases: ['db'],
      for_user: 'e\u0301',
    }));
    const expectedHmac = operation.req.hmac;
    expect(operation.req).toEqual({
      op: 'create_api_token',
      label: 'robot',
      ttl_secs: 9_007_199_254_740_993n,
      mode: 'read_write',
      channels: 'both',
      databases: ['db'],
      for_user: 'e\u0301',
      hmac: expectedHmac,
    });
    expect(Object.keys(operation.req).sort()).toEqual(['channels', 'databases', 'for_user', 'hmac', 'label', 'mode', 'op', 'ttl_secs']);
    expectSignature(operation.req, {
      op: 'create_api_token',
      label: 'robot',
      ttl_secs: 9_007_199_254_740_993n,
      mode: 'read_write',
      channels: 'both',
      databases: ['db'],
      for_user: 'e\u0301',
    });
    pushReply(socket, operation.rid, createdResponse({
      label: 'robot',
      owner: 'e\u0301',
      mode: 'read_write',
      channels: 'both',
      databases: ['db'],
    }));
    const created = await operation.result;
    expect(created).toEqual({
      id: tokenId,
      token: created.token,
      label: 'robot',
      owner: 'e\u0301',
      mode: 'read_write',
      channels: 'both',
      databases: ['db'],
      created_at_ns: 9_007_199_254_740_993n,
      expires_at_ns: 2_000_000_000_000_000_000n,
    });
    expect(created.token).toBeInstanceOf(ApiToken);
    expect(created.created_at_ns).toBe(9_007_199_254_740_993n);
    expect(created.expires_at_ns).toBe(2_000_000_000_000_000_000n);
    expect(created.mode).toBe('read_write');
    expect(created.channels).toBe('both');
    expect(created.databases).toEqual(['db']);
    expect(String(created.token)).not.toContain(tokenSecret);
    expect(JSON.stringify(created.token)).not.toContain(tokenSecret);
    expect(JSON.stringify(created, (_key, value) => typeof value === 'bigint' ? value.toString() : value)).not.toContain(tokenSecret);
    expect(inspect(created, { depth: 5 })).not.toContain(tokenSecret);
  });

  it('lists rows from tokens, normalizes integers and strips unlisted secret metadata', async () => {
    const { client, socket } = await makeClient();
    const operation = await captureRequest(socket, () => client.apiTokens.list({
      for_user: 'e\u0301',
    }));
    expect(operation.req.op).toBe('list_api_tokens');
    expect(operation.req).toEqual({
      op: 'list_api_tokens',
      for_user: 'e\u0301',
      hmac: operation.req.hmac,
    });
    expect(Object.keys(operation.req).sort()).toEqual(['for_user', 'hmac', 'op']);
    expect(operation.req.for_user).toBe('e\u0301');
    expectSignature(operation.req, {
      op: 'list_api_tokens',
      for_user: 'e\u0301',
    });
    pushReply(socket, operation.rid, {
      kind: 'api_tokens_listed',
      tokens: [
        {
          id: tokenId,
          label: 'robot',
          owner: 'e\u0301',
          mode: 'read_write',
          channels: 'both',
          databases: ['db'],
          created_at_ns: 7,
          expires_at_ns: 9_007_199_254_740_993n,
          revoked_at_ns: 9,
          token: tokenSecret,
          mac: tokenSecret,
          hash: tokenSecret,
        },
      ],
    });
    const rows = await operation.result;
    expect(rows).toEqual([
      {
        id: tokenId,
        label: 'robot',
        owner: 'e\u0301',
        mode: 'read_write',
        channels: 'both',
        databases: ['db'],
        created_at_ns: 7n,
        expires_at_ns: 9_007_199_254_740_993n,
        revoked_at_ns: 9n,
      },
    ]);
    expect(JSON.stringify(rows, (_key, value) => typeof value === 'bigint' ? value.toString() : value)).not.toContain(tokenSecret);
  });

  it('revokes one token with the exact signed wire operation', async () => {
    const { client, socket } = await makeClient();
    const operation = await captureRequest(socket, () => client.apiTokens.revoke(tokenId));
    expect(operation.req.op).toBe('revoke_api_token');
    expect(operation.req).toEqual({
      op: 'revoke_api_token',
      id: tokenId,
      hmac: operation.req.hmac,
    });
    expect(Object.keys(operation.req).sort()).toEqual(['hmac', 'id', 'op']);
    expectSignature(operation.req, {
      op: 'revoke_api_token',
      id: tokenId,
    });
    pushReply(socket, operation.rid, {
      kind: 'api_token_revoked',
    });
    expect(await operation.result).toBeUndefined();
  });

  it('revokes user tokens with raw username and returns a bigint count', async () => {
    const { client, socket } = await makeClient();
    const operation = await captureRequest(socket, () => client.apiTokens.revokeUser({
      user: 'e\u0301',
    }));
    expect(operation.req).toEqual({
      op: 'revoke_user_tokens',
      user: 'e\u0301',
      hmac: operation.req.hmac,
    });
    expect(operation.req.user).toBe('e\u0301');
    expect(Object.keys(operation.req).sort()).toEqual(['hmac', 'op', 'user']);
    expectSignature(operation.req, {
      op: 'revoke_user_tokens',
      user: 'e\u0301',
    });
    pushReply(socket, operation.rid, {
      kind: 'api_user_tokens_revoked',
      count: 7,
    });
    expect(await operation.result).toBe(7n);
  });

  it('sends self-service operations with omitted optionals and exact keys', async () => {
    const { client, socket } = await makeClient();
    const create = await captureRequest(socket, () => client.apiTokens.create({
      label: 'local',
    }));
    expect(create.req).toEqual({
      op: 'create_api_token',
      label: 'local',
      hmac: create.req.hmac,
    });
    expect(Object.keys(create.req).sort()).toEqual(['hmac', 'label', 'op']);
    const createReply = rejection(create.result);
    pushReply(socket, create.rid, {
      kind: 'wrong',
    });
    expect((await createReply).message).toBe('malformed api_token_created response');

    const list = await captureRequest(socket, () => client.apiTokens.list());
    expect(list.req).toEqual({
      op: 'list_api_tokens',
      hmac: list.req.hmac,
    });
    expect(Object.keys(list.req).sort()).toEqual(['hmac', 'op']);
    pushReply(socket, list.rid, {
      kind: 'api_tokens_listed',
      tokens: [],
    });
    expect(await list.result).toEqual([]);

    const revoke = await captureRequest(socket, () => client.apiTokens.revoke(tokenId));
    expect(revoke.req).toEqual({
      op: 'revoke_api_token',
      id: tokenId,
      hmac: revoke.req.hmac,
    });
    expect(Object.keys(revoke.req).sort()).toEqual(['hmac', 'id', 'op']);
    pushReply(socket, revoke.rid, {
      kind: 'api_token_revoked',
    });
    expect(await revoke.result).toBeUndefined();

    const revokeUser = await captureRequest(socket, () => client.apiTokens.revokeUser());
    expect(revokeUser.req).toEqual({
      op: 'revoke_user_tokens',
      hmac: revokeUser.req.hmac,
    });
    expect(Object.keys(revokeUser.req).sort()).toEqual(['hmac', 'op']);
    pushReply(socket, revokeUser.rid, {
      kind: 'api_user_tokens_revoked',
      count: 0n,
    });
    expect(await revokeUser.result).toBe(0n);
  });

  it('maps server errors and recognizes exactly the token-admin code set', async () => {
    const { client, socket } = await makeClient();
    const operation = await captureRequest(socket, () => client.apiTokens.list());
    const result = rejection(operation.result);
    pushReply(socket, operation.rid, {
      kind: 'error',
      code: 'token_label_exists',
      message: 'label already exists',
    });
    const error = await result;
    expect(error).toBeInstanceOf(ShamirDbError);
    expect(error.cause).toBeUndefined();
    expect((error as ShamirDbError).code).toBe('token_label_exists');
    expect(isTokenAdminError(error)).toBe(true);
    expect(isTokenAdminError(error, 'token_label_exists')).toBe(true);
    expect(isTokenAdminError(error, 'token_not_found')).toBe(false);
    const expectedCodes: TokenAdminErrorCode[] = [
      'permission_denied', 'token_not_found', 'token_browser_session', 'token_request_denied',
      'token_self_service_disabled', 'read_only_replica', 'hmac_required', 'hmac_mismatch',
      'session_invalidated', 'invalid_username', 'not_found', 'not_supported', 'internal',
      'token_label_exists', 'token_limit_reached', 'token_ttl_invalid',
    ];
    expect([...TOKEN_ADMIN_ERROR_CODES].sort()).toEqual([...expectedCodes].sort());
    for (const code of expectedCodes) {
      expect(isTokenAdminError(new ShamirDbError(code, 'x'), code)).toBe(true);
      const codeError = new ShamirDbError(code, 'safe message');
      expect(codeError.message).not.toContain(tokenSecret);
      expect(codeError.cause).toBeUndefined();
    }
    expect(isTokenAdminError(new ShamirDbError('timeout', 'x'))).toBe(false);
    expect(isTokenAdminError({
      code: 'token_label_exists',
    })).toBe(false);
    expect(error.message).not.toContain(tokenSecret);
  });

  it('rejects unexpected response kinds with operation-specific messages', async () => {
    const { client, socket } = await makeClient();
    const list = await captureRequest(socket, () => client.apiTokens.list());
    const listResult = rejection(list.result);
    pushReply(socket, list.rid, {
      kind: 'wrong',
    });
    const listError = await listResult;
    expect(listError.message).toBe('unexpected DbResponse kind for list_api_tokens: wrong');
    expect(listError.message).not.toContain(tokenSecret);
    expect(listError.cause).toBeUndefined();

    const revoke = await captureRequest(socket, () => client.apiTokens.revoke(tokenId));
    const revokeResult = rejection(revoke.result);
    pushReply(socket, revoke.rid, {
      kind: 'wrong',
    });
    const revokeError = await revokeResult;
    expect(revokeError.message).toBe('unexpected DbResponse kind for revoke_api_token: wrong');
    expect(revokeError.message).not.toContain(tokenSecret);
    expect(revokeError.cause).toBeUndefined();

    const revokeUser = await captureRequest(socket, () => client.apiTokens.revokeUser());
    const userResult = rejection(revokeUser.result);
    pushReply(socket, revokeUser.rid, {
      kind: 'wrong',
    });
    const userError = await userResult;
    expect(userError.message).toBe('unexpected DbResponse kind for revoke_user_tokens: wrong');
    expect(userError.message).not.toContain(tokenSecret);
    expect(userError.cause).toBeUndefined();
  });

  it('rejects malformed creation responses with fixed secret-free errors', async () => {
    const malformed: Array<Record<string, unknown>> = [
      {
        token: 'short',
      },
      {
        token: `${tokenSecret.slice(0, -1)}B`,
      },
      {
        token: null,
      },
      {
        id: 'BBBBBBBBBBBBBBBBBBBBBB',
      },
      {
        id: 'short',
      },
      {
        label: undefined,
      },
      {
        label: null,
      },
      {
        owner: undefined,
      },
      {
        owner: null,
      },
      {
        mode: undefined,
      },
      {
        mode: 'admin',
      },
      {
        channels: undefined,
      },
      {
        channels: 'tcp',
      },
      {
        kind: 'wrong',
      },
      {
        created_at_ns: undefined,
      },
      {
        created_at_ns: -1,
      },
      {
        created_at_ns: 1.5,
      },
      {
        expires_at_ns: undefined,
      },
      {
        expires_at_ns: -1n,
      },
      {
        expires_at_ns: 1.5,
      },
      {
        databases: undefined,
      },
      {
        databases: ['db', 1],
      },
    ];
    for (const changes of malformed) {
      const { client, socket } = await makeClient();
      const operation = await captureRequest(socket, () => client.apiTokens.create({
        label: 'robot',
      }));
      const result = rejection(operation.result);
      const base = createdResponse();
      const response = Object.fromEntries(
        Object.entries({ ...base, ...changes }).filter(([, value]) => value !== undefined),
      );
      pushReply(socket, operation.rid, response);
      const error = await result;
      expect(error.message).toBe('malformed api_token_created response');
      expect(error.message).not.toContain(tokenSecret);
      expect(error.cause).toBeUndefined();
    }
  });

  it('validates malformed listing rows and revoke counts', async () => {
    const { client, socket } = await makeClient();
    const list = await captureRequest(socket, () => client.apiTokens.list());
    expect(list.req).toEqual({
      op: 'list_api_tokens',
      hmac: list.req.hmac,
    });
    expect(Object.keys(list.req).sort()).toEqual(['hmac', 'op']);
    const listResult = rejection(list.result);
    pushReply(socket, list.rid, {
      kind: 'api_tokens_listed',
      tokens: [
        {
          id: tokenId,
        },
      ],
    });
    expect((await listResult).message).toBe('malformed api token response');

    const revokeUser = await captureRequest(socket, () => client.apiTokens.revokeUser());
    expect(revokeUser.req).toEqual({
      op: 'revoke_user_tokens',
      hmac: revokeUser.req.hmac,
    });
    expect(Object.keys(revokeUser.req).sort()).toEqual(['hmac', 'op']);
    const countResult = rejection(revokeUser.result);
    pushReply(socket, revokeUser.rid, {
      kind: 'api_user_tokens_revoked',
      count: -1,
    });
    expect((await countResult).message).toBe('malformed api token response');
  });

  it('normalizes small creation timestamps and absent, null and wide revocation timestamps', async () => {
    const { client, socket } = await makeClient();
    const create = await captureRequest(socket, () => client.apiTokens.create({
      label: 'small',
    }));
    expect(create.req).toEqual({
      op: 'create_api_token',
      label: 'small',
      hmac: create.req.hmac,
    });
    expectSignature(create.req, {
      op: 'create_api_token',
      label: 'small',
    });
    pushReply(socket, create.rid, createdResponse({
      label: 'small',
      created_at_ns: 1,
      expires_at_ns: 2,
    }));
    const created = await create.result;
    expect(created.created_at_ns).toBe(1n);
    expect(created.expires_at_ns).toBe(2n);

    const list = await captureRequest(socket, () => client.apiTokens.list());
    expect(list.req).toEqual({
      op: 'list_api_tokens',
      hmac: list.req.hmac,
    });
    expectSignature(list.req, {
      op: 'list_api_tokens',
    });
    pushReply(socket, list.rid, {
      kind: 'api_tokens_listed',
      tokens: [
        {
          id: tokenId,
          label: 'missing-revocation',
          owner: 'owner',
          mode: 'read_only',
          channels: 'rest',
          databases: null,
          created_at_ns: 3,
          expires_at_ns: 4,
        },
        {
          id: tokenId,
          label: 'null-revocation',
          owner: 'owner',
          mode: 'read_only',
          channels: 'rest',
          databases: null,
          created_at_ns: 5,
          expires_at_ns: 6,
          revoked_at_ns: null,
        },
        {
          id: tokenId,
          label: 'wide-revocation',
          owner: 'owner',
          mode: 'read_write',
          channels: 'both',
          databases: ['db'],
          created_at_ns: 7,
          expires_at_ns: 8,
          revoked_at_ns: 9_007_199_254_740_993n,
        },
      ],
    });
    const rows = await list.result;
    expect(rows.map(row => row.created_at_ns)).toEqual([3n, 5n, 7n]);
    expect(rows.map(row => row.expires_at_ns)).toEqual([4n, 6n, 8n]);
    expect(rows.map(row => row.revoked_at_ns)).toEqual([null, null, 9_007_199_254_740_993n]);

    const revokeUser = await captureRequest(socket, () => client.apiTokens.revokeUser());
    expectSignature(revokeUser.req, {
      op: 'revoke_user_tokens',
    });
    pushReply(socket, revokeUser.rid, {
      kind: 'api_user_tokens_revoked',
      count: 9_007_199_254_740_993n,
    });
    expect(await revokeUser.result).toBe(9_007_199_254_740_993n);
  });

  it('preserves delegated getDdlOpStatus error and null behavior', async () => {
    const { client, socket } = await makeClient();
    const failed = await captureRequest(socket, () => client.getDdlOpStatus('db', 'main', 't', 'op'));
    const failure = rejection(failed.result);
    pushReply(socket, failed.rid, {
      kind: 'error',
      code: 'not_supported',
      message: 'unsupported',
    });
    const failureError = await failure;
    expect(failureError).toBeInstanceOf(ShamirDbError);
    expect(failureError.message).toBe('db error [not_supported]: unsupported');

    const status = await captureRequest(socket, () => client.getDdlOpStatus('db', 'main', 't', 'op'));
    pushReply(socket, status.rid, {
      kind: 'ddl_op_status',
      status: null,
    });
    expect(await status.result).toBeNull();

    const wrongKind = await captureRequest(socket, () => client.getDdlOpStatus('db', 'main', 't', 'op'));
    const wrongKindResult = rejection(wrongKind.result);
    pushReply(socket, wrongKind.rid, {
      kind: 'wrong',
    });
    expect((await wrongKindResult).message).toBe('unexpected DbResponse kind for getDdlOpStatus: wrong');
  });
});
