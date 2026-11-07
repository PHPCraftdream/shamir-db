import { spawn, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import { Batch, admin, connectLocal, ddl, write } from '../../../../index.js';
import type { ShamirClient } from '../../../../index.js';
import {
  ADMIN_PASSWORD,
  ADMIN_USER,
  HOST,
  SERVER_AVAILABLE,
  SERVER_BIN,
  assertServerBinaryFresh,
  br,
  generateSelfSignedCert,
} from '../../../e2e-harness.js';

export const ORIGIN = 'https://localhost';
export const SERVICE_USER = `token_svc_${process.pid}`;
export const SERVICE_PASSWORD = 'token service password';

export interface TokenFixtureOptions {
  loginTtl?: number;
  loginEnabled?: boolean;
  loginAllowReadWrite?: boolean;
  loginAuthorityAllowlist?: readonly string[];
  /** Server log level (`info` by default); `debug` records the subscription pipeline step by step. */
  logLevel?: 'info' | 'debug' | 'trace';
}

export interface TokenServer {
  dir: string;
  ipc: string;
  http: number;
  https: number;
  /** WebSocket SCRAM listener (TLS, no exporter): the session type `changePassword` supports. */
  scramWs: number;
  child: ChildProcess;
  logs: () => string;
  stop: () => Promise<void>;
}

function ipcAddress(dir: string): string {
  return process.platform === 'win32'
    ? `\\\\.\\pipe\\shamir-token-e2e-${path.basename(dir)}`
    : path.join(dir, 'token.sock');
}

function writeConfig(dir: string, ipc: string, options: TokenFixtureOptions = {}): string {
  const cert = path.join(dir, 'cert.pem').replace(/\\/g, '/');
  const key = path.join(dir, 'key.pem').replace(/\\/g, '/');
  const cfg = `data_dir: ${dir.replace(/\\/g, '/')}
logging: {
    level: ${options.logLevel ?? 'info'}
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
        kind: unix
        addr: ${ipc}
        profile: plain
    }
    {
        kind: http
        addr: ${HOST}:0
        tls: false
        max_request_body_bytes: 1024
        head_timeout_ms: 5000
        body_timeout_ms: 30000
        request_timeout_ms: 60000
        login: {
            enabled: ${options.loginEnabled ?? true}
            authority_allowlist: [${(options.loginAuthorityAllowlist ?? []).join(', ')}]
        }
        host_authority_allowlist: [localhost]
        browser_origin_allowlist: [${ORIGIN}]
    }
    {
        kind: http
        addr: ${HOST}:0
        tls: true
        max_request_body_bytes: 1024
        head_timeout_ms: 5000
        body_timeout_ms: 30000
        request_timeout_ms: 60000
        login: {
            enabled: ${options.loginEnabled ?? true}
            authority_allowlist: [${(options.loginAuthorityAllowlist ?? []).join(', ')}]
        }
        host_authority_allowlist: [localhost]
        browser_origin_allowlist: [${ORIGIN}]
    }
    {
        kind: ws
        addr: ${HOST}:0
        path: /shamir/v1/browser
        profile: tls_no_export
        browser_origin_allowlist: [${ORIGIN}]
    }
]
tls: {
    cert_path: ${cert}
    key_path: ${key}
}
security: {
    connection: {
        auth_init_timeout_ms: 10000
        max_active_connections: 100
    }
    auth_init_rate_per_second: 1000
    query_limits: {
        max_result_size_bytes: 10485760
        max_execution_time_secs: 30
        max_queries_per_batch: 32
    }
    cursors: {
        max_cursors_per_session: 1
        idle_timeout_secs: 60
    }
}
tokens: {
    login_ttl: ${options.loginTtl ?? 86400}
    login_allow_read_write: ${options.loginAllowReadWrite ?? false}
}
audit: {
    max_file_size_mb: 0
    retention_days: 0
}
observability: {
    addr: 127.0.0.1:0
}
`;
  const configPath = path.join(dir, 'server.ktav');
  fs.writeFileSync(configPath, cfg);
  return configPath;
}

function readBoundListeners(logs: string, ipc: string): { http: number; https: number; scramWs: number } | undefined {
  const lines = logs.replace(/\x1b\[[0-9;]*m/g, '').split('\n');
  const complete = lines.slice(0, -1).filter((line) => /listener bound/i.test(line));
  const ipcBound = complete.some((line) => /kind=unix/i.test(line) && line.includes(ipc));
  const httpAddresses: string[] = [];
  const wsAddresses: string[] = [];
  for (const line of complete) {
    const addressMatch = /local_addr=([^\s,]+)/.exec(line);
    if (!addressMatch) continue;
    const address = addressMatch[1]!.replace(/^"|"$/g, '');
    if (/kind="http"/i.test(line)) httpAddresses.push(address);
    else if (/kind="?ws"?/i.test(line)) wsAddresses.push(address);
  }
  if (!ipcBound || httpAddresses.length < 2 || wsAddresses.length < 1) return undefined;
  const first = httpAddresses[0]!;
  const second = httpAddresses[1]!;
  const httpPort = /:(\d+)$/.exec(first)?.[1];
  const httpsPort = /:(\d+)$/.exec(second)?.[1];
  const wsPort = /:(\d+)$/.exec(wsAddresses[0]!)?.[1];
  if (!httpPort || !httpsPort || !wsPort || httpPort === httpsPort) return undefined;
  return { http: Number(httpPort), https: Number(httpsPort), scramWs: Number(wsPort) };
}

export async function startTokenServer(options: TokenFixtureOptions = {}): Promise<TokenServer> {
  assertServerBinaryFresh();
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-token-e2e-'));
  const ipc = ipcAddress(dir);
  let child: ChildProcess | undefined;
  let logs = '';
  try {
    if (!generateSelfSignedCert(dir)) throw new Error('openssl unavailable for token e2e TLS listener');
    const configPath = writeConfig(dir, ipc, options);
    child = spawn(SERVER_BIN, ['--config', configPath, '--bootstrap-password', ADMIN_PASSWORD], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout?.on('data', (chunk: Buffer) => { logs += chunk.toString(); });
    child.stderr?.on('data', (chunk: Buffer) => { logs += chunk.toString(); });
    const proc = child;
    const ports = await new Promise<{ http: number; https: number; scramWs: number }>((resolve, reject) => {
      let settled = false;
      const finish = (error?: Error, value?: { http: number; https: number; scramWs: number }): void => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        proc.stdout?.removeListener('data', check);
        proc.stderr?.removeListener('data', check);
        proc.removeListener('error', onError);
        proc.removeListener('exit', onExit);
        error ? reject(error) : resolve(value!);
      };
      const check = (): void => {
        const lines = logs.replace(/\x1b\[[0-9;]*m/g, '').split('\n');
        const completeLogs = `${lines.slice(0, -1).join('\n')}\n`;
        const result = readBoundListeners(completeLogs, ipc);
        if (result) finish(undefined, result);
      };
      const onError = (error: Error): void => finish(error);
      const onExit = (code: number | null, signal: NodeJS.Signals | null): void => finish(new Error(`server exited (${code}/${signal})\n${logs}`));
      const timer = setTimeout(() => finish(new Error(`listener bind deadline exceeded\n${logs}`)), 15000);
      proc.stdout?.on('data', check);
      proc.stderr?.on('data', check);
      proc.once('error', onError);
      proc.once('exit', onExit);
      check();
    });
    return { dir, ipc, ...ports, child: proc, logs: () => logs, stop: () => stop(proc, dir) };
  } catch (error) {
    if (child) await stop(child, dir);
    else fs.rmSync(dir, { recursive: true, force: true });
    throw error;
  }
}

async function waitForExit(child: ChildProcess, timeoutMs: number): Promise<boolean> {
  if (child.exitCode !== null || child.signalCode !== null) return true;
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const finish = (exited: boolean): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.removeListener('exit', onExit);
      resolve(exited);
    };
    const onExit = (): void => finish(true);
    const timer = setTimeout(() => finish(false), timeoutMs);
    child.once('exit', onExit);
  });
}

