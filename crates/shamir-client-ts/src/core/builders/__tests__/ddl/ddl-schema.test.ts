import { describe, it, expect } from 'vitest';
import { ddl } from '../../ddl.js';
import { canonicalDropDb, canonicalDropRepo, canonicalDropTable, canonicalDropIndex, canonicalStartMigration, canonicalCommitMigration, canonicalRollbackMigration, canonicalSetRetention, canonicalPurgeHistory } from '../../../hmac.js';

const fakeSigner = { hmacTagHex: (c: Uint8Array): string => 'tag:' + c.length };

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

// ── field().default() (③.2c: literal and expression defaults) ───────
// INSERT-path stamp-enforcement: literals → apply_defaults fast path (②.4c);
// expressions (ComputedExpr/$fn/$ref) → apply_transforms (③.2c).
// Here we only assert the wire-shape (mirrors serde `skip_serializing_if`).

describe('field().default()', () => {
  // ── Literal defaults (②.4b regression — must remain green) ──────────

  it('emits default: <number> on the wire', () => {
    const rule = ddl.field(['count']).int().default(5).build();
    expect(rule.default).toBe(5);
    expect(rule).toEqual({
      path: ['count'],
      type: 'int',
      default: 5,
    });
  });

  it('emits default: <string> on the wire (any WireValue)', () => {
    const rule = ddl.field(['role']).string().default('guest').build();
    expect(rule.default).toBe('guest');
  });

  it('omits default when not set', () => {
    const rule = ddl.field(['x']).int().build();
    expect(rule).not.toHaveProperty('default');
  });

  // ── Expression defaults (③.2c) ────────────────────────────────────────

  it('emits default: {$fn: "strings/upper"} for simple $fn expression', () => {
    const rule = ddl.field(['tag']).string().default({ $fn: 'strings/upper' }).build();
    expect(rule.default).toEqual({ $fn: 'strings/upper' });
    expect(rule).toEqual({
      path: ['tag'],
      type: 'string',
      default: { $fn: 'strings/upper' },
    });
  });

  it('emits default: {$fn: {name, args}} for complex $fn expression', () => {
    const expr = { $fn: { name: 'strings/upper', args: [{ $ref: ['value'] }] } };
    const rule = ddl.field(['tag']).string().default(expr).build();
    expect(rule.default).toEqual(expr);
  });

  it('emits default: {$ref: [field]} for field-reference expression', () => {
    const rule = ddl.field(['tag']).string().default({ $ref: ['source'] }).build();
    expect(rule.default).toEqual({ $ref: ['source'] });
  });
});

// ── foreignKey (on_delete / on_update) ──────────────────────────────

describe('field().foreignKey()', () => {
  it('defaults on_delete to "restrict"', () => {
    const rule = ddl.field(['parent_id']).int().foreignKey('parent', 'id').build();
    expect(rule.foreign_key).toEqual({
      ref_table: 'parent',
      ref_field: 'id',
      on_delete: 'restrict',
    });
  });

  it('emits on_delete: "cascade" when explicitly set', () => {
    const rule = ddl
      .field(['parent_id'])
      .int()
      .foreignKey('parent', 'id', { onDelete: 'cascade' })
      .build();
    expect(rule.foreign_key).toEqual({
      ref_table: 'parent',
      ref_field: 'id',
      on_delete: 'cascade',
    });
  });

  // ── Phase ②.2a — on_update (surface only) ────────────────────────────

  it('omits on_update when not set (default no_action)', () => {
    const rule = ddl.field(['parent_id']).int().foreignKey('parent', 'id').build();
    expect(rule.foreign_key).toBeDefined();
    expect(rule.foreign_key!.on_update).toBeUndefined();
  });

  it('emits on_update: "cascade" when explicitly set (and on_delete defaults to "restrict")', () => {
    const rule = ddl
      .field(['parent_id'])
      .int()
      .foreignKey('parent', 'id', { onUpdate: 'cascade' })
      .build();
    expect(rule.foreign_key).toEqual({
      ref_table: 'parent',
      ref_field: 'id',
      on_delete: 'restrict',
      on_update: 'cascade',
    });
  });

  it('emits both on_delete and on_update when both are set', () => {
    const rule = ddl
      .field(['parent_id'])
      .int()
      .foreignKey('parent', 'id', {
        onDelete: 'set_null',
        onUpdate: 'cascade',
      })
      .build();
    expect(rule.foreign_key).toEqual({
      ref_table: 'parent',
      ref_field: 'id',
      on_delete: 'set_null',
      on_update: 'cascade',
    });
  });

  it('omits on_update when explicitly set to "no_action"', () => {
    const rule = ddl
      .field(['parent_id'])
      .int()
      .foreignKey('parent', 'id', { onUpdate: 'no_action' })
      .build();
    expect(rule.foreign_key).toEqual({
      ref_table: 'parent',
      ref_field: 'id',
      on_delete: 'restrict',
    });
    expect(rule.foreign_key!.on_update).toBeUndefined();
  });
});

