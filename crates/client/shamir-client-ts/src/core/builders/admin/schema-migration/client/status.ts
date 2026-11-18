/**
 * Client-side applied-prefix comparison for the application-schema
 * migration `status` result — the TypeScript mirror of
 * `crates/client/shamir-client/src/receipts/schema_migration/status.rs`
 * (`verify_reported_prefix`, `verify_prefix`, `compare_with_set`).
 *
 * PURE helper: no transport, no decoding, no sending. The raw status
 * result arrives already validated for wire shape by the independent
 * `../decode.ts` decoder; this module only VERIFIES the reported applied
 * prefix and compares it with the local
 * `ApplicationMigrationSet` (from `../canonical/set.js`).
 *
 * Semantics (mirroring the Rust behavioural reference exactly):
 *   - Remote-only `verifyReportedPrefix`: the reported prefix counts as
 *     verified only when every ordinal is contiguous and positive from 1,
 *     no migration id repeats, the server's own checksum verdict is
 *     `valid` for a nonempty journal, the journal reports a nonzero
 *     durable fence generation, an EMPTY journal reports no head tuple at
 *     all, and the final entry matches the reported head tuple exactly.
 *     Every other outcome is `unverified` — or the concrete `gap` break.
 *   - `verifyPrefix` layers the LOCAL comparison onto the remote verdict:
 *     id AND canonical checksum of the local set must match the journal
 *     at EVERY applied ordinal (via `set.atOrdinal`).
 *   - `compareMigrationStatus` produces the typed client view, including
 *     the next local ordinal only when the coverage is provably
 *     empty/verified, the server reported no error, and the local set
 *     still extends past the journal head (`headOrdinal + 1` is a single
 *     lookup, not a scan). The raw head tuple and the server's
 *     checksum verdict are exposed on the view for diagnostics.
 *
 * All ordinals and fence generations are compared as `bigint` (u64 wire
 * convention: `number | bigint`); the returned view uses `bigint`
 * throughout. Checksums on the response side are `Uint8Array` (msgpack
 * binary). Field naming mirrors the Rust client view: snake_case for the
 * reported target/server error (wire-shaped) and `fence_generation` for
 * the journal fence; in-flight evidence uses camelCase.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  SchemaMigrationChecksumValidation,
  SchemaMigrationErrorCode,
  SchemaMigrationStatusResult,
  SchemaMigrationTarget,
} from '../../../../types/schema/migration-protocol.js';
import type { ApplicationMigrationSet } from '../canonical/set.js';

// ── Local scalar helpers ────────────────────────────────────────────

/** Normalize a u64 wire value (`number | bigint`) to `bigint`. */
function asBigInt(value: number | bigint): bigint {
  return typeof value === 'bigint' ? value : BigInt(value);
}

