import type { DurabilityOverride } from '../types/ddl.js';

export type WalRetentionBudget = number | string | bigint;

const U64_MAX = 18446744073709551615n;
const U64_DECIMAL = /^(?:0|[1-9][0-9]*)$/;

/** Validate an exact positive u64; convert bigint > i64::MAX to serde-safe decimal text. */
export function validateWalRetentionBudget(value: number | string | bigint): number | string | bigint {
  let exact: bigint;
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value <= 0) {
      throw new RangeError('wal_retention_max_bytes number must be a positive safe integer; use bigint for larger values');
    }
    exact = BigInt(value);
  } else if (typeof value === 'bigint') {
    exact = value;
  } else {
    if (!U64_DECIMAL.test(value)) throw new RangeError('wal_retention_max_bytes must be a canonical decimal u64 string');
    exact = BigInt(value);
  }
  if (exact <= 0n || exact > U64_MAX) throw new RangeError('wal_retention_max_bytes must be in the positive u64 range');
  return exact > 9223372036854775807n ? exact.toString() : value;
}

export function durabilityOverride(value: DurabilityOverride): DurabilityOverride {
  if (value.wal_retention_max_bytes !== undefined) {
    return { ...value, wal_retention_max_bytes: validateWalRetentionBudget(value.wal_retention_max_bytes) };
  }
  return { ...value };
}
