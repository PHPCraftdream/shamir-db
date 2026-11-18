/**
 * End-to-end application-schema MIGRATIONS test (release-1 R1c step 3b).
 *
 * Drives the typed TS migration tool (`ApplicationMigrationClient` over an
 * `ApplicationMigrationSet`) against the REAL server, mirroring the Rust
 * behavioural reference
 * `crates/devtools/shamir-e2e/tests/internals/rust_migration_tests.rs`:
 *
 * 1. two ordered applies on a durable Fjall repo (`applyNext` sends exactly
 *    one ordinal with the status-derived predecessor and fence generation),
 *    then a repeat apply of the fully covered set — the typed local no-op
 *    `nothing_to_apply` (nothing sent),
 * 2. the FULL status prefix afterwards (ids / ordinals / canonical
 *    checksums from the set itself, nonzero fence generation), with the
 *    actually created table read back through `ddl.describeTable`,
 * 3. DRIFT on the FIRST body: reported read-only by `status`, and
 *    `applyNext` refuses BEFORE any apply send — proven by a spy on the
 *    transport counting exactly the two read-only `status` sends and zero
 *    apply sends,
 * 4. TARGET DISCOVERY through the status identity mismatch: a probe client
 *    over a deliberately WRONG identity gets the read-only status reply
 *    with `server_error === 'schema_migration_identity_mismatch'` and
 *    `reported_target` carrying the SERVING identity (never a write),
 * 5. a same-one-body-set RACE on a separate database with two independent
 *    connections, staged NON-TIMING deterministically. HONEST BOUNDARY: this
 *    is a HIGH-LEVEL concurrent invocation of two `applyNext` calls with a
 *    DETERMINISTIC PREFLIGHT INTERLEAVING coordinated at the typed client's
 *    transport boundary — it is NOT a server-side simultaneous-writers race
 *    (the server would allow two identical `applied` success replies there;
 *    `apply.rs:134-149`, mirrored by Rust `apply_fence_tests.rs:20-74`).
 *    Both contenders are released from ONE shared gate and both are proven
 *    to be INSIDE `applyNext` at their preflight boundary before either
 *    makes progress; the first contender's preflight then actually SENDS
 *    (real request, real reply) and its apply settles durably; only then
 *    does the second contender's preflight SEND — over its own SECOND real
 *    connection — and the typed client returns the genuine typed refusal
 *    `nothing_to_apply`. Nothing is faked: both outcomes are real server
 *    answers to real requests. Asserted as a SET of exactly two outcome
 *    codes (`applied` + `nothing_to_apply`) with exactly ONE durable prefix
 *    entry (exact id / ordinal / checksum, fence generation 2).
 *
 * Requires the release server binary:
 *   cargo build --release -p shamir-server
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import {
  ShamirClient,
  Batch,
  ddl,
  ApplicationMigrationClient,
  MigrationError,
  ApplicationMigrationSet,
  schemaMigration,
  migrationDdl,
} from '../../index.js';
import type { BatchResponse, MigrationStatus } from '../../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  uniqueDbName,
} from '../e2e-harness.js';
import type { ServerHandle } from '../e2e-harness.js';

// ─── constants ───────────────────────────────────────────────────────────────

/** Deliberately wrong target identity for the discovery probe. */
const WRONG_IDENTITY = '1111111111111111';

const M1_ID = 'ts-mig-create-items';
const M2_ID = 'ts-mig-create-audit';

const REQUEST_TIMEOUT_MS = 30_000;
const TEST_TIMEOUT_MS = 180_000;

// ─── helpers ─────────────────────────────────────────────────────────────────

/**
 * Bound any single await by the per-request deadline. The deadline timer is
 * ALWAYS cleared once the race settles, so no timer outlives the await.
 */
function bounded<T>(promise: Promise<T>, ms = REQUEST_TIMEOUT_MS): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`request deadline (${ms}ms) exceeded`)),
      ms,
    );
  });
  return Promise.race([promise, deadline]).finally(() => clearTimeout(timer));
}

/** MigrationError guard: typed instance + expected code. */
function isMigrationError(e: unknown, code: string): e is MigrationError {
  return e instanceof MigrationError && e.code === code;
}

/** One migration body with a single `create_table` op. */
function tableBody(
  id: string,
  ordinal: number | bigint,
  name: string,
  table: string,
) {
  return schemaMigration(id, ordinal, name, [migrationDdl.createTable(table)]);
}

/**
 * Fixture: a fresh database with a PERSISTENT Fjall repo (`main`, engine
 * `fjall`) — the durable repo is built EXPLICITLY with the typed
 * `Batch.create().add(...).build()` builder (never raw JSON).
 */
