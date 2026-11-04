/**
 * Connection-matrix server helper.
 *
 * Spawns the prebuilt `shamir-server` with SIX listeners at once (TCP+TLS,
 * WebSocket+TLS, plain loopback TCP, HTTPS and plain HTTP with password login,
 * local IPC) so one process serves every napi connection variant.
 * Separate from helpers/server.js so the shared single-listener server used
 * by the other suites stays untouched.
 */

'use strict';

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const SERVER_BIN = path.join(
  REPO_ROOT,
  'target',
  'release',
  process.platform === 'win32' ? 'shamir-server.exe' : 'shamir-server'
);

const { ADMIN_USER, ADMIN_PASSWORD } = require('./server');

const HOST = '127.0.0.1';
const DEFAULT_PORT_BASE = 14200;
const LISTENER_COUNT = 6;
const ANSI_RE = /\x1b\[[0-9;]*m/g;

function portBase() {
  const raw = process.env.SHAMIR_E2E_MATRIX_PORT_BASE;
  if (raw === undefined || raw === '') return DEFAULT_PORT_BASE;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 1024 || n > 65530) {
    throw new Error(`SHAMIR_E2E_MATRIX_PORT_BASE must be an integer in 1024..65530, got "${raw}"`);
  }
  return n;
}

function ipcAddress(dataDir) {
  if (process.platform === 'win32') {
    return `\\\\.\\pipe\\shamir-napi-e2e-${process.pid}-${Date.now()}`;
  }
  return path.join(dataDir, `shamir-napi-e2e-${process.pid}.sock`);
}

function writeKtavConfig(dir, ports, ipcAddr) {
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
        kind: tcp
        addr: ${HOST}:${ports.tcp}
        profile: tls_exporter
    }
    {
        kind: ws
        addr: ${HOST}:${ports.ws}
        path: /shamir/v1
        profile: tls_exporter
    }
    {
        kind: tcp
        addr: ${HOST}:${ports.plain}
        profile: plain
    }
    {
        kind: http
        addr: ${HOST}:${ports.httpTls}
        tls: true
        login: {
            enabled: true
        }
    }
    {
        kind: http
        addr: ${HOST}:${ports.httpPlain}
        tls: false
        login: {
            enabled: true
        }
    }
    {
        kind: unix
        addr: ${ipcAddr}
        profile: plain
    }
]

tls: {
    cert_path: ${path.join(dir, 'cert.pem').replace(/\\/g, '/')}
    key_path:  ${path.join(dir, 'key.pem').replace(/\\/g, '/')}
}

security: {
    connection: {
        auth_init_timeout_ms: 5000
        max_active_connections: 100
    }
    ## Every connection dials 127.0.0.1 (one subnet); the default 10/s
    ## auth_init limit would drain on the burst of sessions in this file.
    auth_init_rate_per_second: 1000
    query_limits: {
        max_result_size_bytes:    10485760
        max_execution_time_secs:  10
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

async function readCert(dir) {
  const certPath = path.join(dir, 'cert.pem');
  const deadline = Date.now() + 2000;
  for (;;) {
    try {
      const pem = fs.readFileSync(certPath, 'utf8');
      if (pem.includes('BEGIN CERTIFICATE')) return pem;
    } catch (_) {
      /* not written yet */
    }
    if (Date.now() >= deadline) {
      throw new Error(`cert.pem was not generated at ${certPath} within 2s`);
    }
    await new Promise((r) => setTimeout(r, 100));
  }
}

/**
 * Start the multi-listener server. Returns
 * `{ host, ports: {tcp, ws, plain, httpTls, httpPlain}, ipcAddr, caPem,
 *    user, password, stop, logs }`.
 */
async function startMatrixServer() {
  if (!fs.existsSync(SERVER_BIN)) {
    throw new Error(
      `Server binary not found at ${SERVER_BIN}.\n` +
        `Run \`npm run build:server\` first.`
    );
  }

  const base = portBase();
  const ports = {
    tcp: base,
    ws: base + 1,
    plain: base + 2,
    httpTls: base + 3,
    httpPlain: base + 4,
  };

  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-e2e-matrix-'));
  const ipcAddr = ipcAddress(dataDir);
  const configPath = writeKtavConfig(dataDir, ports, ipcAddr);

  const child = spawn(
    SERVER_BIN,
    ['--config', configPath, '--bootstrap-password', ADMIN_PASSWORD],
    { stdio: ['ignore', 'pipe', 'pipe'] }
  );

  let logBuf = '';
  function onData(chunk) {
    logBuf += chunk.toString();
  }
  child.stdout.on('data', onData);
  child.stderr.on('data', onData);

  async function stop() {
    if (!child.killed && child.exitCode === null) {
      child.kill(process.platform === 'win32' ? 'SIGKILL' : 'SIGTERM');
      await new Promise((r) => child.once('exit', r));
    }
    try {
      fs.rmSync(dataDir, { recursive: true, force: true });
    } catch (_) {
      /* redb files may stay locked briefly on Windows; ignore */
    }
  }

  try {
    await new Promise((resolve, reject) => {
      let resolved = false;
      const timeout = setTimeout(() => {
        if (!resolved) {
          resolved = true;
          reject(new Error(`Matrix server failed to bind ${LISTENER_COUNT} listeners within 20s.\nLogs:\n${logBuf}`));
        }
      }, 20_000);

      function check() {
        if (resolved) return;
        const plain = logBuf.replace(ANSI_RE, '');
        const bound = (plain.match(/listener bound/gi) || []).length;
        if (bound >= LISTENER_COUNT) {
          resolved = true;
          clearTimeout(timeout);
          setTimeout(() => resolve(), 100);
        }
      }
      child.stdout.on('data', check);
      child.stderr.on('data', check);
      child.on('exit', (code, signal) => {
        if (!resolved) {
          resolved = true;
          clearTimeout(timeout);
          reject(
            new Error(
              `Matrix server exited prematurely (code=${code} signal=${signal}).\nLogs:\n${logBuf}`
            )
          );
        }
      });
    });

    const caPem = await readCert(dataDir);

    return {
      host: HOST,
      ports,
      ipcAddr,
      caPem,
      user: ADMIN_USER,
      password: ADMIN_PASSWORD,
      stop,
      logs: () => logBuf,
    };
  } catch (err) {
    await stop();
    throw err;
  }
}

module.exports = { startMatrixServer };
