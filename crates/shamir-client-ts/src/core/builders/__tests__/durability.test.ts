import { describe, it, expect } from 'vitest';
import { ddl } from '../ddl.js';

describe('repository durability builders', () => {
  it('emits sparse create and alter overrides losslessly', () => {
    const durability = { wal_commit: 'synced' as const, wal_retention_max_bytes: 9223372036854775808n, on_wal_retention_exceeded: 'declare_gap' as const, replication_serve: 'durable' as const };
    expect(ddl.createRepo('r', { durability })).toEqual({ create_repo: 'r', durability: { ...durability, wal_retention_max_bytes: '9223372036854775808' } });
    expect(ddl.createRepo('r', { durability: { wal_retention_max_bytes: 18446744073709551615n } })).toEqual({ create_repo: 'r', durability: { wal_retention_max_bytes: '18446744073709551615' } });
    expect(ddl.alterRepo('r', durability)).toEqual({ alter_repo: 'r', durability });
    expect(ddl.describeRepo('r')).toEqual({ describe_repo: 'r' });
    const response = { describe_repo: 'r', repo: 'db', engine: 'in_memory', path: null, durability: { wal_commit: 'buffered', wal_retention_max_bytes: '18446744073709551615', on_wal_retention_exceeded: 'refuse_writes', replication_serve: 'durable' }, durability_override: null };
    expect(response.durability.wal_retention_max_bytes).toBe('18446744073709551615');
  });

  it('rejects imprecise, nonpositive and out-of-u64 number budgets', () => {
    for (const value of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, Infinity]) {
      expect(() => ddl.createRepo('r', { durability: { wal_retention_max_bytes: value } })).toThrow(RangeError);
    }
    expect(() => ddl.createRepo('r', { durability: { wal_retention_max_bytes: 18446744073709551616n } })).toThrow(RangeError);
    for (const value of ['01', '+1', '1.0', '', '18446744073709551616']) {
      expect(() => ddl.createRepo('r', { durability: { wal_retention_max_bytes: value } })).toThrow(RangeError);
    }
  });
});
