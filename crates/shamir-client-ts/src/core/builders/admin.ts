/**
 * Access-control (ACL) + RBAC admin operation builders — the CODE that
 * constructs the wire shapes declared in `../types/admin.ts`. Mirrors
 * `crates/shamir-query-types/src/admin/access.rs` and
 * `crates/shamir-query-types/src/auth/types.rs`.
 *
 * Non-HMAC ops are plain functions returning the wire object.
 * HMAC-gated ops (`drop_user`, `drop_role`, `chmod`, `chown`, `chgrp`,
 * `create_user`, `create_role`, `grant_role`, `revoke_role`,
 * `create_group`, `drop_group`, `rename_group`, `add_group_member`,
 * `remove_group_member`) take a `signer: HmacSigner`, build the canonical
 * input via `../hmac.ts`, and attach the HMAC tag.
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
  canonicalChmod,
  canonicalChown,
  canonicalChgrp,
  canonicalCreateUser,
  canonicalCreateRole,
  canonicalGrantRole,
  canonicalRevokeRole,
  canonicalCreateGroup,
  canonicalDropGroup,
  canonicalRenameGroup,
  canonicalAddGroupMember,
  canonicalRemoveGroupMember,
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

// ── ACL ops (chmod / chown / chgrp — HMAC-gated) ────────────────────

/** Change mode bits on a resource (HMAC-gated). canonical = `canonicalChmod(resource, mode)`. */
export function chmod(signer: HmacSigner, resource: ResourceRef, mode: number): ChmodOp {
  const canonical = canonicalChmod(resource, mode);
  return { chmod: resource, mode, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Transfer ownership of a resource (HMAC-gated).
 * canonical = `canonicalChown(resource, owner)`.
 *
 * `owner` accepts:
 *   - `string`  — username, hashed to `principalId(username)` (bigint).
 *   - `bigint`  — pre-computed principal id.
 *   - `number`  — raw numeric id (only safe for values <= 2^53).
 */
export function chown(
  signer: HmacSigner,
  resource: ResourceRef,
  owner: string | bigint | number,
): ChownOp {
  const resolved = typeof owner === 'string' ? principalId(owner) : owner;
  const canonical = canonicalChown(resource, resolved);
  return { chown: resource, owner: resolved, hmac: signer.hmacTagHex(canonical) };
}

/** Change group on a resource (HMAC-gated). canonical = `canonicalChgrp(resource, group)`. */
export function chgrp(signer: HmacSigner, resource: ResourceRef, group: number | null): ChgrpOp {
  const canonical = canonicalChgrp(resource, group);
  return { chgrp: resource, group, hmac: signer.hmacTagHex(canonical) };
}

/** Create a new group (HMAC-gated). canonical = `canonicalCreateGroup(name)`. */
export function createGroup(signer: HmacSigner, name: string): CreateGroupOp {
  const canonical = canonicalCreateGroup(name);
  return { create_group: name, hmac: signer.hmacTagHex(canonical) };
}

/** Drop a group by reference (HMAC-gated). canonical = `canonicalDropGroup(ref)`. */
export function dropGroup(
  signer: HmacSigner,
  ref: GroupRef,
  opts?: { if_exists?: boolean },
): DropGroupOp {
  const canonical = canonicalDropGroup(ref);
  const op: DropGroupOp = { drop_group: ref, hmac: signer.hmacTagHex(canonical) };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/**
 * Rename a group (HMAC-gated). Groups are id-keyed, so this only updates
 * the display name; members and resource references (which store the
 * group id) are unaffected. canonical = `canonicalRenameGroup(ref, to)`.
 */
export function renameGroup(signer: HmacSigner, ref: GroupRef, to: string): RenameGroupOp {
  const canonical = canonicalRenameGroup(ref, to);
  return { rename_group: ref, to, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Add a user to a group (HMAC-gated).
 * canonical = `canonicalAddGroupMember(ref, user)`.
 *
 * `user` accepts:
 *   - `string`  — username, hashed to `principalId(username)` (bigint).
 *   - `bigint`  — pre-computed principal id.
 *   - `number`  — raw numeric id (only safe for values <= 2^53).
 */
export function addGroupMember(
  signer: HmacSigner,
  ref: GroupRef,
  user: string | bigint | number,
): AddGroupMemberOp {
  const resolved = typeof user === 'string' ? principalId(user) : user;
  const canonical = canonicalAddGroupMember(ref, resolved);
  return { add_group_member: ref, user: resolved, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Remove a user from a group (HMAC-gated).
 * canonical = `canonicalRemoveGroupMember(ref, user)`.
 *
 * `user` accepts:
 *   - `string`  — username, hashed to `principalId(username)` (bigint).
 *   - `bigint`  — pre-computed principal id.
 *   - `number`  — raw numeric id (only safe for values <= 2^53).
 */
export function removeGroupMember(
  signer: HmacSigner,
  ref: GroupRef,
  user: string | bigint | number,
): RemoveGroupMemberOp {
  const resolved = typeof user === 'string' ? principalId(user) : user;
  const canonical = canonicalRemoveGroupMember(ref, resolved);
  return { remove_group_member: ref, user: resolved, hmac: signer.hmacTagHex(canonical) };
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
 * Create a user (HMAC-gated). `roles` is `#[serde(default)]` WITHOUT skip
 * → always present on the wire. Emits `roles: []` when none provided.
 * `password` is a SecretString on the Rust side → plain string on wire.
 * canonical = `canonicalCreateUser(username)` — the password is NEVER
 * part of the canonical input.
 */
export function createUser(
  signer: HmacSigner,
  name: string,
  password: string,
  opts?: { roles?: string[]; profile?: WireValue; database?: string },
): CreateUserOp {
  const canonical = canonicalCreateUser(name);
  const op: CreateUserOp = {
    create_user: name,
    password,
    roles: opts?.roles ?? [],
    hmac: signer.hmacTagHex(canonical),
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

/**
 * Create a role (HMAC-gated). canonical = `canonicalCreateRole(name)` —
 * the permissions list is not part of the canonical input, mirroring
 * `dropRole`'s precedent of identifying by name only.
 */
export function createRole(
  signer: HmacSigner,
  name: string,
  permissions: Permission[],
): CreateRoleOp {
  const canonical = canonicalCreateRole(name);
  return { create_role: name, permissions, hmac: signer.hmacTagHex(canonical) };
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

/**
 * Grant a role to a user (HMAC-gated) — the single most dangerous op in
 * the system (e.g. granting `superuser` to an attacker-controlled account).
 * canonical = `canonicalGrantRole(role, user)`.
 */
export function grantRole(signer: HmacSigner, role: string, user: string): GrantRoleOp {
  const canonical = canonicalGrantRole(role, user);
  return { grant_role: role, user, hmac: signer.hmacTagHex(canonical) };
}

/** Revoke a role from a user (HMAC-gated). canonical = `canonicalRevokeRole(role, user)`. */
export function revokeRole(signer: HmacSigner, role: string, user: string): RevokeRoleOp {
  const canonical = canonicalRevokeRole(role, user);
  return { revoke_role: role, user, hmac: signer.hmacTagHex(canonical) };
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
