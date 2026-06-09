/**
 * DDL (admin) operation builders — the CODE that constructs the wire shapes
 * declared in `../types/ddl.ts`. Mirrors
 * `crates/shamir-query-types/src/admin/types.rs`.
 *
 * Non-HMAC ops are plain functions returning the wire object.
 * HMAC-gated ops take a `signer: HmacSigner` + `dbInUse`, build the
 * canonical input via `../hmac.ts`, and attach `hmac: signer.hmacTagHex(…)`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  HmacSigner,
  Retention,
  BufferConfigDto,
  BufferConfigPatch,
  PurgeScope,
  WriteOpKind,
  CreateDbOp,
  CreateRepoOp,
  CreateTableOp,
  CreateIndexOp,
  SetBufferConfigOp,
  GetBufferConfigOp,
  AlterBufferConfigOp,
  MigrationStatusOp,
  CreateFunctionOp,
  DropFunctionOp,
  RenameFunctionOp,
  CreateValidatorOp,
  DropValidatorOp,
  RenameValidatorOp,
  BindValidatorOp,
  UnbindValidatorOp,
  ListValidatorsOp,
  CreateFunctionFolderOp,
  SetRetentionOp,
  PurgeHistoryOp,
  ChangesSinceOp,
  ListOp,
  DropDbOp,
  DropRepoOp,
  DropTableOp,
  DropIndexOp,
  StartMigrationOp,
  CommitMigrationOp,
  RollbackMigrationOp,
} from '../types/ddl.js';
import {
  canonicalDropDb,
  canonicalDropRepo,
  canonicalDropTable,
  canonicalDropIndex,
  canonicalStartMigration,
  canonicalCommitMigration,
  canonicalRollbackMigration,
} from '../hmac.js';

// ── Helpers ─────────────────────────────────────────────────────────

const DEFAULT_REPO = 'main';

function repoOrDefault(repo?: string): string {
  return repo ?? DEFAULT_REPO;
}

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

/** Create a new repository. */
export function createRepo(
  name: string,
  opts?: {
    engine?: string;
    path?: string;
    tables?: string[];
    if_not_exists?: boolean;
  },
): CreateRepoOp {
  const op: CreateRepoOp = { create_repo: name };
  if (opts?.engine !== undefined) op.engine = opts.engine;
  if (opts?.path !== undefined) op.path = opts.path;
  if (opts?.tables !== undefined && opts.tables.length > 0)
    op.tables = opts.tables;
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
  },
): CreateTableOp {
  const op: CreateTableOp = {
    create_table: name,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.if_not_exists) op.if_not_exists = true;
  if (opts?.retention !== undefined) op.retention = opts.retention;
  return op;
}

/** Create an index on a table. */
export function createIndex(
  name: string,
  table: string,
  fields: string[][],
  opts?: {
    unique?: boolean;
    sorted?: boolean;
    repo?: string;
    index_type?: string;
    fts_tokenizer?: string;
    fts_language?: string;
    functional_op?: string;
    functional_args?: import('../types/write.js').Json[];
    vector_dim?: number;
    vector_metric?: string;
    include?: string[][];
    if_not_exists?: boolean;
  },
): CreateIndexOp {
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields,
    unique: opts?.unique ?? false,
    sorted: opts?.sorted ?? false,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.index_type !== undefined) op.index_type = opts.index_type;
  if (opts?.fts_tokenizer !== undefined)
    op.fts_tokenizer = opts.fts_tokenizer;
  if (opts?.fts_language !== undefined)
    op.fts_language = opts.fts_language;
  if (opts?.functional_op !== undefined)
    op.functional_op = opts.functional_op;
  if (opts?.functional_args !== undefined)
    op.functional_args = opts.functional_args;
  if (opts?.vector_dim !== undefined) op.vector_dim = opts.vector_dim;
  if (opts?.vector_metric !== undefined)
    op.vector_metric = opts.vector_metric;
  if (opts?.include !== undefined && opts.include.length > 0)
    op.include = opts.include;
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
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

/** Create (or replace) a stored function. */
export function createFunction(
  name: string,
  opts?: {
    source?: string;
    wasm?: string;
    replace?: boolean;
  },
): CreateFunctionOp {
  const op: CreateFunctionOp = {
    create_function: name,
    replace: opts?.replace ?? false,
  };
  if (opts?.source !== undefined) op.source = opts.source;
  if (opts?.wasm !== undefined) op.wasm = opts.wasm;
  return op;
}

/** Drop a stored function. */
export function dropFunction(name: string): DropFunctionOp {
  return { drop_function: name };
}

/** Rename a stored function. */
export function renameFunction(
  oldName: string,
  newName: string,
): RenameFunctionOp {
  return { rename_function: oldName, to: newName };
}

/** Create (or replace) a validator. */
export function createValidator(
  name: string,
  opts?: {
    source?: string;
    wasm?: string;
    replace?: boolean;
  },
): CreateValidatorOp {
  const op: CreateValidatorOp = {
    create_validator: name,
    replace: opts?.replace ?? false,
  };
  if (opts?.source !== undefined) op.source = opts.source;
  if (opts?.wasm !== undefined) op.wasm = opts.wasm;
  return op;
}

