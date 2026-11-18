/**
 * Canonical field-rule encoding for the schema-migration canonical format v1
 * — TypeScript mirror of `encode_rule` / `encode_rules` / `encode_num` /
 * `encode_fk_action` in
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/canonical/ddl.rs`.
 *
 * Field order inside a rule is FIXED (mirrors the Rust destructuring of
 * `FieldRuleDto { path, r#type, constraints }` where `constraints` is
 * serde-flattened):
 *   `path` (count || str*), `type`, then in order: required, nullable,
 *   unsigned, min, max, len, max_len, min_len, one_of, default, array_of,
 *   format, foreign_key, unique, auto_now, auto_now_add — every constraint
 *   optional (`0`, or `1 || value`), `auto_now`/`auto_now_add` plain booleans.
 *
 * `min` / `max` use the migration widened bound ({@link MigrationNumBound}):
 * a plain safe-integer `number` or `bigint` is `NumDto::Int(i64)` (tag byte
 * `0`), a {@link MigrationFloat} instance is `NumDto::F64` bit-exact
 * (including `1.0` / `-0.0`) with tag byte `1` — the tag distinguishes
 * `2` from `2.0` whose canonical bytes differ.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { FkAction } from '../../../../types/schema/ddl.js';
import type {
  MigrationFieldRuleDto,
  MigrationNumBound,
} from '../../../../types/schema/migration-body.js';
import { MigrationFloat } from '../model/float.js';
import { CanonicalEncodeError, Encoder } from './encoder.js';
import { encodeFilterValue, encodeQueryValue } from './values.js';

/** `rules = count || rule*` (authored order preserved). */
export function encodeRules(e: Encoder, rules: readonly MigrationFieldRuleDto[]): void {
  e.count(rules.length);
  for (const rule of rules) encodeRule(e, rule);
}

/** One `FieldRuleDto` in the exact canonical field order. */
export function encodeRule(e: Encoder, rule: MigrationFieldRuleDto): void {
  e.strings(rule.path);
  e.str(rule.type);
  optBool(e, rule.required);
  optBool(e, rule.nullable);
  optBool(e, rule.unsigned);
  optNum(e, rule.min);
  optNum(e, rule.max);
  optU64(e, rule.len);
  optU64(e, rule.max_len);
  optU64(e, rule.min_len);
  if (rule.one_of !== undefined) {
    const values = rule.one_of;
    e.u8(1);
    e.count(values.length);
    for (const v of values) encodeQueryValue(e, v, 0);
  } else {
    e.u8(0);
  }
  if (rule.default !== undefined) {
    e.u8(1);
    encodeFilterValue(e, rule.default, 0);
  } else {
    e.u8(0);
  }
  e.opt(rule.array_of, (v) => e.str(v));
  e.opt(rule.format, (v) => e.str(v));
  if (rule.foreign_key !== undefined) {
    const key = rule.foreign_key;
    e.u8(1);
    e.str(key.ref_table);
    e.str(key.ref_field);
    e.u8(fkActionByte(key.on_delete));
    e.u8(fkActionByte(key.on_update));
  } else {
    e.u8(0);
  }
  optBool(e, rule.unique);
  e.bool(rule.auto_now === true);
  e.bool(rule.auto_now_add === true);
}

/**
 * Optional boolean: `1 || bool` when the key is present (even `false`),
 * `0` when absent — Rust encodes `Some(false)` and `None` differently.
 */
function optBool(e: Encoder, value: boolean | undefined): void {
  if (value !== undefined) {
    e.u8(1);
    e.bool(value);
  } else {
    e.u8(0);
  }
}

/** Optional u64 (`number | bigint` per the package u64 convention). */
function optU64(e: Encoder, value: number | bigint | undefined): void {
  if (value !== undefined) {
    e.u8(1);
    e.u64(value);
  } else {
    e.u8(0);
  }
}

/**
 * Optional numeric bound, mirroring `encode_num` (ddl.rs 436-456): the
 * optional-presence byte (`1` when present) precedes the variant tag —
 * `0` + i64 for `NumDto::Int`, `1` + bit-exact f64 for `NumDto::F64`.
 *
 * A plain safe-integer `number` is `Int`; a finite fractional `number` is
 * encoded bit-exactly as `F64` (the wire framing leaves a fractional msgpack
 * number as f64); any integer-but-unsafe or non-finite `number` must be
 * wrapped in a {@link MigrationFloat} (`F64`, bit-exact, preserving `-0.0`).
 */
function optNum(e: Encoder, value: MigrationNumBound | undefined): void {
  if (value === undefined) {
    e.u8(0);
    return;
  }
  if (value instanceof MigrationFloat) {
    if (!Number.isFinite(value.value)) {
      throw new CanonicalEncodeError('non-finite number');
    }
    e.u8(1);
    e.u8(1);
    e.f64Bits(value.value);
    return;
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) {
      throw new CanonicalEncodeError('non-finite number');
    }
    if (Number.isSafeInteger(value)) {
      e.u8(1);
      e.u8(0);
      e.i64(value);
      return;
    }
    // A finite fractional number is framed as msgpack F64 on the wire —
    // encode it bit-exactly as `NumDto::F64`.
    e.u8(1);
    e.u8(1);
    e.f64Bits(value);
    return;
  }
  e.u8(1);
  e.u8(0);
  e.i64(value);
}

/** `FkAction` tag bytes; a missing action is the serde default `no_action`. */
function fkActionByte(action: FkAction | undefined): number {
  switch (action) {
    case undefined:
    case 'no_action':
      return 0;
    case 'restrict':
      return 1;
    case 'cascade':
      return 2;
    case 'set_null':
      return 3;
    default: {
      const exhaustive: never = action;
      throw new CanonicalEncodeError(`unknown FK action: ${String(exhaustive)}`);
    }
  }
}
