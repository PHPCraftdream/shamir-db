/**
 * FG-1 e2e: u64 > i64::MAX round-trip + litU64 filter match.
 *
 * Writes a field whose value is the exact decimal string of `u64::MAX`
 * (the representation `Value::Big` serialises to on the wire), reads it
 * back losslessly, and confirms an `Eq` filter via `litU64(u64::MAX)`
 * matches it through the real server pipeline.
 *
 * NOTE: values stored via the normal write path (as decimal strings, which
 * is how `QueryValue::Big` serialises) round-trip as `msgpack str` bytes.
 * On read-back the lens decodes these as `RecordValue::Str(Cow::Borrowed)`,
 * `scalar_at` returns `ScalarRef::Str`, and `scalar_ref_cmp_qv(Str, Str)`
 * succeeds. (The filter-match gap documented in the engine-level
 * `u64_big_filter_match_tests.rs` applies ONLY to raw `uint64` storage
 * bytes — the exotic case from external non-Rust/non-TS encoders.)
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, Query, filter, write } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
  setupDb,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e u64>max round-trip + litU64 filter (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let dbName: string;

    const U64_MAX_STR = '18446744073709551615';
    const I64_MAX_PLUS_1_STR = '9223372036854775808';

    beforeAll(async () => {
      server = await startServer();
      client = await connectAdmin(HOST, server.port);
      dbName = await setupDb(client!, 'u64big', ['vals']);
    }, 60_000);

    afterAll(async () => {
      if (client) { try { await client.close(); } catch { /* ok */ } client = null; }
      if (server) { await server.stop(); server = null; }
    }, 15_000);

    it('writes u64::MAX as a decimal string and reads it back losslessly', async () => {
      // Write the value as the exact decimal string — the same wire
      // representation Rust's `Value::Big(u64::MAX)` produces.
      await br(await Batch.create('ins-big')
        .add('ins', write.insert('vals', {
          id: 'big1',
          n: U64_MAX_STR,
        }))
        .execute(client!, dbName));

      const rows = await client!.db(dbName).query('vals')
        .where(filter.eq('id', 'big1')).rows();
      expect(rows.length).toBe(1);
      // The value must survive the round-trip without corruption.
      expect(String(rows[0].n)).toBe(U64_MAX_STR);
    });

    it('Eq filter via litU64(u64::MAX) matches the stored value', async () => {
      // litU64(u64::MAX) returns the decimal string — the same text the
      // server stored. The Eq comparison is Str-vs-Str through the real
      // filter-eval path.
      const rows = await client!.db(dbName).query('vals')
        .where(filter.eq('n', filter.litU64(BigInt(U64_MAX_STR))))
        .rows();
      expect(rows.length).toBe(1);
      expect(String(rows[0].id)).toBe('big1');
    });

    it('Eq filter via litU64 for a different large value does NOT match', async () => {
      const rows = await client!.db(dbName).query('vals')
        .where(filter.eq('n', filter.litU64(BigInt(I64_MAX_PLUS_1_STR))))
        .rows();
      expect(rows.length).toBe(0);
    });

    it('writes i64::MAX+1 and matches with litU64', async () => {
      await br(await Batch.create('ins-i64max1')
        .add('ins', write.insert('vals', {
          id: 'big2',
          n: I64_MAX_PLUS_1_STR,
        }))
        .execute(client!, dbName));

      const rows = await client!.db(dbName).query('vals')
        .where(filter.eq('n', filter.litU64(BigInt(I64_MAX_PLUS_1_STR))))
        .rows();
      expect(rows.length).toBe(1);
      expect(String(rows[0].id)).toBe('big2');
      expect(String(rows[0].n)).toBe(I64_MAX_PLUS_1_STR);
    });

    it('litU64 with small value still works for normal Int fields', async () => {
      await br(await Batch.create('ins-small')
        .add('ins', write.insert('vals', {
          id: 'small1',
          n: 42,
        }))
        .execute(client!, dbName));

      const rows = await client!.db(dbName).query('vals')
        .where(filter.eq('n', filter.litU64(42)))
        .rows();
      expect(rows.length).toBe(1);
      expect(String(rows[0].id)).toBe('small1');
    });
  },
);
