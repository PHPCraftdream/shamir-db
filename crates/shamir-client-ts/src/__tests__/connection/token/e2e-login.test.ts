import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { LoginError, login } from '../../../index.js';
import type { ShamirClient } from '../../../index.js';
import { setupTokenFixture, SERVER_AVAILABLE, HOST, SERVICE_PASSWORD, SERVICE_USER } from './support/harness.js';
import type { TokenServer } from './support/harness.js';

describe.skipIf(!SERVER_AVAILABLE)('HTTP password token login e2e', () => {
  let server: TokenServer | null = null; let scram: ShamirClient | null = null; let admin: ShamirClient | null = null;
  beforeAll(async () => { const f = await setupTokenFixture(); ({ server, scram, admin } = f); }, 60000);
  afterAll(async () => { if (admin) await admin.close().catch(() => undefined); if (scram) await scram.close().catch(() => undefined); if (server) await server.stop(); }, 15000);
  const endpoint = () => `http://${HOST}:${server!.http}`;
  it('returns a working token client and types password failure', async () => {
    const result = await login({ username: SERVICE_USER, password: SERVICE_PASSWORD, url: endpoint(), scope: { label: `login-${process.pid}`, mode: 'read_only', channels: 'rest' }, requestTimeoutMs: 10000 });
    const connected = await result.connect({ channel: 'http', requestTimeoutMs: 10000 });
    try { await connected.ping(); } finally { await connected.close(); }
    const wrongPassword = login({ username: SERVICE_USER, password: 'wrong password', url: endpoint(), scope: { label: `login-wrong-${process.pid}`, channels: 'rest' }, requestTimeoutMs: 10000 });
    await expect(wrongPassword).rejects.toBeInstanceOf(LoginError);
    await expect(wrongPassword).rejects.toMatchObject({ code: 'authentication_failed', status: 401 });
  });
  it('exercises backoff for repeated failures on the same account last', async () => {
    const deadline = Date.now() + 30000;
    let lastError: unknown;
    for (let attempt = 0; attempt < 32 && Date.now() < deadline; attempt += 1) {
      try {
        await login({ username: SERVICE_USER, password: 'wrong password', url: endpoint(), scope: { label: `login-limit-${process.pid}-${attempt}`, channels: 'rest' }, requestTimeoutMs: 10000 });
        throw new Error('Incorrect password unexpectedly authenticated');
      } catch (error) {
        if (error instanceof LoginError && error.code === 'rate_limited') { lastError = error; break; }
        if (!(error instanceof LoginError) || error.code !== 'authentication_failed') throw error;
        lastError = error;
      }
    }
    expect(lastError).toBeInstanceOf(LoginError);
    expect(lastError).toMatchObject({ code: 'rate_limited', status: 429 });
    expect((lastError as LoginError).retryAfterMs).toBeGreaterThan(0);
  }, 60000);
});
