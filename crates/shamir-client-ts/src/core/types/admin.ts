/**
 * Access-control (ACL) + RBAC admin wire types — type-only mirror of
 * `crates/shamir-query-types/src/admin/access.rs` and
 * `crates/shamir-query-types/src/auth/types.rs`.
 *
 * Pure type declarations; the constructor/builder code that assembles these
 * shapes lives in `../../builders/admin.ts`.
 *
 * CRITICAL: Two distinct "resource" notions live here — do NOT conflate them.
 *
 *   1. **ResourceRef** (access.rs, `#[serde(untagged)]`) — single-key object
 *      used by chmod/chown/chgrp: `{database}`, `{store:[…]}`,
 *      `{table:[…]}`, `{function}`, `{function_folder}`,
 *      `{function_namespace}`.
 *
 *   2. **Resource** (auth/types.rs, `#[serde(tag="scope")]`) — tagged enum
 *      used inside Permission: `{scope:"global"}`, `{scope:"database",…}`,
 *      `{scope:"repo",…}`, `{scope:"table",…}`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { WireValue } from './write.js';
import type { Filter } from './filter.js';
import type { HmacSigner } from './ddl.js';

export type { HmacSigner } from './ddl.js';

// ── ResourceRef (access.rs, untagged, single-key) ───────────────────

/**
 * Wire-friendly securable resource reference (access.rs `ResourceRef`).
 * `#[serde(untagged)]` — each variant is a single-key object so the
 * discriminator is unambiguous.
 */
export type ResourceRef =
  | { database: string }
  | { store: [string, string] }
  | { table: [string, string, string] }
  | { function: string }
  | { function_folder: string[] }
  | { function_namespace: true };

// ── GroupRef (access.rs, untagged) ───────────────────────────────────

/**
 * Reference to a group — by name or numeric id.
 * `#[serde(untagged)]`: `{name}` | `{id}`.
 */
export type GroupRef = { name: string } | { id: number };

// ── Resource (auth/types.rs, tag="scope") ────────────────────────────

/**
 * Permission resource scope (auth/types.rs `Resource`).
 * `#[serde(tag = "scope", rename_all = "lowercase")]`.
 */
export type Resource =
  | { scope: 'global' }
  | { scope: 'database'; database: string }
  | { scope: 'repo'; database: string; repo: string }
  | { scope: 'table'; database: string; repo: string; table: string };

// ── Action & Effect ─────────────────────────────────────────────────

/**
 * Action type (auth/types.rs `Action`).
 * `#[serde(rename_all = "snake_case")]`.
 */
export type Action =
  | 'read'
  | 'insert'
  | 'update'
  | 'delete'
  | 'create'
  | 'drop'
  | 'alter'
  | 'manage_users'
  | 'manage_roles'
  | 'all';

/**
 * Permission effect (auth/types.rs `Effect`).
 * `#[serde(rename_all = "lowercase")]`.
 */
export type Effect = 'allow' | 'deny';

// ── Permission ──────────────────────────────────────────────────────

/**
 * Single permission entry (auth/types.rs `Permission`).
 * `row_filter` is `#[serde(rename = "where", skip_serializing_if = "Option::is_none")]`
 * → omitted when absent; wire key is `"where"`.
 */
export interface Permission {
  effect: Effect;
  actions: Action[];
  resource: Resource;
  where?: Filter;
}

// ── ACL ops (access.rs) — chmod/chown/chgrp are HMAC-gated ─────────

