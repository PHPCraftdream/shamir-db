/**
 * DDL index builders — `createIndex` plus the strict-by-default typed index
 * constructors (`hashIndex`, `uniqueIndex`, `sortedIndex`, `ftsIndex`,
 * `functionalIndex`, `vectorIndex`) and index drop/rename. Split out of the
 * former monolithic `builders/ddl.ts`.
 */

import type {
  HmacSigner,
  CreateIndexOp,
  DropIndexOp,
  RenameIndexOp,
} from '../../types/schema/ddl.js';
import { repoOrDefault } from './ddl-schema.js';
import { canonicalBatchOp } from '../../protocol/hmac.js';


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
    functional_args?: import('../../types/write/write.js').WireValue[];
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
    args?: import('../../types/write/write.js').WireValue[];
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
 */
export function dropIndex(
  signer: HmacSigner,
  dbInUse: string,
  repo: string,
  table: string,
  index: string,
  opts?: { if_exists?: boolean },
): DropIndexOp {
  const canonical = canonicalBatchOp(dbInUse, { drop_index: index, repo, table, if_exists: opts?.if_exists ?? false });
  const op: DropIndexOp = {
    drop_index: index,
    table,
    repo,
    hmac: signer.hmacTagHex(canonical),
  };
  if (opts?.if_exists) op.if_exists = true;
  return op;
}

