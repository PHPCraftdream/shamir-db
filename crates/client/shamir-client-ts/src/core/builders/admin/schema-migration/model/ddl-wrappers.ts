/**
 * Migration-specific DDL wrapper compositions — one wrapper for each of the
 * 19 admitted `SchemaMigrationOp` kinds, composing the EXISTING builders in
 * `builders/ddl/` (`ddl-schema.ts`, `ddl-index.ts`, `ddl-func.ts`).
 *
 * Design rules (brief §What 1, "Do not"):
 *   - The public migration API carries NO auth and NO signer: wrappers that
 *     go through HMAC-gated builders (`dropTable`, `dropIndex`,
 *     `dropFunction`, `dropValidator`, gated `createFunction` /
 *     `createValidator`) call them privately with an inert signer producing
 *     a throwaway tag, then STRIP the `hmac` key from the result. The old
 *     builders are not changed; the migration payload simply never carries
 *     a per-op HMAC (the body is authenticated by the journal checksum).
 *   - `migrationDdl` is the camelCase wrapper namespace; each member returns
 *     a ready `{ operation, payload }` discriminator object.
 *   - Table-schema payloads accept {@link MigrationFieldRuleDto} whose
 *     `min` / `max` widen to {@link MigrationNumBound} and whose `one_of` /
 *     `default` accept {@link MigrationLiteral} (`MigrationFloat` instances
 *     are carried VERBATIM into the payload so the canonical encoder can
 *     emit the bit-exact f64 tag; the conversion to the plain wire
 *     `NumDto` / JSON value happens at submission encoding, not here).
 *   - HMAC-stripping is STRUCTURAL: `stripHmac` returns `Omit<T, 'hmac'>`,
 *     so the migration payload types (`Omit<…, 'hmac'>` variants in
 *     `migration-body.ts`) match exactly and `canonicalBatchOp` (which
 *     already accepts an optional `hmac`) needs no change.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  SchemaMigrationOp,
  MigrationFieldRuleDto,
  MigrationNumBound,
  MigrationLiteral,
  MigrationCreateTablePayload,
  MigrationSetTableSchemaPayload,
  MigrationAddSchemaRulePayload,
  MigrationDropTablePayload,
  MigrationDropIndexPayload,
  MigrationCreateFunctionPayload,
  MigrationDropFunctionPayload,
  MigrationCreateValidatorPayload,
  MigrationDropValidatorPayload,
} from '../../../../types/schema/migration-body.js';
import type {
  Retention,
  FieldRuleDto,
  FunctionLang,
  FunctionOptimize,
  WriteOpKind,
} from '../../../../types/schema/ddl.js';
import type { MigrationFloat } from './float.js';
import { MigrationError } from '../client/error.js';
import { markBuilderMigrationOp } from './operation-provenance.js';
import { validateMigrationIndex, validateFunctionalIndexOptions } from './validation.js';
import {
  createTable,
  dropTable,
  renameTable,
  setTableSchema,
  addSchemaRule,
  removeSchemaRule,
} from '../../../ddl/ddl-schema.js';
import { createIndex, dropIndex, renameIndex } from '../../../ddl/ddl-index.js';
import {
  createFunction,
  dropFunction,
  renameFunction,
  checkFunction,
  alterFunction,
  createValidator,
  dropValidator,
  renameValidator,
  bindValidator,
  unbindValidator,
} from '../../../ddl/ddl-func.js';

export type { MigrationNumBound, MigrationFloat };

// ── Inert-signer plumbing (private) ─────────────────────────────────

/**
 * Inert HMAC signer: the gated builders require a signer object and some of
 * them throw without an `hmac` option, so the migration wrappers satisfy
 * that shape locally with a fixed dummy tag which is then stripped. The
 * dummy is never sent — `stripHmac` removes the key (by destructuring, NOT
 * by mutation) before returning, so the result is `Omit<T, 'hmac'>`: the
 * exact migration payload type, and `hmac` can never reappear on the wire.
 */
const INERT_SIGNER = { hmacTagHex: (_canonical: Uint8Array): string => '0'.repeat(64) };

/**
 * Remove the `hmac` key the gated builder attached. Destructuring (not
 * `delete`) so the input object is never mutated and the return type drops
 * the key structurally.
 */
function stripHmac<T extends { hmac?: string }>(op: T): Omit<T, 'hmac'> {
  const { hmac: _inertTag, ...rest } = op;
  return rest;
}

