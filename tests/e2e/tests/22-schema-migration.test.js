/**
 * Application-schema MIGRATIONS through the Node napi binding
 * (release-1 R1c step 4, design §7.4). Batches are produced ONLY with the
 * typed TypeScript builders; the napi `execute(Buffer)` FFI is exercised
 * as TRANSPORT ONLY.
 *
 * 1. `..._accepts_typed_typescript_built_batch` — a builder-built status
 *    batch through the RAW Buffer FFI agrees with the typed status read.
 * 2. `..._error_and_status_round_trip` — applied-prefix status round trip;
 *    client-side `drift` refusal AND the wire-level top-level
 *    `DbResponse::Error` (`schema_migration_checksum_mismatch`, mapped by
 *    the wrapper to `ShamirDbError`); identity mismatch preserved.
 * 3. `..._does_not_retry_uncertain_apply` — REAL response loss through the
 *    controlled proxy (`helpers/controlled-proxy`): the apply lands and
 *    persists, the typed `unknown_outcome` is resolved as `applied` via
 *    read-only status — never resubmitted. Send counts are precise at the
 *    TS transport boundary only (see the test's observability note).
 */

'use strict';

const { encode, decode } = require('@msgpack/msgpack');
const { ShamirClient, ShamirDbError } = require('shamir-client');
const native = require('shamir-client/native');
const { createControlledProxy } = require('../helpers/controlled-proxy');
const {
  Batch,
  ddl,
  ApplicationMigrationClient,
  ApplicationMigrationSet,
  MigrationError,
  schemaMigration,
  migrationDdl,
  applicationSchemaStatus,
  applicationSchemaPlan,
  schemaMigrationSubmission,
} = require('@shamir/client');

const WRONG_IDENTITY = '1111111111111111'; // verified probe literal of the TS e2e harness
const STATUS_ALIAS = 'application_schema_status';
const PLAN_ALIAS = 'application_schema_plan';
const APPLY_ALIAS = 'apply_application_schema_migration';

/** Generous per-request deadline (120 s; the hang-detector convention). */
const REQUEST_DEADLINE_MS = 120_000;

// ── helpers ─────────────────────────────────────────────────────────

/** Bound any single await by the deadline; the timer never outlives it. */
function bounded(promise, ms = REQUEST_DEADLINE_MS) {
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`request deadline (${ms}ms) exceeded`)),
      ms,
    );
  });
  return Promise.race([promise, deadline]).finally(() => clearTimeout(timer));
}

function assert(cond, message) {
  if (!cond) throw new Error(message || 'expected truthy value');
}

function assertEq(actual, expected, message) {
  if (actual !== expected) {
    throw new Error(
      `${message || 'values differ'}: expected ${String(expected)}, got ${String(actual)}`,
    );
  }
}

function isMigrationError(e, code) {
  return e instanceof MigrationError && e.code === code;
}

function toHex(bytes) {
  return Buffer.from(bytes).toString('hex');
}

/**
 * The RAW napi `execute(db, Buffer)` FFI — transport only. The batch is
 * a builder-built object; encoding mirrors the wrapper's own contract
 * (`useBigInt64`), nothing is hand-built on the wire.
 */
async function executeBuffer(client, db, batchObj) {
  const buf = Buffer.from(encode(batchObj, { useBigInt64: true }));
  const resp = await bounded(native.ShamirClient.prototype.execute.call(client, db, buf));
  return decode(resp, { useBigInt64: true });
}

/** One `create_table` migration body at the given one-based ordinal. */
function tableBody(id, ordinal, name, table, opts) {
  return schemaMigration(id, ordinal, name, [migrationDdl.createTable(table, opts)]);
}

let localDbCounter = 0;
/** Per-process unique db name — no shared fixtures in this file. */
function uniqueDbName(label) {
  localDbCounter += 1;
  return `t_${label}_${process.pid}_${localDbCounter}`;
}

