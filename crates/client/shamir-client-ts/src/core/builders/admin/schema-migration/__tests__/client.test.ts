/**
 * Deterministic unit tests for `ApplicationMigrationClient` (release-1
 * R1c step 3b) over the test-socket fixture — no real server, no
 * hand-built wire: every server reply is a RAW result record, every
 * request is produced by the typed client and the real builders.
 *
 * Covered (design §7.3 starting set):
 *   - the migration set builds the ordered typed body with per-body
 *     canonical checksums,
 *   - `status` and `plan` never submit an apply,
 *   - `applyNext` honours the server result and never blindly retries an
 *     unknown outcome (fake timers fire the request deadline after the
 *     apply send; exactly ONE apply then ONE status via
 *     `resolveUnknownApply` with the opaque attempt token),
 *   - a changed checksum of an APPLIED body (drift at a non-head
 *     ordinal) and a reported prefix GAP are both refused BEFORE any
 *     apply send,
 *   - stable §5.3 error mapping: the old peer's `invalid_request` /
 *     `Unknown operation type` is `unsupported_server`, the deferred
 *     admission is recognized in both shapes, the nine stable codes map
 *     one-to-one, and a nonempty journal without the R1c 3a additive
 *     fields is typed unsupported-server before anything is sent.
 */

import { describe, it, expect, vi } from 'vitest';

import { fixture } from '../../../../visibility/__tests__/fixture.js';
import {
  ApplicationMigrationClient,
} from '../client/client.js';
import { MigrationError } from '../client/error.js';
import { mapMigrationServerError } from '../client/error.js';
import { ApplicationMigrationSet } from '../canonical/set.js';
import {
  schemaMigration,
  migrationDdl,
} from '../model/index.js';

// ── raw reply helpers (server-shaped result records ONLY) ───────────

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

/** Raw status of a TRULY empty journal (additive fields present, zero). */
function emptyJournal(identity: string): Record<string, unknown> {
  return statusRecord(identity, {
    last_applied_ordinal: 0,
    fence_generation: 0,
  });
}

/** One raw applied prefix entry. */
function appliedEntry(
  id: string,
  ordinal: number,
  checksum: Uint8Array,
): Record<string, unknown> {
  return { id, ordinal, checksum };
}

/** The aliases the migration client may put on the wire. */
const STATUS_ALIAS = 'application_schema_status';
const PLAN_ALIAS = 'application_schema_plan';
const APPLY_ALIAS = 'apply_application_schema_migration';

/**
 * The decoded `queries` map of one captured batch request (msgpack-decoded,
 * so u64 fields arrive as `bigint` — never JSON-stringify a request).
 */
function queriesOf(
  request: Record<string, unknown>,
): Record<string, unknown> {
  const queries = (request.batch as { queries?: unknown } | undefined)
    ?.queries;
  return queries !== undefined && typeof queries === 'object'
    ? (queries as Record<string, unknown>)
    : {};
}

/** True when NO captured request is an apply submission. */
function noApplySent(requests: Record<string, unknown>[]): boolean {
  return requests.every((request) =>
    Object.values(queriesOf(request)).every(
      (entry) =>
        (entry as { operation?: unknown }).operation !== APPLY_ALIAS,
    ),
  );
}

/** Count captured requests that carry a given alias in their `queries`. */
function countAlias(
  requests: Record<string, unknown>[],
  alias: string,
): number {
  return requests.filter((request) => alias in queriesOf(request)).length;
}

/**
 * Assert the full application-schema wire shape of the entry captured
 * under `alias`: the marker, the snake_case operation and a payload.
 */
