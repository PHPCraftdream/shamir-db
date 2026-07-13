/**
 * Admin-builder wire-shape tests (ACL + RBAC).
 *
 * The authority for every shape is:
 *   - ACL ops: `crates/shamir-query-types/src/admin/access.rs`
 *   - RBAC ops: `crates/shamir-query-types/src/auth/types.rs`
 * Cross-checked with e2e tests `tests/e2e/tests/08-admin-ddl.test.js`,
 * `12-hmac-gate.test.js`.
 */

import { describe, it, expect } from 'vitest';
import { admin } from '../admin.js';
import type { Action, SetSuperuserOp } from '../../types/admin.js';
import {
  canonicalDropUser,
  canonicalChmod,
  canonicalChown,
  canonicalChgrp,
  canonicalCreateUser,
  canonicalGrantRole,
  canonicalRevokeRole,
  canonicalSetSuperuser,
  canonicalCreateGroup,
  canonicalDropGroup,
  canonicalRenameGroup,
  canonicalAddGroupMember,
  canonicalRemoveGroupMember,
} from '../../hmac.js';

/** Fake signer that returns a predictable tag based on canonical length. */
const fakeSigner = {
  hmacTagHex: (c: Uint8Array): string => 'tag:' + c.length,
};

// ── ResourceRef constructors (untagged, single-key) ─────────────────

describe('ResourceRef (untagged, single-key)', () => {
  it('refDatabase → {database}', () => {
    expect(admin.refDatabase('mydb')).toEqual({ database: 'mydb' });
  });

  it('refStore → {store: [db, store]}', () => {
    expect(admin.refStore('mydb', 'main')).toEqual({
      store: ['mydb', 'main'],
    });
  });

  it('refTable → {table: [db, store, table]}', () => {
    expect(admin.refTable('mydb', 'main', 'users')).toEqual({
      table: ['mydb', 'main', 'users'],
    });
  });

  it('refFunction → {function}', () => {
    expect(admin.refFunction('my_fn')).toEqual({ function: 'my_fn' });
  });

  it('refFunctionFolder → {function_folder: string[]}', () => {
    expect(admin.refFunctionFolder(['reports', 'daily'])).toEqual({
      function_folder: ['reports', 'daily'],
    });
  });

  it('refFunctionNamespace → {function_namespace: true}', () => {
    expect(admin.refFunctionNamespace()).toEqual({ function_namespace: true });
  });
});

// ── Resource (permission scope, tag="scope") ────────────────────────

describe('Resource (tag="scope")', () => {
  it('scopeGlobal → {scope: "global"}', () => {
    expect(admin.scopeGlobal()).toEqual({ scope: 'global' });
  });

  it('scopeDatabase → {scope:"database", database}', () => {
    expect(admin.scopeDatabase('mydb')).toEqual({
      scope: 'database',
      database: 'mydb',
    });
  });

  it('scopeRepo → {scope:"repo", database, repo}', () => {
    expect(admin.scopeRepo('mydb', 'main')).toEqual({
      scope: 'repo',
      database: 'mydb',
      repo: 'main',
    });
  });

  it('scopeTable → {scope:"table", database, repo, table}', () => {
    expect(admin.scopeTable('mydb', 'main', 'users')).toEqual({
      scope: 'table',
      database: 'mydb',
      repo: 'main',
      table: 'users',
    });
  });
});

// ── ResourceRef vs Resource are distinct shapes ──────────────────────

describe('ResourceRef ≠ Resource (distinct wire shapes)', () => {
  it('refDatabase is single-key, NOT tagged', () => {
    const ref = admin.refDatabase('mydb');
    expect(ref).toEqual({ database: 'mydb' });
    expect('scope' in ref).toBe(false);
  });

  it('scopeDatabase IS tagged', () => {
    const res = admin.scopeDatabase('mydb');
    expect(res).toEqual({ scope: 'database', database: 'mydb' });
    expect('scope' in res).toBe(true);
  });

  it('refTable is untagged array, scopeTable is tagged object', () => {
    const ref = admin.refTable('db', 'main', 'users');
    const res = admin.scopeTable('db', 'main', 'users');
    // refTable: single key "table" with array value
    expect(ref).toEqual({ table: ['db', 'main', 'users'] });
    // scopeTable: tagged "scope" with separate fields
    expect(res).toEqual({
      scope: 'table',
      database: 'db',
      repo: 'main',
      table: 'users',
    });
  });
});

