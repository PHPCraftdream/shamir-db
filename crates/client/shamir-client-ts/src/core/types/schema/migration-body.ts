/**
 * Application-schema migration body types — type-only mirror of
 * `crates/query/shamir-query-types/src/admin/types/schema_migration/model/`
 * (`operation.rs`, `metadata.rs`, `body.rs`).
 *
 * Pure type declarations; the builder code that assembles these shapes lives
 * in `../../builders/admin/schema-migration/model/`.
 *
 * Wire shape encoded here (mirrors the Rust serde attributes):
 *   - `SchemaMigrationOp` is
 *     `#[serde(tag = "operation", content = "payload", rename_all = "snake_case")]`
 *     — internally tagged `{ operation, payload }`; the `payload` is the SAME
 *     DTO shape the plain batch op uses (externally-tagged variant content,
 *     e.g. `{ create_table: name, repo, schema }`).
 *   - `SchemaMigrationBody.function_dependencies` is
 *     `#[serde(default, skip_serializing_if = "Vec::is_empty")]` — OPTIONAL
 *     (`?`) here; an empty list is omitted from the wire.
 *   - `metadata.ordinal` is a Rust `u64` — `number | bigint` per the
 *     package's existing u64 convention.
 *
 * Migration-specific constraint bounds: because the canonical format v1
 * distinguishes `NumDto::Int(i64)` from `NumDto::F64(f64)` bit-exactly
 * (including negative zero), migration field rules widen `min` / `max` to
 * {@link MigrationNumBound} (`bigint | number | MigrationFloat`). A plain
 * `number` bound is an integer (encoded as `Int`); a float bound must be
 * wrapped with `migrationFloat(value)` to be tagged as `F64`. See
 * `../../builders/admin/schema-migration/model/float.ts`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  CreateTableOp,
  DropTableOp,
  RenameTableOp,
  CreateIndexOp,
  DropIndexOp,
  RenameIndexOp,
  SetTableSchemaOp,
  AddSchemaRuleOp,
  RemoveSchemaRuleOp,
  CreateFunctionOp,
  DropFunctionOp,
  RenameFunctionOp,
  CheckFunctionOp,
  AlterFunctionOp,
  CreateValidatorOp,
  DropValidatorOp,
  RenameValidatorOp,
  BindValidatorOp,
  UnbindValidatorOp,
  FieldRuleDto,
  Retention,
} from './ddl.js';
import type { MigrationFloat } from '../../builders/admin/schema-migration/model/float.js';

/**
 * Numeric bound accepted by migration-specific field rules. `bigint` and
 * safe-integer `number` encode as `NumDto::Int`; `MigrationFloat` encodes
 * bit-exactly as `NumDto::F64` (canonical format v1 needs the explicit tag
 * because `2.0` and `2` are DIFFERENT canonical bytes).
 */
export type MigrationNumBound = bigint | number | MigrationFloat;

/**
 * A canonical-admitted literal value inside a migration body (mirrors the
 * `Value` / `FilterValue` grammar of canonical format v1 in
 * `canonical/{value,ddl}.rs`): JSON scalars, `bigint` (Rust i64/u64),
 * {@link MigrationFloat} (bit-exact f64 tag), `Uint8Array` (Rust bytes) and
 * recursive arrays / plain-string-keyed maps. NOT admitted: computed
 * expression forms (`$fn` / `$ref` / …) — validation rejects them.
 */
export type MigrationLiteral =
  | null
  | boolean
  | number
  | bigint
  | MigrationFloat
  | Uint8Array
  | string
  | MigrationLiteral[]
  | { [key: string]: MigrationLiteral };

/**
 * A field rule as accepted inside a migration body. Identical to the plain
 * `FieldRuleDto` except the Rust-admitted migration-only value types:
 *   - `min` / `max` widen to {@link MigrationNumBound} (the plain DTO keeps
 *     `NumDto = number` and is NOT widened — coordinated decision, see brief
 *     §Layout / "do not widen");
 *   - `len` / `max_len` / `min_len` are Rust `u64` — `number | bigint` per
 *     the package's existing u64 convention;
 *   - `one_of` / `default` accept {@link MigrationLiteral} (adds `bigint`,
 *     `MigrationFloat`, `Uint8Array` over the plain JSON `WireValue`).
 */
export type MigrationFieldRuleDto = Omit<
  FieldRuleDto,
  'min' | 'max' | 'len' | 'max_len' | 'min_len' | 'one_of' | 'default'
> & {
  min?: MigrationNumBound;
  max?: MigrationNumBound;
  /** Rust `u64`; `bigint` for values above 2^53. */
  len?: number | bigint;
  /** Rust `u64`; `bigint` for values above 2^53. */
  max_len?: number | bigint;
  /** Rust `u64`; `bigint` for values above 2^53. */
  min_len?: number | bigint;
  one_of?: MigrationLiteral[];
  default?: MigrationLiteral;
};

/** Migration payload for `create_table` — schema rules use migration bounds. */
export type MigrationCreateTablePayload = Omit<CreateTableOp, 'schema'> & {
  schema?: MigrationFieldRuleDto[];
};

/** Migration payload for `set_table_schema` — schema rules use migration bounds. */
export type MigrationSetTableSchemaPayload = Omit<SetTableSchemaOp, 'schema'> & {
  schema: MigrationFieldRuleDto[];
};

/** Migration payload for `add_schema_rule` — the rule uses migration bounds. */
export type MigrationAddSchemaRulePayload = Omit<AddSchemaRuleOp, 'rule'> & {
  rule: MigrationFieldRuleDto;
};

