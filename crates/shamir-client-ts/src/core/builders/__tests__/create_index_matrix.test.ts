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
  // A plain case-COUNT check (`rejectCases.length >= 12`) cannot actually
  // prove per-variant coverage: deleting the sole case for one Rust
  // CreateIndexBuildError variant would still pass at 12+ cases as long as
  // some OTHER variant had picked up a second case (exactly what #1004's
  // VectorDimRequired boundary-value pair did) — an `@oh` review caught the
  // Rust mirror of this exact gap. TS has no typed error enum to match
  // against (createIndex() just throws a plain Error), so this checks
  // coverage by CASE NAME instead — each of these canonical names is this
  // fixture's own established one-name-per-variant convention (see the
  // Rust-side `variant_tag` function in create_index_matrix.rs for the
  // authoritative variant list this must stay in sync with).

  it('has a reject case for every CreateIndexBuildError variant, by canonical name', () => {
    const rejectNames = new Set(rejectCases.map((c) => c.name));
    const expectedCanonicalNames = [
      'unique_and_sorted_rejected',
      'include_without_sorted_rejected',
      'sorted_multi_field_rejected',
      'empty_fields_rejected',
      'unique_unsupported_for_type_rejected',
      'sorted_unsupported_for_type_rejected',
      'vector_dim_required_rejected', // or vector_dim_zero_rejected — either proves VectorDimRequired
      'unknown_vector_metric_rejected',
      'vector_options_on_non_vector_rejected',
      'fts_options_on_non_fts_rejected',
      'functional_options_on_non_functional_rejected',
      'include_unsupported_for_type_rejected',
    ];
    for (const name of expectedCanonicalNames) {
      expect(rejectNames.has(name)).toBe(true);
    }
  });

  it('has at least 9 accept cases (6 original + 3 additional)', () => {
    expect(acceptCases.length).toBeGreaterThanOrEqual(9);
  });
});

/** Escape a string for use inside a RegExp (for reason_contains matching). */
function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