/**
 * Widen a migration field rule to the plain `FieldRuleDto` shape for the
 * underlying builders. `MigrationFloat` instances pass through untouched:
 * the wrapper payload keeps the tagged bound (the structural type is
 * `MigrationFieldRuleDto`; the plain cast is safe because the runtime shape
 * is identical apart from the bound values, which the canonical encoder
 * resolves by inspecting for `MigrationFloat`).
 */
function toPlainRule(rule: MigrationFieldRuleDto): FieldRuleDto {
  // Vetted bridge cast: the underlying builder stores the rule opaquely and
  // the canonical encoder resolves the admitted migration-only literal forms
  // (`bigint` / `MigrationFloat` / `Uint8Array`) by runtime inspection, so
  // the plain runtime shape is identical.
  return rule as unknown as FieldRuleDto;
}

function toPlainRules(rules: MigrationFieldRuleDto[]): FieldRuleDto[] {
  return rules.map(toPlainRule);
}

/**
 * Narrow a plain `FieldRuleDto` to the migration-wide {@link MigrationFieldRuleDto}.
 * Vetted: every plain value is a legal migration literal, so the cast only
 * ever makes the (never-populated) wider arms `bigint` / `MigrationFloat` /
 * `Uint8Array` available; no runtime conversion is needed.
 */
function toMigrationRule(rule: FieldRuleDto): MigrationFieldRuleDto {
  return rule as unknown as MigrationFieldRuleDto;
}

// ── Wrapper options (mirroring the underlying builders) ─────────────

export interface MigrationCreateTableOptions {
  repo?: string;
  if_not_exists?: boolean;
  retention?: Retention;
  schema?: MigrationFieldRuleDto[];
}

export interface MigrationSetTableSchemaOptions {
  repo?: string;
  expectedVersion?: number | bigint;
}

/**
 * `create_index` options at the migration boundary. Same as the plain
 * `createIndex` builder options; `functional_args` remain ACCEPTED here so
 * the typed refusal is observable, but ANY supplied `functional_args` —
 * including `[]` — yields a typed `MigrationError` (the server never reads
 * them, so they must not enter the canonical checksum).
 */
export interface MigrationCreateIndexOptions {
  unique?: boolean;
  sorted?: boolean;
  repo?: string;
  index_type?: string;
  fts_tokenizer?: string;
  fts_language?: string;
  functional_op?: string;
  functional_args?: MigrationLiteral[];
  vector_dim?: number;
  vector_metric?: string;
  /** V5.2 #411 — `"sq8"` enables SQ8 scalar quantization. Opt-in. */
  vector_quantization?: string;
  include?: string[][];
  if_not_exists?: boolean;
}

export interface MigrationCreateFunctionOptions {
  source?: string;
  wasm?: string;
  lang?: FunctionLang;
  optimize?: FunctionOptimize;
  replace?: boolean;
  visibility?: string;
  security?: string;
  secret_grants?: string[];
  net_grants?: string[];
}

export interface MigrationCreateValidatorOptions {
  source?: string;
  wasm?: string;
  replace?: boolean;
}

// ── The wrapper namespace ───────────────────────────────────────────

/**
 * CamelCase wrapper namespace covering EXACTLY the 19 admitted migration
 * operation kinds. Each member returns a `{ operation, payload }` object;
 * no read/introspection/list/write operations are admitted.
 */
