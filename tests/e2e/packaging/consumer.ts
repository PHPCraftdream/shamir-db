/**
 * Type-level consumer check for the published `shamir-client` package.
 *
 * Compiled with `tsc --noEmit` in CI (`node-binding` job): exercises the
 * package's `.d.ts` (`crates/shamir-client-node/js/wrapper.d.ts`) exactly as
 * an external consumer would see it. This file is never executed.
 */

import { ShamirClient, ShamirDbError, isRetryableCode } from 'shamir-client';

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

void check();
