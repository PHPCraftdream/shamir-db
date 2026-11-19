/**
 * `ApplicationMigrationClient` — the TypeScript mirror of the Rust
 * migration tool in
 * `crates/client/shamir-client/src/receipts/schema_migration/`
 * (`status.rs` / `plan.rs` / `apply.rs` / `resolve.rs`), composed over a
 * `ShamirClient` (only `execute` is required) instead of growing
 * `client.ts` / `db.ts`.
 *
 * Semantics mirrored exactly (design §5.1–§5.3):
 *   - `status(db)`: one read-only `application_schema_status` send, the
 *     result decoded and compared with the local set through the ONE
 *     shared prefix verification (`./status.js`). Pure read.
 *   - `plan(db)`: a `status` preflight first, then a read-only
 *     `application_schema_plan` carrying the WHOLE local set (applied
 *     prefix included) with the journal head reported by the preflight as
 *     the expected predecessor. The plan reply is validated against the
 *     promised migration (protocol version, full metadata, canonical
 *     checksum, exact predecessor) before it is returned. Never writes.
 *     An unresolved journal is refused through the in-flight entry's OWN
 *     status (`in_flight_error` split), never through the blanket
 *     `schema_migration_in_doubt` code the real server also reports for
 *     a merely RUNNING migration.
 *   - `applyNext(db)`: refuses BEFORE any send on prefix drift, a
 *     reported-prefix gap, a journal beyond the local set, an unverified
 *     journal checksum, a contradictory fence shape, a server-reported
 *     journal error, or a fully covered set. Otherwise sends exactly ONE
 *     next ordinal with the status-derived expected predecessor and fence
 *     generation. A matching unresolved in-flight head (same id and
 *     ordinal as the next local body, nonzero REPORTED fence generation,
 *     a known journal event-kind status) is the explicit identical-body
 *     resume: the same stored body is submitted with the fence generation
 *     the status reported for that event — the server re-checks its
 *     persisted checksum before any effect.
 *   - Lost responses (§5.3): a transport failure, timeout, limit or
 *     undecodable reply after an apply was SENT throws the typed
 *     `unknown_outcome` error carrying an opaque frozen `UnknownApply`
 *     attempt token — the apply is NEVER resent, and no `retryable` flag
 *     on any transport error is honoured.
 *   - `resolveUnknownApply(attempt)`: validates the token against the set
 *     (exact id / ordinal / canonical checksum), runs ONE read-only
 *     `status` on the token's database and maps the outcome
 *     (`applied` / `ready_to_apply` /
 *     `evidence_unavailable` / `unmatched_unresolved`); a matching
 *     unresolved in-flight head THROWS the typed refusal classified by
 *     the flight's OWN status via the shared `in_flight_error` split:
 *     the uncertain window (`in_doubt` / `failed`) is the
 *     `schema_migration_in_doubt` error carrying `.operationIndex` and
 *     `.reason`; every other event status (running, recovery) is
 *     `schema_migration_in_progress`. A DIFFERENT migration's flight is
 *     never treated as proof of this attempt: a reported `in_doubt` code
 *     beside a non-matching flight is classified by that flight's status,
 *     and without any reported error the established `unmatched_unresolved`
 *     verdict is returned (a later other-flight never shadows a PROVEN
 *     applied attempt — proof-first). Never resends.
 *
 * There is no down/rollback API and no fallback to individual DDL calls.
 * Read-only failures (`status` / `plan` / `resolve`) are typed and thrown,
 * never retried here; the caller may simply repeat a read.
 *
 * PLATFORM-AGNOSTIC.
 */

import { Batch } from '../../../../builders/index.js';
import type { ShamirClient } from '../../../../client/client.js';
import { ShamirDbError } from '../../../../support/errors.js';
import type {
  ApplicationSchemaEntry,
  ChecksumRequest,
  SchemaMigrationApplyResult,
  SchemaMigrationInFlightResult,
  SchemaMigrationPlanResult,
  SchemaMigrationPredecessor,
  SchemaMigrationStatusResult,
  SchemaMigrationTarget,
  U64,
} from '../../../../types/schema/migration-protocol.js';
import type {
  ApplicationMigrationSet,
  MigrationSetEntry,
} from '../canonical/set.js';
import {
  applicationSchemaPlan,
  applicationSchemaStatus,
  applyApplicationSchemaMigration,
  schemaMigrationSubmission,
} from '../wire.js';
import {
  decodeMigrationApply,
  decodeMigrationPlan,
  decodeMigrationStatus,
  sameChecksum,
} from './decode.js';
import {
  MigrationError,
  mapMigrationServerError,
  type UnknownApply,
} from './error.js';
import {
  compareMigrationStatus,
  verifyPrefix,
  verifyReportedPrefix,
  type MigrationStatus,
} from './status.js';

