/**
 * Connection matrix — every napi connection variant against ONE server that
 * exposes TCP+TLS, WS+TLS, plain loopback TCP, HTTPS, plain HTTP (both with
 * password login) and a local IPC listener (see helpers/matrix-server.js).
 *
 * The matrix server is started lazily by the first test and stopped by the
 * last one; the shared `server`/`client` of the orchestrator are not used.
 */

'use strict';

const process = require('process');
const { Query, filter, write } = require('@shamir/client');
const { ShamirClient, ShamirTokenHttpClient, ShamirDbError, login } = require('shamir-client');
const { startMatrixServer } = require('../helpers/matrix-server');

module.exports = async function ({ test, assert, assertEq, assertThrows, fixtures }) {
  // Bounded deadlines so a hung connect fails the test instead of the run.
  const SESSION_TIMEOUTS = { connectTimeoutMs: 10000, handshakeTimeoutMs: 30000, requestTimeoutMs: 30000 };
  const RESUME_TIMEOUTS = { handshakeTimeoutMs: 30000, requestTimeoutMs: 30000 };
  const LOCAL_TIMEOUTS = { connectTimeoutMs: 10000, requestTimeoutMs: 30000 };
  const TOKEN_TIMEOUTS = { connectTimeoutMs: 10000, requestTimeoutMs: 30000 };

  let matrixPromise = null;
  let counter = 0;

  function matrix() {
    if (matrixPromise === null) matrixPromise = startMatrixServer();
    return matrixPromise;
  }

  function unique(prefix) {
    counter += 1;
    return `${prefix}-${process.pid}-${counter}`;
  }

  async function closeQuietly(c) {
    if (c) {
      try {
        await c.close();
      } catch (_) {
        /* best-effort cleanup */
      }
    }
  }

  async function adminTcp(m) {
    return ShamirClient.connect({
      host: m.host,
      port: m.ports.tcp,
      serverName: 'localhost',
      username: m.user,
      password: m.password,
      acceptNewHost: true,
      ...SESSION_TIMEOUTS,
    });
  }

  async function withAdmin(fn) {
    const m = await matrix();
    const admin = await adminTcp(m);
    try {
      return await fn(admin, m);
    } finally {
      await closeQuietly(admin);
    }
  }

  // A fresh table with one seeded row, built with the shared fixtures.
  async function freshDb(admin, label) {
    const db = await fixtures.setupDb(admin, label, ['t']);
    await fixtures.seed(admin, db, 't', [{ id: 'a', qty: 1 }]);
    return db;
  }

  async function readRows(c, db, id) {
    const resp = await c.execute(db, {
      id: 'r',
      queries: { r: Query.from('t').where(filter.eq('id', id)).build() },
    });
    return resp.results.r.records;
  }

  async function writeRow(c, db, id, qty) {
    await c.execute(db, {
      id: 'w',
      queries: { w: write.upsert('t', { id }, { id, qty }) },
    });
  }

  async function newToken(admin, { channels = 'both', mode = 'read_write', label } = {}) {
    return admin.createApiToken({
      label: label || unique('matrix-tok'),
      ttlSecs: 3600,
      mode,
      channels,
    });
  }

  function tokenHttpOpts(m, token) {
    return { host: m.host, port: m.ports.httpTls, serverName: 'localhost', caPem: m.caPem, token, ...TOKEN_TIMEOUTS };
  }

  function tokenWsOpts(m, token) {
    return {
      host: m.host,
      port: m.ports.httpTls,
      serverName: 'localhost',
      caPem: m.caPem,
      token,
      ...TOKEN_TIMEOUTS,
      handshakeTimeoutMs: 30000,
    };
  }

  // Secret material must never appear in an error message, in any common
  // text encoding.
  function leaks(message, bufs) {
    for (const b of bufs) {
      const buf = Buffer.from(b);
      if (buf.length === 0) continue;
      if (message.toLowerCase().includes(buf.toString('hex'))) return true;
      if (message.includes(buf.toString('base64'))) return true;
      if (message.includes(buf.toString('base64url'))) return true;
    }
    return false;
  }

  async function ordinaryUser(admin) {
    const name = unique('matrix-user');
    const password = `pw-${unique('secret')}-Zx9`;
    await admin.createScramUser(name, password, []);
    return { name, password };
  }

  test('matrix server starts with six listeners', async () => {
    const m = await matrix();
    assert(m.caPem.includes('BEGIN CERTIFICATE'), 'caPem must be a PEM certificate');
    assert(m.ipcAddr.length > 0, 'ipc address must be set');
  });

  // ─── connectWs ─────────────────────────────────────────────────────────

  test('connectWs: scram session, ping and execute round trip', async () => {
    const m = await matrix();
    const c = await ShamirClient.connectWs({
      host: m.host,
      port: m.ports.ws,
      serverName: 'localhost',
      username: m.user,
      password: m.password,
      acceptNewHost: true,
      ...SESSION_TIMEOUTS,
    });
    try {
      assertEq(c.authKind(), 'scram');
      assertEq(c.sessionId().length, 32);
      await c.ping();
      const db = await freshDb(c, 'mx_ws');
      const rows = await readRows(c, db, 'a');
      assertEq(rows.length, 1);
    } finally {
      await closeQuietly(c);
    }
  });

  // ─── connectPlain ──────────────────────────────────────────────────────

  test('connectPlain: loopback plain TCP ping', async () => {
    const m = await matrix();
    const c = await ShamirClient.connectPlain({
      host: m.host,
      port: m.ports.plain,
      username: m.user,
      password: m.password,
      acceptNewHost: true,
      ...SESSION_TIMEOUTS,
    });
    try {
      assertEq(c.authKind(), 'scram');
      await c.ping();
    } finally {
      await closeQuietly(c);
    }
  });

  test('connectPlain: non-loopback host is refused before any socket', async () => {
    const m = await matrix();
    await assertThrows(
      () =>
        ShamirClient.connectPlain({
          host: '192.0.2.1',
          port: m.ports.plain,
          username: m.user,
          password: m.password,
          acceptNewHost: true,
          ...SESSION_TIMEOUTS,
        }),
      (e) => /loopback/i.test(e.message),
      'non-loopback plain connect must name the loopback rule'
    );
  });

  // ─── connectLocal ──────────────────────────────────────────────────────

  test('connectLocal: IPC listener ping', async () => {
    const m = await matrix();
    const c = await ShamirClient.connectLocal({
      addr: m.ipcAddr,
      username: m.user,
      password: m.password,
      acceptNewHost: true,
      ...LOCAL_TIMEOUTS,
    });
    try {
      await c.ping();
    } finally {
      await closeQuietly(c);
    }
  });

  // ─── resume / resumeWs ─────────────────────────────────────────────────

  test('resume: TCP+TLS resumption from a fresh session ticket', async () => {
    const m = await matrix();
    const first = await adminTcp(m);
    let resumed = null;
    try {
      const ticket = first.resumptionTicket();
      assert(ticket !== null && ticket.length > 0, 'expected a resumption ticket');
      resumed = await ShamirClient.resume({
        host: m.host,
        port: m.ports.tcp,
        serverName: 'localhost',
        ticket,
        trustedPin: first.serverPubKeyPin(),
        ...RESUME_TIMEOUTS,
      });
      await resumed.ping();
    } finally {
      await closeQuietly(resumed);
      await closeQuietly(first);
    }
  });

  test('resumeWs: ticket obtained over TCP', async () => {
    const m = await matrix();
    const first = await adminTcp(m);
    let resumed = null;
    try {
      const ticket = first.resumptionTicket();
      assert(ticket !== null && ticket.length > 0, 'expected a resumption ticket');
      resumed = await ShamirClient.resumeWs({
        host: m.host,
        port: m.ports.ws,
        serverName: 'localhost',
        ticket,
        trustedPin: first.serverPubKeyPin(),
        ...RESUME_TIMEOUTS,
      });
      await resumed.ping();
    } finally {
      await closeQuietly(resumed);
      await closeQuietly(first);
    }
  });

  test('resumeWs: ticket obtained over WS', async () => {
    const m = await matrix();
    const first = await ShamirClient.connectWs({
      host: m.host,
      port: m.ports.ws,
      serverName: 'localhost',
      username: m.user,
      password: m.password,
      acceptNewHost: true,
      ...SESSION_TIMEOUTS,
    });
    let resumed = null;
    try {
      const ticket = first.resumptionTicket();
      assert(ticket !== null && ticket.length > 0, 'expected a resumption ticket');
      resumed = await ShamirClient.resumeWs({
        host: m.host,
        port: m.ports.ws,
        serverName: 'localhost',
        ticket,
        trustedPin: first.serverPubKeyPin(),
        ...RESUME_TIMEOUTS,
      });
      await resumed.ping();
    } finally {
      await closeQuietly(resumed);
      await closeQuietly(first);
    }
  });

  test('resume: wrong pin rejects without echoing ticket or pin', async () => {
    const m = await matrix();
    const first = await adminTcp(m);
    try {
      const ticket = Buffer.from(first.resumptionTicket());
      const realPin = Buffer.from(first.serverPubKeyPin());
      const wrongPin = Buffer.from(realPin.map((b) => b ^ 0xff));
      const err = await assertThrows(() =>
        ShamirClient.resume({
          host: m.host,
          port: m.ports.tcp,
          serverName: 'localhost',
          ticket,
          trustedPin: wrongPin,
          ...RESUME_TIMEOUTS,
        })
      );
      assert(!leaks(err.message, [ticket, realPin, wrongPin]), 'error must not echo ticket or pin');
    } finally {
      await closeQuietly(first);
    }
  });

  test('resume: corrupted ticket rejects without echoing ticket or pin', async () => {
    const m = await matrix();
    const first = await adminTcp(m);
    try {
      const original = Buffer.from(first.resumptionTicket());
      const pin = Buffer.from(first.serverPubKeyPin());
      const corrupted = Buffer.from(original);
      corrupted[corrupted.length - 1] ^= 0xff;
      corrupted[0] ^= 0xff;
      const err = await assertThrows(() =>
        ShamirClient.resume({
          host: m.host,
          port: m.ports.tcp,
          serverName: 'localhost',
          ticket: corrupted,
          trustedPin: pin,
          ...RESUME_TIMEOUTS,
        })
      );
      assert(!leaks(err.message, [original, corrupted, pin]), 'error must not echo ticket or pin');
    } finally {
      await closeQuietly(first);
    }
  });

  // ─── API tokens over REST ──────────────────────────────────────────────

  test('createApiToken: admin gets a shk1 token with the requested scope', async () => {
    await withAdmin(async (admin) => {
      const label = unique('matrix-scope');
      const t = await newToken(admin, { label });
      assert(typeof t.token === 'string' && t.token.startsWith('shk1.'), 'token must start with shk1.');
      assertEq(t.label, label);
      assertEq(t.mode, 'read_write');
      assertEq(t.channels, 'both');
    });
  });

  test('token over HTTPS: ping, write then read', async () => {
    await withAdmin(async (admin, m) => {
      const db = await freshDb(admin, 'mx_tokhttps');
      const t = await newToken(admin);
      const http = await ShamirTokenHttpClient.connect(tokenHttpOpts(m, t.token));
      try {
        await http.ping();
        await writeRow(http, db, 'b', 2);
        const rows = await readRows(http, db, 'b');
        assertEq(rows.length, 1);
      } finally {
        await closeQuietly(http);
      }
    });
  });

  test('token over plain loopback HTTP: ping', async () => {
    await withAdmin(async (admin, m) => {
      const t = await newToken(admin, { channels: 'rest' });
      const http = await ShamirTokenHttpClient.connect({
        host: m.host,
        port: m.ports.httpPlain,
        plainLoopback: true,
        token: t.token,
        ...TOKEN_TIMEOUTS,
      });
      try {
        await http.ping();
      } finally {
        await closeQuietly(http);
      }
    });
  });

  test('read_only token rejects a write', async () => {
    await withAdmin(async (admin, m) => {
      const db = await freshDb(admin, 'mx_readonly');
      const t = await newToken(admin, { mode: 'read_only', channels: 'rest' });
      const http = await ShamirTokenHttpClient.connect(tokenHttpOpts(m, t.token));
      try {
        // A read_only token can still read.
        const rows = await readRows(http, db, 'a');
        assertEq(rows.length, 1);
        await assertThrows(
          () => writeRow(http, db, 'denied', 9),
          (e) => e instanceof ShamirDbError,
          'read_only token must not write'
        );
        // The denied row must not exist.
        assertEq((await readRows(admin, db, 'denied')).length, 0);
      } finally {
        await closeQuietly(http);
      }
    });
  });

  // ─── API tokens over WebSocket ─────────────────────────────────────────

  test('connectTokenWs: token session reads, refuses admin calls', async () => {
    await withAdmin(async (admin, m) => {
      const db = await freshDb(admin, 'mx_tokws');
      const t = await newToken(admin, { channels: 'ws' });
      const c = await ShamirClient.connectTokenWs(tokenWsOpts(m, t.token));
      try {
        assertEq(c.authKind(), 'token');
        await c.ping();
        assertEq((await readRows(c, db, 'a')).length, 1);

        const denied = (e) => e instanceof ShamirDbError && e.code === 'token_request_denied';
        await assertThrows(
          () => c.createScramUser(unique('matrix-denied'), 'pw-denied-Zx9', []),
          denied,
          'token session must not create users'
        );
        await assertThrows(
          () => c.createApiToken({ label: unique('matrix-denied-tok') }),
          denied,
          'token session must not mint tokens'
        );

        const pin = c.serverPubKeyPin();
        assertEq(pin.length, 32);
        assert(
          pin.every((b) => b === 0),
          'token session has no server pin (32 zero bytes)'
        );
      } finally {
        await closeQuietly(c);
      }
    });
  });

  test('connectTokenWs: token with channels both is accepted', async () => {
    await withAdmin(async (admin, m) => {
      const t = await newToken(admin, { channels: 'both' });
      const c = await ShamirClient.connectTokenWs(tokenWsOpts(m, t.token));
      try {
        assertEq(c.authKind(), 'token');
        await c.ping();
      } finally {
        await closeQuietly(c);
      }
    });
  });

  test('connectTokenWs: rest-only token is refused', async () => {
    await withAdmin(async (admin, m) => {
      const t = await newToken(admin, { channels: 'rest' });
      await assertThrows(
        () => ShamirClient.connectTokenWs(tokenWsOpts(m, t.token)),
        (e) => e instanceof ShamirDbError && e.code === 'token_channel_denied',
        'rest-only token must be channel-denied over WS'
      );
    });
  });

  // ─── password login ────────────────────────────────────────────────────

  function assertIssued(issued, user) {
    assert(typeof issued.token === 'string' && issued.token.startsWith('shk1.'), 'token must start with shk1.');
    assert(Buffer.isBuffer(issued.id), 'id must be a Buffer');
    assertEq(issued.id.length, 16);
    assertEq(issued.owner, user);
  }

  test('login over HTTPS: read_only rest token usable with ShamirTokenHttpClient', async () => {
    await withAdmin(async (admin, m) => {
      const u = await ordinaryUser(admin);
      const issued = await login({
        host: m.host,
        port: m.ports.httpTls,
        serverName: 'localhost',
        caPem: m.caPem,
        username: u.name,
        password: u.password,
        label: unique('matrix-login'),
        mode: 'read_only',
        channels: 'rest',
        ...TOKEN_TIMEOUTS,
      });
      assertIssued(issued, u.name);
      const http = await ShamirTokenHttpClient.connect(tokenHttpOpts(m, issued.token));
      try {
        await http.ping();
      } finally {
        await closeQuietly(http);
      }
    });
  });

  test('login over plain loopback HTTP: read_only rest token usable', async () => {
    await withAdmin(async (admin, m) => {
      const u = await ordinaryUser(admin);
      const issued = await login({
        host: m.host,
        port: m.ports.httpPlain,
        plainLoopback: true,
        username: u.name,
        password: u.password,
        label: unique('matrix-login-plain'),
        mode: 'read_only',
        channels: 'rest',
        ...TOKEN_TIMEOUTS,
      });
      assertIssued(issued, u.name);
      const http = await ShamirTokenHttpClient.connect({
        host: m.host,
        port: m.ports.httpPlain,
        plainLoopback: true,
        token: issued.token,
        ...TOKEN_TIMEOUTS,
      });
      try {
        await http.ping();
      } finally {
        await closeQuietly(http);
      }
    });
  });

  test('login with channels both: token works over connectTokenWs', async () => {
    await withAdmin(async (admin, m) => {
      const u = await ordinaryUser(admin);
      const issued = await login({
        host: m.host,
        port: m.ports.httpTls,
        serverName: 'localhost',
        caPem: m.caPem,
        username: u.name,
        password: u.password,
        label: unique('matrix-login-both'),
        mode: 'read_only',
        channels: 'both',
        ...TOKEN_TIMEOUTS,
      });
      assertIssued(issued, u.name);
      const c = await ShamirClient.connectTokenWs(tokenWsOpts(m, issued.token));
      try {
        assertEq(c.authKind(), 'token');
        await c.ping();
      } finally {
        await closeQuietly(c);
      }
    });
  });

  test('login: wrong password rejects without echoing the password', async () => {
    await withAdmin(async (admin, m) => {
      const u = await ordinaryUser(admin);
      const wrong = `wrong-${unique('pw')}-Qq7`;
      const err = await assertThrows(() =>
        login({
          host: m.host,
          port: m.ports.httpTls,
          serverName: 'localhost',
          caPem: m.caPem,
          username: u.name,
          password: wrong,
          mode: 'read_only',
          channels: 'rest',
          ...TOKEN_TIMEOUTS,
        })
      );
      assert(!err.message.includes(wrong), 'error must not echo the supplied password');
      assert(!err.message.includes(u.password), 'error must not echo the real password');
    });
  });

  test('login: superuser account is denied', async () => {
    const m = await matrix();
    await assertThrows(
      () =>
        login({
          host: m.host,
          port: m.ports.httpTls,
          serverName: 'localhost',
          caPem: m.caPem,
          username: m.user,
          password: m.password,
          mode: 'read_only',
          channels: 'rest',
          ...TOKEN_TIMEOUTS,
        }),
      (e) => /login_account_denied/.test(e.message),
      'superuser login must be denied'
    );
  });

  // ─── secret hygiene ────────────────────────────────────────────────────

  test('ShamirTokenHttpClient.connect: malformed token rejects without echo', async () => {
    const m = await matrix();
    const bogus = 'not-a-token-SECRETMARKER-12345';
    const err = await assertThrows(() =>
      ShamirTokenHttpClient.connect({
        host: m.host,
        port: m.ports.httpPlain,
        plainLoopback: true,
        token: bogus,
        ...TOKEN_TIMEOUTS,
      })
    );
    assert(!String(err.message).includes(bogus), 'error must not echo the supplied token');
  });

  test('matrix server teardown', async () => {
    if (matrixPromise === null) return;
    let m = null;
    try {
      m = await matrixPromise;
    } catch (_) {
      return; // start failed; startMatrixServer already cleaned up
    }
    await m.stop();
  });
};