/**
 * Migration payload for `create_index` — the OPTIONAL `functional_args`
 * field accepts the canonical-admitted {@link MigrationLiteral} forms
 * (`bigint`, {@link MigrationFloat}, `Uint8Array` in addition to the plain
 * JSON `WireValue`), mirroring the Rust `Value` grammar of canonical format
 * v1. The field is accepted by the TYPE so the typed refusal is observable,
 * but the migration model REFUSES any supplied `functional_args` (including
 * `[]`): the server's functional-index backend never reads them, so they
 * must not enter the canonical checksum.
 */
export type MigrationCreateIndexPayload = Omit<CreateIndexOp, 'functional_args'> & {
  functional_args?: MigrationLiteral[];
};

// ── HMAC-free payload variants ─────────────────────────────────────

/**
 * The six HMAC-gated DDL DTOs reused as migration payloads. The plain DTOs
 * carry a REQUIRED `hmac` (the gated builders always attach one); the
 * migration API carries no per-op HMAC — each migration payload is the SAME
 * shape with the `hmac` key removed. Wrappers strip the inert builder tag
 * before returning.
 */
export type MigrationDropTablePayload = Omit<DropTableOp, 'hmac'>;
export type MigrationDropIndexPayload = Omit<DropIndexOp, 'hmac'>;
export type MigrationCreateFunctionPayload = Omit<CreateFunctionOp, 'hmac'>;
export type MigrationDropFunctionPayload = Omit<DropFunctionOp, 'hmac'>;
export type MigrationCreateValidatorPayload = Omit<CreateValidatorOp, 'hmac'>;
export type MigrationDropValidatorPayload = Omit<DropValidatorOp, 'hmac'>;

// ── The 19 admitted operations ──────────────────────────────────────

/**
 * Closed admitted schema-migration operation subset, discriminated by the
 * snake_case `operation` tag; `payload` is the corresponding DDL DTO
 * (mirrors `SchemaMigrationOp` in Rust `model/operation.rs`).
 *
 * Structural note: no `hmac` field appears on any migration payload —
 * each variant's payload type omits the `hmac` key (the six HMAC-gated
 * DTOs are reused as `Omit<…, 'hmac'>` variants, see
 * {@link MigrationDropTablePayload} and siblings). Authorization remains
 * a server-side concern; the client carries no auth or signer.
 */
export type SchemaMigrationOp =
  | { operation: 'create_table'; payload: MigrationCreateTablePayload }
  | { operation: 'drop_table'; payload: MigrationDropTablePayload }
  | { operation: 'rename_table'; payload: RenameTableOp }
  | { operation: 'create_index'; payload: MigrationCreateIndexPayload }
  | { operation: 'drop_index'; payload: MigrationDropIndexPayload }
  | { operation: 'rename_index'; payload: RenameIndexOp }
  | { operation: 'set_table_schema'; payload: MigrationSetTableSchemaPayload }
  | { operation: 'add_schema_rule'; payload: MigrationAddSchemaRulePayload }
  | { operation: 'remove_schema_rule'; payload: RemoveSchemaRuleOp }
  | { operation: 'create_function'; payload: MigrationCreateFunctionPayload }
  | { operation: 'drop_function'; payload: MigrationDropFunctionPayload }
  | { operation: 'rename_function'; payload: RenameFunctionOp }
  | { operation: 'check_function'; payload: CheckFunctionOp }
  | { operation: 'alter_function'; payload: AlterFunctionOp }
  | { operation: 'create_validator'; payload: MigrationCreateValidatorPayload }
  | { operation: 'drop_validator'; payload: MigrationDropValidatorPayload }
  | { operation: 'rename_validator'; payload: RenameValidatorOp }
  | { operation: 'bind_validator'; payload: BindValidatorOp }
  | { operation: 'unbind_validator'; payload: UnbindValidatorOp };

/**
 * Stable, content-independent identity and ordering for one migration
 * (mirrors `SchemaMigrationMetadata`; `deny_unknown_fields` — no extra keys).
 */
export interface SchemaMigrationMetadata {
  id: string;
  /** Rust `u64`; `bigint` for values above 2^53. */
  ordinal: number | bigint;
  display_name: string;
  /** Canonical format version; only `1` is defined. */
  canonical_version: 1;
}

/**
 * Author-declared dependency of one stored function on named functions,
 * considered during function-change planning
 * (mirrors `SchemaMigrationFunctionDependencies`).
 */
export interface SchemaMigrationFunctionDependencies {
  function: string;
  depends_on: string[];
}

/**
 * One ordered, typed migration body. Operation order is effect order
 * (mirrors `SchemaMigrationBody` in Rust `model/body.rs`).
 * `function_dependencies` is omitted from the wire when empty.
 */
export interface SchemaMigrationBody {
  metadata: SchemaMigrationMetadata;
  function_dependencies?: SchemaMigrationFunctionDependencies[];
  operations: SchemaMigrationOp[];
}

/**
 * Migration-set construction options accepted by `schemaMigration()` for
 * table schemas. Retention passes through unchanged: its numeric fields are
 * already Rust `u64` (`number | bigint`, see `Retention`), so no widening
 * exists for retention in canonical format v1.
 */
export type MigrationTableSchemaOptions = {
  repo?: string;
  if_not_exists?: boolean;
  retention?: Retention;
  schema?: MigrationFieldRuleDto[];
};