async function stop(child: ChildProcess, dir: string): Promise<void> {
  if (child.pid === undefined) {
    fs.rmSync(dir, { recursive: true, force: true });
    return;
  }
  if (child.exitCode === null && child.signalCode === null) {
    child.kill('SIGTERM');
    let exited = await waitForExit(child, 5000);
    if (!exited) {
      child.kill('SIGKILL');
      exited = await waitForExit(child, 5000);
    }
    if (!exited) throw new Error('shamir-server did not exit before bounded shutdown deadline');
  }
  fs.rmSync(dir, { recursive: true, force: true });
}

export async function setupTokenFixture(options: TokenFixtureOptions = {}): Promise<{ server: TokenServer; scram: ShamirClient; admin: ShamirClient; db: string }> {
  const server = await startTokenServer(options);
  let scram: ShamirClient | undefined;
  try {
    scram = await connectLocal({
      addr: server.ipc,
      username: ADMIN_USER,
      password: ADMIN_PASSWORD,
      connectTimeoutMs: 10000,
      requestTimeoutMs: 10000,
    });
    await scram.createScramUser(SERVICE_USER, SERVICE_PASSWORD, []);
    const tableSetupBatch = Batch.create('token-table-acl')
      .add('table', ddl.createTable('items', { repo: 'main' }))
      .add('database_mode', admin.chmod(scram, admin.refDatabase('default'), 0o757))
      .add('store_mode', admin.chmod(scram, admin.refStore('default', 'main'), 0o711));
    br(await tableSetupBatch.execute(scram, 'default'));
    const tableModeBatch = Batch.create('token-table-mode')
      .add('table_mode', admin.chmod(scram, admin.refTable('default', 'main', 'items'), 0o777));
    br(await tableModeBatch.execute(scram, 'default'));
    const seedBatch = Batch.create('token-seed').add('rows', write.insert('items', [
      {
        id: 'row-a',
        value: 'alpha',
      },
      {
        id: 'row-b',
        value: 'bravo',
      },
      {
        id: 'row-c',
        value: 'charlie',
      },
      {
        id: 'row-d',
        value: 'delta',
      },
    ]));
    br(await seedBatch.execute(scram, 'default'));
    return { server, scram, admin: scram, db: 'default' };
  } catch (error) {
    if (scram) await scram.close().catch(() => undefined);
    await server.stop();
    throw error;
  }
}

export { SERVER_AVAILABLE, HOST, br };
