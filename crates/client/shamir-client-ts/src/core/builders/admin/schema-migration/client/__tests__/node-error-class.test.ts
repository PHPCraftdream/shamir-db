import { describe, it, expect } from 'vitest';

import { ApplicationMigrationClient } from '../client.js';
import { MigrationError } from '../error.js';
import { ApplicationMigrationSet } from '../../canonical/set.js';
import { schemaMigration, migrationDdl } from '../../model/index.js';

// Node-binding look-alike: same stable shape, DIFFERENT prototype (never
// `instanceof` the TS `ShamirDbError` of `support/errors.js`).
class NodeShamirDbError {
  name = 'ShamirDbError';
  constructor(
    readonly code: string,
    readonly detail?: string,
  ) {}
}

const IDENTITY = '2222222222222222';
const M1 = 'node-err-class-mig';
const STATUS_ALIAS = 'application_schema_status';
const APPLY_ALIAS = 'apply_application_schema_migration';

function emptyJournal(): Record<string, unknown> {
  return {
    protocol_version: 1,
    target: { database_identity: IDENTITY },
    last_applied_ordinal: 0,
    checksum_validation: 'valid',
    fence_generation: 0,
  };
}

type Behavior = (
  alias: string,
) => { ok: true; record: unknown } | { ok: false; error: unknown };

function transport(behavior: Behavior): {
  execute(
    db: string,
    batch: object,
  ): Promise<{ results: Record<string, { records: unknown[] }> }>;
  readonly seenAliases: string[];
} {
  const seenAliases: string[] = [];
  return {
    seenAliases,
    async execute(_db: string, batch: object) {
      const queries = (
        batch as { queries?: Record<string, unknown> } | undefined
      )?.queries;
      const aliases = Object.keys(queries ?? {});
      seenAliases.push(...aliases);
      for (const alias of aliases) {
        const outcome = behavior(alias);
        if (!outcome.ok) throw outcome.error;
        return { results: { [alias]: { records: [outcome.record] } } };
      }
      throw new Error('scripted transport received no known alias');
    },
  };
}

function makeSet(): ApplicationMigrationSet {
  return new ApplicationMigrationSet([
    schemaMigration(M1, 1n, 'create goods table', [
      migrationDdl.createTable('node_err_goods'),
    ]),
  ]);
}

async function applyError(
  behavior: Behavior,
): Promise<MigrationError | undefined> {
  const migrations = new ApplicationMigrationClient(
    transport(behavior),
    { database_identity: IDENTITY },
    makeSet(),
  );
  try {
    await migrations.applyNext('db');
  } catch (error) {
    return error as MigrationError;
  }
  return undefined;
}

async function statusError(
  behavior: Behavior,
): Promise<MigrationError | undefined> {
  const migrations = new ApplicationMigrationClient(
    transport(behavior),
    { database_identity: IDENTITY },
    makeSet(),
  );
  try {
    await migrations.status('db');
  } catch (error) {
    return error as MigrationError;
  }
  return undefined;
}

describe('definitive server rejection across ShamirDbError runtimes', () => {
  it('applyDefiniteNodeClassCodeIsTypedNotUnknown', async () => {
    const seenAliases: string[] = [];
    const caught = await applyError((alias) => {
      seenAliases.push(alias);
      return alias === APPLY_ALIAS
        ? {
            ok: false,
            error: new NodeShamirDbError(
              'schema_migration_predecessor_mismatch',
              'predecessor does not match the journal head',
            ),
          }
        : { ok: true, record: emptyJournal() };
    });
    expect(caught).toBeInstanceOf(MigrationError);
    expect(caught?.code).toBe('schema_migration_predecessor_mismatch');
    expect(caught?.serverCode).toBe('schema_migration_predecessor_mismatch');
    expect(caught?.retryable).toBe(false);
    expect(caught?.attempt).toBeUndefined();
    expect(seenAliases).toEqual([STATUS_ALIAS, APPLY_ALIAS]);
  });

  it('readNodeClassCodeIsTyped', async () => {
    const caught = await statusError(() => ({
      ok: false,
      error: new NodeShamirDbError(
        'schema_migration_durability_unavailable',
        'journal is unreadable',
      ),
    }));
    expect(caught).toBeInstanceOf(MigrationError);
    expect(caught?.code).toBe('schema_migration_durability_unavailable');
  });

  it('uncertainCodesAndNonCarriersStayUnknownOutcome', async () => {
    for (const failure of [
      new NodeShamirDbError('timeout', 'deferred'),
      new NodeShamirDbError('limits'),
      new NodeShamirDbError('result_too_large'),
      new Error('connection reset'),
      { name: 'NotShamirDbError', code: 'schema_migration_gap', detail: 'x' },
      'schema_migration_gap',
      null,
    ]) {
      const caught = await applyError((alias) =>
        alias === APPLY_ALIAS
          ? { ok: false, error: failure }
          : { ok: true, record: emptyJournal() },
      );
      expect(caught, String(failure)).toBeInstanceOf(MigrationError);
      expect((caught as MigrationError).code).toBe('unknown_outcome');
      expect((caught as MigrationError).attempt?.migrationId).toBe(M1);
    }
  });

  it('missingDetailCarrierStillDefiniteWithSafeFallback', async () => {
    const caught = await applyError((alias) =>
      alias === APPLY_ALIAS
        ? {
            ok: false,
            error: { name: 'ShamirDbError', code: 'schema_migration_gap' },
          }
        : { ok: true, record: emptyJournal() },
    );
    expect(caught).toBeInstanceOf(MigrationError);
    expect(caught?.code).toBe('schema_migration_gap');
    expect(caught?.message).not.toContain('undefined');
  });

  it('unsupportedServerMatchesExactDetailOnly', async () => {
    const unsupported = await applyError((alias) =>
      alias === APPLY_ALIAS
        ? {
            ok: false,
            error: new NodeShamirDbError(
              'invalid_request',
              'Unknown operation type',
            ),
          }
        : { ok: true, record: emptyJournal() },
    );
    expect(unsupported?.code).toBe('unsupported_server');
    expect(unsupported?.serverCode).toBe('invalid_request');

    const other = await applyError((alias) =>
      alias === APPLY_ALIAS
        ? {
            ok: false,
            error: new NodeShamirDbError('invalid_request', 'bad payload'),
          }
        : { ok: true, record: emptyJournal() },
    );
    expect(other?.code).toBe('server');
    expect(other?.serverCode).toBe('invalid_request');
  });
});