/** Byte-for-byte comparison of two 32-byte checksums. */
function sameChecksum(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

// ── Coverage ────────────────────────────────────────────────────────

/**
 * Explicit applied-prefix coverage of the local set against the journal
 * (`PrefixCoverage` in Rust, as a discriminated union on `kind`).
 */
export type PrefixCoverage =
  /** The journal reports no applied history. */
  | { kind: 'empty' }
  /**
   * The full applied prefix is PROVEN: contiguous ordinals from 1, no
   * duplicate ids, the final entry consistent with the reported head
   * tuple under a server-verified checksum, a nonzero fence for a
   * nonempty journal, and (in `verifyPrefix`) the id AND canonical
   * checksum of the local set matching at EVERY applied ordinal.
   */
  | { kind: 'verified'; ordinal: bigint }
  /**
   * DRIFT: the applied journal entry at the given ordinal differs from
   * the local body (id or canonical checksum).
   */
  | {
      kind: 'drift';
      ordinal: bigint;
      journalId: string;
      journalChecksum: Uint8Array;
      localId: string;
    }
  /**
   * The journal prefix extends beyond the local set: the compiled set is
   * SHORTER than the applied journal.
   */
  | { kind: 'journal_beyond_set'; ordinal: bigint }
  /**
   * The reported applied sequence has a GAP: the entries jump (expected
   * ordinal `expected`, actual `actual`). Nothing beyond the gap is
   * proven and apply is refused.
   */
  | { kind: 'gap'; expected: bigint; actual: bigint }
  /**
   * Nothing is verified: the server checksum of the head is not `valid`,
   * a nonempty journal reports no durable fence generation, the applied
   * array is missing or contradictory, a duplicate id appears, or the
   * final entry is inconsistent with the reported head tuple. Never
   * treated as verified and never paired with a next ordinal.
   */
  | { kind: 'unverified'; ordinal: bigint };

// ── Client view ─────────────────────────────────────────────────────

/** The unresolved journal head as reported by the server. */
export interface MigrationInFlight {
  migrationId: string;
  ordinal: bigint;
  /** Index of the pending operation inside the migration, when recorded. */
  operationIndex?: bigint;
  /** Fence generation the in-flight (or failed) event ran under. */
  fence_generation: bigint;
  /** Journal event kind string (`started`, `in_doubt`, `failed`, ...). */
  status: string;
  /** Recorded evidence of an in-doubt or failed event, when returned. */
  inDoubtReason?: string;
}

/** One applied journal entry as reported by the server. */
export interface MigrationAppliedEntry {
  id: string;
  ordinal: bigint;
  checksum: Uint8Array;
}

/**
 * Client view of one status query compared against the local set
 * (`MigrationStatus` in Rust).
 */
export interface MigrationStatus {
  /** Target identity the server answered for. On
   * `schema_migration_identity_mismatch` this echoes the SERVING
   * identity, not the requested one. */
  reported_target: SchemaMigrationTarget;
  /** Reported journal head id, when the journal is nonempty. */
  last_applied_id?: string;
  /** Reported journal head ordinal (`0` when empty). */
  last_applied_ordinal: bigint;
  /** Reported head checksum, when the journal is nonempty. */
  last_applied_checksum?: Uint8Array;
  /** The server's own checksum verdict for the head. */
  checksum_validation: SchemaMigrationChecksumValidation;
  /**
   * The smallest local ordinal not covered by the reported journal head —
   * absent when the journal already covers the whole set, or when nothing
   * is verified (drift, unverified checksum, malformed prefix,
   * beyond-set head or a server-reported error).
   */
  next_local_ordinal?: bigint;
  /** Typed server-side error carried inside the status record. */
  server_error?: SchemaMigrationErrorCode;
  /** Explicit applied-prefix coverage (see {@link PrefixCoverage}). */
  coverage: PrefixCoverage;
  /** Every applied migration as reported, in ordinal order. */
  applied: readonly MigrationAppliedEntry[];
  /** The unresolved journal head, when one exists. */
  in_flight?: MigrationInFlight;
  /**
   * Highest durable fence generation of the journal (`0` when empty,
   * the `expected_fence_generation` of the next fresh apply).
   */
  fence_generation: bigint;
}

// ── Remote-only verification ────────────────────────────────────────

/**
 * The REMOTE-ONLY verification of the reported applied prefix — shape,
 * contiguity, duplicate ids, head-tuple consistency and server checksum
 * verdict, with NO local set involved. `verifyPrefix` layers the local
 * comparison onto its `empty`/`verified` outcomes; `resolveUnknownApply`
 * can use it alone so a valid REPORTED prefix can confirm an attempt even
 * when the local set ends before a later head.
 */
export function verifyReportedPrefix(
  rawStatus: SchemaMigrationStatusResult,
): PrefixCoverage {
  const headOrdinal = asBigInt(rawStatus.last_applied_ordinal);
  const applied = rawStatus.applied ?? [];
  if (applied.length === 0) {
    // No full prefix on the wire: only a truly blank journal (ordinal 0
    // with NO head tuple) is a verified statement; a nonempty head
    // without a prefix proves nothing, and an empty journal that still
    // reports a head id/checksum is contradictory — fail closed either
    // way.
    if (
      headOrdinal === 0n &&
      rawStatus.last_applied_id === undefined &&
      rawStatus.last_applied_checksum === undefined
    ) {
      return { kind: 'empty' };
    }
    return { kind: 'unverified', ordinal: headOrdinal };
  }
  const head = applied[applied.length - 1]!;
  // Contiguous positive order from ordinal 1; any break is a concrete
  // GAP (e.g. [1, 3]: expected 2, actual 3), not a vague "unverified".
  for (let index = 0; index < applied.length; index++) {
    const expected = BigInt(index) + 1n;
    const actual = asBigInt(applied[index]!.ordinal);
    if (actual !== expected) {
      return { kind: 'gap', expected, actual };
    }
  }
  // No migration id may repeat within the reported prefix.
  const seen = new Set<string>();
  for (const entry of applied) {
    if (seen.has(entry.id)) {
      return { kind: 'unverified', ordinal: headOrdinal };
    }
    seen.add(entry.id);
  }
  // The server checksum verdict must be `valid` — an invalid or missing
  // verification NEVER counts as verified and never yields a next
  // ordinal.
  if (rawStatus.checksum_validation !== 'valid') {
    return { kind: 'unverified', ordinal: headOrdinal };
  }
  // A NONEMPTY journal must report a real durable fence generation: a 0
  // there is a contradictory shape (a missing field on an old server is
  // typed UnsupportedServer by the caller before this runs).
  if (headOrdinal !== 0n && asBigInt(rawStatus.fence_generation) === 0n) {
    return { kind: 'unverified', ordinal: headOrdinal };
  }
  // The final entry must be CONSISTENT with the reported head tuple.
  const headConsistent =
    BigInt(applied.length) === headOrdinal &&
    rawStatus.last_applied_id === head.id &&
    rawStatus.last_applied_checksum !== undefined &&
    sameChecksum(rawStatus.last_applied_checksum, head.checksum);
  if (!headConsistent) {
    return { kind: 'unverified', ordinal: headOrdinal };
  }
  return { kind: 'verified', ordinal: headOrdinal };
}

// ── Local comparison ────────────────────────────────────────────────

/**
 * The shared verification of the reported applied prefix against the
 * local set — the ONE comparison used by `status` and `applyNext`, so no
 * path can drift from another: the remote-only shape/head/checksum checks
 * first, then the local id AND canonical checksum for every applied
 * ordinal.
 */
export function verifyPrefix(
  rawStatus: SchemaMigrationStatusResult,
  set: ApplicationMigrationSet,
): PrefixCoverage {
  let remote = verifyReportedPrefix(rawStatus);
  // Only a contiguous, head-consistent, checksum-valid reported prefix
  // can be compared locally; every other outcome is final.
  if (remote.kind !== 'empty' && remote.kind !== 'verified') {
    return remote;
  }
  const headOrdinal = BigInt(rawStatus.last_applied_ordinal);
  // A server-reported error (e.g. `schema_migration_identity_mismatch`)
  // means the reported head does not describe THIS journal view — the
  // verified result is suppressed (fail closed).
  if (rawStatus.error !== undefined && remote.kind === 'verified') {
    remote = { kind: 'unverified', ordinal: headOrdinal };
  }
  const applied = rawStatus.applied ?? [];
  for (const entry of applied) {
    const ordinal = asBigInt(entry.ordinal);
    const local = set.atOrdinal(ordinal);
    if (local === undefined) {
      return { kind: 'journal_beyond_set', ordinal: headOrdinal };
    }
    if (
      local.body.metadata.id !== entry.id ||
      !sameChecksum(local.checksum, entry.checksum)
    ) {
      return {
        kind: 'drift',
        ordinal,
        journalId: entry.id,
        journalChecksum: entry.checksum,
        localId: local.body.metadata.id,
      };
    }
  }
  return remote;
}

/**
 * Compare a decoded status record with the local set, through the ONE
 * shared prefix verification. A server-reported error (e.g.
 * `schema_migration_identity_mismatch`) suppresses the local next-ordinal
 * computation: the reported head does not describe this set's journal
 * view. No `verified` coverage and no next ordinal is ever produced on an
 * unverified checksum or an identity mismatch.
 */
export function compareMigrationStatus(
  rawStatus: SchemaMigrationStatusResult,
  set: ApplicationMigrationSet,
): MigrationStatus {
  const coverage = verifyPrefix(rawStatus, set);
  const headOrdinal = asBigInt(rawStatus.last_applied_ordinal);
  let nextLocalOrdinal: bigint | undefined;
  if (
    rawStatus.error === undefined &&
    (coverage.kind === 'empty' || coverage.kind === 'verified')
  ) {
    // The smallest local ordinal not covered by the reported journal
    // head is `headOrdinal + 1` when the set still extends past it —
    // one lookup; a journal beyond the set is already typed
    // `journal_beyond_set` inside `verifyPrefix`.
    if (set.atOrdinal(headOrdinal + 1n) !== undefined) {
      nextLocalOrdinal = headOrdinal + 1n;
    }
  }
  const rawInFlight = rawStatus.in_flight;
  const inFlight = rawInFlight
    ? {
        migrationId: rawInFlight.migration_id,
        ordinal: asBigInt(rawInFlight.ordinal),
        ...(rawInFlight.operation_index !== undefined
          ? { operationIndex: asBigInt(rawInFlight.operation_index) }
          : {}),
        fence_generation: asBigInt(rawInFlight.fence_generation),
        status: rawInFlight.status,
        ...(rawInFlight.in_doubt_reason !== undefined
          ? { inDoubtReason: rawInFlight.in_doubt_reason }
          : {}),
      }
    : undefined;
  return {
    reported_target: rawStatus.target,
    ...(rawStatus.last_applied_id !== undefined
      ? { last_applied_id: rawStatus.last_applied_id }
      : {}),
    last_applied_ordinal: headOrdinal,
    ...(rawStatus.last_applied_checksum !== undefined
      ? { last_applied_checksum: rawStatus.last_applied_checksum }
      : {}),
    checksum_validation: rawStatus.checksum_validation,
    ...(nextLocalOrdinal !== undefined ? { next_local_ordinal: nextLocalOrdinal } : {}),
    ...(rawStatus.error !== undefined ? { server_error: rawStatus.error } : {}),
    coverage,
    applied: (rawStatus.applied ?? []).map((entry) => ({
      id: entry.id,
      ordinal: asBigInt(entry.ordinal),
      checksum: entry.checksum,
    })),
    ...(inFlight !== undefined ? { in_flight: inFlight } : {}),
    fence_generation: asBigInt(rawStatus.fence_generation),
  };
}
