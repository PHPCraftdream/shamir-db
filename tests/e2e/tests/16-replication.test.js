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
const { ddl, admin, write, replication } = require('@shamir/client');
const hmac = require('../helpers/hmac');

/** Encode a ReplRequest object → msgpack Buffer for the napi boundary. */
function replBuf(req) {
  return Buffer.from(encode(req));
}

/** Decode a msgpack ReplResponse Buffer → JS object. */
function replDecode(buf) {
  return decode(new Uint8Array(buf));
}

module.exports = async function ({ client, server, fixtures, test, assert, assertEq, assertThrows }) {
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
      queries: { mk: ddl.createDb(db) },
    });
    await client.execute(db, {
      id: 'repl-setup-schema',
      queries: {
        r: ddl.createRepo(repo),
        t: ddl.createTable(table, { repo }),
      },
    });

    // Write 3 transactional upserts — each commit emits a changelog event.
    // NOTE: `transactional` is a BatchRequest-level flag (see
    // `batch_request.rs`), not a per-op field — the prior hand-rolled
    // `{ transactional: true, set, key, value }` shape carried it on the
    // op object, where `SetOp` (no `deny_unknown_fields`) silently
    // dropped it on deserialize. `write.upsert` reproduces the exact
    // same effective wire shape (`{ set, key, value }`).
    for (let i = 0; i < 3; i += 1) {
      const sku = `X${i}`;
      await client.execute(db, {
        id: `repl-write-${i}`,
        queries: {
          w: write.upsert(table, { sku }, { sku, qty: i }),
        },
      });
    }

    // OPEN db + repo + table (0o777) so the non-superuser replicator
    // session can read via the normal Shomer DAC path. `chmod` is an
    // HMAC-gated destructive op — the tag is built by `admin.chmod`
    // (canonical input = resource+mode) via the shared `signerFor`
    // adapter. ResourceRef constructors mirror the wire shapes in
    // access_ddl_tests.rs:
    //   db:    { database: "app" }
    //   repo:  { store: ["app","main"] }
    //   table: { table: ["app","main","items"] }
    const MODE_777 = 0o777; // 511 decimal
    const signer = hmac.signerFor(client);
    await client.execute(db, {
      id: 'repl-chmod-db',
      queries: { c: admin.chmod(signer, admin.refDatabase(db), MODE_777) },
    });
    await client.execute(db, {
      id: 'repl-chmod-repo',
      queries: { c: admin.chmod(signer, admin.refStore(db, repo), MODE_777) },
    });
    await client.execute(db, {
      id: 'repl-chmod-table',
      queries: { c: admin.chmod(signer, admin.refTable(db, repo, table), MODE_777) },
    });

    // Create the replicator-role user + a plain user (no roles) for the
    // deny-by-default scenario. The `replicator` pseudo-role is reserved and
    // cannot be attached via `createScramUser`'s generic roles array — it
    // requires the dedicated `SetReplicator` wire op.
    await client.createScramUser(replUser, replPw, []);
    await client.setReplicator(replUser, true);
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

  // ---------------------------------------------------------------------
  // Scenario 5: full lifecycle over the `AdminOp`-level publication /
  // subscription / replication-profile catalogue — `list_publications`,
  // `drop_publication`, `drop_replication_profile`, `list_subscriptions`,
  // `alter_subscription` (pause/resume/set_profile), `replication_status`,
  // and `drop_subscription`. These 7 wire ops have unit + msgpack-parity
  // coverage (`repl_ops_tests.rs` / `repl_parity.test.ts`) but — before this
  // scenario — zero live-server execution: `create_publication` /
  // `create_replication_profile` / `create_subscription` are already proven
  // live by 17-replication-convergence.test.js, but nothing exercises the
  // matching drop/list/alter/status half.
  //
  // Unlike ReplHello/ReplPull above (the privileged `client.repl(Buffer)`
  // napi method), these are ordinary `BatchOp`s sent through
  // `client.execute()` — built exclusively via the `replication` builder
  // namespace (`crates/shamir-client-ts/src/core/builders/replication.ts`),
  // never hand-assembled wire objects (repo-wide rule, CLAUDE.md).
  //
  // Response shapes are per `crates/shamir-db/src/shamir_db/execute/
  // admin_replication.rs`:
  //   create_publication          → { created_publication: name }
  //   list_publications           → { publications: [{ name, scopes }, ...] }
  //   drop_publication            → { dropped_publication: name, existed: bool }
  //   create_replication_profile  → { created_replication_profile: name }
  //   drop_replication_profile    → { dropped_replication_profile: name, existed: bool }
  //   create_subscription         → { created_subscription: name }
  //   list_subscriptions          → { subscriptions: [{ name, upstream, publication, profile, state }, ...] }
  //   alter_subscription          → { altered_subscription: name } (or a
  //                                  `not_found`-coded BatchError if the
  //                                  subscription doesn't exist)
  //   replication_status          → { subscriptions: [{ name, state }, ...] }
  //   drop_subscription           → { dropped_subscription: name, existed: bool }
  // ---------------------------------------------------------------------
  test('lifecycle: create_publication -> list_publications -> drop_publication', async () => {
    const pubName = 'pub_lifecycle';

    const createResp = await client.execute(db, {
      id: 'lifecycle-create-publication',
      queries: {
        p: replication.publication(pubName, [replication.replScope(db, { repo })]),
      },
    });
    assertEq(
      createResp.results.p.records[0].created_publication,
      pubName,
      `create_publication response: ${JSON.stringify(createResp.results.p)}`
    );

    const listResp1 = await client.execute(db, {
      id: 'lifecycle-list-publications-1',
      queries: { l: replication.listPublications() },
    });
    const pubs1 = listResp1.results.l.records[0].publications;
    assert(Array.isArray(pubs1), `publications should be an array: ${JSON.stringify(pubs1)}`);
    assert(
      pubs1.some((p) => p.name === pubName),
      `${pubName} missing from list_publications: ${JSON.stringify(pubs1)}`
    );

    const dropResp = await client.execute(db, {
      id: 'lifecycle-drop-publication',
      queries: { d: replication.dropPublication(pubName) },
    });
    assertEq(
      dropResp.results.d.records[0].dropped_publication,
      pubName,
      `drop_publication response: ${JSON.stringify(dropResp.results.d)}`
    );
    assertEq(
      dropResp.results.d.records[0].existed,
      true,
      `drop_publication should report existed=true: ${JSON.stringify(dropResp.results.d)}`
    );

    const listResp2 = await client.execute(db, {
      id: 'lifecycle-list-publications-2',
      queries: { l: replication.listPublications() },
    });
    const pubs2 = listResp2.results.l.records[0].publications;
    assert(
      !pubs2.some((p) => p.name === pubName),
      `${pubName} still listed after drop_publication: ${JSON.stringify(pubs2)}`
    );

    // Dropping again is a no-op (not an error) — `existed` flips to false.
    const dropAgainResp = await client.execute(db, {
      id: 'lifecycle-drop-publication-again',
      queries: { d: replication.dropPublication(pubName) },
    });
    assertEq(
      dropAgainResp.results.d.records[0].existed,
      false,
      `re-dropping an absent publication should report existed=false: ${JSON.stringify(dropAgainResp.results.d)}`
    );
  });

  test('lifecycle: create_replication_profile -> drop_replication_profile (existed flag both ways)', async () => {
    const profileName = 'profile_lifecycle';

    const createResp = await client.execute(db, {
      id: 'lifecycle-create-profile',
      queries: {
        cp: replication.replicationProfile(profileName, [
          replication.replStream(replication.replScope(db, { repo }), 'pull', 'read_only'),
        ]),
      },
    });
    assertEq(
      createResp.results.cp.records[0].created_replication_profile,
      profileName,
      `create_replication_profile response: ${JSON.stringify(createResp.results.cp)}`
    );

    const dropResp = await client.execute(db, {
      id: 'lifecycle-drop-profile',
      queries: { dp: replication.dropReplicationProfile(profileName) },
    });
    assertEq(
      dropResp.results.dp.records[0].dropped_replication_profile,
      profileName,
      `drop_replication_profile response: ${JSON.stringify(dropResp.results.dp)}`
    );
    assertEq(
      dropResp.results.dp.records[0].existed,
      true,
      `drop_replication_profile should report existed=true: ${JSON.stringify(dropResp.results.dp)}`
    );

    // Wire contract (admin_replication.rs::handle_drop_replication_profile):
    // dropping an absent profile is NOT an error — it is a no-op that
    // reports `existed: false`.
    const dropAgainResp = await client.execute(db, {
      id: 'lifecycle-drop-profile-again',
      queries: { dp: replication.dropReplicationProfile(profileName) },
    });
    assertEq(
      dropAgainResp.results.dp.records[0].dropped_replication_profile,
      profileName,
      `re-drop should still echo the name: ${JSON.stringify(dropAgainResp.results.dp)}`
    );
    assertEq(
      dropAgainResp.results.dp.records[0].existed,
      false,
      `re-dropping an absent profile should report existed=false: ${JSON.stringify(dropAgainResp.results.dp)}`
    );
  });

  test('lifecycle: create_subscription -> list_subscriptions -> alter_subscription (pause/resume/set_profile) -> replication_status -> drop_subscription', async () => {
    const profileA = 'sub_lifecycle_profile_a';
    const profileB = 'sub_lifecycle_profile_b';
    const subName = 'sub_lifecycle';
    const upstream = `tcp://${server.host}:${server.port}`;

    // Two profiles so `set_profile` has somewhere to rebind to.
    await client.execute(db, {
      id: 'lifecycle-sub-setup-profiles',
      queries: {
        pa: replication.replicationProfile(profileA, [
          replication.replStream(replication.replScope(db, { repo }), 'pull', 'read_only'),
        ]),
        pb: replication.replicationProfile(profileB, [
          replication.replStream(replication.replScope(db, { repo }), 'pull', 'read_only'),
        ]),
      },
    });

    const createResp = await client.execute(db, {
      id: 'lifecycle-create-subscription',
      queries: {
        cs: replication.subscription(subName, {
          upstream,
          publication: 'pub_conv', // arbitrary — 386-a create_subscription does not validate existence
          profile: profileA,
        }),
      },
    });
    assertEq(
      createResp.results.cs.records[0].created_subscription,
      subName,
      `create_subscription response: ${JSON.stringify(createResp.results.cs)}`
    );

    // list_subscriptions: the new subscription appears, freshly active,
    // bound to profileA.
    const listResp1 = await client.execute(db, {
      id: 'lifecycle-list-subscriptions-1',
      queries: { l: replication.listSubscriptions() },
    });
    const subs1 = listResp1.results.l.records[0].subscriptions;
    assert(Array.isArray(subs1), `subscriptions should be an array: ${JSON.stringify(subs1)}`);
    const sub1 = subs1.find((s) => s.name === subName);
    assert(sub1, `${subName} missing from list_subscriptions: ${JSON.stringify(subs1)}`);
    assertEq(sub1.state, 'active', `fresh subscription should be active: ${JSON.stringify(sub1)}`);
    assertEq(sub1.profile, profileA, `fresh subscription profile: ${JSON.stringify(sub1)}`);

    // alter_subscription: pause.
    const pauseResp = await client.execute(db, {
      id: 'lifecycle-alter-pause',
      queries: { a: replication.alterSubscription(subName, 'pause') },
    });
    assertEq(
      pauseResp.results.a.records[0].altered_subscription,
      subName,
      `alter_subscription (pause) response: ${JSON.stringify(pauseResp.results.a)}`
    );
    const statusAfterPause = await client.execute(db, {
      id: 'lifecycle-status-after-pause',
      queries: { s: replication.replicationStatus() },
    });
    const pausedEntry = statusAfterPause.results.s.records[0].subscriptions.find(
      (s) => s.name === subName
    );
    assert(pausedEntry, `${subName} missing from replication_status after pause`);
    assertEq(
      pausedEntry.state,
      'paused',
      `replication_status should reflect paused: ${JSON.stringify(pausedEntry)}`
    );

    // alter_subscription: resume.
    const resumeResp = await client.execute(db, {
      id: 'lifecycle-alter-resume',
      queries: { a: replication.alterSubscription(subName, 'resume') },
    });
    assertEq(
      resumeResp.results.a.records[0].altered_subscription,
      subName,
      `alter_subscription (resume) response: ${JSON.stringify(resumeResp.results.a)}`
    );
    const listAfterResume = await client.execute(db, {
      id: 'lifecycle-list-after-resume',
      queries: { l: replication.listSubscriptions() },
    });
    const resumedEntry = listAfterResume.results.l.records[0].subscriptions.find(
      (s) => s.name === subName
    );
    assert(resumedEntry, `${subName} missing from list_subscriptions after resume`);
    assertEq(
      resumedEntry.state,
      'active',
      `list_subscriptions should reflect active after resume: ${JSON.stringify(resumedEntry)}`
    );

    // alter_subscription: set_profile → rebind to profileB.
    const setProfileResp = await client.execute(db, {
      id: 'lifecycle-alter-set-profile',
      queries: { a: replication.alterSubscription(subName, { set_profile: profileB }) },
    });
    assertEq(
      setProfileResp.results.a.records[0].altered_subscription,
      subName,
      `alter_subscription (set_profile) response: ${JSON.stringify(setProfileResp.results.a)}`
    );
    const listAfterSetProfile = await client.execute(db, {
      id: 'lifecycle-list-after-set-profile',
      queries: { l: replication.listSubscriptions() },
    });
    const rebound = listAfterSetProfile.results.l.records[0].subscriptions.find(
      (s) => s.name === subName
    );
    assert(rebound, `${subName} missing from list_subscriptions after set_profile`);
    assertEq(
      rebound.profile,
      profileB,
      `list_subscriptions should reflect the rebound profile: ${JSON.stringify(rebound)}`
    );

    // replication_status mid-lifecycle: shape/fields sanity (name + state
    // present on every entry, including our still-active subscription).
    const statusResp = await client.execute(db, {
      id: 'lifecycle-status-mid',
      queries: { s: replication.replicationStatus() },
    });
    const statusEntries = statusResp.results.s.records[0].subscriptions;
    assert(Array.isArray(statusEntries), `replication_status.subscriptions should be an array: ${JSON.stringify(statusEntries)}`);
    const statusEntry = statusEntries.find((s) => s.name === subName);
    assert(statusEntry, `${subName} missing from replication_status: ${JSON.stringify(statusEntries)}`);
    assertEq(
      Object.prototype.hasOwnProperty.call(statusEntry, 'name'),
      true,
      `replication_status entry should have a name field: ${JSON.stringify(statusEntry)}`
    );
    assertEq(
      statusEntry.state,
      'active',
      `replication_status should reflect active mid-lifecycle: ${JSON.stringify(statusEntry)}`
    );

    // alter_subscription against a name that never existed → not_found
    // (admin_replication.rs::handle_alter_subscription returns a
    // `BatchError::QueryError { code: Some("not_found"), .. }` when the
    // lookup finds no matching row).
    await assertThrows(
      () =>
        client.execute(db, {
          id: 'lifecycle-alter-missing',
          queries: { a: replication.alterSubscription('sub_never_existed', 'pause') },
        }),
      (err) => /not_found|not found/i.test(err.message),
      'alter_subscription on a non-existent subscription should raise a not_found error'
    );

    // drop_subscription: removed from a subsequent list_subscriptions.
    const dropResp = await client.execute(db, {
      id: 'lifecycle-drop-subscription',
      queries: { d: replication.dropSubscription(subName) },
    });
    assertEq(
      dropResp.results.d.records[0].dropped_subscription,
      subName,
      `drop_subscription response: ${JSON.stringify(dropResp.results.d)}`
    );
    assertEq(
      dropResp.results.d.records[0].existed,
      true,
      `drop_subscription should report existed=true: ${JSON.stringify(dropResp.results.d)}`
    );

    const listResp2 = await client.execute(db, {
      id: 'lifecycle-list-subscriptions-2',
      queries: { l: replication.listSubscriptions() },
    });
    const subs2 = listResp2.results.l.records[0].subscriptions;
    assert(
      !subs2.some((s) => s.name === subName),
      `${subName} still listed after drop_subscription: ${JSON.stringify(subs2)}`
    );

    // Cleanup the two profiles created for this scenario (best-effort, not
    // asserted — keeps the catalogue tidy for any later file that lists
    // profiles, though nothing currently does).
    await client.execute(db, {
      id: 'lifecycle-sub-cleanup-profiles',
      queries: {
        dpa: replication.dropReplicationProfile(profileA),
        dpb: replication.dropReplicationProfile(profileB),
      },
    });
  });
};
