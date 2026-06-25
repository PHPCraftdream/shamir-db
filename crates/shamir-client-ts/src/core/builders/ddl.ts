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
  FieldRuleDto,
  ConstraintsDto,
  FkAction,
  SetTableSchemaOp,
  AddSchemaRuleOp,
  RemoveSchemaRuleOp,
  GetTableSchemaOp,
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
    functional_args?: import('../types/write.js').WireValue[];
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
export function dropFunction(
  name: string,
  opts?: { if_exists?: boolean },
): DropFunctionOp {
  const op: DropFunctionOp = { drop_function: name };
  if (opts?.if_exists) op.if_exists = true;
  return op;
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
export function dropValidator(
  name: string,
  opts?: { if_exists?: boolean },
): DropValidatorOp {
  const op: DropValidatorOp = { drop_validator: name };
  if (opts?.if_exists) op.if_exists = true;
  return op;
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
  opts?: { if_exists?: boolean },
): DropTableOp {
  const canonical = canonicalDropTable(dbInUse, repo, table);
  const op: DropTableOp = {
    drop_table: table,
    repo,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/** Drop an index (HMAC-gated). */
export function dropIndex(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  table: string,
  index: string,
  opts?: { unique?: boolean; if_exists?: boolean },
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
  if (opts?.if_exists) op.if_exists = true;
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

// ── field() fluent API ──────────────────────────────────────────────

/**
 * Fluent builder for a single `FieldRuleDto`. Mirrors the Rust
 * `shamir_query_builder::ddl::field()` API.
 *
 * ```ts
 * field(["email"]).string().max(255).required()
 * field(["age"]).int().min(0).max(150)
 * ```
 */
export class FieldBuilder {
  private _path: string[];
  private _type = '';
  private _constraints: ConstraintsDto = {};

  constructor(path: string[]) {
    this._path = path;
  }

  // ── type setters ────────────────────────────────────────────────
  string(): this { this._type = 'string'; return this; }
  int(): this { this._type = 'int'; return this; }
  f64(): this { this._type = 'f64'; return this; }
  dec(): this { this._type = 'dec'; return this; }
  bool(): this { this._type = 'bool'; return this; }
  bin(): this { this._type = 'bin'; return this; }
  list(): this { this._type = 'list'; return this; }
  map(): this { this._type = 'map'; return this; }
  any(): this { this._type = 'any'; return this; }
  typeTag(tag: string): this { this._type = tag; return this; }

  // ── constraint setters ──────────────────────────────────────────
  required(): this { this._constraints.required = true; return this; }
  nullable(): this { this._constraints.nullable = true; return this; }
  unsigned(): this { this._constraints.unsigned = true; return this; }
  min(v: number): this { this._constraints.min = v; return this; }
  max(v: number): this { this._constraints.max = v; return this; }
  len(v: number): this { this._constraints.len = v; return this; }
  maxLen(v: number): this { this._constraints.max_len = v; return this; }
  minLen(v: number): this { this._constraints.min_len = v; return this; }
  arrayOf(tag: string): this { this._constraints.array_of = tag; return this; }

  // ── Phase B constraint setters ──────────────────────────────────

  /**
   * Phase B — scalar-bridge: validate the field by calling the named
   * registered scalar as a predicate.
   */
  scalar(name: string): this { this._constraints.scalar = name; return this; }

  /**
   * Allowed-value set (enum constraint).
   */
  oneOf(values: import('../types/write.js').WireValue[]): this {
    this._constraints.one_of = values;
    return this;
  }

  /**
   * Phase C3 — unique constraint.
   * The field value must not duplicate any existing row in the same table.
   */
  unique(): this { this._constraints.unique = true; return this; }

  /**
   * Phase B — named format check (`"email"` / `"url"` / `"uuid"` / `"date"`).
   */
  format(kind: string): this { this._constraints.format = kind; return this; }

  /**
   * Phase B — cross-field comparison against another path.
   * `op` is the comparison operator string (`"<"`, `"<="`, `"=="`, `"!="`,
   * `">="`, `">"`).
   */
  compare(other: string[], op: string): this {
    this._constraints.compare = { other, op };
    return this;
  }

  /**
   * Phase C2 — forward-only foreign-key reference.
   * The field value must exist in `refTable.refField`.
   * `onDelete` defaults to `'restrict'` (matching the Rust builder); pass
   * `'cascade'` / `'set_null'` / `'no_action'` to override.
   */
  foreignKey(
    refTable: string,
    refField: string,
    opts?: { onDelete?: FkAction },
  ): this {
    this._constraints.foreign_key = {
      ref_table: refTable,
      ref_field: refField,
      on_delete: opts?.onDelete ?? 'restrict',
    };
    return this;
  }

  /** Finalize into a wire-ready `FieldRuleDto`. */
  build(): FieldRuleDto {
    const dto: FieldRuleDto = {
      path: this._path,
      type: this._type,
    };
    // Spread only defined constraint keys (mirrors serde skip_serializing_if).
    for (const [k, v] of Object.entries(this._constraints)) {
      if (v !== undefined) {
        (dto as unknown as Record<string, unknown>)[k] = v;
      }
    }
    return dto;
  }
}

/** Start building a `FieldRuleDto` for the given path segments. */
export function field(path: string[]): FieldBuilder {
  return new FieldBuilder(path);
}

// ── Schema DDL ops ─────────────────────────────────────────────────

/** Whole-replace a table's declarative schema. */
export function setTableSchema(
  table: string,
  schema: FieldRuleDto[],
  opts?: { repo?: string; expectedVersion?: number },
): SetTableSchemaOp {
  const op: SetTableSchemaOp = {
    set_table_schema: table,
    repo: repoOrDefault(opts?.repo),
    schema,
  };
  if (opts?.expectedVersion !== undefined)
    op.expected_version = opts.expectedVersion;
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

/** Aggregate namespace — every DDL constructor in one object. */
export const ddl = {
  currentOnly,
  olderThan,
  olderThanAge,
  field,
  FieldBuilder,
  createDb,
  createRepo,
  createTable,
  createIndex,
  setTableSchema,
  addSchemaRule,
  removeSchemaRule,
  getTableSchema,
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
