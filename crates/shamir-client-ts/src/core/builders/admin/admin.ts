/**
 * Access-control (ACL) + RBAC admin operation builders — the CODE that
 * constructs the wire shapes declared in `../types/admin.ts`. Mirrors
 * `crates/shamir-query-types/src/admin/access.rs` and
 * `crates/shamir-query-types/src/auth/types.rs`.
 *
 * Non-HMAC ops are plain functions returning the wire object.
 * HMAC-gated ops (`drop_user`, `chmod`, `chown`, `chgrp`,
 * HMAC-gated operation builders take a `signer: HmacSigner`, encode the full
 * v2 operation DTO, and attach the resulting tag.
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
  GrantRoleOp,
  RevokeRoleOp,
  SetSuperuserOp,
  SetReplicatorOp,
} from '../../types/access/admin.js';

import type { Filter } from '../../types/read/filter.js';
import type { WireValue } from '../../types/write/write.js';

import { canonicalBatchOp, canonicalDbRequest } from '../../protocol/hmac.js';

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

/** Change mode bits on a resource (HMAC-gated). canonical = the complete op DTO passed to `canonicalBatchOp`. */
export function chmod(signer: HmacSigner, resource: ResourceRef, mode: number): ChmodOp {
  const canonical = canonicalBatchOp('', { chmod: resource, mode });
  return { chmod: resource, mode, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Transfer ownership of a resource (HMAC-gated).
 * canonical = the complete v2 chown DTO.
 *
 * `owner` accepts:
 *   - `bigint` — pre-computed principal id.
 *   - `number` — raw numeric id (only safe for values <= 2^53).
 *
 * The server's `principal64` is a real, server-assigned random id (task
 * #548), NOT reproducible client-side from a username. Callers must resolve
 * a username to its real principal64 first — see
 * `ShamirClient.resolvePrincipal(username)`.
 */
export function chown(
  signer: HmacSigner,
  resource: ResourceRef,
  owner: bigint | number,
): ChownOp {
  const canonical = canonicalBatchOp('', { chown: resource, owner });
  return { chown: resource, owner, hmac: signer.hmacTagHex(canonical) };
}

/** Change group on a resource (HMAC-gated); canonical v2 encodes its complete DTO. */
export function chgrp(signer: HmacSigner, resource: ResourceRef, group: number | null): ChgrpOp {
  const canonical = canonicalBatchOp('', { chgrp: resource, group });
  return { chgrp: resource, group, hmac: signer.hmacTagHex(canonical) };
}

/** Create a new group (HMAC-gated), signed over the complete v2 DTO. */
export function createGroup(signer: HmacSigner, name: string): CreateGroupOp {
  const canonical = canonicalBatchOp('', { create_group: name });
  return { create_group: name, hmac: signer.hmacTagHex(canonical) };
}

/** Drop a group by reference (HMAC-gated), signed over the complete v2 DTO. */
export function dropGroup(
  signer: HmacSigner,
  ref: GroupRef,
  opts?: { if_exists?: boolean },
): DropGroupOp {
  const canonical = canonicalBatchOp('', { drop_group: ref, if_exists: opts?.if_exists ?? false });
  const op: DropGroupOp = { drop_group: ref, hmac: signer.hmacTagHex(canonical) };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/**
 * Rename a group (HMAC-gated). Groups are id-keyed, so this only updates
 * the display name; members and resource references (which store the
 * group id) are unaffected. The complete operation DTO is encoded by `canonicalBatchOp`.
 */
export function renameGroup(signer: HmacSigner, ref: GroupRef, to: string): RenameGroupOp {
  const canonical = canonicalBatchOp('', { rename_group: ref, to });
  return { rename_group: ref, to, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Add a user to a group (HMAC-gated).
 * canonical v2 encodes the full add-group-member DTO.
 *
 * `user` accepts:
 *   - `bigint` — pre-computed principal id.
 *   - `number` — raw numeric id (only safe for values <= 2^53).
 *
 * The server's `principal64` is a real, server-assigned random id (task
 * #548), NOT reproducible client-side from a username. Callers must resolve
 * a username to its real principal64 first — see
 * `ShamirClient.resolvePrincipal(username)`.
 */
export function addGroupMember(
  signer: HmacSigner,
  ref: GroupRef,
  user: bigint | number,
): AddGroupMemberOp {
  const canonical = canonicalBatchOp('', { add_group_member: ref, user });
  return { add_group_member: ref, user, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Remove a user from a group (HMAC-gated).
 * canonical v2 encodes the full remove-group-member DTO.
 *
 * `user` accepts:
 *   - `bigint` — pre-computed principal id.
 *   - `number` — raw numeric id (only safe for values <= 2^53).
 *
 * The server's `principal64` is a real, server-assigned random id (task
 * #548), NOT reproducible client-side from a username. Callers must resolve
 * a username to its real principal64 first — see
 * `ShamirClient.resolvePrincipal(username)`.
 */
export function removeGroupMember(
  signer: HmacSigner,
  ref: GroupRef,
  user: bigint | number,
): RemoveGroupMemberOp {
  const canonical = canonicalBatchOp('', { remove_group_member: ref, user });
  return { remove_group_member: ref, user, hmac: signer.hmacTagHex(canonical) };
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
 * canonical v2 input covers all supplied fields except `hmac` and password.
 */
export function createUser(
  signer: HmacSigner,
  name: string,
  password: string,
  opts?: { roles?: string[]; profile?: WireValue; database?: string },
): CreateUserOp {
  const canonical = canonicalBatchOp('', { create_user: name, password, roles: opts?.roles ?? [], database: opts?.database });
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

/** Drop a user (HMAC-gated), signed over the full v2 operation DTO. */
export function dropUser(
  signer: HmacSigner,
  username: string,
  opts?: { if_exists?: boolean },
): DropUserOp {
  const canonical = canonicalBatchOp('', { drop_user: username, if_exists: opts?.if_exists ?? false });
  const op: DropUserOp = {
    drop_user: username,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/**
 * Grant or revoke superuser status on an existing SCRAM-directory account
 * (top-level `DbRequest::SetSuperuser`, NOT a `BatchOp`). Requires an
 * already-superuser session. The HMAC tag is UNCONDITIONAL — every call
 * signs it using the complete v2 operation DTO.
 *
 * This is the first standalone (non-batch) admin op modelled as a builder
 * in this module: the other top-level `DbRequest` variant the TS client
 * handles (`create_scram_user`) lives only as a `ShamirClient` method.
 * `setSuperuser` follows the SAME builder pattern as the HMAC-gated
 * `BatchOp` builders here (signer + canonical + `.hmac` field), but
 * emits the top-level wire shape `{ op: "set_superuser", ... }` rather
 * than a single-key `BatchOp` object. `ShamirClient.setSuperuser` sends
 * it via `sendDbRequest`.
 */
export function setSuperuser(
  signer: HmacSigner,
  user: string,
  on: boolean,
): SetSuperuserOp {
  const canonical = canonicalDbRequest({ op: 'set_superuser', user, on });
  return { op: 'set_superuser', user, on, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Grant or revoke replicator status on an existing SCRAM-directory account
 * (top-level `DbRequest::SetReplicator`, task #621, NOT a `BatchOp`).
 * Requires an already-superuser session. The HMAC tag is UNCONDITIONAL —
 * every call signs it using the complete v2 operation DTO.
 *
 * Mirrors `setSuperuser`'s builder shape exactly (signer + canonical +
 * `.hmac` field), emitting the top-level wire shape
 * `{ op: "set_replicator", ... }`. `ShamirClient.setReplicator` sends it
 * via `sendDbRequest`.
 */
export function setReplicator(
  signer: HmacSigner,
  user: string,
  on: boolean,
): SetReplicatorOp {
  const canonical = canonicalDbRequest({ op: 'set_replicator', user, on });
  return { op: 'set_replicator', user, on, hmac: signer.hmacTagHex(canonical) };
}

/**
 * Grant a role to a user (HMAC-gated) — the single most dangerous op in
 * the system (e.g. granting `superuser` to an attacker-controlled account).
 * canonical v2 input encodes the full operation DTO.
 *
 * "Role" is now a plain string label attached to a directory user (task
 * #549); there is no "role object" to create/drop/rename. `grantRole` /
 * `revokeRole` are the only role-mutating ops.
 */
export function grantRole(signer: HmacSigner, role: string, user: string): GrantRoleOp {
  const canonical = canonicalBatchOp('', { grant_role: role, user });
  return { grant_role: role, user, hmac: signer.hmacTagHex(canonical) };
}

/** Revoke a role from a user (HMAC-gated); canonical v2 encodes the full DTO. */
export function revokeRole(signer: HmacSigner, role: string, user: string): RevokeRoleOp {
  const canonical = canonicalBatchOp('', { revoke_role: role, user });
  return { revoke_role: role, user, hmac: signer.hmacTagHex(canonical) };
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
  setSuperuser,
  setReplicator,
  grantRole,
  revokeRole,
};
