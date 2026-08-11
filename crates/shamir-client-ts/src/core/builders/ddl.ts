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
  ForeignKeyDto,
  FkAction,
  SetTableSchemaOp,
  AddSchemaRuleOp,
  RemoveSchemaRuleOp,
  GetTableSchemaOp,
  DescribeTableOp,
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
  RenameFunctionFolderOp,
  SetRetentionOp,
  PurgeHistoryOp,
  ChangesSinceOp,
  InternerDumpOp,
  InternerTouchOp,
  ListOp,
  DropDbOp,
  DropRepoOp,
  DropTableOp,
  DropIndexOp,
  RenameTableOp,
  RenameRepoOp,
  RenameDbOp,
  RenameIndexOp,
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
  canonicalSetRetention,
  canonicalPurgeHistory,
} from '../hmac.js';
import { assertSafeVersion } from './write.js';

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

/**
 * Create an index on a table.
 *
 * **`CREATE INDEX` holds a table-wide write lock for the ENTIRE backfill
 * scan** (every other writer queues for the full duration). On a
 * medium-to-large table this is a write OUTAGE, not a brief pause — see
 * KNOWN_LIMITATIONS.md §3. Because there is no server-side per-DDL timeout,
 * the request is bounded only by the client's `requestTimeoutMs`. For a
 * `createIndex` call against a large table, pass a generous
 * `requestTimeoutMs` (or `0` to disable it) on the `execute`/`Batch.execute`
 * call that carries this op, so the client does not abort a legitimately
 * long-running build.
 */
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
    /** V5.2 #411 — `"sq8"` enables SQ8 scalar quantization. Opt-in. */
    vector_quantization?: string;
    include?: string[][];
    if_not_exists?: boolean;
  },
): CreateIndexOp {
  if (opts?.unique && opts?.sorted) {
    throw new Error(
      'createIndex: an index cannot be both unique and sorted ' +
        '(server rejects this combination — see admin_table_index.rs)',
    );
  }

  // P1-6 (#970): cross-type validation — mirrors admin_table_index.rs and
  // CreateIndex::try_build() so a caller sees the same rejection everywhere.
  const itype = opts?.index_type;
  const nonBtree =
    itype === 'vector' || itype === 'fts' || itype === 'functional';

  // 1. At least one field for any index type.
  if (fields.length === 0) {
    throw new Error(
      'createIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  // 2. unique is only for btree/hash indexes.
  if (opts?.unique && nonBtree) {
    throw new Error(
      `createIndex: \`unique\` is not supported for '${itype}' indexes; ` +
        `only btree/hash indexes can be unique (server rejects this combination — see admin_table_index.rs)`,
    );
  }
  // 3. sorted is only for btree indexes.
  if (opts?.sorted && nonBtree) {
    throw new Error(
      `createIndex: \`sorted\` is not supported for '${itype}' indexes ` +
        `(server rejects this combination — see admin_table_index.rs)`,
    );
  }
  // 4. Vector index requires vector_dim > 0.
  if (
    itype === 'vector' &&
    (opts?.vector_dim === undefined || opts.vector_dim === 0)
  ) {
    throw new Error(
      'createIndex: vector index requires `vector_dim` > 0 ' +
        '(server rejects a missing or zero dimension — see admin_table_index.rs)',
    );
  }
  // 5. Vector metric must be a recognized value.
  if (itype === 'vector' && opts?.vector_metric !== undefined) {
    const m = opts.vector_metric;
    if (m !== 'l2' && m !== 'dot' && m !== 'cosine') {
      throw new Error(
        `createIndex: unknown vector_metric '${m}'; expected 'l2', 'dot', or 'cosine' ` +
          `(server rejects unrecognized metric strings — see admin_table_index.rs)`,
      );
    }
  }
  // 6. Vector-specific options only on vector indexes.
  if (
    itype !== 'vector' &&
    (opts?.vector_dim !== undefined ||
      opts?.vector_metric !== undefined ||
      opts?.vector_quantization !== undefined)
  ) {
    throw new Error(
      'createIndex: vector_dim/vector_metric/vector_quantization are only valid for ' +
        "'vector' indexes (server rejects these options on non-vector index types — see admin_table_index.rs)",
    );
  }
  // 7. FTS-specific options only on FTS indexes.
  if (
    itype !== 'fts' &&
    (opts?.fts_tokenizer !== undefined || opts?.fts_language !== undefined)
  ) {
    throw new Error(
      'createIndex: fts_tokenizer/fts_language are only valid for ' +
        "'fts' indexes (server rejects these options on non-fts index types — see admin_table_index.rs)",
    );
  }
  // 8. Functional-specific options only on functional indexes.
  if (
    itype !== 'functional' &&
    (opts?.functional_op !== undefined ||
      opts?.functional_args !== undefined)
  ) {
    throw new Error(
      'createIndex: functional_op/functional_args are only valid for ' +
        "'functional' indexes (server rejects these options on non-functional index types — see admin_table_index.rs)",
    );
  }
  // 9. `include` (covering index) is only meaningful for sorted btree indexes.
  if (opts?.include !== undefined && opts.include.length > 0 && nonBtree) {
    throw new Error(
      `createIndex: \`include\` is not supported for '${itype}' indexes; ` +
        `covering fields are only valid for sorted indexes (server rejects this combination — see admin_table_index.rs)`,
    );
  }
  // include without sorted is rejected for the btree family (mirrors the
  // server's pre-existing check that runs after the non-btree dispatch).
  if (
    opts?.include !== undefined &&
    opts.include.length > 0 &&
    !opts?.sorted
  ) {
    throw new Error(
      'createIndex: `include` is only valid for sorted indexes; call sorted: true ' +
        'before include, or drop the include option (server rejects include without sorted — see admin_table_index.rs)',
    );
  }
  // 10. Sorted indexes are single-field scalar columns only (mirrors
  // CreateIndexBuildError::SortedMultiField in Rust's try_build() and the
  // server's "Sorted index requires exactly one field (composite TBD)" check
  // in admin_table_index.rs).
  if (opts?.sorted && fields.length !== 1) {
    throw new Error(
      `createIndex: a sorted index requires exactly one field, got ${fields.length}; ` +
        'sorted indexes are single-field scalar columns only (server rejects multi-field sorted indexes — see admin_table_index.rs)',
    );
  }

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
  if (opts?.vector_quantization !== undefined)
    op.vector_quantization = opts.vector_quantization;
  if (opts?.include !== undefined && opts.include.length > 0)
    op.include = opts.include;
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

// ── Typed index constructors (strict-by-default) ───────────────────────────

/** FTS tokenizer string literal type. */
export type Tokenizer = 'whitespace' | 'unicode';

/** Vector metric string literal type. */
export type Metric = 'l2' | 'cosine' | 'dot';

/** Vector quantization string literal type (undefined = Off/unquantized). */
export type Quantization = 'sq8' | undefined;

/**
 * Create a hash/btree index on one or more fields (not unique).
 *
 * This is a **strict-by-default** typed constructor: it validates that
 * `fields` is non-empty, then produces a valid `CreateIndexOp` directly.
 * The output is byte-identical to the equivalent `createIndex()` call.
 *
 * @param name Index name
 * @param table Table name
 * @param fields Field paths (array of path arrays)
 * @param opts Optional: repo, if_not_exists
 */
export function hashIndex(
  name: string,
  table: string,
  fields: string[][],
  opts?: { repo?: string; if_not_exists?: boolean },
): CreateIndexOp {
  if (fields.length === 0) {
    throw new Error(
      'hashIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields,
    unique: false,
    sorted: false,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/**
 * Create a unique hash/btree index on one or more fields.
 *
 * This is a **strict-by-default** typed constructor: it validates that
 * `fields` is non-empty, then produces a valid `CreateIndexOp` directly.
 *
 * @param name Index name
 * @param table Table name
 * @param fields Field paths (array of path arrays)
 * @param opts Optional: repo, if_not_exists
 */
export function uniqueIndex(
  name: string,
  table: string,
  fields: string[][],
  opts?: { repo?: string; if_not_exists?: boolean },
): CreateIndexOp {
  if (fields.length === 0) {
    throw new Error(
      'uniqueIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields,
    unique: true,
    sorted: false,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/**
 * Create a sorted (value-ordered) index on a single field.
 *
 * This is a **strict-by-default** typed constructor: it validates that
 * `field` is non-empty, then produces a valid `CreateIndexOp` directly.
 * The field parameter is typed as a single path (string[]), making
 * multi-field sorted indexes a type error.
 *
 * @param name Index name
 * @param table Table name
 * @param field Single field path
 * @param opts Optional: repo, if_not_exists
 */
export function sortedIndex(
  name: string,
  table: string,
  field: string[],
  opts?: { repo?: string; if_not_exists?: boolean },
): CreateIndexOp {
  if (field.length === 0) {
    throw new Error(
      'sortedIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields: [field],
    unique: false,
    sorted: true,
    repo: repoOrDefault(opts?.repo),
  };
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/**
 * Create a sorted index with covering (include) fields.
 *
 * This is a **strict-by-default** typed constructor that validates the
 * field path is non-empty, accepts both the sorted field and covering
 * fields, and produces a valid `CreateIndexOp`.
 *
 * @param name Index name
 * @param table Table name
 * @param field Single field path
 * @param include Covering field paths
 * @param opts Optional: repo, if_not_exists
 */
export function sortedWithIncludeIndex(
  name: string,
  table: string,
  field: string[],
  include: string[][],
  opts?: { repo?: string; if_not_exists?: boolean },
): CreateIndexOp {
  if (field.length === 0) {
    throw new Error(
      'sortedWithIncludeIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields: [field],
    unique: false,
    sorted: true,
    repo: repoOrDefault(opts?.repo),
    include: include.length > 0 ? include : undefined,
  };
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/**
 * Create a full-text search index on a single field with a tokenizer.
 *
 * This is a **strict-by-default** typed constructor: it validates that
 * `field` is non-empty, then produces a valid `CreateIndexOp` directly.
 *
 * @param name Index name
 * @param table Table name
 * @param field Single field path
 * @param tokenizer FTS tokenizer ('whitespace' | 'unicode')
 * @param opts Optional: language, repo, if_not_exists
 */
export function ftsIndex(
  name: string,
  table: string,
  field: string[],
  tokenizer: Tokenizer,
  opts?: {
    language?: string;
    repo?: string;
    if_not_exists?: boolean;
  },
): CreateIndexOp {
  if (field.length === 0) {
    throw new Error(
      'ftsIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields: [field],
    unique: false,
    sorted: false,
    repo: repoOrDefault(opts?.repo),
    index_type: 'fts',
    fts_tokenizer: tokenizer,
  };
  if (opts?.language !== undefined) op.fts_language = opts.language;
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/**
 * Create a functional (derived) index on a single field.
 *
 * This is a **strict-by-default** typed constructor: it validates that
 * `field` is non-empty, then produces a valid `CreateIndexOp` directly.
 *
 * @param name Index name
 * @param table Table name
 * @param field Single field path
 * @param func Function name
 * @param opts Optional: args, repo, if_not_exists
 */
export function functionalIndex(
  name: string,
  table: string,
  field: string[],
  func: string,
  opts?: {
    args?: import('../types/write.js').WireValue[];
    repo?: string;
    if_not_exists?: boolean;
  },
): CreateIndexOp {
  if (field.length === 0) {
    throw new Error(
      'functionalIndex: CREATE INDEX requires at least one field ' +
        '(server rejects an empty fields list for all index types — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields: [field],
    unique: false,
    sorted: false,
    repo: repoOrDefault(opts?.repo),
    index_type: 'functional',
    functional_op: func,
  };
  if (opts?.args !== undefined && opts.args.length > 0)
    op.functional_args = opts.args;
  if (opts?.if_not_exists) op.if_not_exists = true;
  return op;
}

/**
 * Create a vector index on a single field with dimension, metric, and quantization.
 *
 * This is a **strict-by-default** typed constructor: it produces a valid
 * `CreateIndexOp` directly with no validation needed.
 *
 * @param name Index name
 * @param table Table name
 * @param field Single field path
 * @param dim Vector dimension (must be > 0)
 * @param metric Vector metric ('l2' | 'cosine' | 'dot')
 * @param quantization Quantization mode ('sq8' | undefined = Off)
 * @param opts Optional: repo, if_not_exists
 */
export function vectorIndex(
  name: string,
  table: string,
  field: string[],
  dim: number,
  metric: Metric,
  quantization: Quantization,
  opts?: { repo?: string; if_not_exists?: boolean },
): CreateIndexOp {
  if (dim <= 0) {
    throw new Error(
      `vectorIndex: vector_dim must be > 0, got ${dim} ` +
        '(server rejects zero or negative dimensions — see admin_table_index.rs)',
    );
  }
  const op: CreateIndexOp = {
    create_index: name,
    table,
    fields: [field],
    unique: false,
    sorted: false,
    repo: repoOrDefault(opts?.repo),
    index_type: 'vector',
    vector_dim: dim,
    vector_metric: metric,
  };
  if (quantization !== undefined) op.vector_quantization = quantization;
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
    /** `"public"` or `"private"` (absent → `"private"`). */
    visibility?: string;
    /** `"invoker"` or `"definer"` (absent → `"invoker"`). `"definer"` requires `hmac`. */
    security?: string;
    /** OS-seeded env-var secret grants. Non-empty requires `hmac`. */
    secret_grants?: string[];
    /** Hex HMAC tag, required IFF `security === "definer"` or non-empty `secret_grants`. */
    hmac?: string;
  },
): CreateFunctionOp {
  const op: CreateFunctionOp = {
    create_function: name,
    replace: opts?.replace ?? false,
  };
  if (opts?.source !== undefined) op.source = opts.source;
  if (opts?.wasm !== undefined) op.wasm = opts.wasm;
  if (opts?.visibility !== undefined) op.visibility = opts.visibility;
  if (opts?.security !== undefined) op.security = opts.security;
  if (opts?.secret_grants !== undefined && opts.secret_grants.length > 0) {
    op.secret_grants = opts.secret_grants;
  }
  if (opts?.hmac !== undefined) op.hmac = opts.hmac;
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

/** Rename a function folder (and its descendant subtree) by path segments. */
export function renameFunctionFolder(
  from: string[],
  to: string[],
): RenameFunctionFolderOp {
  return { rename_function_folder: from, to };
}

/**
 * Change a live table's history-retention policy (HMAC-gated).
 * canonical = `canonicalSetRetention(dbInUse, repo, table, retention)`.
 */
export function setRetention(
  signer: HmacSigner,
  dbInUse: string,
  table: string,
  retention: Retention,
  opts?: { repo?: string },
): SetRetentionOp {
  const repo = repoOrDefault(opts?.repo);
  const canonical = canonicalSetRetention(dbInUse, repo, table, retention);
  return {
    set_retention: table,
    repo,
    retention,
    hmac: signer.hmacTagHex(canonical),
  };
}

/**
 * Imperative history purge for a table (HMAC-gated) — irreversible
 * audit-trail loss. canonical = `canonicalPurgeHistory(dbInUse, repo, table, scope)`.
 */
export function purgeHistory(
  signer: HmacSigner,
  dbInUse: string,
  table: string,
  scope: PurgeScope,
  opts?: { repo?: string },
): PurgeHistoryOp {
  const repo = repoOrDefault(opts?.repo);
  const canonical = canonicalPurgeHistory(dbInUse, repo, table, scope);
  return {
    purge_history: table,
    repo,
    scope,
    hmac: signer.hmacTagHex(canonical),
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

/** Rename an index on a table (in-place rekey, no data loss). Not HMAC-gated. */
export function renameIndex(
  table: string,
  from: string,
  to: string,
  opts?: { repo?: string; if_exists?: boolean },
): RenameIndexOp {
  const op: RenameIndexOp = {
    rename_index: from,
    to,
    table,
  };
  if (opts?.repo !== undefined) op.repo = opts.repo;
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

/**
 * Drop an index (HMAC-gated).
 *
 * **Note:** The `unique` option is now informational-only and used only for
 * HMAC canonical input generation. The server resolves the actual index family
 * from the catalog (regular hash, unique hash, sorted, or index2) and drops
 * whichever family the index actually belongs to — it no longer trusts the
 * client's `unique` hint for resolution. Setting this incorrectly does not
 * affect which index is dropped, only changes the bytes signed into the HMAC.
 */
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
  };  if (unique) op.unique = true;
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
   * ③.2c — default value (literal or expression) stamped on INSERT for an
   * absent field (extends Phase ②.4b literal-only to expression).
   *
   * - **Literal** forms (null/bool/number/string/array/object) route through
   *   the fast `apply_defaults` path (②.4c behaviour is unchanged).
   * - **Expression** `ComputedExpr` forms (`$fn` / `$ref` / etc.) route
   *   through `apply_transforms` → `eval_write_value` → `builtin_scalars()`
   *   at admission-time. User scalars are NOT available here (same boundary
   *   as inline `$fn` write-field expressions).
   *
   * Accepts `WriteValue` (superset of `WireValue` + `ComputedExpr`). Mirrors
   * the Rust builder's `.default(impl Into<FilterValue>)`.
   */
  default(value: import('../types/write.js').WriteValue): this {
    this._constraints.default = value;
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
   * ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
   * this field on **every** write (INSERT and UPDATE). The server clock is
   * always authoritative — any caller-supplied value is overwritten.
   *
   * Typical usage: `updated_at` field. Mirrors the Rust `.auto_now()` builder.
   */
  autoNow(): this { this._constraints.auto_now = true; return this; }

  /**
   * ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
   * this field on **INSERT** only, and only when the field is absent.
   * An explicitly-supplied value (including explicit `null`) is preserved.
   *
   * Typical usage: `created_at` field. Mirrors the Rust `.auto_now_add()` builder.
   */
  autoNowAdd(): this { this._constraints.auto_now_add = true; return this; }

  /**
   * Phase C2 — forward-only foreign-key reference.
   * The field value must exist in `refTable.refField`.
   * `onDelete` defaults to `'restrict'` (matching the Rust builder); pass
   * `'cascade'` / `'set_null'` / `'no_action'` to override.
   * `onUpdate` defaults to `'no_action'` (Phase ②.2a — surface only;
   * enforcement in ②.2b; additive — existing callers keep current behavior).
   */
  foreignKey(
    refTable: string,
    refField: string,
    opts?: { onDelete?: FkAction; onUpdate?: FkAction },
  ): this {
    const fk: ForeignKeyDto = {
      ref_table: refTable,
      ref_field: refField,
      on_delete: opts?.onDelete ?? 'restrict',
    };
    const onUpdate = opts?.onUpdate ?? 'no_action';
    if (onUpdate !== 'no_action') {
      fk.on_update = onUpdate;
    }
    this._constraints.foreign_key = fk;
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
  describeTable,
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
  renameFunctionFolder,
  setRetention,
  purgeHistory,
  changesSince,
  internerDump,
  internerTouch,
  listDatabases,
  listRepos,
  listTables,
  listIndexes,
  listUsers,
  listFunctions,
  listValidators_,
  listFunctionFolders,
  dropDb,
  dropRepo,
  dropTable,
  dropIndex,
  renameTable,
  renameRepo,
  renameDb,
  renameIndex,
  startMigration,
  commitMigration,
  rollbackMigration,
};
