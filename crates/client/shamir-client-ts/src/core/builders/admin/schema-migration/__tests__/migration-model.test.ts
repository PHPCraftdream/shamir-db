/**
 * R1c follow-up regression tests (release-1 R1c 3b), findings 1 & 3 plus
 * the provenance/`__proto__` follow-up round:
 *   - The `createIndex` WRAPPER ITSELF throws typed
 *     `MigrationError('invalid_set')` when `functional_op`/`functional_args`
 *     are given for a non-functional (or omitted) `index_type`; the admitted
 *     functional-op allowlist is exactly `lower | upper | trim | length`
 *     (what the server's index backend builds directly); and ANY supplied
 *     `functional_args` — including `[]` — is refused because the server's
 *     functional-index backend never reads them.
 *   - Builder provenance is asserted BEFORE any semantic read at BOTH
 *     authoring boundaries (`schemaMigration` and the direct
 *     `ApplicationMigrationSet` constructor): hand-built / spread ops fail
 *     with the provenance diagnostic regardless of their semantics; a
 *     GENUINE builder op MUTATED in place afterwards still fails with the
 *     real semantic diagnostic (mutation-based semantic regression). A
 *     non-`MigrationError` escaping `validateOp` (e.g. a deleted
 *     `check_function` spec) is wrapped as a typed `invalid_set` carrying
 *     the ZERO-BASED operation index; an explicit unminted op and a
 *     THROWING payload accessor are both refused at both boundaries.
 *   - An OWN enumerable `__proto__` key at any nesting of the migration
 *     body or its function dependencies is refused (`invalid_set`, reason
 *     `would not survive the wire`); a marker-free plain map passes.
 *
 * Every failure path is LOCAL (typed, pre-encode/pre-send): no client and no
 * fixture transport. Positive cases only mint and clone in-memory bodies.
 */

import { describe, it, expect } from 'vitest';

import { migrationDdl, schemaMigration } from '../model/index.js';
import { ApplicationMigrationSet, MigrationError } from '../index.js';
import type {
  SchemaMigrationOp,
  SchemaMigrationBody,
  SchemaMigrationFunctionDependencies,
  MigrationCreateIndexPayload,
  MigrationFieldRuleDto,
  MigrationLiteral,
} from '../../../../types/schema/migration-body.js';

// ── local helpers (pure, no client, no fixture) ─────────────────────

/** Narrow a union op to its `create_index` payload without casts. */
function createIndexPayload(op: SchemaMigrationOp): MigrationCreateIndexPayload {
  if (op.operation !== 'create_index') throw new Error('expected a create_index op');
  return op.payload;
}

/** Expect `fn` to throw a typed `MigrationError` with the given local code. */
function expectInvalidSet(fn: () => unknown, messagePattern?: RegExp): void {
  let caught: unknown;
  try {
    fn();
  } catch (error) {
    caught = error;
  }
  expect(caught).toBeInstanceOf(MigrationError);
  const error = caught as MigrationError;
  expect(error.code).toBe('invalid_set');
  expect(error.retryable).toBe(false);
  if (messagePattern !== undefined) expect(error.message).toMatch(messagePattern);
}

/** A minimal valid functional (lower, no args) body at a given ordinal. */
function lowerBody(id: string, ordinal: number): SchemaMigrationBody {
  return schemaMigration(
    id,
    ordinal,
    `lower ${id}`,
    [migrationDdl.createIndex(`idx_${id}`, 't', [['col']], { index_type: 'functional', functional_op: 'lower' })],
  );
}

/** A hand-built raw `create_index` op with the genuine DTO keys. */
function rawCreateIndexOp(payload: Partial<MigrationCreateIndexPayload>): SchemaMigrationOp {
  return {
    operation: 'create_index',
    payload: {
      create_index: 'idx',
      table: 't',
      fields: [['col']],
      unique: false,
      sorted: false,
      repo: 'main',
      ...payload,
    },
  } satisfies SchemaMigrationOp;
}

