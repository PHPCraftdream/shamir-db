/**
 * DDL misc builders — retention/history, buffer config, migrations status,
 * interner and list operations. Split out of the former monolithic
 * `builders/ddl.ts`.
 */

import type {
  HmacSigner,
  Retention,
  PurgeScope,
  BufferConfigDto,
  BufferConfigPatch,
  SetBufferConfigOp,
  GetBufferConfigOp,
  AlterBufferConfigOp,
  MigrationStatusOp,
  SetRetentionOp,
  PurgeHistoryOp,
  ChangesSinceOp,
  InternerDumpOp,
  InternerTouchOp,
  ListOp,
} from '../../types/schema/ddl.js';
import { repoOrDefault } from './ddl-schema.js';
import { canonicalBatchOp } from '../../protocol/hmac.js';


/** Retention helper: CurrentOnly — no history retained. */
export function currentOnly(): Retention {
  return { max_count: 0 };
}

// ── Purge-scope constructors ────────────────────────────────────────

/** Purge history older than an epoch-millis timestamp. */
export function olderThan(timestamp: number): PurgeScope {
  return { older_than: { timestamp } };
}

/** Purge history older than this age (seconds). */
export function olderThanAge(age_secs: number): PurgeScope {
  return { older_than_age: { age_secs } };
}

/** Persist a full buffer config for a table. */
export function setBufferConfig(
  table: string,
  config: BufferConfigDto,
  opts?: { repo?: string },
): SetBufferConfigOp {
  return {
    set_buffer_config: table,
    repo: repoOrDefault(opts?.repo),
    config,
  };
}

/** Read the persisted buffer config for a table. */
export function getBufferConfig(
  table: string,
  opts?: { repo?: string },
): GetBufferConfigOp {
  return {
    get_buffer_config: table,
    repo: repoOrDefault(opts?.repo),
  };
}

/** Partial-update buffer config knobs. */
export function alterBufferConfig(
  table: string,
  patch: BufferConfigPatch,
  opts?: { repo?: string },
): AlterBufferConfigOp {
  return {
    alter_buffer_config: table,
    repo: repoOrDefault(opts?.repo),
    patch,
  };
}

/** Query migration status by ID, or list all active migrations. */
export function migrationStatus(
  idOrEmpty: string,
): MigrationStatusOp {
  return { migration_status: idOrEmpty };
}

/**
 * Change a live table's history-retention policy (HMAC-gated).
 * canonical v2 encodes the complete operation DTO.
 */
export function setRetention(
  signer: HmacSigner,
  dbInUse: string,
  table: string,
  retention: Retention,
  opts?: { repo?: string },
): SetRetentionOp {
  const repo = repoOrDefault(opts?.repo);
  const canonical = canonicalBatchOp(dbInUse, { set_retention: table, repo, retention });
  return {
    set_retention: table,
    repo,
    retention,
    hmac: signer.hmacTagHex(canonical),
  };
}

/**
 * Imperative history purge for a table (HMAC-gated) — irreversible
 * audit-trail loss. Canonical v2 encodes the complete operation DTO.
 */
export function purgeHistory(
  signer: HmacSigner,
  dbInUse: string,
  table: string,
  scope: PurgeScope,
  opts?: { repo?: string },
): PurgeHistoryOp {
  const repo = repoOrDefault(opts?.repo);
  const canonical = canonicalBatchOp(dbInUse, { purge_history: table, repo, scope });
  return {
    purge_history: table,
    repo,
    scope,
    hmac: signer.hmacTagHex(canonical),
  };
}

/** One-shot "changes since version V" read. */
export function changesSince(
  cursor: number | bigint,
  opts?: { repo?: string; limit?: number },
): ChangesSinceOp {
  const op: ChangesSinceOp = {
    changes_since: cursor,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.limit !== undefined) op.limit = opts.limit;
  return op;
}

/**
 * Dump a repo's interner dictionary (id → name). `interner_dump` defaults
 * to "main" and is always present on the wire; `since` is omitted unless
 * set (delta-refresh cursor — only entries with id > `since`).
 */
export function internerDump(
  opts?: { repo?: string; since?: number },
): InternerDumpOp {
  const op: InternerDumpOp = {
    interner_dump: repoOrDefault(opts?.repo),
  };
  if (opts?.since != null) op.since = opts.since;
  return op;
}

/**
 * Register field NAMES in a repo's interner (idempotent — returns the
 * name → id mapping). `interner_touch` defaults to "main".
 */
export function internerTouch(
  names: string[],
  opts?: { repo?: string },
): InternerTouchOp {
  return {
    interner_touch: repoOrDefault(opts?.repo),
    names,
  };
}

// ── List ops ────────────────────────────────────────────────────────

export function listDatabases(): ListOp {
  return { list: 'databases' };
}

export function listRepos(): ListOp {
  return { list: 'repos' };
}

export function listTables(opts?: { repo?: string }): ListOp {
  return { list: 'tables', repo: repoOrDefault(opts?.repo) };
}

export function listIndexes(
  table: string,
  opts?: { repo?: string },
): ListOp {
  return { list: 'indexes', table, repo: repoOrDefault(opts?.repo) };
}

export function listUsers(): ListOp {
  return { list: 'users' };
}
