/**
 * Client-side interner encoding/decoding ops for the id-keyed write path.
 *
 * Mirrors `crates/shamir-client/src/interner_cache_ops.rs`:
 *   - `encodeRecordIdMsgpack`  — string-keyed record -> id-keyed msgpack bytes
 *   - `qvHasFnMarker`         — detect $fn records (not id-encodable)
 *   - `collectFieldNames`     — walk INSERT/SET/UPDATE ops, collect field names
 *   - `deinternResponse`      — id-keyed result rows -> name-keyed objects
 *
 * PLATFORM-AGNOSTIC.
 */

import { encode as msgpackEncode, decode as msgpackDecode } from '@msgpack/msgpack';
import type { FieldMap, InternerCacheRegistry } from './field-map.js';
import type { BatchResponse, QueryResult } from './types/batch.js';
import type { WireValue } from './types/write.js';

// ── encodeRecordIdMsgpack ───────────────────────────────────────────────────

/**
 * Encode a single string-keyed record to id-keyed storage msgpack bytes.
 *
 * Map keys are replaced with their interner id, encoded as msgpack `bin`
 * with minimal little-endian bytes (1/2/4/8), matching the Rust
 * `InternerKey::serialize` format exactly. Values and nested structures
 * are recursively processed; nested map keys are also interned.
 *
 * All field names MUST already be present in `fieldMap` (call `touchFields`
 * first). Throws if a field name is not found in the cache.
 *
 * The output bytes are built manually because `@msgpack/msgpack` does not
 * natively support maps with `Uint8Array` (bin) keys. The format is:
 * msgpack map header + for each entry: bin8 header + LE id bytes + msgpack
 * encoded value.
 */
export function encodeRecordIdMsgpack(
  record: Record<string, WireValue>,
  fieldMap: FieldMap,
): Uint8Array {
  const parts: Uint8Array[] = [];
  encodeIdKeyedMap(record, fieldMap, parts);
  return concatParts(parts);
}

/**
 * Encode a string-keyed object as a msgpack map with bin keys into `parts`.
 * Keys are interned via fieldMap; values are recursively encoded (nested
 * maps get the same treatment; other values use standard msgpack encoding).
 */
function encodeIdKeyedMap(
  obj: Record<string, WireValue>,
  fieldMap: FieldMap,
  parts: Uint8Array[],
): void {
  const entries = Object.entries(obj);
  const count = entries.length;

  // Map header (matching rmp_serde: FixMap if <=15, Map16 if <=65535, Map32).
  if (count <= 15) {
    parts.push(new Uint8Array([0x80 | count]));
  } else if (count <= 65535) {
    const buf = new Uint8Array(3);
    buf[0] = 0xde;
    buf[1] = (count >> 8) & 0xFF;
    buf[2] = count & 0xFF;
    parts.push(buf);
  } else {
    const buf = new Uint8Array(5);
    buf[0] = 0xdf;
    buf[1] = (count >> 24) & 0xFF;
    buf[2] = (count >> 16) & 0xFF;
    buf[3] = (count >> 8) & 0xFF;
    buf[4] = count & 0xFF;
    parts.push(buf);
  }

  for (const [key, value] of entries) {
    const id = fieldMap.getId(key);
    if (id === undefined) {
      throw new Error(
        `field '${key}' not in FieldMap — touchFields must be called first`,
      );
    }
    // Key: msgpack bin8 + LE id bytes (InternerKey::serialize).
    const idBytes = internerKeyToLeBytes(id);
    const binHeader = new Uint8Array(2);
    binHeader[0] = 0xc4; // bin8
    binHeader[1] = idBytes.length;
    parts.push(binHeader);
    parts.push(idBytes);

    // Value: recursively encode (nested maps get id-keyed treatment).
    encodeIdKeyedValue(value, fieldMap, parts);
  }
}

/**
 * Encode a value into `parts`. If it's a plain object (map), use id-keyed
 * encoding. Otherwise use standard `@msgpack/msgpack` encoding.
 */
