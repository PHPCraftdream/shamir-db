/**
 * Whole-body canonical bytes and SHA-256 checksum for the schema-migration
 * canonical format v1 — TypeScript mirror of
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/canonical/body.rs`
 * (`canonical_body` / `canonical_body_checksum`).
 *
 * Body field order: `id:string`, `ordinal:u64`, `display_name:string`,
 * function dependencies, operations. Dependency rows are sorted by function
 * UTF-8 BYTES (never JS string order); each dependency-name list is sorted
 * likewise. Operations retain authored order and are encoded as
 * `u32 length || operation bytes`. The checksum is
 * `SHA-256(DOMAIN || canonical_version:u16be || canonical_body)` with
 * `DOMAIN = b"shamir-db schema migration\0"` — byte-for-byte what the Rust
 * client and the server compute.
 *
 * All failures (validation, unknown version, unencodable value) are typed
 * `MigrationError('canonical', …)`.
 *
 * PLATFORM-AGNOSTIC.
 */

import { sha256 } from '@noble/hashes/sha256';
import type { SchemaMigrationBody } from '../../../../types/schema/migration-body.js';
import { MigrationError } from '../client/error.js';
import { validateMigrationBody } from '../model/index.js';
import { Encoder, compareUtf8, concatBytes } from './encoder.js';
import { encodeMigrationOperation } from './operations.js';

const utf8 = new TextEncoder();
const DOMAIN = utf8.encode('shamir-db schema migration\0');

function fail(message: string): never {
  throw new MigrationError('canonical', message);
}

/**
 * Canonical bytes of one validated migration body (format v1). Validates
 * first — exactly like Rust `canonical_body` — then encodes.
 */
export function canonicalMigrationBody(body: SchemaMigrationBody): Uint8Array {
  if (body.metadata.canonical_version !== 1) {
    fail(
      `unknown schema migration canonical version ${String(body.metadata.canonical_version)}`,
    );
  }
  try {
    validateMigrationBody(body);
  } catch (error) {
    fail(`invalid schema migration body: ${(error as Error).message}`);
  }
  const e = new Encoder();
  try {
    e.str(body.metadata.id);
    e.u64(body.metadata.ordinal);
    e.str(body.metadata.display_name);
    const dependencies = [...(body.function_dependencies ?? [])].sort((a, b) =>
      compareUtf8(a.function, b.function),
    );
    e.count(dependencies.length);
    for (const dependency of dependencies) {
      e.str(dependency.function);
      const names = [...dependency.depends_on].sort(compareUtf8);
      e.count(names.length);
      for (const name of names) e.str(name);
    }
    e.count(body.operations.length);
    for (const operation of body.operations) {
      e.bytes(encodeMigrationOperation(operation));
    }
  } catch (error) {
    fail(`schema migration canonical encoding failed: ${(error as Error).message}`);
  }
  return e.finish();
}

/**
 * SHA-256 checksum over `DOMAIN || canonical_version(u16 BE) || canonical
 * body` — the value journaled by the server and compared on apply.
 */
export function migrationBodyChecksum(body: SchemaMigrationBody): Uint8Array {
  const canonical = canonicalMigrationBody(body);
  const view = new DataView(new ArrayBuffer(2));
  view.setUint16(0, body.metadata.canonical_version, false);
  return sha256(concatBytes([DOMAIN, new Uint8Array(view.buffer.slice(0)), canonical]));
}

/** Aliases matching the Rust names (`canonical_body` / `canonical_body_checksum`). */
export const canonicalBody = canonicalMigrationBody;
export const checksum = migrationBodyChecksum;
