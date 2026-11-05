import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { TokenTransportError, connect } from '../../../index.js';
import { connect as browserConnect } from '../../../browser.js';
import type { TokenWsClient } from '../../../index.js';
import type { ShamirClient } from '../../../index.js';
import { setupTokenFixture, SERVER_AVAILABLE, ORIGIN } from './support/harness.js';
import type { TokenServer } from './support/harness.js';
import { installBrowserSocket } from './support/browser-socket.js';
import { FailureRecorder, withFailureDiagnostics } from './support/failure-diagnostics.js';

describe.skipIf(!SERVER_AVAILABLE)('token revoke and expiry e2e', () => {
  let server: TokenServer | null = null;
  let scram: ShamirClient | null = null;
  let admin: ShamirClient | null = null;
  const user = `token_svc_${process.pid}`;
  beforeAll(async () => { const f = await setupTokenFixture(); ({ server, scram, admin } = f); }, 60000);
  afterAll(async () => {
    if (admin) await admin.close().catch(() => undefined);
    if (scram) await scram.close().catch(() => undefined);
    if (server) await server.stop();
  }, 15000);

  it('observes browser WS revocation close code and subsequent HTTP rejection', async () => {
    const recorder = new FailureRecorder();
    await withFailureDiagnostics(recorder, () => server!.logs(), async () => {
      const created = await scram!.apiTokens.create({ label: `revoke-${process.pid}`, for_user: user, mode: 'read_write', channels: 'both', ttl_secs: 3600 });
      recorder.mark('token created');
      const observer = installBrowserSocket(ORIGIN);
      let ws: TokenWsClient | undefined;
      try {
        recorder.mark('browser ws connect sent');
        ws = await browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${server!.http}`, token: created.token, requestTimeoutMs: 10000 });
        recorder.mark('browser ws connected');
        await ws.ping();
        recorder.mark('first ping acknowledged');
        await scram!.apiTokens.revoke(created.id);
        recorder.mark('revoke issued');
        const closed = await observer.sockets[0]!.waitForClose(10000);
        recorder.event(`close code=${closed.code} reason=${closed.reason}`);
        expect(closed).toMatchObject({ code: 4011, reason: 'token_revoked' });
        await expect(ws.ping()).rejects.toMatchObject({ code: 'connection_closed' } satisfies Partial<TokenTransportError>);
      } finally { if (ws) await ws.close().catch(() => undefined); observer.restore(); }
      const http = await connect({ auth: 'token', url: `http://127.0.0.1:${server!.http}`, token: created.token, requestTimeoutMs: 10000 });
      try { await expect(http.ping()).rejects.toMatchObject({ code: 'unauthorized', status: 401 } satisfies Partial<TokenTransportError>); }
      finally { await http.close(); }
    });
  });

  it('observes natural browser WS expiry close code and subsequent REST rejection', async () => {
    const recorder = new FailureRecorder();
    await withFailureDiagnostics(recorder, () => server!.logs(), async () => {
      const created = await scram!.apiTokens.create({ label: `expiry-${process.pid}`, for_user: user, mode: 'read_only', channels: 'both', ttl_secs: 3 });
      recorder.mark('token created');
      const observer = installBrowserSocket(ORIGIN);
      let ws: TokenWsClient | undefined;
      try {
        recorder.mark('browser ws connect sent');
        ws = await browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${server!.http}`, token: created.token, requestTimeoutMs: 10000 });
        recorder.mark('browser ws connected');
        await ws.ping();
        recorder.mark('first ping acknowledged');
        const closed = await observer.sockets[0]!.waitForClose(15000);
        recorder.event(`close code=${closed.code} reason=${closed.reason}`);
        expect(closed).toMatchObject({ code: 4012, reason: 'token_expired' });
        await expect(ws.ping()).rejects.toMatchObject({ code: 'connection_closed' } satisfies Partial<TokenTransportError>);
      } finally { if (ws) await ws.close().catch(() => undefined); observer.restore(); }
      const http = await connect({ auth: 'token', url: `http://127.0.0.1:${server!.http}`, token: created.token, requestTimeoutMs: 10000 });
      try { await expect(http.ping()).rejects.toMatchObject({ code: 'unauthorized', status: 401 } satisfies Partial<TokenTransportError>); }
      finally { await http.close(); }
    });
  }, 20000);

  it('observes REST-only token browser WS channel denial', async () => {
    const recorder = new FailureRecorder();
    await withFailureDiagnostics(recorder, () => server!.logs(), async () => {
      const created = await scram!.apiTokens.create({ label: `rest-only-${process.pid}`, for_user: user, mode: 'read_only', channels: 'rest', ttl_secs: 3600 });
      recorder.mark('token created');
      const observer = installBrowserSocket(ORIGIN);
      try {
        recorder.mark('browser ws connect sent');
        const rejected = browserConnect({ auth: 'token', channel: 'ws', url: `ws://127.0.0.1:${server!.http}`, token: created.token, connectTimeoutMs: 10000 });
        recorder.mark('browser ws connect promise created');
        await expect(rejected).rejects.toBeInstanceOf(TokenTransportError);
        recorder.mark('connect rejected as expected');
        const closed = await observer.sockets[0]!.waitForClose(5000);
        recorder.event(`close code=${closed.code} reason=${closed.reason}`);
        expect(closed).toMatchObject({ code: 4013, reason: 'token_channel_denied' });
      } finally { observer.restore(); }
    });
  });
});