function encodeIdKeyedValue(
  value: WireValue,
  fieldMap: FieldMap,
  parts: Uint8Array[],
): void {
  if (value !== null && value !== undefined && !Array.isArray(value) && typeof value === 'object') {
    // Nested map — recurse with id-keyed encoding.
    encodeIdKeyedMap(value as Record<string, WireValue>, fieldMap, parts);
  } else if (Array.isArray(value)) {
    // Array: encode header + each element.
    const len = value.length;
    if (len <= 15) {
      parts.push(new Uint8Array([0x90 | len]));
    } else if (len <= 65535) {
      const buf = new Uint8Array(3);
      buf[0] = 0xdc;
      buf[1] = (len >> 8) & 0xFF;
      buf[2] = len & 0xFF;
      parts.push(buf);
    } else {
      const buf = new Uint8Array(5);
      buf[0] = 0xdd;
      buf[1] = (len >> 24) & 0xFF;
      buf[2] = (len >> 16) & 0xFF;
      buf[3] = (len >> 8) & 0xFF;
      buf[4] = len & 0xFF;
      parts.push(buf);
    }
    for (const item of value) {
      encodeIdKeyedValue(item, fieldMap, parts);
    }
  } else {
    // Scalar (null, boolean, number, string) — use standard msgpack.
    parts.push(msgpackEncode(value));
  }
}

/** Concatenate an array of Uint8Array parts into a single Uint8Array. */
function concatParts(parts: Uint8Array[]): Uint8Array {
  let totalLen = 0;
  for (const p of parts) totalLen += p.length;
  const result = new Uint8Array(totalLen);
  let offset = 0;
  for (const p of parts) {
    result.set(p, offset);
    offset += p.length;
  }
  return result;
}

/**
 * Encode an interner id as minimal-width little-endian bytes, matching
 * the Rust `InternerKey::serialize` format (serialized as msgpack `bin`
 * via `serialize_bytes`).
 *
 * Width: 1 byte if id <= 255, 2 if <= 65535, 4 if <= 2^32-1, else 8.
 */
function internerKeyToLeBytes(id: bigint): Uint8Array {
  if (id <= 0xFFn) {
    return new Uint8Array([Number(id)]);
  } else if (id <= 0xFFFFn) {
    const buf = new Uint8Array(2);
    const n = Number(id);
    buf[0] = n & 0xFF;
    buf[1] = (n >> 8) & 0xFF;
    return buf;
  } else if (id <= 0xFFFFFFFFn) {
    const buf = new Uint8Array(4);
    const n = Number(id);
    buf[0] = n & 0xFF;
    buf[1] = (n >> 8) & 0xFF;
    buf[2] = (n >> 16) & 0xFF;
    buf[3] = (n >> 24) & 0xFF;
    return buf;
  } else {
    const buf = new Uint8Array(8);
    const lo = Number(id & 0xFFFFFFFFn);
    const hi = Number((id >> 32n) & 0xFFFFFFFFn);
    buf[0] = lo & 0xFF;
    buf[1] = (lo >> 8) & 0xFF;
    buf[2] = (lo >> 16) & 0xFF;
    buf[3] = (lo >> 24) & 0xFF;
    buf[4] = hi & 0xFF;
    buf[5] = (hi >> 8) & 0xFF;
    buf[6] = (hi >> 16) & 0xFF;
    buf[7] = (hi >> 24) & 0xFF;
    return buf;
  }
}

/**
 * Decode a minimal-width LE bin key back to a bigint id.
 * Mirrors `InternerKey::from_raw_bytes`.
 */
function leBytesToId(bytes: Uint8Array): bigint {
  switch (bytes.length) {
    case 1:
      return BigInt(bytes[0]);
    case 2:
      return BigInt(bytes[0] | (bytes[1] << 8));
    case 4:
      return BigInt(
        (bytes[0] | (bytes[1] << 8) | (bytes[2] << 16) | (bytes[3] << 24)) >>> 0,
      );
    case 8: {
      const lo = (bytes[0] | (bytes[1] << 8) | (bytes[2] << 16) | (bytes[3] << 24)) >>> 0;
      const hi = (bytes[4] | (bytes[5] << 8) | (bytes[6] << 16) | (bytes[7] << 24)) >>> 0;
      return BigInt(lo) | (BigInt(hi) << 32n);
    }
    default:
      throw new Error(
        `invalid InternerKey length: ${bytes.length} (must be 1, 2, 4, or 8)`,
      );
  }
}

// ── qvHasFnMarker ───────────────────────────────────────────────────────────

/**
 * Returns `true` if `v` contains a map with the key `"$fn"` anywhere in the
 * value tree. Records containing `$fn` rely on server-side evaluation and
 * MUST NOT be encoded as id-keyed storage bytes.
 *
 * Mirrors the Rust `qv_has_fn_marker`.
 */
export function qvHasFnMarker(v: WireValue): boolean {
  if (v === null || v === undefined) return false;
  if (Array.isArray(v)) {
    return v.some((item) => qvHasFnMarker(item));
  }
  if (typeof v === 'object') {
    const obj = v as Record<string, WireValue>;
    if ('$fn' in obj) return true;
    return Object.values(obj).some((child) => qvHasFnMarker(child));
  }
  return false;
}

