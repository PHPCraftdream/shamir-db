/**
 * End-to-end tests — live subscriptions against a live shamir-server.
 *
 * Split out of `e2e.test.ts` so the subscriptions topic owns its own file
 * (clearer git-blame, smaller donor file). Vitest runs test files in
 * parallel workers, so this file uses its OWN port to avoid EADDRINUSE
 * against the sibling e2e file.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { spawn, ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import { connect } from '../index.js';
import type { ShamirClient, BatchResponse, Json } from '../index.js';
import {
  Query,
  Batch,
  filter,
  write,
  ddl,
} from '../index.js';

// ─── server binary path ───────────────────────────────────────────────────────

const _fileUrl = new URL(import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1');
const REPO_ROOT = path.resolve(path.dirname(_fileUrl), '../../../..');

const SERVER_BIN = path.join(
  REPO_ROOT,
  'target',
  'release',
  process.platform === 'win32' ? 'shamir-server.exe' : 'shamir-server',
);

const SERVER_AVAILABLE = fs.existsSync(SERVER_BIN);

// ─── server lifecycle ─────────────────────────────────────────────────────────

const HOST = '127.0.0.1';
const PORT = 13761;
const ADMIN_USER = 'admin';
const ADMIN_PASSWORD = 'correct horse battery staple';
const ORIGIN = `https://${HOST}`;

interface ServerHandle {
  stop: () => Promise<void>;
  logs: () => string;
}

function writeKtavConfig(dir: string): string {
  const certPath = path.join(dir, 'cert.pem').replace(/\\/g, '/');
  const keyPath = path.join(dir, 'key.pem').replace(/\\/g, '/');
  const cfg = `
data_dir: ${dir.replace(/\\/g, '/')}

logging: {
    level: info
    slow_query_threshold_ms: 0
}

kdf_defaults: {
    memory_kb: 19456
    time: 2
    parallelism: 1
    argon2_version: 19
}

argon2_concurrent_max: 4

listeners: [
    {
        kind: ws
        addr: ${HOST}:${PORT}
        path: /shamir/v1/browser
        profile: tls_no_export
        browser_origin_allowlist: [
            ${ORIGIN}
        ]
    }
]

tls: {
    cert_path: ${certPath}
    key_path:  ${keyPath}
}

security: {
    connection: {
        auth_init_timeout_ms: 10000
        max_active_connections: 100
    }
    query_limits: {
        max_result_size_bytes:    10485760
        max_execution_time_secs:  30
        max_queries_per_batch:    32
    }
}

audit: {
    max_file_size_mb: 0
    retention_days: 0
}

observability: {
    addr: 127.0.0.1:0
}
`.trim();

  const configPath = path.join(dir, 'server.ktav');
  fs.writeFileSync(configPath, cfg);
  return configPath;
}

function generateSelfSignedCert(dir: string): boolean {
  try {
    const { execFileSync } = require('node:child_process') as typeof import('node:child_process');
    execFileSync('openssl', [
      'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
      '-keyout', path.join(dir, 'key.pem'),
      '-out', path.join(dir, 'cert.pem'),
      '-days', '1',
      '-subj', '/CN=localhost',
      '-addext', 'subjectAltName=IP:127.0.0.1,DNS:localhost',
    ], { stdio: 'ignore', timeout: 15_000 });
    return true;
  } catch {
    return false;
  }
}

async function startServer(): Promise<ServerHandle> {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-ts-e2e-'));
  const hasCert = generateSelfSignedCert(dataDir);
  if (!hasCert) {
    fs.rmSync(dataDir, { recursive: true, force: true });
    throw new Error('openssl not available — cannot generate self-signed cert');
  }

  const configPath = writeKtavConfig(dataDir);
  const child = spawn(
    SERVER_BIN,
    ['--config', configPath, '--bootstrap-password', ADMIN_PASSWORD],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  ) as ChildProcess;

  let logBuf = '';
  child.stdout?.on('data', (chunk: Buffer) => { logBuf += chunk.toString(); });
  child.stderr?.on('data', (chunk: Buffer) => { logBuf += chunk.toString(); });

  await new Promise<void>((resolve, reject) => {
    let done = false;
    const timer = setTimeout(() => {
      if (!done) {
        done = true;
        reject(new Error(`Server failed to bind within 15s.\nLogs:\n${logBuf}`));
      }
    }, 15_000);

    function check() {
      if (done) return;
      if (/listener bound/i.test(logBuf)) {
        done = true;
        clearTimeout(timer);
        setTimeout(() => resolve(), 150);
      }
    }
    child.stdout?.on('data', check);
    child.stderr?.on('data', check);
    child.on('exit', (code: number | null, signal: string | null) => {
      if (!done) {
        done = true;
        clearTimeout(timer);
        reject(new Error(
          `Server exited prematurely (code=${code} signal=${signal}).\nLogs:\n${logBuf}`,
        ));
      }
    });
  });

  return {
    stop: async () => {
      if (!child.killed) {
        child.kill(process.platform === 'win32' ? 'SIGKILL' : 'SIGTERM');
        await new Promise<void>((r) => child.once('exit', r));
      }
      try { fs.rmSync(dataDir, { recursive: true, force: true }); } catch { /* ok */ }
    },
    logs: () => logBuf,
  };
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/**
 * Unwrap the server's DbResponse envelope. The server returns
 * `{ kind: "batch", response: BatchResponse }`. We extract the inner
 * `response` for typed access.
 */
