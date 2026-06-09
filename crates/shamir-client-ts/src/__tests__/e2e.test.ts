/**
 * End-to-end tests — typed builders against a live shamir-server.
 *
 * Spawns a single server + connects a single client in `beforeAll`.
 * Each `it` exercises one scenario ported from the JS e2e suite
 * (tests/e2e/tests/02–08, 12, 15) but uses ONLY the typed TS builders.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { spawn, ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import { connect } from '../index.js';
import type { ShamirClient, BatchResponse } from '../index.js';
import {
  Query,
  Batch,
  // filter
  eq, ne, gt, gte, lt, lte, in_, notIn, and, or, between,
  // select
  field, countAll, sum, avg, min, max,
  // write
  insert, update, upsert, del,
  // ddl
  createDb, createRepo, createTable, createIndex,
  dropDb, dropTable, dropIndex, listDatabases, listIndexes,
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
const PORT = 13760;
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
    queries: { mk: createDb(db) },
  });

  const queries: Record<string, object> = { mr: createRepo('main') };
  for (let i = 0; i < tableNames.length; i += 1) {
    queries[`tb${i}`] = createTable(tableNames[i], { repo: 'main' });
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
  records: object[],
  keyFields: string[] = ['id'],
): Promise<BatchResponse> {
  const queries: Record<string, object> = {};
  records.forEach((r, i) => {
    const key: Record<string, unknown> = {};
    for (const k of keyFields) key[k] = (r as Record<string, unknown>)[k];
    queries[`s${i}`] = upsert(table, key, r);
  });
  return br(await client.execute(db, { id: `seed-${db}-${table}`, queries }));
}

// ─── test suite ───────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e typed builders (requires release binary)',
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
        console.error('[e2e.test] connection failed. Server logs:\n' + server.logs());
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

    // ── 1. connect ──────────────────────────────────────────────────────

    it('connect: session id is 32 bytes', () => {
      expect(client).not.toBeNull();
      expect(client!.sessionId().length).toBe(32);
    });

    // ── 2. setup ────────────────────────────────────────────────────────

    let crudDb: string;

    it('setup: create db + repo + table', async () => {
      crudDb = await setupDb(client!, 'crud', ['items']);
      expect(typeof crudDb).toBe('string');
      expect(crudDb.length).toBeGreaterThan(0);
    });

    // ── 3. CRUD (port 02) ───────────────────────────────────────────────

    it('CRUD: insert single record', async () => {
      const resp = br(await Batch.create('ins-one')
        .add('ins', insert('items', [{ id: 'A1', name: 'widget', qty: 10 }]))
        .execute(client!, crudDb));
      expect(resp.results.ins.records.length).toBe(1);
    });

    it('CRUD: read all returns the inserted record', async () => {
      const resp = br(await Batch.create('read-all')
        .add('all', Query.from('items'))
        .execute(client!, crudDb));
      const recs = resp.results.all.records;
      expect(recs.length).toBe(1);
      expect(recs[0].id).toBe('A1');
      expect(recs[0].qty).toBe(10);
    });

    it('CRUD: upsert a new key', async () => {
      await br(await Batch.create('set-new')
        .add('s', upsert('items', { id: 'B2' }, { id: 'B2', name: 'gear', qty: 3 }))
        .execute(client!, crudDb));
      const resp = br(await Batch.create('count-after-set')
        .add('all', Query.from('items'))
        .execute(client!, crudDb));
      expect(resp.results.all.records.length).toBe(2);
    });

    it('CRUD: upsert overwrites an existing key', async () => {
      await br(await Batch.create('set-existing')
        .add('s', upsert('items', { id: 'A1' }, { id: 'A1', name: 'widget-v2', qty: 99 }))
        .execute(client!, crudDb));
      const resp = br(await Batch.create('read-A1')
        .add('a', Query.from('items').where(eq('id', 'A1')))
        .execute(client!, crudDb));
      expect(resp.results.a.records.length).toBe(1);
      expect(resp.results.a.records[0].name).toBe('widget-v2');
      expect(resp.results.a.records[0].qty).toBe(99);
    });

    it('CRUD: update by filter', async () => {
      await br(await Batch.create('upd')
        .add('u', update('items').where(eq('id', 'B2')).set({ qty: 7 }).build())
        .execute(client!, crudDb));
      const resp = br(await Batch.create('read-B2')
        .add('b', Query.from('items').where(eq('id', 'B2')))
        .execute(client!, crudDb));
      expect(resp.results.b.records[0].qty).toBe(7);
    });

    it('CRUD: delete by filter', async () => {
      await br(await Batch.create('del')
        .add('d', del('items', eq('id', 'A1')))
        .execute(client!, crudDb));
      const resp = br(await Batch.create('read-after-del')
        .add('all', Query.from('items'))
        .execute(client!, crudDb));
      expect(resp.results.all.records.length).toBe(1);
      expect(resp.results.all.records[0].id).toBe('B2');
    });

    // ── 4. Filters (port 05) ────────────────────────────────────────────

    let filterDb: string;
    const filterSeed = [
      { id: 'a', qty: 1, tag: 'red', addr: { city: 'NYC' } },
      { id: 'b', qty: 5, tag: 'red', addr: { city: 'LA' } },
      { id: 'c', qty: 10, tag: 'blue', addr: { city: 'NYC' } },
      { id: 'd', qty: 25, tag: 'blue', addr: { city: 'SF' } },
      { id: 'e', qty: 50, tag: 'green', addr: { city: 'LA' } },
    ];

    it('filters: setup + seed', async () => {
      filterDb = await setupDb(client!, 'filters', ['t']);
      await seed(client!, filterDb, 't', filterSeed);
    });

    async function filteredRead(where: ReturnType<typeof eq>): Promise<object[]> {
      const resp = br(await Batch.create('r')
        .add('r', Query.from('t').where(where))
        .execute(client!, filterDb));
      return resp.results.r.records;
    }

    it('filters: eq', async () => {
      const r = await filteredRead(eq('tag', 'red'));
      expect(r.length).toBe(2);
    });

    it('filters: ne', async () => {
      const r = await filteredRead(ne('tag', 'red'));
      expect(r.length).toBe(3);
    });

    it('filters: gt', async () => {
      const r = await filteredRead(gt('qty', 10));
      expect(r.length).toBe(2);
    });

    it('filters: gte', async () => {
      const r = await filteredRead(gte('qty', 10));
      expect(r.length).toBe(3);
    });

    it('filters: lt', async () => {
      const r = await filteredRead(lt('qty', 10));
      expect(r.length).toBe(2);
    });

    it('filters: lte', async () => {
      const r = await filteredRead(lte('qty', 10));
      expect(r.length).toBe(3);
    });

    it('filters: in', async () => {
      const r = await filteredRead(in_('tag', ['red', 'green']));
      expect(r.length).toBe(3);
    });

    it('filters: not_in', async () => {
      const r = await filteredRead(notIn('tag', ['red', 'green']));
      expect(r.length).toBe(2);
    });

    it('filters: between', async () => {
      const r = await filteredRead(between('qty', 5, 25));
      expect(r.length).toBe(3);
    });

    it('filters: and', async () => {
      const r = await filteredRead(
        and([
          eq('tag', 'blue'),
          gt('qty', 10),
        ]),
      );
      expect(r.length).toBe(1);
      expect((r[0] as Record<string, unknown>).id).toBe('d');
    });

    it('filters: or', async () => {
      const r = await filteredRead(
        or([
          eq('tag', 'green'),
          gt('qty', 20),
        ]),
      );
      expect(r.length).toBe(2);
    });

    it('filters: nested AND/OR', async () => {
      const r = await filteredRead(
        and([
          or([
            eq('tag', 'red'),
            eq('tag', 'blue'),
          ]),
          gte('qty', 5),
        ]),
      );
      expect(r.length).toBe(3);
    });

    it('filters: nested field path', async () => {
      const r = await filteredRead(eq(['addr', 'city'], 'NYC'));
      expect(r.length).toBe(2);
      const ids = r.map((x: Record<string, unknown>) => x.id).sort();
      expect(ids).toContain('a');
      expect(ids).toContain('c');
    });

    // ── 5. Projections + aggregations (port 06) ─────────────────────────

    let aggDb: string;

    it('agg: setup orders', async () => {
      aggDb = await setupDb(client!, 'agg', ['orders']);
      await seed(client!, aggDb, 'orders', [
        { id: 'o1', user: 'alice', amount: 100, region: 'eu' },
        { id: 'o2', user: 'alice', amount: 200, region: 'eu' },
        { id: 'o3', user: 'bob', amount: 50, region: 'us' },
        { id: 'o4', user: 'bob', amount: 75, region: 'us' },
        { id: 'o5', user: 'carol', amount: 500, region: 'eu' },
      ]);
    });

    it('agg: select specific fields (column projection)', async () => {
      const resp = br(await Batch.create('proj')
        .add('r', Query.from('orders').select([field('user'), field('amount')]))
        .execute(client!, aggDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      for (const r of recs) {
        expect('user' in r).toBe(true);
        expect('amount' in r).toBe(true);
        expect('id' in r).toBe(false);
        expect('region' in r).toBe(false);
      }
    });

    it('agg: count_all aggregate', async () => {
      const resp = br(await Batch.create('cnt')
        .add('c', Query.from('orders').select([countAll('n')]))
        .execute(client!, aggDb));
      const r = resp.results.c.records;
      expect(r.length).toBe(1);
      expect(r[0].n).toBe(5);
    });

    it('agg: sum + avg + min + max', async () => {
      const resp = br(await Batch.create('sums')
        .add('s', Query.from('orders').select([
          sum('amount', { alias: 'total' }),
          avg('amount', { alias: 'mean' }),
          min('amount', { alias: 'lo' }),
          max('amount', { alias: 'hi' }),
        ]))
        .execute(client!, aggDb));
      const r = resp.results.s.records[0];
      expect(r.total).toBe(925);
      expect(r.mean).toBe(185);
      expect(r.lo).toBe(50);
      expect(r.hi).toBe(500);
    });

    it('agg: group_by user -> count + sum', async () => {
      const resp = br(await Batch.create('gb')
        .add('g', Query.from('orders')
          .groupBy('user')
          .select([
            field('user'),
            countAll('n_orders'),
            sum('amount', { alias: 'total' }),
          ])
          .orderByAsc('user'))
        .execute(client!, aggDb));
      const recs = resp.results.g.records;
      expect(recs.length).toBe(3);
      expect(recs[0].user).toBe('alice');
      expect(recs[0].n_orders).toBe(2);
      expect(recs[0].total).toBe(300);
      expect(recs[1].user).toBe('bob');
      expect(recs[1].total).toBe(125);
      expect(recs[2].user).toBe('carol');
      expect(recs[2].total).toBe(500);
    });

    // ── 6. Sorting + pagination (port 07) ───────────────────────────────

    let pageDb: string;
    const PN = 20;

    it('sort/page: setup 20 records', async () => {
      pageDb = await setupDb(client!, 'page', ['items']);
      const records: object[] = [];
      for (let i = 0; i < PN; i += 1) {
        records.push({
          id: `r${String(i).padStart(2, '0')}`,
          score: (i * 7) % 100,
          bucket: i % 3,
        });
      }
      await seed(client!, pageDb, 'items', records);
    });

    it('sort/page: order_by score asc', async () => {
      const resp = br(await Batch.create('asc')
        .add('r', Query.from('items').orderByAsc('score'))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(PN);
      for (let i = 1; i < recs.length; i += 1) {
        expect(recs[i - 1].score <= recs[i].score).toBe(true);
      }
    });

    it('sort/page: order_by score desc', async () => {
      const resp = br(await Batch.create('desc')
        .add('r', Query.from('items').orderByDesc('score'))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      for (let i = 1; i < recs.length; i += 1) {
        expect(recs[i - 1].score >= recs[i].score).toBe(true);
      }
    });

    it('sort/page: order_by multiple fields (bucket asc, score desc)', async () => {
      const resp = br(await Batch.create('multi')
        .add('r', Query.from('items')
          .orderBy([
            { field: ['bucket'], direction: 'asc' },
            { field: ['score'], direction: 'desc' },
          ]))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      for (let i = 1; i < recs.length; i += 1) {
        const prev = recs[i - 1];
        const cur = recs[i];
        if (prev.bucket === cur.bucket) {
          expect(prev.score >= cur.score).toBe(true);
        } else {
          expect(prev.bucket < cur.bucket).toBe(true);
        }
      }
    });

    it('sort/page: LIMIT/OFFSET first page', async () => {
      const resp = br(await Batch.create('p1')
        .add('r', Query.from('items')
          .orderByAsc('id')
          .limit(5)
          .offset(0))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      expect(recs[0].id).toBe('r00');
      expect(recs[4].id).toBe('r04');
    });

    it('sort/page: LIMIT/OFFSET second page', async () => {
      const resp = br(await Batch.create('p2')
        .add('r', Query.from('items')
          .orderByAsc('id')
          .limit(5)
          .offset(5))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      expect(recs[0].id).toBe('r05');
      expect(recs[4].id).toBe('r09');
    });

    it('sort/page: LIMIT past end', async () => {
      const resp = br(await Batch.create('p-end')
        .add('r', Query.from('items')
          .orderByAsc('id')
          .limit(5)
          .offset(18))
        .execute(client!, pageDb));
      expect(resp.results.r.records.length).toBe(2);
    });

    it('sort/page: count_total returns full size with paginated records', async () => {
      const resp = br(await Batch.create('ct')
        .add('r', Query.from('items')
          .where(gte('score', 50))
          .limit(3)
          .offset(0)
          .countTotal())
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      const pag = resp.results.r.pagination;
      expect(recs.length).toBe(3);
      expect(pag).toBeDefined();
      expect(typeof pag!.total_count).toBe('number');
      expect(pag!.total_count! > 3).toBe(true);
    });

    // ── 7. Batch multi + deps (port 03/04) ──────────────────────────────

    let multiDb: string;

    it('batch: setup multi tables', async () => {
      multiDb = await setupDb(client!, 'multi', ['users', 'orders', 'products']);
      await seed(client!, multiDb, 'users', [
        { id: 'u1', name: 'Alice' },
        { id: 'u2', name: 'Bob' },
      ]);
      await seed(client!, multiDb, 'orders', [
        { id: 'o1', user_id: 'u1', total: 100 },
        { id: 'o2', user_id: 'u2', total: 50 },
        { id: 'o3', user_id: 'u1', total: 250 },
        { id: 'o4', user_id: 'u1', total: 30 },
      ]);
      await seed(client!, multiDb, 'products', [
        { id: 'p1', name: 'Widget', price: 9.99 },
        { id: 'p2', name: 'Gear', price: 14.5 },
        { id: 'p3', name: 'Sprocket', price: 22.0 },
        { id: 'p4', name: 'Bolt', price: 0.5 },
      ]);
    });

    it('batch: three independent reads return correct counts', async () => {
      const resp = br(await Batch.create('multi-read')
        .add('u', Query.from('users'))
        .add('o', Query.from('orders'))
        .add('p', Query.from('products'))
        .execute(client!, multiDb));
      expect(Object.keys(resp.results).length).toBe(3);
      expect(resp.results.u.records.length).toBe(2);
      expect(resp.results.o.records.length).toBe(4);
      expect(resp.results.p.records.length).toBe(4);
    });

    it('batch: execution_plan groups independent queries into one stage', async () => {
      const resp = br(await Batch.create('stages')
        .add('u', Query.from('users'))
        .add('o', Query.from('orders'))
        .add('p', Query.from('products'))
        .execute(client!, multiDb));
      const plan = resp.execution_plan;
      expect(Array.isArray(plan)).toBe(true);
      expect(plan.length).toBe(1);
      expect(plan[0].length).toBe(3);
    });

    it('batch: parent -> child via $query reference', async () => {
      const resp = br(await client.execute(multiDb, {
        id: 'parent-child',
        queries: {
          user: {
            from: 'users',
            where: { op: 'eq', field: ['name'], value: 'Alice' },
          },
          orders: {
            from: 'orders',
            where: {
              op: 'eq',
              field: ['user_id'],
              value: { $query: '@user', path: '[0].id' },
            },
          },
        },
      }));
      const orders = resp.results.orders.records;
      expect(orders.length).toBe(3);
      for (const o of orders) {
        expect(o.user_id).toBe('u1');
      }
    });

    it('batch: execution_plan reflects dep (two stages)', async () => {
      const resp = br(await client.execute(multiDb, {
        id: 'plan-shape',
        queries: {
          user: { from: 'users', where: { op: 'eq', field: ['id'], value: 'u1' } },
          orders: {
            from: 'orders',
            where: {
              op: 'eq',
              field: ['user_id'],
              value: { $query: '@user', path: '[0].id' },
            },
          },
        },
      }));
      const plan = resp.execution_plan;
      expect(plan.length).toBe(2);
      expect(plan[0][0]).toBe('user');
      expect(plan[1][0]).toBe('orders');
    });

    it('batch: column ref via @alias[].field — IN expansion', async () => {
      const resp = br(await client.execute(multiDb, {
        id: 'array-ref',
        queries: {
          all_users: { from: 'users' },
          their_orders: {
            from: 'orders',
            where: {
              op: 'in',
              field: ['user_id'],
              values: [{ $query: '@all_users', path: '[].id' }],
            },
          },
        },
      }));
      expect(resp.results.their_orders.records.length).toBe(4);
    });

    // ── 8. Admin DDL + HMAC (port 08/12) ────────────────────────────────

    it('DDL: list databases includes default', async () => {
      const resp = br(await Batch.create('lsdb')
        .add('l', listDatabases())
        .execute(client!, 'default'));
      const names = resp.results.l.records[0].databases as string[];
      expect(Array.isArray(names)).toBe(true);
      expect(names).toContain('default');
    });

    it('DDL: create_index + list + drop_index (hmac)', async () => {
      const idxDb = await setupDb(client!, 'ddl_idx', ['t']);

      await br(await Batch.create('mk-idx')
        .add('i', createIndex('by_email', 't', [['email']]))
        .execute(client!, idxDb));

      const lsResp = br(await Batch.create('ls-idx')
        .add('l', listIndexes('t'))
        .execute(client!, idxDb));
      const indexNames = (lsResp.results.l.records[0].indexes as Array<{ name: string }>).map(i => i.name);
      expect(indexNames).toContain('by_email');

      // Drop with HMAC — client IS the HmacSigner
      await br(await Batch.create('rm-idx')
        .add('d', dropIndex(client!, idxDb, 'main', 't', 'by_email'))
        .execute(client!, idxDb));

      const ls2 = br(await Batch.create('ls-idx2')
        .add('l', listIndexes('t'))
        .execute(client!, idxDb));
      const afterNames = (ls2.results.l.records[0].indexes as Array<{ name: string }>).map(i => i.name);
      expect(afterNames).not.toContain('by_email');
    });

    it('HMAC: drop_table without hmac -> hmac_required', async () => {
      const hmacDb = await setupDb(client!, 'hmac_miss', ['t']);
      await expect(
        client!.execute(hmacDb, {
          id: 1,
          queries: { d: { drop_table: 't', repo: 'main' } },
        }),
      ).rejects.toThrow(/hmac_required/);
    });

    it('HMAC: drop_table with wrong hmac -> hmac_mismatch', async () => {
      const hmacDb = await setupDb(client!, 'hmac_wrong', ['t']);
      await expect(
        client!.execute(hmacDb, {
          id: 1,
          queries: {
            d: {
              drop_table: 't',
              repo: 'main',
              hmac: 'aa'.repeat(32),
            },
          },
        }),
      ).rejects.toThrow(/hmac_mismatch/);
    });

    it('HMAC: drop_table with correct hmac succeeds', async () => {
      const hmacDb = await setupDb(client!, 'hmac_ok', ['t']);
      const resp = br(await Batch.create('drop-ok')
        .add('d', dropTable(client!, hmacDb, 'main', 't'))
        .execute(client!, hmacDb));
      const row = resp.results.d.records[0] as Record<string, unknown>;
      expect(row.dropped_table).toBe('t');
      expect(row.existed).toBe(true);
    });

    it('HMAC: drop_db without hmac -> hmac_required', async () => {
      const victim = await setupDb(client!, 'hmac_miss_db', []);
      await expect(
        client!.execute('default', {
          id: 1,
          queries: { d: { drop_db: victim } },
        }),
      ).rejects.toThrow(/hmac_required/);
    });

    it('HMAC: drop_db with correct hmac + cascade succeeds', async () => {
      const victim = await setupDb(client!, 'hmac_ok_db', []);
      const resp = br(await Batch.create('drop-db-ok')
        .add('d', dropDb(client!, victim, { cascade: true }))
        .execute(client!, 'default'));
      const row = resp.results.d.records[0] as Record<string, unknown>;
      expect(row.dropped).toBe(victim);
    });

    // ── 9. Transactions (port 15) ───────────────────────────────────────

    let txDb: string;

    it('tx: setup items + logs', async () => {
      txDb = await setupDb(client!, 'tx_e2e', ['items', 'logs']);
    });

    it('tx: transactional insert + read returns committed data', async () => {
      const ins = br(await Batch.create('tx-si-1-ins')
        .add('ins', insert('items', [{ name: 'widget', qty: 10 }]))
        .transactional()
        .execute(client!, txDb));
      expect(ins.transaction).toBeDefined();
      expect(ins.transaction!.status).toBe('committed');
      expect(ins.transaction!.tx_id).toBeGreaterThan(0);
      expect(ins.transaction!.commit_version).toBeGreaterThan(0);
      expect(ins.results.ins.records.length).toBeGreaterThanOrEqual(1);

      const readResp = br(await Batch.create('tx-si-1-read')
        .add('read', Query.from('items'))
        .execute(client!, txDb));
      const recs = readResp.results.read.records;
      expect(recs.length).toBeGreaterThanOrEqual(1);
      const names = recs.map(r => r.name);
      expect(names).toContain('widget');
    });

    it('tx: cross-table insert is atomic', async () => {
      const resp = br(await Batch.create('tx-cross-table')
        .add('ins_items', insert('items', [{ name: 'cross-item' }]))
        .add('ins_logs', insert('logs', [{ event: 'item_created' }]))
        .transactional()
        .execute(client!, txDb));
      expect(resp.transaction!.status).toBe('committed');
      expect(resp.results.ins_items.records.length).toBeGreaterThanOrEqual(1);
      expect(resp.results.ins_logs.records.length).toBeGreaterThanOrEqual(1);
    });

    it('tx: isolation serializable accepted', async () => {
      const resp = br(await Batch.create('tx-ssi')
        .add('ins', insert('items', [{ name: 'ssi-item' }]))
        .transactional('serializable')
        .execute(client!, txDb));
      expect(resp.transaction!.status).toBe('committed');
    });

    it('tx: non-tx insert works alongside tx infra', async () => {
      const resp = br(await Batch.create('non-tx')
        .add('ins', insert('items', [{ name: 'plain-item' }]))
        .execute(client!, txDb));
      expect(!resp.transaction || resp.transaction === undefined).toBe(true);
      expect(resp.results.ins.records.length).toBeGreaterThanOrEqual(1);
    });
  },
);

describe('e2e.test skip reason', () => {
  it('reports why the e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        `[e2e.test] SKIPPED — server binary not found at:\n  ${SERVER_BIN}\n` +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