// ── collectFieldNames ───────────────────────────────────────────────────────

/**
 * Walk a batch entry, collecting field names that appear as top-level map
 * keys of insert/upsert/update record values, grouped by repo.
 *
 * Recurses into nested maps so that `{ "profile": { "age": 30 } }` registers
 * both "profile" and "age".
 *
 * Mirrors the Rust `collect_field_names`.
 */
export function collectFieldNames(
  entry: Record<string, unknown>,
  out: Map<string, string[]>,
): void {
  let repo: string;
  let values: WireValue[];

  if ('insert_into' in entry) {
    repo = tableRefToRepo(entry['insert_into']);
    values = (entry['values'] as WireValue[]) ?? [];
  } else if ('set' in entry && 'key' in entry && 'value' in entry) {
    // SetOp (upsert).
    repo = tableRefToRepo(entry['set']);
    values = [entry['key'] as WireValue, entry['value'] as WireValue];
  } else if ('update' in entry && 'set' in entry) {
    // UpdateOp.
    repo = tableRefToRepo(entry['update']);
    values = [entry['set'] as WireValue];
  } else {
    return;
  }

  if (values.length === 0) return;

  let bucket = out.get(repo);
  if (!bucket) {
    bucket = [];
    out.set(repo, bucket);
  }
  for (const v of values) {
    collectMapKeys(v, bucket);
  }
}

/** Recursively collect map keys from a value (and its nested maps). */
function collectMapKeys(v: WireValue, out: string[]): void {
  if (v === null || v === undefined) return;
  if (Array.isArray(v)) {
    for (const item of v) {
      collectMapKeys(item, out);
    }
    return;
  }
  if (typeof v === 'object') {
    const obj = v as Record<string, WireValue>;
    for (const [key, child] of Object.entries(obj)) {
      out.push(key);
      collectMapKeys(child, out);
    }
  }
}

/** Extract the repo string from a table-ref wire value. */
function tableRefToRepo(tableRef: unknown): string {
  if (Array.isArray(tableRef) && tableRef.length >= 1) {
    return String(tableRef[0]);
  }
  return 'main';
}

// ── deinternResponse ────────────────────────────────────────────────────────

/**
 * De-intern id-keyed result rows in `response` back to name-keyed objects.
 *
 * Walks every result's `records` array. If a record is a `Uint8Array` (the
 * server returns id-keyed rows as raw bytes when `result_encoding = Id`),
 * decodes the msgpack and replaces bin keys with their field names via the
 * FieldMap.
 *
 * Mirrors the Rust `deintern_response`.
 */
export function deinternResponse(
  registry: InternerCacheRegistry,
  db: string,
  response: BatchResponse,
  repos: string[],
): BatchResponse {
  for (const result of Object.values(response.results)) {
    deinternQueryResult(registry, db, result, repos);
  }
  return response;
}

/** De-intern all id-bytes records in a single QueryResult. */
function deinternQueryResult(
  registry: InternerCacheRegistry,
  db: string,
  result: QueryResult,
  repos: string[],
): void {
  for (let i = 0; i < result.records.length; i++) {
    const rec = result.records[i];
    if (rec instanceof Uint8Array) {
      // Id-keyed bytes row. Decode manually (bin-keyed maps are not
      // supported by @msgpack/msgpack's default decoder).
      const deinterned = decodeIdKeyedRecord(registry, db, rec, repos);
      result.records[i] = deinterned as Record<string, WireValue>;
    }
  }
}

/**
 * Manually decode an id-keyed msgpack record and de-intern the bin keys
 * to field names. The format is a msgpack map where keys are bin (LE id
 * bytes) and values are standard msgpack values.
 *
 * Uses a cursor-based parser for the top-level map and nested maps with
 * bin keys; delegates scalar/array/string-keyed-map values to
 * `@msgpack/msgpack.decode()`.
 */
function decodeIdKeyedRecord(
  registry: InternerCacheRegistry,
  db: string,
  bytes: Uint8Array,
  repos: string[],
): Record<string, WireValue> {
  const cursor = { pos: 0 };
  return decodeIdKeyedMapFromBytes(registry, db, bytes, cursor, repos);
}