// ── Constants and small helpers ─────────────────────────────────────

/** Batch alias / batch id used for every single-op send. */
const STATUS_ALIAS = 'application_schema_status';
const PLAN_ALIAS = 'application_schema_plan';
const APPLY_ALIAS = 'apply_application_schema_migration';

/** The only canonical format version this client speaks. */
const PROTOCOL_VERSION = 1;

/** Journal event-kind statuses a matching resume may run under. */
const RESUMABLE_FLIGHT_STATUSES: ReadonlySet<string> = new Set([
  'started',
  'operation_intent',
  'operation_complete',
  'in_doubt',
  'failed',
  'recovery_acquired',
]);

/** Something with `execute(db, batch)` — the minimal client surface. */
export interface MigrationTransport {
  execute(
    db: string,
    batch: object,
  ): Promise<{ results: Record<string, { records: unknown[] }> }>;
}

/** Normalize a u64 wire value (`number | bigint`) to `bigint`. */
function asBigInt(value: U64): bigint {
  return typeof value === 'bigint' ? value : BigInt(value);
}

/** Lowercase-hex encoding of a byte checksum, for typed error payloads. */
function toHex(bytes: Uint8Array): string {
  let hex = '';
  for (const byte of bytes) {
    hex += byte.toString(16).padStart(2, '0');
  }
  return hex;
}

/**
 * One local set entry resolved through the set's own API: `atOrdinal`
 * returns the validated `MigrationSetEntry` (cloned body plus cached
 * canonical checksum at a one-based ordinal). The body's
 * `metadata.ordinal` (u64 wire value, possibly a bigint) must agree with
 * the requested ordinal; a divergence is an invalid set, never silently
 * accepted.
 */
function setEntryAt(
  set: ApplicationMigrationSet,
  ordinal: bigint,
): MigrationSetEntry | undefined {
  const entry = set.atOrdinal(ordinal);
  if (entry === undefined) return undefined;
  const bodyOrdinal = BigInt(entry.body.metadata.ordinal);
  if (bodyOrdinal !== ordinal) {
    throw new MigrationError(
      'invalid_set',
      `migration ${entry.body.metadata.id} carries ordinal ${String(bodyOrdinal)} but was resolved at ${String(ordinal)}`,
    );
  }
  return entry;
}

// ── Resolution outcome ──────────────────────────────────────────────

/**
 * Typed outcome of reconciling an unknown apply with one status query
 * (mirror of `UnknownApplyResolution` in the Rust `resolve.rs`).
 */
export type UnknownApplyResolution =
  /** The attempt's exact id/ordinal/checksum appears in the VALID
   * reported applied prefix — applied, never resend, even when a later
   * migration has overtaken the head. */
  | { kind: 'applied'; migrationId: string; ordinal: bigint }
  /** Verified absence: proven coverage, no unresolved event, the head
   * being exactly the attempt's predecessor and the attempt being the
   * next local ordinal — safe to call `applyNext` again on the SAME set
   * (the identical-body server resume re-checks the persisted checksum). */
  | { kind: 'ready_to_apply' }
  /** The journal moved past (or cannot prove) the attempt without
   * proving it applied: reconcile out of band. */
  | { kind: 'evidence_unavailable'; journalOrdinal: bigint }
  /** An unresolved event exists for a DIFFERENT migration. */
  | { kind: 'unmatched_unresolved'; journalOrdinal: bigint };

// ── Shared send/decode helpers ──────────────────────────────────────

/**
 * The typed refusal for an UNRESOLVED journal head, classified by the
 * in-flight entry's OWN status — the exact mirror of `in_flight_error` in
 * the Rust `apply.rs`: the uncertain window (`in_doubt` / `failed`) is
 * the `schema_migration_in_doubt` error preserving the server's pending
 * operation index and recorded reason when reported; every other event
 * status (`started`, `operation_intent`, `operation_complete`,
 * `recovery_acquired`, …) is `schema_migration_in_progress` — the
 * migration is unresolved and holds the fence. The real server reports
 * the `schema_migration_in_doubt` CODE for a merely RUNNING migration
 * too, so callers must classify through the flight entry, never through
 * the reported code alone.
 */
