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

function writeKtavConfig(dir, host, port, { replication } = {}) {
  const replicationBlock = replication
    ? `
replication: {
    node_id: ${replication.nodeId}
    replicator_user: ${replication.replicatorUser}
    replicator_password: ${replication.replicatorPassword}
    server_name: ${replication.serverName || 'localhost'}
}
`
    : '';

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
    # The default per-subnet auth_init rate limit is 10/s (spec §8). Every
    # connection in this harness dials 127.0.0.1 — one subnet — so a burst of
    # logins (admin + per-file SCRAM users + the 16/17 replication sessions)
    # drains the token bucket and the server rejects the next dial with a TLS
    # CloseNotify, surfacing client-side as "read challenge: io: early eof".
    # The Rust e2e suites set this to 1000 (permission_e2e.rs, repl_pull_e2e.rs,
    # max_connections.rs) for the same reason; mirror them here.
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

# Random port to avoid collisions with whatever else might own 9090.
observability: {
    addr: 127.0.0.1:0
}
${replicationBlock}`.trim();

  const configPath = path.join(dir, 'server.ktav');
  fs.writeFileSync(configPath, cfg);
  return configPath;
}

/**
 * Start a server. Returns `{ host, port, user, password, stop, logs }`.
 * `stop()` is async — kills the subprocess and removes the tempdir.
 *
 * `replication` (optional) — follower-side `[replication]` ktav block
 * (`crates/shamir-server/src/config.rs::ReplicationConfig`):
 *   { nodeId, replicatorUser, replicatorPassword, serverName }
 * `replicatorUser`/`replicatorPassword` MUST be the credentials of a
 * `replicator`-role account already created on the leader (see
 * `startServerWithReplication` below, and 16-replication.test.js Scenario 1
 * for how that account + OPEN access path is set up).
 */
async function startServer({
  host = DEFAULT_HOST,
  port = DEFAULT_PORT,
  replication,
} = {}) {
  if (!fs.existsSync(SERVER_BIN)) {
    throw new Error(
      `Server binary not found at ${SERVER_BIN}.\n` +
        `Run \`npm run build:server\` first.`
    );
  }

  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-e2e-'));
  const configPath = writeKtavConfig(dataDir, host, port, { replication });

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

/**
 * Convenience wrapper: start a FOLLOWER server whose ktav config carries a
 * `[replication]` block pointing at an already-running leader's replicator
 * account (task #388 — two-server convergence e2e).
 *
 * The caller is responsible for creating the `replicator`-role user on the
 * leader (and the OPEN access path it needs) BEFORE calling this, and for
 * issuing the `create_replication_profile`/`create_subscription` admin batch
 * against the returned follower's own client afterwards — this helper only
 * spawns the process with the right ktav `[replication]` block.
 *
 * `replicatorUser`/`replicatorPassword` — credentials of the `replicator`
 * role account already created on the LEADER (see 16-replication.test.js
 * Scenario 1: `createScramUser(user, pw, ['replicator'])` + OPEN db/repo/
 * table access so the non-superuser replicator session can read).
 *
 * Returns the same `{ host, port, user, password, stop, logs }` shape as
 * `startServer` (the `user`/`password` here are still the FOLLOWER's own
 * bootstrap admin — used to run DDL locally, e.g. creating the matching
 * schema + the replication_profile/subscription).
 */
async function startServerWithReplication({
  host = DEFAULT_HOST,
  port = DEFAULT_PORT + 1,
  nodeId = 'follower-1',
  replicatorUser,
  replicatorPassword,
  serverName = 'localhost',
} = {}) {
  if (!replicatorUser || !replicatorPassword) {
    throw new Error(
      'startServerWithReplication requires replicatorUser + replicatorPassword ' +
        '(credentials of a replicator-role account already created on the leader).'
    );
  }
  return startServer({
    host,
    port,
    replication: {
      nodeId,
      replicatorUser,
      replicatorPassword,
      serverName,
    },
  });
}

module.exports = {
  startServer,
  startServerWithReplication,
  ADMIN_USER,
  ADMIN_PASSWORD,
};
