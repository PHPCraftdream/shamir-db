/**
 * Cross-language msgpack parity test — TS client ↔ Rust fixture.
 *
 * Asserts SEMANTIC wire parity: the TS builder produces a wire object that,
 * when msgpack-encoded and decoded, matches the Rust fixture's decoded shape.
 * Byte-IDENTICAL parity is not achievable for `f32` fields because
 * `@msgpack/msgpack` encodes integer-valued floats (1.0, 0.0, 2.0) as
 * positive-fixint, while Rust `rmp_serde` always emits float32 for `f32`-typed
 * fields. Both encodings are msgpack-valid and mutually decodable: Rust's
 * `from_slice` decodes `01` → `1.0f32`, and TS's `decode` decodes `ca3f800000`
 * → `1.0`. This test pins that mutual decodability + semantic equality.
 *
 * The authoritative wire contract is the Rust fixture
 * `crates/shamir-query-builder/tests/fixtures/vector_filter_msgpack.json`.
 *
 * See `docs/roadmap/VECTOR_PRODUCTION_EXECUTION.md` Phase P1, Sheet 1.1.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { encode, decode } from '@msgpack/msgpack';
import { vectorSimilarity } from '../filter.js';
import { ddl } from '../ddl.js';

// ── Fixture loader ──────────────────────────────────────────────────

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = resolve(
  __dirname,
  // From src/core/builders/__tests__ up to crates/, then into the sibling
  // shamir-query-builder crate. Five levels of `..`.
  '../../../../..',
  'shamir-query-builder',
  'tests',
  'fixtures',
  'vector_filter_msgpack.json',
);

/**
 * Load the Rust fixture and return only the label → hex-string entries,
 * skipping the documentation keys (`_comment`, `_key_order_note`,
 * `_value_notes`) which are not hex.
 */
function loadFixtureHex(): Record<string, string> {
  const text = readFileSync(FIXTURE_PATH, 'utf8');
  const raw = JSON.parse(text) as Record<string, unknown>;
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(raw)) {
    if (key.startsWith('_')) continue;
    if (typeof value !== 'string') continue;
    out[key] = value;
  }
  return out;
}

/** Decode a hex string to a JS object via msgpack. */
function decodeHex(hex: string): unknown {
  const bytes = Buffer.from(hex, 'hex');
  return decode(bytes);
}

/** Encode a wire object with `@msgpack/msgpack`, return lowercase hex. */
function encodeToHex(filter: unknown): string {
  const bytes = encode(filter);
  return Buffer.from(bytes).toString('hex');
}

// ── Canonical inputs (mirror Rust `canonical_filters` 1:1) ──────────

const QUERY = [1.0, 0.0, 0.5];

function canonicalTsFilters(): Array<{ label: string; filter: unknown }> {
  return [
    {
      label: 'vector_similarity_bare',
      filter: vectorSimilarity('emb', QUERY, 10),
    },
    {
      label: 'vector_similarity_ef_search',
      filter: vectorSimilarity('emb', QUERY, 10, { efSearch: 400 }),
    },
    {
      label: 'vector_similarity_ef_and_oversample',
      filter: vectorSimilarity('emb', QUERY, 10, {
        efSearch: 400,
        oversample: 2.0,
      }),
    },
  ];
}

// ── Parity assertions ───────────────────────────────────────────────

