/**
 * Typed, ordered application migration SET (`ApplicationMigrationSet`) —
 * mirror of the Rust client's `receipts/schema_migration/set.rs` semantics:
 * a frozen ordered list of validated bodies with positive, contiguous,
 * 1-based ordinals and unique stable ids, with defensive entry access.
 *
 * Every body handed in is validated, deep-cloned (`cloneMigrationBody`) so
 * later caller mutation can never change the set, and its canonical SHA-256
 * (`migrationBodyChecksum`, byte-for-byte the Rust/server value) is computed
 * once over the snapshot and cached. Every entry leaving the set (`entries`,
 * `atOrdinal`) is a fresh copy — the caller can never alias the private
 * snapshots.
 *
 * All construction failures are typed `MigrationError('invalid_set', …)`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { SchemaMigrationBody } from '../../../../types/schema/migration-body.js';
import { MigrationError } from '../client/error.js';
import { cloneMigrationBody, validateMigrationBody } from '../model/index.js';
import { assertBuilderMigrationOp } from '../model/operation-provenance.js';
import { migrationBodyChecksum } from './body.js';

/** One validated migration at its one-based ordinal, with cached checksum. */
export type MigrationSetEntry = {
  /** One-based position in the set (contiguous by construction). */
  readonly ordinal: bigint;
  /** The typed body (cloned on every hand-out). */
  readonly body: SchemaMigrationBody;
  /** Canonical SHA-256 over the body snapshot, cached at construction. */
  readonly checksum: Uint8Array;
};

function failInvalidSet(message: string): never {
  throw new MigrationError('invalid_set', message);
}

/**
 * Normalize an ordinal to `bigint`, or `undefined` for a malformed value
 * (non-finite / unsafe / negative `number` is rejected, never guessed).
 */
function toBigint(ordinal: number | bigint): bigint | undefined {
  if (typeof ordinal === 'bigint') {
    return ordinal < 0n ? undefined : ordinal;
  }
  if (!Number.isSafeInteger(ordinal) || ordinal < 0) return undefined;
  return BigInt(ordinal);
}

/**
 * Ordered, frozen migration set with positive contiguous 1-based ordinals
 * and unique stable ids.
 */
export class ApplicationMigrationSet {
  private readonly entries_: readonly MigrationSetEntry[];

  /**
   * Build a set from an ordered list of typed bodies. Each body is
   * validated, its ordinal must continue the contiguous 1..n sequence and
   * its id must be unique — mirrors the Rust set validation. The body is
   * deep-cloned into a private snapshot BEFORE its canonical checksum is
   * computed, so a caller mutating the input afterwards cannot change
   * either the body or its cached checksum.
   */
  constructor(bodies: readonly SchemaMigrationBody[]) {
    const entries: MigrationSetEntry[] = [];
    const ids = new Set<string>();
    let previous = 0n;
    for (const body of bodies) {
      // Cheap shape guards BEFORE the provenance pass, so it never walks a
      // malformed body.
      if (body === null || typeof body !== 'object') {
        failInvalidSet('migration body must be a non-null object');
      }
      if (!Array.isArray(body.operations)) {
        failInvalidSet('migration body operations must be an array');
      }
      // Provenance FIRST, for ALL operations in one pass (WeakSet membership
      // — no payload read, no per-op interleaving with validation), before
      // any semantic walk touches an operation payload.
      for (const op of body.operations) {
        assertBuilderMigrationOp(op, 'migration set');
      }
      // Semantic validation (which now also runs the per-operation own
      // `__proto__` wire guard inside its indexed try/catch). It throws
      // typed `MigrationError('invalid_set', …)` only — no wrapper needed
      // here, and none that could trip over malformed metadata.
      validateMigrationBody(body);
      const ordinal = toBigint(body.metadata.ordinal);
      if (ordinal === undefined || ordinal <= 0n) {
        failInvalidSet(`migration ${String(body.metadata.id)}: ordinal must be a positive integer`);
      }
      if (ordinal !== previous + 1n) {
        failInvalidSet(
          `migration ordinals must be contiguous from 1: expected ${String(previous + 1n)}, got ${String(ordinal)}`,
        );
      }
      if (ids.has(body.metadata.id)) {
        failInvalidSet(`duplicate migration id: ${String(body.metadata.id)}`);
      }
      ids.add(body.metadata.id);
      const snapshot = cloneMigrationBody(body);
      entries.push({
        ordinal,
        body: snapshot,
        checksum: migrationBodyChecksum(snapshot),
      });
      previous = ordinal;
    }
    this.entries_ = Object.freeze(entries);
  }

  /** Number of migrations in the set (ordinals run 1..length). */
  get length(): number {
    return this.entries_.length;
  }

  /**
   * Defensive copies of every entry, in authored (ordinal) order: the body
   * and the checksum bytes are fresh on every call.
   */
  get entries(): MigrationSetEntry[] {
    return this.entries_.map((entry) => ({
      ordinal: entry.ordinal,
      body: cloneMigrationBody(entry.body),
      checksum: Uint8Array.from(entry.checksum),
    }));
  }

  /**
   * A defensive copy of the entry at one-based `ordinal`, or `undefined`
   * when it does not exist (or the ordinal is a malformed / unsafe /
   * negative number — rejected, never guessed).
   */
  atOrdinal(ordinal: number | bigint): MigrationSetEntry | undefined {
    const n = toBigint(ordinal);
    if (n === undefined || n < 1n || n > BigInt(this.entries_.length)) {
      return undefined;
    }
    const entry = this.entries_[Number(n) - 1];
    if (entry === undefined) return undefined;
    return {
      ordinal: entry.ordinal,
      body: cloneMigrationBody(entry.body),
      checksum: Uint8Array.from(entry.checksum),
    };
  }
}
