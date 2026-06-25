/**
 * Shared e2e test harness for ShamirDB TS client tests.
 *
 * Provides server lifecycle management with ephemeral port allocation,
 * CARGO_TARGET_DIR-aware binary resolution, TLS cert generation,
 * and common helpers (br, uniqueDbName, setupDb, seed, connectAdmin, connectAs).
 */

import { spawn, ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';

import { connect } from '../index.js';
import type { ShamirClient, BatchResponse, WireValue } from '../index.js';
import { ddl, write } from '../index.js';

// ─── server binary path (CARGO_TARGET_DIR-aware) ─────────────────────────────

const _fileUrl = new URL(import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1');
const REPO_ROOT = path.resolve(path.dirname(_fileUrl), '../../../..');

const EXE_NAME = process.platform === 'win32' ? 'shamir-server.exe' : 'shamir-server';

/**
 * Resolve the server binary path.
 *
 * 1. If SHAMIR_SERVER_BIN is set, use it verbatim — an explicit override that
 *    lets a faster `cargo build` (debug) profile feed the e2e suite without a
 *    20-minute release build. Highest priority.
 * 2. If CARGO_TARGET_DIR is set and the release binary exists there, use it.
 * 3. Otherwise fall back to <repo>/target/release/<exe>.
 *
 * Prefers CARGO_TARGET_DIR when both exist — `target/release` may contain
 * a stale binary that lacks recent server features.
 */
export function serverBinPath(): string {
  const explicit = process.env.SHAMIR_SERVER_BIN;
  if (explicit) return explicit;

  const cargoTargetDir = process.env.CARGO_TARGET_DIR;
  if (cargoTargetDir) {
    const candidate = path.join(cargoTargetDir, 'release', EXE_NAME);
    if (fs.existsSync(candidate)) return candidate;
  }
  return path.join(REPO_ROOT, 'target', 'release', EXE_NAME);
}

export const SERVER_BIN = serverBinPath();
export const SERVER_AVAILABLE = fs.existsSync(SERVER_BIN);

// ─── constants ───────────────────────────────────────────────────────────────

export const HOST = '127.0.0.1';
export const ADMIN_USER = 'admin';
export const ADMIN_PASSWORD = 'correct horse battery staple';
export const ORIGIN = `https://${HOST}`;

// ─── types ───────────────────────────────────────────────────────────────────

export interface ServerHandle {
  stop: () => Promise<void>;
  logs: () => string;
  port: number;
}

// ─── ephemeral port ──────────────────────────────────────────────────────────

/**
 * Find a free TCP port by binding to port 0, reading the assigned port,
 * then closing the server. The OS guarantees uniqueness at the moment of
 * allocation; a brief TOCTOU window exists but is negligible in practice
 * for test-local servers.
 */
async function getFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.listen(0, HOST, () => {
      const addr = srv.address();
      if (!addr || typeof addr === 'string') {
        srv.close(() => reject(new Error('unexpected address type')));
        return;
      }
      const port = addr.port;
      srv.close(() => resolve(port));
    });
    srv.on('error', reject);
  });
}

// ─── config / cert ───────────────────────────────────────────────────────────

export function writeKtavConfig(
  dir: string,
  opts: { host: string; port: number; origin: string },
): string {
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
        addr: ${opts.host}:${opts.port}
        path: /shamir/v1/browser
        profile: tls_no_export
        browser_origin_allowlist: [
            ${opts.origin}
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

export function generateSelfSignedCert(dir: string): boolean {
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

// ─── server lifecycle ────────────────────────────────────────────────────────

export async function startServer(opts?: {
  port?: number;
}): Promise<ServerHandle> {
  const port = opts?.port ?? await getFreePort();
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-ts-e2e-'));
  const hasCert = generateSelfSignedCert(dataDir);
  if (!hasCert) {
    fs.rmSync(dataDir, { recursive: true, force: true });
    throw new Error('openssl not available — cannot generate self-signed cert');
  }

  const configPath = writeKtavConfig(dataDir, { host: HOST, port, origin: ORIGIN });
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
    port,
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

// ─── connection helpers ──────────────────────────────────────────────────────

export async function connectAdmin(host: string, port: number): Promise<ShamirClient> {
  return connect({
    host,
    port,
    username: ADMIN_USER,
    password: ADMIN_PASSWORD,
    tls: { rejectUnauthorized: false },
    origin: ORIGIN,
  });
}

export async function connectAs(
  host: string,
  port: number,
  username: string,
  password: string,
): Promise<ShamirClient> {
  return connect({
    host,
    port,
    username,
    password,
    tls: { rejectUnauthorized: false },
    origin: ORIGIN,
  });
}

// ─── data helpers ────────────────────────────────────────────────────────────

/**
 * Unwrap the server's DbResponse envelope.
 */
export function br(raw: object): BatchResponse {
  const env = raw as { kind?: string; response?: BatchResponse };
  if (env.kind === 'batch' && env.response) return env.response;
  if ('results' in raw && 'execution_plan' in raw) return raw as BatchResponse;
  throw new Error(`unexpected response shape: ${JSON.stringify(Object.keys(raw))}`);
}

let dbCounter = 0;

export function uniqueDbName(label: string): string {
  dbCounter += 1;
  return `ts_${label}_${process.pid}_${dbCounter}`;
}

export async function setupDb(
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

export async function seed(
  client: ShamirClient,
  db: string,
  table: string,
  records: Array<Record<string, WireValue>>,
  keyFields: string[] = ['id'],
): Promise<BatchResponse> {
  const queries: Record<string, object> = {};
  records.forEach((r, i) => {
    const key: Record<string, WireValue> = {};
    for (const k of keyFields) key[k] = r[k];
    queries[`s${i}`] = write.upsert(table, key, r);
  });
  return br(await client.execute(db, { id: `seed-${db}-${table}`, queries }));
}
