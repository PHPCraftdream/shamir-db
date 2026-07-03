/**
 * Replication pull-API end-to-end (REPLICATION §5).
 *
 * Exercises the privileged `client.repl(Buffer)` napi method (added in
 * `crates/shamir-client-node/src/lib.rs`) against a live `shamir-server`:
 *
 *   1. Setup — as the bootstrap admin (superuser): create `app/main/items`,
 *      write rows (each commit emits a changelog event), OPEN the access
 *      path (chmod 0o777 on db + repo + table) so a non-superuser
 *      `replicator`-role session can read via the normal Shomer DAC path
 *      (the explicit OPEN pattern proven by `permission_e2e.rs` Scenario 3
 *      and `repl_pull_e2e.rs` Scenario B — NOT a superuser-bypass), then
 *      create the `repl` user with the `replicator` role.
 *   2. ReplHello — the `repl` session learns `leader_epoch == 1` and that
 *      `app/main` is advertised with `current_version > 0`.
 *   3. ReplPull — from version 0 returns a non-empty `events` byte blob
 *      (encoded `Vec<ChangelogEvent>`) and `current_version > 0`.
 *   4. Deny-by-default — a plain user (no `replicator` role) sends
 *      ReplHello and gets `repl_kind === "error"`, `code === "bad_role"`.
 *
 * Wire shapes (`crates/shamir-query-types/src/wire/repl.rs`):
 *   ReplRequest  — internally tagged on `repl_op`  (snake_case).
 *   ReplResponse — internally tagged on `repl_kind` (snake_case).
 * The `events` field in `Pull` is `serde_bytes` — a raw byte blob.
 *
 * The napi `repl` method takes/returns msgpack `Buffer`s (matching the
 * `execute` FFI pattern) — we encode/decode with `@msgpack/msgpack` here.
 *
 * NOTE: This file requires the napi binding to be rebuilt from the current
 * source (which adds `repl`). The prebuilt `.node` in the repo may predate
 * the method — run `npm run build:binding` first.
 */

'use strict';

const { encode, decode } = require('@msgpack/msgpack');
const { ShamirClient } = require('shamir-client');

/** Encode a ReplRequest object → msgpack Buffer for the napi boundary. */
function replBuf(req) {
  return Buffer.from(encode(req));
}

/** Decode a msgpack ReplResponse Buffer → JS object. */
function replDecode(buf) {
  return decode(new Uint8Array(buf));
}

