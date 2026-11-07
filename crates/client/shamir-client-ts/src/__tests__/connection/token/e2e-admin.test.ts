import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ApiToken, TokenTransportError, connect } from '../../../index.js';
import type { ShamirClient } from '../../../index.js';
import { setupTokenFixture, SERVER_AVAILABLE, SERVICE_USER } from './support/harness.js';
import type { TokenServer } from './support/harness.js';

describe.skipIf(!SERVER_AVAILABLE)('token admin e2e', () => {
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

  it('creates a usable token, redacts listings, and revokes individual and owner tokens', async () => {
    const first = await scram!.apiTokens.create({
      label: `admin-${process.pid}`,
      for_user: SERVICE_USER,
      mode: 'read_write',
      channels: 'both',
      ttl_secs: 3600,
    });
    expect(first.token).toBeInstanceOf(ApiToken);
    expect(String(first.token)).toBe('[REDACTED]');
    expect(first.id).toHaveLength(22);
    expect(first.created_at_ns).toBeLessThan(first.expires_at_ns);

    const firstClient = await connect({
      auth: 'token',
      token: first.token,
      url: `http://127.0.0.1:${server!.http}`,
      requestTimeoutMs: 10000,
    });
    try {
      await expect(firstClient.ping()).resolves.toEqual({ kind: 'pong' });
    } finally {
      await firstClient.close();
    }

    const listed = await scram!.apiTokens.list({ for_user: SERVICE_USER });
    const listedFirst = listed.find((row) => row.id === first.id);
    expect(listedFirst).toBeDefined();
    expect(listedFirst).toHaveProperty('label', first.label);
    for (const row of listed) {
      expect(row).not.toHaveProperty('token');
      expect(row).not.toHaveProperty('secret');
      expect(row).not.toHaveProperty('mac');
      expect(row).not.toHaveProperty('hash');
    }

    await scram!.apiTokens.revoke(first.id);
    const afterRevoke = await connect({
      auth: 'token',
      token: first.token,
      url: `http://127.0.0.1:${server!.http}`,
      requestTimeoutMs: 10000,
    });
    try {
      const revokedPing = afterRevoke.ping();
      await expect(revokedPing).rejects.toBeInstanceOf(TokenTransportError);
      await expect(revokedPing).rejects.toMatchObject({ code: 'unauthorized', status: 401 });
    } finally {
      await afterRevoke.close();
    }

    const second = await scram!.apiTokens.create({
      label: `admin-second-${process.pid}`,
      for_user: SERVICE_USER,
      ttl_secs: 3600,
    });
    const third = await scram!.apiTokens.create({
      label: `admin-third-${process.pid}`,
      for_user: SERVICE_USER,
      ttl_secs: 3600,
    });
    expect(await scram!.apiTokens.revokeUser({ user: SERVICE_USER })).toBe(2n);

    for (const revoked of [second, third]) {
      // Listener failure limiter: 2 auth failures per second per subnet during the 60 s warm-up.
      await new Promise((resolve) => setTimeout(resolve, 1100));
      const revokedClient = await connect({
        auth: 'token',
        token: revoked.token,
        url: `http://127.0.0.1:${server!.http}`,
        requestTimeoutMs: 10000,
      });
      try {
        const revokedPing = revokedClient.ping();
        await expect(revokedPing).rejects.toBeInstanceOf(TokenTransportError);
        await expect(revokedPing).rejects.toMatchObject({ code: 'unauthorized', status: 401 });
      } finally {
        await revokedClient.close();
      }
    }

    const finalList = await scram!.apiTokens.list({ for_user: SERVICE_USER });
    const revokedCount = finalList.filter(
      (row) => row.revoked_at_ns !== null && row.revoked_at_ns !== undefined,
    ).length;
    expect(revokedCount).toBeGreaterThanOrEqual(3);
  });
});