// ── field() Phase B/C constraint wire-shapes ────────────────────────
//
// Covers the six field constraints that were previously only exercised by
// server-gated e2e tests: scalar / one_of / format / compare / unique /
// foreign_key (full on_delete matrix). Each test asserts the flattened wire
// shape of `FieldRuleDto` (constraints are `#[serde(flatten)]`).

describe('field() Phase B/C constraint wire-shapes', () => {
  it('scalar(name) flattens to {scalar: name} on the rule', () => {
    const rule = ddl.field(['email']).string().scalar('is_email').build();
    expect(rule).toEqual({
      path: ['email'],
      type: 'string',
      scalar: 'is_email',
    });
  });

  it('oneOf(values) flattens to {one_of: [...]} on the rule', () => {
    const rule = ddl
      .field(['status'])
      .string()
      .oneOf(['draft', 'published', 'archived'])
      .build();
    expect(rule).toEqual({
      path: ['status'],
      type: 'string',
      one_of: ['draft', 'published', 'archived'],
    });
  });

  it('format(kind) flattens to {format: kind} on the rule', () => {
    const rule = ddl.field(['email']).string().format('email').build();
    expect(rule).toEqual({
      path: ['email'],
      type: 'string',
      format: 'email',
    });
  });

  it('compare(other, op) flattens to {compare: {other, op}} on the rule', () => {
    const rule = ddl
      .field(['end_date'])
      .string()
      .compare(['start_date'], '>=')
      .build();
    expect(rule).toEqual({
      path: ['end_date'],
      type: 'string',
      compare: {
        other: ['start_date'],
        op: '>=',
      },
    });
  });

  it('unique() flattens to {unique: true} on the rule', () => {
    const rule = ddl.field(['email']).string().unique().build();
    expect(rule).toEqual({
      path: ['email'],
      type: 'string',
      unique: true,
    });
  });

  it('foreignKey on_delete matrix: restrict (default) / cascade / set_null / no_action', () => {
    // default → restrict
    const restrict = ddl.field(['owner_id']).int().foreignKey('users', 'id').build();
    expect(restrict.foreign_key).toEqual({
      ref_table: 'users',
      ref_field: 'id',
      on_delete: 'restrict',
    });

    // explicit cascade
    const cascade = ddl
      .field(['owner_id'])
      .int()
      .foreignKey('users', 'id', { onDelete: 'cascade' })
      .build();
    expect(cascade.foreign_key).toEqual({
      ref_table: 'users',
      ref_field: 'id',
      on_delete: 'cascade',
    });

    // explicit set_null
    const setNull = ddl
      .field(['owner_id'])
      .int()
      .foreignKey('users', 'id', { onDelete: 'set_null' })
      .build();
    expect(setNull.foreign_key).toEqual({
      ref_table: 'users',
      ref_field: 'id',
      on_delete: 'set_null',
    });

    // explicit no_action
    const noAction = ddl
      .field(['owner_id'])
      .int()
      .foreignKey('users', 'id', { onDelete: 'no_action' })
      .build();
    expect(noAction.foreign_key).toEqual({
      ref_table: 'users',
      ref_field: 'id',
      on_delete: 'no_action',
    });
  });

  it('constraints are omitted entirely when no Phase B/C setter is called', () => {
    const rule = ddl.field(['name']).string().max(64).build();
    expect(rule).toEqual({
      path: ['name'],
      type: 'string',
      max: 64,
    });
    expect(rule).not.toHaveProperty('scalar');
    expect(rule).not.toHaveProperty('one_of');
    expect(rule).not.toHaveProperty('format');
    expect(rule).not.toHaveProperty('compare');
    expect(rule).not.toHaveProperty('unique');
    expect(rule).not.toHaveProperty('foreign_key');
    expect(rule).not.toHaveProperty('auto_now');
    expect(rule).not.toHaveProperty('auto_now_add');
  });
});