/**
 * Change mode bits on a resource (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface ChmodOp {
  chmod: ResourceRef;
  mode: number;
  hmac?: string;
}

/**
 * Change owner on a resource (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface ChownOp {
  chown: ResourceRef;
  owner: number | bigint;
  hmac?: string;
}

/**
 * Change group on a resource (HMAC-gated). `group` is `Option<u64>` —
 * ALWAYS present on the wire. `null` clears the group.
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface ChgrpOp {
  chgrp: ResourceRef;
  group: number | null;
  hmac?: string;
}

/**
 * Create a new group (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface CreateGroupOp {
  create_group: string;
  hmac?: string;
}

/**
 * Drop an existing group by name or id (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface DropGroupOp {
  drop_group: GroupRef;
  if_exists?: boolean;
  hmac?: string;
}

/**
 * Rename an existing group (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface RenameGroupOp {
  rename_group: GroupRef;
  to: string;
  hmac?: string;
}

/**
 * Add a user to a group (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface AddGroupMemberOp {
  add_group_member: GroupRef;
  user: number | bigint;
  hmac?: string;
}

/**
 * Remove a user from a group (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface RemoveGroupMemberOp {
  remove_group_member: GroupRef;
  user: number | bigint;
  hmac?: string;
}

/**
 * Access-tree introspection (access.rs `AccessTreeOp`).
 * `depth` and `db` are `skip_serializing_if = "Option::is_none"`.
 */
export interface AccessTreeOp {
  access_tree: true;
  depth?: number;
  db?: string;
}

// ── RBAC ops (auth/types.rs) ────────────────────────────────────────

/**
 * Create a user (auth/types.rs `CreateUserOp`). HMAC-gated.
 *
 * - `password` is a `SecretString` on the Rust side → plain string on wire.
 * - `roles` is `#[serde(default)]` WITHOUT skip → **always present** on the
 *   wire (emit `roles: []` when none).
 * - `profile` / `database` / `hmac` are `skip_serializing_if = "Option::is_none"`.
 * - `hmac` canonical is username-only — the password is NEVER part of it.
 */
export interface CreateUserOp {
  create_user: string;
  password: string;
  roles: string[];
  profile?: WireValue;
  database?: string;
  hmac?: string;
}

/**
 * Drop a user (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface DropUserOp {
  drop_user: string;
  hmac: string;
  if_exists?: boolean;
}

/**
 * Grant a role to a user (HMAC-gated) — the single most dangerous op in
 * the system (e.g. granting `superuser` to an attacker-controlled account).
 */
export interface GrantRoleOp {
  grant_role: string;
  user: string;
  hmac?: string;
}

/** Revoke a role from a user (HMAC-gated). */
export interface RevokeRoleOp {
  revoke_role: string;
  user: string;
  hmac?: string;
}

// ── Top-level DbRequest ops (NOT BatchOps) ──────────────────────────
//
// These mirror `DbRequest` variants that are dispatched directly by the
// server's connection layer rather than through the batch engine. On the
// TS side they are built by the admin builder (`setSuperuser`) and sent
// via `ShamirClient.sendDbRequest`, the same path `createScramUser` uses.
// The serde discriminator key is `"op"` (`#[serde(tag = "op")]`).

/**
 * Grant or revoke superuser status on an existing SCRAM-directory account
 * (top-level `DbRequest::SetSuperuser`). Requires an already-superuser
 * session AND an HMAC confirmation tag. The tag is UNCONDITIONAL — always
 * present, unlike the conditional gate on `CreateFunctionOp`.
 *
 * `hmac` is `Option<String>` on the Rust side; the client builder always
 * supplies the signed tag, so it is typed `string` here.
 */
export interface SetSuperuserOp {
  op: 'set_superuser';
  user: string;
  on: boolean;
  hmac: string;
}

// ── Union ───────────────────────────────────────────────────────────

/** Union of all ACL + RBAC admin operations. */
export type AdminOp =
  | ChmodOp
  | ChownOp
  | ChgrpOp
  | CreateGroupOp
  | DropGroupOp
  | RenameGroupOp
  | AddGroupMemberOp
  | RemoveGroupMemberOp
  | AccessTreeOp
  | CreateUserOp
  | DropUserOp
  | GrantRoleOp
  | RevokeRoleOp;