async function setupDurableDb(
  client: ShamirClient,
  label: string,
): Promise<string> {
  const db = uniqueDbName(`mig_${label}`);
  await bounded(
    client.execute(
      'default',
      Batch.create(`setup-${db}-db`).add('mk', ddl.createDb(db)).build(),
    ),
  );
  await bounded(
    client.execute(
      db,
      Batch.create(`setup-${db}-repo`)
        .add('mr', ddl.createRepo('main', { engine: 'fjall' }))
        .build(),
    ),
  );
  return db;
}

/**
 * Identity discovery: a client over the deliberately wrong identity probes
 * `status` — read-only, the mismatch arrives INSIDE the typed status result
 * as `server_error === 'schema_migration_identity_mismatch'` while
 * `reported_target` echoes the SERVING identity. Never a write, never a
 * throw. Returns a client rebuilt with the discovered identity plus the
 * identity string itself.
 */
async function discoverIdentity(
  client: ShamirClient,
  set: ApplicationMigrationSet,
  db: string,
): Promise<{ migrations: ApplicationMigrationClient; identity: string }> {
  const probe = new ApplicationMigrationClient(
    client,
    { database_identity: WRONG_IDENTITY },
    set,
  );
  const probeStatus: MigrationStatus = await bounded(probe.status(db));
  expect(probeStatus.server_error).toBe('schema_migration_identity_mismatch');
  const servingIdentity = probeStatus.reported_target.database_identity;
  expect(servingIdentity).toBeTruthy();
  expect(servingIdentity).not.toBe(WRONG_IDENTITY);
  return {
    migrations: new ApplicationMigrationClient(
      client,
      { database_identity: servingIdentity },
      set,
    ),
    identity: servingIdentity,
  };
}

/**
 * A promise gate: `wait` is awaited, `open` releases every waiter.
 */
function makeGate(): { wait: Promise<void>; open: () => void } {
  let open!: () => void;
  const wait = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { wait, open };
}

const hasOp = (batch: object, op: string): boolean =>
  Object.values((batch as { queries?: Record<string, object> }).queries ?? {}).some(
    (q) => (q as { operation?: string }).operation === op,
  );

/**
 * Race transport for the deterministic preflight interleaving: apply sends
 * pass through untouched; a preflight `application_schema_status` send first
 * announces that its caller has entered `applyNext` and reached the preflight
 * boundary (`onPreflightBoundary`), and may additionally be HELD (pure
 * promise chaining — no timers, no sleeps) until `holdPreflightSend`
 * resolves before the real request goes out over `client`.
 */
function raceTransport(
  client: ShamirClient,
  opts: {
    onPreflightBoundary: () => void;
    holdPreflightSend?: Promise<void>;
  },
): Pick<ShamirClient, 'execute'> {
  return {
    execute: (db: string, batch: object): Promise<BatchResponse> => {
      if (!hasOp(batch, 'application_schema_status')) {
        return client.execute(db, batch);
      }
      opts.onPreflightBoundary();
      const send = (opts.holdPreflightSend ?? Promise.resolve()).then(() =>
        client.execute(db, batch),
      );
      return send;
    },
  };
}

/**
 * Transport spy: wraps `client.execute`, records every send, and classifies
 * each by the migration op key it carries (`application_schema_status` vs
 * `apply_application_schema_migration`).
 */
function spyTransport(client: ShamirClient): {
  execute: ShamirClient['execute'];
  statusSends: () => number;
  applySends: () => number;
} {
  const batches: object[] = [];
  return {
    execute: (db: string, batch: object): Promise<BatchResponse> => {
      batches.push(batch);
      return client.execute(db, batch);
    },
    statusSends: () =>
      batches.filter((b) => hasOp(b, 'application_schema_status')).length,
    applySends: () =>
      batches.filter((b) => hasOp(b, 'apply_application_schema_migration'))
        .length,
  };
}

