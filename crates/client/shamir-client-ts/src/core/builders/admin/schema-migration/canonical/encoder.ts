/**
 * Primitive canonical encoder for the schema-migration canonical format v1 —
 * TypeScript mirror of
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/canonical/body.rs`
 * (`Encoder`).
 *
 * All lengths and counts are `u32` big-endian; strings are UTF-8 bytes with
 * no normalization; integers are big-endian; booleans are one byte (`0`/`1`);
 * optional values are `0` or `1 || value`.
 *
 * u64/i64 are encoded through `DataView.setBigUint64` / `setBigInt64` so
 * values above 2^53 round-trip exactly; `number` arguments must be safe
 * integers. There is deliberately no process/node API here.
 *
 * PLATFORM-AGNOSTIC.
 */

const utf8 = new TextEncoder();

/** Concatenate byte chunks into one fresh `Uint8Array`. */
export function concatBytes(chunks: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let i = 0;
  for (const c of chunks) {
    out.set(c, i);
    i += c.length;
  }
  return out;
}

/**
 * Compare two strings by their UTF-8 BYTES, not by JS UTF-16 code-unit
 * order (the two differ for astral characters) — mirrors Rust
 * `left.as_bytes().cmp(right.as_bytes())`.
 */
export function compareUtf8(left: string, right: string): number {
  const a = utf8.encode(left);
  const b = utf8.encode(right);
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
}

/** Return a NEW array sorted by UTF-8 bytes (input is not mutated). */
export function sortedByUtf8(values: readonly string[]): string[] {
  return [...values].sort(compareUtf8);
}

const U64_MAX = 0xffff_ffff_ffff_ffffn;
const I64_MIN = -0x8000_0000_0000_0000n;
const I64_MAX = 0x7fff_ffff_ffff_ffffn;
const u64View = new DataView(new ArrayBuffer(8));

/** Thrown by encoders for values the canonical format cannot represent. */
export class CanonicalEncodeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CanonicalEncodeError';
  }
}

/**
 * Byte sink mirroring Rust `Encoder` in `canonical/body.rs`. Every write
 * appends to `chunks`; `finish()` concatenates.
 */
export class Encoder {
  private readonly chunks: Uint8Array[] = [];

  raw(bytes: Uint8Array): void {
    this.chunks.push(bytes);
  }

  u8(value: number): void {
    this.raw(new Uint8Array([value]));
  }

  bool(value: boolean): void {
    this.u8(value ? 1 : 0);
  }

  u16(value: number): void {
    if (!Number.isInteger(value) || value < 0 || value > 0xffff) {
      throw new CanonicalEncodeError(`u16 out of range: ${String(value)}`);
    }
    this.raw(new Uint8Array([(value >> 8) & 255, value & 255]));
  }

  u32(value: number): void {
    if (!Number.isInteger(value) || value < 0 || value > 0xffff_ffff) {
      throw new CanonicalEncodeError(`u32 out of range: ${String(value)}`);
    }
    this.raw(new Uint8Array([(value >>> 24) & 255, (value >>> 16) & 255, (value >>> 8) & 255, value & 255]));
  }

  /**
   * Rust `u64`. Accepts a safe-integer `number` or a `bigint` (the package
   * u64 convention); rejects anything outside `[0, 2^64)`.
   */
  u64(value: number | bigint): void {
    const n = typeof value === 'number' ? safeInteger(value) : value;
    if (n < 0n || n > U64_MAX) {
      throw new CanonicalEncodeError(`u64 out of range: ${n.toString()}`);
    }
    u64View.setBigUint64(0, n, false);
    this.raw(new Uint8Array(u64View.buffer.slice(0)));
  }

  /** Rust `i64`; accepts a safe-integer `number` or a `bigint`. */
  i64(value: number | bigint): void {
    const n = typeof value === 'number' ? safeInteger(value) : value;
    if (n < I64_MIN || n > I64_MAX) {
      throw new CanonicalEncodeError(`i64 out of range: ${n.toString()}`);
    }
    u64View.setBigInt64(0, n, false);
    this.raw(new Uint8Array(u64View.buffer.slice(0)));
  }

  /** Bit-exact f64 (including negative zero) — mirrors `f64::to_bits().to_be_bytes()`. */
  f64Bits(value: number): void {
    u64View.setFloat64(0, value, false);
    this.raw(new Uint8Array(u64View.buffer.slice(0)));
  }

  /** `u32` length prefix + raw bytes. */
  bytes(value: Uint8Array): void {
    this.u32(value.length);
    this.raw(value);
  }

  /** `u32` length prefix + UTF-8 bytes (no normalization). */
  str(value: string): void {
    this.bytes(utf8.encode(value));
  }

  /** `0`, or `1 || value` (value is present when not `null`/`undefined`). */
  opt<T>(value: T | null | undefined, write: (v: T) => void): void {
    if (value == null) this.u8(0);
    else {
      this.u8(1);
      write(value);
    }
  }

  /** `u32` count of a list. */
  count(value: number): void {
    this.u32(value);
  }

  /** `count || str*` for a plain list of strings (order preserved). */
  strings(values: readonly string[]): void {
    this.count(values.length);
    for (const v of values) this.str(v);
  }

  finish(): Uint8Array {
    return concatBytes(this.chunks);
  }
}

function safeInteger(value: number): bigint {
  if (!Number.isSafeInteger(value)) {
    throw new CanonicalEncodeError(
      `unsafe integer ${String(value)}; use bigint for values above 2^53`,
    );
  }
  return BigInt(value);
}