function inFlightError(
  flight: SchemaMigrationInFlightResult,
  attempt?: UnknownApply,
): MigrationError {
  if (flight.status === 'in_doubt' || flight.status === 'failed') {
    const index =
      flight.operation_index !== undefined
        ? asBigInt(flight.operation_index)
        : undefined;
    const reason = flight.in_doubt_reason;
    const message =
      reason !== undefined && index !== undefined
        ? `migration is in doubt at operation ${index}: ${reason}`
        : reason !== undefined
          ? `migration is in doubt: ${reason}`
          : index !== undefined
            ? `migration is in doubt at operation ${index}`
            : 'migration is in doubt';
    return new MigrationError('schema_migration_in_doubt', message, {
      serverCode: 'schema_migration_in_doubt',
      ...(attempt !== undefined ? { attempt } : {}),
      ...(index !== undefined ? { operationIndex: index } : {}),
      ...(reason !== undefined ? { reason } : {}),
    });
  }
  return new MigrationError(
    'schema_migration_in_progress',
    `migration is unresolved (${JSON.stringify(flight.status)}); it holds the fence`,
    {
      serverCode: 'schema_migration_in_progress',
      ...(attempt !== undefined ? { attempt } : {}),
    },
  );
}

/**
 * The stable structural shape a definitive server rejection carrier must
 * present: `name === 'ShamirDbError'` and a string `code`. The `detail`
 * is OPTIONAL — the Node binding (`shamir-client` `js/wrapper.js`)
 * throws its own `ShamirDbError` class (same stable `name`, string
 * `code` / `detail`, same `db error [code]: detail` message) with a
 * DIFFERENT prototype, so no `instanceof` — not even of `Error` across
 * realms — may gate recognition.
 */
type ShamirDbErrorShape = Pick<ShamirDbError, 'code'> & {
  detail?: ShamirDbError['detail'];
};

/** Recognize a definitive server rejection across runtimes. */
function isShamirDbError(error: unknown): error is ShamirDbErrorShape {
  if (error instanceof ShamirDbError) return true;
  if (typeof error !== 'object' || error === null) return false;
  const shaped = error as ShamirDbErrorShape & { name?: unknown };
  return shaped.name === 'ShamirDbError' && typeof shaped.code === 'string';
}

/** The carrier's detail, safe when the field is absent (empty string). */
function dbErrorDetail(error: ShamirDbErrorShape): string {
  return typeof error.detail === 'string' ? error.detail : '';
}

/**
 * Map a DEFINITIVE server rejection from the apply send (a `ShamirDbError`
 * from either runtime — see {@link isShamirDbError}) into the typed
 * migration error — EXCEPT for the non-definitive codes (`timeout`,
 * `limits`, `result_too_large`) and any non-`ShamirDbError` failure, which
 * stay unmapped so the caller types them as unknown-outcome.
 */
function mapDefiniteServerError(error: unknown): MigrationError | undefined {
  if (!isShamirDbError(error)) return undefined;
  if (
    error.code === 'timeout' ||
    error.code === 'limits' ||
    error.code === 'result_too_large'
  ) {
    return undefined;
  }
  return mapMigrationServerError(error.code, dbErrorDetail(error));
}

/**
 * Send one single-op batch and return the single record the server filed
 * under the alias. A missing alias or an empty record list is a response
 * shape failure (read-only calls: typed shape error; the apply caller
 * wraps its own send into an unknown-outcome error itself).
 */
async function sendSingle(
  client: MigrationTransport,
  db: string,
  alias: string,
  op: ApplicationSchemaEntry,
): Promise<unknown> {
  const batch = Batch.create(alias).add(alias, op).build();
  const response = await client.execute(db, batch);
  const result = response.results[alias];
  if (result === undefined || result.records.length === 0) {
    throw new MigrationError(
      'shape_mismatch',
      `server returned no payload for the application-schema op (alias '${alias}')`,
    );
  }
  return result.records[0];
}

/**
 * Read-only wrapper around `sendSingle`: every definitive server
 * rejection (`ShamirDbError`) is typed through
 * `mapMigrationServerError(code, detail)` — including the old-server
 * exact `invalid_request` text — while any other failure propagates
 * unchanged. Read-only calls have no uncertain outcome.
 */
async function sendSingleRead(
  client: MigrationTransport,
  db: string,
  alias: string,
  op: ApplicationSchemaEntry,
): Promise<unknown> {
  try {
    return await sendSingle(client, db, alias, op);
  } catch (error) {
    if (isShamirDbError(error)) {
      throw mapMigrationServerError(error.code, dbErrorDetail(error));
    }
    throw error;
  }
}

/**
 * The typed predecessor for a plan/apply request, checked: the baseline
 * (`migration_id` omitted, ordinal 0, all-zero checksum) for an empty
 * journal, otherwise the reported head with id AND checksum present. A
 * head reported WITHOUT its proof fields is never filled with an all-zero
 * fallback — it fails closed.
 */
