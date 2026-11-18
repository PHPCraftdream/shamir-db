/**
 * Canonical value encoders for the schema-migration canonical format v1 —
 * TypeScript mirror of `encode_value` / `encode_filter_value` in
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/canonical/ddl.rs`.
 *
 * Two distinct tag tables exist on the Rust side and BOTH are mirrored:
 *   - `QueryValue` (`encode_value`): Null 0, Bool 1, Int 2, F64 3, Big 4,
 *     Dec 5, Str 6, Bin 7, List 8, Set 9, Map 10 — used for
 *     `functional_args` and `one_of`.
 *   - `FilterValue` (`encode_filter_value`): Null 0, Bool 1, Int 2, Float 3,
 *     Binary 4, String 5, Array 6 — used for constraint `default`; the
 *     dynamic variants (`FieldRef`/`QueryRef`/`FnCall`/`Expr`/`Cond`/`Param`)
 *     are excluded and rejected.
 *
 * TS JSON-shaped values (`WireValue` / `WriteValue`) carry no Int/Float tag,
 * so the split follows the wire-value convention: a safe-integer `number` or
 * `bigint` encodes as `Int` (i64); a fractional `number` encodes as
 * `F64`/`Float` bit-exactly. An EXPLICIT f64 that would otherwise look
 * integral (e.g. `1.0`, `-0.0`) or an unsafe integer must be wrapped in a
 * {@link MigrationFloat} instance (`.value`) — see `../model/float.js`.
 *
 * PLATFORM-AGNOSTIC.
 */

import { MigrationFloat } from '../model/float.js';
import { CanonicalEncodeError, Encoder, compareUtf8 } from './encoder.js';

const MAX_DEPTH = 64;

function checkDepth(depth: number, what: string): void {
  if (depth > MAX_DEPTH) {
    throw new CanonicalEncodeError(`${what} nesting exceeds ${MAX_DEPTH}`);
  }
}

/** Reject non-finite floats — mirrors the Rust `is_finite` guards. */
function finiteFloat(value: number, what: string): number {
  if (!Number.isFinite(value)) {
    throw new CanonicalEncodeError(`non-finite ${what}`);
  }
  return value;
}

/**
 * `QueryValue`-table encoding (`encode_value` in `ddl.rs`). Accepts the
 * JSON-shaped `WireValue` union the plain builders produce.
 */
export function encodeQueryValue(e: Encoder, value: unknown, depth = 0): void {
  checkDepth(depth, 'query value');
  if (value === null) {
    e.u8(0);
    return;
  }
  if (typeof value === 'boolean') {
    e.u8(1);
    e.bool(value);
    return;
  }
  if (typeof value === 'number') {
    encodeNumber(e, value, 3, 'query float');
    return;
  }
  if (typeof value === 'bigint') {
    e.u8(2);
    e.i64(value);
    return;
  }
  if (value instanceof MigrationFloat) {
    e.u8(3);
    e.f64Bits(finiteFloat(value.value, 'query float'));
    return;
  }
  if (value instanceof Uint8Array) {
    e.u8(7);
    e.bytes(value);
    return;
  }
  if (typeof value === 'string') {
    e.u8(6);
    e.str(value);
    return;
  }
  if (Array.isArray(value)) {
    e.u8(8);
    e.count(value.length);
    for (const item of value) encodeQueryValue(e, item, depth + 1);
    return;
  }
  // Plain objects are `QueryValue::Map` — every key, including `$ref` etc., is
  // a literal map key on this table; validation (not encoding) filters dynamic
  // forms.
  if (typeof value === 'object') {
    encodeMap(e, value as Record<string, unknown>, depth);
    return;
  }
  throw new CanonicalEncodeError('query value cannot be canonically encoded');
}

/**
 * `FilterValue`-table encoding (`encode_filter_value` in `ddl.rs`). Accepts
 * the JSON-shaped `WriteValue` union; literal-only — expression forms are
 * rejected exactly like the Rust dynamic-variant arm.
 */
export function encodeFilterValue(e: Encoder, value: unknown, depth = 0): void {
  checkDepth(depth, 'filter value');
  if (value === null) {
    e.u8(0);
    return;
  }
  if (typeof value === 'boolean') {
    e.u8(1);
    e.bool(value);
    return;
  }
  if (typeof value === 'number') {
    encodeNumber(e, value, 3, 'filter float');
    return;
  }
  if (typeof value === 'bigint') {
    e.u8(2);
    e.i64(value);
    return;
  }
  if (value instanceof MigrationFloat) {
    e.u8(3);
    e.f64Bits(finiteFloat(value.value, 'filter float'));
    return;
  }
  if (value instanceof Uint8Array) {
    e.u8(4);
    e.bytes(value);
    return;
  }
  if (typeof value === 'string') {
    e.u8(5);
    e.str(value);
    return;
  }
  if (Array.isArray(value)) {
    e.u8(6);
    e.count(value.length);
    for (const item of value) encodeFilterValue(e, item, depth + 1);
    return;
  }
  throw new CanonicalEncodeError('dynamic filter value is excluded');
}

/**
 * Safe integer → Int tag; fractional finite → Float/F64 tag. An unsafe
 * integer number must be wrapped in {@link MigrationFloat} (which also
 * preserves `-0.0` bit-exactly): the wire framing promotes wide integer
 * numbers to `bigint`, so encoding them as floats would diverge.
 */
function encodeNumber(e: Encoder, value: number, floatTag: number, what: string): void {
  if (Number.isSafeInteger(value)) {
    e.u8(2);
    e.i64(value);
    return;
  }
  if (Number.isInteger(value)) {
    throw new CanonicalEncodeError(
      `unsafe integer ${what}: wrap in MigrationFloat to encode as f64`,
    );
  }
  e.u8(floatTag);
  e.f64Bits(finiteFloat(value, what));
}

interface MapEntry {
  key: string;
  bytes: Uint8Array;
}

/**
 * `QueryValue::Map` encoding: entries sorted by UTF-8 KEY BYTES (never JS
 * string order), values length-prefixed (`u32 || bytes`).
 */
function encodeMap(e: Encoder, value: Record<string, unknown>, depth: number): void {
  const entries: MapEntry[] = Object.keys(value).map((key) => {
    const v = new Encoder();
    encodeQueryValue(v, value[key], depth + 1);
    return { key, bytes: v.finish() };
  });
  entries.sort((a, b) => compareUtf8(a.key, b.key));
  e.u8(10);
  e.count(entries.length);
  for (const { key, bytes } of entries) {
    e.str(key);
    e.bytes(bytes);
  }
}