module.exports = async function ({ client, server, fixtures, test, assert, assertEq }) {
  // ---------------------------------------------------------------------
  // Shared setup (runs once for this file; the runner calls tests in the
  // order they are registered, so a leading `test('setup', ...)` is the
  // conventional place for file-scoped fixtures — see 15-transactions).
  // ---------------------------------------------------------------------
  const db = 'app';
  const repo = 'main';
  const table = 'items';

  const replUser = 'repl';
  const replPw = 'repl-password';
  const plainUser = 'plain';
  const plainPw = 'plain-password';

  test('setup: admin creates app/main/items, writes rows, opens access, creates users', async () => {
    // create_db must run against `default` (target doesn't exist yet).
    await client.execute('default', {
      id: 'repl-setup-db',
      queries: { mk: { create_db: db } },
    });
    await client.execute(db, {
      id: 'repl-setup-schema',
      queries: {
        r: { create_repo: repo },
        t: { create_table: table, repo },
      },
    });

    // Write 3 transactional upserts — each commit emits a changelog event.
    for (let i = 0; i < 3; i += 1) {
      const sku = `X${i}`;
      await client.execute(db, {
        id: `repl-write-${i}`,
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

    // OPEN db + repo + table (0o777) so the non-superuser replicator
    // session can read via the normal Shomer DAC path.
    // chmod wire shape (access_ddl_tests.rs):
    //   db:    { chmod: { database: "app" }, mode: 511 }
    //   repo:  { chmod: { store: ["app","main"] }, mode: 511 }
    //   table: { chmod: { table: ["app","main","items"] }, mode: 511 }
    const MODE_777 = 0o777; // 511 decimal
    await client.execute(db, {
      id: 'repl-chmod-db',
      queries: { c: { chmod: { database: db }, mode: MODE_777 } },
    });
    await client.execute(db, {
      id: 'repl-chmod-repo',
      queries: { c: { chmod: { store: [db, repo] }, mode: MODE_777 } },
    });
    await client.execute(db, {
      id: 'repl-chmod-table',
      queries: { c: { chmod: { table: [db, repo, table] }, mode: MODE_777 } },
    });

    // Create the replicator-role user + a plain user (no roles) for the
    // deny-by-default scenario.
    await client.createScramUser(replUser, replPw, ['replicator']);
    await client.createScramUser(plainUser, plainPw, []);
  });

  // ---------------------------------------------------------------------
  // Scenario 2 + 3: ReplHello + ReplPull as the `replicator`-role user.
  // ---------------------------------------------------------------------
  test('ReplHello as replicator → leader_epoch 1, app/main advertised', async () => {
    const repl = await ShamirClient.connect({
      host: server.host,
      port: server.port,
      serverName: 'localhost',
      username: replUser,
      password: replPw,
      acceptNewHost: true,
    });
    try {
      const resp = replDecode(await repl.repl(replBuf({
        repl_op: 'hello',
        proto_ver: 1,
        node_id: 'n1',
      })));

      assertEq(resp.repl_kind, 'hello', `expected hello, got ${JSON.stringify(resp)}`);
      assertEq(resp.leader_epoch, 1, `leader_epoch: ${JSON.stringify(resp)}`);
      assert(Array.isArray(resp.repos), `repos should be an array: ${JSON.stringify(resp)}`);

      const main = resp.repos.find((r) => r.db === db && r.repo === repo);
      assert(main, `app/main not in advertised repos: ${JSON.stringify(resp.repos)}`);

      // The journal writer is async — poll until current_version > 0.
      // Mirrors repl_pull_e2e.rs Scenario B, which loops up to 100×10ms.
      if (main.current_version === 0) {
        let latest = main;
        for (let attempt = 0; attempt < 100; attempt += 1) {
          const again = replDecode(await repl.repl(replBuf({
            repl_op: 'hello',
            proto_ver: 1,
            node_id: 'n1',
          })));
          latest = again.repos.find((r) => r.db === db && r.repo === repo);
          if (latest && latest.current_version > 0) break;
          await new Promise((r) => setTimeout(r, 10));
        }
        assert(
          latest && latest.current_version > 0,
          `current_version never rose above 0 (async journal): ${JSON.stringify(latest)}`
        );
      }
    } finally {
      await repl.close();
    }
  });

  test('ReplPull as replicator → non-empty events, current_version > 0', async () => {
    const repl = await ShamirClient.connect({
      host: server.host,
      port: server.port,
      serverName: 'localhost',
      username: replUser,
      password: replPw,
      acceptNewHost: true,
    });
    try {
      const resp = replDecode(await repl.repl(replBuf({
        repl_op: 'pull',
        db,
        repo,
        from_version: 0,
        limit: 100,
      })));

      assertEq(resp.repl_kind, 'pull', `expected pull, got ${JSON.stringify(resp)}`);
      assertEq(resp.leader_epoch, 1, `leader_epoch: ${JSON.stringify(resp)}`);
      assert(
        resp.current_version > 0,
        `current_version > 0 after writes: ${JSON.stringify(resp)}`
      );
      // events is a serde_bytes blob → arrives as a Node Buffer (or
      // Uint8Array under msgpack v3). Just assert non-empty.
      const eventsLen = resp.events && resp.events.length;
      assert(
        typeof eventsLen === 'number' && eventsLen > 0,
        `events should be a non-empty byte blob: ${JSON.stringify(resp.events)}`
      );
    } finally {
      await repl.close();
    }
  });

  // ---------------------------------------------------------------------
  // Scenario 4: deny-by-default — plain user (no replicator role).
  // ---------------------------------------------------------------------
  test('ReplHello as plain user → error / bad_role', async () => {
    const plain = await ShamirClient.connect({
      host: server.host,
      port: server.port,
      serverName: 'localhost',
      username: plainUser,
      password: plainPw,
      acceptNewHost: true,
    });
    try {
      const resp = replDecode(await plain.repl(replBuf({
        repl_op: 'hello',
        proto_ver: 1,
        node_id: 'n2',
      })));

      // Unlike DbResponse::Error (which the napi layer turns into a
      // thrown exception), ReplResponse::Error is a *successful* wire
      // reply carrying the repl-layer error variant — so it decodes
      // normally here.
      assertEq(resp.repl_kind, 'error', `expected error, got ${JSON.stringify(resp)}`);
      assertEq(resp.code, 'bad_role', `expected bad_role, got ${JSON.stringify(resp)}`);
      assertEq(resp.leader_epoch, 1, `leader_epoch: ${JSON.stringify(resp)}`);
    } finally {
      await plain.close();
    }
  });
};
