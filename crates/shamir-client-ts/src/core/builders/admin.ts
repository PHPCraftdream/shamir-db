/**
 * Access-control (ACL) + RBAC admin operation builders — the CODE that
 * constructs the wire shapes declared in `../types/admin.ts`. Mirrors
 * `crates/shamir-query-types/src/admin/access.rs` and
 * `crates/shamir-query-types/src/auth/types.rs`.
 *
 * Non-HMAC ops are plain functions returning the wire object.
 * HMAC-gated ops (`drop_user`, `drop_role`) take a `signer: HmacSigner`,
 * build the canonical input via `../hmac.ts`, and attach the HMAC tag.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  HmacSigner,
  ResourceRef,
  GroupRef,
  Resource,
  Action,
  Effect,
  Permission,
  ChmodOp,
  ChownOp,
  ChgrpOp,
  CreateGroupOp,
  DropGroupOp,
  RenameGroupOp,
  AddGroupMemberOp,
  RemoveGroupMemberOp,
  AccessTreeOp,
  CreateUserOp,
  DropUserOp,
  CreateRoleOp,
  DropRoleOp,
  GrantRoleOp,
  RevokeRoleOp,
  RenameRoleOp,
} from '../types/admin.js';

import type { Filter } from '../types/filter.js';
import type { WireValue } from '../types/write.js';

import {
  canonicalDropUser,
  canonicalDropRole,
} from '../hmac.js';

import { principalId } from '../principal-id.js';

// ── ResourceRef constructors (access.rs, untagged, single-key) ──────

export function refDatabase(db: string): ResourceRef {
  return { database: db };
}

export function refStore(db: string, store: string): ResourceRef {
  return { store: [db, store] };
}

export function refTable(db: string, store: string, table: string): ResourceRef {
  return { table: [db, store, table] };
}

export function refFunction(name: string): ResourceRef {
  return { function: name };
}

export function refFunctionFolder(segs: string[]): ResourceRef {
  return { function_folder: segs };
}

export function refFunctionNamespace(): ResourceRef {
  return { function_namespace: true };
}

// ── Resource (permission scope) constructors (auth/types.rs, tag="scope") ─

export function scopeGlobal(): Resource {
  return { scope: 'global' };
}

export function scopeDatabase(db: string): Resource {
  return { scope: 'database', database: db };
}

export function scopeRepo(db: string, repo: string): Resource {
  return { scope: 'repo', database: db, repo };
}

export function scopeTable(db: string, repo: string, table: string): Resource {
  return { scope: 'table', database: db, repo, table };
}

// ── GroupRef constructors ───────────────────────────────────────────

export function groupName(name: string): GroupRef {
  return { name };
}

export function groupId(id: number): GroupRef {
  return { id };
}

// ── ACL ops (all NON-HMAC) ──────────────────────────────────────────

export function chmod(resource: ResourceRef, mode: number): ChmodOp {
  return { chmod: resource, mode };
}

/**
 * Transfer ownership of a resource.
 *
 * `owner` accepts:
 *   - `string`  — username, hashed to `principalId(username)` (bigint).
 *   - `bigint`  — pre-computed principal id.
 *   - `number`  — raw numeric id (only safe for values <= 2^53).
 */
export function chown(resource: ResourceRef, owner: string | bigint | number): ChownOp {
  const resolved = typeof owner === 'string' ? principalId(owner) : owner;
  return { chown: resource, owner: resolved };
}

export function chgrp(resource: ResourceRef, group: number | null): ChgrpOp {
  return { chgrp: resource, group };
}

export function createGroup(name: string): CreateGroupOp {
  return { create_group: name };
}

