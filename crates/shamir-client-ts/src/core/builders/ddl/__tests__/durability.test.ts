import { describe, it, expect } from 'vitest';
import { createRepo, alterRepo, describeRepo } from '../../admin/repo-admin/index.js';
import { encode, decode } from '../../../protocol/framing.js';

describe('repository durability builders', () => {
  it('emits sparse create and alter overrides losslessly', () => {
    const durability = { wal_commit: 'synced' as const, wal_retention_max_bytes: 9223372036854775808n, on_wal_retention_exceeded: 'declare_gap' as const, replication_serve: 'durable' as const };
    expect(createRepo('r', { durability })).toEqual({ create_repo: 'r', durability: { ...durability, wal_retention_max_bytes: '9223372036854775808' } });
    expect(createRepo('r', { durability: { wal_retention_max_bytes: 18446744073709551615n } })).toEqual({ create_repo: 'r', durability: { wal_retention_max_bytes: '18446744073709551615' } });
    expect(alterRepo('r', durability)).toEqual({ alter_repo: 'r', durability: { ...durability, wal_retention_max_bytes: '9223372036854775808' } });
    expect(alterRepo('r', { replication_serve: 'completed_reseed' as const })).toEqual({ alter_repo: 'r', durability: { replication_serve: 'completed_reseed' } });
    expect(describeRepo('r')).toEqual({ describe_repo: 'r' });
    const response = decode(encode({ durability: { wal_retention_max_bytes: 9223372036854775808n } })) as { durability: { wal_retention_max_bytes: string | bigint } };
    expect(response.durability.wal_retention_max_bytes).toBe(9223372036854775808n);
    const serverWire = decode(encode({ durability: { wal_retention_max_bytes: '18446744073709551615' } })) as { durability: { wal_retention_max_bytes: string | bigint } };
    expect(serverWire.durability.wal_retention_max_bytes).toBe('18446744073709551615');
    const small = decode(encode({ durability: { wal_retention_max_bytes: 1073741824 } })) as { durability: { wal_retention_max_bytes: number } };
    expect(small.durability.wal_retention_max_bytes).toBe(1073741824);
  });

  it('rejects imprecise, nonpositive and out-of-u64 number budgets', () => {
    expect(createRepo('r', { durability: { wal_retention_max_bytes: 9223372036854775807n } }).durability?.wal_retention_max_bytes).toBe(9223372036854775807n);
    expect(createRepo('r', { durability: { wal_retention_max_bytes: 9223372036854775808n } }).durability?.wal_retention_max_bytes).toBe('9223372036854775808');
    expect(createRepo('r', { durability: { wal_retention_max_bytes: '18446744073709551615' } }).durability?.wal_retention_max_bytes).toBe('18446744073709551615');
    for (const value of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, Infinity]) {
      expect(() => createRepo('r', { durability: { wal_retention_max_bytes: value } })).toThrow(RangeError);
    }
    expect(() => createRepo('r', { durability: { wal_retention_max_bytes: 18446744073709551616n } })).toThrow(RangeError);
    for (const value of ['01', '+1', '1.0', '', '18446744073709551616']) {
      expect(() => createRepo('r', { durability: { wal_retention_max_bytes: value } })).toThrow(RangeError);
    }
  });
});
