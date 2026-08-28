/**
 * End-to-end test — `connectLocal` (local IPC transport, spec
 * TRANSPORT_UNIX.md) against a live `shamir-server`.
 *
 * Self-contained rather than reusing `e2e-harness.ts`'s `startServer` /
 * `writeKtavConfig`: those are shaped for the `kind: ws` browser listener
 * every other `e2e-*.test.ts` file connects through. A `kind: unix`
 * listener needs its own ktav block (no `path`/`browser_origin_allowlist`,
 * `profile: plain` instead of `tls_no_export`) and its own endpoint
 * addressing (a socket path / pipe name, not a host:port) — reusing the
 * generically-applicable pieces of the harness (`SERVER_BIN`,
 * `assertServerBinaryFresh`, `ADMIN_USER`/`ADMIN_PASSWORD`, spawn/wait/
 * teardown shape) while writing the `kind: unix` config and connection
 * step directly, mirroring `crates/shamir-client/tests/smoke_local.rs`
 * (the Rust SDK's equivalent test).
 */

import { spawn, ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { connectLocal, ddl, write, Query, Batch } from '../index.js';
import {
  SERVER_BIN,
  SERVER_AVAILABLE,
  assertServerBinaryFresh,
  ADMIN_USER,
  ADMIN_PASSWORD,
  br,
  uniqueDbName,
} from './e2e-harness.js';

// ─── kind: unix server spawn ────────────────────────────────────────────────

/**
 * Local-IPC endpoint. On Unix, a socket path inside `dataDir`. On Windows,
 * a Named Pipe name — unique per process (there is no filesystem meaning
 * to a pipe name, so `process.pid` is the uniqueness source instead).
 */
function ipcAddr(dataDir: string): string {
  if (process.platform === 'win32') {
    return `\\\\.\\pipe\\shamir-ts-e2e-${process.pid}`;
  }
  return path.join(dataDir, 'shamir-ts-e2e.sock');
}

function writeUnixKtavConfig(dir: string, addr: string): string {
  // `load_or_generate` (server_launcher.rs) self-signs a cert at these
  // paths on boot regardless of whether any TLS-bearing listener is
  // configured — no `openssl`/pre-generated cert needed for a
  // `kind: unix`-only deployment, same as the Rust `smoke_local.rs`
  // fixture.
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
        kind: unix
        addr: ${addr}
        profile: plain
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
    auth_init_rate_per_second: 1000
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

interface LocalServerHandle {
  addr: string;
  stop: () => Promise<void>;
  logs: () => string;
}

async function startUnixServer(): Promise<LocalServerHandle> {
  assertServerBinaryFresh();
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-ts-e2e-unix-'));
  const addr = ipcAddr(dataDir);
  const configPath = writeUnixKtavConfig(dataDir, addr);

  const child = spawn(
    SERVER_BIN,
    ['--config', configPath, '--bootstrap-password', ADMIN_PASSWORD],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  ) as ChildProcess;

  let logBuf = '';
  child.stdout?.on('data', (chunk: Buffer) => {
    logBuf += chunk.toString();
  });
  child.stderr?.on('data', (chunk: Buffer) => {
    logBuf += chunk.toString();
  });

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
        reject(
          new Error(`Server exited prematurely (code=${code} signal=${signal}).\nLogs:\n${logBuf}`),
        );
      }
    });
  });

  return {
    addr,
    stop: async () => {
      if (!child.killed) {
        child.kill(process.platform === 'win32' ? 'SIGKILL' : 'SIGTERM');
        await new Promise<void>((r) => child.once('exit', r));
      }
      try {
        fs.rmSync(dataDir, { recursive: true, force: true });
      } catch {
        /* ok */
      }
    },
    logs: () => logBuf,
  };
}

// ─── test suite ─────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e connectLocal — local IPC transport (requires release binary)',
  () => {
    let server: LocalServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startUnixServer();
      try {
        client = await connectLocal({
          addr: server.addr,
          username: ADMIN_USER,
          password: ADMIN_PASSWORD,
        });
      } catch (e) {
        console.error('[e2e-connect-local] connection failed. Server logs:\n' + server.logs());
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try {
          await client.close();
        } catch {
          /* ok */
        }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    it('connect: session id is 32 bytes, TOFU pin captured', () => {
      expect(client).not.toBeNull();
      expect(client!.sessionId().length).toBe(32);
      expect(client!.serverPubKeyPin().some((b) => b !== 0)).toBe(true);
    });

    it('server issues a resumption ticket over the unix transport', () => {
      expect(client!.resumptionTicket()).toBeDefined();
    });

    it('ping / create db / create table / write + read round-trip', async () => {
      const c = client!;

      const db = uniqueDbName('ipc');
      await br(
        await Batch.create('mk-db').add('mk', ddl.createDb(db)).execute(c, 'default'),
      );
      await br(
        await Batch.create('mk-table')
          .add('mr', ddl.createRepo('main'))
          .add('tb', ddl.createTable('items', { repo: 'main' }))
          .execute(c, db),
      );

      const resp = br(
        await Batch.create('rw')
          .add('ins', write.upsert('items', { sku: 'X1' }, { sku: 'X1', qty: 42 }))
          .add('rd', Query.from('items'))
          .execute(c, db),
      );

      const rd = resp.results.rd;
      expect(rd).toBeDefined();
      expect(rd!.records.length).toBe(1);
      expect(rd!.records[0].sku).toBe('X1');
      expect(rd!.records[0].qty).toBe(42);
    });
  },
);
