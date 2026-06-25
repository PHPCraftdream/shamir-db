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

// ── ACL ops (access.rs) — all NON-HMAC ─────────────────────────────

export interface ChmodOp {
  chmod: ResourceRef;
  mode: number;
}

export interface ChownOp {
  chown: ResourceRef;
  owner: number | bigint;
}

/**
 * `group` is `Option<u64>` — ALWAYS present on the wire.
 * `null` clears the group.
 */
export interface ChgrpOp {
  chgrp: ResourceRef;
  group: number | null;
}

export interface CreateGroupOp {
  create_group: string;
}

export interface DropGroupOp {
  drop_group: GroupRef;
  if_exists?: boolean;
}

export interface AddGroupMemberOp {
  add_group_member: GroupRef;
  user: number | bigint;
}

export interface RemoveGroupMemberOp {
  remove_group_member: GroupRef;
  user: number | bigint;
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
 * Create a user (auth/types.rs `CreateUserOp`).
 *
 * - `password` is a `SecretString` on the Rust side → plain string on wire.
 * - `roles` is `#[serde(default)]` WITHOUT skip → **always present** on the
 *   wire (emit `roles: []` when none).
 * - `profile` / `database` are `skip_serializing_if = "Option::is_none"`.
 */
export interface CreateUserOp {
  create_user: string;
  password: string;
  roles: string[];
  profile?: WireValue;
  database?: string;
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

export interface CreateRoleOp {
  create_role: string;
  permissions: Permission[];
}

/**
 * Drop a role (HMAC-gated).
 * `hmac` is `Option<String>` with skip → present when signed.
 */
export interface DropRoleOp {
  drop_role: string;
  hmac: string;
  if_exists?: boolean;
}

export interface GrantRoleOp {
  grant_role: string;
  user: string;
}

export interface RevokeRoleOp {
  revoke_role: string;
  user: string;
}

// ── Union ───────────────────────────────────────────────────────────

/** Union of all ACL + RBAC admin operations. */
export type AdminOp =
  | ChmodOp
  | ChownOp
  | ChgrpOp
  | CreateGroupOp
  | DropGroupOp
  | AddGroupMemberOp
  | RemoveGroupMemberOp
  | AccessTreeOp
  | CreateUserOp
  | DropUserOp
  | CreateRoleOp
  | DropRoleOp
  | GrantRoleOp
  | RevokeRoleOp;