/** Read a msgpack map with bin keys from `bytes` starting at `cursor.pos`. */
function decodeIdKeyedMapFromBytes(
  registry: InternerCacheRegistry,
  db: string,
  bytes: Uint8Array,
  cursor: { pos: number },
  repos: string[],
): Record<string, WireValue> {
  const count = readMapLen(bytes, cursor);
  const result: Record<string, WireValue> = {};

  for (let i = 0; i < count; i++) {
    // Read key: expected to be bin (0xc4 = bin8).
    const keyMarker = bytes[cursor.pos++];
    if (keyMarker !== 0xc4 && keyMarker !== 0xc5 && keyMarker !== 0xc6) {
      throw new Error(
        `de-intern: expected bin key marker, got 0x${keyMarker.toString(16)}`,
      );
    }
    let keyLen: number;
    if (keyMarker === 0xc4) {
      keyLen = bytes[cursor.pos++];
    } else if (keyMarker === 0xc5) {
      keyLen = (bytes[cursor.pos] << 8) | bytes[cursor.pos + 1];
      cursor.pos += 2;
    } else {
      keyLen =
        (bytes[cursor.pos] << 24) |
        (bytes[cursor.pos + 1] << 16) |
        (bytes[cursor.pos + 2] << 8) |
        bytes[cursor.pos + 3];
      cursor.pos += 4;
    }
    const keyBytes = bytes.subarray(cursor.pos, cursor.pos + keyLen);
    cursor.pos += keyLen;
    const id = leBytesToId(keyBytes);

    const name = resolveIdFromRepos(registry, db, id, repos);
    if (name === undefined) {
      throw new Error(
        `de-intern: field id ${id} not found in any FieldMap (db=${db})`,
      );
    }

    // Read value: peek at the marker to decide how to decode.
    const valMarker = bytes[cursor.pos];
    let value: WireValue;
    if (isMapMarker(valMarker)) {
      // Nested map — recurse.
      value = decodeIdKeyedMapFromBytes(registry, db, bytes, cursor, repos) as WireValue;
    } else {
      // Non-map value: find value extent and decode with @msgpack/msgpack.
      const valStart = cursor.pos;
      skipMsgpackValue(bytes, cursor);
      const valBytes = bytes.subarray(valStart, cursor.pos);
      value = msgpackDecode(valBytes) as WireValue;
    }

    result[name] = value;
  }
  return result;
}

/** Read a msgpack map header length from `bytes` at `cursor.pos`. */
function readMapLen(bytes: Uint8Array, cursor: { pos: number }): number {
  const marker = bytes[cursor.pos++];
  if ((marker & 0xF0) === 0x80) return marker & 0x0F; // FixMap
  if (marker === 0xde) {
    const n = (bytes[cursor.pos] << 8) | bytes[cursor.pos + 1];
    cursor.pos += 2;
    return n;
  }
  if (marker === 0xdf) {
    const n =
      (bytes[cursor.pos] << 24) |
      (bytes[cursor.pos + 1] << 16) |
      (bytes[cursor.pos + 2] << 8) |
      bytes[cursor.pos + 3];
    cursor.pos += 4;
    return n;
  }
  throw new Error(`de-intern: expected map marker, got 0x${marker.toString(16)}`);
}

/** Check if a msgpack marker is a map marker. */
function isMapMarker(marker: number): boolean {
  return (marker & 0xF0) === 0x80 || marker === 0xde || marker === 0xdf;
}

/**
 * Skip a single msgpack value in `bytes` starting at `cursor.pos`,
 * advancing the cursor past the entire value (including nested structures).
 */
