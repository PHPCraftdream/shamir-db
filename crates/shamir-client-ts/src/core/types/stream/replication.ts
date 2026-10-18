/**
 * Replication DDL operation wire types — type-only mirror of
 * `crates/shamir-query-types/src/admin/types/repl_ops.rs`.
 *
 * Pure type declarations; the constructor/builder code that assembles these
 * shapes lives in `../../builders/replication.ts`.
 *
 * Serde notes encoded here (so the builder emits the exact wire shape):
 *   - `ReplScope.repo` / `ReplScope.table` are
 *     `#[serde(default, skip_serializing_if = "Option::is_none")]` — OPTIONAL
 *     (`?`) here; the builder OMITS them when unset (no `null`/`undefined`).
 *   - `ReplStream.direction` / `ReplStream.mode` have ONLY `#[serde(default)]`
 *     (NO skip) — they are ALWAYS present on the wire (default `"pull"` /
 *     `"read_only"`).
 *   - `SubAction` is an externally-tagged enum with `rename_all =
 *     "snake_case"`: unit variants `Pause`/`Resume` serialize as bare strings
 *     `"pause"`/`"resume"`; the newtype `SetProfile(String)` serializes as
 *     `{ "set_profile": <name> }`.
 *   - The three read-only introspection ops are presence-only boolean flags
 *     (`#[serde(default, skip_serializing_if = "is_false")]`) — the builder
 *     emits `{ <discriminator>: true }`.
 *
 * PLATFORM-AGNOSTIC.
 */

// ── Sub-DTOs ────────────────────────────────────────────────────────

/**
 * Replication scope — the `(db[, repo[, table]])` triple identifying *what*
 * is replicated. `repo`/`table` are omitted from the wire when unset
 * (`skip_serializing_if = "Option::is_none"`): `{ db: "app" }` means the
 * whole database; `{ db: "app", repo: "main" }` means the whole repo.
 */
export interface ReplScope {
  db: string;
  /** Repository inside `db`. Omitted from wire when unset → whole database. */
  repo?: string;
  /** Table inside `repo`. Omitted from wire when unset → whole repository. */
  table?: string;
}

/**
 * Replication direction, relative to the node owning the profile stream.
 * `#[serde(rename_all = "snake_case")]`. The serde default is `"pull"`.
 */
export type ReplDirection = 'pull' | 'push' | 'both';

/**
 * Replication access mode for a stream.
 * `#[serde(rename_all = "snake_case")]`. The serde default is `"read_only"`.
 */
export type ReplMode = 'read_only' | 'read_write';

/**
 * One `(scope, direction, mode)` rule inside a `CreateReplicationProfileOp`.
 * `direction` / `mode` have `#[serde(default)]` with NO skip — they are
 * ALWAYS present on the wire.
 */
export interface ReplStream {
  scope: ReplScope;
  direction: ReplDirection;
  mode: ReplMode;
}

/**
 * Action taken by `AlterSubscriptionOp` on an existing subscription.
 * Externally-tagged enum, `rename_all = "snake_case"`:
 *   - `Pause`        → bare string `"pause"`
 *   - `Resume`       → bare string `"resume"`
 *   - `SetProfile(s)` → `{ "set_profile": s }`
 */
export type SubAction = 'pause' | 'resume' | { set_profile: string };

// ── Profile ops ─────────────────────────────────────────────────────

/** `{ create_replication_profile: name, streams: ReplStream[] }` */
export interface CreateReplicationProfileOp {
  create_replication_profile: string;
  streams: ReplStream[];
}

/** `{ drop_replication_profile: name }` */
export interface DropReplicationProfileOp {
  drop_replication_profile: string;
}

// ── Publication ops ─────────────────────────────────────────────────

/** `{ create_publication: name, scopes: ReplScope[] }` */
export interface CreatePublicationOp {
  create_publication: string;
  scopes: ReplScope[];
}

/** `{ drop_publication: name }` */
export interface DropPublicationOp {
  drop_publication: string;
}

// ── Subscription ops ────────────────────────────────────────────────

/** `{ create_subscription, upstream, publication, profile }` */
export interface CreateSubscriptionOp {
  create_subscription: string;
  upstream: string;
  publication: string;
  profile: string;
}

/** `{ drop_subscription: name }` */
export interface DropSubscriptionOp {
  drop_subscription: string;
}

/** `{ alter_subscription: name, action: SubAction }` */
export interface AlterSubscriptionOp {
  alter_subscription: string;
  action: SubAction;
}

// ── Read-only introspection ops (presence-only boolean flags) ───────

/** `{ list_publications: true }` — presence-only boolean discriminator. */
export interface ListPublicationsOp {
  list_publications: true;
}

/** `{ list_subscriptions: true }` — presence-only boolean discriminator. */
export interface ListSubscriptionsOp {
  list_subscriptions: true;
}

/** `{ replication_status: true }` — presence-only boolean discriminator. */
export interface ReplicationStatusOp {
  replication_status: true;
}

// ── Union ───────────────────────────────────────────────────────────

/** Union of all replication DDL admin operations. */
export type ReplicationOp =
  | CreateReplicationProfileOp
  | DropReplicationProfileOp
  | CreatePublicationOp
  | DropPublicationOp
  | CreateSubscriptionOp
  | DropSubscriptionOp
  | AlterSubscriptionOp
  | ListPublicationsOp
  | ListSubscriptionsOp
  | ReplicationStatusOp;
