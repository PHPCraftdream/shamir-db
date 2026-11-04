/**
 * Type-level consumer check for the published `shamir-client` package.
 *
 * Compiled with `tsc --noEmit` in CI (`node-binding` job): exercises the
 * package's `.d.ts` (`crates/shamir-client-node/js/wrapper.d.ts`) exactly as
 * an external consumer would see it. This file is never executed.
 */

import {
  ShamirClient,
  ShamirDbError,
  ShamirTokenHttpClient,
  isRetryableCode,
  login,
} from 'shamir-client';

async function check(): Promise<void> {
  // Class construction + static factory.
  const client: ShamirClient = await ShamirClient.connect({
    host: '127.0.0.1',
    port: 1,
    serverName: 'localhost',
    username: 'u',
    password: 'p',
    acceptNewHost: true,
  });

  // Instance methods from the wrapper's class declaration.
  const pin: Buffer = client.serverPubKeyPin();
  const sid: Buffer = client.sessionId();
  const expiry: bigint = client.expiresAtNs();
  const ticket: Buffer | null = client.resumptionTicket();
  // Type-only: the batch is never built or sent from this file.
  const batch: Parameters<ShamirClient['execute']>[1] = {};
  await client.execute('default', batch);
  await client.ping();
  await client.close();

  // Typed error surface.
  try {
    await client.execute('nope', batch);
  } catch (e) {
    if (e instanceof ShamirDbError) {
      const code: string = e.code;
      const detail: string = e.detail;
      const retryable: boolean = e.retryable;
      console.log(code, detail, retryable, pin.length, sid.length, expiry, ticket !== null);
    }
  }

  // Helper export.
  const retry: boolean = isRetryableCode('timeout');
  console.log(retry);
}

// Connection-matrix API surface: type-level only, never called.
export async function checkMatrix(): Promise<void> {
  const ws: ShamirClient = await ShamirClient.connectWs({
    host: '127.0.0.1',
    port: 1,
    serverName: 'localhost',
    username: 'u',
    password: 'p',
    acceptNewHost: true,
  });
  const plain: ShamirClient = await ShamirClient.connectPlain({
    host: '127.0.0.1',
    port: 1,
    username: 'u',
    password: 'p',
  });
  const local: ShamirClient = await ShamirClient.connectLocal({
    addr: 'shamir.sock',
    username: 'u',
    password: 'p',
  });

  const ticket = Buffer.alloc(16);
  const trustedPin = Buffer.alloc(32);
  const resumed: ShamirClient = await ShamirClient.resume({
    host: '127.0.0.1',
    port: 1,
    serverName: 'localhost',
    ticket,
    trustedPin,
  });
  const resumedWs: ShamirClient = await ShamirClient.resumeWs({
    host: '127.0.0.1',
    port: 1,
    serverName: 'localhost',
    ticket,
    trustedPin,
  });

  const kind: 'scram' | 'token' | 'unknown' = ws.authKind();
  const created = await ws.createApiToken({
    label: 'l',
    ttlSecs: 60,
    mode: 'read_only',
    channels: 'both',
  });
  const secret: string = created.token;

  const viaWs: ShamirClient = await ShamirClient.connectTokenWs({
    host: '127.0.0.1',
    port: 1,
    plainLoopback: true,
    token: secret,
  });

  const http: ShamirTokenHttpClient = await ShamirTokenHttpClient.connect({
    host: '127.0.0.1',
    port: 1,
    serverName: 'localhost',
    caPem: '',
    token: secret,
  });
  await http.ping();
  await http.close();

  const issued = await login({
    host: '127.0.0.1',
    port: 1,
    plainLoopback: true,
    username: 'u',
    password: 'p',
  });
  const issuedId: Buffer = issued.id;
  console.log(plain, local, resumed, resumedWs, kind, viaWs, issuedId.length, issued.token);
}

void check();
