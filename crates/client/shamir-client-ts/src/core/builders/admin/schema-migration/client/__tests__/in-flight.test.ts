/**
 * Deterministic unit tests for the UNRESOLVED-journal classification
 * (release-1 R1c 3b follow-up finding 4): every unresolved branch —
 * `plan` preflight, unmatched `applyNext` flight, the matching head in
 * `resolveUnknownApply`, and a DIFFERENT migration's flight beside a
 * reported `schema_migration_in_doubt` code — classifies through the
 * in-flight entry's OWN status (mirror of `in_flight_error` in the Rust
 * `apply.rs`):
 *   - `in_doubt` / `failed` → `schema_migration_in_doubt` with the
 *     server's operation index and reason when reported;
 *   - anything else (running, recovery) → `schema_migration_in_progress`.
 * The real server reports the `schema_migration_in_doubt` CODE for a
 * merely RUNNING migration too, so no branch may trust the blanket code.
 *
 * Over the test-socket fixture — no real server, no hand-built wire.
 * Every thrown/returned promise is observed before any reply is
 * delivered; request counts assert nothing beyond the single status
 * query is ever sent (no apply, no plan, no retry).
 */

import { afterEach, describe, it, expect } from 'vitest';

import { fixture } from '../../../../../visibility/__tests__/fixture.js';
import { ApplicationMigrationClient } from '../client.js';
import { MigrationError } from '../error.js';
import { ApplicationMigrationSet } from '../../canonical/set.js';
import { schemaMigration, migrationDdl } from '../../model/index.js';

/**
 * Fixture wrapper that tracks every opened ShamirClient so `afterEach`
 * can close them all — the bounded mock socket owns a read loop, so an
 * unclosed client would leak it across tests.
 */
const openClients: Awaited<ReturnType<typeof fixture>>['client'][] = [];

async function openFixture(
  requestTimeoutMs = 0,
): Promise<Awaited<ReturnType<typeof fixture>>> {
  const f = await fixture(requestTimeoutMs);
  openClients.push(f.client);
  return f;
}

afterEach(async () => {
  for (const client of openClients.splice(0)) {
    await client.close();
  }
});

// ── raw reply helpers (server-shaped result records ONLY) ───────────

const IDENTITY = '2222222222222222';
const M1 = 'ts-mig-create-items';
const M2 = 'ts-mig-create-index';
const OTHER = 'ts-mig-other-owner';

const STATUS_ALIAS = 'application_schema_status';
const PLAN_ALIAS = 'application_schema_plan';
const APPLY_ALIAS = 'apply_application_schema_migration';

/** Base of every raw status record: protocol 1, valid verdict. */
function statusRecord(
  identity: string,
  fields: Record<string, unknown>,
): Record<string, unknown> {
  return {
    protocol_version: 1,
    target: { database_identity: identity },
    last_applied_ordinal: 0,
    checksum_validation: 'valid',
    ...fields,
  };
}

/** One raw in-flight owner with an explicit flight fence. */
function flight(
  status: string,
  fields: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    migration_id: OTHER,
    ordinal: 1n,
    fence_generation: 7n,
    status,
    ...fields,
  };
}

/** The decoded `queries` map of one captured batch request. */
function queriesOf(
  request: Record<string, unknown>,
): Record<string, unknown> {
  const queries = (request.batch as { queries?: unknown } | undefined)
    ?.queries;
  return queries !== undefined && typeof queries === 'object'
    ? (queries as Record<string, unknown>)
    : {};
}

/** Count captured requests that carry a given alias in their `queries`. */
function countAlias(
  requests: Record<string, unknown>[],
  alias: string,
): number {
  return requests.filter((request) => alias in queriesOf(request)).length;
}

/** Answer the oldest queued request with a single-record batch reply. */
async function answer(
  f: Awaited<ReturnType<typeof fixture>>,
  alias: string,
  record: unknown,
): Promise<void> {
  await f.nextRequest();
  f.respond({
    kind: 'batch',
    response: {
      id: 1,
      results: { [alias]: { records: [record] } },
      execution_plan: [],
      execution_time_us: 0,
    },
  });
}

// ── shared local fixtures ───────────────────────────────────────────

/** The typed set under test: two ordered bodies. */
function makeSet(): ApplicationMigrationSet {
  return new ApplicationMigrationSet([
    schemaMigration(M1, 1n, 'create items table', [
      migrationDdl.createTable('mig_items'),
    ]),
    schemaMigration(M2, 2n, 'create items index', [
      migrationDdl.createIndex('mig_items_idx', 'mig_items', [['id']]),
    ]),
  ]);
}

