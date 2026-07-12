/**
 * End-to-end permission enforcement tests — exercises ACL mode bits,
 * group membership, role round-trips, and catalog visibility against
 * a live shamir-server.
 *
 * Own startServer (ephemeral port) — no conflict with other e2e suites.
 *
 * ## Server permission model (documented, not invented)
 *
 * ShamirDB uses **Unix-style mode bits** (chmod/chown/chgrp) for
 * per-resource access control.  G.4c: new resources default to enforced
 * owner-rwx (0o700); tests that need open access explicitly `chmod 0o777`
 * the db + store + table so traversal-Execute reaches the target.
 * `listDatabases` is classified as an **admin op** and
 * requires `is_superuser` (the "superuser" role); non-superusers
 * receive `permission_denied`.  When superusers call `listDatabases`,
 * they see ALL databases — the server does NOT filter the catalog by
 * per-database permissions.
 *
 * Therefore "different users see different databases" is NOT the
 * server's current visibility model.  Instead:
 *   - Non-superusers cannot list databases at all.
 *   - Access control is enforced per-operation (read/write on a
 *     specific table/store/database), not at the catalog level.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse } from '../index.js';
import {
  Batch,
  Query,
  filter,
  write,
  ddl,
  admin,
} from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  connectAs,
  br,
  uniqueDbName,
  setupDb,
  seed,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

// principal_id gap FIXED: chown / addGroupMember now accept username strings,
// hash them to bigint via principalId(), and the framing layer encodes BigInt
// as msgpack uint64 (`useBigInt64: true`).  See e2e-principal.test.ts for the
// cross-language hash-match proof.

// ─── test suite ──────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e permissions (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let adminClient: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        adminClient = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error(
          '[e2e-permissions] connection failed. Server logs:\n' + server.logs(),
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

    // ── helper: create a SCRAM user and connect ──────────────────────────

    async function createUserAndConnect(
      name: string,
      password: string,
    ): Promise<ShamirClient> {
      await adminClient!.createScramUser(name, password, []);
      return connectAs(HOST, server!.port, name, password);
    }

    // ════════════════════════════════════════════════════════════════════
    //  A. CAPABILITY: denied -> grant -> allowed -> revoke -> denied
    // ════════════════════════════════════════════════════════════════════

    const USER_A = `perm_a_${process.pid}`;
    const USER_A_PW = 'alpha password for test';
    let userAClient: ShamirClient | null = null;
    let capDb: string;

    it('A-setup: create user A and restricted db/table', async () => {
      capDb = await setupDb(adminClient!, 'perm_cap', ['secrets']);

      // Seed some data as admin
      await seed(adminClient!, capDb, 'secrets', [
        {
          id: 'r1',
          payload: 'classified',
        },
      ]);

      // chmod the database to 0o700 (owner-only, execute on ancestors)
      br(await adminClient!.execute(capDb, {
        id: 'chmod-db',
        queries: {
          ch: admin.chmod(adminClient!, 
            admin.refDatabase(capDb),
            0o700,
          ),
        },
      }));

      // Create user A (no roles -> not superuser)
      userAClient = await createUserAndConnect(USER_A, USER_A_PW);
    });

    afterAll(async () => {
      if (userAClient) {
        try { await userAClient.close(); } catch { /* ok */ }
        userAClient = null;
      }
    }, 15_000);

    it('A1: user A denied — DDL (createDb) requires superuser', async () => {
      try {
        await userAClient!.execute('default', {
          id: 'unauth-createdb',
          queries: {
            c: ddl.createDb(uniqueDbName('should_fail')),
          },
        });
        expect.unreachable('should have thrown');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/permission_denied/);
      }
    });

    it('A2: user A denied — read on owner-only table', async () => {
      try {
        await Batch.create('unauth-read')
          .add('r', Query.from('secrets'))
          .execute(userAClient!, capDb);
        expect.unreachable('should have thrown');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    it('A3: user A denied — insert on owner-only table', async () => {
      try {
        await Batch.create('unauth-ins')
          .add('i', write.insert('secrets', [
            {
              id: 'hack',
              payload: 'injected',
            },
          ]))
          .execute(userAClient!, capDb);
        expect.unreachable('should have thrown');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    it('A4: admin chmod to 0o777 (open) -> user A CAN read', async () => {
      // Grant access by opening mode bits.
      // G.4c: new stores default to enforced (0o700), so the store must also
      // be opened for traversal-Execute to reach the table.
      br(await adminClient!.execute(capDb, {
        id: 'chmod-open',
        queries: {
          ch_db: admin.chmod(adminClient!, 
            admin.refDatabase(capDb),
            0o777,
          ),
          ch_store: admin.chmod(adminClient!, 
            admin.refStore(capDb, 'main'),
            0o777,
          ),
          ch_tbl: admin.chmod(adminClient!, 
            admin.refTable(capDb, 'main', 'secrets'),
            0o777,
          ),
        },
      }));

      // Now user A can read
      const resp = br(await Batch.create('auth-read')
        .add('r', Query.from('secrets'))
        .execute(userAClient!, capDb));
      expect(resp.results.r.records.length).toBe(1);
      expect(resp.results.r.records[0].payload).toBe('classified');
    });

    it('A5: admin chmod back to 0o700 -> user A denied again', async () => {
      br(await adminClient!.execute(capDb, {
        id: 'chmod-restrict',
        queries: {
          ch: admin.chmod(adminClient!, 
            admin.refDatabase(capDb),
            0o700,
          ),
        },
      }));

      try {
        await Batch.create('re-denied')
          .add('r', Query.from('secrets'))
          .execute(userAClient!, capDb);
        expect.unreachable('should have thrown after revoke');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    // ── A6: createGroup round-trip ──────────────────────────────────────

    it('A6: createGroup round-trip — group created and returned', async () => {
      // createGroup succeeds and returns a group_id.
      // addGroupMember/removeGroupMember are now exercised in
      // e2e-principal.test.ts (principalId gap is fixed).
      const grpResp = br(await adminClient!.execute(capDb, {
        id: 'mk-group',
        queries: {
          g: admin.createGroup(adminClient!, 'readers'),
        },
      }));
      const groupId = (grpResp.results.g.records[0] as Record<string, unknown>)
        .group_id as number;
      expect(typeof groupId).toBe('number');
      expect(groupId).toBeGreaterThan(0);
    });

    // ── A8: createRole / grantRole / revokeRole round-trip ─────────────

    it('A8: createRole/grantRole/revokeRole round-trip + superuser via SCRAM role', async () => {
      // SERVER MODEL (two-store, verified against the engine):
      //   Session roles — and therefore `is_superuser` — are read from the
      //   SCRAM user record's `roles`, set at createScramUser time and baked
      //   into the login ticket. grantRole/revokeRole mutate the SEPARATE
      //   RBAC system_store.users_table; they do NOT retroactively flip the
      //   superuser bit of an existing SCRAM login session. So admin
      //   capability is proven via a user CREATED with the 'superuser' role,
      //   while grantRole/revokeRole are exercised for builder/wire round-trip.

      // createRole round-trips (builder + server accept it).
      br(await adminClient!.execute('default', {
        id: 'mk-role',
        queries: {
          cr: admin.createRole(adminClient!, 'test_admin', [
            admin.permission('allow', ['all'], admin.scopeGlobal()),
          ]),
        },
      }));

      // grantRole / revokeRole round-trip on the RBAC layer — must not error.
      br(await adminClient!.execute('default', {
        id: 'mk-rbac-user',
        queries: {
          cu: admin.createUser(adminClient!, USER_A, USER_A_PW),
        },
      }));
      br(await adminClient!.execute('default', {
        id: 'grant-su',
        queries: {
          gr: admin.grantRole(adminClient!, 'superuser', USER_A),
        },
      }));
      br(await adminClient!.execute('default', {
        id: 'revoke-su',
        queries: {
          rv: admin.revokeRole(adminClient!, 'superuser', USER_A),
        },
      }));

      // Admin capability: a user created WITH the 'superuser' role gets an
      // is_superuser session and can run admin ops (listDatabases).
      const suUser = `perm_su_${process.pid}`;
      const suPw = 'superuser password for test';
      await adminClient!.createScramUser(suUser, suPw, ['superuser']);
      const suClient = await connectAs(HOST, server!.port, suUser, suPw);
      try {
        const resp = br(await Batch.create('su-list')
          .add('l', ddl.listDatabases())
          .execute(suClient, 'default'));
        const dbs = resp.results.l.records[0].databases as string[];
        expect(Array.isArray(dbs)).toBe(true);
        expect(dbs).toContain('default');
      } finally {
        await suClient.close();
      }

      // A non-superuser (USER_A logged in with roles=[]) is denied admin ops.
      try {
        await Batch.create('denied-list')
          .add('l', ddl.listDatabases())
          .execute(userAClient!, 'default');
        expect.unreachable('non-superuser must be denied listDatabases');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/permission_denied/);
      }
    });

    // ── A9: accessTree round-trip ──────────────────────────────────────

    it('A9: accessTree returns resource hierarchy (admin)', async () => {
      const resp = br(await adminClient!.execute(capDb, {
        id: 'access-tree',
        queries: {
          t: admin.accessTree({ db: capDb }),
        },
      }));
      // Response shape (server access_control.rs::access_tree):
      //   records[0] = { access_tree: { resources: { name, kind, owner, mode,
      //                  children }, functions: [...], principals: {users,groups} } }
      const rec = resp.results.t.records[0] as Record<string, unknown>;
      expect(rec).toBeDefined();
      const tree = rec.access_tree as Record<string, unknown>;
      expect(tree).toBeDefined();
      const resources = tree.resources as Record<string, unknown>;
      expect(typeof resources.name).toBe('string');
      expect(resources.kind).toBeDefined();
      expect(tree.principals).toBeDefined();
    });

    // ════════════════════════════════════════════════════════════════════
    //  B. VISIBILITY — document the REAL server model
    // ════════════════════════════════════════════════════════════════════
    //
    // The brief asks: "A.listDatabases() != B.listDatabases()".
    // The REAL server model:
    //   1. listDatabases is an admin op requiring superuser.
    //   2. Non-superusers get permission_denied (cannot list at all).
    //   3. Superusers see ALL databases — no per-db filtering.
    //   4. Access control is enforced per-operation on specific resources.
    //
    // Tests below prove this model.

    const USER_B = `perm_b_${process.pid}`;
    const USER_B_PW = 'bravo password for test';
    let userBClient: ShamirClient | null = null;
    let visDb1: string;
    let visDb2: string;

    it('B-setup: create two databases and two users', async () => {
      visDb1 = await setupDb(adminClient!, 'vis_db1', ['t1']);
      visDb2 = await setupDb(adminClient!, 'vis_db2', ['t2']);

      await seed(adminClient!, visDb1, 't1', [
        {
          id: 'd1r1',
          val: 'db1-data',
        },
      ]);
      await seed(adminClient!, visDb2, 't2', [
        {
          id: 'd2r1',
          val: 'db2-data',
        },
      ]);

      // chmod db1: 0o700 (owner-only)
      br(await adminClient!.execute(visDb1, {
        id: 'chmod-db1',
        queries: {
          ch: admin.chmod(adminClient!, 
            admin.refDatabase(visDb1),
            0o700,
          ),
        },
      }));

      // chmod db2: 0o700 (owner-only)
      br(await adminClient!.execute(visDb2, {
        id: 'chmod-db2',
        queries: {
          ch: admin.chmod(adminClient!, 
            admin.refDatabase(visDb2),
            0o700,
          ),
        },
      }));

      userBClient = await createUserAndConnect(USER_B, USER_B_PW);
    });

    afterAll(async () => {
      if (userBClient) {
        try { await userBClient.close(); } catch { /* ok */ }
        userBClient = null;
      }
    }, 15_000);

    it('B1: non-superuser A cannot listDatabases at all (permission_denied)', async () => {
      try {
        await Batch.create('vis-list-a')
          .add('l', ddl.listDatabases())
          .execute(userAClient!, 'default');
        expect.unreachable('non-superuser should be denied listDatabases');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/permission_denied/);
      }
    });

    it('B2: non-superuser B cannot listDatabases at all (permission_denied)', async () => {
      try {
        await Batch.create('vis-list-b')
          .add('l', ddl.listDatabases())
          .execute(userBClient!, 'default');
        expect.unreachable('non-superuser should be denied listDatabases');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/permission_denied/);
      }
    });

    it('B3: user A cannot read db1 (owner-only) without grant', async () => {
      try {
        await Batch.create('vis-a-db1')
          .add('r', Query.from('t1'))
          .execute(userAClient!, visDb1);
        expect.unreachable('A should be denied from db1');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    it('B4: user B cannot read db2 (owner-only) without grant', async () => {
      try {
        await Batch.create('vis-b-db2')
          .add('r', Query.from('t2'))
          .execute(userBClient!, visDb2);
        expect.unreachable('B should be denied from db2');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    it('B5: admin opens db1 to user A (chmod 0o777) -> A can read db1, still denied db2', async () => {
      // Open db1 to all.
      // G.4c: the store + table default to enforced (0o700); open them too so
      // traversal-Execute reaches the table.
      br(await adminClient!.execute(visDb1, {
        id: 'open-db1',
        queries: {
          ch_db: admin.chmod(adminClient!, 
            admin.refDatabase(visDb1),
            0o777,
          ),
          ch_store: admin.chmod(adminClient!, 
            admin.refStore(visDb1, 'main'),
            0o777,
          ),
          ch_tbl: admin.chmod(adminClient!, 
            admin.refTable(visDb1, 'main', 't1'),
            0o777,
          ),
        },
      }));

      // A can now read db1
      const resp = br(await Batch.create('a-reads-db1')
        .add('r', Query.from('t1'))
        .execute(userAClient!, visDb1));
      expect(resp.results.r.records.length).toBe(1);
      expect(resp.results.r.records[0].val).toBe('db1-data');

      // A still cannot read db2
      try {
        await Batch.create('a-denied-db2')
          .add('r', Query.from('t2'))
          .execute(userAClient!, visDb2);
        expect.unreachable('A should still be denied from db2');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    it('B6: admin opens db2 to user B (chmod 0o777) -> B can read db2, still denied db1 (re-restricted)', async () => {
      // Re-restrict db1
      br(await adminClient!.execute(visDb1, {
        id: 're-restrict-db1',
        queries: {
          ch: admin.chmod(adminClient!, 
            admin.refDatabase(visDb1),
            0o700,
          ),
        },
      }));

      // Open db2 to all.
      // G.4c: the store + table default to enforced (0o700); open them too.
      br(await adminClient!.execute(visDb2, {
        id: 'open-db2',
        queries: {
          ch_db: admin.chmod(adminClient!, 
            admin.refDatabase(visDb2),
            0o777,
          ),
          ch_store: admin.chmod(adminClient!, 
            admin.refStore(visDb2, 'main'),
            0o777,
          ),
          ch_tbl: admin.chmod(adminClient!, 
            admin.refTable(visDb2, 'main', 't2'),
            0o777,
          ),
        },
      }));

      // B can read db2
      const resp = br(await Batch.create('b-reads-db2')
        .add('r', Query.from('t2'))
        .execute(userBClient!, visDb2));
      expect(resp.results.r.records.length).toBe(1);
      expect(resp.results.r.records[0].val).toBe('db2-data');

      // B cannot read db1 (re-restricted)
      try {
        await Batch.create('b-denied-db1')
          .add('r', Query.from('t1'))
          .execute(userBClient!, visDb1);
        expect.unreachable('B should be denied from db1');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/access_denied/);
      }
    });

    it('B7: admin grants A access to db2 -> A can now read both', async () => {
      // Open db1 back for all.
      // G.4c: re-open the store + table too (they default to enforced).
      br(await adminClient!.execute(visDb1, {
        id: 'open-db1-again',
        queries: {
          ch_db: admin.chmod(adminClient!, 
            admin.refDatabase(visDb1),
            0o777,
          ),
          ch_store: admin.chmod(adminClient!, 
            admin.refStore(visDb1, 'main'),
            0o777,
          ),
          ch_tbl: admin.chmod(adminClient!, 
            admin.refTable(visDb1, 'main', 't1'),
            0o777,
          ),
        },
      }));

      // db2 is already 0o777 from B6
      // A can read both
      const r1 = br(await Batch.create('a-db1-final')
        .add('r', Query.from('t1'))
        .execute(userAClient!, visDb1));
      expect(r1.results.r.records[0].val).toBe('db1-data');

      const r2 = br(await Batch.create('a-db2-final')
        .add('r', Query.from('t2'))
        .execute(userAClient!, visDb2));
      expect(r2.results.r.records[0].val).toBe('db2-data');
    });

    it('B8: superuser listDatabases sees ALL databases (no per-db filtering)', async () => {
      // Admin (superuser) sees everything
      const resp = br(await Batch.create('admin-list-all')
        .add('l', ddl.listDatabases())
        .execute(adminClient!, 'default'));
      const dbs = resp.results.l.records[0].databases as string[];
      expect(dbs).toContain('default');
      expect(dbs).toContain(visDb1);
      expect(dbs).toContain(visDb2);
    });

    // ── A10: insert allowed after chmod grant ──────────────────────────

    it('A10: user A can insert after db is opened (data write)', async () => {
      // visDb1 is open from B7
      const resp = br(await Batch.create('a-insert')
        .add('i', write.insert('t1', [
          {
            id: 'a-new',
            val: 'written-by-A',
          },
        ]))
        .execute(userAClient!, visDb1));
      expect(resp.results.i.records.length).toBe(1);

      // Verify the record
      const verify = br(await Batch.create('a-verify')
        .add('r', Query.from('t1').where(filter.eq('id', 'a-new')))
        .execute(userAClient!, visDb1));
      expect(verify.results.r.records[0].val).toBe('written-by-A');
    });

    // ════════════════════════════════════════════════════════════════════
    //  G.3 (C3) — dropUser / dropRole / chgrp lifecycle round-trips
    // ════════════════════════════════════════════════════════════════════

    // ── G3-dropUser: create a SCRAM user, then dropUser (HMAC) ──────────

    it('G3-dropUser: createScramUser -> dropUser (HMAC) -> if_exists no-op', async () => {
      const name = `drop_u_${process.pid}_${Date.now()}`;
      const pw = 'drop user password';

      // Create the user via the SCRAM path (used everywhere else in the suite).
      await adminClient!.createScramUser(name, pw, []);

      // dropUser is HMAC-gated; the admin client IS the HmacSigner.
      // Dropping an existing user must succeed without error.
      br(await adminClient!.execute('default', {
        id: 'drop-user',
        queries: {
          d: admin.dropUser(adminClient!, name),
        },
      }));

      // Idempotency hardening: a second drop WITHOUT if_exists must fail
      // (user gone), but WITH if_exists:true it must be a no-op.
      try {
        await adminClient!.execute('default', {
          id: 'drop-user-again-strict',
          queries: {
            d: admin.dropUser(adminClient!, name),
          },
        });
        expect.unreachable('re-drop without if_exists should fail');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        // Server-side error for a missing user on a strict drop.
        expect(msg.length).toBeGreaterThan(0);
      }

      // With if_exists the re-drop is a clean no-op.
      br(await adminClient!.execute('default', {
        id: 'drop-user-again-if-exists',
        queries: {
          d: admin.dropUser(adminClient!, name, { if_exists: true }),
        },
      }));
    });

    // ── G3-dropRole: createRole -> dropRole (HMAC) ───────────────────────

    it('G3-dropRole: createRole -> dropRole (HMAC) -> if_exists no-op', async () => {
      const role = `g3_role_${process.pid}_${Date.now()}`;

      // createRole round-trips (builder + server accept it).
      br(await adminClient!.execute('default', {
        id: 'mk-role-g3',
        queries: {
          cr: admin.createRole(adminClient!, role, [
            admin.permission('allow', ['all'], admin.scopeGlobal()),
          ]),
        },
      }));

      // dropRole is HMAC-gated; dropping an existing role must succeed.
      br(await adminClient!.execute('default', {
        id: 'drop-role-g3',
        queries: {
          dr: admin.dropRole(adminClient!, role),
        },
      }));

      // Idempotency hardening: re-drop without if_exists fails, with it
      // succeeds (no-op).
      try {
        await adminClient!.execute('default', {
          id: 'drop-role-again-strict',
          queries: {
            dr: admin.dropRole(adminClient!, role),
          },
        });
        expect.unreachable('re-drop role without if_exists should fail');
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg.length).toBeGreaterThan(0);
      }

      br(await adminClient!.execute('default', {
        id: 'drop-role-again-if-exists',
        queries: {
          dr: admin.dropRole(adminClient!, role, { if_exists: true }),
        },
      }));
    });

    // ── G3-chgrp: createGroup -> chgrp(database) -> accessTree readback ─

    it('G3-chgrp: createGroup -> chgrp(database, gid) -> accessTree group readback', async () => {
      const db = await setupDb(adminClient!, 'g3_chgrp', ['g3tbl']);
      const groupName = `g3_grp_${process.pid}_${Date.now()}`;

      // createGroup returns a numeric group_id (admin_access.rs:153).
      const grpResp = br(await adminClient!.execute(db, {
        id: 'mk-group-g3',
        queries: {
          g: admin.createGroup(adminClient!, groupName),
        },
      }));
      const gid = (grpResp.results.g.records[0] as Record<string, unknown>)
        .group_id as number;
      expect(typeof gid).toBe('number');
      expect(gid).toBeGreaterThan(0);

      // chgrp the database resource to the new group (non-HMAC ACL op).
      const chgrpResp = br(await adminClient!.execute(db, {
        id: 'chgrp-db-g3',
        queries: {
          c: admin.chgrp(adminClient!, admin.refDatabase(db), gid),
        },
      }));
      const chgrpRow = chgrpResp.results.c.records[0] as Record<string, unknown>;
      // Echo: the server returns the applied group id.
      expect(chgrpRow.group).toBe(gid);

      // Persistence readback via accessTree. The access_tree node for a
      // database resource carries a `group` field (access_control.rs:586)
      // holding the numeric group id (or null). Find the db node and assert.
      const treeResp = br(await adminClient!.execute(db, {
        id: 'access-tree-g3',
        queries: {
          t: admin.accessTree({ db }),
        },
      }));
      const rec = treeResp.results.t.records[0] as Record<string, unknown>;
      const tree = rec.access_tree as Record<string, unknown>;
      const resources = tree.resources as Record<string, unknown>;
      // The tree root is { name:"/", kind:"root", children:[<db nodes>] }.
      // The db_filter restricted the tree to just our db, so the db node is
      // the single child of root (access_control.rs:481-525).
      expect(resources.kind).toBe('root');
      const dbChildren = resources.children as Array<Record<string, unknown>>;
      const dbNode = dbChildren.find(c => c.name === db);
      expect(dbNode).toBeDefined();
      // access_tree nodes carry a `group` field (access_control.rs:586)
      // holding the numeric group id (or null).
      expect(dbNode!.group).toBe(gid);
    });

    // ════════════════════════════════════════════════════════════════════
    //  G.4d (A2) — group-membership access grant path
    // ════════════════════════════════════════════════════════════════════

    const USER_G = `perm_g_${process.pid}`;
    const USER_G_PW = 'group user password';
    let gClient: ShamirClient | null = null;

    afterAll(async () => {
      if (gClient) {
        try { await gClient.close(); } catch { /* ok */ }
        gClient = null;
      }
    }, 15_000);

    it('A11/G4d-group: group membership + chgrp + group bits grant read; removal re-denies', async () => {
      // Fresh db + table, seeded as admin.
      const gdb = await setupDb(adminClient!, 'perm_grp', ['vault']);
      await seed(adminClient!, gdb, 'vault', [
        { id: 'g1', secret: 'group-only' },
      ]);

      // Fresh non-superuser user.
      gClient = await createUserAndConnect(USER_G, USER_G_PW);

      // Precondition: without group membership the user is denied
      // (enforced 0o700 default on all three resources).
      try {
        await Batch.create('g-denied-pre')
          .add('r', Query.from('vault'))
          .execute(gClient, gdb);
        expect.unreachable('should be denied before group grant');
      } catch (e: unknown) {
        expect((e as Error).message).toMatch(/access_denied/);
      }

      // Create a group and add the user.
      const grpResp = br(await adminClient!.execute(gdb, {
        id: 'mk-group-g4d',
        queries: {
          g: admin.createGroup(adminClient!, `g4d_grp_${process.pid}_${Date.now()}`),
        },
      }));
      const gid = (grpResp.results.g.records[0] as Record<string, unknown>)
        .group_id as number;
      expect(typeof gid).toBe('number');
      expect(gid).toBeGreaterThan(0);

      br(await adminClient!.execute(gdb, {
        id: 'add-member-g4d',
        queries: {
          a: admin.addGroupMember(adminClient!, admin.groupId(gid), USER_G),
        },
      }));

      // chgrp db + store + table to the group, then chmod 0o770
      // (owner-rwx + group-rwx: group gets x on ancestors for traversal
      // and r on the table; other = 0).
      br(await adminClient!.execute(gdb, {
        id: 'chgrp-g4d',
        queries: {
          cg_db: admin.chgrp(adminClient!, admin.refDatabase(gdb), gid),
          cg_store: admin.chgrp(adminClient!, admin.refStore(gdb, 'main'), gid),
          cg_tbl: admin.chgrp(adminClient!, admin.refTable(gdb, 'main', 'vault'), gid),
        },
      }));
      br(await adminClient!.execute(gdb, {
        id: 'chmod-g4d',
        queries: {
          cm_db: admin.chmod(adminClient!, admin.refDatabase(gdb), 0o770),
          cm_store: admin.chmod(adminClient!, admin.refStore(gdb, 'main'), 0o770),
          cm_tbl: admin.chmod(adminClient!, admin.refTable(gdb, 'main', 'vault'), 0o770),
        },
      }));

      // Now the user CAN read via group bits.
      const resp = br(await Batch.create('g-read-ok')
        .add('r', Query.from('vault'))
        .execute(gClient, gdb));
      expect(resp.results.r.records.length).toBe(1);
      expect(resp.results.r.records[0].secret).toBe('group-only');

      // Remove the user from the group → access re-denied
      // (group bits are still 0o770, but the user is no longer a member;
      // other = 0, so no fallback).
      br(await adminClient!.execute(gdb, {
        id: 'rm-member-g4d',
        queries: {
          r: admin.removeGroupMember(adminClient!, admin.groupId(gid), USER_G),
        },
      }));

      try {
        await Batch.create('g-denied-post')
          .add('r', Query.from('vault'))
          .execute(gClient, gdb);
        expect.unreachable('should be denied after group removal');
      } catch (e: unknown) {
        expect((e as Error).message).toMatch(/access_denied/);
      }
    });
  },
);

describe('e2e-permissions.test skip reason', () => {
  it('reports why the permissions e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-permissions] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