function br(raw: object): BatchResponse {
  const env = raw as { kind?: string; response?: BatchResponse };
  if (env.kind === 'batch' && env.response) return env.response;
  // Fallback: if already a BatchResponse shape, return as-is
  if ('results' in raw && 'execution_plan' in raw) return raw as BatchResponse;
  throw new Error(`unexpected response shape: ${JSON.stringify(Object.keys(raw))}`);
}

let dbCounter = 0;
function uniqueDbName(label: string): string {
  dbCounter += 1;
  return `ts_${label}_${process.pid}_${dbCounter}`;
}

async function setupDb(
  client: ShamirClient,
  label: string,
  tableNames: string[] = ['items'],
): Promise<string> {
  const db = uniqueDbName(label);

  await client.execute('default', {
    id: `setup-${db}-db`,
    queries: { mk: ddl.createDb(db) },
  });

  const queries: Record<string, object> = { mr: ddl.createRepo('main') };
  for (let i = 0; i < tableNames.length; i += 1) {
    queries[`tb${i}`] = ddl.createTable(tableNames[i], { repo: 'main' });
  }
  await client.execute(db, {
    id: `setup-${db}-tables`,
    queries,
  });

  return db;
}

async function seed(
  client: ShamirClient,
  db: string,
  table: string,
  records: Array<Record<string, Json>>,
  keyFields: string[] = ['id'],
): Promise<BatchResponse> {
  const queries: Record<string, object> = {};
  records.forEach((r, i) => {
    const key: Record<string, Json> = {};
    for (const k of keyFields) key[k] = r[k];
    queries[`s${i}`] = write.upsert(table, key, r);
  });
  return br(await client.execute(db, { id: `seed-${db}-${table}`, queries }));
}