/** Durable Fjall repo database (the migrations journal needs it). */
async function setupDurableDb(client, label) {
  const db = uniqueDbName(label);
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
 * Identity discovery: a probe client over a deliberately WRONG identity
 * does one READ-ONLY status send; the reply carries the stable
 * `schema_migration_identity_mismatch` error and the SERVING identity.
 */
async function discoverIdentity(client, set, db) {
  const probe = new ApplicationMigrationClient(
    client,
    { database_identity: WRONG_IDENTITY },
    set,
  );
  const probeStatus = await bounded(probe.status(db));
  assertEq(probeStatus.server_error, 'schema_migration_identity_mismatch',
    'probe status should report the identity mismatch');
  const identity = probeStatus.reported_target.database_identity;
  assert(typeof identity === 'string' && identity.length > 0,
    'serving identity should be reported');
  assert(identity !== WRONG_IDENTITY, 'serving identity should differ from the probe');
  return identity;
}

module.exports = async function (ctx) {
  const { client, server, test } = ctx;

  // ──────────────────────────────────────────────────────────────────
  // §7.4 — the Buffer FFI accepts a typed TypeScript-built batch
  // ──────────────────────────────────────────────────────────────────

  test('node_migration_execute_accepts_typed_typescript_built_batch', async () => {
    const set = new ApplicationMigrationSet([
      tableBody('node-mig-typed-batch', 1, 'create items', 'items'),
    ]);
    const db = await setupDurableDb(client, 'node_mig_typed');
    const identity = await discoverIdentity(client, set, db);

    // Typed read through the binding (wrapper object FFI over the same
    // native transport): empty baseline.
    const migrations = new ApplicationMigrationClient(
      client,
      { database_identity: identity },
      set,
    );
    const st = await bounded(migrations.status(db));
    assert(!st.server_error, 'status should not report an error');
    assertEq(BigInt(st.last_applied_ordinal), 0n, 'journal head');
    assert(st.in_flight === undefined || st.in_flight === null,
      'no unresolved entry on a fresh journal');

    // The SAME read-only op, built by the wire builder, sent through the
    // RAW Buffer FFI: accepted, and the decoded reply agrees.
    const batch = Batch.create(`st-${db}`)
      .add(STATUS_ALIAS, applicationSchemaStatus({ database_identity: identity }))
      .build();
    const resp = await executeBuffer(client, db, batch);
    const record = resp.results[STATUS_ALIAS].records[0];
    assert(record, 'raw FFI status should return a record');
    assert(!record.error, 'raw FFI status should not report an error');
    assertEq(BigInt(record.last_applied_ordinal), 0n,
      'raw FFI status agrees with the typed client on the journal head');
    assertEq(record.target.database_identity, identity,
      'raw FFI status echoes the serving identity');
  });

  // ──────────────────────────────────────────────────────────────────
  // §7.4 — typed error and status round trip through the binding
  // ──────────────────────────────────────────────────────────────────

  test('node_migration_error_and_status_round_trip', async () => {
    const set = new ApplicationMigrationSet([
      tableBody('node-mig-round-trip', 1, 'create goods', 'goods'),
    ]);
    const db = await setupDurableDb(client, 'node_mig_rt');
    const identity = await discoverIdentity(client, set, db);
    const migrations = new ApplicationMigrationClient(
      client,
      { database_identity: identity },
      set,
    );

    // Apply ordinal 1 for real, then the FULL status round trip: the
    // applied prefix (id / ordinal / canonical checksum) matches the
    // local set exactly.
    const applyResult = await bounded(migrations.applyNext(db));
    assertEq(BigInt(applyResult.ordinal), 1n, 'applied ordinal');
    assertEq(applyResult.status, 'applied', 'journal status');
    const st = await bounded(migrations.status(db));
    assert(Array.isArray(st.applied) && st.applied.length === 1,
      'exactly one applied journal entry');
    const entry = set.atOrdinal(1);
    assertEq(st.applied[0].id, entry.body.metadata.id, 'applied id');
    assertEq(BigInt(st.applied[0].ordinal), 1n, 'applied ordinal');
    assertEq(toHex(st.applied[0].checksum), toHex(entry.checksum),
      'applied canonical checksum round-trips byte-faithfully');
    assert(BigInt(st.fence_generation) > 0n, 'fence generation is nonzero');

    // CHECKSUM DRIFT: a same-id same-ordinal body with a DIFFERENT
    // operation is refused as typed `drift` (checksum mismatch against
    // the journal), before any apply send.
    const drifted = new ApplicationMigrationSet([
      tableBody('node-mig-round-trip', 1, 'create goods', 'other_table'),
    ]);
    const driftedClient = new ApplicationMigrationClient(
      client,
      { database_identity: identity },
      drifted,
    );
    let driftErr = null;
    try {
      await bounded(driftedClient.applyNext(db));
    } catch (e) {
      driftErr = e;
    }
    assert(driftErr, 'drifted apply must be refused');
    assert(isMigrationError(driftErr, 'drift'),
      `expected typed drift error, got ${driftErr && driftErr.code}: ${driftErr && driftErr.message}`);
    assertEq(driftErr.retryable, false, 'drift is never retryable');

    // WIRE-LEVEL drift, asserted through the binding itself (the
    // client-side `drift` is never transmitted): a builder
    // `application_schema_plan` carrying the DRIFTED applied prefix is
    // refused by the real server with a TOP-LEVEL `DbResponse::Error`
    // (handler.rs: error_response) carrying the stable code
    // `schema_migration_checksum_mismatch` — NOT a per-op record error.
    const driftedSubmissions = drifted.entries.map(({ body, checksum }) =>
      schemaMigrationSubmission(body, checksum),
    );
    const driftPlanBatch = Batch.create(PLAN_ALIAS)
      .add(PLAN_ALIAS, applicationSchemaPlan(
        { database_identity: identity },
        {
          migration_id: st.applied[0].id,
          ordinal: st.applied[0].ordinal,
          checksum: st.applied[0].checksum,
        },
        driftedSubmissions,
      ))
      .build();
    const driftResp = await executeBuffer(client, db, driftPlanBatch);
    // Raw FFI: the plan rejection is the TOP-LEVEL error envelope
    // `{kind:'error',code,message}` — there is no `results` payload.
    assertEq(driftResp && driftResp.kind, 'error',
      'the drifted plan is a top-level DbResponse::Error');
    assertEq(driftResp && driftResp.code, 'schema_migration_checksum_mismatch',
      'the stable checksum-mismatch code on the wire');
    assert(String(driftResp && driftResp.message)
        .includes('redefines applied ordinal'),
      `diagnostic preserved: ${driftResp && driftResp.message}`);

    // The typed wrapper maps the same raw envelope to `ShamirDbError`
    // with the stable code (native lib.rs `encode_db_error` → wrapper.js
    // `decodeOrThrow`) — one execution, no duplicated raw assertion.
    let driftTypedErr = null;
    try {
      await bounded(client.execute(db, driftPlanBatch));
    } catch (e) {
      driftTypedErr = e;
    }
    assert(driftTypedErr instanceof ShamirDbError,
      `expected ShamirDbError, got ${driftTypedErr && driftTypedErr.name}`);
    assertEq(driftTypedErr.code, 'schema_migration_checksum_mismatch',
      'typed stable code from the raw error envelope');

    // SNAPSHOT: the refusal left the journal untouched (a plan never
    // writes): head still ordinal 1 with the ORIGINAL canonical
    // checksum, no unresolved entry, no reported journal error.
    const stAfterDrift = await bounded(migrations.status(db));
    assert(Array.isArray(stAfterDrift.applied) && stAfterDrift.applied.length === 1,
      'exactly one applied journal entry after the refused plan');
    assertEq(stAfterDrift.applied[0].id, entry.body.metadata.id,
      'applied id unchanged after the refused plan');
    assertEq(toHex(stAfterDrift.applied[0].checksum), toHex(entry.checksum),
      'applied canonical checksum unchanged after the refused plan');
    assert(stAfterDrift.in_flight === undefined || stAfterDrift.in_flight === null,
      'no unresolved entry after the refused plan');
    assert(!stAfterDrift.server_error,
      'no journal error reported after the refused plan');

    // IDENTITY: the wrong-identity probe preserves the server's stable
    // `schema_migration_identity_mismatch` code through the binding.
    let identProbeErr = null;
    try {
      const wrongTarget = new ApplicationMigrationClient(
        client,
        { database_identity: WRONG_IDENTITY },
        set,
      );
      await bounded(wrongTarget.applyNext(db));
    } catch (e) {
      identProbeErr = e;
    }
    assert(identProbeErr, 'wrong-identity apply must be refused');
    assert(isMigrationError(identProbeErr, 'schema_migration_identity_mismatch'),
      `expected typed identity-mismatch error, got ${identProbeErr && identProbeErr.code}`);
    assertEq(identProbeErr.serverCode, 'schema_migration_identity_mismatch',
      'server code preserved verbatim');
    assertEq(identProbeErr.retryable, false, 'identity mismatch is never retryable');

    // IN-DOUBT through REAL PUBLIC BUILDERS ONLY, no crash seam (fresh
    // database inside this test): the migration's table already exists, so
    // the handler's `if_not_exists` succeeds silently while the exact
    // created-from-absent postcondition FAILS — the server records
    // started + operation intent + in_doubt with no completion, exactly
    // like the shamir-db crash seam
    // (apply_execution_tests.rs `a_failed_postcondition_leaves_started_and_
    // intent_without_completion_and_is_reported`) but over the real wire.
    const indoubtDb = await setupDurableDb(client, 'node_mig_indoubt');
    await bounded(client.execute(indoubtDb,
      Batch.create(`ct-${indoubtDb}`)
        .add('ct', ddl.createTable('items_rt', { repo: 'main' }))
        .build()));
    const indoubtSet = new ApplicationMigrationSet([
      tableBody('node-mig-in-doubt', 1, 'conflicting create', 'items_rt',
        { repo: 'main', if_not_exists: true }),
    ]);
    const indoubtIdentity = await discoverIdentity(client, indoubtSet, indoubtDb);
    const indoubtMigrations = new ApplicationMigrationClient(
      client,
      { database_identity: indoubtIdentity },
      indoubtSet,
    );
    let postcondErr = null;
    try {
      await bounded(indoubtMigrations.applyNext(indoubtDb));
    } catch (e) {
      postcondErr = e;
    }
    assert(postcondErr, 'the conflicting create must be refused');
    assert(isMigrationError(postcondErr, 'schema_migration_postcondition_failed'),
      `expected typed postcondition failure, got ${postcondErr && postcondErr.code}`);
    assertEq(postcondErr.retryable, false, 'postcondition failure is never retryable');

    // Status reports the typed in-doubt: the stable `server_error` code,
    // the unresolved in-flight entry with its status, operation index and
    // the recorded postcondition evidence, and an EMPTY applied prefix.
    const indoubtSt = await bounded(indoubtMigrations.status(indoubtDb));
    assertEq(indoubtSt.server_error, 'schema_migration_in_doubt',
      'status fails closed with the stable in-doubt code');
    const flight = indoubtSt.in_flight;
    assert(flight, 'the unresolved in-flight entry is reported');
    assertEq(flight.migrationId, 'node-mig-in-doubt', 'in-flight id');
    assertEq(BigInt(flight.ordinal), 1n, 'in-flight ordinal');
    assertEq(flight.status, 'in_doubt', 'in-flight status');
    assertEq(BigInt(flight.operationIndex), 0n, 'in-flight operation index');
    assert(String(flight.inDoubtReason).includes('postcondition'),
      `evidence names the failed postcondition: ${flight.inDoubtReason}`);
    assert(Array.isArray(indoubtSt.applied) && indoubtSt.applied.length === 0,
      'nothing was applied');

    // PLAN REFUSAL during the unresolved window: the real server refuses
    // every plan while the journal is unresolved with the top-level
    // `DbResponse::Error` envelope carrying the stable
    // `schema_migration_in_doubt` code (plan.rs unresolved check), asserted
    // through the RAW Buffer FFI first.
    const emptyPredecessor = { ordinal: 0, checksum: new Array(32).fill(0) };
    const indoubtPlanBatch = Batch.create(PLAN_ALIAS)
      .add(PLAN_ALIAS, applicationSchemaPlan(
        { database_identity: indoubtIdentity },
        emptyPredecessor,
        indoubtSet.entries.map(({ body, checksum }) =>
          schemaMigrationSubmission(body, checksum)),
      ))
      .build();
    const indoubtPlanResp = await executeBuffer(client, indoubtDb, indoubtPlanBatch);
    assertEq(indoubtPlanResp && indoubtPlanResp.kind, 'error',
      'the plan during the unresolved window is a top-level DbResponse::Error');
    assertEq(indoubtPlanResp && indoubtPlanResp.code, 'schema_migration_in_doubt',
      'the stable in-doubt code on the wire');

    // The typed wrapper maps the SAME envelope to `ShamirDbError` with the
    // stable code (native lib.rs `encode_db_error` → wrapper.js
    // `decodeOrThrow`).
    let indoubtTypedErr = null;
    try {
      await bounded(client.execute(indoubtDb, indoubtPlanBatch));
    } catch (e) {
      indoubtTypedErr = e;
    }
    assert(indoubtTypedErr instanceof ShamirDbError,
      `expected ShamirDbError, got ${indoubtTypedErr && indoubtTypedErr.name}`);
    assertEq(indoubtTypedErr.code, 'schema_migration_in_doubt',
      'typed stable in-doubt code from the raw error envelope');

    // Honest boundary: the unsupported-operation code cannot be PROVOKED
    // here — it requires an OLD REAL PEER server or a SCRIPTED TLS PEER,
    // neither of which exists in this suite; that mapping is covered by
    // the TS/Rust unit suites instead.
  });

  // ──────────────────────────────────────────────────────────────────
  // §7.4 — no binding-level replay of an uncertain apply
  // ──────────────────────────────────────────────────────────────────

  test('node_migration_does_not_retry_uncertain_apply', async () => {
    const set = new ApplicationMigrationSet([
      tableBody('node-mig-no-retry', 1, 'create ledger', 'ledger'),
    ]);
    const db = await setupDurableDb(client, 'node_mig_noretry');
    const identity = await discoverIdentity(client, set, db);
    const migrations = new ApplicationMigrationClient(
      client,
      { database_identity: identity },
      set,
    );

    // REAL response loss through the controlled TCP proxy (the same
    // helper the connection-matrix tests use): an independent napi
    // client is connected THROUGH the proxy. The plain preflight
    // status flows freely before the gate; the gate arms exactly when
    // the typed APPLY send reaches the TS transport boundary, so the
    // server's apply REPLY is held at the proxy — the apply has already
    // executed server-side — and is then lost with the connection.
    //
    // Honest observability boundary: TCP CHUNK counts at the proxy are
    // NOT request counts (one request may span several chunks; chunks
    // may coalesce), so the precise send count is taken at the TS
    // transport boundary (`MigrationTransport.execute`). Below that —
    // inside the napi binding and the socket — this out-of-process Node
    // suite can only observe the proxy's armed request/response holds,
    // not exact per-request counts.
    const proxy = await bounded(
      createControlledProxy(server.host, server.port,
        { timeoutMs: REQUEST_DEADLINE_MS }),
    );
    let proxied = null;
    let applySends = 0;
    let statusSends = 0;
    try {
      proxied = await bounded(ShamirClient.connect({
        host: proxy.host,
        port: proxy.port,
        serverName: 'localhost',
        username: server.user,
        password: server.password,
        acceptNewHost: true,
        connectTimeoutMs: REQUEST_DEADLINE_MS,
        handshakeTimeoutMs: REQUEST_DEADLINE_MS,
        requestTimeoutMs: REQUEST_DEADLINE_MS,
      }));
      await bounded(proxied.ping());
      const throughProxy = {
        async execute(seamDb, batch) {
          const aliases = Object.keys((batch && batch.queries) || {});
          if (aliases.includes(APPLY_ALIAS)) {
            applySends += 1;
            proxy.arm(); // gate from THIS apply send on
          }
          if (aliases.includes(STATUS_ALIAS)) statusSends += 1;
          return bounded(proxied.execute(seamDb, batch));
        },
      };
      const proxiedMigrations = new ApplicationMigrationClient(
        throughProxy,
        { database_identity: identity },
        set,
      );

      // Plain REAL status before the armed gate: flows untouched.
      const baseline = await bounded(proxiedMigrations.status(db));
      assert(!baseline.server_error, 'baseline status should not error');
      assertEq(BigInt(baseline.last_applied_ordinal), 0n, 'baseline journal head');

      // The typed APPLY send: the request is forwarded to the real
      // server, whose reply the proxy holds.
      let err = null;
      const applyPending = bounded(proxiedMigrations.applyNext(db));
      const applyGuard = applyPending.catch(() => undefined);
      await bounded(proxy.waitForClientData('apply request reaching the server'));
      await bounded(proxy.waitForHeldResponse('apply reply held at the proxy'));
      assertEq(applySends, 1,
        'EXACTLY ONE apply send at the TS transport boundary');
      assertEq(statusSends, 2, 'baseline + preflight status only');

      // The apply DID land: a fresh DIRECT real status OUTSIDE the proxy
      // proves the persisted journal event BEFORE the connection is
      // dropped — and the pending send count must not have moved.
      const stAfterLoss = await bounded(migrations.status(db));
      assert(Array.isArray(stAfterLoss.applied) && stAfterLoss.applied.length === 1,
        'the lost-response apply persisted exactly ONE journal event');
      assertEq(stAfterLoss.applied[0].id, 'node-mig-no-retry', 'journal id');
      assertEq(BigInt(stAfterLoss.applied[0].ordinal), 1n, 'journal ordinal');
      assertEq(applySends, 1,
        'still exactly ONE apply send after the persisted journal is proven');

      // Drop the connection: the held reply is discarded and the native
      // call rejects — that rejection is the observed transport failure
      // the typed caller must classify, never a replay.
      await bounded(proxy.close());
      try {
        await applyPending;
      } catch (e) {
        err = e;
      }
      await applyGuard;
      assert(err, 'the lost apply response must surface as an error');
      assert(isMigrationError(err, 'unknown_outcome'),
        `expected typed unknown_outcome, got ${err && err.code}: ${err && err.message}`);
      assertEq(err.retryable, false, 'unknown outcome is never retryable');
      assert(err.attempt, 'the frozen attempt token is attached');
      assertEq(err.attempt.db, db, 'attempt token db');
      assertEq(err.attempt.databaseIdentity, identity, 'attempt token identity');
      assertEq(err.attempt.migrationId, 'node-mig-no-retry', 'attempt token id');
      assertEq(BigInt(err.attempt.ordinal), 1n, 'attempt token ordinal');
      assertEq(err.attempt.checksum, toHex(set.atOrdinal(1).checksum),
        'attempt token checksum');
      assertEq(applySends, 1,
        'still exactly ONE apply send AFTER the pending rejection');

      // The caller resolves the SAME attempt as `applied` through a
      // counted transport that admits READ-ONLY STATUS only: any apply
      // alias there would prove a replay, so it throws instead.
      let resolveStatuses = 0;
      const statusOnly = {
        async execute(seamDb, batch) {
          const aliases = Object.keys((batch && batch.queries) || {});
          assert(!aliases.includes(APPLY_ALIAS),
            'REPLAY GUARD: no apply may be sent during resolution');
          if (aliases.includes(STATUS_ALIAS)) resolveStatuses += 1;
          return bounded(client.execute(seamDb, batch));
        },
      };
      const resolver = new ApplicationMigrationClient(
        statusOnly,
        { database_identity: identity },
        set,
      );
      const verdict = await bounded(resolver.resolveUnknownApply(err.attempt));
      assertEq(verdict && verdict.kind, 'applied', 'reconciliation verdict');
      assertEq(verdict && verdict.migrationId, 'node-mig-no-retry', 'verdict id');
      assert(BigInt(verdict && verdict.ordinal) === 1n, 'verdict ordinal');
      assertEq(resolveStatuses, 1,
        'resolution went through EXACTLY ONE real status read');
      assertEq(applySends, 1,
        'still exactly ONE apply send AFTER reconciliation — never replayed');

      // Journal invariant afterwards: still exactly ONE applied event.
      const stFinal = await bounded(migrations.status(db));
      assert(Array.isArray(stFinal.applied) && stFinal.applied.length === 1,
        'exactly ONE applied journal event for the ordinal at the end');
      assertEq(BigInt(stFinal.applied[0].ordinal), 1n, 'journal ordinal');
      assertEq(stFinal.applied[0].id, 'node-mig-no-retry', 'journal id');

      // The migration's PHYSICAL effect is real: the created table reads
      // back through a typed `describe_table` batch.
      const desc = await bounded(client.execute(db,
        Batch.create(`desc-${db}`)
          .add('d', ddl.describeTable('ledger'))
          .build()));
      const descRec = desc.results.d.records[0];
      assert(descRec && !descRec.error, 'describe_table should return a record');
      assertEq(descRec.describe_table, 'ledger',
        'the applied migration physically created the table');
    } finally {
      await bounded(proxy.close());
      if (proxied) {
        await bounded(proxied.close().catch(() => {}));
      }
    }
  });
};