function rawBody(operations: SchemaMigrationOp[]): SchemaMigrationBody {
  return {
    metadata: { id: 'raw', ordinal: 1n, display_name: 'raw', canonical_version: 1 },
    operations,
  };
}

/** A functional-index body factory with typed migration-literal args. */
function functionalBody(
  functional_op: string,
  functional_args?: MigrationLiteral[],
): () => SchemaMigrationBody {
  return () =>
    schemaMigration('m1', 1n, 'arity', [
      migrationDdl.createIndex('idx', 't', [['col']], {
        index_type: 'functional',
        functional_op,
        ...(functional_args !== undefined ? { functional_args } : {}),
      }),
    ]);
}

// ── finding 1: the wrapper itself refuses non-functional context ────

describe('finding 1 — wrapper throws on btree + functional_op/functional_args', () => {
  it('btree + functional_args is a typed invalid_set thrown by the wrapper call', () => {
    expectInvalidSet(
      () => migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'btree', functional_args: [] }),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
  });

  it('btree + functional_op is the same typed refusal at the wrapper call', () => {
    expectInvalidSet(
      () => migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'btree', functional_op: 'lower' }),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
  });
});

// ── finding 1: raw DTO semantics at both authoring boundaries ───────

describe('finding 1 — raw and spread inputs at the authoring boundaries (provenance FIRST)', () => {
  it('raw btree + functional_args fails with the PROVENANCE diagnostic before any semantic read', () => {
    const op = rawCreateIndexOp({ index_type: 'btree', functional_args: [] });
    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'bad context', [op]),
      /operation was not produced by migrationDdl/,
    );
    expectInvalidSet(
      () => new ApplicationMigrationSet([rawBody([op])]),
      /operation was not produced by migrationDdl/,
    );
  });

  it('spread of a genuine op with args added fails with the PROVENANCE diagnostic at both boundaries', () => {
    const valid = migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional', functional_op: 'lower' });
    const negative: SchemaMigrationOp = {
      operation: 'create_index',
      payload: { ...createIndexPayload(valid), index_type: 'btree', functional_args: [] },
    };
    expectInvalidSet(() => schemaMigration('m1', 1n, 'spread', [negative]), /not produced by migrationDdl/);
    expectInvalidSet(() => new ApplicationMigrationSet([rawBody([negative])]), /not produced by migrationDdl/);
  });

  it('a GENUINE btree op mutated in place still fails with the ORIGINAL semantic diagnostic', () => {
    // Minted valid, then mutated AFTER minting: provenance holds, semantics
    // must still be checked and reject the mutation (with the migration
    // diagnostic, not the plain builder's).
    const genuine = migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional', functional_op: 'lower' });
    createIndexPayload(genuine).index_type = 'btree';
    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'mutated btree', [genuine]),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
    expectInvalidSet(
      () => new ApplicationMigrationSet([rawBody([genuine])]),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
  });

  it('a semantically VALID hand-built op still fails with the provenance diagnostic', () => {
    const op = rawCreateIndexOp({ index_type: 'functional', functional_op: 'lower' });
    expectInvalidSet(() => schemaMigration('m1', 1n, 'hand-built', [op]), /operation was not produced by migrationDdl/);
    expectInvalidSet(() => new ApplicationMigrationSet([rawBody([op])]), /operation was not produced by migrationDdl/);
  });

  it('an exact spread of a genuine valid op is refused with the provenance diagnostic', () => {
    const genuine = migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional', functional_op: 'lower' });
    const spread: SchemaMigrationOp = { ...genuine };
    expectInvalidSet(() => schemaMigration('m1', 1n, 'spread', [spread]), /not produced by migrationDdl/);
    expectInvalidSet(() => new ApplicationMigrationSet([rawBody([spread])]), /not produced by migrationDdl/);
  });
});

// ── finding 1: functional-index admission matrix ────────────────────