export const migrationDdl = {
  // ── table ───────────────────────────────────────────────────────

  /** `create_table` — composed with the existing `createTable` builder. */
  createTable(name: string, opts?: MigrationCreateTableOptions): SchemaMigrationOp {
    const { schema, ...rest } = opts ?? {};
    // Compose WITHOUT the builder `schema` slot, then re-attach the migration
    // rules directly, so an EXPLICIT empty `schema: []` survives on the wire
    // (the builder itself drops empty arrays) and the migration-wide rule
    // type needs no cast.
    const { schema: _builderSchema, ...base } = createTable(name, rest);
    const payload: MigrationCreateTablePayload = { ...base };
    if (schema !== undefined) payload.schema = schema;
    return markBuilderMigrationOp({ operation: 'create_table', payload });
  },

  /** `drop_table` — HMAC-stripped (no auth in the migration API). */
  dropTable(
    table: string,
    opts?: { repo?: string; if_exists?: boolean; cascade?: boolean },
  ): SchemaMigrationOp {
    const payload = dropTableOpFor(table, opts);
    return markBuilderMigrationOp({ operation: 'drop_table', payload });
  },

  /** `rename_table` — composed with the existing `renameTable` builder. */
  renameTable(from: string, to: string, opts?: { repo?: string }): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'rename_table', payload: renameTable(from, to, opts) });
  },

  // ── index ───────────────────────────────────────────────────────

  /** `create_index` — composed with the existing `createIndex` builder; migration-wide validation runs first. */
  createIndex(
    name: string,
    table: string,
    fields: string[][],
    opts?: MigrationCreateIndexOptions,
  ): SchemaMigrationOp {
    const { functional_args, ...rest } = opts ?? {};
    // Validate before the plain builder so ignored arguments are refused
    // rather than dropped.
    validateFunctionalIndexOptions(rest.index_type, rest.functional_op, functional_args);
    let base;
    try {
      base = createIndex(name, table, fields, rest);
    } catch (error) {
      throw new MigrationError(
        'invalid_set',
        `create_index: ${(error as Error).message}`,
      );
    }
    // The guards above prove `functional_args` are absent, so dropping the
    // builder's plain slot structurally yields exactly the migration payload
    // type — no cast.
    const { functional_args: _plainArgs, ...payload } = base;
    validateMigrationIndex(payload);
    return markBuilderMigrationOp({ operation: 'create_index', payload });
  },

  /** `drop_index` — HMAC-stripped (no auth in the migration API). */
  dropIndex(
    table: string,
    index: string,
    opts?: { repo?: string; if_exists?: boolean },
  ): SchemaMigrationOp {
    const repo = opts?.repo ?? 'main';
    const payload: MigrationDropIndexPayload = stripHmac(
      dropIndex(INERT_SIGNER, '', repo, table, index, opts),
    );
    return markBuilderMigrationOp({ operation: 'drop_index', payload });
  },

  /** `rename_index` — composed with the existing `renameIndex` builder. */
  renameIndex(
    table: string,
    from: string,
    to: string,
    opts?: { repo?: string; if_exists?: boolean },
  ): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'rename_index', payload: renameIndex(table, from, to, opts) });
  },

  // ── schema ──────────────────────────────────────────────────────

  /** `set_table_schema` — schema rules accept migration bounds. */
  setTableSchema(
    table: string,
    schema: MigrationFieldRuleDto[],
    opts?: MigrationSetTableSchemaOptions,
  ): SchemaMigrationOp {
    const built = setTableSchema(table, toPlainRules(schema), opts);
    // Re-attach the migration rules directly (dropping the plain slot) — no
    // cast needed; the plain pass-through is the vetted bridge in
    // `toPlainRules` (the builder stores rules opaquely).
    const { schema: _plainRules, ...base } = built;
    const payload: MigrationSetTableSchemaPayload = { ...base, schema };
    return markBuilderMigrationOp({ operation: 'set_table_schema', payload });
  },

  /** `add_schema_rule` — the rule accepts migration bounds. */
  addSchemaRule(
    table: string,
    rule: MigrationFieldRuleDto,
    opts?: { repo?: string },
  ): SchemaMigrationOp {
    const built = addSchemaRule(table, toPlainRule(rule), opts);
    // Re-attach the migration rule directly (dropping the plain slot) — no
    // cast needed; the plain pass-through is the vetted bridge in
    // `toPlainRule` (the builder stores the rule opaquely).
    const { rule: _plainRule, ...base } = built;
    const payload: MigrationAddSchemaRulePayload = { ...base, rule };
    return markBuilderMigrationOp({ operation: 'add_schema_rule', payload });
  },

  /**
   * Compose a migration field rule from a plain `FieldRuleDto` plus explicit
   * migration bounds (`min` / `max` widen to `MigrationNumBound`, so
   * `MigrationFloat` instances can be carried verbatim). Composed from the
   * DTO fields — no raw query assembly here.
   */
  withBounds(
    rule: FieldRuleDto,
    bounds: { min?: MigrationNumBound; max?: MigrationNumBound },
  ): MigrationFieldRuleDto {
    const base = toMigrationRule(rule);
    return {
      ...base,
      ...(bounds.min !== undefined ? { min: bounds.min } : {}),
      ...(bounds.max !== undefined ? { max: bounds.max } : {}),
    };
  },

  /** `remove_schema_rule` — composed with the existing builder. */
  removeSchemaRule(
    table: string,
    path: string[],
    opts?: { repo?: string },
  ): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'remove_schema_rule', payload: removeSchemaRule(table, path, opts) });
  },

  // ── function ────────────────────────────────────────────────────

  /**
   * `create_function` — HMAC-stripped. The gated builder throws on
   * `replace`/`definer`/grants without an `hmac`; the wrapper supplies a
   * dummy tag privately and strips it, so the migration body never carries
   * a per-op HMAC.
   */
  createFunction(name: string, opts?: MigrationCreateFunctionOptions): SchemaMigrationOp {
    const payload: MigrationCreateFunctionPayload = stripHmac(
      createFunction(name, { ...opts, hmac: INERT_SIGNER.hmacTagHex(new Uint8Array(0)) }),
    );
    return markBuilderMigrationOp({ operation: 'create_function', payload });
  },

  /** `drop_function` — HMAC-stripped. */
  dropFunction(name: string, opts?: { if_exists?: boolean }): SchemaMigrationOp {
    const payload: MigrationDropFunctionPayload = stripHmac(
      dropFunction(name, { if_exists: opts?.if_exists, hmac: INERT_SIGNER.hmacTagHex(new Uint8Array(0)) }),
    );
    return markBuilderMigrationOp({ operation: 'drop_function', payload });
  },

  /** `rename_function` — composed with the existing builder. */
  renameFunction(from: string, to: string): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'rename_function', payload: renameFunction(from, to) });
  },

  /** `check_function` — composed with the existing builder. */
  checkFunction(opts: Parameters<typeof checkFunction>[0]): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'check_function', payload: checkFunction(opts) });
  },

  /** `alter_function` — composed with the existing builder. */
  alterFunction(name: string, optimize: FunctionOptimize): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'alter_function', payload: alterFunction(name, optimize) });
  },

  // ── validator ───────────────────────────────────────────────────

  /** `create_validator` — HMAC-stripped (see `createFunction`). */
  createValidator(name: string, opts?: MigrationCreateValidatorOptions): SchemaMigrationOp {
    const payload: MigrationCreateValidatorPayload = stripHmac(
      createValidator(name, { ...opts, hmac: INERT_SIGNER.hmacTagHex(new Uint8Array(0)) }),
    );
    return markBuilderMigrationOp({ operation: 'create_validator', payload });
  },

  /** `drop_validator` — HMAC-stripped. */
  dropValidator(name: string, opts?: { if_exists?: boolean }): SchemaMigrationOp {
    const payload: MigrationDropValidatorPayload = stripHmac(
      dropValidator(name, { if_exists: opts?.if_exists, hmac: INERT_SIGNER.hmacTagHex(new Uint8Array(0)) }),
    );
    return markBuilderMigrationOp({ operation: 'drop_validator', payload });
  },

  /** `rename_validator` — composed with the existing builder. */
  renameValidator(from: string, to: string): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'rename_validator', payload: renameValidator(from, to) });
  },

  /** `bind_validator` — composed with the existing builder. */
  bindValidator(
    name: string,
    table: string,
    ops: WriteOpKind[],
    priority: number,
    target: { db: string; repo?: string },
  ): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'bind_validator', payload: bindValidator(name, table, ops, priority, target) });
  },

  /** `unbind_validator` — composed with the existing builder. */
  unbindValidator(
    name: string,
    target: { db: string; repo?: string; table: string },
  ): SchemaMigrationOp {
    return markBuilderMigrationOp({ operation: 'unbind_validator', payload: unbindValidator(name, target) });
  },
} as const;

/** The 19 admitted migration operation kinds, in the Rust model's order. */
export const MIGRATION_OPERATION_KINDS: readonly SchemaMigrationOp['operation'][] = [
  'create_table',
  'drop_table',
  'rename_table',
  'create_index',
  'drop_index',
  'rename_index',
  'set_table_schema',
  'add_schema_rule',
  'remove_schema_rule',
  'create_function',
  'drop_function',
  'rename_function',
  'check_function',
  'alter_function',
  'create_validator',
  'drop_validator',
  'rename_validator',
  'bind_validator',
  'unbind_validator',
] as const;

function dropTableOpFor(
  table: string,
  opts?: { repo?: string; if_exists?: boolean; cascade?: boolean },
): MigrationDropTablePayload {
  return stripHmac(dropTable(INERT_SIGNER, '', opts?.repo ?? 'main', table, opts));
}