// ─── test suite ───────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e application-schema migrations (requires release binary)',
  () => {
    // One PERSISTENT server shared by the tests.
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await bounded(startServer(), 60_000);
      try {
        client = await bounded(connectAdmin(HOST, server.port), 60_000);
      } catch (e) {
        console.error(
          '[e2e-schema-migrations] connection failed. Server logs:\n' +
            server!.logs(),
        );
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try {
          await bounded(client.close());
        } catch {
          /* ok */
        }
        client = null;
      }
      if (server) {
        await bounded(server.stop());
        server = null;
      }
    });

    it(
      'ordered applies, repeat no-op, drift refusal, full status prefix',
      async () => {
        const setFull = new ApplicationMigrationSet([
          tableBody(M1_ID, 1n, 'create items table', 'mig_items'),
          tableBody(M2_ID, 2n, 'create audit table', 'mig_audit'),
        ]);

        const db = await setupDurableDb(client!, 'seq');
        const { migrations } = await discoverIdentity(client!, setFull, db);

        // --- honest empty-journal view (read-only) ----------------------
        const empty = await bounded(migrations.status(db));
        expect(empty.server_error).toBeUndefined();
        expect(empty.coverage.kind).toBe('empty');
        expect(empty.applied).toHaveLength(0);
        expect(empty.fence_generation).toBe(0n);
        expect(empty.next_local_ordinal).toBe(1n);

        // --- first real apply (ordinal 1) -------------------------------
        const appliedFirst = await bounded(migrations.applyNext(db));
        expect(appliedFirst.migration_id).toBe(M1_ID);
        expect(BigInt(appliedFirst.ordinal)).toBe(1n);
        expect(appliedFirst.status).toBe('applied');

        // --- second real apply (ordinal 2, resume via status) -----------
        const appliedSecond = await bounded(migrations.applyNext(db));
        expect(appliedSecond.migration_id).toBe(M2_ID);
        expect(BigInt(appliedSecond.ordinal)).toBe(2n);
        expect(appliedSecond.status).toBe('applied');

        // --- DRIFT: tampered FIRST body, identical second, pending third.
        // Without the drift the set would still have something to send, so
        // any refusal is on drift alone. Status reports it read-only;
        // applyNext refuses BEFORE any apply send — proven by the spy:
        // exactly the two read-only status sends (the explicit status and
        // the applyNext preflight), zero apply sends.
        const driftSet = new ApplicationMigrationSet([
          tableBody(M1_ID, 1n, 'create items table', 'mig_items_tampered'),
          tableBody(M2_ID, 2n, 'create audit table', 'mig_audit'),
          tableBody('ts-mig-create-metrics', 3n, 'create metrics table', 'mig_metrics'),
        ]);
        const spy = spyTransport(client!);
        const driftProbed = new ApplicationMigrationClient(
          spy,
          { database_identity: (await bounded(migrations.status(db)))
              .reported_target.database_identity },
          driftSet,
        );
        const driftStatus = await bounded(driftProbed.status(db));
        expect(driftStatus.coverage).toMatchObject({ kind: 'drift', ordinal: 1n });
        let driftApplyError: unknown;
        try {
          await bounded(driftProbed.applyNext(db));
        } catch (e) {
          driftApplyError = e;
        }
        expect(isMigrationError(driftApplyError, 'drift')).toBe(true);
        expect(spy.statusSends()).toBe(2);
        expect(spy.applySends()).toBe(0);

        // --- repeat of the FULL set after both applies: nothing left ----
        let fullRepeatError: unknown;
        try {
          await bounded(migrations.applyNext(db));
        } catch (e) {
          fullRepeatError = e;
        }
        expect(isMigrationError(fullRepeatError, 'nothing_to_apply')).toBe(true);

        // --- final status: FULL applied prefix, exact ids/ordinals/ ------
        // --- checksums taken from the set itself, durable fence ----------
        const final = await bounded(migrations.status(db));
        expect(final.server_error).toBeUndefined();
        expect(final.coverage).toMatchObject({ kind: 'verified', ordinal: 2n });
        expect(final.applied.map((a) => [a.id, a.ordinal] as const)).toEqual([
          [M1_ID, 1n],
          [M2_ID, 2n],
        ]);
        expect(final.applied[0]!.checksum).toEqual(setFull.entries[0]!.checksum);
        expect(final.applied[1]!.checksum).toEqual(setFull.entries[1]!.checksum);
        expect(final.fence_generation).toBeGreaterThan(0n);
        expect(final.next_local_ordinal).toBeUndefined();

        // --- the actually created table is read back (not only journal) --
        const readBack = await bounded(
          client!.execute(
            db,
            Batch.create(`mig-readback-${db}`)
              .add('q', ddl.describeTable('mig_items'))
              .build(),
          ),
        );
        expect(
          (readBack as { results?: Record<string, unknown> }).results?.q,
        ).toBeTruthy();
      },
      TEST_TIMEOUT_MS,
    );

    it(
      'race: same one-body set, two connections — one apply wins',
      async () => {
        const raceSet = new ApplicationMigrationSet([
          tableBody('ts-mig-race', 1n, 'race table', 'mig_race'),
        ]);

        // SEPARATE database, TWO independent connections.
        const db = await setupDurableDb(client!, 'race');
        // Open connA first; if connB's connect fails, connA is still
        // closed by the finally block below.
        const connA = await bounded(connectAdmin(HOST, server!.port));
        let connB: ShamirClient | null = null;
        try {
          connB = await bounded(connectAdmin(HOST, server!.port));
          const discovered = await discoverIdentity(connA, raceSet, db);

          // HONEST BOUNDARY: this is a high-level concurrent invocation of
          // two `applyNext` calls with a deterministic preflight interleaving
          // coordinated at the typed client's transport boundary — NOT a
          // server-side simultaneous-writers race (there the server would
          // allow two identical `applied` replies). Both outcomes below are
          // genuine server answers to real requests.
          const gate = makeGate(); // releases BOTH `applyNext` invocations
          const enteredA = makeGate(); // A entered applyNext, preflight boundary
          const enteredB = makeGate(); // B entered applyNext, announces readiness
          let settleA!: () => void;
          const settledA = new Promise<void>((resolve) => {
            settleA = resolve;
          });

          const a = new ApplicationMigrationClient(
            raceTransport(connA, {
              onPreflightBoundary: enteredA.open,
              // A's preflight only actually SENDS once B has ALSO entered
              // applyNext and announced its preflight boundary.
              holdPreflightSend: enteredB.wait,
            }),
            { database_identity: discovered.identity },
            raceSet,
          );
          const b = new ApplicationMigrationClient(
            raceTransport(connB, {
              onPreflightBoundary: enteredB.open,
              // B's preflight SEND is held until A's whole applyNext has
              // settled durably; B then reads the completed journal over its
              // own SECOND real connection and gets the typed refusal.
              holdPreflightSend: settledA,
            }),
            { database_identity: discovered.identity },
            raceSet,
          );

          // Observe BOTH contenders BEFORE opening the gate, so a rejection
          // can never go unobserved; the settled result is awaited below.
          const outcomes = Promise.allSettled([
            gate.wait.then(async () => {
              try {
                return await bounded(a.applyNext(db));
              } finally {
                settleA();
              }
            }),
            gate.wait.then(() => bounded(b.applyNext(db))),
          ]);

          // ONE shared gate releases both contenders.
          gate.open();

          // Both contenders are INSIDE applyNext at their preflight boundary
          // before the first is allowed to make any progress (A's send is
          // structurally gated on `enteredB`, so this holds by construction).
          await bounded(Promise.all([enteredA.wait, enteredB.wait]));

          const settled = await outcomes;
          expect(settled).toHaveLength(2);

          // EXACTLY two genuine outcomes: one `applied`, one typed
          // `nothing_to_apply` — as a set, never the join order.
          const codes = new Set<string>();
          let winner: { migration_id: string; ordinal: number | bigint } |
            undefined;
          for (const o of settled) {
            if (o.status === 'fulfilled') {
              expect(o.value.status).toBe('applied');
              codes.add('applied');
              winner = o.value;
            } else {
              expect(
                o.reason,
                `unexpected race rejection: ${String(o.reason)}`,
              ).toBeInstanceOf(MigrationError);
              expect((o.reason as MigrationError).code).toBe('nothing_to_apply');
              codes.add('nothing_to_apply');
            }
          }
          expect(codes.size).toBe(2);
          expect([...codes].sort()).toEqual(
            ['applied', 'nothing_to_apply'].sort(),
          );
          expect(winner).toBeDefined();
          expect(winner!.migration_id).toBe('ts-mig-race');
          expect(BigInt(winner!.ordinal)).toBe(1n);

          // Exactly ONE durable prefix entry afterwards, with the exact
          // id / ordinal / checksum from the set, and fence generation 2.
          const after = await bounded(discovered.migrations.status(db));
          expect(after.applied).toHaveLength(1);
          expect(after.applied[0]!.id).toBe('ts-mig-race');
          expect(after.applied[0]!.ordinal).toBe(1n);
          expect(after.applied[0]!.checksum).toEqual(
            raceSet.entries[0]!.checksum,
          );
          expect(after.fence_generation).toBe(2n);
        } finally {
          for (const c of [connA, connB]) {
            if (!c) continue;
            try {
              await bounded(c.close());
            } catch {
              /* ok */
            }
          }
        }
      },
      TEST_TIMEOUT_MS,
    );

    it(
      'real concurrent wire applies: idempotent double reply admitted, exact single journal',
      async () => {
        const wireSet = new ApplicationMigrationSet([
          tableBody('ts-mig-race-wire', 1n, 'wire race table', 'mig_race_wire'),
        ]);

        // SEPARATE database, TWO independent connections, ONE shared
        // apply-send barrier: each contender's `applyNext` performs its REAL
        // empty preflight status send untouched; its APPLY send is then held
        // at the barrier until BOTH contenders have arrived — i.e. both
        // preflights (empty journal) have completed as real wire requests
        // before either APPLY goes out. The barrier is pure promise
        // chaining (no timers, no sleeps).
        const db = await setupDurableDb(client!, 'race_wire');
        const connA = await bounded(connectAdmin(HOST, server!.port));
        let connB: ShamirClient | null = null;
        try {
          connB = await bounded(connectAdmin(HOST, server!.port));
          const { identity } = await discoverIdentity(connA, wireSet, db);

          // Shared apply-send barrier: the FIRST arriving APPLY send waits;
          // the SECOND arrival opens the gate and releases both. Reuses the
          // file's `makeGate` primitive.
          const releaseGate = makeGate();
          let arrivals = 0;
          const applyBarrierTransport = (
            conn: ShamirClient,
          ): Pick<ShamirClient, 'execute'> => ({
            execute: (b: string, batch: object): Promise<BatchResponse> => {
              if (!hasOp(batch, 'apply_application_schema_migration')) {
                // Real status (preflight) sends pass through untouched.
                return conn.execute(b, batch);
              }
              const arrival = ++arrivals;
              if (arrival === 2) releaseGate.open();
              // Held until BOTH applyNext calls have completed their real
              // empty preflights and arrived here with their APPLY.
              return releaseGate.wait.then(() => conn.execute(b, batch));
            },
          });

          const a = new ApplicationMigrationClient(
            applyBarrierTransport(connA),
            { database_identity: identity },
            wireSet,
          );
          const b = new ApplicationMigrationClient(
            applyBarrierTransport(connB),
            { database_identity: identity },
            wireSet,
          );

          // Observe both sends BEFORE either starts (no unobserved rejects).
          const outcomes = Promise.allSettled([
            bounded(a.applyNext(db)),
            bounded(b.applyNext(db)),
          ]);
          const settled = await outcomes;
          expect(settled).toHaveLength(2);
          expect(arrivals).toBe(2);

          // HONEST BOUNDARY: both contenders derived their apply from
          // completed empty-journal preflights, then both APPLY sends went
          // out over the real wire concurrently. With true simultaneous
          // server-side processing the fence admits the identical body
          // twice (idempotent double `applied` reply, apply.rs:134-149);
          // with the loser arriving while the winner still holds the fence
          // the server answers the stable `schema_migration_in_progress`
          // fence refusal (apply.rs:166-179, mapped 1:1 by
          // `mapMigrationServerError`). BOTH are admitted here; any OTHER
          // stable refusal would need its own confirmed reason and is NOT
          // accepted. The invariant under either admitted interleaving is
          // the EXACT single journal entry.
          const codes = new Set<string>();
          let applied = 0;
          for (const o of settled) {
            if (o.status === 'fulfilled') {
              expect(o.value.status).toBe('applied');
              expect(o.value.migration_id).toBe('ts-mig-race-wire');
              expect(BigInt(o.value.ordinal)).toBe(1n);
              applied += 1;
              codes.add('applied');
            } else {
              expect(
                o.reason,
                `unexpected wire-race rejection: ${String(o.reason)}`,
              ).toBeInstanceOf(MigrationError);
              expect((o.reason as MigrationError).code).toBe(
                'schema_migration_in_progress',
              );
              codes.add('schema_migration_in_progress');
            }
          }
          expect(codes.has('applied')).toBe(true);
          expect(applied).toBeGreaterThanOrEqual(1);

          // The invariant: EXACTLY ONE durable prefix entry, the exact
          // id / ordinal / checksum from the set; the fence advanced
          // EXACTLY to 2 — only ONE operation actually ran (an idempotent
          // second `applied` reply is a no-op on the fence).
          const after = await bounded(a.status(db));
          expect(after.applied).toHaveLength(1);
          expect(after.applied[0]!.id).toBe('ts-mig-race-wire');
          expect(after.applied[0]!.ordinal).toBe(1n);
          expect(after.applied[0]!.checksum).toEqual(wireSet.entries[0]!.checksum);
          expect(after.fence_generation).toBe(2n);
        } finally {
          for (const c of [connA, connB]) {
            if (!c) continue;
            try {
              await bounded(c.close());
            } catch {
              /* ok */
            }
          }
        }
      },
      TEST_TIMEOUT_MS,
    );
  },
);