describe('finding 1 — functional-index admission matrix (wrapper-level)', () => {
  it('each ADMITTED builtin (lower/upper/trim/length) succeeds with no args', () => {
    for (const op of ['lower', 'upper', 'trim', 'length'] as const) {
      const payload = createIndexPayload(functionalBody(op)().operations[0]);
      expect(payload.functional_op).toBe(op);
      expect(payload.functional_args).toBeUndefined();
    }
  });

  it('substring/mod/coalesce/concat are OUTSIDE the migration allowlist', () => {
    for (const op of ['substring', 'mod', 'coalesce', 'concat'] as const) {
      expectInvalidSet(
        functionalBody(op),
        /index function is outside the built-in functional-index allowlist/,
      );
      // Even with args supplied, the allowlist diagnostic wins (Rust order).
      expectInvalidSet(
        functionalBody(op, [1]),
        /index function is outside the built-in functional-index allowlist/,
      );
    }
  });

  it('ANY supplied functional_args — including [] — is refused, on every admitted op', () => {
    for (const op of ['lower', 'upper', 'trim', 'length'] as const) {
      expectInvalidSet(
        functionalBody(op, []),
        /functional_args are not admitted because functional indexes do not use them/,
      );
      expectInvalidSet(
        functionalBody(op, ['x']),
        /functional_args are not admitted because functional indexes do not use them/,
      );
    }
  });

  it('functional options on ANY non-functional family are refused with the builder-meaning diagnostic', () => {
    for (const index_type of ['btree', 'fts', 'vector'] as const) {
      expectInvalidSet(
        () => migrationDdl.createIndex('idx', 't', [['col']], { index_type, functional_op: 'lower' }),
        /functional_op\/functional_args are only valid for 'functional' indexes/,
      );
      expectInvalidSet(
        () => migrationDdl.createIndex('idx', 't', [['col']], { index_type, functional_args: [] }),
        /functional_op\/functional_args are only valid for 'functional' indexes/,
      );
    }
  });

  it('the functional guard precedes the plain builder: a vector + functional_op case WITH a valid vector_dim still gets the migration diagnostic', () => {
    // The plain builder checks the vector family (dim) FIRST when it runs;
    // the wrapper runs the functional-option guards before it, so the
    // functional-family diagnostic wins even though the vector options are
    // internally consistent.
    expectInvalidSet(
      () => migrationDdl.createIndex('idx', 't', [['col']], {
        index_type: 'vector',
        vector_dim: 4,
        vector_metric: 'l2',
        functional_op: 'lower',
      }),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
  });

  it('functional options with the index_type OMITTED are refused identically', () => {
    expectInvalidSet(
      () => migrationDdl.createIndex('idx', 't', [['col']], { functional_op: 'lower' }),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
    expectInvalidSet(
      () => migrationDdl.createIndex('idx', 't', [['col']], { functional_args: [] }),
      /functional_op\/functional_args are only valid for 'functional' indexes/,
    );
  });

  it('functional index without an explicit op is refused with the explicit-op diagnostic', () => {
    expectInvalidSet(
      () => migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional' }),
      /functional index requires an explicit built-in operation/,
    );
  });

  it('a GENUINE admitted op MUTATED to add functional_args still fails with the real diagnostic', () => {
    // Both authoring boundaries validate the mutation BEFORE any canonical
    // encoding: the failure is the typed `invalid_set`, never a canonical
    // encode error, so the encoder is never reached with refused args.
    const genuine = migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional', functional_op: 'lower' });
    createIndexPayload(genuine).functional_args = [];
    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'mutated args', [genuine]),
      /functional_args are not admitted because functional indexes do not use them/,
    );
    expectInvalidSet(
      () => new ApplicationMigrationSet([rawBody([genuine])]),
      /functional_args are not admitted because functional indexes do not use them/,
    );
  });

  it('lower with no extras succeeds and keeps functional_args off the payload (control)', () => {
    const payload = createIndexPayload(lowerBody('lower-ok', 1).operations[0]);
    expect(payload.functional_op).toBe('lower');
    expect(payload.functional_args).toBeUndefined();
  });
});

