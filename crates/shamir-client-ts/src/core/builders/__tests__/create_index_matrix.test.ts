/**
 * Matrix-driven CREATE INDEX validation + wire-parity test (TS half of #998).
 *
 * Reads the SAME fixture file as the Rust test
 * (`crates/shamir-query-builder/tests/fixtures/create_index_matrix.json`),
 * drives `createIndex()` for each case, and asserts:
 *
 * - **accept** cases: `createIndex()` does NOT throw, and the msgpack-encoded
 *   wire bytes match the `wire_hex` captured in the fixture (byte-identical
 *   parity with Rust's `rmp_serde::to_vec_named`).
 * - **reject** cases: `createIndex()` throws synchronously, and the error
 *   message contains the `reason_contains` substring (case-insensitive).
 *
 * The fixture is the single source of truth for both toolchains — adding a case
 * there automatically extends both the Rust and TS test surfaces.
 *
 * Byte-identical hex parity is achievable for CreateIndex (no f32 fields):
 * `@msgpack/msgpack.encode` preserves JS object insertion order, and the TS
 * builder (`ddl.ts createIndex`) inserts keys in the same order as the Rust
 * `CreateIndexOp` struct declaration. See `_key_order_note` in the fixture.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { encode } from '@msgpack/msgpack';
import { createIndex } from '../ddl.js';
import type { CreateIndexOp } from '../../types/ddl.js';
import type { WireValue } from '../../types/write.js';

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
  'create_index_matrix.json',
);

// ── Types (mirror the fixture schema) ───────────────────────────────

interface CaseInput {
  name: string;
  table: string;
  fields: string[][];
  unique?: boolean;
  sorted?: boolean;
  repo?: string;
  index_type?: string;
  fts_tokenizer?: string;
  fts_language?: string;
  functional_op?: string;
  functional_args?: WireValue[];
  vector_dim?: number;
  vector_metric?: string;
  vector_quantization?: string;
  include?: string[][];
  if_not_exists?: boolean;
}

interface AcceptCase {
  name: string;
  input: CaseInput;
  expect: 'accept';
  wire_hex: string;
}

interface RejectCase {
  name: string;
  input: CaseInput;
  expect: 'reject';
  reason_contains: string;
}

type MatrixCase = AcceptCase | RejectCase;

interface Fixture {
  _comment?: string;
  _key_order_note?: string;
  _value_notes?: unknown;
  _consumer_notes?: unknown;
  cases: MatrixCase[];
}

function loadFixture(): Fixture {
  const text = readFileSync(FIXTURE_PATH, 'utf8');
  return JSON.parse(text) as Fixture;
}

// ── Helpers ─────────────────────────────────────────────────────────

/** Encode a wire object with `@msgpack/msgpack` and return lowercase hex. */
function encodeToHex(op: unknown): string {
  const bytes = encode(op);
  return Buffer.from(bytes).toString('hex');
}

/** Map a fixture CaseInput to the TS createIndex() call, return the wire op. */
function buildOp(input: CaseInput): CreateIndexOp {
  const opts: Record<string, unknown> = {};
  if (input.unique !== undefined) opts.unique = input.unique;
  if (input.sorted !== undefined) opts.sorted = input.sorted;
  if (input.repo !== undefined) opts.repo = input.repo;
  if (input.index_type !== undefined) opts.index_type = input.index_type;
  if (input.fts_tokenizer !== undefined) opts.fts_tokenizer = input.fts_tokenizer;
  if (input.fts_language !== undefined) opts.fts_language = input.fts_language;
  if (input.functional_op !== undefined) opts.functional_op = input.functional_op;
  if (input.functional_args !== undefined) opts.functional_args = input.functional_args;
  if (input.vector_dim !== undefined) opts.vector_dim = input.vector_dim;
  if (input.vector_metric !== undefined) opts.vector_metric = input.vector_metric;
  if (input.vector_quantization !== undefined)
    opts.vector_quantization = input.vector_quantization;
  if (input.include !== undefined) opts.include = input.include;
  if (input.if_not_exists !== undefined) opts.if_not_exists = input.if_not_exists;
  return createIndex(
    input.name,
    input.table,
    input.fields,
    Object.keys(opts).length > 0 ? opts : undefined,
  );
}

// ── Tests ───────────────────────────────────────────────────────────

describe('create_index_matrix (shared fixture)', () => {
  const fixture = loadFixture();
  const acceptCases = fixture.cases.filter(
    (c): c is AcceptCase => c.expect === 'accept',
  );
  const rejectCases = fixture.cases.filter(
    (c): c is RejectCase => c.expect === 'reject',
  );

  // ── Accept cases: build succeeds + wire hex matches ──────────────────

  describe('accept cases', () => {
    for (const c of acceptCases) {
      it(`${c.name}: builds + wire hex matches Rust fixture`, () => {
        const op = buildOp(c.input);
        const actualHex = encodeToHex(op);
        expect(actualHex).toBe(c.wire_hex);
      });
    }
  });

  // ── Reject cases: throws + reason_contains matches ───────────────────

  describe('reject cases', () => {
    for (const c of rejectCases) {
      it(`${c.name}: throws with reason containing "${c.reason_contains}"`, () => {
        expect(() => buildOp(c.input)).toThrow(
          expect.objectContaining({
            message: expect.stringMatching(
              new RegExp(escapeRegex(c.reason_contains), 'i'),
            ),
          }),
        );
      });
    }
  });

  // ── Completeness: one reject case per error variant ──────────────────
  //
  // "At least" one per variant, not "exactly" — #1004 added a second
  // VectorDimRequired case (`vector_dim_zero_rejected`, an explicit
  // `vector_dim: 0` alongside the pre-existing omitted-dim case) as a
  // boundary-value pair, so a variant can legitimately have >1 case.

  it('has at least 12 reject cases (one per CreateIndexBuildError variant)', () => {
    expect(rejectCases.length).toBeGreaterThanOrEqual(12);
  });

  it('has at least 9 accept cases (6 original + 3 additional)', () => {
    expect(acceptCases.length).toBeGreaterThanOrEqual(9);
  });
});

/** Escape a string for use inside a RegExp (for reason_contains matching). */
function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