/** Drop a validator. */
export function dropValidator(name: string): DropValidatorOp {
  return { drop_validator: name };
}

/** Rename a validator. */
export function renameValidator(
  oldName: string,
  newName: string,
): RenameValidatorOp {
  return { rename_validator: oldName, to: newName };
}

/** Bind a validator to a table on specified write operations. */
export function bindValidator(
  name: string,
  table: string,
  ops: WriteOpKind[],
  priority: number,
  opts: {
    db: string;
    repo?: string;
  },
): BindValidatorOp {
  return {
    bind_validator: name,
    db: opts.db,
    repo: repoOrDefault(opts.repo),
    table,
    ops,
    priority,
  };
}

/** Unbind a validator from a table. */
export function unbindValidator(
  name: string,
  opts: {
    db: string;
    repo?: string;
    table: string;
  },
): UnbindValidatorOp {
  return {
    unbind_validator: name,
    db: opts.db,
    repo: repoOrDefault(opts.repo),
    table: opts.table,
  };
}

/** List validator bindings for a table. */
export function listValidators(
  table: string,
  opts: {
    db: string;
    repo?: string;
  },
): ListValidatorsOp {
  return {
    list_validators: table,
    db: opts.db,
    repo: repoOrDefault(opts.repo),
  };
}

/** Create a function folder by path segments. */
export function createFunctionFolder(
  segments: string[],
): CreateFunctionFolderOp {
  return { create_function_folder: segments };
}

/** Change a live table's history-retention policy. */
export function setRetention(
  table: string,
  retention: Retention,
  opts?: { repo?: string },
): SetRetentionOp {
  return {
    set_retention: table,
    repo: repoOrDefault(opts?.repo),
    retention,
  };
}

/** Imperative history purge for a table. */
export function purgeHistory(
  table: string,
  scope: PurgeScope,
  opts?: { repo?: string },
): PurgeHistoryOp {
  return {
    purge_history: table,
    repo: repoOrDefault(opts?.repo),
    scope,
  };
}

/** One-shot "changes since version V" read. */
export function changesSince(
  cursor: number,
  opts?: { repo?: string; limit?: number },
): ChangesSinceOp {
  const op: ChangesSinceOp = {
    changes_since: cursor,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.limit !== undefined) op.limit = opts.limit;
  return op;
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

export function listRoles(): ListOp {
  return { list: 'roles' };
}

export function listFunctions(opts?: { folder?: string }): ListOp {
  const op: ListOp = { list: 'functions' };
  if (opts?.folder !== undefined) {
    (op as { list: 'functions'; folder?: string }).folder = opts.folder;
  }
  return op;
}

export function listValidators_(): ListOp {
  return { list: 'validators' };
}

export function listFunctionFolders(opts?: { parent?: string }): ListOp {
  const op: ListOp = { list: 'function_folders' };
  if (opts?.parent !== undefined) {
    (op as { list: 'function_folders'; parent?: string }).parent =
      opts.parent;
  }
  return op;
}

// ── HMAC-gated ops ──────────────────────────────────────────────────

/** Drop a database (HMAC-gated). */
export function dropDb(
  signer: HmacSigner,
  db: string,
  opts?: { cascade?: boolean },
): DropDbOp {
  const canonical = canonicalDropDb(db);
  const op: DropDbOp = {
    drop_db: db,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.cascade) op.cascade = true;
  return op;
}

/** Drop a repository (HMAC-gated). */
export function dropRepo(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  opts?: { cascade?: boolean },
): DropRepoOp {
  const canonical = canonicalDropRepo(dbInUse, repo);
  const op: DropRepoOp = {
    drop_repo: repo,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.cascade) op.cascade = true;
  return op;
}

/** Drop a table (HMAC-gated). */
export function dropTable(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  table: string,
): DropTableOp {
  const canonical = canonicalDropTable(dbInUse, repo, table);
  return {
    drop_table: table,
    repo,
    hmac: signer.hmacTagHex(canonical),
  };
}

/** Drop an index (HMAC-gated). */
export function dropIndex(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  table: string,
  index: string,
  opts?: { unique?: boolean },
): DropIndexOp {
  const unique = opts?.unique ?? false;
  const canonical = canonicalDropIndex(dbInUse, repo, table, index, unique);
  const op: DropIndexOp = {
    drop_index: index,
    table,
    repo,
    hmac: signer.hmacTagHex(canonical),
  };
  if (unique) op.unique = true;
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

/** Aggregate namespace — every DDL constructor in one object. */
export const ddl = {
  currentOnly,
  olderThan,
  olderThanAge,
  createDb,
  createRepo,
  createTable,
  createIndex,
  setBufferConfig,
  getBufferConfig,
  alterBufferConfig,
  migrationStatus,
  createFunction,
  dropFunction,
  renameFunction,
  createValidator,
  dropValidator,
  renameValidator,
  bindValidator,
  unbindValidator,
  listValidators,
  createFunctionFolder,
  setRetention,
  purgeHistory,
  changesSince,
  listDatabases,
  listRepos,
  listTables,
  listIndexes,
  listUsers,
  listRoles,
  listFunctions,
  listValidators_,
  listFunctionFolders,
  dropDb,
  dropRepo,
  dropTable,
  dropIndex,
  startMigration,
  commitMigration,
  rollbackMigration,
};
