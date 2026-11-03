import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import { join } from 'node:path';
import { Batch, LoginError, Query, TokenTransportError, connect, connectLocal, login, subscribe, write } from '../../../index.js';
import type { LoginOptions, ShamirClient, TokenWsClient } from '../../../index.js';
import { decode } from '../../../core/protocol/framing.js';
import { setupTokenFixture, SERVER_AVAILABLE, HOST, SERVICE_PASSWORD, SERVICE_USER, ORIGIN } from './support/harness.js';
import type { TokenServer } from './support/harness.js';
import { installBrowserSocket } from './support/browser-socket.js';
import { connect as browserConnect, login as browserLogin } from '../../../browser.js';
import { installBrowserFetch, preloadBrowserCrypto } from './support/browser-fetch.js';

function withDeadline<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error('Operation deadline exceeded')), timeoutMs);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

describe.skipIf(!SERVER_AVAILABLE)('HTTP password token login e2e', () => {
  let server: TokenServer | null = null;
  let scram: ShamirClient | null = null;
  let admin: ShamirClient | null = null;
  beforeAll(async () => { const fixture = await setupTokenFixture(); ({ server, scram, admin } = fixture); }, 60000);
  afterAll(async () => { if (admin) await admin.close().catch(() => undefined); if (scram) await scram.close().catch(() => undefined); if (server) await server.stop(); }, 15000);
  const endpoint = () => `http://${HOST}:${server!.http}`;
  const loginOptions = (label: string, url = endpoint(), extra: Partial<LoginOptions> = {}): LoginOptions => ({ username: SERVICE_USER, password: SERVICE_PASSWORD, url, label, timeoutMs: 30000, connectTimeoutMs: 10000, requestTimeoutMs: 10000, ...extra });

  it('returns a working token client', async () => {
    const result = await login(loginOptions(`login-${process.pid}`, endpoint(), { mode: 'read_only', channels: 'rest' }));
    const client = await result.connect({ channel: 'http', requestTimeoutMs: 10000 });
    try { await client.ping(); } finally { await client.close(); }
  }, 60000);

  it('logs in over HTTP and verified HTTPS and uses batch, WS transaction, and subscription', async () => {
    const fixture = await setupTokenFixture({ loginAllowReadWrite: true });
    try {
      const ca = fs.readFileSync(join(fixture.server.dir, 'cert.pem'), 'utf8');
      const origins = [`http://${HOST}:${fixture.server.http}`, `https://127.0.0.1:${fixture.server.https}`] as const;
      for (const [index, url] of origins.entries()) {
        const result = await login(loginOptions(`login-matrix-${process.pid}-${index}`, url, { ...(index === 1 ? { ca } : {}), mode: 'read_write', channels: 'both' }));
        const http = await result.connect({ channel: 'http', requestTimeoutMs: 10000 });
        const id = `login-data-${process.pid}-${index}`;
        try {
          await http.execute('default', Batch.create(`login-write-${index}`).add('put', write.upsert('items', { id }, { id, value: 'saved' })).transactional().build());
          const read = await http.execute('default', Batch.create(`login-read-${index}`).add('row', Query.from('items').whereEq('id', id)).build());
          expect(read.results.row!.records).toContainEqual({ id, value: 'saved' });
          await expect(http.db('default').tx(async () => undefined)).rejects.toMatchObject({ code: 'not_supported_over_rest' });
        } finally { await http.close(); }
        const ws = await result.connect({ channel: 'ws', requestTimeoutMs: 10000 });
        try {
          const txId = `login-tx-${process.pid}-${index}`;
          const transaction = await ws.db('default').tx(async (tx) => { await tx.run(write.insert('items', [{ id: txId, value: 'committed' }])); return txId; });
          expect(transaction.status).toBe('committed');
          const txRead = await ws.execute('default', Batch.create(`login-tx-read-${index}`).add('row', Query.from('items').whereEq('id', txId)).build());
          expect(txRead.results.row!.records).toContainEqual({ id: txId, value: 'committed' });
          const live = await ws.db('default').runLive(Batch.create(`login-sub-${index}`).add('sub', subscribe({ store: 'main', table: 'items' })));
          try {
            const pending = live.subs.sub!.next();
            const pushId = `push-${index}-${process.pid}`;
            const event = await (async (): Promise<Awaited<typeof pending>> => {
              let timer: ReturnType<typeof setTimeout> | undefined;
              try {
                await ws.execute('default', Batch.create(`login-push-${index}`).add('put', write.insert('items', [{ id: pushId, value: 'push' }])).build());
                return await Promise.race([pending, new Promise<undefined>((resolve) => { timer = setTimeout(() => resolve(undefined), 10000); })]);
              } finally { if (timer !== undefined) clearTimeout(timer); }
            })();
            expect(event).toBeDefined();
            expect(event!.kind).toBe('event');
            expect(event!.data).toBeInstanceOf(Uint8Array);
            const payload = decode(event!.data!);
            expect(payload).toMatchObject({
              table: 'items',
              op: 'put',
              value: { id: pushId, value: 'push' },
            });
          } finally { await live.subs.sub!.unsubscribe().catch(() => undefined); }
        } finally { await ws.close(); }
      }
    } finally { await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 120000);

  it('uses browser login fetch transport then browser token first-frame WebSocket', async () => {
    const fixture = await setupTokenFixture();
    const observer = installBrowserSocket(ORIGIN);
    let fetchTransport: ReturnType<typeof installBrowserFetch> | undefined;
    let client: TokenWsClient | undefined;
    const requests: string[] = [];
    try {
      await withDeadline(preloadBrowserCrypto(), 15000);
      fetchTransport = installBrowserFetch((observation) => requests.push(new URL(observation.url).pathname), ORIGIN);
      const url = `http://${HOST}:${fixture.server.http}`;
      const result = await browserLogin(loginOptions(`login-browser-${process.pid}`, url, { channels: 'ws' }));
      client = await browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${fixture.server.http}`, token: result.token, requestTimeoutMs: 10000, connectTimeoutMs: 10000 });
      await client.ping();
      expect(requests).toEqual(['/shamir/v1/auth/begin', '/shamir/v1/auth/finish']);
      expect(observer.sockets[0]!.pathname).toBe('/shamir/v1/ws/browser');
      expect(observer.sockets[0]!.protocol).toBe('shamir-v1');
      expect(observer.sockets[0]!.firstFrame).toMatchObject({ binary: true, tokenOnly: true, tokenLength: 71, canonical: true, byteLength: 80 });
      expect(observer.sockets[0]!.hasAuthorization).toBe(false);
    } finally { if (client) await client.close().catch(() => undefined); fetchTransport?.restore(); observer.restore(); await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 60000);

  it('expires a login token at configured login TTL on the server', async () => {
    const fixture = await setupTokenFixture({ loginTtl: 4, loginAllowReadWrite: true });
    try {
      const url = `http://${HOST}:${fixture.server.http}`;
      const result = await login(loginOptions(`login-ttl-${process.pid}`, url, { mode: 'read_write', channels: 'both' }));
      const http = await connect({ auth: 'token', channel: 'http', url, token: result.token, requestTimeoutMs: 10000 });
      const observer = installBrowserSocket(ORIGIN);
      let ws: TokenWsClient | undefined;
      try {
        await http.ping();
        ws = await browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${fixture.server.http}`, token: result.token, requestTimeoutMs: 10000 });
        await ws.ping();
        expect(result.metadata.expiresAtNs - result.metadata.createdAtNs).toBe(4_000_000_000n);
        const closed = await observer.sockets[0]!.waitForClose(15000);
        expect(closed).toMatchObject({ code: 4012, reason: 'token_expired' });
        const deadline = Date.now() + 5000;
        let expired = false;
        while (Date.now() < deadline) {
          try { await http.ping(); }
          catch (error) {
            if (error instanceof TokenTransportError && error.code === 'unauthorized' && error.status === 401) { expired = true; break; }
            throw error;
          }
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        expect(expired).toBe(true);
      } finally { if (ws) await ws.close().catch(() => undefined); observer.restore(); await http.close(); }
    } finally { await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 60000);

  it('revokes a login-issued token and terminates its session', async () => {
    const fixture = await setupTokenFixture();
    const observer = installBrowserSocket(ORIGIN);
    let ws: TokenWsClient | undefined;
    try {
      const url = `http://${HOST}:${fixture.server.http}`;
      const result = await login(loginOptions(`login-revoke-${process.pid}`, url, { channels: 'both' }));
      ws = await browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${fixture.server.http}`, token: result.token, requestTimeoutMs: 10000 });
      await ws.ping();
      await fixture.scram.apiTokens.revoke(Buffer.from(result.metadata.id).toString('base64url'));
      expect(await observer.sockets[0]!.waitForClose(10000)).toMatchObject({ code: 4011, reason: 'token_revoked' });
      const http = await connect({ auth: 'token', channel: 'http', url, token: result.token, requestTimeoutMs: 10000 });
      try { await expect(http.ping()).rejects.toMatchObject({ code: 'unauthorized', status: 401 }); } finally { await http.close(); }
    } finally { if (ws) await ws.close().catch(() => undefined); observer.restore(); await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 60000);

  it('invalidates login-issued tokens after SCRAM password change', async () => {
    const fixture = await setupTokenFixture();
    try {
      const user = `login-password-${process.pid}`;
      const oldPassword = 'old-login-password';
      const newPassword = 'new-login-password';
      await fixture.scram.createScramUser(user, oldPassword, []);
      const url = `http://${HOST}:${fixture.server.http}`;
      const result = await login(loginOptions(`login-password-change-${process.pid}`, url, { username: user, password: oldPassword, channels: 'both' }));
      const observer = installBrowserSocket(ORIGIN);
      let ws: TokenWsClient | undefined;
      try {
        ws = await browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${fixture.server.http}`, token: result.token, requestTimeoutMs: 10000 });
        await ws.ping();
        const http = await connect({ auth: 'token', channel: 'http', url, token: result.token, requestTimeoutMs: 10000 });
        try { await http.ping(); } finally { await http.close(); }
        // changePassword builds its proof for the WebSocket/TLS (no exporter) binding, so use the SCRAM WS listener, not IPC.
        const userClient = await connect({ host: HOST, port: fixture.server.scramWs, username: user, password: oldPassword, tls: { rejectUnauthorized: false }, origin: ORIGIN });
        try { await userClient.changePassword(oldPassword, newPassword); } finally { await userClient.close(); }
        expect(await observer.sockets[0]!.waitForClose(10000)).toMatchObject({ code: 4011, reason: 'token_revoked' });
        const afterChange = await connect({ auth: 'token', channel: 'http', url, token: result.token, requestTimeoutMs: 10000 });
        try { await expect(afterChange.ping()).rejects.toMatchObject({ code: 'unauthorized', status: 401 }); } finally { await afterChange.close(); }
      } finally { if (ws) await ws.close().catch(() => undefined); observer.restore(); }
    } finally { await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 60000);

  it('enforces role eligibility, login read-only mode and channel restrictions', async () => {
    const fixture = await setupTokenFixture({ loginAllowReadWrite: true });
    try {
      const url = `http://${HOST}:${fixture.server.http}`;
      const superuser = `login-super-${process.pid}`;
      const replicator = `login-repl-${process.pid}`;
      await fixture.scram.createScramUser(superuser, 'role-password', []);
      await fixture.scram.setSuperuser(superuser, true);
      await fixture.scram.createScramUser(replicator, 'role-password', []);
      await fixture.scram.setReplicator(replicator, true);
      for (const username of [superuser, replicator]) {
        await expect(login(loginOptions(`login-ineligible-${username}`, url, { username, password: 'role-password' }))).rejects.toMatchObject({ code: 'login_account_denied', status: 403 });
      }
      const readOnly = await login(loginOptions(`login-ro-${process.pid}`, url, { mode: 'read_only', channels: 'both' }));
      for (const channel of ['http', 'ws'] as const) {
        const client = await readOnly.connect({ channel, requestTimeoutMs: 10000 });
        try { await expect(client.execute('default', Batch.create(`login-ro-write-${channel}`).add('write', write.insert('items', [{ id: `ro-${channel}-${process.pid}`, value: 'no' }])).build())).rejects.toMatchObject({ code: 'token_read_only' }); }
        finally { await client.close(); }
      }
    } finally { await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 120000);

  it('enforces login channel restrictions on http and ws', async () => {
    // Own server: the per-source login credit bucket (10 requests/min per /24) is shared by every login.
    const fixture = await setupTokenFixture({ loginAllowReadWrite: true });
    try {
      const url = `http://${HOST}:${fixture.server.http}`;
      const restOnly = await login(loginOptions(`login-channel-${process.pid}`, url, { channels: 'rest' }));
      const restOnWs = connect({
        auth: 'token',
        channel: 'ws',
        url: `ws://${HOST}:${fixture.server.http}`,
        token: restOnly.token,
        connectTimeoutMs: 10000,
        requestTimeoutMs: 10000,
      });
      await expect(restOnWs).rejects.toMatchObject({ code: 'permission_denied', status: 403 });
      const observer = installBrowserSocket(ORIGIN);
      try {
        await expect(browserConnect({
          auth: 'token',
          channel: 'ws',
          url: `ws://127.0.0.1:${fixture.server.http}`,
          token: restOnly.token,
          requestTimeoutMs: 10000,
          connectTimeoutMs: 10000,
        })).rejects.toMatchObject({ code: 'authentication_failed' });
        expect(await observer.sockets[0]!.waitForClose(5000)).toMatchObject({ code: 4013, reason: 'token_channel_denied' });
      } finally { observer.restore(); }
      const wsOnly = await login(loginOptions(`login-channel-ws-${process.pid}`, url, { channels: 'ws' }));
      const httpClient = await connect({ auth: 'token', channel: 'http', url, token: wsOnly.token, requestTimeoutMs: 10000 });
      try { await expect(httpClient.ping()).rejects.toMatchObject({ code: 'token_channel_denied', status: 403 }); }
      finally { await httpClient.close(); }
    } finally { await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 120000);

  it('rejects disabled login routes and a wrong Host authority', async () => {
    const disabled = await setupTokenFixture({ loginEnabled: false });
    try { await expect(login(loginOptions(`login-disabled-${process.pid}`, `http://${HOST}:${disabled.server.http}`))).rejects.toMatchObject({ code: 'unsupported_server', status: 404 }); }
    finally { await disabled.admin.close().catch(() => undefined); await disabled.scram.close().catch(() => undefined); await disabled.server.stop(); }
    const authority = await setupTokenFixture({ loginAuthorityAllowlist: ['only.example.invalid'] });
    try { await expect(login(loginOptions(`login-authority-${process.pid}`, `http://127.0.0.1:${authority.server.http}`))).rejects.toMatchObject({ code: 'authority_forbidden', status: 403 }); }
    finally { await authority.admin.close().catch(() => undefined); await authority.scram.close().catch(() => undefined); await authority.server.stop(); }
  }, 60000);

  it('rate-limits a burst of failing login attempts from one source (last, isolated server)', async () => {
    const fixture = await setupTokenFixture();
    const user = `dedicated-login-user-${process.pid}`;
    try {
      await fixture.scram.createScramUser(user, 'dedicated-login-password', []);
      const before = await fixture.scram.apiTokens.list({ for_user: user });
      const rejected = login(loginOptions(`login-wrong-${process.pid}`, `http://${HOST}:${fixture.server.http}`, { username: user, password: 'wrong password' }));
      await expect(rejected).rejects.toBeInstanceOf(LoginError);
      await expect(rejected).rejects.toMatchObject({ code: 'authentication_failed', status: 401 });
      expect(await fixture.scram.apiTokens.list({ for_user: user })).toEqual(before);
      const url = `http://${HOST}:${fixture.server.http}`;
      const deadline = Date.now() + 30000;
      let lastError: unknown;
      for (let attempt = 0; attempt < 5 && Date.now() < deadline; attempt += 1) {
        try { await login(loginOptions(`login-limit-${process.pid}-${attempt}`, url, { username: user, password: 'wrong password' })); throw new Error('Incorrect password unexpectedly authenticated'); }
        catch (error) { if (error instanceof LoginError && error.code === 'rate_limited') { lastError = error; break; } if (!(error instanceof LoginError) || error.code !== 'authentication_failed') throw error; lastError = error; }
      }
      expect(lastError).toBeInstanceOf(LoginError);
      expect(lastError).toMatchObject({ code: 'rate_limited', status: 429 });
      expect((lastError as LoginError).retryAfterMs).toBeGreaterThan(0);
    } finally { await fixture.admin.close().catch(() => undefined); await fixture.scram.close().catch(() => undefined); await fixture.server.stop(); }
  }, 60000);
});
