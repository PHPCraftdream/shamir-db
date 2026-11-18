/**
 * Module-private provenance for builder-produced migration operations.
 *
 * Each of the 19 `migrationDdl.*` wrappers marks its returned
 * `{ operation, payload }` object as BUILDER-PRODUCED via a module-private
 * `WeakSet`. The authoring boundaries (`schemaMigration` and the
 * `ApplicationMigrationSet` constructor) assert provenance FIRST — for ALL
 * operations in one pass, right after the cheap body/operation-array shape
 * checks and BEFORE any semantic read of any operation payload (no per-op
 * interleaving with validation) — so a hand-built or spread operation is
 * rejected with the provenance diagnostic without ever inspecting its
 * payload.
 *
 * INTERNAL ONLY: nothing here is re-exported from `model/index.ts` or the
 * package barrels, so no caller can mark an arbitrary object through the
 * public surface. `cloneMigrationBody` preserves membership (for genuine
 * builder ops only) via {@link preserveMigrationOpProvenance}, so defensive
 * copies handed out through `ApplicationMigrationSet.entries` / `atOrdinal`
 * keep usable provenance.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { SchemaMigrationOp } from '../../../../types/schema/migration-body.js';
import { MigrationError } from '../client/error.js';

/** Module-private registry of builder-produced migration op objects. */
const builderOps = new WeakSet<object>();

/**
 * Mark one freshly built `{ operation, payload }` object as
 * builder-produced. Internal — called only by the `migrationDdl.*` wrappers.
 */
export function markBuilderMigrationOp(op: SchemaMigrationOp): SchemaMigrationOp {
  builderOps.add(op);
  return op;
}

/**
 * Typed provenance guard: the op must have been produced by a `migrationDdl`
 * wrapper. Throws `MigrationError('invalid_set', …)` — never a generic
 * `TypeError`. Internal — called only by the authoring boundaries, FIRST
 * (after the cheap shape checks) and BEFORE any semantic validation reads
 * the operation payload.
 */
export function assertBuilderMigrationOp(op: SchemaMigrationOp, where: string): void {
  if (typeof op !== 'object' || op === null || !builderOps.has(op)) {
    throw new MigrationError(
      'invalid_set',
      `${where}: operation was not produced by migrationDdl — hand-built or spread operation objects are not admitted; compose operations with the migrationDdl.* wrappers`,
    );
  }
}

/**
 * Preserve provenance across a defensive deep copy: when `source` is a
 * genuine builder-produced op, register its clone. Guarded — anything not in
 * the registry (hand-built ops) stays unmarked. Internal — called only by
 * `cloneMigrationBody` after the generic deep copy, over the paired
 * source/clone operation entries.
 */
export function preserveMigrationOpProvenance(
  source: SchemaMigrationOp,
  clone: SchemaMigrationOp,
): void {
  if (typeof source === 'object' && source !== null && builderOps.has(source)) {
    builderOps.add(clone);
  }
}
