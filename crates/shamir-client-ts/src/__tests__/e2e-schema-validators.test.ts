/**
 * End-to-end tests for declarative schema validators — behaviour coverage.
 *
 * Exercises accept/reject + error codes for every constraint family
 * (scalar, format, compare, FK, unique) and rule lifecycle ops
 * against a live shamir-server. Mirrors the Rust e2e suites:
 *   declarative_schema_e2e.rs        (Phase A scalars)
 *   declarative_schema_ddl_e2e.rs    (DDL round-trip + reopen)
 *   declarative_schema_fk_e2e.rs     (Phase C2 FK)
 *   declarative_schema_unique_e2e.rs (Phase C3 unique)
 *   declarative_schema_relational_e2e.rs (combined FK+unique)
 *
 * TX PATH NOTE (Phase C):
 *   At the Rust engine level, FK and unique constraints require ctx.db = Some
 *   (transactional path). The Rust e2e tests use `b.transactional()` for this.
 *   At the server wire level, the server wraps all batch execution in a tx
 *   context, so FK/unique constraints fire even in autocommit batches.
 *   The FK/unique tests below use `Batch.transactional()` to mirror the Rust
 *   e2e pattern, but dedicated tests confirm autocommit also enforces them.
 *
 * Own startServer (ephemeral port) — no conflict with other e2e suites.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse, WriteValue } from '../index.js';
import { Batch, ddl, write } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
  setupDb,
  seed,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

// ── helpers ──────────────────────────────────────────────────────────

/**
 * Try an insert via a NON-transactional batch (autocommit path).
 * Returns `{ ok, error }`.
 */
async function tryInsert(
  client: ShamirClient,
  db: string,
  table: string,
  record: WriteValue,
): Promise<{ ok: boolean; error?: string }> {
  try {
    const resp = br(
      await Batch.create('try-ins')
        .add('ins', write.insert(table, record))
        .execute(client, db),
    );
    // Check for transaction abort (should not happen on autocommit, but safety).
    if (resp.transaction && resp.transaction.status === 'aborted') {
      return { ok: false, error: resp.transaction.reason ?? 'aborted' };
    }
    return { ok: true };
  } catch (e: unknown) {
    return { ok: false, error: (e as Error).message };
  }
}

/**
 * Try an insert via a TRANSACTIONAL batch (ctx.db = Some on server).
 * FK/unique constraints fire only in this path.
 */
async function tryInsertTx(
  client: ShamirClient,
  db: string,
  table: string,
  record: WriteValue,
): Promise<{ ok: boolean; error?: string }> {
  try {
    const resp = br(
      await Batch.create('try-ins-tx')
        .transactional()
        .add('ins', write.insert(table, record))
        .execute(client, db),
    );
    if (resp.transaction && resp.transaction.status === 'aborted') {
      return { ok: false, error: resp.transaction.reason ?? 'aborted' };
    }
    return { ok: true };
  } catch (e: unknown) {
    return { ok: false, error: (e as Error).message };
  }
}

/**
 * Insert TWO records in a single transactional batch (two statements).
 * Used to test read-your-own-writes / staged-probe for unique violations.
 */
async function tryInsertTxTwo(
  client: ShamirClient,
  db: string,
  table: string,
  r1: WriteValue,
  r2: WriteValue,
): Promise<{ ok: boolean; error?: string }> {
  try {
    const resp = br(
      await Batch.create('try-ins-tx-2')
        .transactional()
        .add('ins1', write.insert(table, r1))
        .add('ins2', write.insert(table, r2))
        .execute(client, db),
    );
    if (resp.transaction && resp.transaction.status === 'aborted') {
      return { ok: false, error: resp.transaction.reason ?? 'aborted' };
    }
    return { ok: true };
  } catch (e: unknown) {
    return { ok: false, error: (e as Error).message };
  }
}

