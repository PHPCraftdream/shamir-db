/**
 * DDL-builder wire-shape tests.
 *
 * The authority for every shape is
 * `crates/shamir-query-types/src/admin/types.rs` (serde: skip_serializing_if,
 * default, default_repo, rename_all) cross-checked with e2e test helpers
 * `tests/e2e/helpers/hmac.js` and `tests/e2e/tests/08-admin-ddl.test.js`.
 */

import { describe, it, expect } from 'vitest';
import { ddl } from '../ddl.js';
import {
  canonicalDropDb,
  canonicalDropRepo,
  canonicalDropTable,
  canonicalDropIndex,
  canonicalStartMigration,
  canonicalCommitMigration,
  canonicalRollbackMigration,
} from '../../hmac.js';

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

  it('unique=true + sorted=true + explicit repo', () => {
    const op = ddl.createIndex('idx_name', 'users', [['name']], {
      unique: true,
      sorted: true,
      repo: 'analytics',
    });
    expect(op.unique).toBe(true);
    expect(op.sorted).toBe(true);
    expect(op.repo).toBe('analytics');
  });

  it('FTS options omitted when not set', () => {
    const op = ddl.createIndex('idx_fts', 'docs', [['body']]);
    expect(op).not.toHaveProperty('index_type');
    expect(op).not.toHaveProperty('fts_tokenizer');
    expect(op).not.toHaveProperty('fts_language');
    expect(op).not.toHaveProperty('functional_op');
    expect(op).not.toHaveProperty('vector_dim');
    expect(op).not.toHaveProperty('include');
    expect(op).not.toHaveProperty('if_not_exists');
  });

  it('includes index_type, vector options, include when set', () => {
    const op = ddl.createIndex('vidx', 'docs', [['embedding']], {
      index_type: 'vector',
      vector_dim: 128,
      vector_metric: 'cosine',
      include: [['metadata']],
      if_not_exists: true,
    });
    expect(op.index_type).toBe('vector');
    expect(op.vector_dim).toBe(128);
    expect(op.vector_metric).toBe('cosine');
    expect(op.include).toEqual([['metadata']]);
    expect(op.if_not_exists).toBe(true);
  });

  it('omits empty include array', () => {
    const op = ddl.createIndex('idx', 't', [['f']], { include: [] });
    expect(op).not.toHaveProperty('include');
  });
});

// ── buffer config ops ───────────────────────────────────────────────

