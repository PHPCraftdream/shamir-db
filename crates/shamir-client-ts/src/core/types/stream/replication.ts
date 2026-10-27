/**
 * Replication DDL operation wire types — type-only mirror of
 * `crates/shamir-query-types/src/admin/types/repl_ops.rs`.
 *
 * Optional scope fields are omitted when unset. Stream direction/mode are
 * always present; SubAction uses externally tagged serde representations.
 */
export interface ReplScope {
  db: string;
  repo?: string;
  table?: string;
}
export type ReplDirection = 'pull' | 'push' | 'both';
export type ReplMode = 'read_only' | 'read_write';
export interface ReplStream {
  scope: ReplScope;
  direction: ReplDirection;
  mode: ReplMode;
}
export type SubAction = 'pause' | 'resume' | { set_profile: string };

export interface CreateReplicationProfileOp {
  create_replication_profile: string;
  streams: ReplStream[];
}
export interface DropReplicationProfileOp {
  drop_replication_profile: string;
  hmac: string;
}
export interface CreatePublicationOp {
  create_publication: string;
  scopes: ReplScope[];
}
export interface DropPublicationOp {
  drop_publication: string;
  hmac: string;
}
export interface CreateSubscriptionOp {
  create_subscription: string;
  upstream: string;
  publication: string;
  profile: string;
}
export interface DropSubscriptionOp {
  drop_subscription: string;
  hmac: string;
}
export interface AlterSubscriptionOp {
  alter_subscription: string;
  action: SubAction;
}
export interface ListPublicationsOp { list_publications: true }
export interface ListSubscriptionsOp { list_subscriptions: true }
export interface ReplicationStatusOp { replication_status: true }

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