function expectSchemaEntry(
  request: Record<string, unknown>,
  alias: string,
): void {
  const entry = queriesOf(request)[alias] as
    | Record<string, unknown>
    | undefined;
  expect(entry).toBeDefined();
  expect(entry!.application_schema).toBe(true);
  expect(entry!.operation).toBe(alias);
  expect(typeof entry!.payload).toBe('object');
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

const IDENTITY = '2222222222222222';
const M1 = 'ts-mig-create-items';
const M2 = 'ts-mig-create-index';
const M3 = 'ts-mig-create-audit';

/** The typed set under test: three ordered bodies, three op kinds. */
function makeSet(): ApplicationMigrationSet {
  return new ApplicationMigrationSet([
    schemaMigration(M1, 1n, 'create items table', [
      migrationDdl.createTable('mig_items'),
    ]),
    schemaMigration(M2, 2n, 'create items index', [
      migrationDdl.createIndex('mig_items_idx', 'mig_items', [['id']]),
    ]),
    schemaMigration(M3, 3n, 'create audit table', [
      migrationDdl.createTable('mig_audit'),
    ]),
  ]);
}

/** Runner over the fixture transport. */
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

/** Raw status of a journal whose head is `ordinal` with `entries`. */
function journalAt(
  identity: string,
  ordinal: number,
  entries: Record<string, unknown>[],
  fence: number,
  headId: string,
  headChecksum: Uint8Array,
): Record<string, unknown> {
  return statusRecord(identity, {
    last_applied_id: headId,
    last_applied_ordinal: ordinal,
    last_applied_checksum: headChecksum,
    applied: entries,
    fence_generation: fence,
  });
}

// ── the suite ───────────────────────────────────────────────────────

describe('application-schema migration client (deterministic fixture)', () => {
  it('migrationSetBuildsOrderedTypedBody', () => {
    const set = makeSet();
    // Ordered typed bodies, ordinals contiguous from 1.
    expect(set.entries.map((entry) => entry.ordinal)).toEqual([1n, 2n, 3n]);
    expect(set.entries.map((entry) => entry.body.metadata.id)).toEqual([
      M1,
      M2,
      M3,
    ]);
    expect(
      set.entries.map((entry) => entry.body.metadata.display_name),
    ).toEqual(['create items table', 'create items index', 'create audit table']);
    // Typed operations kept in authored order per body.
    expect(set.entries[0]!.body.operations).toHaveLength(1);
    expect(set.entries[1]!.body.operations).toHaveLength(1);
    // One 32-byte canonical checksum per body, all distinct.
    const checksums = set.entries.map(({ checksum }) => checksum);
    expect(checksums).toHaveLength(3);
    for (const checksum of checksums) {
      expect(checksum).toBeInstanceOf(Uint8Array);
      expect(checksum).toHaveLength(32);
    }
    expect(new Set(checksums.map((c) => Buffer.from(c).toString('hex'))).size).toBe(3);
  });

  it('migrationStatusAndPlanDoNotSubmitApply', async () => {
    const f = await fixture();
    try {
      const set = makeSet();
      const migrations = runner(f.client, set);

      // status: one read-only status send, nothing else.
      const pending = migrations.status('db');
      await answer(f, STATUS_ALIAS, emptyJournal(IDENTITY));
      const view = await pending;
      expect(view.coverage.kind).toBe('empty');
      expect(f.requests).toHaveLength(1);
      expect(noApplySent(f.requests)).toBe(true);

      // plan: status preflight, then the plan send — never an apply.
      const planPromise = migrations.plan('db');
      await answer(f, STATUS_ALIAS, emptyJournal(IDENTITY));
      await answer(f, PLAN_ALIAS, {
        protocol_version: 1,
        migration: {
          id: M1,
          ordinal: 1,
          display_name: 'create items table',
          canonical_version: 1,
        },
        checksum: set.entries[0]!.checksum,
        predecessor: { ordinal: 0, checksum: new Uint8Array(32) },
        operations: [
          { operation_index: 0, operation_kind: 'create_table', target: 'mig_items' },
        ],
      });
      const plan = await planPromise;
      expect(plan.migration.id).toBe(M1);
      expect(plan.predecessor.ordinal).toBe(0n);
      expect(f.requests).toHaveLength(3);
      expect(noApplySent(f.requests)).toBe(true);
      expect(countAlias(f.requests, STATUS_ALIAS)).toBe(2);
      expect(countAlias(f.requests, PLAN_ALIAS)).toBe(1);
    } finally {
      await f.client.close();
    }
  });

  it('migrationApplyUsesServerResultAndDoesNotBlindlyRetry', async () => {
    // Fake timers fire the client's request deadline AFTER the apply was
    // sent but before any reply: the outcome is unknown.
    vi.useFakeTimers();
    const f = await fixture(50);
    try {
      const set = makeSet();
      const migrations = runner(f.client, set);

      const applyPromise = migrations.applyNext('db');
      // Observe the rejection BEFORE the deadline fires: no unhandled
      // rejection while the fake clock advances.
      let error: unknown;
      const observed = applyPromise.then(
        () => {
          throw new Error('applyNext unexpectedly resolved');
        },
        (e: unknown) => {
          error = e;
        },
      );
      // The status preflight sees an empty journal; the apply goes out…
      await answer(f, STATUS_ALIAS, emptyJournal(IDENTITY));
      const applyRequest = await f.nextRequest(); // …the apply send…
      expectSchemaEntry(applyRequest, APPLY_ALIAS);
      // …but its reply never arrives: the request deadline fires.
      await vi.advanceTimersByTimeAsync(50);
      await observed;
      expect(error).toBeInstanceOf(MigrationError);
      const unknown = error as MigrationError;
      expect(unknown.code).toBe('unknown_outcome');
      expect(unknown.retryable).toBe(false);
      // The frozen attempt token describes exactly the body in flight.
      expect(unknown.attempt).toMatchObject({
        db: 'db',
        databaseIdentity: IDENTITY,
        migrationId: M1,
        ordinal: 1n,
      });
      expect(unknown.attempt!.checksum).toHaveLength(64);

      // NO blind retry: exactly ONE apply on the wire so far.
      expect(countAlias(f.requests, APPLY_ALIAS)).toBe(1);

      // The in-flight submission was the exact ordinal-1 body.
      const submission = (
        queriesOf(applyRequest)[APPLY_ALIAS] as {
          payload: { migration: { metadata: { id: string; ordinal: bigint } } };
        }
      ).payload.migration.metadata;
      expect(submission.id).toBe(M1);
      expect(submission.ordinal).toBe(1n);

      // Reconciliation: resolveUnknownApply sends ONE read-only status.
      const resolvePromise = migrations.resolveUnknownApply(unknown.attempt!);
      const checksum = set.entries[0]!.checksum;
      await answer(
        f,
        STATUS_ALIAS,
        journalAt(
          IDENTITY,
          1,
          [appliedEntry(M1, 1, checksum)],
          1,
          M1,
          checksum,
        ),
      );
      const resolution = await resolvePromise;
      expect(resolution).toEqual({
        kind: 'applied',
        migrationId: M1,
        ordinal: 1n,
      });

      // The transcript so far: one status, one apply, one status — never
      // a second apply, never a plan.
      expect(countAlias(f.requests, STATUS_ALIAS)).toBe(2);
      expect(countAlias(f.requests, APPLY_ALIAS)).toBe(1);
      expect(countAlias(f.requests, PLAN_ALIAS)).toBe(0);

      // DEFINITIVE SUCCESSFUL APPLY: with the head now at ordinal 1, the
      // next `applyNext` submits ordinal 2 and the server's `applied`
      // result is returned verbatim — no unknown-outcome detour.
      const okPromise = migrations.applyNext('db');
      // …whose preflight status sees head at ordinal 1, fence 12.
      await answer(
        f,
        STATUS_ALIAS,
        journalAt(
          IDENTITY,
          1,
          [appliedEntry(M1, 1, checksum)],
          12,
          M1,
          checksum,
        ),
      );
      const applyRequest2 = await f.nextRequest();
      expectSchemaEntry(applyRequest2, APPLY_ALIAS);
      const applyPayload2 = (
        queriesOf(applyRequest2)[APPLY_ALIAS] as {
          payload: {
            migration: { metadata: { id: string; ordinal: bigint } };
            expected_predecessor: {
              migration_id?: string;
              ordinal: bigint;
              checksum: number[];
            };
            expected_fence_generation: bigint;
          };
        }
      ).payload;
      expect(applyPayload2.expected_fence_generation === 12n).toBe(true);
      expect(applyPayload2.expected_predecessor).toMatchObject({
        migration_id: M1,
        ordinal: 1n,
      });
      expect(Array.from(applyPayload2.expected_predecessor.checksum)).toEqual(
        Array.from(checksum),
      );
      expect(
        (
          queriesOf(applyRequest2)[APPLY_ALIAS] as {
            payload: {
              migration: { metadata: { id: string; ordinal: bigint } };
            };
          }
        ).payload.migration.metadata,
      ).toMatchObject({ id: M2, ordinal: 2n });
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          results: {
            [APPLY_ALIAS]: {
              records: [
                {
                  protocol_version: 1,
                  migration_id: M2,
                  ordinal: 2,
                  checksum: set.entries[1]!.checksum,
                  status: 'applied',
                  fence_generation: 1,
                },
              ],
            },
          },
          execution_plan: [],
          execution_time_us: 0,
        },
      });
      const applied = await okPromise;
      expect(applied.migration_id).toBe(M2);
      expect(applied.ordinal).toBe(2n);
      expect(applied.status).toBe('applied');
      expect(applied.error).toBeUndefined();
      // The full transcript: the preflight status, the timed-out apply,
      // the resolve status, the second apply's preflight status (head at
      // ordinal 1, fence 12) and the definitive apply — two applies
      // total, never a plan.
      expect(countAlias(f.requests, STATUS_ALIAS)).toBe(3);
      expect(countAlias(f.requests, APPLY_ALIAS)).toBe(2);
      expect(countAlias(f.requests, PLAN_ALIAS)).toBe(0);
    } finally {
      await f.client.close();
      vi.useRealTimers();
    }
  });

  it('migrationChecksumDetectsChangedAppliedBody', async () => {
    const f = await fixture();
    try {
      const set = makeSet();
      const migrations = runner(f.client, set);
      const [c1, c2] = set.entries.map(({ checksum }) => checksum);

      // NON-HEAD DRIFT: ordinal 1's reported checksum differs from the
      // local body while the head (ordinal 2) itself is consistent.
      const tampered = Uint8Array.from(c1!);
      tampered[0] = (tampered[0]! + 1) % 256;
      const driftPromise = migrations.applyNext('db');
      await answer(
        f,
        STATUS_ALIAS,
        journalAt(
          IDENTITY,
          2,
          [appliedEntry(M1, 1, tampered), appliedEntry(M2, 2, c2!)],
          3,
          M2,
          c2!,
        ),
      );
      let driftError: unknown;
      try {
        await driftPromise;
      } catch (e) {
        driftError = e;
      }
      expect(driftError).toBeInstanceOf(MigrationError);
      expect((driftError as MigrationError).code).toBe('drift');

      // GAP: the reported prefix [1, 3] skips ordinal 2.
      const gapPromise = migrations.applyNext('db');
      await answer(
        f,
        STATUS_ALIAS,
        journalAt(
          IDENTITY,
          3,
          [appliedEntry(M1, 1, c1!), appliedEntry(M2, 3, c2!)],
          4,
          M2,
          c2!,
        ),
      );
      let gapError: unknown;
      try {
        await gapPromise;
      } catch (e) {
        gapError = e;
      }
      expect(gapError).toBeInstanceOf(MigrationError);
      expect((gapError as MigrationError).code).toBe('schema_migration_gap');

      // Both refusals happened BEFORE any send beyond the preflights.
      expect(countAlias(f.requests, STATUS_ALIAS)).toBe(2);
      expect(noApplySent(f.requests)).toBe(true);
    } finally {
      await f.client.close();
    }
  });

  it('migrationErrorCodesMapStablyIncludingDeferredAndOldServer', async () => {
    // The old peer's unsupported-operation decoder report, matched
    // NARROWLY: that exact code + message, not every `invalid_request`.
    const oldServer = mapMigrationServerError(
      'invalid_request',
      'Unknown operation type',
    );
    expect(oldServer).toBeInstanceOf(MigrationError);
    expect(oldServer.code).toBe('unsupported_server');
    expect(oldServer.serverCode).toBe('invalid_request');
    expect(oldServer.message).toBe('Unknown operation type');
    // A different message on the same code stays a plain server error.
    expect(mapMigrationServerError('invalid_request', 'bad field').code).toBe(
      'server',
    );

    // The nine §5.3 stable codes map one-to-one, message preserved.
    const stable = [
      'schema_migration_in_progress',
      'schema_migration_in_doubt',
      'schema_migration_checksum_mismatch',
      'schema_migration_gap',
      'schema_migration_predecessor_mismatch',
      'schema_migration_follower_read_only',
      'schema_migration_durability_unavailable',
      'schema_migration_postcondition_failed',
      'schema_migration_identity_mismatch',
    ] as const;
    for (const code of stable) {
      const mapped = mapMigrationServerError(code, `detail of ${code}`);
      expect(mapped.code).toBe(code);
      expect(mapped.message).toBe(`detail of ${code}`);
      expect(mapped.retryable).toBe(false);
    }

    // The deferred admission, in BOTH observed shapes.
    const rawDeferred = mapMigrationServerError(
      'internal',
      'schema_migration_operation_deferred: journal busy',
    );
    expect(rawDeferred.code).toBe('deferred_operation');
    const wrappedDeferred = mapMigrationServerError(
      'query',
      "Query 'apply_application_schema_migration' failed: schema_migration_operation_deferred: journal busy",
    );
    expect(wrappedDeferred.code).toBe('deferred_operation');
    // Not every wrapped query error is deferred.
    expect(mapMigrationServerError('query', "Query 'x' failed: boom").code).toBe(
      'server',
    );
    // Unrecognized codes fall back, preserving the original verbatim.
    const fallback = mapMigrationServerError('boom_code', 'unexpected');
    expect(fallback.code).toBe('server');
    expect(fallback.serverCode).toBe('boom_code');

    // Old server, live path: a nonempty journal WITHOUT the additive
    // R1c 3a fields is typed unsupported-server before anything is sent.
    const f = await fixture();
    try {
      const migrations = runner(f.client, makeSet());
      const statusPromise = migrations.status('db');
      await answer(
        f,
        STATUS_ALIAS,
        statusRecord(IDENTITY, {
          last_applied_id: M1,
          last_applied_ordinal: 1,
          last_applied_checksum: new Uint8Array(32),
        }),
      );
      let unsupportedError: unknown;
      try {
        await statusPromise;
      } catch (e) {
        unsupportedError = e;
      }
      expect(unsupportedError).toBeInstanceOf(MigrationError);
      expect((unsupportedError as MigrationError).code).toBe(
        'unsupported_server',
      );
      expect(noApplySent(f.requests)).toBe(true);

      // LIVE old-op fixture: an in-flight `status` answered on the wire
      // with the old peer's exact error is typed `unsupported_server`.
      const oldOpPromise = migrations.status('db');
      await f.nextRequest();
      f.respond({
        kind: 'error',
        code: 'invalid_request',
        message: 'Unknown operation type',
      });
      let oldOpError: unknown;
      try {
        await oldOpPromise;
      } catch (e) {
        oldOpError = e;
      }
      expect(oldOpError).toBeInstanceOf(MigrationError);
      const oldOp = oldOpError as MigrationError;
      expect(oldOp.code).toBe('unsupported_server');
      expect(oldOp.serverCode).toBe('invalid_request');
      expect(oldOp.message).toBe('Unknown operation type');
    } finally {
      await f.client.close();
    }
  });
});
