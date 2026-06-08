/**
 * Integration test — connect via the Node entry point against a real
 * shamir-server subprocess, run a trivial execute, close.
 *
 * Mirrors the approach in tests/e2e/helpers/server.js but uses a `ws`
 * listener (kind: ws, profile: tls_no_export) required by the browser path.
 *
 * Skip-guard: if the server binary is absent, the test is skipped with an
 * explanation.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { spawn, ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { connect } from '../index.js';
import type { ShamirClient } from '../index.js';

// ─── server binary path ───────────────────────────────────────────────────────

// File layout: src/__tests__/connect.test.ts → src → shamir-client-ts → crates → shamir-db
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
const PORT = 13751; // distinct port to avoid collision with other e2e runners
const ADMIN_USER = 'admin';
const ADMIN_PASSWORD = 'correct horse battery staple';
const ORIGIN = `https://${HOST}`;

interface ServerHandle {
  stop: () => Promise<void>;
  logs: () => string;
}

function writeKtavConfig(dir: string): string {
  // Use a self-signed cert generated with openssl if present, otherwise
  // skip the test. The server requires real TLS even for loopback.
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
    throw new Error('openssl not available — cannot generate self-signed cert for integration test');
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

// ─── test suite ───────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'connect integration (requires release binary)',
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
        console.error('[connect.test] connection failed. Server logs:\n' + server.logs());
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

    it('produces a 32-byte session id after connect', () => {
      expect(client).not.toBeNull();
      expect(client!.sessionId().length).toBe(32);
    });

    it('execute (Ping) returns a response object', async () => {
      expect(client).not.toBeNull();
      // Use the Ping DbRequest — zero DB cost, simplest possible response.
      const resp = await client!.ping();
      expect(typeof resp).toBe('object');
    }, 30_000);
  },
);

describe('connect.test skip reason', () => {
  it('reports why the integration test was skipped', () => {
    if (SERVER_AVAILABLE) {
      // Not skipped — integration tests ran above.
      expect(true).toBe(true);
    } else {
      console.warn(
        `[connect.test] SKIPPED — server binary not found at:\n  ${SERVER_BIN}\n` +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false); // document the skip reason
    }
  });
});