// ─── test suite ──────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e schema validators — behaviour (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error(
          '[e2e-schema-validators] connection failed. Server logs:\n' +
            server.logs(),
        );
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try { await client.close(); } catch { /* ok */ }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    // ══════════════════════════════════════════════════════════════════
    // 1. SCALAR CONSTRAINTS (Phase A)
    // ══════════════════════════════════════════════════════════════════

    it('required: missing required field -> missing_required', async () => {
      const db = await setupDb(client!, 'sv_req', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['name']).string().required().build(),
        ]))
        .execute(client!, db));

      // Valid
      const ok = await tryInsert(client!, db, 'items', {
        name: 'widget',
      });
      expect(ok.ok).toBe(true);

      // Missing required
      const bad = await tryInsert(client!, db, 'items', {
        other: 'value',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('missing_required');
    });

    it('type_mismatch: string where int expected', async () => {
      const db = await setupDb(client!, 'sv_type', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['age']).int().required().build(),
        ]))
        .execute(client!, db));

      // Valid
      const ok = await tryInsert(client!, db, 'items', { age: 25 });
      expect(ok.ok).toBe(true);

      // Type mismatch
      const bad = await tryInsert(client!, db, 'items', { age: 'twenty-five' });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('type_mismatch');
    });

    it('out_of_range: int min/max boundaries', async () => {
      const db = await setupDb(client!, 'sv_range', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['age']).int().min(0).max(150).required().build(),
        ]))
        .execute(client!, db));

      // At boundaries - accepted
      const okMin = await tryInsert(client!, db, 'items', { age: 0 });
      expect(okMin.ok).toBe(true);

      const okMax = await tryInsert(client!, db, 'items', { age: 150 });
      expect(okMax.ok).toBe(true);

      // Below min
      const low = await tryInsert(client!, db, 'items', { age: -1 });
      expect(low.ok).toBe(false);
      expect(low.error).toContain('out_of_range');

      // Above max
      const high = await tryInsert(client!, db, 'items', { age: 200 });
      expect(high.ok).toBe(false);
      expect(high.error).toContain('out_of_range');
    });

    it('unsigned: negative int -> out_of_range', async () => {
      const db = await setupDb(client!, 'sv_unsigned', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['count']).int().unsigned().required().build(),
        ]))
        .execute(client!, db));

      // Positive - ok
      const ok = await tryInsert(client!, db, 'items', { count: 42 });
      expect(ok.ok).toBe(true);

      // Zero - ok
      const zero = await tryInsert(client!, db, 'items', { count: 0 });
      expect(zero.ok).toBe(true);

      // Negative - rejected
      const neg = await tryInsert(client!, db, 'items', { count: -1 });
      expect(neg.ok).toBe(false);
      expect(neg.error).toContain('out_of_range');
    });

    it('one_of (not_in_enum): value not in allowed set', async () => {
      const db = await setupDb(client!, 'sv_oneof', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['status']).string().oneOf(['active', 'inactive']).required().build(),
        ]))
        .execute(client!, db));

      // Valid
      const ok = await tryInsert(client!, db, 'items', { status: 'active' });
      expect(ok.ok).toBe(true);

      // Invalid
      const bad = await tryInsert(client!, db, 'items', { status: 'deleted' });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('not_in_enum');
    });

    it('len (wrong_length): exact string length', async () => {
      const db = await setupDb(client!, 'sv_len', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['zip']).string().len(5).required().build(),
        ]))
        .execute(client!, db));

      // Exact length - ok
      const ok = await tryInsert(client!, db, 'items', { zip: '12345' });
      expect(ok.ok).toBe(true);

      // Wrong length
      const bad = await tryInsert(client!, db, 'items', { zip: '123' });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('wrong_length');
    });

    it('min_len/max_len (too_short/too_long): string length range', async () => {
      const db = await setupDb(client!, 'sv_strlen', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['code']).string().minLen(2).maxLen(5).required().build(),
        ]))
        .execute(client!, db));

      // Within range
      const ok = await tryInsert(client!, db, 'items', { code: 'abc' });
      expect(ok.ok).toBe(true);

      // Too short
      const short = await tryInsert(client!, db, 'items', { code: 'a' });
      expect(short.ok).toBe(false);
      expect(short.error).toContain('too_short');

      // Too long
      const long = await tryInsert(client!, db, 'items', { code: 'abcdef' });
      expect(long.ok).toBe(false);
      expect(long.error).toContain('too_long');
    });

    it('nullable: null accepted when nullable, rejected when not', async () => {
      const db = await setupDb(client!, 'sv_null', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['bio']).string().required().nullable().build(),
          ddl.field(['email']).string().required().build(),
        ]))
        .execute(client!, db));

      // Null on nullable field - ok
      const ok = await tryInsert(client!, db, 'items', {
        bio: null,
        email: 'a@b.com',
      });
      expect(ok.ok).toBe(true);

      // Null on non-nullable field
      const bad = await tryInsert(client!, db, 'items', {
        bio: 'hello',
        email: null,
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('null_not_allowed');
    });

    it('nested path validation', async () => {
      const db = await setupDb(client!, 'sv_nested', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['address', 'zip']).string().len(5).required().build(),
        ]))
        .execute(client!, db));

      // Valid nested
      const ok = await tryInsert(client!, db, 'items', {
        address: { zip: '12345' },
      });
      expect(ok.ok).toBe(true);

      // Invalid nested
      const bad = await tryInsert(client!, db, 'items', {
        address: { zip: '123' },
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('wrong_length');
    });

    it('optional field absent is accepted', async () => {
      const db = await setupDb(client!, 'sv_opt', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['age']).int().min(0).max(150).build(),
        ]))
        .execute(client!, db));

      // age is optional, absent -> accepted
      const ok = await tryInsert(client!, db, 'items', { name: 'Alice' });
      expect(ok.ok).toBe(true);
    });

    // ══════════════════════════════════════════════════════════════════
    // 2. FORMAT CONSTRAINTS (Phase B)
    // ══════════════════════════════════════════════════════════════════

    it('format(email): valid/invalid -> bad_format', async () => {
      const db = await setupDb(client!, 'sv_fmt_email', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['email']).string().format('email').required().build(),
        ]))
        .execute(client!, db));

      const ok = await tryInsert(client!, db, 'items', {
        email: 'alice@example.com',
      });
      expect(ok.ok).toBe(true);

      const bad = await tryInsert(client!, db, 'items', {
        email: 'not-an-email',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('bad_format');
    });

    it('format(uuid): valid/invalid -> bad_format', async () => {
      const db = await setupDb(client!, 'sv_fmt_uuid', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['device_id']).string().format('uuid').required().build(),
        ]))
        .execute(client!, db));

      const ok = await tryInsert(client!, db, 'items', {
        device_id: '550e8400-e29b-41d4-a716-446655440000',
      });
      expect(ok.ok).toBe(true);

      const bad = await tryInsert(client!, db, 'items', {
        device_id: 'not-a-uuid',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('bad_format');
    });

    it('format(date): valid/invalid -> bad_format', async () => {
      const db = await setupDb(client!, 'sv_fmt_date', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['created_at']).string().format('date').required().build(),
        ]))
        .execute(client!, db));

      // RFC-3339
      const ok1 = await tryInsert(client!, db, 'items', {
        created_at: '2024-01-31T08:30:00Z',
      });
      expect(ok1.ok).toBe(true);

      // Bare calendar date
      const ok2 = await tryInsert(client!, db, 'items', {
        created_at: '2024-01-31',
      });
      expect(ok2.ok).toBe(true);

      // Invalid
      const bad = await tryInsert(client!, db, 'items', {
        created_at: 'hello',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('bad_format');
    });

    it('format(url): valid/invalid -> bad_format', async () => {
      const db = await setupDb(client!, 'sv_fmt_url', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['website']).string().format('url').required().build(),
        ]))
        .execute(client!, db));

      const ok = await tryInsert(client!, db, 'items', {
        website: 'https://example.com',
      });
      expect(ok.ok).toBe(true);

      const bad = await tryInsert(client!, db, 'items', {
        website: 'not a url',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('bad_format');
    });

    // ══════════════════════════════════════════════════════════════════
    // 3. COMPARE (cross-field, Phase B)
    // ══════════════════════════════════════════════════════════════════

    it('compare (end >= start): compare_violation', async () => {
      const db = await setupDb(client!, 'sv_compare', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['start']).int().required().build(),
          ddl.field(['end']).int().required().compare(['start'], '>=').build(),
        ]))
        .execute(client!, db));

      // end >= start -> ok
      const ok = await tryInsert(client!, db, 'items', {
        start: 10,
        end: 20,
      });
      expect(ok.ok).toBe(true);

      // end < start -> rejected
      const bad = await tryInsert(client!, db, 'items', {
        start: 10,
        end: 5,
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('compare_violation');
    });

    it('compare: skipped when other path absent', async () => {
      const db = await setupDb(client!, 'sv_cmp_skip', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['end']).int().compare(['start'], '>=').build(),
        ]))
        .execute(client!, db));

      // Only end present, start absent -> cross-field skipped, accepted
      const ok = await tryInsert(client!, db, 'items', { end: 42 });
      expect(ok.ok).toBe(true);
    });

    // ══════════════════════════════════════════════════════════════════
    // 4. FOREIGN KEY (Phase C2) — requires transactional batch
    // ══════════════════════════════════════════════════════════════════

    it('foreign_key: accept existing ref, reject missing -> fk_violation', async () => {
      const db = uniqueDbName('sv_fk');

      // Create db + repo with two tables
      await client!.execute('default', {
        id: 'setup-fk-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-fk-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('departments', { repo: 'main' }),
          t2: ddl.createTable('employees', { repo: 'main' }),
        },
      });

      // Create index on departments.dept_id (required for FK lookup)
      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('dept_id_idx', 'departments', [['dept_id']]))
        .execute(client!, db));

      // Seed a parent row
      await seed(client!, db, 'departments', [
        {
          dept_id: 100,
          name: 'Engineering',
        },
      ], ['dept_id']);

      // Set FK schema on employees
      br(await Batch.create('set-fk-schema')
        .add('s', ddl.setTableSchema('employees', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['dept_id']).int().required().foreignKey('departments', 'dept_id').build(),
        ]))
        .execute(client!, db));

      // Valid: dept_id=100 exists in parent (transactional path)
      const ok = await tryInsertTx(client!, db, 'employees', {
        name: 'Alice',
        dept_id: 100,
      });
      expect(ok.ok).toBe(true);

      // Invalid: dept_id=999 does NOT exist
      const bad = await tryInsertTx(client!, db, 'employees', {
        name: 'Bob',
        dept_id: 999,
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('fk_violation');
    });

    it('foreign_key: FK without index on parent -> fk_requires_index at DDL time', async () => {
      const db = uniqueDbName('sv_fk_noidx');

      await client!.execute('default', {
        id: 'setup-fk-noidx-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-fk-noidx-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('departments', { repo: 'main' }),
          t2: ddl.createTable('employees', { repo: 'main' }),
        },
      });

      // NO index on departments.dept_id

      // Try to set FK schema -> should fail at DDL time
      try {
        br(await Batch.create('set-fk-schema')
          .add('s', ddl.setTableSchema('employees', [
            ddl.field(['dept_id']).int().required().foreignKey('departments', 'dept_id').build(),
          ]))
          .execute(client!, db));
        expect.fail('should have thrown fk_requires_index');
      } catch (e: unknown) {
        expect((e as Error).message).toContain('fk_requires_index');
      }
    });

    it('foreign_key: autocommit also enforces FK (server wraps in tx context)', async () => {
      // NOTE: The Rust engine-level tests document that FK checks require
      // ctx.db = Some (transactional path). At the server wire level,
      // the server wraps all batch execution in a tx context, so FK
      // constraints fire even in autocommit batches. This test confirms
      // that behavior from the TS client's perspective.
      const db = uniqueDbName('sv_fk_auto');

      await client!.execute('default', {
        id: 'setup-fk-auto-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-fk-auto-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('departments', { repo: 'main' }),
          t2: ddl.createTable('employees', { repo: 'main' }),
        },
      });

      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('dept_id_idx', 'departments', [['dept_id']]))
        .execute(client!, db));

      br(await Batch.create('set-fk-schema')
        .add('s', ddl.setTableSchema('employees', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['dept_id']).int().required().foreignKey('departments', 'dept_id').build(),
        ]))
        .execute(client!, db));

      // Autocommit insert with non-existing FK ref is also rejected
      // (server wraps in tx context at the wire level).
      const auto = await tryInsert(client!, db, 'employees', {
        name: 'Ghost',
        dept_id: 999,
      });
      expect(auto.ok).toBe(false);
      expect(auto.error).toContain('fk_violation');
    });

    // ══════════════════════════════════════════════════════════════════
    // 5. UNIQUE (Phase C3) — requires transactional batch
    // ══════════════════════════════════════════════════════════════════

    it('unique: accept new, reject duplicate -> unique_violation', async () => {
      const db = uniqueDbName('sv_uniq');

      await client!.execute('default', {
        id: 'setup-uniq-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-uniq-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('users', { repo: 'main' }),
        },
      });

      // Create index on users.email (required for unique constraint)
      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('email_idx', 'users', [['email']]))
        .execute(client!, db));

      // Set unique schema
      br(await Batch.create('set-uniq-schema')
        .add('s', ddl.setTableSchema('users', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['email']).string().required().unique().build(),
        ]))
        .execute(client!, db));

      // First insert - ok
      const ok1 = await tryInsertTx(client!, db, 'users', {
        name: 'Alice',
        email: 'alice@test.com',
      });
      expect(ok1.ok).toBe(true);

      // Duplicate email - rejected
      const bad = await tryInsertTx(client!, db, 'users', {
        name: 'Bob',
        email: 'alice@test.com',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('unique_violation');

      // Different email - ok
      const ok2 = await tryInsertTx(client!, db, 'users', {
        name: 'Carol',
        email: 'carol@test.com',
      });
      expect(ok2.ok).toBe(true);
    });

    it('unique: batch-duplicate within single tx (read-your-own-writes)', async () => {
      const db = uniqueDbName('sv_uniq_ryw');

      await client!.execute('default', {
        id: 'setup-ryw-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-ryw-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('users', { repo: 'main' }),
        },
      });

      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('email_idx', 'users', [['email']]))
        .execute(client!, db));

      br(await Batch.create('set-uniq-schema')
        .add('s', ddl.setTableSchema('users', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['email']).string().required().unique().build(),
        ]))
        .execute(client!, db));

      // Two inserts with SAME email in one tx batch -> staged-probe detects dup
      const bad = await tryInsertTxTwo(
        client!,
        db,
        'users',
        {
          name: 'Alice',
          email: 'dup@test.com',
        },
        {
          name: 'Bob',
          email: 'dup@test.com',
        },
      );
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('unique_violation');
    });

    it('unique: without index on field -> unique_requires_index at DDL time', async () => {
      const db = uniqueDbName('sv_uniq_noidx');

      await client!.execute('default', {
        id: 'setup-uniq-noidx-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-uniq-noidx-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('users', { repo: 'main' }),
        },
      });

      // NO index on users.email

      try {
        br(await Batch.create('set-uniq-schema')
          .add('s', ddl.setTableSchema('users', [
            ddl.field(['email']).string().required().unique().build(),
          ]))
          .execute(client!, db));
        expect.fail('should have thrown unique_requires_index');
      } catch (e: unknown) {
        expect((e as Error).message).toContain('unique_requires_index');
      }
    });

    it('unique: autocommit also enforces unique (server wraps in tx context)', async () => {
      // NOTE: The Rust engine-level tests document that unique checks require
      // ctx.db = Some (transactional path). At the server wire level,
      // the server wraps all batch execution in a tx context, so unique
      // constraints fire even in autocommit batches. This test confirms
      // that behavior from the TS client's perspective.
      const db = uniqueDbName('sv_uniq_auto');

      await client!.execute('default', {
        id: 'setup-uniq-auto-db',
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: 'setup-uniq-auto-tables',
        queries: {
          mr: ddl.createRepo('main'),
          t1: ddl.createTable('users', { repo: 'main' }),
        },
      });

      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('email_idx', 'users', [['email']]))
        .execute(client!, db));

      br(await Batch.create('set-uniq-schema')
        .add('s', ddl.setTableSchema('users', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['email']).string().required().unique().build(),
        ]))
        .execute(client!, db));

      // First insert (autocommit)
      const ok1 = await tryInsert(client!, db, 'users', {
        name: 'Alice',
        email: 'dup-auto@test.com',
      });
      expect(ok1.ok).toBe(true);

      // Duplicate via autocommit is also rejected
      // (server wraps in tx context at the wire level).
      const auto = await tryInsert(client!, db, 'users', {
        name: 'Bob',
        email: 'dup-auto@test.com',
      });
      expect(auto.ok).toBe(false);
      expect(auto.error).toContain('unique_violation');
    });

    // ══════════════════════════════════════════════════════════════════
    // 6. RULE LIFECYCLE (add/remove/get)
    // ══════════════════════════════════════════════════════════════════

    it('lifecycle: addSchemaRule starts rejecting, removeSchemaRule stops, getTableSchema reflects', async () => {
      const db = await setupDb(client!, 'sv_lifecycle', ['items']);

      // Initial schema: name required
      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['name']).string().required().build(),
        ]))
        .execute(client!, db));

      // Insert with age=-5 should pass (no age rule yet)
      const before = await tryInsert(client!, db, 'items', {
        name: 'test',
        age: -5,
      });
      expect(before.ok).toBe(true);

      // Add age rule: min=0
      br(await Batch.create('add-rule')
        .add('a', ddl.addSchemaRule('items',
          ddl.field(['age']).int().min(0).max(150).build(),
        ))
        .execute(client!, db));

      // Now age=-5 should be rejected
      const after = await tryInsert(client!, db, 'items', {
        name: 'test2',
        age: -5,
      });
      expect(after.ok).toBe(false);
      expect(after.error).toContain('out_of_range');

      // getTableSchema should show both rules
      const get1 = br(await Batch.create('get-schema')
        .add('g', ddl.getTableSchema('items'))
        .execute(client!, db));
      const schema1 = JSON.stringify(get1.results.g.records[0]);
      expect(schema1).toContain('name');
      expect(schema1).toContain('age');

      // Remove the age rule
      br(await Batch.create('rm-rule')
        .add('r', ddl.removeSchemaRule('items', ['age']))
        .execute(client!, db));

      // age=-5 should pass again
      const removed = await tryInsert(client!, db, 'items', {
        name: 'test3',
        age: -5,
      });
      expect(removed.ok).toBe(true);

      // getTableSchema should show only name rule
      const get2 = br(await Batch.create('get-schema-2')
        .add('g', ddl.getTableSchema('items'))
        .execute(client!, db));
      const schema2 = JSON.stringify(get2.results.g.records[0]);
      expect(schema2).toContain('name');
      expect(schema2).not.toContain('"age"');
    });

    // ══════════════════════════════════════════════════════════════════
    // 7. PERSISTENCE (reconnect — schema still enforced)
    // ══════════════════════════════════════════════════════════════════

    it('persistence: schema survives client reconnect', async () => {
      const db = await setupDb(client!, 'sv_persist', ['items']);

      // Set schema
      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['email']).string().format('email').required().build(),
        ]))
        .execute(client!, db));

      // Valid insert
      const ok = await tryInsert(client!, db, 'items', {
        email: 'alice@example.com',
      });
      expect(ok.ok).toBe(true);

      // Close and reconnect
      await client!.close();
      client = await connectAdmin(HOST, server!.port);

      // Schema should still be enforced after reconnect
      const bad = await tryInsert(client!, db, 'items', {
        email: 'not-an-email',
      });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('bad_format');

      // Valid insert still works
      const ok2 = await tryInsert(client!, db, 'items', {
        email: 'bob@example.com',
      });
      expect(ok2.ok).toBe(true);
    });

    // ══════════════════════════════════════════════════════════════════
    // 8. ERROR ACCUMULATION
    // ══════════════════════════════════════════════════════════════════

    it('multiple rule violations are accumulated in one error', async () => {
      const db = await setupDb(client!, 'sv_accum', ['items']);

      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('items', [
          ddl.field(['email']).string().required().build(),
          ddl.field(['age']).int().required().build(),
          ddl.field(['name']).string().required().build(),
        ]))
        .execute(client!, db));

      // All three required fields missing
      const bad = await tryInsert(client!, db, 'items', { x: 1 });
      expect(bad.ok).toBe(false);
      expect(bad.error).toContain('missing_required');
    });
  },
);

describe('e2e-schema-validators skip reason', () => {
  it('reports why the schema validator e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-schema-validators] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