function checkedPredecessor(
  raw: SchemaMigrationStatusResult,
): SchemaMigrationPredecessor {
  if (asBigInt(raw.last_applied_ordinal) === 0n) {
    return { ordinal: 0, checksum: new Array<number>(32).fill(0) };
  }
  if (raw.last_applied_id !== undefined && raw.last_applied_checksum !== undefined) {
    return {
      migration_id: raw.last_applied_id,
      ordinal: raw.last_applied_ordinal,
      checksum: Array.from(raw.last_applied_checksum) as ChecksumRequest,
    };
  }
  throw new MigrationError(
    'journal_unverified',
    `journal head at ordinal ${raw.last_applied_ordinal} reports no id/checksum proof (validation ${raw.checksum_validation})`,
  );
}

/**
 * Map the shared prefix verification onto the typed preflight refusals
 * (mirror of `prefix_check` in the Rust `apply.rs`): gap, drift at any
 * applied ordinal, a journal beyond the local set, an unverified journal
 * checksum, or a contradictory shape — all before any apply send.
 */
function prefixCheck(
  raw: SchemaMigrationStatusResult,
  set: ApplicationMigrationSet,
): void {
  const coverage = verifyPrefix(raw, set);
  if (coverage.kind === 'empty' || coverage.kind === 'verified') return;
  switch (coverage.kind) {
    case 'gap':
      throw new MigrationError(
        'schema_migration_gap',
        `the applied prefix reported by the journal has a gap: expected ordinal ${coverage.expected} but found ${coverage.actual}`,
      );
    case 'drift':
      throw new MigrationError(
        'drift',
        `drift at ordinal ${coverage.ordinal}: journal has ${coverage.journalId} / ${toHex(coverage.journalChecksum)} but the local set has ${coverage.localId}`,
      );
    case 'journal_beyond_set':
      throw new MigrationError(
        'journal_beyond_set',
        `journal head ordinal ${coverage.ordinal} is beyond the local migration set`,
      );
    case 'unverified':
      // A NONEMPTY journal reporting fence generation 0 is a CONTRADICTORY
      // shape (the fresh send would fence against an impossible
      // generation); a `valid` checksum verdict with an unverifiable
      // prefix is a malformed report shape; anything else is an
      // unverifiable journal checksum. All refuse before any send.
      if (
        asBigInt(raw.last_applied_ordinal) !== 0n &&
        asBigInt(raw.fence_generation) === 0n
      ) {
        throw new MigrationError(
          'shape_mismatch',
          `clean journal head at ordinal ${raw.last_applied_ordinal} reports fence generation 0`,
        );
      }
      if (raw.checksum_validation === 'valid') {
        throw new MigrationError(
          'shape_mismatch',
          `status reports checksum_validation valid but the applied prefix at ordinal ${raw.last_applied_ordinal} cannot be verified (malformed head tuple or missing/inconsistent applied list)`,
        );
      }
      throw new MigrationError(
        'journal_unverified',
        `journal checksum could not be verified (${raw.checksum_validation}); refusing to plan or apply`,
      );
  }
}

// ── The client ──────────────────────────────────────────────────────

/**
 * The typed application-schema migration tool over one database target
 * and one local set (compose it per set/target; it holds no session
 * state).
 */
export class ApplicationMigrationClient {
  private readonly client: MigrationTransport;
  private readonly target: SchemaMigrationTarget;
  private readonly set: ApplicationMigrationSet;

  constructor(
    client: Pick<ShamirClient, 'execute'>,
    target: SchemaMigrationTarget,
    set: ApplicationMigrationSet,
  ) {
    this.client = client;
    this.target = { database_identity: target.database_identity };
    this.set = set;
  }

  /**
   * One read-only `status` send: the journal decoded and compared with
   * the local set through the ONE shared prefix verification. Pure read;
   * nothing is ever written. Old-server refusal is enforced by the
   * decoder: a nonempty journal without the applied prefix / fence
   * generation is typed unsupported-server before any comparison runs.
   */
  async status(db: string): Promise<MigrationStatus> {
    const raw = await this.queryStatusRaw(db);
    return compareMigrationStatus(raw, this.set);
  }

