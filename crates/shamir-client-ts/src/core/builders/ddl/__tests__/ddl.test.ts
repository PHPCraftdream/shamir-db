/**
 * DDL-builder wire-shape tests.
 *
 * The authority for every shape is
 * `crates/shamir-query-types/src/admin/types.rs` (serde: skip_serializing_if,
 * default, default_repo, rename_all) cross-checked with e2e test helpers
 * `tests/e2e/helpers/hmac.js` and `tests/e2e/tests/08-admin-ddl.test.js`.
 */

import { describe, it, expect } from 'vitest';
import { ddl } from '../index.js';
import { canonicalBatchOp } from '../../../protocol/hmac.js';

/** Fake signer that returns a predictable tag based on canonical length. */
const fakeSigner = {
  hmacTagHex: (c: Uint8Array): string => 'tag:' + c.length,
};

// ── createDb ────────────────────────────────────────────────────────

describe('createDb', () => {
  it('emits {create_db}; no if_not_exists when false', () => {
    const op = ddl.createDb('testdb');
    expect(op).toEqual({ create_db: 'testdb' });
    expect(op).not.toHaveProperty('if_not_exists');
  });

  it('emits if_not_exists: true when set', () => {
    const op = ddl.createDb('testdb', { if_not_exists: true });
    expect(op).toEqual({ create_db: 'testdb', if_not_exists: true });
  });
});

// ── createRepo ──────────────────────────────────────────────────────

describe('createRepo', () => {
  it('emits {create_repo} with no optional fields', () => {
    const op = ddl.createRepo('myrepo');
    expect(op).toEqual({ create_repo: 'myrepo' });
    expect(op).not.toHaveProperty('engine');
    expect(op).not.toHaveProperty('path');
    expect(op).not.toHaveProperty('tables');
    expect(op).not.toHaveProperty('if_not_exists');
  });

  it('includes all optional fields when provided', () => {
    const op = ddl.createRepo('myrepo', {
      engine: 'mem',
      path: '/data',
      tables: ['users', 'sessions'],
      if_not_exists: true,
    });
    expect(op).toEqual({
      create_repo: 'myrepo',
      engine: 'mem',
      path: '/data',
      tables: ['users', 'sessions'],
      if_not_exists: true,
    });
  });

  it('omits empty tables array', () => {
    const op = ddl.createRepo('r', { tables: [] });
    expect(op).not.toHaveProperty('tables');
  });
});

// ── createTable ─────────────────────────────────────────────────────

describe('createTable', () => {
  it('emits {create_table, repo: "main"} by default', () => {
    const op = ddl.createTable('users');
    expect(op).toEqual({
      create_table: 'users',
      repo: 'main',
    });
  });

  it('explicit repo; if_not_exists + retention', () => {
    const op = ddl.createTable('users', {
      repo: 'hot',
      if_not_exists: true,
      retention: { max_count: 5 },
    });
    expect(op).toEqual({
      create_table: 'users',
      repo: 'hot',
      if_not_exists: true,
      retention: { max_count: 5 },
    });
  });

  it('omits if_not_exists when false', () => {
    const op = ddl.createTable('t', { if_not_exists: false });
    expect(op).not.toHaveProperty('if_not_exists');
  });
});

// ── createIndex ─────────────────────────────────────────────────────

