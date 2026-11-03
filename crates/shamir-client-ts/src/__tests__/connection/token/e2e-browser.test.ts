import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import { join } from 'node:path';
import { TokenTransportError } from '../../../index.js';
import { connect as browserConnect } from '../../../browser.js';
import type { ShamirClient, TokenWsClient } from '../../../index.js';
import { setupTokenFixture, SERVER_AVAILABLE, ORIGIN } from './support/harness.js';
import type { TokenServer } from './support/harness.js';
import { installBrowserSocket } from './support/browser-socket.js';

describe.skipIf(!SERVER_AVAILABLE)('browser token socket e2e', () => {
  let server: TokenServer | null = null;
  let scram: ShamirClient | null = null;
  let admin: ShamirClient | null = null;

  beforeAll(async () => {
    const fixture = await setupTokenFixture();
    ({ server, scram, admin } = fixture);
  }, 60000);

  afterAll(async () => {
    if (admin) await admin.close().catch(() => undefined);
    if (scram) await scram.close().catch(() => undefined);
    if (server) await server.stop();
  }, 15000);

  it.each([
    { scheme: 'ws' as const, port: 'http' as const },
    { scheme: 'wss' as const, port: 'https' as const },
  ])('sends the token in a real binary $scheme browser first frame with no Authorization header', async ({ scheme, port }) => {
    const token = (await scram!.apiTokens.create({
      label: `browser-${scheme}-${process.pid}`,
      for_user: `token_svc_${process.pid}`,
      mode: 'read_only',
      channels: 'ws',
      ttl_secs: 3600,
    })).token;
    const ca = scheme === 'wss' ? fs.readFileSync(join(server!.dir, 'cert.pem'), 'utf8') : undefined;
    const observer = installBrowserSocket(ORIGIN, ca);
    let client: TokenWsClient | undefined;
    try {
      const portNumber = server![port];
      client = await browserConnect({
        auth: 'token',
        channel: 'ws',
        url: `${scheme}://127.0.0.1:${portNumber}` as `ws://${string}` | `wss://${string}`,
        token,
        connectTimeoutMs: 10000,
        requestTimeoutMs: 10000,
      });
      await client.ping();
      const socket = observer.sockets[0];
      expect(socket).toBeDefined();
      expect(socket!.pathname).toBe('/shamir/v1/ws/browser');
      expect(socket!.protocol).toBe('shamir-v1');
      expect(socket!.hasAuthorization).toBe(false);
      expect(socket!.firstFrame).toEqual({
        binary: true,
        tokenOnly: true,
        tokenLength: 71,
        canonical: true,
        byteLength: 80,
      });
    } finally {
      if (client) await client.close().catch(() => undefined);
      observer.restore();
    }
  });

  it('records the actual origin rejection response', async () => {
    const token = (await scram!.apiTokens.create({
      label: `origin-${process.pid}`,
      for_user: `token_svc_${process.pid}`,
      mode: 'read_only',
      channels: 'ws',
      ttl_secs: 3600,
    })).token;
    const observer = installBrowserSocket('https://unlisted.invalid');
    try {
      const rejected = browserConnect({
        auth: 'token',
        channel: 'ws',
        url: `ws://127.0.0.1:${server!.http}`,
        token,
        connectTimeoutMs: 5000,
        requestTimeoutMs: 5000,
      });
      await expect(rejected).rejects.toBeInstanceOf(TokenTransportError);
      await expect(rejected).rejects.toMatchObject({ code: 'authentication_failed' });
      const socket = observer.sockets[0];
      expect(socket).toBeDefined();
      expect(socket!.unexpectedStatus).toBe(403);
      expect(socket!.hasAuthorization).toBe(false);
      const closed = await socket!.waitForClose(5000);
      expect(closed).toMatchObject({ code: 1006 });
    } finally {
      observer.restore();
    }
  });
});
