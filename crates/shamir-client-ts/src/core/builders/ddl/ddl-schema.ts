/**
 * DDL schema & lifecycle builders — database/table/repo create, drop and
 * rename, table schema rules, introspection and online migrations. Split
 * out of the former monolithic `builders/ddl.ts`.
 */

import type {
  HmacSigner,
  Retention,
  FieldRuleDto,
  SetTableSchemaOp,
  AddSchemaRuleOp,
  RemoveSchemaRuleOp,
  GetTableSchemaOp,
  DescribeTableOp,
  CreateDbOp,
  CreateTableOp,
  DropDbOp,
  DropRepoOp,
  DropTableOp,
  RenameTableOp,
  RenameRepoOp,
  RenameDbOp,
  StartMigrationOp,
  CommitMigrationOp,
  RollbackMigrationOp,
} from '../../types/schema/ddl.js';
import { assertSafeVersion } from '../write/write.js';
import {
  canonicalDropDb,
  canonicalDropRepo,
  canonicalDropTable,
  canonicalStartMigration,
  canonicalCommitMigration,
  canonicalRollbackMigration,
} from '../../protocol/hmac.js';


// ── Helpers ─────────────────────────────────────────────────────────

const DEFAULT_REPO = 'main';

export function repoOrDefault(repo?: string): string {
  return repo ?? DEFAULT_REPO;
}

// ── Non-HMAC ops ────────────────────────────────────────────────────

/** Create a new database. */
export function createDb(
  name: string,
  opts?: { if_not_exists?: boolean },
): CreateDbOp {
  const op: CreateDbOp = { create_db: name };
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/** Create a table in a repository. */
export function createTable(
  name: string,
  opts?: {
    repo?: string;
    if_not_exists?: boolean;
    retention?: Retention;
    schema?: FieldRuleDto[];
  },
): CreateTableOp {
  const op: CreateTableOp = {
    create_table: name,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.if_not_exists) op.if_not_exists = true;
  if (opts?.retention !== undefined) op.retention = opts.retention;
  if (opts?.schema !== undefined && opts.schema.length > 0)
    op.schema = opts.schema;
  return op;
}

// ── HMAC-gated ops ──────────────────────────────────────────────────

/** Drop a database (HMAC-gated). */
export function dropDb(
  signer: HmacSigner,
  db: string,
  opts?: { cascade?: boolean; if_exists?: boolean },
): DropDbOp {
  const canonical = canonicalDropDb(db);
  const op: DropDbOp = {
    drop_db: db,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.cascade) op.cascade = true;
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/** Drop a repository (HMAC-gated). */
export function dropRepo(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  opts?: { cascade?: boolean; if_exists?: boolean },
): DropRepoOp {
  const canonical = canonicalDropRepo(dbInUse, repo);
  const op: DropRepoOp = {
    drop_repo: repo,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.cascade) op.cascade = true;
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/** Drop a table (HMAC-gated). */
export function dropTable(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  table: string,
  opts?: { if_exists?: boolean; cascade?: boolean },
): DropTableOp {
  const canonical = canonicalDropTable(dbInUse, repo, table);
  const op: DropTableOp = {
    drop_table: table,
    repo,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.if_exists) op.if_exists = true;
  if (opts?.cascade) op.cascade = true;
  return op;
}

/** Rename a table inside a repository. Not HMAC-gated. */
export function renameTable(
  from: string,
  to: string,
  opts?: { repo?: string },
): RenameTableOp {
  const op: RenameTableOp = {
    rename_table: from,
    to,
  };
  if (opts?.repo !== undefined) op.repo = opts.repo;
  return op;
}

/** Rename a repository inside the current database. Not HMAC-gated. */
export function renameRepo(from: string, to: string): RenameRepoOp {
  const op: RenameRepoOp = {
    rename_repo: from,
    to,
  };
  return op;
}

/** Rename a database (pure catalogue re-key, no file move). Not HMAC-gated. */
export function renameDb(from: string, to: string): RenameDbOp {
  const op: RenameDbOp = {
    rename_db: from,
    to,
  };
  return op;
}


/** Start an online table migration (HMAC-gated). */
export function startMigration(
  signer: HmacSigner,
  dbInUse: string,
  srcRepo: string,
  table: string,
  dstRepo: string,
  dstEngine: string,
  opts?: { dst_path?: string },
): StartMigrationOp {
  const canonical = canonicalStartMigration(
    dbInUse,
    srcRepo,
    table,
    dstRepo,
    dstEngine,
  );
  const op: StartMigrationOp = {
    start_migration: table,
    repo: srcRepo,
    dst_repo: dstRepo,
    dst_engine: dstEngine,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.dst_path !== undefined) op.dst_path = opts.dst_path;
  return op;
}

/** Commit a running migration (HMAC-gated). */
export function commitMigration(
  signer: HmacSigner,
  dbInUse: string,
  migrationId: string,
): CommitMigrationOp {
  const canonical = canonicalCommitMigration(dbInUse, migrationId);
  return {
    commit_migration: migrationId,
    hmac: signer.hmacTagHex(canonical),
  };
}

/** Rollback a running migration (HMAC-gated). */
export function rollbackMigration(
  signer: HmacSigner,
  dbInUse: string,
  migrationId: string,
): RollbackMigrationOp {
  const canonical = canonicalRollbackMigration(dbInUse, migrationId);
  return {
    rollback_migration: migrationId,
    hmac: signer.hmacTagHex(canonical),
  };
}

// ── Schema DDL ops ─────────────────────────────────────────────────

/** Whole-replace a table's declarative schema. */
export function setTableSchema(
  table: string,
  schema: FieldRuleDto[],
  opts?: { repo?: string; expectedVersion?: number | bigint },
): SetTableSchemaOp {
  const op: SetTableSchemaOp = {
    set_table_schema: table,
    repo: repoOrDefault(opts?.repo),
    schema,
  };
  if (opts?.expectedVersion !== undefined) {
    assertSafeVersion(opts.expectedVersion, 'setTableSchema(opts.expectedVersion)');
    op.expected_version = opts.expectedVersion;
  }
  return op;
}

/** Add (or replace by path) a single rule in a table's schema. */
export function addSchemaRule(
  table: string,
  rule: FieldRuleDto,
  opts?: { repo?: string },
): AddSchemaRuleOp {
  return {
    add_schema_rule: table,
    repo: repoOrDefault(opts?.repo),
    rule,
  };
}

/** Remove a rule from a table's schema by path. */
export function removeSchemaRule(
  table: string,
  path: string[],
  opts?: { repo?: string },
): RemoveSchemaRuleOp {
  return {
    remove_schema_rule: table,
    repo: repoOrDefault(opts?.repo),
    path,
  };
}

/** Read a table's declarative schema (introspection). */
export function getTableSchema(
  table: string,
  opts?: { repo?: string },
): GetTableSchemaOp {
  return {
    get_table_schema: table,
    repo: repoOrDefault(opts?.repo),
  };
}

/** Describe a table — full introspection in one response. */
export function describeTable(
  table: string,
  opts?: { repo?: string },
): DescribeTableOp {
  return {
    describe_table: table,
    repo: repoOrDefault(opts?.repo),
  };
}