describe('createIndex', () => {
  it('emits unique+sorted always present (serde default, no skip)', () => {
    const op = ddl.createIndex('idx_email', 'users', [['email']]);
    expect(op).toEqual({
      create_index: 'idx_email',
      table: 'users',
      fields: [['email']],
      unique: false,
      sorted: false,
      repo: 'main',
    });
  });

  it('unique=true (sorted=false) + explicit repo', () => {
    const op = ddl.createIndex('idx_name', 'users', [['name']], {
      unique: true,
      repo: 'analytics',
    });
    expect(op.unique).toBe(true);
    expect(op.sorted).toBe(false);
    expect(op.repo).toBe('analytics');
  });

  it('FTS options omitted when not set', () => {
    const op = ddl.createIndex('idx_fts', 'docs', [['body']]);
    expect(op).not.toHaveProperty('index_type');
    expect(op).not.toHaveProperty('fts_tokenizer');
    expect(op).not.toHaveProperty('fts_language');
    expect(op).not.toHaveProperty('functional_op');
    expect(op).not.toHaveProperty('vector_dim');
    expect(op).not.toHaveProperty('vector_metric');
    expect(op).not.toHaveProperty('vector_quantization');
    expect(op).not.toHaveProperty('include');
    expect(op).not.toHaveProperty('if_not_exists');
  });

  it('includes index_type, vector options when set', () => {
    const op = ddl.createIndex('vidx', 'docs', [['embedding']], {
      index_type: 'vector',
      vector_dim: 128,
      vector_metric: 'cosine',
      if_not_exists: true,
    });
    expect(op.index_type).toBe('vector');
    expect(op.vector_dim).toBe(128);
    expect(op.vector_metric).toBe('cosine');
    expect(op.if_not_exists).toBe(true);
    // V5.2 #411 — absent when not opted-in (wire back-compat).
    expect(op).not.toHaveProperty('vector_quantization');
  });

  it('includes covering fields (include) when set on a sorted btree index', () => {
    const op = ddl.createIndex('sidx', 'users', [['age']], {
      sorted: true,
      include: [['email']],
    });
    expect(op.sorted).toBe(true);
    expect(op.include).toEqual([['email']]);
  });

  // ── V5.2 #411 — vector quantization (SQ8) ───────────────────────────

  it('vector_quantization "sq8" lands on the op (dim+metric+quantization together)', () => {
    const op = ddl.createIndex('vidx_q', 'docs', [['embedding']], {
      index_type: 'vector',
      vector_dim: 256,
      vector_metric: 'cosine',
      vector_quantization: 'sq8',
    });
    expect(op.vector_quantization).toBe('sq8');
    // Full vector trio is present together.
    expect(op.vector_dim).toBe(256);
    expect(op.vector_metric).toBe('cosine');
  });

  it('vector_quantization omitted (undefined, not null) when not set', () => {
    const op = ddl.createIndex('vidx_plain', 'docs', [['embedding']], {
      index_type: 'vector',
      vector_dim: 128,
      vector_metric: 'l2',
    });
    expect(op).not.toHaveProperty('vector_quantization');
    // Strict undefined, never null — serde skip_serializing_if parity.
    expect((op as unknown as Record<string, unknown>).vector_quantization).toBeUndefined();
  });

  it('omits empty include array', () => {
    const op = ddl.createIndex('idx', 't', [['f']], { include: [] });
    expect(op).not.toHaveProperty('include');
  });

  it('throws when unique + sorted are both requested (server rejects this combination)', () => {
    expect(() =>
      ddl.createIndex('idx_bad', 't', [['f']], { unique: true, sorted: true }),
    ).toThrow(/cannot be both unique and sorted/);
  });

  it('throws when include is set without sorted (btree family)', () => {
    expect(() =>
      ddl.createIndex('idx_bad', 't', [['f']], { include: [['meta']] }),
    ).toThrow(/include.*only valid for sorted indexes/);
  });

  // ── #998: sorted-multi-field parity (was missing — now mirrors Rust's
  //    CreateIndexBuildError::SortedMultiField) ─────────────────────────

  it('throws when sorted index has more than one field', () => {
    expect(() =>
      ddl.createIndex('idx_bad', 't', [['a'], ['b']], { sorted: true }),
    ).toThrow(/sorted index requires exactly one field.*got 2/);
  });

  // ── P1-6 (#970): cross-type validation parity ──────────────────────

  it('throws when fields is empty (all index types)', () => {
    expect(() => ddl.createIndex('idx', 't', [])).toThrow(
      /requires at least one field/,
    );
  });

  it('throws when unique is set on a vector index', () => {
    expect(() =>
      ddl.createIndex('vidx', 't', [['f']], {
        index_type: 'vector',
        unique: true,
        vector_dim: 128,
      }),
    ).toThrow(/unique.*not supported for 'vector'/);
  });

  it('throws when sorted is set on an fts index', () => {
    expect(() =>
      ddl.createIndex('fidx', 't', [['f']], {
        index_type: 'fts',
        sorted: true,
      }),
    ).toThrow(/sorted.*not supported for 'fts'/);
  });

  it('throws when vector index has no vector_dim', () => {
    expect(() =>
      ddl.createIndex('vidx', 't', [['f']], { index_type: 'vector' }),
    ).toThrow(/vector_dim.*> 0/);
  });

  it('throws when vector index has vector_dim = 0', () => {
    expect(() =>
      ddl.createIndex('vidx', 't', [['f']], {
        index_type: 'vector',
        vector_dim: 0,
      }),
    ).toThrow(/vector_dim.*> 0/);
  });

  it('accepts vector_dim = 1 (boundary positive)', () => {
    expect(() =>
      ddl.createIndex('vidx', 't', [['f']], {
        index_type: 'vector',
        vector_dim: 1,
      }),
    ).not.toThrow();
  });

  it('throws when vector_metric is unrecognized', () => {
    expect(() =>
      ddl.createIndex('vidx', 't', [['f']], {
        index_type: 'vector',
        vector_dim: 128,
        vector_metric: 'consine',
      }),
    ).toThrow(/unknown vector_metric 'consine'/);
  });

  it('throws when vector options are set on a non-vector index', () => {
    expect(() =>
      ddl.createIndex('idx', 't', [['f']], { vector_dim: 128 }),
    ).toThrow(/only valid for 'vector' indexes/);
  });

  it('throws when fts options are set on a non-fts index', () => {
    expect(() =>
      ddl.createIndex('idx', 't', [['f']], { fts_tokenizer: 'whitespace' }),
    ).toThrow(/only valid for 'fts' indexes/);
  });

  it('throws when functional options are set on a non-functional index', () => {
    expect(() =>
      ddl.createIndex('idx', 't', [['f']], { functional_op: 'lower' }),
    ).toThrow(/only valid for 'functional' indexes/);
  });

  it('throws when include is set on a non-btree index', () => {
    expect(() =>
      ddl.createIndex('fidx', 't', [['f']], {
        index_type: 'fts',
        include: [['title']],
      }),
    ).toThrow(/include.*not supported for 'fts'/);
  });

  it('accepts valid specialized indexes (boundary positives)', () => {
    expect(() =>
      ddl.createIndex('vidx', 't', [['f']], {
        index_type: 'vector',
        vector_dim: 384,
        vector_metric: 'cosine',
      }),
    ).not.toThrow();
    expect(() =>
      ddl.createIndex('fidx', 't', [['f']], {
        index_type: 'fts',
        fts_tokenizer: 'unicode',
      }),
    ).not.toThrow();
    expect(() =>
      ddl.createIndex('fnidx', 't', [['f']], {
        index_type: 'functional',
        functional_op: 'lower',
      }),
    ).not.toThrow();
  });
});

