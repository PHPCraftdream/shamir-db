/**
 * End-to-end test orchestrator.
 *
 *   1. Spawns a real shamir-server release subprocess against a tempdir.
 *   2. Opens one shared `ShamirClient` connection (full TLS+SCRAM).
 *   3. Iterates `tests/*.test.js`, runs each under that shared client.
 *   4. Tears the client + server down, exits with the aggregate code.
 *
 * Setup (once):
 *   npm install      # in this dir
 *   npm run build    # builds shamir-server release + .node binding
 *
 * Run:
 *   npm test
 */

'use strict';

const path = require('path');

const { ShamirClient } = require('shamir-client');
const { startServer } = require('./helpers/server');
const { runAll } = require('./helpers/runner');
const fixtures = require('./helpers/fixtures');

async function main() {
  console.log('Spawning shamir-server (release)...');
  const server = await startServer();
  console.log(`Server bound on ${server.host}:${server.port}.`);

  let client = null;
  let exitCode = 0;
  try {
    console.log('Connecting (TLS 1.3 + SCRAM-Argon2id)...');
    client = await ShamirClient.connect({
      host: server.host,
      port: server.port,
      serverName: 'localhost',
      username: server.user,
      password: server.password,
      acceptNewHost: true,
    });
    console.log('Connected.\n');

    const sharedCtx = {
      client,
      server,
      fixtures,
    };

    const summary = await runAll(path.join(__dirname, 'tests'), sharedCtx);
    if (summary.fail > 0) exitCode = 1;
  } catch (e) {
    console.error('\n❌ Orchestrator failed:', e.message || e);
    console.error('\n--- server logs ---\n' + server.logs());
    exitCode = 2;
  } finally {
    if (client) {
      try {
        await client.close();
      } catch (_) {}
    }
    await server.stop();
  }

  process.exit(exitCode);
}

main().catch((e) => {
  console.error('Fatal:', e);
  process.exit(3);
});
