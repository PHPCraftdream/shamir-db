/**
 * End-to-end principal-id verification — proves `ShamirClient.resolvePrincipal`
 * returns the server's real, server-assigned principal64 for a username, and
 * that the resolved id is consistent across surfaces (accessTree principals,
 * chown owner, addGroupMember member). BigInt on the wire.
 *
 * Pre-#569 this file asserted `principalId(username) === server principal_id`,
 * treating the principal as a client-side fxhash of the username. Task #548
 * replaced that with a real random server-assigned `user_id` (projected into
 * `principal64`), so the offline hash assumption was invalidated; task #569
 * removed the broken `principalId()` client and added the async
 * `resolvePrincipal(username)` round-trip used here as the oracle instead.
 *
 * Own startServer (ephemeral port) — no conflict with other e2e suites.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { admin } from '../index.js';
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

    // ── 1. resolvePrincipal === server principal64 from accessTree ──────

    const TEST_USER = `pid_test_${process.pid}`;
    const TEST_PW = 'principal-id test pw';
    let chownDb: string;
    let resolvedId: bigint;

    it('resolvePrincipal(username) matches accessTree principals.users id', async () => {
      // Create the user via SCRAM only — that alone adds it to the durable
      // directory (and therefore to accessTree principals.users). The previous
      // redundant `admin.createUser(TEST_USER, ...)` here collided with the
      // already-created SCRAM account ("username exists"); removed per the
      // task #560 A8 precedent (one creation method per username).
      await adminClient!.createScramUser(TEST_USER, TEST_PW, []);

      // The new oracle: resolve via the live directory.
      resolvedId = await adminClient!.resolvePrincipal(TEST_USER);
      expect(typeof resolvedId).toBe('bigint');
      expect(resolvedId).toBeGreaterThan(0n);

      // Cross-check: the same id appears in the GLOBAL accessTree's
      // principals.users list.
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

      const entry = users.find((u) => u.name === TEST_USER);
      expect(entry).toBeDefined();

      // The critical assertion: the resolved id matches the directory's id.
      const serverId = BigInt(entry!.id);
      expect(resolvedId).toBe(serverId);
    });

    // ── 2. chown round-trip ──────────────────────────────────────────────

    it('chown with resolved principal64 works (BigInt on wire)', async () => {
      chownDb = await setupDb(adminClient!, 'pid_chown', ['items']);

      // chown the database to TEST_USER (by resolved principal64)
      br(await adminClient!.execute(chownDb, {
        id: 'chown-user',
        queries: {
          ch: admin.chown(adminClient!, admin.refDatabase(chownDb), resolvedId),
        },
      }));

      // Verify via accessTree: the database resource's owner should be the
      // resolved principal64.
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
      expect(owner).toBe(resolvedId);
    });

    // ── 3. addGroupMember round-trip ─────────────────────────────────────

    it('addGroupMember with resolved principal64 works (BigInt on wire)', async () => {
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

      // Add TEST_USER to group by resolved principal64 (BigInt on wire)
      br(await adminClient!.execute(chownDb, {
        id: 'add-member',
        queries: {
          am: admin.addGroupMember(
            adminClient!,
            admin.groupName('testers'),
            resolvedId,
          ),
        },
      }));

      // Verify via accessTree: principals.groups should have 'testers'
      // with a member whose id matches the resolved principal64.
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
      expect(memberIds).toContainEqual(resolvedId);
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