// ── buffer config ops ───────────────────────────────────────────────

describe('buffer config', () => {
  const cfg: import('../../../types/schema/ddl.js').BufferConfigDto = {
    max_bytes: 1024,
    max_entries: 100,
    flush_interval_ms: 5000,
    flush_batch_size: 10,
  };

  it('setBufferConfig emits {set_buffer_config, repo, config}', () => {
    const op = ddl.setBufferConfig('users', cfg);
    expect(op).toEqual({
      set_buffer_config: 'users',
      repo: 'main',
      config: cfg,
    });
  });

  it('getBufferConfig emits {get_buffer_config, repo}', () => {
    const op = ddl.getBufferConfig('users', { repo: 'hot' });
    expect(op).toEqual({
      get_buffer_config: 'users',
      repo: 'hot',
    });
  });

  it('alterBufferConfig with double-option ttl_ms', () => {
    const op = ddl.alterBufferConfig('users', {
      ttl_ms: null,
      max_bytes: 2048,
    });
    expect(op.patch.ttl_ms).toBeNull();
    expect(op.patch.max_bytes).toBe(2048);
    expect(op.repo).toBe('main');
  });
});

// ── retention helpers ───────────────────────────────────────────────

describe('retention', () => {
  it('currentOnly() → {max_count: 0}', () => {
    expect(ddl.currentOnly()).toEqual({ max_count: 0 });
  });
});

// ── purge scope ─────────────────────────────────────────────────────

describe('purge scope', () => {
  it('olderThan emits {older_than: {timestamp}}', () => {
    expect(ddl.olderThan(1700000000000)).toEqual({
      older_than: { timestamp: 1700000000000 },
    });
  });

  it('olderThanAge emits {older_than_age: {age_secs}}', () => {
    expect(ddl.olderThanAge(86400)).toEqual({
      older_than_age: { age_secs: 86400 },
    });
  });
});

// ── purgeHistory / setRetention ─────────────────────────────────────