// ── GroupRef constructors ───────────────────────────────────────────

describe('GroupRef', () => {
  it('groupName → {name}', () => {
    expect(admin.groupName('devs')).toEqual({ name: 'devs' });
  });

  it('groupId → {id}', () => {
    expect(admin.groupId(3)).toEqual({ id: 3 });
  });
});

// ── ACL ops ─────────────────────────────────────────────────────────

describe('chmod (HMAC)', () => {
  it('emits {chmod: ResourceRef, mode, hmac}', () => {
    const resource = admin.refTable('db', 'main', 'users');
    const canonical = canonicalChmod(resource, 0o740);
    const op = admin.chmod(fakeSigner, resource, 0o740);
    expect(op).toEqual({
      chmod: { table: ['db', 'main', 'users'] },
      mode: 0o740,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('chown (HMAC)', () => {
  it('emits {chown: ResourceRef, owner, hmac} with number', () => {
    const resource = admin.refDatabase('mydb');
    const canonical = canonicalChown(resource, 7);
    const op = admin.chown(fakeSigner, resource, 7);
    expect(op).toEqual({
      chown: { database: 'mydb' },
      owner: 7,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('accepts bigint owner', () => {
    const resource = admin.refDatabase('mydb');
    const op = admin.chown(fakeSigner, resource, 42n);
    expect(op).toEqual({
      chown: { database: 'mydb' },
      owner: 42n,
      hmac: fakeSigner.hmacTagHex(canonicalChown(resource, 42n)),
    });
  });

  it('string username is a compile-time error (task #569 removed the overload)', () => {
    // The string overload was removed: a username MUST be resolved to its
    // real principal64 first via `ShamirClient.resolvePrincipal(username)`.
    // `@ts-expect-error` turns a regression (re-adding the string arm) into
    // a compile error — same pattern as batch.test.ts's records_idmsgpack
    // compile-fail guard.
    const resource = admin.refDatabase('mydb');
    // @ts-expect-error string username is no longer accepted; pass bigint|number
    const op = admin.chown(fakeSigner, resource, 'alice');
    // At runtime the string slips through (TS types are erased), but the
    // owner field echoes it verbatim now — no silent hashing to a wrong id.
    expect((op as { owner: unknown }).owner).toBe('alice');
  });
});

describe('chgrp (HMAC)', () => {
  it('emits {chgrp: ResourceRef, group: number, hmac}', () => {
    const resource = admin.refStore('db', 'main');
    const canonical = canonicalChgrp(resource, 3);
    const op = admin.chgrp(fakeSigner, resource, 3);
    expect(op).toEqual({
      chgrp: { store: ['db', 'main'] },
      group: 3,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('group:null clears the group', () => {
    const resource = admin.refDatabase('mydb');
    const canonical = canonicalChgrp(resource, null);
    const op = admin.chgrp(fakeSigner, resource, null);
    expect(op).toEqual({
      chgrp: { database: 'mydb' },
      group: null,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('createGroup', () => {
  it('emits {create_group: name, hmac} — hmac = signer over canonicalCreateGroup(name)', () => {
    const canonical = canonicalCreateGroup('devs');
    const op = admin.createGroup(fakeSigner, 'devs');
    expect(op).toEqual({
      create_group: 'devs',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('dropGroup', () => {
  it('by name — hmac = signer over canonicalDropGroup(ref)', () => {
    const ref = admin.groupName('devs');
    const canonical = canonicalDropGroup(ref);
    const op = admin.dropGroup(fakeSigner, ref);
    expect(op).toEqual({
      drop_group: { name: 'devs' },
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('by id', () => {
    const ref = admin.groupId(3);
    const canonical = canonicalDropGroup(ref);
    const op = admin.dropGroup(fakeSigner, ref);
    expect(op).toEqual({
      drop_group: { id: 3 },
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('emits if_exists when true', () => {
    const op = admin.dropGroup(fakeSigner, admin.groupName('devs'), { if_exists: true });
    expect(op.if_exists).toBe(true);
  });
});

describe('renameGroup', () => {
  it('by name — hmac = signer over canonicalRenameGroup(ref, to)', () => {
    const ref = admin.groupName('devs');
    const canonical = canonicalRenameGroup(ref, 'engineers');
    const op = admin.renameGroup(fakeSigner, ref, 'engineers');
    expect(op).toEqual({
      rename_group: { name: 'devs' },
      to: 'engineers',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('by id', () => {
    const ref = admin.groupId(3);
    const canonical = canonicalRenameGroup(ref, 'engineers');
    const op = admin.renameGroup(fakeSigner, ref, 'engineers');
    expect(op).toEqual({
      rename_group: { id: 3 },
      to: 'engineers',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('addGroupMember', () => {
  it('emits {add_group_member: GroupRef, user, hmac} with number', () => {
    const ref = admin.groupName('devs');
    const canonical = canonicalAddGroupMember(ref, 42);
    const op = admin.addGroupMember(fakeSigner, ref, 42);
    expect(op).toEqual({
      add_group_member: { name: 'devs' },
      user: 42,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('string username is a compile-time error (task #569 removed the overload)', () => {
    // The string overload was removed: a username MUST be resolved to its
    // real principal64 first via `ShamirClient.resolvePrincipal(username)`.
    const ref = admin.groupName('devs');
    // @ts-expect-error string username is no longer accepted; pass bigint|number
    const op = admin.addGroupMember(fakeSigner, ref, 'bob');
    expect((op as { user: unknown }).user).toBe('bob');
  });
});

describe('removeGroupMember', () => {
  it('emits {remove_group_member: GroupRef, user, hmac}', () => {
    const ref = admin.groupId(5);
    const canonical = canonicalRemoveGroupMember(ref, 7);
    const op = admin.removeGroupMember(fakeSigner, ref, 7);
    expect(op).toEqual({
      remove_group_member: { id: 5 },
      user: 7,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('accessTree', () => {
  it('bare → {access_tree: true}', () => {
    expect(admin.accessTree()).toEqual({ access_tree: true });
  });

  it('with depth', () => {
    expect(admin.accessTree({ depth: 2 })).toEqual({
      access_tree: true,
      depth: 2,
    });
  });

  it('with db filter', () => {
    expect(admin.accessTree({ db: 'mydb' })).toEqual({
      access_tree: true,
      db: 'mydb',
    });
  });

  it('with both depth and db', () => {
    expect(admin.accessTree({ depth: 1, db: 'mydb' })).toEqual({
      access_tree: true,
      depth: 1,
      db: 'mydb',
    });
  });
});

// ── RBAC ops ────────────────────────────────────────────────────────

describe('permission', () => {
  it('bare → {effect, actions, resource}', () => {
    const op = admin.permission(
      'allow',
      ['read', 'insert'],
      admin.scopeTable('db', 'main', 'users'),
    );
    expect(op).toEqual({
      effect: 'allow',
      actions: ['read', 'insert'],
      resource: { scope: 'table', database: 'db', repo: 'main', table: 'users' },
    });
    expect(op).not.toHaveProperty('where');
  });

  it('with where filter', () => {
    const filter = { op: 'eq' as const, field: ['status'], value: 'active' };
    const op = admin.permission(
      'deny',
      ['delete'],
      admin.scopeRepo('db', 'main'),
      { where: filter },
    );
    expect(op).toEqual({
      effect: 'deny',
      actions: ['delete'],
      resource: { scope: 'repo', database: 'db', repo: 'main' },
      where: filter,
    });
  });

  it('all Action values are snake_case strings', () => {
    const actions: Action[] = [
      'read', 'insert', 'update', 'delete', 'create',
      'drop', 'alter', 'manage_users', 'manage_roles', 'all',
    ];
    const op = admin.permission('allow', actions, admin.scopeGlobal());
    expect(op.actions).toEqual(actions);
  });

  it('Effect values are lowercase', () => {
    const allow = admin.permission('allow', ['read'], admin.scopeGlobal());
    const deny = admin.permission('deny', ['read'], admin.scopeGlobal());
    expect(allow.effect).toBe('allow');
    expect(deny.effect).toBe('deny');
  });
});

describe('createUser (HMAC)', () => {
  it('emits roles:[] by default (always present), hmac over username only', () => {
    const canonical = canonicalCreateUser('alice');
    const op = admin.createUser(fakeSigner, 'alice', 's3cret');
    expect(op).toEqual({
      create_user: 'alice',
      password: 's3cret',
      roles: [],
      hmac: fakeSigner.hmacTagHex(canonical),
    });
    expect(op).not.toHaveProperty('profile');
    expect(op).not.toHaveProperty('database');
  });

  it('with roles, profile, database', () => {
    const canonical = canonicalCreateUser('bob');
    const op = admin.createUser(fakeSigner, 'bob', 'pw', {
      roles: ['admin'],
      profile: { department: 'eng' },
      database: 'mydb',
    });
    expect(op).toEqual({
      create_user: 'bob',
      password: 'pw',
      roles: ['admin'],
      profile: { department: 'eng' },
      database: 'mydb',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('hmac canonical never includes the password', () => {
    const opA = admin.createUser(fakeSigner, 'carol', 'pw-one');
    const opB = admin.createUser(fakeSigner, 'carol', 'pw-two-totally-different');
    expect(opA.hmac).toBe(opB.hmac);
  });
});

describe('dropUser (HMAC)', () => {
  it('hmac = signer over canonicalDropUser(username)', () => {
    const username = 'alice';
    const canonical = canonicalDropUser(username);
    const op = admin.dropUser(fakeSigner, username);
    expect(op).toEqual({
      drop_user: 'alice',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

// ── setSuperuser (top-level DbRequest, HMAC-gated) ──────────────────

describe('setSuperuser (HMAC)', () => {
  it('emits {op:"set_superuser", user, on, hmac} — grant (on=true)', () => {
    const canonical = canonicalSetSuperuser('carol', true);
    const op = admin.setSuperuser(fakeSigner, 'carol', true);
    const expected: SetSuperuserOp = {
      op: 'set_superuser',
      user: 'carol',
      on: true,
      hmac: fakeSigner.hmacTagHex(canonical),
    };
    expect(op).toEqual(expected);
  });

  it('emits {op:"set_superuser", user, on, hmac} — revoke (on=false)', () => {
    const canonical = canonicalSetSuperuser('dave', false);
    const op = admin.setSuperuser(fakeSigner, 'dave', false);
    expect(op).toEqual({
      op: 'set_superuser',
      user: 'dave',
      on: false,
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('hmac is unconditional — always present', () => {
    expect(admin.setSuperuser(fakeSigner, 'eve', true).hmac).toBe(
      fakeSigner.hmacTagHex(canonicalSetSuperuser('eve', true)),
    );
    expect(admin.setSuperuser(fakeSigner, 'eve', false).hmac).toBe(
      fakeSigner.hmacTagHex(canonicalSetSuperuser('eve', false)),
    );
  });

  it('on=true and on=false produce distinct canonical bytes (and tags)', () => {
    const grantTag = admin.setSuperuser(fakeSigner, 'frank', true).hmac;
    const revokeTag = admin.setSuperuser(fakeSigner, 'frank', false).hmac;
    expect(grantTag).not.toBe(revokeTag);
  });
});

describe('grantRole (HMAC)', () => {
  it('emits {grant_role, user, hmac}', () => {
    const canonical = canonicalGrantRole('reader', 'alice');
    const op = admin.grantRole(fakeSigner, 'reader', 'alice');
    expect(op).toEqual({
      grant_role: 'reader',
      user: 'alice',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('revokeRole (HMAC)', () => {
  it('emits {revoke_role, user, hmac}', () => {
    const canonical = canonicalRevokeRole('reader', 'bob');
    const op = admin.revokeRole(fakeSigner, 'reader', 'bob');
    expect(op).toEqual({
      revoke_role: 'reader',
      user: 'bob',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

// ── if_exists on admin drop ops ────────────────────────────────────

describe('if_exists on admin drop ops', () => {
  it('dropGroup omits if_exists when not set', () => {
    const op = admin.dropGroup(fakeSigner, admin.groupName('devs'));
    expect(op).not.toHaveProperty('if_exists');
  });

  it('dropUser emits if_exists when true', () => {
    const op = admin.dropUser(fakeSigner, 'alice', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropUser omits if_exists when not set', () => {
    const op = admin.dropUser(fakeSigner, 'alice');
    expect(op).not.toHaveProperty('if_exists');
  });
});