function skipMsgpackValue(bytes: Uint8Array, cursor: { pos: number }): void {
  const marker = bytes[cursor.pos++];

  // Positive fixint / negative fixint
  if (marker <= 0x7f || marker >= 0xe0) return;

  // FixStr (0xa0-0xbf)
  if ((marker & 0xe0) === 0xa0) {
    cursor.pos += marker & 0x1f;
    return;
  }

  // FixMap (0x80-0x8f)
  if ((marker & 0xf0) === 0x80) {
    const count = marker & 0x0f;
    for (let i = 0; i < count; i++) {
      skipMsgpackValue(bytes, cursor); // key
      skipMsgpackValue(bytes, cursor); // value
    }
    return;
  }

  // FixArray (0x90-0x9f)
  if ((marker & 0xf0) === 0x90) {
    const count = marker & 0x0f;
    for (let i = 0; i < count; i++) {
      skipMsgpackValue(bytes, cursor);
    }
    return;
  }

  switch (marker) {
    case 0xc0: // nil
    case 0xc2: // false
    case 0xc3: // true
      return;
    case 0xc4: // bin8
      cursor.pos += bytes[cursor.pos] + 1;
      return;
    case 0xc5: // bin16
      cursor.pos += ((bytes[cursor.pos] << 8) | bytes[cursor.pos + 1]) + 2;
      return;
    case 0xc6: // bin32
      cursor.pos +=
        ((bytes[cursor.pos] << 24) |
          (bytes[cursor.pos + 1] << 16) |
          (bytes[cursor.pos + 2] << 8) |
          bytes[cursor.pos + 3]) +
        4;
      return;
    case 0xca: // float32
      cursor.pos += 4;
      return;
    case 0xcb: // float64
      cursor.pos += 8;
      return;
    case 0xcc: // uint8
      cursor.pos += 1;
      return;
    case 0xcd: // uint16
      cursor.pos += 2;
      return;
    case 0xce: // uint32
      cursor.pos += 4;
      return;
    case 0xcf: // uint64
      cursor.pos += 8;
      return;
    case 0xd0: // int8
      cursor.pos += 1;
      return;
    case 0xd1: // int16
      cursor.pos += 2;
      return;
    case 0xd2: // int32
      cursor.pos += 4;
      return;
    case 0xd3: // int64
      cursor.pos += 8;
      return;
    case 0xd9: // str8
      cursor.pos += bytes[cursor.pos] + 1;
      return;
    case 0xda: // str16
      cursor.pos += ((bytes[cursor.pos] << 8) | bytes[cursor.pos + 1]) + 2;
      return;
    case 0xdb: // str32
      cursor.pos +=
        ((bytes[cursor.pos] << 24) |
          (bytes[cursor.pos + 1] << 16) |
          (bytes[cursor.pos + 2] << 8) |
          bytes[cursor.pos + 3]) +
        4;
      return;
    case 0xdc: { // array16
      const count = (bytes[cursor.pos] << 8) | bytes[cursor.pos + 1];
      cursor.pos += 2;
      for (let i = 0; i < count; i++) skipMsgpackValue(bytes, cursor);
      return;
    }
    case 0xdd: { // array32
      const count =
        (bytes[cursor.pos] << 24) |
        (bytes[cursor.pos + 1] << 16) |
        (bytes[cursor.pos + 2] << 8) |
        bytes[cursor.pos + 3];
      cursor.pos += 4;
      for (let i = 0; i < count; i++) skipMsgpackValue(bytes, cursor);
      return;
    }
    case 0xde: { // map16
      const count = (bytes[cursor.pos] << 8) | bytes[cursor.pos + 1];
      cursor.pos += 2;
      for (let i = 0; i < count; i++) {
        skipMsgpackValue(bytes, cursor);
        skipMsgpackValue(bytes, cursor);
      }
      return;
    }
    case 0xdf: { // map32
      const count =
        (bytes[cursor.pos] << 24) |
        (bytes[cursor.pos + 1] << 16) |
        (bytes[cursor.pos + 2] << 8) |
        bytes[cursor.pos + 3];
      cursor.pos += 4;
      for (let i = 0; i < count; i++) {
        skipMsgpackValue(bytes, cursor);
        skipMsgpackValue(bytes, cursor);
      }
      return;
    }
    // fixext 1/2/4/8/16
    case 0xd4: cursor.pos += 2; return;
    case 0xd5: cursor.pos += 3; return;
    case 0xd6: cursor.pos += 5; return;
    case 0xd7: cursor.pos += 9; return;
    case 0xd8: cursor.pos += 17; return;
    // ext8/16/32
    case 0xc7: cursor.pos += bytes[cursor.pos] + 2; return;
    case 0xc8: cursor.pos += ((bytes[cursor.pos] << 8) | bytes[cursor.pos + 1]) + 3; return;
    case 0xc9:
      cursor.pos +=
        ((bytes[cursor.pos] << 24) |
          (bytes[cursor.pos + 1] << 16) |
          (bytes[cursor.pos + 2] << 8) |
          bytes[cursor.pos + 3]) +
        5;
      return;
    default:
      throw new Error(`de-intern: unknown msgpack marker 0x${marker.toString(16)}`);
  }
}

/** Try to resolve an interner id to a name using any of the repos' FieldMaps. */
function resolveIdFromRepos(
  registry: InternerCacheRegistry,
  db: string,
  id: bigint,
  repos: string[],
): string | undefined {
  for (const repo of repos) {
    const fm = registry.getOrCreate(db, repo);
    const name = fm.getName(id);
    if (name !== undefined) return name;
  }
  return undefined;
}