  /**
   * The server's read-only plan for the NEXT ordinal: a `status` preflight
   * issues the expected predecessor, then ONE `application_schema_plan`
   * carrying the WHOLE local set (applied prefix included) is sent. The
   * reply is validated against the promised migration (exact protocol
   * version, full metadata, canonical checksum and predecessor) before it
   * is returned; any other reply shape fails typed. Never writes.
   */
  async plan(db: string): Promise<SchemaMigrationPlanResult> {
    const raw = await this.queryStatusRaw(db);
    if (raw.in_flight !== undefined) {
      // An unresolved journal is refused through the flight entry's OWN
      // status (`in_flight_error` split): a RUNNING migration reports the
      // `schema_migration_in_doubt` code on the wire too, but its typed
      // refusal is `schema_migration_in_progress`.
      throw inFlightError(raw.in_flight);
    }
    if (raw.error !== undefined) {
      // Any OTHER reported journal error (no in-flight owner) maps typed.
      throw mapMigrationServerError(
        raw.error,
        'the server reported a journal error; no plan is issued',
      );
    }
    const headOrdinal = asBigInt(raw.last_applied_ordinal);
    const next = this.atNextOrdinal(headOrdinal);
    if (next === undefined) {
      if (headOrdinal > BigInt(this.set.length)) {
        throw new MigrationError(
          'journal_beyond_set',
          `journal head ordinal ${headOrdinal} is beyond the local migration set`,
        );
      }
      throw new MigrationError(
        'nothing_to_apply',
        'nothing to apply: every migration in the set is already applied',
      );
    }
    const expectedPredecessor = checkedPredecessor(raw);
    // `SchemaMigrationSubmission::new` in wire form: metadata, operations
    // and (nonempty) function_dependencies copied, checksum as the fixed
    // 32-number wire array. One call per body of the WHOLE set.
    const migrations = this.set.entries.map(({ body, checksum }) =>
      schemaMigrationSubmission(body, checksum),
    );
    const record = await sendSingleRead(
      this.client,
      db,
      PLAN_ALIAS,
      applicationSchemaPlan(this.target, expectedPredecessor, migrations),
    );
    const plan = decodeMigrationPlan(record);
    if (
      asBigInt(plan.protocol_version) !== BigInt(PROTOCOL_VERSION) ||
      !this.planEchoesNext(plan, next, expectedPredecessor)
    ) {
      throw new MigrationError(
        'shape_mismatch',
        `plan reply does not match the promised migration ${next.body.metadata.id} (ordinal ${next.ordinal})`,
      );
    }
    return plan;
  }

