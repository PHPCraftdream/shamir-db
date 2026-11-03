import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import {
  ApiToken,
  Batch,
  Query,
  TokenTransportError,
  connect,
  write,
} from '../../../index.js';
import type { NodeTokenConnectOptions, ShamirClient } from '../../../index.js';
import { tokenValueForTransport } from '../../../entry/api-token.js';
import { setupTokenFixture, SERVER_AVAILABLE, SERVICE_USER } from './support/harness.js';
import type { TokenServer } from './support/harness.js';

async function expectTokenFailure(
  promise: Promise<unknown>,
  code: string,
  status?: number,
): Promise<void> {
  let failure: unknown;
  try {
    await promise;
  } catch (error) {
    failure = error;
  }
  expect(failure).toBeInstanceOf(TokenTransportError);
  expect(failure).toMatchObject({
    code,
    ...(status === undefined ? {} : { status }),
  });
}

describe.skipIf(!SERVER_AVAILABLE)('token transport error e2e', () => {
  let server: TokenServer | null = null;
  let scram: ShamirClient | null = null;
  let admin: ShamirClient | null = null;
  let good: Awaited<ReturnType<ShamirClient['apiTokens']['create']>>['token'];

  beforeAll(async () => {
    const fixture = await setupTokenFixture();
    ({ server, scram, admin } = fixture);
    good = (await scram!.apiTokens.create({
      label: `errors-${process.pid}`,
      for_user: SERVICE_USER,
      mode: 'read_write',
      channels: 'both',
      ttl_secs: 3600,
    })).token;
  }, 60000);

  afterAll(async () => {
    if (admin) await admin.close().catch(() => undefined);
    if (scram) await scram.close().catch(() => undefined);
    if (server) await server.stop();
  }, 15000);

  it('maps malformed, unknown and incorrect credentials to typed errors', async () => {
    const malformed = connect({
      auth: 'token',
      token: 'malformed',
      url: `http://127.0.0.1:${server!.http}`,
      requestTimeoutMs: 10000,
    });
    await expect(malformed).rejects.toBeInstanceOf(TokenTransportError);
    await expect(malformed).rejects.toMatchObject({ code: 'invalid_request' });
    expect(() => ApiToken.from('malformed')).toThrow(TypeError);

    const unknownToken =
      'shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
    const unknownClient = await connect({
      auth: 'token',
      token: unknownToken,
      url: `http://127.0.0.1:${server!.http}`,
      requestTimeoutMs: 10000,
    });
    try {
      await expectTokenFailure(unknownClient.ping(), 'unauthorized', 401);
    } finally {
      await unknownClient.close();
    }

    const rawToken = tokenValueForTransport(good);
    const wrongToken = `${rawToken.slice(0, 28)}${rawToken[28] === 'A' ? 'B' : 'A'}${rawToken.slice(29)}`;
    const wrongClient = await connect({
      auth: 'token',
      token: wrongToken,
      url: `http://127.0.0.1:${server!.http}`,
      requestTimeoutMs: 10000,
    });
    try {
      await expectTokenFailure(wrongClient.ping(), 'unauthorized', 401);
    } finally {
      await wrongClient.close();
    }

    const invalidOptions = {
      auth: 'token',
      token: good,
      url: `http://127.0.0.1:${server!.http}`,
      channel: 'unknown',
    } as unknown as NodeTokenConnectOptions;
    await expectTokenFailure(connect(invalidOptions), 'invalid_request');
  });

  it('enforces request size and synchronous REST tx/cursor capability errors', async () => {
    const client = await connect({
      auth: 'token',
      token: good,
      url: `http://127.0.0.1:${server!.http}`,
      requestTimeoutMs: 10000,
    });
    try {
      const oversizedBatch = Batch.create(`large-${process.pid}`)
        .add('put', write.insert('items', [{
          id: 'large',
          value: 'x'.repeat(4096),
        }]))
        .build();
      const oversizedRequest = client.execute('default', oversizedBatch);
      await expect(oversizedRequest).rejects.toBeInstanceOf(TokenTransportError);
      await expect(oversizedRequest).rejects.toMatchObject({
        code: 'http_body_too_large',
        status: 413,
      });

      await expectTokenFailure(
        client.db('default').tx(async () => undefined),
        'not_supported_over_rest',
      );
      let cursorFailure: unknown;
      try {
        client.db('default').cursor(Query.from('items'), 1);
      } catch (error) {
        cursorFailure = error;
      }
      expect(cursorFailure).toBeInstanceOf(TokenTransportError);
      expect(cursorFailure).toMatchObject({ code: 'not_supported_over_rest' });
    } finally {
      await client.close();
    }
  });
});