// ── finding 3: sorted indexes are single-field only ─────────────────

describe('finding 3 — sorted index over multiple field paths (raw DTO provenance-first)', () => {
  const multi = rawCreateIndexOp({ fields: [['a'], ['b']], sorted: true });

  it('raw sorted multi-field is refused with the PROVENANCE diagnostic by schemaMigration', () => {
    expectInvalidSet(() => schemaMigration('m1', 1n, 'sorted multi', [multi]), /operation was not produced by migrationDdl/);
  });

  it('raw sorted multi-field is refused with the PROVENANCE diagnostic by direct set construction', () => {
    expectInvalidSet(() => new ApplicationMigrationSet([rawBody([multi])]), /operation was not produced by migrationDdl/);
  });

  it('a GENUINE sorted op mutated to two fields still fails with the ORIGINAL semantic diagnostic', () => {
    const genuine = migrationDdl.createIndex('idx', 't', [['a']], { sorted: true });
    createIndexPayload(genuine).fields.push(['b']);
    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'mutated sorted', [genuine]),
      /a sorted index requires exactly one field, got 2/,
    );
    expectInvalidSet(
      () => new ApplicationMigrationSet([rawBody([genuine])]),
      /a sorted index requires exactly one field, got 2/,
    );
  });

  it('sorted with a single field path is accepted (wrapper-minted control)', () => {
    expect(() =>
      schemaMigration('m1', 1n, 'sorted single', [
        migrationDdl.createIndex('idx', 't', [['a']], { sorted: true }),
      ]),
    ).not.toThrow();
  });
});

// ── follow-up: provenance BEFORE semantic reads; wrapped validateOp faults ──

describe('follow-up — provenance precedes semantic reads at both boundaries', () => {
  it('an explicit UNMINTED check_function op {operation, payload:{}} fails provenance at both boundaries', () => {
    const op = { operation: 'check_function', payload: {} } as unknown as SchemaMigrationOp;

    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'unminted check', [op]),
      /operation was not produced by migrationDdl/,
    );
    expectInvalidSet(
      () => new ApplicationMigrationSet([rawBody([op])]),
      /operation was not produced by migrationDdl/,
    );
  });

  it('a hand-built check_function with a THROWING payload getter is refused by provenance without reading the payload', () => {
    let getterRead = false;
    const payload: Record<string, unknown> = {};
    Object.defineProperty(payload, 'check_function', {
      get() {
        getterRead = true;
        throw new Error('hostile accessor');
      },
      enumerable: true,
      configurable: true,
    });
    const op = { operation: 'check_function', payload } as unknown as SchemaMigrationOp;

    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'hand-built check', [op]),
      /operation was not produced by migrationDdl/,
    );
    expect(getterRead).toBe(false);

    expectInvalidSet(
      () => new ApplicationMigrationSet([rawBody([op])]),
      /operation was not produced by migrationDdl/,
    );
    expect(getterRead).toBe(false);
  });

  it('a GENUINE checkFunction with its check_function spec deleted fails invalid_set with the zero-based index', () => {
    const minted = migrationDdl.checkFunction({ source: 'fn main() {}' });
    Reflect.deleteProperty(minted.payload, 'check_function');

    expectInvalidSet(() => schemaMigration('ck', 1n, 'deleted spec', [minted]), /operation 0/);
    expectInvalidSet(() => new ApplicationMigrationSet([rawBody([minted])]), /operation 0/);
  });
});

// ── follow-up: own enumerable __proto__ keys are refused ────────────