  /**
   * Apply exactly the NEXT ordinal of the set. The status preflight
   * verifies the whole applied prefix against the local set and resolves
   * the send path (fresh baseline or matching identical-body resume)
   * BEFORE anything is sent; every refusal above happens before the send.
   * After the send, an echo that does not describe the submitted body, a
   * non-`applied` journal status, or any transport/decode failure throws
   * typed `unknown_outcome` with the frozen attempt token — never a
   * resend, and a result-record error is only trusted on a CONFIRMED
   * echo. Never honours a `retryable` transport classification.
   */
  async applyNext(db: string): Promise<SchemaMigrationApplyResult> {
    const raw = await this.queryStatusRaw(db);
    // 1. Identity first: a mismatched echo (the decoder echoes the
    //    SERVING identity on the stable identity-mismatch error) is the
    //    typed identity error before any coverage runs.
    if (raw.error === 'schema_migration_identity_mismatch') {
      throw mapMigrationServerError(
        raw.error,
        `the server answered for identity ${raw.target.database_identity}, not ${this.target.database_identity}`,
      );
    }
    // 2. Whole-prefix verification first, for every path. A real
    //    unresolved journal reports `schema_migration_in_doubt`; that
    //    code must NOT invalidate the applied-prefix coverage (otherwise
    //    an identical-body resume against a NONEMPTY prefix would be
    //    impossible) — only an identity mismatch hides coverage. Strip
    //    the in-doubt code for the coverage check alone; it is handled
    //    against the in-flight entry below. (Mirrors the Rust server's
    //    own recheck of the stored prefix before a resume.)
    const coverageRaw =
      raw.error === 'schema_migration_in_doubt'
        ? { ...raw, error: undefined }
        : raw;
    prefixCheck(coverageRaw, this.set);
    // 3. Any OTHER server-reported journal error before NothingToApply.
    if (raw.error !== undefined && raw.error !== 'schema_migration_in_doubt') {
      throw mapMigrationServerError(
        raw.error,
        'the server reported a journal error; nothing is applied',
      );
    }
    // 4. The next local ordinal of the verified set.
    const headOrdinal = asBigInt(raw.last_applied_ordinal);
    const next = this.atNextOrdinal(headOrdinal);
    if (next === undefined) {
      throw new MigrationError(
        'nothing_to_apply',
        'nothing to apply: every migration in the set is already applied',
      );
    }
    // 5. Resolve the send path with the in-flight guard.
    const flight = raw.in_flight;
    let expectedFenceGeneration: U64;
    if (
      flight !== undefined &&
      flight.migration_id === next.body.metadata.id &&
      asBigInt(flight.ordinal) === next.ordinal
    ) {
      // Matching resume: the identical stored body with the fence
      // generation the status REPORTED for the in-flight event; the
      // server re-checks its persisted checksum before any effect. Fail
      // closed on an unknown journal state.
      if (
        asBigInt(flight.fence_generation) === 0n ||
        !RESUMABLE_FLIGHT_STATUSES.has(flight.status)
      ) {
        throw new MigrationError(
          'shape_mismatch',
          `unresolved entry reports status ${JSON.stringify(flight.status)} / generation ${flight.fence_generation} — not a resumeable journal state`,
        );
      }
      expectedFenceGeneration = flight.fence_generation;
    } else if (flight !== undefined) {
      // Unmatched unresolved entry: classify by the flight's OWN status
      // (`in_flight_error` split — the uncertain window is in_doubt with
      // the server's evidence, anything else in_progress holding the
      // fence), never the blanket wire code.
      throw inFlightError(flight);
    } else {
      if (raw.error !== undefined) {
        // An `in_doubt` code without an in-flight entry is an unknown
        // journal state; map it typed, never resume.
        throw mapMigrationServerError(
          raw.error,
          'the server reported a journal error; nothing is applied',
        );
      }
      if (headOrdinal !== 0n && asBigInt(raw.fence_generation) === 0n) {
        throw new MigrationError(
          'shape_mismatch',
          `clean journal head at ordinal ${headOrdinal} reports fence generation 0`,
        );
      }
      // Fresh start: the top-level durable generation (0 when empty).
      expectedFenceGeneration = raw.fence_generation;
    }
    // 6. The attempt token is bound BEFORE the send: database, target
    //    identity and the exact id/ordinal/checksum of the body in flight.
    const attempt: UnknownApply = Object.freeze({
      db,
      databaseIdentity: this.target.database_identity,
      migrationId: next.body.metadata.id,
      ordinal: next.ordinal,
      checksum: toHex(next.checksum),
    });
    // 7. Send exactly ONE apply. Any non-definitive outcome after the
    //    send (transport failure, timeout, limit, close, undecodable
    //    reply) is typed unknown_outcome — the apply is never resent and
    //    no `retryable` transport flag is honoured.
    let record: unknown;
    try {
      record = await sendSingle(
        this.client,
        db,
        APPLY_ALIAS,
        applyApplicationSchemaMigration(
          this.target,
          checkedPredecessor(raw),
          expectedFenceGeneration,
          // `SchemaMigrationSubmission::new` in wire form — the exact
          // shape the identical-body server resume re-checks.
          schemaMigrationSubmission(next.body, next.checksum),
        ),
      );
    } catch (error) {
      // A DEFINITIVE server rejection (a `ShamirDbError` of EITHER
      // runtime — the TS nominal class or the Node binding's own class,
      // see `isShamirDbError`) is mapped as-is with the verbatim detail
      // — it is a known outcome, never uncertainty. Anything else
      // (timeout, limit, close, transport failure) leaves the outcome
      // UNKNOWN after the send: typed unknown_outcome with the frozen
      // attempt token, no `cause` nesting, no retryable flag honoured,
      // never a resend.
      throw (
        mapDefiniteServerError(error) ??
        new MigrationError(
          'unknown_outcome',
          'outcome unknown; query status (resolveUnknownApply) — the apply was not resent',
          { attempt },
        )
      );
    }
    let result: SchemaMigrationApplyResult;
    try {
      result = decodeMigrationApply(record);
    } catch {
      throw new MigrationError(
        'unknown_outcome',
        'outcome unknown; the apply reply could not be decoded — query status',
        { attempt },
      );
    }
    // 8. Identity/protocol echo FIRST: a reply that does not describe the
    //    body we sent, or speaks another protocol version, is UNKNOWN —
    //    its `error` field (if any) must never hide that uncertainty.
    if (
      asBigInt(result.protocol_version) !== BigInt(PROTOCOL_VERSION) ||
      result.migration_id !== attempt.migrationId ||
      asBigInt(result.ordinal) !== attempt.ordinal ||
      result.checksum === undefined ||
      !sameChecksum(result.checksum, next.checksum)
    ) {
      throw new MigrationError(
        'unknown_outcome',
        `outcome unknown; the apply result does not describe the submitted migration (migration_id ${JSON.stringify(result.migration_id)}, ordinal ${result.ordinal}, protocol_version ${result.protocol_version}) — query status`,
        { attempt },
      );
    }
    // 9. A result-record error on the CONFIRMED echo is a stable server
    //    state, never an Ok and never uncertainty.
    if (result.error !== undefined) {
      throw mapMigrationServerError(
        result.error,
        `apply of ${result.migration_id} (ordinal ${next.ordinal}) was recorded with an error state`,
      );
    }
    // 10. A confirmed echo whose journal status is not `applied` leaves
    //    the durable outcome unknown.
    if (result.status !== 'applied') {
      throw new MigrationError(
        'unknown_outcome',
        `outcome unknown; the apply result status is ${JSON.stringify(result.status)}, not applied — query status`,
        { attempt },
      );
    }
    return result;
  }