// ─── live subscriptions (A5) ─────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'live subscriptions (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connect({
          host: HOST,
          port: PORT,
          username: ADMIN_USER,
          password: ADMIN_PASSWORD,
          tls: { rejectUnauthorized: false },
          origin: ORIGIN,
        });
      } catch (e) {
        console.error('[live-subs] connection failed. Server logs:\n' + server.logs());
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try { await client.close(); } catch { /* ok */ }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    /** Race a promise against a timeout — resolves with `null` if it doesn't settle in time. */
    async function withTimeout<T>(p: Promise<T>, ms: number): Promise<T | null> {
      let to: ReturnType<typeof setTimeout> | null = null;
      const timer = new Promise<null>((resolve) => {
        to = setTimeout(() => resolve(null), ms);
      });
      const out = await Promise.race([p, timer]);
      if (to) clearTimeout(to);
      return out;
    }

    // 1. records delivery — INSERT must yield an event.
    it('records: insert matching the filter delivers an event', async () => {
      const dbName = await setupDb(client!, 'sub_records', ['m']);
      const db = client!.db(dbName);

      const { subs } = await db.runLive(
        db.batch('sub-1').subscribe('m', {
          store: 'main',
          table: 'm',
          where: (f) => f.eq('x', 1),
        }),
      );
      expect(subs.m).toBeDefined();
      expect(typeof subs.m.subId).toBe('number');

      await db.batch('ins').add('i', write.insert('m', [{ id: 'k1', x: 1 }])).run();

      const ev = await withTimeout(subs.m.next(), 1500);
      expect(ev).not.toBeNull();
      expect(ev!.done).toBe(false);
      expect(ev!.value.kind).toBe('event');

      await subs.m.unsubscribe().catch(() => {});
    });

    // 2. filter proves wire savings — unmatched rows must not arrive.
    it('filter: unmatched rows are dropped server-side (no wire frame)', async () => {
      const dbName = await setupDb(client!, 'sub_filter', ['messages']);
      const db = client!.db(dbName);

      const { subs } = await db.runLive(
        db.batch('sub-f').subscribe('messages', {
          store: 'main',
          table: 'messages',
          where: (f) => f.eq('thread_id', 42),
        }),
      );

      // Hold ONE next() across the whole assertion. Each next() is its own
      // waiter; if we timed it out and then called next() again, the brought-
      // in event would be delivered to the abandoned waiter — not the new one.
      const pending = subs.messages.next();

      // Insert a NON-matching row. The server-side filter drops the event
      // before any push frame is emitted — the wire stays empty.
      await db.batch('noise')
        .add('i', write.insert('messages', [{ id: 'n1', thread_id: 7, body: 'noise' }]))
        .run();

      const none = await Promise.race([
        pending.then((v) => ({ resolved: true as const, v })),
        new Promise<{ resolved: false }>((r) => setTimeout(() => r({ resolved: false }), 300)),
      ]);
      expect(none.resolved).toBe(false); // no frame within 300 ms

      // Now insert a matching row — it must arrive on the SAME pending next().
      await db.batch('real')
        .add('i', write.insert('messages', [{ id: 'r1', thread_id: 42, body: 'real' }]))
        .run();

      const ev = await withTimeout(pending, 1500);
      expect(ev).not.toBeNull();
      expect(ev!.value.kind).toBe('event');

      await subs.messages.unsubscribe().catch(() => {});
    });

    // 3. two subscriptions in one batch — each on its own stream.
    it('multi: two subscribe ops in one batch yield two independent streams', async () => {
      const dbName = await setupDb(client!, 'sub_multi', ['a', 'b']);
      const db = client!.db(dbName);

      const { response, subs } = await db.runLive(
        db.batch('multi-sub')
          .subscribe('sa', { store: 'main', table: 'a', where: (f) => f.eq('k', 1) })
          .subscribe('sb', { store: 'main', table: 'b', where: (f) => f.eq('k', 2) }),
      );
      expect(subs.sa).toBeDefined();
      expect(subs.sb).toBeDefined();
      expect(subs.sa.subId).not.toBe(subs.sb.subId);
      // Both grants carry a numeric sub id.
      expect(typeof (response.results.sa.value as { sub?: unknown }).sub).toBe('number');
      expect(typeof (response.results.sb.value as { sub?: unknown }).sub).toBe('number');

      await db.batch('ins-a').add('i', write.insert('a', [{ id: 'aa', k: 1 }])).run();
      await db.batch('ins-b').add('i', write.insert('b', [{ id: 'bb', k: 2 }])).run();

      const eva = await withTimeout(subs.sa.next(), 1500);
      const evb = await withTimeout(subs.sb.next(), 1500);
      expect(eva).not.toBeNull();
      expect(evb).not.toBeNull();
      expect(eva!.value.kind).toBe('event');
      expect(evb!.value.kind).toBe('event');

      await subs.sa.unsubscribe().catch(() => {});
      await subs.sb.unsubscribe().catch(() => {});
    });

    // 4. handle: reactive sub-batch delivers an event with the batch's result.
    it('handle: reactive sub-batch delivers a frame on insert', async () => {
      const dbName = await setupDb(client!, 'sub_handle', ['threads', 'msgs']);
      const db = client!.db(dbName);
      await seed(client!, dbName, 'threads', [{ id: 't1', topic: 'hello' }]);

      const { subs } = await db.runLive(
        db.batch('sub-h').subscribe('h', {
          store: 'main',
          table: 'msgs',
          // Reactive sub-batch: read all threads on every event. We don't
          // reference $event here — the test only verifies that the reactive
          // delivery path produces a non-empty frame on insert.
          handle: (b) => b.add('t', Query.from('threads')),
        }),
      );

      await db.batch('ins').add('i', write.insert('msgs', [{ id: 'm1', body: 'hi' }])).run();

      const ev = await withTimeout(subs.h.next(), 1500);
      expect(ev).not.toBeNull();
      expect(ev!.value.kind).toBe('event');

      await subs.h.unsubscribe().catch(() => {});
    });

    // 5. initial snapshot — pre-seeded rows arrive before `ready`.
    it('initial: pre-seeded rows arrive before kind:"ready"', async () => {
      const dbName = await setupDb(client!, 'sub_initial', ['items']);
      const db = client!.db(dbName);
      const N = 3;
      const seedRecs: Array<Record<string, Json>> = [];
      for (let i = 0; i < N; i += 1) seedRecs.push({ id: `s${i}`, n: i });
      await seed(client!, dbName, 'items', seedRecs);

      const { subs } = await db.runLive(
        db.batch('sub-i').subscribe(
          'i',
          { store: 'main', table: 'items' },
          { initial: true },
        ),
      );

      // Collect events until 'ready'.
      let preReady = 0;
      let sawReady = false;
      for (let i = 0; i < N + 5 && !sawReady; i += 1) {
        const ev = await withTimeout(subs.i.next(), 2000);
        if (!ev || ev.done) break;
        if (ev.value.kind === 'ready') { sawReady = true; break; }
        if (ev.value.kind === 'event') preReady += 1;
      }
      expect(sawReady).toBe(true);
      expect(preReady).toBeGreaterThanOrEqual(N);

      // Live events still flow after ready.
      await db.batch('ins').add('i', write.insert('items', [{ id: 'live', n: 99 }])).run();
      const live = await withTimeout(subs.i.next(), 1500);
      expect(live).not.toBeNull();
      expect(live!.value.kind).toBe('event');

      await subs.i.unsubscribe().catch(() => {});
    });

    // 6. unsubscribe stops the stream.
    it('unsubscribe: stream goes done; further inserts do not yield events', async () => {
      const dbName = await setupDb(client!, 'sub_unsub', ['x']);
      const db = client!.db(dbName);

      const { subs } = await db.runLive(
        db.batch('sub-u').subscribe('x', {
          store: 'main',
          table: 'x',
          where: (f) => f.eq('k', 1),
        }),
      );

      await db.batch('a').add('i', write.insert('x', [{ id: 'x1', k: 1 }])).run();
      const first = await withTimeout(subs.x.next(), 1500);
      expect(first).not.toBeNull();
      expect(first!.value.kind).toBe('event');

      await subs.x.unsubscribe();

      await db.batch('b').add('i', write.insert('x', [{ id: 'x2', k: 1 }])).run();
      const after = await withTimeout(subs.x.next(), 400);
      // Either timed out (null), or the iterator reports done.
      if (after !== null) {
        expect(after.done).toBe(true);
      }
    });

    // 7. grant refusal — multi-repo subscription is rejected.
    it('refusal: multi-repo subscription is rejected by the server', async () => {
      const dbName = uniqueDbName('sub_refuse');
      await client!.execute('default', {
        id: `setup-${dbName}-db`,
        queries: { mk: ddl.createDb(dbName) },
      });
      // Create two repos with one table each.
      await client!.execute(dbName, {
        id: `setup-${dbName}-repos`,
        queries: {
          r1: ddl.createRepo('main'),
          r2: ddl.createRepo('other'),
          t1: ddl.createTable('a', { repo: 'main' }),
          t2: ddl.createTable('b', { repo: 'other' }),
        },
      });
      const db = client!.db(dbName);

      // Subscribe to two sources spanning two repos — must throw with the
      // documented multi_repo_subscriptions_not_supported code.
      await expect(
        db.runLive(
          db.batch('refuse').subscribe('s', [
            { store: 'main', table: 'a' },
            { store: 'other', table: 'b' },
          ]),
        ),
      ).rejects.toThrow(/multi_repo_subscriptions_not_supported|multi-repo/i);
    });
  },
);

describe('e2e-subscriptions.test skip reason', () => {
  it('reports why the e2e-subscriptions test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        `[e2e-subscriptions.test] SKIPPED — server binary not found at:\n  ${SERVER_BIN}\n` +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
