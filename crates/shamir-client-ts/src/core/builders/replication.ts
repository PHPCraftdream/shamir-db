/**
 * Replication DDL operation builders — the CODE that constructs the wire
 * shapes declared in `../types/replication.ts`. Mirrors
 * `crates/shamir-query-types/src/admin/types/repl_ops.rs` and the fluent
 * Rust builders in `crates/shamir-query-builder/src/ddl/replication.rs`.
 *
 * All ops are plain functions returning the wire object (no HMAC gating on
 * the replication surface). Mirrors the `ddl.ts` / `admin.ts` pattern: flat
 * named function exports + an aggregate `replication` namespace object.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  ReplScope,
  ReplDirection,
  ReplMode,
  ReplStream,
  SubAction,
  CreateReplicationProfileOp,
  DropReplicationProfileOp,
  CreatePublicationOp,
  DropPublicationOp,
  CreateSubscriptionOp,
  DropSubscriptionOp,
  AlterSubscriptionOp,
  ListPublicationsOp,
  ListSubscriptionsOp,
  ReplicationStatusOp,
} from '../types/replication.js';

// ── Helpers ─────────────────────────────────────────────────────────

/**
 * Build a `ReplScope` for the given database, optionally narrowed to a
 * repository and/or table. `repo` / `table` are OMITTED from the wire object
 * when unset (mirrors `skip_serializing_if = "Option::is_none"` — the builder
 * never emits `null`/`undefined`, it simply omits the key).
 *
 *   replScope('app')                       → { db: 'app' }
 *   replScope('app', { repo: 'main' })     → { db: 'app', repo: 'main' }
 *   replScope('app', { repo: 'main', table: 'users' })
 *                                          → { db: 'app', repo: 'main', table: 'users' }
 */
export function replScope(
  db: string,
  opts?: { repo?: string; table?: string },
): ReplScope {
  const scope: ReplScope = { db };
  if (opts?.repo !== undefined) scope.repo = opts.repo;
  if (opts?.table !== undefined) scope.table = opts.table;
  return scope;
}

/**
 * Build a `ReplStream` — the atomic `(scope, direction, mode)` replication
 * policy rule. `direction` / `mode` are ALWAYS present on the wire
 * (`#[serde(default)]` with NO skip — defaults `"pull"` / `"read_only"`).
 */
export function replStream(
  scope: ReplScope,
  direction: ReplDirection = 'pull',
  mode: ReplMode = 'read_only',
): ReplStream {
  return { scope, direction, mode };
}

// ── Profile ops ─────────────────────────────────────────────────────

/**
 * Create a named replication-profile template bundling a set of stream rules.
 * Wire shape: `{ create_replication_profile: name, streams: [...] }`.
 */
export function replicationProfile(
  name: string,
  streams: ReplStream[],
): CreateReplicationProfileOp {
  return {
    create_replication_profile: name,
    streams,
  };
}

/** Drop a named replication profile. Wire shape: `{ drop_replication_profile: name }`. */
export function dropReplicationProfile(name: string): DropReplicationProfileOp {
  return { drop_replication_profile: name };
}

// ── Publication ops ─────────────────────────────────────────────────

/**
 * Declare a publication — a set of `ReplScope`s that downstream subscribers
 * may pull. Wire shape: `{ create_publication: name, scopes: [...] }`.
 */
export function publication(
  name: string,
  scopes: ReplScope[],
): CreatePublicationOp {
  return {
    create_publication: name,
    scopes,
  };
}

/** Drop a publication by name. Wire shape: `{ drop_publication: name }`. */
export function dropPublication(name: string): DropPublicationOp {
  return { drop_publication: name };
}

// ── Subscription ops ────────────────────────────────────────────────

/**
 * Subscribe this node to a remote publication, bound to a local replication
 * profile. All four fields are required.
 * Wire shape: `{ create_subscription, upstream, publication, profile }`.
 */
export function subscription(
  name: string,
  opts: { upstream: string; publication: string; profile: string },
): CreateSubscriptionOp {
  return {
    create_subscription: name,
    upstream: opts.upstream,
    publication: opts.publication,
    profile: opts.profile,
  };
}

/** Drop a subscription by name. Wire shape: `{ drop_subscription: name }`. */
export function dropSubscription(name: string): DropSubscriptionOp {
  return { drop_subscription: name };
}

/**
 * Alter an existing subscription: pause, resume, or rebind to a different
 * profile. Wire shape: `{ alter_subscription: name, action: SubAction }`.
 *
 * `action` mirrors the externally-tagged `SubAction` enum:
 *   - `'pause'`                                  → `"pause"`
 *   - `'resume'`                                 → `"resume"`
 *   - `{ set_profile: 'cluster2' }`              → `{ "set_profile": "cluster2" }`
 */
export function alterSubscription(
  name: string,
  action: SubAction,
): AlterSubscriptionOp {
  return {
    alter_subscription: name,
    action,
  };
}

// ── Read-only introspection ops (presence-only boolean flags) ───────

/** List all publications defined on this node. Wire shape: `{ list_publications: true }`. */
export function listPublications(): ListPublicationsOp {
  return { list_publications: true };
}

/** List all subscriptions defined on this node. Wire shape: `{ list_subscriptions: true }`. */
export function listSubscriptions(): ListSubscriptionsOp {
  return { list_subscriptions: true };
}

/** Inspect the runtime replication status. Wire shape: `{ replication_status: true }`. */
export function replicationStatus(): ReplicationStatusOp {
  return { replication_status: true };
}

// ── Aggregate namespace ─────────────────────────────────────────────

/** Aggregate namespace — every replication constructor in one object. */
export const replication = {
  replScope,
  replStream,
  replicationProfile,
  dropReplicationProfile,
  publication,
  dropPublication,
  subscription,
  dropSubscription,
  alterSubscription,
  listPublications,
  listSubscriptions,
  replicationStatus,
};