  /**
   * Reconcile an uncertain apply with ONE read-only `status` query for
   * the same migration id (design §5.3). The token is validated against
   * the set first (exact id, ordinal and canonical checksum at its
   * ordinal) so every verdict is bound to the identical body; a mismatch
   * fails closed before any send. The apply is never resent; on
   * `ready_to_apply` the caller may call `applyNext` again with the SAME
   * set — the server's identical-body resume path re-checks the persisted
   * checksum before any effect.
   */
  async resolveUnknownApply(attempt: UnknownApply): Promise<UnknownApplyResolution> {
    // The token belongs to THIS runner: target identity, exact ordinal,
    // id and canonical checksum of the set's body — any divergence fails
    // closed before any send.
    if (attempt.databaseIdentity !== this.target.database_identity) {
      throw new MigrationError(
        'attempt_does_not_match_set',
        `the attempt token targets identity ${attempt.databaseIdentity}, not this runner's ${this.target.database_identity}`,
      );
    }
    const attemptOrdinal = asBigInt(attempt.ordinal);
    const entry = this.set.atOrdinal(attemptOrdinal);
    const attemptChecksum =
      entry !== undefined && entry.body.metadata.id === attempt.migrationId
        ? entry.checksum
        : undefined;
    if (
      entry === undefined ||
      attemptChecksum === undefined ||
      toHex(attemptChecksum) !== attempt.checksum
    ) {
      throw new MigrationError(
        'attempt_does_not_match_set',
        `the attempt token (migration ${attempt.migrationId}, ordinal ${attempt.ordinal}) does not describe a body of this set`,
      );
    }
    const target: SchemaMigrationTarget = {
      database_identity: attempt.databaseIdentity,
    };
    const raw = await this.queryStatusRawFor(attempt.db, target);
    // Journal content is only trusted when the server answered for the
    // requested identity; a mismatched echo is the typed identity error,
    // never evidence.
    if (raw.target.database_identity !== target.database_identity) {
      throw mapMigrationServerError(
        raw.error === undefined ? 'schema_migration_identity_mismatch' : raw.error,
        "the status target does not match the attempt's target identity",
      );
    }
    // Classification order: the attempt's OWN unresolved head first, then
    // the exact prefix-entry proof, then overtake/others. A matching
    // flight is THROWN as the typed refusal classified by the flight's
    // OWN status (shared `in_flight_error` split, attempt token attached)
    // — never a returned union arm: the caller must reconcile (matching
    // resume / operator evidence), not pattern-match a verdict.
    const flight = raw.in_flight;
    if (
      flight !== undefined &&
      flight.migration_id === attempt.migrationId &&
      asBigInt(flight.ordinal) === attemptOrdinal
    ) {
      throw inFlightError(flight, attempt);
    }
    // The attempt's exact entry anywhere in the VALID REPORTED prefix
    // (remote-only verification — the local set may legitimately end
    // before a later head) proves it applied, even when the head has
    // since overtaken it.
    const attemptInPrefix =
      verifyReportedPrefix(raw).kind === 'verified' &&
      (raw.applied ?? []).some(
        (applied) =>
          asBigInt(applied.ordinal) === attemptOrdinal &&
          applied.id === attempt.migrationId &&
          sameChecksum(applied.checksum, attemptChecksum),
      );
    // A server-reported in-doubt error from a LATER migration does not
    // shadow a proven applied attempt; any other error maps typed.
    if (raw.error !== undefined) {
      const shadowed =
        raw.error === 'schema_migration_in_doubt' && attemptInPrefix;
      if (!shadowed) {
        if (
          raw.error === 'schema_migration_in_doubt' &&
          raw.in_flight !== undefined
        ) {
          // The real server reports the in_doubt CODE for a merely
          // RUNNING other flight too: classify through that flight's OWN
          // status (not the attempt's, and no applied-attempt proof),
          // never through the blanket code.
          throw inFlightError(raw.in_flight);
        }
        throw mapMigrationServerError(
          raw.error,
          'the server reported a journal error; the attempt cannot be resolved',
        );
      }
    }
    if (attemptInPrefix) {
      return { kind: 'applied', migrationId: attempt.migrationId, ordinal: attemptOrdinal };
    }
    const journalOrdinal = asBigInt(raw.last_applied_ordinal);
    if (raw.in_flight !== undefined) {
      // A DIFFERENT migration's unresolved event without any reported
      // error: kept as the established RETURNED verdict (Rust parity,
      // `resolve.rs::UnknownApplyResolution::UnmatchedUnresolved`) — a
      // refusal here would strand callers that pattern-match it; the arm
      // stays reachable and documented.
      return { kind: 'unmatched_unresolved', journalOrdinal };
    }
    // Overtaken (or the same ordinal holds a different body, or the
    // prefix cannot prove the attempt): NOT proven — no false success.
    if (journalOrdinal >= attemptOrdinal) {
      return { kind: 'evidence_unavailable', journalOrdinal };
    }
    // Verified absence requires ALL of: proven coverage against the local
    // set, no unresolved event, the head being exactly the attempt's
    // predecessor, and the attempt being the next local ordinal. A
    // nonempty journal that reports fence generation 0 never counts as
    // ready. Drift, a shorter set, or any head/next mismatch is
    // evidence-unavailable; nothing is resent automatically.
    const coverage = verifyPrefix(raw, this.set);
    const headProven = coverage.kind === 'empty' || coverage.kind === 'verified';
    const fenceUsable = journalOrdinal === 0n || asBigInt(raw.fence_generation) !== 0n;
    const predecessorMatches = journalOrdinal === (attemptOrdinal > 0n ? attemptOrdinal - 1n : 0n);
    const nextIsAttempt =
      this.atNextOrdinal(journalOrdinal)?.ordinal === attemptOrdinal;
    if (headProven && fenceUsable && predecessorMatches && nextIsAttempt) {
      return { kind: 'ready_to_apply' };
    }
    return { kind: 'evidence_unavailable', journalOrdinal };
  }

