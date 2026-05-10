/**
 * Server lifecycle helper.
 *
 * Spawns the prebuilt `shamir-server` release binary against a fresh
 * tempdir + a generated `.ktav` config, waits for the listener to bind,
 * and returns a handle. `stop()` kills the subprocess and cleans the
 * tempdir.
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

const DEFAULT_HOST = '127.0.0.1';
const DEFAULT_PORT = 13742;
const ADMIN_USER = 'admin';
const ADMIN_PASSWORD = 'correct horse battery staple';

function writeKtavConfig(dir, host, port) {
  const cfg = `
data_dir: ${dir.replace(/\\/g, '/')}

logging: {
    # INFO so we can detect the "listener bound" line; warnings still
    # printed for diagnostics on test failure.
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
        addr: ${host}:${port}
        profile: tls_exporter
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

# Random port to avoid collisions with whatever else might own 9090.
observability: {
    addr: 127.0.0.1:0
}
`.trim();

  const configPath = path.join(dir, 'server.ktav');
  fs.writeFileSync(configPath, cfg);
  return configPath;
}

/**
 * Start a server. Returns `{ host, port, user, password, stop, logs }`.
 * `stop()` is async — kills the subprocess and removes the tempdir.
 */
async function startServer({ host = DEFAULT_HOST, port = DEFAULT_PORT } = {}) {
  if (!fs.existsSync(SERVER_BIN)) {
    throw new Error(
      `Server binary not found at ${SERVER_BIN}.\n` +
        `Run \`npm run build:server\` first.`
    );
  }

  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-e2e-'));
  const configPath = writeKtavConfig(dataDir, host, port);

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

  // Wait for "listener bound" or premature exit.
  const ready = await new Promise((resolve, reject) => {
    let resolved = false;
    const timeout = setTimeout(() => {
      if (!resolved) {
        resolved = true;
        reject(new Error(`Server failed to bind within 10s.\nLogs:\n${logBuf}`));
      }
    }, 10_000);

    function check() {
      if (resolved) return;
      if (/listener bound/i.test(logBuf)) {
        resolved = true;
        clearTimeout(timeout);
        // Brief grace for the accept loop.
        setTimeout(() => resolve(true), 100);
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
            `Server exited prematurely (code=${code} signal=${signal}).\nLogs:\n${logBuf}`
          )
        );
      }
    });
  });
  if (!ready) throw new Error('Unreachable');

  async function stop() {
    if (!child.killed) {
      child.kill(process.platform === 'win32' ? 'SIGKILL' : 'SIGTERM');
      await new Promise((r) => child.once('exit', r));
    }
    try {
      fs.rmSync(dataDir, { recursive: true, force: true });
    } catch (_) {
      /* redb files may stay locked briefly on Windows; ignore */
    }
  }

  return {
    host,
    port,
    user: ADMIN_USER,
    password: ADMIN_PASSWORD,
    stop,
    logs: () => logBuf,
  };
}

module.exports = { startServer, ADMIN_USER, ADMIN_PASSWORD };
