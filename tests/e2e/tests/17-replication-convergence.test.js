/**
 * Two-server replication convergence e2e (task #388).
 *
 * Unlike every other file in `tests/e2e/tests/`, this test does NOT run
 * solely against the shared `ctx.server`/`ctx.client` (which the orchestrator
 * treats as the LEADER here) — it also spins up a second, independent
 * `shamir-server` process (the FOLLOWER) via
 * `helpers/server.js::startServerWithReplication`, wires it up with a
 * `[replication]` ktav block, and drives BOTH nodes with their own
 * `ShamirClient` connections.
 *
 * Flow (per the #388 brief):
 *   1. LEADER (shared `ctx.server`/`ctx.client`, i.e. the bootstrap admin):
 *      create db `app` + repo `main` + table `items`; OPEN db/repo/table
 *      access (0o777, same pattern as 16-replication.test.js Scenario 1);
 *      create a `repl`/`replicator`-role user; create a `create_publication`
 *      exposing the `app/main` scope; write a few rows.
 *   2. FOLLOWER: start a second server with a `[replication]` block pointing
 *      at the leader's `repl` credentials. On the follower (its own
 *      bootstrap admin) create the SAME db/repo/table schema (apply_replicated
 *      writes into an EXISTING table — it does not create schema), then run
 *      an admin batch creating a `replication_profile` (pull stream over
 *      `app/main`) + a `create_subscription` bound to that profile and the
 *      leader's `tcp://host:port` + publication name.
 *   3. Poll the follower via its own client (`SELECT` over `items`) until it
 *      reports the same row count as the leader, within a window that
 *      comfortably covers the 10s `SubscriptionSupervisor` reconcile-tick
 *      (`crates/shamir-server/src/server/server_launcher.rs` —
 *      `REPL_RECONCILE_INTERVAL = Duration::from_secs(10)`) plus pull-loop
 *      latency.
 *   4. Increment: write more rows on the leader, confirm the follower catches
 *      up again.
 *
 * NodeMode (ReadOnly) gate on the follower is NOT exercised here: as of
 * 386-c, `ReplicationConfig` (`crates/shamir-server/src/config.rs`) has no
 * `node_mode` field yet — only `node_id` / `replicator_user` /
 * `replicator_password` / `server_name`. Left for a future task (see the
 * brief's Part B step 5 and this file's final comment block).
 *
 * IMPORTANT — MSVC/build requirement: this file spawns a SECOND real
 * `shamir-server` release binary and drives it with a second napi
 * `ShamirClient` connection. Like every other file here it requires
 * `npm run build` (cargo release binary + napi binding) on an MSVC host
 * before `npm test` can execute it. It has not been run in this sandbox
 * (non-MSVC); it was written and reviewed against
 * `helpers/server.js`/`16-replication.test.js` conventions and the current
 * `crates/shamir-query-types/src/admin/types/repl_ops.rs` wire shapes.
 */

'use strict';

const { ShamirClient } = require('shamir-client');
const { startServerWithReplication } = require('../helpers/server');

// SubscriptionSupervisor reconcile-tick (server_launcher.rs
// REPL_RECONCILE_INTERVAL). The poll window below MUST stay comfortably
// above this, or convergence checks will spuriously time out on a slow CI
// box that just missed one tick.
const RECONCILE_TICK_MS = 10_000;
const CONVERGENCE_TIMEOUT_MS = 30_000;
const CONVERGENCE_POLL_MS = 500;