  // ── Internals ─────────────────────────────────────────────────────

  /** One read-only status send for THIS target, decoded. */
  private queryStatusRaw(db: string): Promise<SchemaMigrationStatusResult> {
    return this.queryStatusRawFor(db, this.target);
  }

  /** One read-only status send for an explicit target, decoded against
   *  that target (a mismatched echo is only tolerated on the stable
   *  identity-mismatch error, where the record's target is the SERVING
   *  identity). A definitive server rejection is typed through
   *  `mapMigrationServerError(code, detail)`; read-only calls have no
   *  uncertain outcome, so everything else propagates unchanged. */
  private async queryStatusRawFor(
    db: string,
    target: SchemaMigrationTarget,
  ): Promise<SchemaMigrationStatusResult> {
    const record = await sendSingleRead(
      this.client,
      db,
      STATUS_ALIAS,
      applicationSchemaStatus(target),
    );
    return decodeMigrationStatus(record, target);
  }

  /** The local entry at `head + 1`, if the set extends that far. */
  private atNextOrdinal(headOrdinal: bigint): MigrationSetEntry | undefined {
    return setEntryAt(this.set, headOrdinal + 1n);
  }

  /** Whether the plan reply echoes the promised next migration exactly. */
  private planEchoesNext(
    plan: SchemaMigrationPlanResult,
    next: MigrationSetEntry,
    expectedPredecessor: SchemaMigrationPredecessor,
  ): boolean {
    const metadata = plan.migration;
    const metadataMatches =
      metadata.id === next.body.metadata.id &&
      asBigInt(metadata.ordinal) === next.ordinal &&
      metadata.display_name === next.body.metadata.display_name &&
      asBigInt(metadata.canonical_version) ===
        BigInt(next.body.metadata.canonical_version);
    const checksumMatches = sameChecksum(plan.checksum, next.checksum);
    const predecessor = plan.predecessor;
    const predecessorMatches =
      predecessor.migration_id === expectedPredecessor.migration_id &&
      asBigInt(predecessor.ordinal) === asBigInt(expectedPredecessor.ordinal) &&
      sameChecksum(
        predecessor.checksum,
        Uint8Array.from(expectedPredecessor.checksum),
      );
    return metadataMatches && checksumMatches && predecessorMatches;
  }
}