describe('follow-up — own __proto__ map anywhere in the body is refused', () => {
  /** A plain-object map carrying an OWN enumerable `__proto__` key (computed key). */
  function mapWithOwnProto(): Record<string, unknown> {
    const inner: Record<string, unknown> = { plain: [1, 'x'] };
    return { nested: { ['__proto__']: inner } };
  }

  it('an own __proto__ nested in a one_of map is refused by schemaMigration with the wire reason', () => {
    const rule: MigrationFieldRuleDto = {
      path: ['col'],
      type: 'int',
      one_of: [mapWithOwnProto() as unknown as MigrationLiteral],
    };
    expectInvalidSet(
      () => schemaMigration('m1', 1n, 'proto body', [migrationDdl.setTableSchema('t', [rule])]),
      /would not survive the wire/,
    );
  });

  it('an own __proto__ nested in a one_of map is refused by direct set construction', () => {
    const rule: MigrationFieldRuleDto = {
      path: ['col'],
      type: 'int',
      one_of: [mapWithOwnProto() as unknown as MigrationLiteral],
    };
    const raw: SchemaMigrationBody = {
      metadata: { id: 'raw-proto', ordinal: 1n, display_name: 'raw-proto', canonical_version: 1 },
      operations: [migrationDdl.setTableSchema('t', [rule])],
    };
    expectInvalidSet(() => new ApplicationMigrationSet([raw]), /would not survive the wire/);
  });

  it('an own __proto__ on a function dependency entry is refused before normalization', () => {
    const dep = { function: 'f', depends_on: ['g'] } as SchemaMigrationFunctionDependencies;
    Object.defineProperty(dep, '__proto__', {
      value: { sneaky: true },
      enumerable: true,
      configurable: true,
      writable: true,
    });
    expectInvalidSet(
      () =>
        schemaMigration(
          'm1',
          1n,
          'proto dep',
          [migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional', functional_op: 'lower' })],
          [dep],
        ),
      /would not survive the wire/,
    );
  });

  it('a marker-free plain map in one_of passes both boundaries (positive control)', () => {
    const rule: MigrationFieldRuleDto = {
      path: ['col'],
      type: 'int',
      one_of: [{ nested: { plain: [1, 'x'] } } as unknown as MigrationLiteral],
    };
    expect(() =>
      schemaMigration('pos', 1n, 'clean map', [migrationDdl.setTableSchema('t', [rule])]),
    ).not.toThrow();
    expect(() =>
      new ApplicationMigrationSet([
        schemaMigration('pos', 1n, 'clean map', [migrationDdl.setTableSchema('t', [rule])]),
      ]),
    ).not.toThrow();
  });
});

// ── positive flow: clones and copies keep the checksum unchanged ────

describe('positive flow — cloned bodies / set entry copies accepted, checksum unchanged', () => {
  it('a set entry body copy is accepted by another set with an identical checksum', () => {
    const first = new ApplicationMigrationSet([lowerBody('a', 1), lowerBody('b', 2)]);
    const entries = first.entries;
    expect(entries.map((e) => e.ordinal)).toEqual([1n, 2n]);

    // The set hands out defensive copies; those copies feed a SECOND set.
    const second = new ApplicationMigrationSet(entries.map((e) => e.body));
    expect(second.length).toBe(2);

    const secondEntries = second.entries;
    for (let i = 0; i < 2; i += 1) {
      expect(Array.from(secondEntries[i].checksum)).toEqual(Array.from(entries[i].checksum));
      expect(secondEntries[i].body.metadata.id).toBe(entries[i].body.metadata.id);
    }
  });

  it('cloneMigrationBody of a validated body re-enters a set with the same checksum', () => {
    const body = schemaMigration('solo', 7n, 'solo', [
      migrationDdl.createIndex('idx', 't', [['col']], { index_type: 'functional', functional_op: 'lower' }),
    ]);
    const direct = new ApplicationMigrationSet([{ ...body, metadata: { ...body.metadata, ordinal: 1n } }]);
    const viaClone = new ApplicationMigrationSet([schemaMigration('solo', 1n, 'solo', body.operations)]);
    expect(Array.from(viaClone.entries[0].checksum)).toEqual(Array.from(direct.entries[0].checksum));
  });
});
