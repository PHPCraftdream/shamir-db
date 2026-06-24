/**
 * Replica of `fxhash::hash64` (fxhash 0.2.1, `FxHasher64`) for computing
 * `principal_id` in TypeScript.
 *
 * Algorithm (from the Rust `fxhash` crate `write64` + `FxHasher64::write_u8`):
 *
 *   SEED = 0x517cc1b727220a95  (the FxHasher64 multiplication constant)
 *   ROTATE = 5
 *
 *   hash_word(h, word): h = rotl5(h) ^ word; h = h * SEED  [wrapping u64]
 *
 *   write64(h, bytes):
 *     while len >= 8 -> read u64 LE, hash_word
 *     if len >= 4   -> read u32 LE, hash_word(n as u64)
 *     for remaining bytes (0-3): hash_word(byte as u64)
 *
 *   <str as Hash>::hash calls:
 *     hasher.write(self.as_bytes())   // the write64 path
 *     hasher.write_u8(0xff)           // terminator
 *
 *   FxHasher64::write_u8(i) -> self.hash.hash_word(i as u64)
 *
 *   principal_id(username) = hash64(username) & (i64::MAX as u64)
 *
 * All arithmetic is done in BigInt with explicit masking to emulate
 * wrapping u64 semantics.
 *
 * PLATFORM-AGNOSTIC.
 */

const SEED64 = 0x517cc1b727220a95n;
const MASK64 = 0xFFFFFFFFFFFFFFFFn;
const I64MAX = 0x7FFFFFFFFFFFFFFFn;

function hashWord(h: bigint, word: bigint): bigint {
  h = ((h << 5n) | (h >> 59n)) & MASK64; // rotate_left(5)
  h ^= word;
  h = (h * SEED64) & MASK64; // wrapping_mul
  return h;
}

/**
 * Compute the principal id for a username, matching the server's
 * `fxhash::hash64(username) & (i64::MAX as u64)`.
 *
 * Returns a `bigint` that must be encoded as a msgpack uint64 on the wire
 * (requires `useBigInt64: true` in the msgpack encoder).
 */
export function principalId(username: string): bigint {
  const bytes = new TextEncoder().encode(username);
  let h = 0n;
  let offset = 0;
  const len = bytes.length;

  // write64: 8-byte (u64 LE NativeEndian) chunks
  while (offset + 8 <= len) {
    const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 8);
    const lo = BigInt(view.getUint32(0, true));
    const hi = BigInt(view.getUint32(4, true));
    const word = (hi << 32n) | lo;
    h = hashWord(h, word);
    offset += 8;
  }

  // write64: 4-byte (u32 LE) chunk if remaining >= 4
  if (offset + 4 <= len) {
    const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 4);
    const word = BigInt(view.getUint32(0, true));
    h = hashWord(h, word);
    offset += 4;
  }

  // write64: remaining bytes one-by-one (0 to 3 bytes)
  while (offset < len) {
    h = hashWord(h, BigInt(bytes[offset]));
    offset += 1;
  }

  // FxHasher64::write_u8(0xFF) — the str Hash trait terminator
  h = hashWord(h, 0xFFn);

  return h & I64MAX;
}
