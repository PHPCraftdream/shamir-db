import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Batch, Query, TokenTransportError, connect, filter, write } from '../../../index.js';
import type { ShamirClient } from '../../../index.js';
import { setupTokenFixture, SERVER_AVAILABLE, SERVICE_USER } from './support/harness.js';
import type { TokenServer } from './support/harness.js';

describe.skipIf(!SERVER_AVAILABLE)('token channel e2e', () => {
  let server: TokenServer | null = null;
  let scram: ShamirClient | null = null;
  let admin: ShamirClient | null = null;
  const db = 'default';

  beforeAll(async () => {
    const fixture = await setupTokenFixture();
    ({ server, scram, admin } = fixture);
  }, 60000);

  afterAll(async () => {
    if (admin) await admin.close().catch(() => undefined);
    if (scram) await scram.close().catch(() => undefined);
    if (server) await server.stop();
  }, 15000);

  it('writes and reads distinct data over HTTP(S) and WS(S)', async () => {
    const token = (await scram!.apiTokens.create({
      label: `matrix-${process.pid}`,
      for_user: SERVICE_USER,
      mode: 'read_write',
      channels: 'both',
      ttl_secs: 3600,
    })).token;
    const transports = [
      { url: `http://127.0.0.1:${server!.http}`, channel: 'http' as const },
      {
        url: `https://127.0.0.1:${server!.https}`,
        channel: 'http' as const,
        tls: { allowInsecureTlsForDevelopment: true as const },
      },
      { url: `ws://127.0.0.1:${server!.http}`, channel: 'ws' as const },
      {
        url: `wss://127.0.0.1:${server!.https}`,
        channel: 'ws' as const,
        tls: { allowInsecureTlsForDevelopment: true as const },
      },
    ];

    for (const [index, entry] of transports.entries()) {
      const id = `matrix-${process.pid}-${index}`;
      const client = await connect({
        auth: 'token', token, ...entry,
        connectTimeoutMs: 10000, requestTimeoutMs: 10000,
      });
      try {
        await expect(client.ping()).resolves.toEqual({ kind: 'pong' });
        const writeBatch = Batch.create(`write-${id}`)
          .add('put', write.upsert(
            'items',
            {
              id,
            },
            {
              id,
              value: `saved-${id}`,
            },
          ))
          .build();
        await client.execute(db, writeBatch);
        const readBatch = Batch.create(`read-${id}`)
          .add('rows', Query.from('items').where(filter.eq('id', id)))
          .build();
        const read = await client.execute(db, readBatch);
        expect(read.results.rows!.records).toContainEqual({
          id,
          value: `saved-${id}`,
        });

        if (client.channel === 'ws') {
          const txId = `tx-${id}`;
          const transaction = await client.db(db).tx(
            async (tx) => {
              await tx.run(write.insert('items', [{
                id: txId,
                value: `committed-${id}`,
              }]));
              return txId;
            },
            { visibilityWait: { mode: 'strict' } },
          );
          expect(transaction.status).toBe('committed');
          expect(transaction.state).toBe('visible');
          const verifyTx = Batch.create(`verify-${txId}`)
            .add('row', Query.from('items').where(filter.eq('id', txId)))
            .build();
          const txRead = await client.execute(db, verifyTx);
          expect(txRead.results.row!.records).toContainEqual({
            id: txId,
            value: `committed-${id}`,
          });

          const cursor = client.db(db).cursor(Query.from('items'), 1);
          try {
            const firstPage = await cursor.next();
            expect(firstPage.done).toBe(false);
            if (!firstPage.done) expect(firstPage.value.id).toBeDefined();
            const secondPage = await cursor.next();
            expect(secondPage.done).toBe(false);
            if (!secondPage.done) {
              expect(secondPage.value.id).toBeDefined();
              if (!firstPage.done) expect(secondPage.value.id).not.toBe(firstPage.value.id);
            }
          } finally {
            await cursor.close();
          }
          const cursorAfterClose = client.db(db).cursor(Query.from('items'), 1);
          try {
            const afterClosePage = await cursorAfterClose.next();
            expect(afterClosePage.done).toBe(false);
            if (!afterClosePage.done) expect(afterClosePage.value.id).toBeDefined();
          } finally {
            await cursorAfterClose.close();
          }
        }
      } finally {
        await client.close();
      }
    }
  }, 60000);

  it('enforces read-only mode on every channel and transport', async () => {
    const token = (await scram!.apiTokens.create({
      label: `ro-${process.pid}`, for_user: SERVICE_USER,
      mode: 'read_only', channels: 'both', ttl_secs: 3600,
    })).token;
    for (const [index, entry] of [
      { url: `http://127.0.0.1:${server!.http}`, channel: 'http' as const },
      {
        url: `https://127.0.0.1:${server!.https}`,
        channel: 'http' as const,
        tls: { allowInsecureTlsForDevelopment: true as const },
      },
      { url: `ws://127.0.0.1:${server!.http}`, channel: 'ws' as const },
      {
        url: `wss://127.0.0.1:${server!.https}`,
        channel: 'ws' as const,
        tls: { allowInsecureTlsForDevelopment: true as const },
      },
    ].entries()) {
      const client = await connect({
        auth: 'token', token, ...entry,
        connectTimeoutMs: 10000, requestTimeoutMs: 10000,
      });
      try {
        const batch = Batch.create(`readonly-${index}-${process.pid}`)
          .add('put', write.insert('items', [{
            id: `denied-${index}-${process.pid}`,
            value: 'no',
          }]))
          .build();
        const writeAttempt = client.execute(db, batch);
        await expect(writeAttempt).rejects.toBeInstanceOf(TokenTransportError);
        await expect(writeAttempt).rejects.toMatchObject({ code: 'token_read_only' });
      } finally {
        await client.close();
      }
    }
  }, 60000);

  it('rejects tokens on channels outside their scope', async () => {
    const restToken = (await scram!.apiTokens.create({
      label: `rest-${process.pid}`, for_user: SERVICE_USER,
      channels: 'rest', ttl_secs: 3600,
    })).token;
    const restOnWs = connect({
      auth: 'token', token: restToken,
      url: `ws://127.0.0.1:${server!.http}`, channel: 'ws',
      requestTimeoutMs: 10000,
    });
    await expect(restOnWs).rejects.toBeInstanceOf(TokenTransportError);
    await expect(restOnWs).rejects.toMatchObject({ code: 'permission_denied', status: 403 });

    const wsToken = (await scram!.apiTokens.create({
      label: `ws-only-${process.pid}`, for_user: SERVICE_USER,
      channels: 'ws', ttl_secs: 3600,
    })).token;
    const httpClient = await connect({
      auth: 'token', token: wsToken,
      url: `http://127.0.0.1:${server!.http}`, requestTimeoutMs: 10000,
    });
    try {
      const ping = httpClient.ping();
      await expect(ping).rejects.toBeInstanceOf(TokenTransportError);
      await expect(ping).rejects.toMatchObject({ code: 'token_channel_denied', status: 403 });
    } finally {
      await httpClient.close();
    }
  }, 30000);
});
