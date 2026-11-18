/**
 * Migration body builder — `schemaMigration()` composes one ordered, typed
 * `SchemaMigrationBody` and `cloneMigrationBody()` returns a defensive deep
 * copy of one (mirror of `SchemaMigrationBody` in Rust
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/model/body.rs`).
 *
 * The canonical v1 shape is normalized exactly as the Rust canonical
 * encoder admits: `metadata.canonical_version` is pinned to `1`,
 * `display_name` is the snake_case wire key, `function_dependencies` is
 * OMITTED when empty (`skip_serializing_if = "Vec::is_empty"`), and the
 * authored operation order is kept.
 *
 * Raw inputs are provenance-checked FIRST (all operations, one pass — no
 * payload read), then guarded against own `__proto__` keys, then copied
 * into the body shape, validated by `validateMigrationBody` (sibling
 * module), and only the validated copy is traversed by `cloneMigrationBody`.
 * All failures are typed `MigrationError('invalid_set', …)`; nothing here
 * throws generic `TypeError`/`RangeError` or forbids an empty display name
 * (Rust accepts).
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  SchemaMigrationBody,
  SchemaMigrationFunctionDependencies,
  SchemaMigrationOp,
} from '../../../../types/schema/migration-body.js';
import { MigrationFloat } from './float.js';
import { MigrationError } from '../client/error.js';
import {
  assertNoOwnProtoKeys,
  assertNoOwnProtoRootKey,
  validateMigrationBody,
} from './validation.js';
import {
  assertBuilderMigrationOp,
  preserveMigrationOpProvenance,
} from './operation-provenance.js';

/**
 * Build one ordered, typed migration body in the exact canonical format v1
 * shape. The raw inputs are copied into the body shape, the copy is
 * validated (`validateMigrationBody`, typed `MigrationError` on any rule
 * violation), and a defensive deep clone of the validated copy is returned,
 * so later caller-side mutation can never change the validated body.
 *
 * @param id - stable, content-independent migration id.
 * @param ordinal - position in the ordered set (Rust `u64` convention).
 * @param displayName - human-readable name (`display_name` on the wire).
 * @param operations - the ordered operation list (effect order is kept).
 * @param deps - optional function dependencies; empty/absent means the
 *   `function_dependencies` key is omitted from the wire shape.
 */
export function schemaMigration(
  id: string,
  ordinal: number | bigint,
  displayName: string,
  operations: readonly SchemaMigrationOp[],
  deps?: readonly SchemaMigrationFunctionDependencies[],
): SchemaMigrationBody {
  // Cheap shape guard: a non-array `operations` must fail typed, not throw
  // a raw TypeError from the spread below.
  if (!Array.isArray(operations)) {
    failInvalidSet(`migration ${id}: operations must be an array`);
  }
  // Provenance FIRST, for ALL operations in one pass (WeakSet membership —
  // no payload read, no per-op interleaving with validation): a hand-built
  // or spread op is rejected before any semantic walk touches its payload.
  for (const op of operations) {
    assertBuilderMigrationOp(op, `migration ${id}`);
  }
  // Own-`__proto__` wire guard on the RAW input — BEFORE the projection
  // below (`[...operations]`, `deps.map(...)`). For `operations` only the
  // ARRAY ROOT is checked: each element's payload is guarded recursively
  // inside `validateMigrationBody`'s indexed per-operation try/catch, and a
  // full raw walk here would be redundant non-indexed work. Function
  // dependencies are still guarded recursively (they are projected here by
  // `deps.map`). Note: it is the legacy marker-free encoder's ASSIGNMENT
  // that drops a `__proto__` own key — not object spread or `Object.keys`.
  // Any non-typed throw (e.g. a hostile accessor) is re-typed as invalid_set.
  try {
    assertNoOwnProtoRootKey(operations, `migration ${id}: operations`);
    if (deps !== undefined) {
      assertNoOwnProtoKeys(deps, `migration ${id}: function_dependencies`);
    }
  } catch (error) {
    if (error instanceof MigrationError) throw error;
    failInvalidSet(`migration ${id}: input failed the wire-shape guard`);
  }
  const body: SchemaMigrationBody = {
    metadata: {
      id,
      ordinal,
      display_name: displayName,
      canonical_version: 1,
    },
    operations: [...operations],
  };
  if (deps !== undefined && deps.length > 0) {
    body.function_dependencies = deps.map((d) => ({
      function: d.function,
      depends_on: [...d.depends_on],
    }));
  }

  // Semantic validation of the projected body (provenance and the raw-input
  // `__proto__` wire guard already ran above, before this projection).
  validateMigrationBody(body);
  return cloneMigrationBody(body);
}

function failInvalidSet(message: string): never {
  throw new MigrationError('invalid_set', message);
}

/**
 * Defensive deep clone of a validated migration body. Preserves, without
 * aliasing: `MigrationFloat` (re-wrapped, negative zero kept verbatim),
 * `Uint8Array` (copied via `new Uint8Array`, never `.slice()` — that can
 * alias a Buffer), `bigint` (immutable, passed through), arrays and plain
 * objects. Map/Set are inadmissible in an admitted body (the Rust literal
 * map is a plain string record); a non-plain class instance other than
 * `MigrationFloat` is rejected, not aliased. Cycle-guarded: a repeated
 * reference is a typed rejection (validation should already have caught it).
 */
export function cloneMigrationBody(body: SchemaMigrationBody): SchemaMigrationBody {
  const seen = new WeakMap<object, unknown>();

  function deep<T>(value: T): T {
    if (value === null || typeof value !== 'object') {
      return value;
    }
    const obj = value as unknown as object;
    if (seen.has(obj)) {
      failInvalidSet('migration body contains a reference cycle');
    }
    if (obj instanceof MigrationFloat) {
      return new MigrationFloat(obj.value) as unknown as T;
    }
    if (obj instanceof Uint8Array) {
      return new Uint8Array(obj) as unknown as T;
    }
    if (Array.isArray(obj)) {
      const out: unknown[] = [];
      seen.set(obj, out);
      for (const v of obj) {
        out.push(deep(v));
      }
      seen.delete(obj);
      return out as unknown as T;
    }
    const proto = Object.getPrototypeOf(obj);
    if (proto !== Object.prototype && proto !== null) {
      failInvalidSet(
        'migration body contains an unsupported class instance (only plain objects, arrays, MigrationFloat and Uint8Array are admitted)',
      );
    }
    const out: Record<string, unknown> = {};
    seen.set(obj, out);
    for (const key of Object.keys(obj)) {
      // defineProperty keeps `__proto__`-like keys as own properties.
      Object.defineProperty(out, key, {
        value: deep((obj as Record<string, unknown>)[key]),
        writable: true,
        enumerable: true,
        configurable: true,
      });
    }
    seen.delete(obj);
    return out as unknown as T;
  }

  const cloned = deep(body);

  // Preserve builder-op provenance across the copy: pair the source and
  // cloned operation entries and register the clone ONLY when the source is
  // a genuine builder-produced op (guarded inside the provenance module),
  // so hand-built ops stay unmarked and `entries`/`atOrdinal` copies remain
  // usable at the authoring boundaries.
  const sourceOps = body.operations;
  const clonedOps = cloned.operations;
  for (let i = 0; i < sourceOps.length; i += 1) {
    const src = sourceOps[i];
    const dst = clonedOps[i];
    if (src !== undefined && dst !== undefined && typeof dst === 'object' && dst !== null) {
      preserveMigrationOpProvenance(src, dst);
    }
  }

  return cloned;
}