export function dropGroup(
  ref: GroupRef,
  opts?: { if_exists?: boolean },
): DropGroupOp {
  const op: DropGroupOp = { drop_group: ref };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/**
 * Rename a group. Groups are id-keyed, so this only updates the display
 * name; members and resource references (which store the group id) are
 * unaffected.
 */
export function renameGroup(ref: GroupRef, to: string): RenameGroupOp {
  return { rename_group: ref, to };
}

/**
 * Add a user to a group.
 *
 * `user` accepts:
 *   - `string`  — username, hashed to `principalId(username)` (bigint).
 *   - `bigint`  — pre-computed principal id.
 *   - `number`  — raw numeric id (only safe for values <= 2^53).
 */
export function addGroupMember(ref: GroupRef, user: string | bigint | number): AddGroupMemberOp {
  const resolved = typeof user === 'string' ? principalId(user) : user;
  return { add_group_member: ref, user: resolved };
}

/**
 * Remove a user from a group.
 *
 * `user` accepts:
 *   - `string`  — username, hashed to `principalId(username)` (bigint).
 *   - `bigint`  — pre-computed principal id.
 *   - `number`  — raw numeric id (only safe for values <= 2^53).
 */
export function removeGroupMember(ref: GroupRef, user: string | bigint | number): RemoveGroupMemberOp {
  const resolved = typeof user === 'string' ? principalId(user) : user;
  return { remove_group_member: ref, user: resolved };
}

export function accessTree(opts?: { depth?: number; db?: string }): AccessTreeOp {
  const op: AccessTreeOp = { access_tree: true };
  if (opts?.depth !== undefined) op.depth = opts.depth;
  if (opts?.db !== undefined) op.db = opts.db;
  return op;
}

// ── RBAC ops ────────────────────────────────────────────────────────

export function permission(
  effect: Effect,
  actions: Action[],
  resource: Resource,
  opts?: { where?: Filter },
): Permission {
  const p: Permission = { effect, actions, resource };
  if (opts?.where !== undefined) p.where = opts.where;
  return p;
}

/**
 * Create a user. `roles` is `#[serde(default)]` WITHOUT skip → always
 * present on the wire. Emits `roles: []` when none provided.
 * `password` is a SecretString on the Rust side → plain string on wire.
 */
export function createUser(
  name: string,
  password: string,
  opts?: { roles?: string[]; profile?: WireValue; database?: string },
): CreateUserOp {
  const op: CreateUserOp = {
    create_user: name,
    password,
    roles: opts?.roles ?? [],
  };
  if (opts?.profile !== undefined) op.profile = opts.profile;
  if (opts?.database !== undefined) op.database = opts.database;
  return op;
}

/** Drop a user (HMAC-gated). canonical = `canonicalDropUser(username)`. */
export function dropUser(
  signer: HmacSigner,
  username: string,
  opts?: { if_exists?: boolean },
): DropUserOp {
  const canonical = canonicalDropUser(username);
  const op: DropUserOp = {
    drop_user: username,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

export function createRole(
  name: string,
  permissions: Permission[],
): CreateRoleOp {
  return { create_role: name, permissions };
}

/** Drop a role (HMAC-gated). canonical = `canonicalDropRole(role)`. */
export function dropRole(
  signer: HmacSigner,
  role: string,
  opts?: { if_exists?: boolean },
): DropRoleOp {
  const canonical = canonicalDropRole(role);
  const op: DropRoleOp = {
    drop_role: role,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

export function grantRole(role: string, user: string): GrantRoleOp {
  return { grant_role: role, user };
}

export function revokeRole(role: string, user: string): RevokeRoleOp {
  return { revoke_role: role, user };
}

/**
 * Rename a role. Re-keys the role record and updates the `roles` list of
 * every user that holds the old name.
 */
export function renameRole(from: string, to: string): RenameRoleOp {
  return { rename_role: from, to };
}

/** Aggregate namespace — every admin constructor in one object. */
export const admin = {
  refDatabase,
  refStore,
  refTable,
  refFunction,
  refFunctionFolder,
  refFunctionNamespace,
  scopeGlobal,
  scopeDatabase,
  scopeRepo,
  scopeTable,
  groupName,
  groupId,
  chmod,
  chown,
  chgrp,
  createGroup,
  dropGroup,
  renameGroup,
  addGroupMember,
  removeGroupMember,
  accessTree,
  permission,
  createUser,
  dropUser,
  createRole,
  dropRole,
  grantRole,
  revokeRole,
  renameRole,
};