/** Runner over the fixture transport for THIS identity and set. */
function runner(
  client: Awaited<ReturnType<typeof fixture>>['client'],
  set: ApplicationMigrationSet,
): ApplicationMigrationClient {
  return new ApplicationMigrationClient(
    client,
    { database_identity: IDENTITY },
    set,
  );
}

/** Lowercase-hex of a checksum, for a hand-built attempt token. */
function toHex(bytes: Uint8Array): string {
  let hex = '';
  for (const byte of bytes) {
    hex += byte.toString(16).padStart(2, '0');
  }
  return hex;
}

/**
 * A valid attempt token for the set's ordinal-1 body — the exact shape
 * `applyNext` binds before a send (db, identity, id, ordinal, hex
 * checksum), so `resolveUnknownApply` accepts it without an apply having
 * been sent.
 */
function attemptFor(set: ApplicationMigrationSet): {
  db: string;
  databaseIdentity: string;
  migrationId: string;
  ordinal: bigint;
  checksum: string;
} {
  return {
    db: 'db',
    databaseIdentity: IDENTITY,
    migrationId: M1,
    ordinal: 1n,
    checksum: toHex(set.entries[0]!.checksum),
  };
}

/** Assert the fixture transcript shows ONLY one status query. */
function expectOnlyStatus(f: Awaited<ReturnType<typeof fixture>>): void {
  expect(f.requests).toHaveLength(1);
  expect(countAlias(f.requests, STATUS_ALIAS)).toBe(1);
  expect(countAlias(f.requests, PLAN_ALIAS)).toBe(0);
  expect(countAlias(f.requests, APPLY_ALIAS)).toBe(0);
}

// ── the suite ───────────────────────────────────────────────────────

