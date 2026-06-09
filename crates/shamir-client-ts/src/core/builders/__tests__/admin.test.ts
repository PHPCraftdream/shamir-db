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
import type { Action } from '../../types/admin.js';
import {
  canonicalDropUser,
  canonicalDropRole,
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

describe('chmod', () => {
  it('emits {chmod: ResourceRef, mode}', () => {
    const op = admin.chmod(admin.refTable('db', 'main', 'users'), 0o740);
    expect(op).toEqual({
      chmod: { table: ['db', 'main', 'users'] },
      mode: 0o740,
    });
  });
});

describe('chown', () => {
  it('emits {chown: ResourceRef, owner}', () => {
    const op = admin.chown(admin.refDatabase('mydb'), 7);
    expect(op).toEqual({
      chown: { database: 'mydb' },
      owner: 7,
    });
  });
});

describe('chgrp', () => {
  it('emits {chgrp: ResourceRef, group: number}', () => {
    const op = admin.chgrp(admin.refStore('db', 'main'), 3);
    expect(op).toEqual({
      chgrp: { store: ['db', 'main'] },
      group: 3,
    });
  });

  it('group:null clears the group', () => {
    const op = admin.chgrp(admin.refDatabase('mydb'), null);
    expect(op).toEqual({
      chgrp: { database: 'mydb' },
      group: null,
    });
  });
});

describe('createGroup', () => {
  it('emits {create_group: name}', () => {
    expect(admin.createGroup('devs')).toEqual({ create_group: 'devs' });
  });
});

describe('dropGroup', () => {
  it('by name', () => {
    const op = admin.dropGroup(admin.groupName('devs'));
    expect(op).toEqual({ drop_group: { name: 'devs' } });
  });

  it('by id', () => {
    const op = admin.dropGroup(admin.groupId(3));
    expect(op).toEqual({ drop_group: { id: 3 } });
  });
});

describe('addGroupMember', () => {
  it('emits {add_group_member: GroupRef, user}', () => {
    const op = admin.addGroupMember(admin.groupName('devs'), 42);
    expect(op).toEqual({
      add_group_member: { name: 'devs' },
      user: 42,
    });
  });
});

describe('removeGroupMember', () => {
  it('emits {remove_group_member: GroupRef, user}', () => {
    const op = admin.removeGroupMember(admin.groupId(5), 7);
    expect(op).toEqual({
      remove_group_member: { id: 5 },
      user: 7,
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

describe('createUser', () => {
  it('emits roles:[] by default (always present)', () => {
    const op = admin.createUser('alice', 's3cret');
    expect(op).toEqual({
      create_user: 'alice',
      password: 's3cret',
      roles: [],
    });
    expect(op).not.toHaveProperty('profile');
    expect(op).not.toHaveProperty('database');
  });

  it('with roles, profile, database', () => {
    const op = admin.createUser('bob', 'pw', {
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
    });
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

describe('createRole', () => {
  it('emits {create_role, permissions}', () => {
    const perms = [
      admin.permission('allow', ['read'], admin.scopeDatabase('mydb')),
    ];
    const op = admin.createRole('reader', perms);
    expect(op).toEqual({
      create_role: 'reader',
      permissions: [
        {
          effect: 'allow',
          actions: ['read'],
          resource: { scope: 'database', database: 'mydb' },
        },
      ],
    });
  });
});

describe('dropRole (HMAC)', () => {
  it('hmac = signer over canonicalDropRole(role)', () => {
    const role = 'admin';
    const canonical = canonicalDropRole(role);
    const op = admin.dropRole(fakeSigner, role);
    expect(op).toEqual({
      drop_role: 'admin',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('grantRole', () => {
  it('emits {grant_role, user}', () => {
    const op = admin.grantRole('reader', 'alice');
    expect(op).toEqual({ grant_role: 'reader', user: 'alice' });
  });
});

describe('revokeRole', () => {
  it('emits {revoke_role, user}', () => {
    const op = admin.revokeRole('reader', 'bob');
    expect(op).toEqual({ revoke_role: 'reader', user: 'bob' });
  });
});
