/**
 * Typed index-constructor validation tests (F-8, #1075).
 *
 * The 6 typed constructors `hashIndex`, `uniqueIndex`, `sortedIndex`,
 * `sortedWithIncludeIndex`, `ftsIndex`, `functionalIndex` previously built
 * their `CreateIndexOp` directly with NO `fields.length === 0` check — unlike
 * `createIndex()` (the legacy path, ddl.ts:203-209) which does check this.
 * `vectorIndex` already validated `dim <= 0` and is the template.
 *
 * These tests prove the empty-fields check throws for all 7 typed constructors,
 * and that the happy path (non-empty fields) still produces a valid op.
 */

import { describe, it, expect } from 'vitest';
import {
  hashIndex,
  uniqueIndex,
  sortedIndex,
  sortedWithIncludeIndex,
  ftsIndex,
  functionalIndex,
  vectorIndex,
} from '../ddl.js';

// ── Empty-fields rejection ──────────────────────────────────────────

describe('typed index constructors reject empty fields', () => {
  it('hashIndex throws on empty fields', () => {
    expect(() => hashIndex('idx', 'users', [])).toThrow(/at least one field/);
  });

  it('uniqueIndex throws on empty fields', () => {
    expect(() => uniqueIndex('idx', 'users', [])).toThrow(/at least one field/);
  });

  it('sortedIndex throws on empty field path', () => {
    expect(() => sortedIndex('idx', 'users', [])).toThrow(/at least one field/);
  });

  it('sortedWithIncludeIndex throws on empty field path', () => {
    expect(() =>
      sortedWithIncludeIndex('idx', 'users', [], [['email']]),
    ).toThrow(/at least one field/);
  });

  it('ftsIndex throws on empty field path', () => {
    expect(() => ftsIndex('idx', 'posts', [], 'whitespace')).toThrow(
      /at least one field/,
    );
  });

  it('functionalIndex throws on empty field path', () => {
    expect(() => functionalIndex('idx', 'users', [], 'lower')).toThrow(
      /at least one field/,
    );
  });
});

// ── Happy path (non-empty fields succeed) ───────────────────────────

describe('typed index constructors succeed with non-empty fields', () => {
  it('hashIndex builds with one field', () => {
    const op = hashIndex('idx', 'users', [['email']]);
    expect(op.create_index).toBe('idx');
    expect(op.fields).toEqual([['email']]);
    expect(op.unique).toBe(false);
  });

  it('uniqueIndex builds with one field', () => {
    const op = uniqueIndex('idx', 'users', [['email']]);
    expect(op.unique).toBe(true);
  });

  it('sortedIndex builds with one field', () => {
    const op = sortedIndex('idx', 'users', ['age']);
    expect(op.sorted).toBe(true);
    expect(op.fields).toEqual([['age']]);
  });

  it('sortedWithIncludeIndex builds with one field + include', () => {
    const op = sortedWithIncludeIndex('idx', 'users', ['age'], [['email']]);
    expect(op.sorted).toBe(true);
    expect(op.include).toEqual([['email']]);
  });

  it('ftsIndex builds with one field', () => {
    const op = ftsIndex('idx', 'posts', ['body'], 'whitespace');
    expect(op.index_type).toBe('fts');
    expect(op.fts_tokenizer).toBe('whitespace');
  });

  it('functionalIndex builds with one field', () => {
    const op = functionalIndex('idx', 'users', ['email'], 'lower');
    expect(op.index_type).toBe('functional');
    expect(op.functional_op).toBe('lower');
  });

  it('vectorIndex builds with one field', () => {
    const op = vectorIndex('idx', 'docs', ['embedding'], 384, 'cosine', undefined);
    expect(op.index_type).toBe('vector');
    expect(op.vector_dim).toBe(384);
  });
});
