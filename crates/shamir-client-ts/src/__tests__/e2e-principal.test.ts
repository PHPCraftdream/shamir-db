/**
 * End-to-end principal-id verification — proves the TS fxhash64 replica
 * matches the running server, and that chown / addGroupMember work with
 * username-based principal ids (BigInt on the wire).
 *
 * Own startServer (ephemeral port) — no conflict with other e2e suites.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import {
  admin,
  principalId,
} from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  setupDb,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e principal-id (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let adminClient: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        adminClient = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error(
          '[e2e-principal] connection failed. Server logs:\n' + server.logs(),
        );
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (adminClient) {
        try { await adminClient.close(); } catch { /* ok */ }
        adminClient = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    // ── 1. Hash match: TS principalId === server's principal_id ─────────

    const TEST_USER = `pid_test_${process.pid}`;
    const TEST_PW = 'principal-id test pw';
    let chownDb: string;

    it('TS principalId matches server principal_id from accessTree', async () => {
      // Create user via both SCRAM (for login) and RBAC createUser
      // (so it appears in accessTree principals.users).
      await adminClient!.createScramUser(TEST_USER, TEST_PW, []);
      br(await adminClient!.execute('default', {
        id: 'cu-rbac',
        queries: {
          cu: admin.createUser(adminClient!, TEST_USER, TEST_PW),
        },
      }));

      // Fetch GLOBAL accessTree to see principals.users
      const resp = br(await adminClient!.execute('default', {
        id: 'at-pid',
        queries: {
          t: admin.accessTree(),
        },
      }));

      const rec = resp.results.t.records[0] as Record<string, unknown>;
      const tree = rec.access_tree as Record<string, unknown>;
      const principals = tree.principals as Record<string, unknown>;
      const users = principals.users as Array<{
        id: number | bigint;
        name: string;
      }>;

      // Find our test user
      const entry = users.find((u) => u.name === TEST_USER);
      expect(entry).toBeDefined();

      // The critical assertion: TS hash matches server hash
      const serverId = BigInt(entry!.id);
      const tsId = principalId(TEST_USER);
      expect(tsId).toBe(serverId);
    });

    // ── 2. chown round-trip ──────────────────────────────────────────────

    it('chown with username string works (BigInt on wire)', async () => {
      chownDb = await setupDb(adminClient!, 'pid_chown', ['items']);

      // chown the database to TEST_USER (by username string)
      br(await adminClient!.execute(chownDb, {
        id: 'chown-user',
        queries: {
          ch: admin.chown(adminClient!, admin.refDatabase(chownDb), TEST_USER),
        },
      }));

      // Verify via accessTree: the database resource's owner should be
      // principalId(TEST_USER).
      const resp = br(await adminClient!.execute(chownDb, {
        id: 'at-chown',
        queries: {
          t: admin.accessTree({ db: chownDb }),
        },
      }));

      const rec = resp.results.t.records[0] as Record<string, unknown>;
      const tree = rec.access_tree as Record<string, unknown>;
      const resources = tree.resources as Record<string, unknown>;
      const children = resources.children as Array<Record<string, unknown>>;
      const dbNode = children.find((c) => c.name === chownDb);
      expect(dbNode).toBeDefined();
      const owner = BigInt(dbNode!.owner as number | bigint);
      expect(owner).toBe(principalId(TEST_USER));
    });

    // ── 3. addGroupMember round-trip ─────────────────────────────────────

    it('addGroupMember with username string works (BigInt on wire)', async () => {
      // Create a group
      const grpResp = br(await adminClient!.execute(chownDb, {
        id: 'mk-grp',
        queries: {
          g: admin.createGroup(adminClient!, 'testers'),
        },
      }));
      const groupId = (grpResp.results.g.records[0] as Record<string, unknown>)
        .group_id as number;
      expect(typeof groupId).toBe('number');

      // Add TEST_USER to group by username (BigInt principal id on wire)
      br(await adminClient!.execute(chownDb, {
        id: 'add-member',
        queries: {
          am: admin.addGroupMember(adminClient!, admin.groupName('testers'), TEST_USER),
        },
      }));

      // Verify via accessTree: principals.groups should have 'testers'
      // with a member whose id matches principalId(TEST_USER).
      const resp = br(await adminClient!.execute(chownDb, {
        id: 'at-grp',
        queries: {
          t: admin.accessTree({ db: chownDb }),
        },
      }));

      const rec = resp.results.t.records[0] as Record<string, unknown>;
      const tree = rec.access_tree as Record<string, unknown>;
      const principals = tree.principals as Record<string, unknown>;
      const groups = principals.groups as Array<{
        name: string;
        members: Array<{ id: number | bigint; name: string | null }>;
      }>;

      const testersGroup = groups.find((g) => g.name === 'testers');
      expect(testersGroup).toBeDefined();
      const memberIds = testersGroup!.members.map((m) => BigInt(m.id));
      expect(memberIds).toContainEqual(principalId(TEST_USER));
    });
  },
);

describe('e2e-principal.test skip reason', () => {
  it('reports why the principal e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-principal] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