describe('TS↔Rust VectorSimilarity msgpack parity (V1.1)', () => {
  const fixture = loadFixtureHex();

  it('fixture pins exactly 3 shapes', () => {
    expect(Object.keys(fixture).sort()).toEqual(
      [
        'vector_similarity_bare',
        'vector_similarity_ef_search',
        'vector_similarity_ef_and_oversample',
      ].sort(),
    );
  });

  // Semantic parity: decode the Rust fixture bytes and the TS-built filter
  // bytes, then deep-compare the decoded objects. This proves mutual
  // decodability + semantic equality without requiring byte-identical
  // encoding (which is impossible for f32 integer-valued floats — see the
  // file-level doc comment).
  for (const { label, filter } of canonicalTsFilters()) {
    it(`TS filter matches Rust fixture decode for ${label}`, () => {
      const expectedHex = fixture[label];
      expect(expectedHex, `fixture missing entry for \`${label}\``).toBeTypeOf(
        'string',
      );
      const rustDecoded = decodeHex(expectedHex);
      const tsBytes = encode(filter);
      const tsDecoded = decode(tsBytes);
      expect(tsDecoded).toEqual(rustDecoded);
    });
  }

  // The Rust fixture bytes must be valid msgpack that the TS decoder can
  // read (proving the wire format the Rust server sends is TS-parseable).
  it('Rust fixture bytes are valid msgpack (TS decode)', () => {
    for (const [label, hex] of Object.entries(fixture)) {
      expect(() => decodeHex(hex), `decode ${label}`).not.toThrow();
    }
  });

  it('bare shape omits ef_search and oversample keys from wire', () => {
    const bare = vectorSimilarity('v', [0.0], 1);
    const hex = encodeToHex(bare);
    // ef_search key (0x65665f736561726368 = "ef_search") must NOT appear.
    expect(hex).not.toContain('65665f736561726368');
    // oversample key (0x6f76657273616d706c65 = "oversample") must NOT appear.
    expect(hex).not.toContain('6f76657273616d706c65');
  });

  it('TS efSearch shape contains ef_search key on wire', () => {
    const f = vectorSimilarity('emb', QUERY, 10, { efSearch: 400 });
    const hex = encodeToHex(f);
    expect(hex).toContain('65665f736561726368'); // "ef_search"
    expect(hex).not.toContain('6f76657273616d706c65'); // no "oversample"
  });

  it('TS oversample shape contains oversample key on wire', () => {
    const f = vectorSimilarity('emb', QUERY, 10, { oversample: 2.0 });
    const hex = encodeToHex(f);
    expect(hex).not.toContain('65665f736561726368'); // no "ef_search"
    expect(hex).toContain('6f76657273616d706c65'); // "oversample"
  });
});

// ── V5.2 #411 — CreateIndexOp.vector_quantization wire parity ───────
//
// The Rust wire contract is `CreateIndexOp::vector_quantization:
// Option<String>` with `#[serde(default, skip_serializing_if =
// "Option::is_none")]` (index_ops.rs:65-70). There is no Rust msgpack
// fixture for DDL ops (the fixture above covers only VectorSimilarity
// filters), so we assert the documented contract directly: the TS op
// must (a) carry `vector_quantization` as a string key+value when set,
// (b) OMIT it entirely when unset (serde skip_serializing_if parity —
// back-compat with pre-#411 servers), and (c) round-trip through msgpack
// preserving both properties.

describe('TS↔Rust CreateIndexOp.vector_quantization wire parity (V5.2 #411)', () => {
  it('sq8 op carries vector_quantization:"sq8" as a string on the wire', () => {
    const op = ddl.createIndex('vidx_q', 'docs', [['embedding']], {
      index_type: 'vector',
      vector_dim: 128,
      vector_metric: 'cosine',
      vector_quantization: 'sq8',
    });
    const hex = encodeToHex(op);
    // msgpack key "vector_quantization" (0x766563746f725f7175616e74697a6174696f6e)
    // must appear, followed by a fixstr value "sq8" (0xa3737138).
    expect(hex).toContain('766563746f725f7175616e74697a6174696f6e');
    expect(hex).toContain('a3737138'); // fixstr(3) "sq8"
    // Round-trip: decoded form preserves key+value.
    const decoded = decode(encode(op)) as Record<string, unknown>;
    expect(decoded.vector_quantization).toBe('sq8');
    expect(typeof decoded.vector_quantization).toBe('string');
  });

  it('omits vector_quantization key entirely when unset (skip_serializing_if parity)', () => {
    const op = ddl.createIndex('vidx_plain', 'docs', [['embedding']], {
      index_type: 'vector',
      vector_dim: 128,
      vector_metric: 'cosine',
    });
    const hex = encodeToHex(op);
    // The key must NOT appear anywhere in the encoded bytes.
    expect(hex).not.toContain('766563746f725f7175616e74697a6174696f6e');
    // And the decoded object must not have the property (undefined, not null).
    const decoded = decode(encode(op)) as Record<string, unknown>;
    expect(decoded).not.toHaveProperty('vector_quantization');
    expect(decoded.vector_quantization).toBeUndefined();
  });
});