describe('purgeHistory (HMAC)', () => {
  it('emits {purge_history, repo, scope, hmac}', () => {
    const scope = ddl.olderThanAge(86400);
    const canonical = canonicalBatchOp('mydb', { purge_history: 'users', repo: 'main', scope });
    const op = ddl.purgeHistory(fakeSigner, 'mydb', 'users', scope);
    expect(op).toEqual({
      purge_history: 'users',
      repo: 'main',
      scope: { older_than_age: { age_secs: 86400 } },
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

describe('setRetention (HMAC)', () => {
  it('emits {set_retention, repo, retention, hmac}', () => {
    const retention = ddl.currentOnly();
    const canonical = canonicalBatchOp('mydb', { set_retention: 'users', repo: 'main', retention });
    const op = ddl.setRetention(fakeSigner, 'mydb', 'users', retention);
    expect(op).toEqual({
      set_retention: 'users',
      repo: 'main',
      retention: { max_count: 0 },
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});

// ── changesSince ────────────────────────────────────────────────────

describe('changesSince', () => {
  it('emits {changes_since, repo}; limit omitted when not set', () => {
    const op = ddl.changesSince(42);
    expect(op).toEqual({ changes_since: 42, repo: 'main' });
    expect(op).not.toHaveProperty('limit');
  });

  it('includes limit when set', () => {
    const op = ddl.changesSince(0, { repo: 'analytics', limit: 500 });
    expect(op).toEqual({
      changes_since: 0,
      repo: 'analytics',
      limit: 500,
    });
  });
});

// ── migrationStatus ─────────────────────────────────────────────────

describe('migrationStatus', () => {
  it('emits {migration_status: id}', () => {
    expect(ddl.migrationStatus('m123')).toEqual({ migration_status: 'm123' });
  });
});

// ── function DDL ────────────────────────────────────────────────────

describe('function DDL', () => {
  it('createFunction — replace always present (serde default, no skip)', () => {
    const op = ddl.createFunction('my_fn', { source: 'pub fn …' });
    expect(op).toEqual({
      create_function: 'my_fn',
      source: 'pub fn …',
      replace: false,
    });
  });

  it('createFunction — replace=true, wasm variant', () => {
    expect(() => ddl.createFunction('my_fn', { wasm: '<base64>', replace: true })).toThrow(/HMAC required/);
    const op = ddl.createFunction('my_fn', { wasm: '<base64>', replace: true, hmac: 'tag' });
    expect(op.replace).toBe(true);
    expect(op.wasm).toBe('<base64>');
    expect(op).not.toHaveProperty('source');
  });

  it('createFunction — visibility/security/secret_grants/hmac threaded through', () => {
    const op = ddl.createFunction('my_fn', {
      wasm: '<base64>',
      visibility: 'public',
      security: 'definer',
      secret_grants: ['FOO', 'BAR'],
      hmac: 'deadbeef',
    });
    expect(op.visibility).toBe('public');
    expect(op.security).toBe('definer');
    expect(op.secret_grants).toEqual(['FOO', 'BAR']);
    expect(op.hmac).toBe('deadbeef');
  });

  it('createFunction — new fields absent when not provided (wire back-compat)', () => {
    const op = ddl.createFunction('my_fn', { wasm: '<base64>' });
    expect(op).not.toHaveProperty('visibility');
    expect(op).not.toHaveProperty('security');
    expect(op).not.toHaveProperty('secret_grants');
    expect(op).not.toHaveProperty('hmac');
  });

  it('createFunction — empty secret_grants omitted (skip_serializing_if)', () => {
    const op = ddl.createFunction('my_fn', {
      wasm: '<base64>',
      secret_grants: [],
    });
    expect(op).not.toHaveProperty('secret_grants');
  });

  it('createFunction replacement requires HMAC (rv-query-builder-e6f53e38)', () => {
    expect(() => ddl.createFunction('fn', { replace: true })).toThrow(/HMAC required/);
  });
  it('createFunction rejects definer and secret grants without HMAC (rv-query-builder-e6f53e38)', () => {
    expect(() => ddl.createFunction('fn', { security: 'definer' })).toThrow(/HMAC required/);
    expect(() => ddl.createFunction('fn', { secret_grants: ['TOKEN'] })).toThrow(/HMAC required/);
  });
  it('dropFunction accepts optional hmac and existence flag', () => {
    expect(() => ddl.dropFunction('my_fn')).toThrow(/HMAC required/);
    expect(ddl.dropFunction('my_fn', { if_exists: true, hmac: 'tag' })).toEqual({ drop_function: 'my_fn', if_exists: true, hmac: 'tag' });
  });

  it('renameFunction emits {rename_function, to}', () => {
    expect(ddl.renameFunction('old', 'new')).toEqual({
      rename_function: 'old',
      to: 'new',
    });
  });
});

// ── validator DDL ───────────────────────────────────────────────────

describe('validator DDL', () => {
  it('createValidator — replace always present', () => {
    const op = ddl.createValidator('v_age', { source: 'fn …' });
    expect(op).toEqual({
      create_validator: 'v_age',
      source: 'fn …',
      replace: false,
    });
  });

  it('createValidator replacement requires and carries hmac', () => {
    expect(() => ddl.createValidator('v', { replace: true })).toThrow(/HMAC required/);
    expect(ddl.createValidator('v', { replace: true, hmac: 'tag' }).hmac).toBe('tag');
  });

  it('dropValidator accepts optional hmac', () => {
    expect(() => ddl.dropValidator('v')).toThrow(/HMAC required/);
    expect(ddl.dropValidator('v', { hmac: 'tag' })).toEqual({ drop_validator: 'v', hmac: 'tag' });
  });

  it('renameValidator emits {rename_validator, to}', () => {
    expect(ddl.renameValidator('a', 'b')).toEqual({
      rename_validator: 'a',
      to: 'b',
    });
  });
});

// ── bindValidator / unbindValidator / listValidators ─────────────────

describe('bindValidator', () => {
  it('emits {bind_validator, db, repo, table, ops, priority}', () => {
    const op = ddl.bindValidator('v_age', 'users', ['insert', 'update'], 1500, {
      db: 'testdb',
    });
    expect(op).toEqual({
      bind_validator: 'v_age',
      db: 'testdb',
      repo: 'main',
      table: 'users',
      ops: ['insert', 'update'],
      priority: 1500,
    });
  });

  it('all four write-op kinds serialize correctly', () => {
    const op = ddl.bindValidator('v', 't', ['insert', 'update', 'upsert', 'delete'], 100, {
      db: 'db',
      repo: 'r',
    });
    expect(op.ops).toEqual(['insert', 'update', 'upsert', 'delete']);
  });
});

describe('unbindValidator', () => {
  it('emits {unbind_validator, db, repo, table}', () => {
    const op = ddl.unbindValidator('v_age', {
      db: 'testdb',
      table: 'users',
    });
    expect(op).toEqual({
      unbind_validator: 'v_age',
      db: 'testdb',
      repo: 'main',
      table: 'users',
    });
  });
});

describe('listValidators (DDL)', () => {
  it('emits {list_validators, db, repo}', () => {
    const op = ddl.listValidators('users', { db: 'testdb' });
    expect(op).toEqual({
      list_validators: 'users',
      db: 'testdb',
      repo: 'main',
    });
  });
});

// ── createFunctionFolder ────────────────────────────────────────────

describe('createFunctionFolder', () => {
  it('emits {create_function_folder: string[]}', () => {
    expect(ddl.createFunctionFolder(['reports', 'daily'])).toEqual({
      create_function_folder: ['reports', 'daily'],
    });
  });
});

// ── renameFunctionFolder ───────────────────────────────────────────

describe('renameFunctionFolder', () => {
  it('emits {rename_function_folder: string[], to: string[]}', () => {
    expect(ddl.renameFunctionFolder(['a', 'b'], ['a', 'c'])).toEqual({
      rename_function_folder: ['a', 'b'],
      to: ['a', 'c'],
    });
  });
});

// ── list ops ────────────────────────────────────────────────────────

describe('list ops', () => {
  it('listDatabases → {list: "databases"}', () => {
    expect(ddl.listDatabases()).toEqual({ list: 'databases' });
  });

  it('listRepos → {list: "repos"}', () => {
    expect(ddl.listRepos()).toEqual({ list: 'repos' });
  });

  it('listTables → {list: "tables", repo}', () => {
    expect(ddl.listTables()).toEqual({ list: 'tables', repo: 'main' });
    expect(ddl.listTables({ repo: 'hot' })).toEqual({
      list: 'tables',
      repo: 'hot',
    });
  });

  it('listIndexes → {list: "indexes", table, repo}', () => {
    expect(ddl.listIndexes('users')).toEqual({
      list: 'indexes',
      table: 'users',
      repo: 'main',
    });
  });

  it('listUsers → {list: "users"}', () => {
    expect(ddl.listUsers()).toEqual({ list: 'users' });
  });

  it('listFunctions without folder', () => {
    expect(ddl.listFunctions()).toEqual({ list: 'functions' });
  });

  it('listFunctions with folder filter', () => {
    expect(ddl.listFunctions({ folder: 'reports' })).toEqual({
      list: 'functions',
      folder: 'reports',
    });
  });

  it('listValidators_ → {list: "validators"}', () => {
    expect(ddl.listValidators_()).toEqual({ list: 'validators' });
  });

  it('listFunctionFolders without parent', () => {
    expect(ddl.listFunctionFolders()).toEqual({ list: 'function_folders' });
  });

  it('listFunctionFolders with parent filter', () => {
    expect(ddl.listFunctionFolders({ parent: 'reports' })).toEqual({
      list: 'function_folders',
      parent: 'reports',
    });
  });
});

// ── field() fluent builder ──────────────────────────────────────────
