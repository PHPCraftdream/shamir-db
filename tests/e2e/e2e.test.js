/**
 * End-to-end test for the Node binding.
 *
 *   1. Spawns a real shamir-server subprocess against a fresh tempdir.
 *   2. Connects via the native ShamirClient binding (TLS + SCRAM).
 *   3. Exercises ping → create_db → create_repo+table → set+read.
 *   4. Closes the client cleanly.
 *   5. Kills the server.
 *
 * Setup (once):
 *   npm install                 # in this dir
 *   npm run build               # builds shamir-server release + .node binding
 *
 * Run:
 *   npm test
 */

'use strict';

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const { ShamirClient } = require('shamir-client');

const HOST = '127.0.0.1';
const PORT = 13742;
const ADMIN_USER = 'admin';
const ADMIN_PASSWORD = 'correct horse battery staple';

const REPO_ROOT = path.resolve(__dirname, '..', '..');
const SERVER_BIN = path.join(
  REPO_ROOT,
  'target',
  'release',
  process.platform === 'win32' ? 'shamir-server.exe' : 'shamir-server'
);

function writeKtavConfig(dir) {
  // Minimum viable .ktav config — single TCP listener on a fixed test
  // port, fast Argon2id (spec §3.7.2 floor) so we don't wait 2 s for
  // the handshake.
  const cfg = `
data_dir: ${dir.replace(/\\/g, '/')}

logging: {
    # Need INFO so we can detect the "listener bound" line and know
    # when to start connecting.
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
        addr: ${HOST}:${PORT}
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

# Random port for the observability HTTP server — avoids collisions
# with whatever else might own 9090 on the dev machine.
observability: {
    addr: 127.0.0.1:0
}
`.trim();

  const configPath = path.join(dir, 'server.ktav');
  fs.writeFileSync(configPath, cfg);
  return configPath;
}

/**
 * Spawn the server, wait for it to bind, return the child handle.
 * Resolves once we see the "listener bound" tracing line in stderr.
 */
function startServer(configPath, dataDir) {
  return new Promise((resolve, reject) => {
    if (!fs.existsSync(SERVER_BIN)) {
      return reject(
        new Error(
          `Server binary not found at ${SERVER_BIN}. ` +
            `Run \`npm run build:server\` first.`
        )
      );
    }

    const child = spawn(
      SERVER_BIN,
      ['--config', configPath, '--bootstrap-password', ADMIN_PASSWORD],
      { stdio: ['ignore', 'pipe', 'pipe'] }
    );

    let logBuf = '';
    let resolved = false;

    const timeout = setTimeout(() => {
      if (!resolved) {
        resolved = true;
        child.kill('SIGKILL');
        reject(new Error(`Server failed to bind within 10s.\nLogs:\n${logBuf}`));
      }
    }, 10_000);

    function onData(chunk) {
      logBuf += chunk.toString();
      // The transport-tcp listener emits a tracing line containing
      // "tcp listener bound" once the socket is open. Resolve on that.
      if (!resolved && /listener bound/i.test(logBuf)) {
        resolved = true;
        clearTimeout(timeout);
        // Give the accept loop a beat.
        setTimeout(() => resolve(child), 100);
      }
    }
    child.stdout.on('data', onData);
    child.stderr.on('data', onData);
    child.on('exit', (code, signal) => {
      if (!resolved) {
        resolved = true;
        clearTimeout(timeout);
        reject(
          new Error(
            `Server exited prematurely (code=${code} signal=${signal}).\n` +
              `Logs:\n${logBuf}`
          )
        );
      }
    });

    // Stash log buffer for failure diagnostics.
    child._logBuf = () => logBuf;
  });
}

async function killServer(child) {
  if (!child || child.killed) return;
  child.kill(process.platform === 'win32' ? 'SIGKILL' : 'SIGTERM');
  await new Promise((r) => child.once('exit', r));
}

async function run() {
  console.log('• Setting up tempdir + config...');
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'shamir-e2e-'));
  const configPath = writeKtavConfig(dataDir);

  console.log(`• Spawning server (port ${PORT})...`);
  const server = await startServer(configPath, dataDir);

  let exitCode = 0;
  try {
    console.log('• Connecting (TLS + SCRAM-Argon2id)...');
    const client = await ShamirClient.connect({
      host: HOST,
      port: PORT,
      serverName: 'localhost',
      username: ADMIN_USER,
      password: ADMIN_PASSWORD,
      acceptNewHost: true,
    });

    const pin = client.serverPubKeyPin();
    console.log(`  → connected. session_id: ${client.sessionId().toString('hex').slice(0, 16)}...`);
    console.log(`  → server pin (sha256 ed25519): ${pin.toString('hex').slice(0, 16)}...`);

    const ticket = client.resumptionTicket();
    if (ticket) {
      console.log(`  → resumption ticket: ${ticket.length} bytes`);
    }

    console.log('• Ping...');
    await client.ping();
    console.log('  → pong');

    console.log('• Creating db "prod"...');
    const mkDb = await client.execute('default', {
      id: 'mk-db',
      queries: { mk: { create_db: 'prod' } },
    });
    if (!mkDb.results || !mkDb.results.mk) {
      throw new Error(`unexpected create_db response: ${JSON.stringify(mkDb)}`);
    }
    console.log('  → ok');

    console.log('• Creating repo "main" + table "items"...');
    await client.execute('prod', {
      id: 'mk-table',
      queries: {
        mr: { create_repo: 'main' },
        tb: { create_table: 'items', repo: 'main' },
      },
    });
    console.log('  → ok');

    console.log('• Set + read in one batch...');
    const rw = await client.execute('prod', {
      id: 'rw',
      queries: {
        ins: { set: 'items', key: { sku: 'X1' }, value: { sku: 'X1', qty: 42 } },
        rd: { from: 'items' },
      },
    });
    const records = rw.results.rd.records;
    if (records.length !== 1) {
      throw new Error(`expected 1 record, got ${records.length}`);
    }
    if (records[0].sku !== 'X1' || records[0].qty !== 42) {
      throw new Error(`unexpected record: ${JSON.stringify(records[0])}`);
    }
    console.log(`  → read back: ${JSON.stringify(records[0])}`);

    console.log('• Closing client...');
    await client.close();
    console.log('  → closed');

    console.log('\n✅ All E2E checks passed.\n');
  } catch (e) {
    console.error('\n❌ E2E test failed:', e.message || e);
    console.error('\n--- server logs ---\n' + (server._logBuf ? server._logBuf() : '(none)'));
    exitCode = 1;
  } finally {
    console.log('• Stopping server...');
    await killServer(server);
    // Best-effort cleanup; on Windows redb files may still be locked
    // briefly even after kill, so ignore failures.
    try {
      fs.rmSync(dataDir, { recursive: true, force: true });
    } catch (_) {}
  }

  process.exit(exitCode);
}

run().catch((e) => {
  console.error('Fatal:', e);
  process.exit(1);
});