describe('buffer config', () => {
  const cfg: import('../../types/ddl.js').BufferConfigDto = {
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

describe('purgeHistory', () => {
  it('emits {purge_history, repo, scope}', () => {
    const op = ddl.purgeHistory('users', ddl.olderThanAge(86400));
    expect(op).toEqual({
      purge_history: 'users',
      repo: 'main',
      scope: { older_than_age: { age_secs: 86400 } },
    });
  });
});

describe('setRetention', () => {
  it('emits {set_retention, repo, retention}', () => {
    const op = ddl.setRetention('users', ddl.currentOnly());
    expect(op).toEqual({
      set_retention: 'users',
      repo: 'main',
      retention: { max_count: 0 },
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
    const op = ddl.createFunction('my_fn', { wasm: '<base64>', replace: true });
    expect(op.replace).toBe(true);
    expect(op.wasm).toBe('<base64>');
    expect(op).not.toHaveProperty('source');
  });

  it('dropFunction emits {drop_function}', () => {
    expect(ddl.dropFunction('my_fn')).toEqual({ drop_function: 'my_fn' });
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

  it('dropValidator emits {drop_validator}', () => {
    expect(ddl.dropValidator('v')).toEqual({ drop_validator: 'v' });
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

  it('listRoles → {list: "roles"}', () => {
    expect(ddl.listRoles()).toEqual({ list: 'roles' });
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

describe('field()', () => {
  it('builds a string field with max + required', () => {
    const rule = ddl.field(['email']).string().max(255).required().build();
    expect(rule).toEqual({
      path: ['email'],
      type: 'string',
      max: 255,
      required: true,
    });
  });

  it('builds an int field with min + max', () => {
    const rule = ddl.field(['age']).int().min(0).max(150).build();
    expect(rule).toEqual({
      path: ['age'],
      type: 'int',
      min: 0,
      max: 150,
    });
  });

  it('builds a nested-path string field with len', () => {
    const rule = ddl.field(['address', 'zip']).string().len(5).build();
    expect(rule).toEqual({
      path: ['address', 'zip'],
      type: 'string',
      len: 5,
    });
  });

  it('omits undefined constraint keys', () => {
    const rule = ddl.field(['x']).bool().build();
    expect(rule).toEqual({
      path: ['x'],
      type: 'bool',
    });
    expect(rule).not.toHaveProperty('required');
    expect(rule).not.toHaveProperty('min');
    expect(rule).not.toHaveProperty('max');
    expect(rule).not.toHaveProperty('len');
  });

  it('supports all type tags', () => {
    expect(ddl.field(['a']).f64().build().type).toBe('f64');
    expect(ddl.field(['a']).dec().build().type).toBe('dec');
    expect(ddl.field(['a']).bin().build().type).toBe('bin');
    expect(ddl.field(['a']).list().build().type).toBe('list');
    expect(ddl.field(['a']).map().build().type).toBe('map');
    expect(ddl.field(['a']).any().build().type).toBe('any');
    expect(ddl.field(['a']).typeTag('custom').build().type).toBe('custom');
  });

  it('supports nullable, unsigned, minLen, maxLen, arrayOf', () => {
    const rule = ddl.field(['tags']).list().nullable().arrayOf('string').minLen(1).maxLen(10).build();
    expect(rule).toEqual({
      path: ['tags'],
      type: 'list',
      nullable: true,
      array_of: 'string',
      min_len: 1,
      max_len: 10,
    });
  });
});

// ── schema DDL ops ─────────────────────────────────────────────────

describe('setTableSchema', () => {
  it('emits {set_table_schema, repo, schema}; expected_version omitted when absent', () => {
    const schema = [
      ddl.field(['email']).string().max(255).required().build(),
    ];
    const op = ddl.setTableSchema('users', schema);
    expect(op).toEqual({
      set_table_schema: 'users',
      repo: 'main',
      schema: [{ path: ['email'], type: 'string', max: 255, required: true }],
    });
    expect(op).not.toHaveProperty('expected_version');
  });

  it('includes expected_version when set', () => {
    const op = ddl.setTableSchema('users', [], { expectedVersion: 3 });
    expect(op.expected_version).toBe(3);
  });

  it('respects explicit repo', () => {
    const op = ddl.setTableSchema('users', [], { repo: 'hot' });
    expect(op.repo).toBe('hot');
  });
});

describe('addSchemaRule', () => {
  it('emits {add_schema_rule, repo, rule}', () => {
    const rule = ddl.field(['nickname']).string().max(64).build();
    const op = ddl.addSchemaRule('users', rule);
    expect(op).toEqual({
      add_schema_rule: 'users',
      repo: 'main',
      rule: { path: ['nickname'], type: 'string', max: 64 },
    });
  });
});

describe('removeSchemaRule', () => {
  it('emits {remove_schema_rule, repo, path}', () => {
    const op = ddl.removeSchemaRule('users', ['nickname']);
    expect(op).toEqual({
      remove_schema_rule: 'users',
      repo: 'main',
      path: ['nickname'],
    });
  });

  it('respects explicit repo', () => {
    const op = ddl.removeSchemaRule('users', ['email'], { repo: 'cold' });
    expect(op.repo).toBe('cold');
  });
});

describe('getTableSchema', () => {
  it('emits {get_table_schema, repo}', () => {
    const op = ddl.getTableSchema('users');
    expect(op).toEqual({
      get_table_schema: 'users',
      repo: 'main',
    });
  });
});

describe('createTable with schema', () => {
  it('emits schema when provided', () => {
    const schema = [
      ddl.field(['email']).string().required().build(),
      ddl.field(['age']).int().min(0).max(150).build(),
    ];
    const op = ddl.createTable('users', { schema });
    expect(op.schema).toEqual([
      { path: ['email'], type: 'string', required: true },
      { path: ['age'], type: 'int', min: 0, max: 150 },
    ]);
  });

  it('omits schema when empty array', () => {
    const op = ddl.createTable('users', { schema: [] });
    expect(op).not.toHaveProperty('schema');
  });

  it('omits schema when not provided', () => {
    const op = ddl.createTable('users');
    expect(op).not.toHaveProperty('schema');
  });
});

// ── HMAC-gated ops ──────────────────────────────────────────────────

describe('HMAC-gated ops', () => {
  it('dropDb — hmac from canonicalDropDb(db)', () => {
    const canonical = canonicalDropDb('testdb');
    const op = ddl.dropDb(fakeSigner, 'testdb');
    expect(op).toEqual({
      drop_db: 'testdb',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
    expect(op).not.toHaveProperty('cascade');
  });

  it('dropDb with cascade', () => {
    const op = ddl.dropDb(fakeSigner, 'testdb', { cascade: true });
    expect(op.cascade).toBe(true);
  });

  it('dropDb cascade=false omits cascade', () => {
    const op = ddl.dropDb(fakeSigner, 'testdb', { cascade: false });
    expect(op).not.toHaveProperty('cascade');
  });

  it('dropRepo — hmac from canonicalDropRepo(dbInUse, repo)', () => {
    const canonical = canonicalDropRepo('mydb', 'main');
    const op = ddl.dropRepo(fakeSigner, 'mydb', 'main');
    expect(op).toEqual({
      drop_repo: 'main',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('dropRepo with cascade', () => {
    const op = ddl.dropRepo(fakeSigner, 'mydb', 'main', { cascade: true });
    expect(op.cascade).toBe(true);
  });

  it('dropTable — hmac from canonicalDropTable', () => {
    const canonical = canonicalDropTable('mydb', 'main', 'users');
    const op = ddl.dropTable(fakeSigner, 'mydb', 'main', 'users');
    expect(op).toEqual({
      drop_table: 'users',
      repo: 'main',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('dropIndex — hmac from canonicalDropIndex; unique omitted when false', () => {
    const canonical = canonicalDropIndex('mydb', 'main', 'users', 'idx_email', false);
    const op = ddl.dropIndex(fakeSigner, 'mydb', 'main', 'users', 'idx_email');
    expect(op).toEqual({
      drop_index: 'idx_email',
      table: 'users',
      repo: 'main',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
    expect(op).not.toHaveProperty('unique');
  });

  it('dropIndex with unique=true — canonical uses 1, wire includes unique', () => {
    const canonical = canonicalDropIndex('mydb', 'main', 'users', 'idx_pk', true);
    const op = ddl.dropIndex(fakeSigner, 'mydb', 'main', 'users', 'idx_pk', {
      unique: true,
    });
    expect(op.unique).toBe(true);
    expect(op.hmac).toBe(fakeSigner.hmacTagHex(canonical));
  });

  it('startMigration — hmac from canonicalStartMigration', () => {
    const canonical = canonicalStartMigration(
      'mydb', 'main', 'users', 'archive', 'log',
    );
    const op = ddl.startMigration(
      fakeSigner, 'mydb', 'main', 'users', 'archive', 'log',
    );
    expect(op).toEqual({
      start_migration: 'users',
      repo: 'main',
      dst_repo: 'archive',
      dst_engine: 'log',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
    expect(op).not.toHaveProperty('dst_path');
  });

  it('startMigration with dst_path', () => {
    const op = ddl.startMigration(
      fakeSigner, 'mydb', 'main', 'users', 'archive', 'log',
      { dst_path: '/data/archive' },
    );
    expect(op.dst_path).toBe('/data/archive');
  });

  it('commitMigration — hmac from canonicalCommitMigration', () => {
    const canonical = canonicalCommitMigration('mydb', 'm123');
    const op = ddl.commitMigration(fakeSigner, 'mydb', 'm123');
    expect(op).toEqual({
      commit_migration: 'm123',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });

  it('rollbackMigration — hmac from canonicalRollbackMigration', () => {
    const canonical = canonicalRollbackMigration('mydb', 'm123');
    const op = ddl.rollbackMigration(fakeSigner, 'mydb', 'm123');
    expect(op).toEqual({
      rollback_migration: 'm123',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
  });
});