// ── ③.2d server-stamping builders ──────────────────────────────────

describe('FieldBuilder — ③.2d autoNow / autoNowAdd', () => {
  it('autoNow() flattens to {auto_now: true} on the rule', () => {
    const rule = ddl.field(['updated_at']).int().autoNow().build();
    expect(rule).toEqual({
      path: ['updated_at'],
      type: 'int',
      auto_now: true,
    });
  });

  it('autoNowAdd() flattens to {auto_now_add: true} on the rule', () => {
    const rule = ddl.field(['created_at']).int().autoNowAdd().build();
    expect(rule).toEqual({
      path: ['created_at'],
      type: 'int',
      auto_now_add: true,
    });
  });

  it('auto_now is omitted when not set', () => {
    const rule = ddl.field(['created_at']).int().build();
    expect(rule).not.toHaveProperty('auto_now');
  });

  it('auto_now_add is omitted when not set', () => {
    const rule = ddl.field(['updated_at']).int().build();
    expect(rule).not.toHaveProperty('auto_now_add');
  });

  it('autoNow and autoNowAdd can coexist on different fields in a schema', () => {
    const schema = [
      ddl.field(['created_at']).int().autoNowAdd().build(),
      ddl.field(['updated_at']).int().autoNow().build(),
    ];
    expect(schema[0]).toEqual({
      path: ['created_at'],
      type: 'int',
      auto_now_add: true,
    });
    expect(schema[1]).toEqual({
      path: ['updated_at'],
      type: 'int',
      auto_now: true,
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

  // ── expectedVersion: number | bigint CAS guard (CR-B6) ─────────────

  it('accepts a bigint expectedVersion and forwards it unmodified', () => {
    const bigVersion = 2n ** 60n;
    const op = ddl.setTableSchema('users', [], { expectedVersion: bigVersion });
    expect(op.expected_version).toBe(bigVersion);
    expect(typeof op.expected_version).toBe('bigint');
  });

  it('throws TypeError on an unsafe number expectedVersion', () => {
    expect(() =>
      ddl.setTableSchema('users', [], {
        expectedVersion: Number.MAX_SAFE_INTEGER + 2,
      }),
    ).toThrow(TypeError);
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

  it('dropIndex — hmac from canonicalDropIndex', () => {
    const canonical = canonicalDropIndex('mydb', 'main', 'users', 'idx_email');
    const op = ddl.dropIndex(fakeSigner, 'mydb', 'main', 'users', 'idx_email');
    expect(op).toEqual({
      drop_index: 'idx_email',
      table: 'users',
      repo: 'main',
      hmac: fakeSigner.hmacTagHex(canonical),
    });
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

// ── if_exists on drop ops ──────────────────────────────────────────

describe('if_exists on drop ops', () => {
  it('dropFunction omits if_exists when not set', () => {
    const op = ddl.dropFunction('my_fn');
    expect(op).not.toHaveProperty('if_exists');
  });

  it('dropFunction emits if_exists when true', () => {
    const op = ddl.dropFunction('my_fn', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropValidator omits if_exists when not set', () => {
    const op = ddl.dropValidator('v');
    expect(op).not.toHaveProperty('if_exists');
  });

  it('dropValidator emits if_exists when true', () => {
    const op = ddl.dropValidator('v', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropDb emits if_exists when true', () => {
    const op = ddl.dropDb(fakeSigner, 'testdb', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropDb omits if_exists when not set', () => {
    const op = ddl.dropDb(fakeSigner, 'testdb');
    expect(op).not.toHaveProperty('if_exists');
  });

  it('dropRepo emits if_exists when true', () => {
    const op = ddl.dropRepo(fakeSigner, 'mydb', 'main', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropTable emits if_exists when true', () => {
    const op = ddl.dropTable(fakeSigner, 'mydb', 'main', 'users', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropTable omits if_exists when not set', () => {
    const op = ddl.dropTable(fakeSigner, 'mydb', 'main', 'users');
    expect(op).not.toHaveProperty('if_exists');
  });

  it('dropTable emits cascade when true', () => {
    const op = ddl.dropTable(fakeSigner, 'mydb', 'main', 'users', { cascade: true });
    expect(op.cascade).toBe(true);
  });

  it('dropTable omits cascade when not set', () => {
    const op = ddl.dropTable(fakeSigner, 'mydb', 'main', 'users');
    expect(op).not.toHaveProperty('cascade');
  });

  it('dropIndex emits if_exists when true', () => {
    const op = ddl.dropIndex(fakeSigner, 'mydb', 'main', 'users', 'idx', { if_exists: true });
    expect(op.if_exists).toBe(true);
  });

  it('dropIndex omits if_exists when not set', () => {
    const op = ddl.dropIndex(fakeSigner, 'mydb', 'main', 'users', 'idx');
    expect(op).not.toHaveProperty('if_exists');
  });
});

// ── renameTable / renameIndex ──────────────────────────────────────

describe('renameTable', () => {
  it('emits {rename_table, to}; repo omitted when not set', () => {
    const op = ddl.renameTable('users', 'people');
    expect(op).toEqual({
      rename_table: 'users',
      to: 'people',
    });
    expect(op).not.toHaveProperty('repo');
  });

  it('includes repo when explicitly set', () => {
    const op = ddl.renameTable('users', 'people', { repo: 'analytics' });
    expect(op.repo).toBe('analytics');
  });
});

describe('renameIndex', () => {
  it('emits {rename_index, to, table}; repo omitted when not set', () => {
    const op = ddl.renameIndex('users', 'idx_email', 'idx_mail');
    expect(op).toEqual({
      rename_index: 'idx_email',
      to: 'idx_mail',
      table: 'users',
    });
    expect(op).not.toHaveProperty('repo');
    expect(op).not.toHaveProperty('if_exists');
  });

  it('includes repo when explicitly set', () => {
    const op = ddl.renameIndex('users', 'idx_email', 'idx_mail', {
      repo: 'analytics',
    });
    expect(op.repo).toBe('analytics');
  });

  it('emits if_exists: true when set', () => {
    const op = ddl.renameIndex('users', 'idx_email', 'idx_mail', {
      if_exists: true,
    });
    expect(op.if_exists).toBe(true);
  });

  it('omits if_exists when not set', () => {
    const op = ddl.renameIndex('users', 'idx_email', 'idx_mail');
    expect(op).not.toHaveProperty('if_exists');
  });

  it('omits if_exists when explicitly false', () => {
    const op = ddl.renameIndex('users', 'idx_email', 'idx_mail', {
      if_exists: false,
    });
    expect(op).not.toHaveProperty('if_exists');
  });
});

// ── renameRepo ─────────────────────────────────────────────────────

describe('renameRepo', () => {
  it('emits exactly {rename_repo, to} (2 keys)', () => {
    const op = ddl.renameRepo('analytics', 'telemetry');
    expect(op).toEqual({
      rename_repo: 'analytics',
      to: 'telemetry',
    });
    expect(Object.keys(op)).toHaveLength(2);
  });
});

// ── renameDb ───────────────────────────────────────────────────────

describe('renameDb', () => {
  it('emits exactly {rename_db, to} (2 keys)', () => {
    const op = ddl.renameDb('old_db', 'new_db');
    expect(op).toEqual({
      rename_db: 'old_db',
      to: 'new_db',
    });
    expect(Object.keys(op)).toHaveLength(2);
  });
});

// ── internerDump / internerTouch ────────────────────────────────────

describe('internerDump', () => {
  it('defaults interner_dump to "main"; since omitted', () => {
    const op = ddl.internerDump();
    expect(op).toEqual({ interner_dump: 'main' });
    expect(op).not.toHaveProperty('since');
  });

  it('respects explicit repo + since', () => {
    const op = ddl.internerDump({ repo: 'archive', since: 12 });
    expect(op).toEqual({ interner_dump: 'archive', since: 12 });
  });

  it('omits since when only repo is set', () => {
    const op = ddl.internerDump({ repo: 'x' });
    expect(op).toEqual({ interner_dump: 'x' });
    expect(op).not.toHaveProperty('since');
  });
});

describe('internerTouch', () => {
  it('defaults interner_touch to "main"; passes names through', () => {
    const op = ddl.internerTouch(['age', 'name']);
    expect(op).toEqual({
      interner_touch: 'main',
      names: ['age', 'name'],
    });
  });

  it('respects explicit repo', () => {
    const op = ddl.internerTouch(['age'], { repo: 'archive' });
    expect(op).toEqual({
      interner_touch: 'archive',
      names: ['age'],
    });
  });
});