module.exports = async function ({ client: leaderClient, server: leaderServer, test, assert, assertEq }) {
  // Use a file-unique db name (NOT the literal 'app') so this file's
  // schema setup doesn't collide with 16-replication.test.js, which runs
  // first against the SAME shared leader server and also creates 'app'.
  // Both nodes (leader + follower) use the same name because apply_replicated
  // writes into a pre-existing table of identical identity.
  const db = 'appconv';
  const repo = 'main';
  const table = 'items';

  const replUser = 'repl-conv';
  const replPw = 'repl-conv-password';
  const publicationName = 'pub_conv';
  const profileName = 'follower_profile';
  const subscriptionName = 'sub_to_leader';

  let follower = null;
  let followerClient = null;

  /** Count rows in `items` via a read-all through the given client. */
  async function countItems(client, targetDb) {
    const resp = await client.execute(targetDb, {
      id: 'count-items',
      queries: { q: { from: table } },
    });
    const records = resp.results && resp.results.q && resp.results.q.records;
    return Array.isArray(records) ? records.length : 0;
  }

  /** Insert `n` transactional rows into `items` on the leader, sku-numbered from `base`. */
  async function writeRows(base, n) {
    for (let i = base; i < base + n; i += 1) {
      const sku = `CONV-${i}`;
      // eslint-disable-next-line no-await-in-loop
      await leaderClient.execute(db, {
        id: `conv-write-${i}`,
        queries: {
          w: {
            transactional: true,
            set: table,
            key: { sku },
            value: { sku, qty: i },
          },
        },
      });
    }
  }

  /** Poll `countItems(followerClient, db)` until it reaches `target` or times out. */
  async function waitForFollowerCount(target) {
    const deadline = Date.now() + CONVERGENCE_TIMEOUT_MS;
    let last = -1;
    while (Date.now() < deadline) {
      // eslint-disable-next-line no-await-in-loop
      last = await countItems(followerClient, db);
      if (last >= target) return last;
      // eslint-disable-next-line no-await-in-loop
      await new Promise((r) => setTimeout(r, CONVERGENCE_POLL_MS));
    }
    return last;
  }

  test('setup: leader creates app/main/items, opens access, publishes, writes rows', async () => {
    await leaderClient.execute('default', {
      id: 'conv-setup-db',
      queries: { mk: { create_db: db } },
    });
    await leaderClient.execute(db, {
      id: 'conv-setup-schema',
      queries: {
        r: { create_repo: repo },
        t: { create_table: table, repo },
      },
    });

    // OPEN db + repo + table (0o777) — same pattern as 16-replication.test.js
    // Scenario 1 — so the non-superuser `replicator`-role session can read
    // via the normal Shomer DAC path.
    const MODE_777 = 0o777;
    await leaderClient.execute(db, {
      id: 'conv-chmod-db',
      queries: { c: { chmod: { database: db }, mode: MODE_777 } },
    });
    await leaderClient.execute(db, {
      id: 'conv-chmod-repo',
      queries: { c: { chmod: { store: [db, repo] }, mode: MODE_777 } },
    });
    await leaderClient.execute(db, {
      id: 'conv-chmod-table',
      queries: { c: { chmod: { table: [db, repo, table] }, mode: MODE_777 } },
    });

    await leaderClient.createScramUser(replUser, replPw, ['replicator']);

    // Declare the leader's publication — the set of scopes downstream
    // subscribers may pull (repl_ops.rs::CreatePublicationOp).
    await leaderClient.execute(db, {
      id: 'conv-create-publication',
      queries: {
        p: {
          create_publication: publicationName,
          scopes: [{ db, repo }],
        },
      },
    });

    await writeRows(0, 3);
  });

  test('follower converges to the leader row count after the reconcile tick', async () => {
    follower = await startServerWithReplication({
      nodeId: 'follower-1',
      replicatorUser: replUser,
      replicatorPassword: replPw,
      serverName: 'localhost',
    });

    followerClient = await ShamirClient.connect({
      host: follower.host,
      port: follower.port,
      serverName: 'localhost',
      username: follower.user,
      password: follower.password,
      acceptNewHost: true,
    });

    // The follower must have the SAME schema locally — apply_replicated
    // writes into an EXISTING table, it does not create schema on the fly.
    await followerClient.execute('default', {
      id: 'conv-follower-db',
      queries: { mk: { create_db: db } },
    });
    await followerClient.execute(db, {
      id: 'conv-follower-schema',
      queries: {
        r: { create_repo: repo },
        t: { create_table: table, repo },
      },
    });

    // Declarative catalogue on the follower (shamir_query_builder::ddl::
    // replication:: shapes, per repl_ops.rs): a replication_profile with a
    // single pull/read_only stream over app/main, then a subscription bound
    // to it, targeting the leader's tcp host:port + publication name.
    const upstream = `tcp://${leaderServer.host}:${leaderServer.port}`;
    await followerClient.execute(db, {
      id: 'conv-follower-subscribe',
      queries: {
        cp: {
          create_replication_profile: profileName,
          streams: [
            {
              scope: { db, repo },
              direction: 'pull',
              mode: 'read_only',
            },
          ],
        },
        cs: {
          create_subscription: subscriptionName,
          upstream,
          publication: publicationName,
          profile: profileName,
        },
      },
    });

    // Poll window MUST exceed the 10s reconcile-tick (the subscription was
    // just created at runtime — the supervisor picks it up on its next
    // reconcile() pass, then the pull-loop still needs to fetch+apply).
    assert(
      CONVERGENCE_TIMEOUT_MS > RECONCILE_TICK_MS,
      'convergence timeout must exceed the reconcile tick'
    );

    const leaderCount = await countItems(leaderClient, db);
    assertEq(leaderCount, 3, `expected 3 seeded rows on leader, got ${leaderCount}`);

    const followerCount = await waitForFollowerCount(leaderCount);
    assertEq(
      followerCount,
      leaderCount,
      `follower did not converge to leader row count within ${CONVERGENCE_TIMEOUT_MS}ms`
    );
  });

  test('increment: follower catches up after more leader writes', async () => {
    await writeRows(3, 2);

    const leaderCount = await countItems(leaderClient, db);
    assertEq(leaderCount, 5, `expected 5 rows on leader after increment, got ${leaderCount}`);

    const followerCount = await waitForFollowerCount(leaderCount);
    assertEq(
      followerCount,
      leaderCount,
      `follower did not catch up to incremented leader row count within ${CONVERGENCE_TIMEOUT_MS}ms`
    );
  });

  // ---------------------------------------------------------------------
  // NOTE — read-only gate (brief Part B step 5): NOT exercised here.
  // `ReplicationConfig` (crates/shamir-server/src/config.rs, as of 386-c)
  // has no `node_mode` field — only node_id/replicator_user/
  // replicator_password/server_name. Wiring `NodeMode::ReadOnly` into the
  // follower's ktav config (and asserting a client write on the follower
  // fails with `read_only_replica`) is left for a future task once that
  // config field is added.
  // ---------------------------------------------------------------------

  test('teardown: stop follower client + server', async () => {
    if (followerClient) {
      try {
        await followerClient.close();
      } catch (_) {
        /* best-effort */
      }
    }
    if (follower) {
      await follower.stop();
    }
  });
};