describe('unresolved journal classification (in_flight_error split)', () => {
  it('planPreflightClassifiesByTheFlightStatusNotTheBlanketCode', async () => {
    const f = await openFixture();
    const set = makeSet();
    const migrations = runner(f.client, set);

    // A RUNNING other migration: the real server reports the in_doubt
    // CODE for it too — the typed refusal must still be in_progress.
    const livePromise = migrations.plan('db').then(
      () => {
        throw new Error('plan unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        error: 'schema_migration_in_doubt',
        in_flight: flight('started'),
      }),
    );
    const liveError = (await livePromise) as MigrationError;
    expect(liveError).toBeInstanceOf(MigrationError);
    expect(liveError.code).toBe('schema_migration_in_progress');
    expect(liveError.message).toContain('started');
    expect(liveError.message).toContain('holds the fence');
    expectOnlyStatus(f);

    // A FAILED migration: the uncertain window — in_doubt with the
    // server's operation index and recorded reason preserved.
    const f2 = await openFixture();
    const migrations2 = runner(f2.client, makeSet());
    const doubtPromise = migrations2.plan('db').then(
      () => {
        throw new Error('plan unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f2,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        error: 'schema_migration_in_doubt',
        in_flight: flight('failed', {
          operation_index: 2n,
          in_doubt_reason: 'durability window closed',
        }),
      }),
    );
    const doubtError = (await doubtPromise) as MigrationError;
    expect(doubtError.code).toBe('schema_migration_in_doubt');
    expect(doubtError.operationIndex).toBe(2n);
    expect(doubtError.reason).toBe('durability window closed');
    expect(doubtError.message).toBe(
      'migration is in doubt at operation 2: durability window closed',
    );
    expectOnlyStatus(f2);
  });

  it('applyNextUnmatchedFlightClassifiesByItsOwnStatus', async () => {
    const f = await openFixture();
    const set = makeSet();
    const migrations = runner(f.client, set);

    // Unmatched RUNNING flight beside the in_doubt code: refusal is
    // in_progress and NOTHING beyond the status is sent (no apply).
    const livePromise = migrations.applyNext('db').then(
      () => {
        throw new Error('applyNext unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        error: 'schema_migration_in_doubt',
        in_flight: flight('started'),
      }),
    );
    const liveError = (await livePromise) as MigrationError;
    expect(liveError.code).toBe('schema_migration_in_progress');
    expect(liveError.retryable).toBe(false);
    expectOnlyStatus(f);

    // Unmatched FAILED flight: in_doubt with the server's evidence.
    const f2 = await openFixture();
    const migrations2 = runner(f2.client, makeSet());
    const doubtPromise = migrations2.applyNext('db').then(
      () => {
        throw new Error('applyNext unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f2,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        error: 'schema_migration_in_doubt',
        in_flight: flight('in_doubt', {
          ordinal: 1n,
          operation_index: 1n,
          in_doubt_reason: 'response lost mid-journal',
        }),
      }),
    );
    const doubtError = (await doubtPromise) as MigrationError;
    expect(doubtError.code).toBe('schema_migration_in_doubt');
    expect(doubtError.operationIndex).toBe(1n);
    expect(doubtError.reason).toBe('response lost mid-journal');
    expectOnlyStatus(f2);
  });

  it('resolveUnknownApplyMatchingHeadClassifiesByItsStatus', async () => {
    const f = await openFixture();
    const set = makeSet();
    const migrations = runner(f.client, set);
    const attempt = attemptFor(set);

    // The attempt IS the unresolved head, merely RUNNING: in_progress,
    // with the attempt token attached — never a resend.
    const livePromise = migrations.resolveUnknownApply(attempt).then(
      () => {
        throw new Error('resolve unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        in_flight: flight('operation_intent', { migration_id: M1 }),
      }),
    );
    const liveError = (await livePromise) as MigrationError;
    expect(liveError.code).toBe('schema_migration_in_progress');
    expect(liveError.attempt).toEqual(attempt);
    expectOnlyStatus(f);

    // The attempt IS the head, FAILED at operation 3: in_doubt with the
    // server's index and reason plus the attempt token.
    const f2 = await openFixture();
    const migrations2 = runner(f2.client, makeSet());
    const doubtPromise = migrations2.resolveUnknownApply(attempt).then(
      () => {
        throw new Error('resolve unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f2,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        in_flight: flight('failed', {
          migration_id: M1,
          operation_index: 3n,
          in_doubt_reason: 'durable write unconfirmed',
        }),
      }),
    );
    const doubtError = (await doubtPromise) as MigrationError;
    expect(doubtError.code).toBe('schema_migration_in_doubt');
    expect(doubtError.operationIndex).toBe(3n);
    expect(doubtError.reason).toBe('durable write unconfirmed');
    expect(doubtError.attempt).toEqual(attempt);
    expectOnlyStatus(f2);
  });

  it('resolveOtherFlightIsClassifiedNotBlanketAndNeverShadowsAProvenApplied', async () => {
    const f = await openFixture();
    const set = makeSet();
    const migrations = runner(f.client, set);
    const attempt = attemptFor(set);

    // A DIFFERENT running flight beside an in_doubt report, no proof of
    // the attempt: classified through the flight's status (in_progress),
    // never the blanket code, never a false applied.
    const livePromise = migrations.resolveUnknownApply(attempt).then(
      () => {
        throw new Error('resolve unexpectedly resolved');
      },
      (e: unknown) => e,
    );
    await answer(
      f,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        error: 'schema_migration_in_doubt',
        in_flight: flight('started'),
      }),
    );
    const liveError = (await livePromise) as MigrationError;
    expect(liveError.code).toBe('schema_migration_in_progress');
    expectOnlyStatus(f);

    // A different unresolved event WITHOUT any reported error: the
    // established returned verdict stays unmatched_unresolved.
    const f2 = await openFixture();
    const migrations2 = runner(f2.client, makeSet());
    const unmatchedPromise = migrations2.resolveUnknownApply(attempt);
    await answer(
      f2,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 0,
        fence_generation: 7n,
        in_flight: flight('recovery_acquired'),
      }),
    );
    expect(await unmatchedPromise).toEqual({
      kind: 'unmatched_unresolved',
      journalOrdinal: 0n,
    });
    expectOnlyStatus(f2);

    // PROOF-FIRST: the attempt's exact entry sits in the valid reported
    // prefix even though a LATER other-flight is unresolved and the
    // server reports in_doubt for it — the proof shadows the code and
    // the verdict is applied.
    const f3 = await openFixture();
    const set3 = makeSet();
    const migrations3 = runner(f3.client, set3);
    const checksum1 = set3.entries[0]!.checksum;
    const checksum2 = set3.entries[1]!.checksum;
    const appliedPromise = migrations3.resolveUnknownApply(
      attemptFor(set3),
    );
    await answer(
      f3,
      STATUS_ALIAS,
      statusRecord(IDENTITY, {
        last_applied_ordinal: 2n,
        last_applied_id: M2,
        last_applied_checksum: checksum2,
        applied: [
          { id: M1, ordinal: 1n, checksum: checksum1 },
          { id: M2, ordinal: 2n, checksum: checksum2 },
        ],
        fence_generation: 9n,
        error: 'schema_migration_in_doubt',
        in_flight: flight('in_doubt', { ordinal: 3n }),
      }),
    );
    expect(await appliedPromise).toEqual({
      kind: 'applied',
      migrationId: M1,
      ordinal: 1n,
    });
    expect(f3.requests).toHaveLength(1);
    expect(countAlias(f3.requests, STATUS_ALIAS)).toBe(1);
    expect(countAlias(f3.requests, APPLY_ALIAS)).toBe(0);
  });
});
