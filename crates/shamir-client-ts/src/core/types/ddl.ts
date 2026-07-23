/**
 * DDL (admin) operation wire types — type-only mirror of
 * `crates/shamir-query-types/src/admin/types.rs`.
 *
 * Pure type declarations; the constructor/builder code that assembles these
 * shapes lives in `../../builders/ddl.ts`.
 *
 * Serde notes encoded here (so the builder emits the exact wire shape):
 *   - fields with `skip_serializing_if` are OPTIONAL (`?`) here — the builder
 *     omits them at their default;
 *   - fields with only `#[serde(default)]` (no skip) are ALWAYS present on
 *     the wire (e.g. `unique`, `sorted`, `replace`, `repo` with `default_repo`);
 *   - `repo` with `#[serde(default = "default_repo")]` and NO skip is ALWAYS
 *     present (emits "main" when unset).
 *
 * PLATFORM-AGNOSTIC.
 */

import type { WireValue, WriteValue } from './write.js';

// ── Schema DTO types ────────────────────────────────────────────────

/**
 * Numeric bound for `min` / `max` constraints on the wire.
 * Mirrors `NumDto` in `schema_ops.rs` — `#[serde(untagged)]`.
 */
export type NumDto = number;

/**
 * Cross-field comparison descriptor (wire form).
 * Mirrors `CompareDto` in `schema_ops.rs`.
 */
export interface CompareDto {
  /** The other field path (flat string segments, NOT interned). */
  other: string[];
  /** Comparison operator: `"<"` / `"<="` / `"=="` / `"!="` / `">="` / `">"`. */
  op: string;
}

/**
 * Constraint fields carried alongside a `FieldRuleDto`.
 * All fields are optional; absent = unconstrained.
 * Mirrors `ConstraintsDto` in `schema_ops.rs` — all `skip_serializing_if`.
 */
/**
 * Foreign-key ON DELETE action (`FkAction`, `rename_all = "snake_case"`).
 * The serde default is `no_action` (backward-compat for persisted schemas);
 * the *builder* default for new FKs is `restrict`.
 */
export type FkAction = 'no_action' | 'restrict' | 'cascade' | 'set_null';

/**
 * Foreign-key reference descriptor (wire form).
 * Mirrors `ForeignKeyDto` in `schema_ops.rs`.
 * `on_delete` / `on_update` are
 * `#[serde(default, skip_serializing_if = "FkAction::is_no_action")]` — they
 * are omitted from the wire when `no_action`; the builder always sets
 * `on_delete` to a non-default value (default `restrict`), while `on_update`
 * defaults to `no_action` (additive — existing FK callers keep current
 * behavior).
 */
export interface ForeignKeyDto {
  /** The parent table name (flat, same repo). */
  ref_table: string;
  /** The field in the parent table that must contain the referenced value. */
  ref_field: string;
  /** ON DELETE action; omitted from wire when `no_action`. */
  on_delete?: FkAction;
  /** ON UPDATE action (Phase ②.2a — surface only); omitted when `no_action`. */
  on_update?: FkAction;
}

export interface ConstraintsDto {
  required?: boolean;
  nullable?: boolean;
  unsigned?: boolean;
  min?: NumDto;
  max?: NumDto;
  len?: number;
  max_len?: number;
  min_len?: number;
  one_of?: WireValue[];
  /**
   * ③.2c — default value (literal or expression) stamped on INSERT for an
   * absent field (extends Phase ②.4b literal-only to expression).
   *
   * Literal `WriteValue` forms (null/bool/number/string/array/object) route
   * through the fast `apply_defaults` path (②.4c behaviour is unchanged).
   * Expression `ComputedExpr` forms (`$fn` / `$ref` / etc.) route through
   * `apply_transforms` → `eval_write_value` → `builtin_scalars()` at
   * admission-time.  User-registered scalars are NOT available here (same
   * boundary as inline `$fn` write-field expressions).
   */
  default?: WriteValue;
  array_of?: string;
  /** Phase B — scalar-bridge: registered scalar name used as a predicate. */
  scalar?: string;
  /** Phase B — named format check: `"email"` / `"url"` / `"uuid"` / `"date"`. */
  format?: string;
  /** Phase B — cross-field comparison against another path. */
  compare?: CompareDto;
  /** Phase C2 — forward-only foreign-key reference. */
  foreign_key?: ForeignKeyDto;
  /** Phase C3 — unique constraint. */
  unique?: boolean;
  /**
   * ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
   * this field on **every** write (INSERT and UPDATE). The server clock is
   * always authoritative — any caller-supplied value is overwritten.
   *
   * Typical usage: `updated_at` field. Omitted from the wire when `false`
   * (mirrors `#[serde(default, skip_serializing_if = "is_false")]`).
   */
  auto_now?: boolean;
  /**
   * ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
   * this field on **INSERT** only, and only when the field is absent.
   * An explicitly-supplied value (including explicit `null`) is preserved.
   *
   * Typical usage: `created_at` field. Omitted from the wire when `false`.
   */
  auto_now_add?: boolean;
}

/**
 * A single field-rule as it travels over the wire (DDL payload).
 * Mirrors `FieldRuleDto` in `schema_ops.rs`.
 * `constraints` is `#[serde(flatten)]` — so on the wire the constraint
 * keys sit at the same level as `path` and `type`.
 */
export type FieldRuleDto = {
  path: string[];
  type: string;
} & ConstraintsDto;

// ── Schema DDL ops ──────────────────────────────────────────────────

/** Whole-replace a table's declarative schema. */
export interface SetTableSchemaOp {
  set_table_schema: string;
  repo: string;
  schema: FieldRuleDto[];
  /** Schema-revision CAS guard — same `u64` wire shape and precision
   * concern as `UpdateOp.expected_version` (see `types/write.ts`): a
   * `number | bigint` here lets a `bigint` schema version round-trip
   * without narrowing. */
  expected_version?: number | bigint;
}

/** Add (or replace by path) a single rule in a table's schema. */
export interface AddSchemaRuleOp {
  add_schema_rule: string;
  repo: string;
  rule: FieldRuleDto;
}

/** Remove a rule from a table's schema by path. */
export interface RemoveSchemaRuleOp {
  remove_schema_rule: string;
  repo: string;
  path: string[];
}

/** Read a table's declarative schema (introspection). */
export interface GetTableSchemaOp {
  get_table_schema: string;
  repo: string;
}

/** Describe a table — full introspection in one response. */
export interface DescribeTableOp {
  describe_table: string;
  repo: string;
}

// ── HMAC signer ─────────────────────────────────────────────────────

/**
 * Structural type for an HMAC-signing capability. The builder calls
 * `hmacTagHex(canonicalBytes)` and attaches the returned hex string as
 * the `hmac` field on destructive ops.
 */
export type HmacSigner = {
  hmacTagHex(canonical: Uint8Array): string;
};

// ── Sub-DTOs ────────────────────────────────────────────────────────

/**
 * Per-table history retention (admin/types.rs `Retention`).
 * All-optional (skip-if-none). `max_count: 0` → CurrentOnly.
 */
export interface Retention {
  max_age_secs?: number;
  max_count?: number;
  min_count?: number;
}

/**
 * Full per-table buffer config (admin/types.rs `BufferConfigDto`).
 * `ttl_ms` is skip-if-none; the rest are required.
 */
export interface BufferConfigDto {
  max_bytes: number;
  max_entries: number;
  ttl_ms?: number;
  flush_interval_ms: number;
  flush_batch_size: number;
}

/**
 * Partial-update payload for `alter_buffer_config`
 * (admin/types.rs `BufferConfigPatch`).
 * `ttl_ms` uses double-option semantics:
 *   * absent → no change,
 *   * null → clear TTL,
 *   * number → set TTL to that many ms.
 */
export interface BufferConfigPatch {
  max_bytes?: number;
  max_entries?: number;
  ttl_ms?: number | null;
  flush_interval_ms?: number;
  flush_batch_size?: number;
}

/**
 * Imperative history purge scope (admin/types.rs `PurgeScope`).
 * `rename_all = "snake_case"`, externally tagged.
 */
export type PurgeScope =
  | { older_than: { timestamp: number } }
  | { older_than_age: { age_secs: number } };

/**
 * Write-operation kind for validator binding (admin/types.rs `WriteOp`).
 * `rename_all = "lowercase"`: `"insert" | "update" | "upsert" | "delete"`.
 */
export type WriteOpKind = 'insert' | 'update' | 'upsert' | 'delete';

// ── Non-HMAC DDL ops ────────────────────────────────────────────────

export interface CreateDbOp {
  create_db: string;
  if_not_exists?: true;
}

export interface CreateRepoOp {
  create_repo: string;
  engine?: string;
  path?: string;
  tables?: string[];
  if_not_exists?: true;
}

export interface CreateTableOp {
  create_table: string;
  repo: string;
  if_not_exists?: true;
  retention?: Retention;
  schema?: FieldRuleDto[];
}

export interface CreateIndexOp {
  create_index: string;
  table: string;
  fields: string[][];
  unique: boolean;
  sorted: boolean;
  repo: string;
  index_type?: string;
  fts_tokenizer?: string;
  fts_language?: string;
  functional_op?: string;
  functional_args?: WireValue[];
  vector_dim?: number;
  vector_metric?: string;
  /**
   * V5.2 #411 — vector quantization method. `"sq8"` → SQ8 scalar quantization
   * (deferred fit at 256 vectors, then a u8-code HNSW graph with
   * dequant-rescore). Omitted / unrecognised → unquantized f32 HNSW path,
   * bit-for-bit identical to pre-#411.
   *
   * Mirrors Rust `CreateIndexOp::vector_quantization: Option<String>`
   * (`#[serde(default, skip_serializing_if = "Option::is_none")]`) — absent
   * from the wire when unset (back-compat).
   */
  vector_quantization?: string;
  include?: string[][];
  if_not_exists?: true;
}

export interface SetBufferConfigOp {
  set_buffer_config: string;
  repo: string;
  config: BufferConfigDto;
}

export interface GetBufferConfigOp {
  get_buffer_config: string;
  repo: string;
}

export interface AlterBufferConfigOp {
  alter_buffer_config: string;
  repo: string;
  patch: BufferConfigPatch;
}

export interface MigrationStatusOp {
  migration_status: string;
}

export interface CreateFunctionOp {
  create_function: string;
  source?: string;
  wasm?: string;
  replace: boolean;
  /**
   * `"public"` or `"private"` (absent → `"private"`, the default).
   * No HMAC required for this field.
   */
  visibility?: string;
  /**
   * `"invoker"` or `"definer"` (absent → `"invoker"`, the default).
   * Setting `"definer"` requires an `hmac` tag.
   */
  security?: string;
  /**
   * OS-seeded env-var secret grants. Non-empty requires an `hmac` tag
   * AND server-side `Manage(Root)` authorization.
   */
  secret_grants?: string[];
  /**
   * Hex-encoded HMAC-SHA256 tag, required IFF `security === "definer"`
   * or `secret_grants` is non-empty. Canonical input:
   * `canonicalCreateFunction(name, security, secret_grants)`.
   */
  hmac?: string;
}

export interface DropFunctionOp {
  drop_function: string;
  if_exists?: boolean;
}

export interface RenameFunctionOp {
  rename_function: string;
  to: string;
}

export interface CreateValidatorOp {
  create_validator: string;
  source?: string;
  wasm?: string;
  replace: boolean;
}

export interface DropValidatorOp {
  drop_validator: string;
  if_exists?: boolean;
}

export interface RenameValidatorOp {
  rename_validator: string;
  to: string;
}

export interface BindValidatorOp {
  bind_validator: string;
  db: string;
  repo: string;
  table: string;
  ops: WriteOpKind[];
  priority: number;
}

export interface UnbindValidatorOp {
  unbind_validator: string;
  db: string;
  repo: string;
  table: string;
}

export interface ListValidatorsOp {
  list_validators: string;
  db: string;
  repo: string;
}

export interface CreateFunctionFolderOp {
  create_function_folder: string[];
}

export interface RenameFunctionFolderOp {
  rename_function_folder: string[];
  to: string[];
}

/** Change a live table's history-retention policy (HMAC-gated). */
export interface SetRetentionOp {
  set_retention: string;
  repo: string;
  retention: Retention;
  hmac?: string;
}

/** Imperative history purge for a table (HMAC-gated) — irreversible
 * audit-trail loss. */
export interface PurgeHistoryOp {
  purge_history: string;
  repo: string;
  scope: PurgeScope;
  hmac?: string;
}

export interface ChangesSinceOp {
  changes_since: number;
  repo: string;
  limit?: number;
}

/**
 * Dump a repo's interner dictionary (id → name). Wire form:
 *   `{ interner_dump: "main" }` or `{ interner_dump: "main", since: 12 }`.
 * `interner_dump` has `#[serde(default = "default_repo")]` and NO skip —
 * it is ALWAYS present (emits "main" when unset). `since` is
 * `#[serde(default, skip_serializing_if = "Option::is_none")]` — omitted
 * when absent (delta-refresh cursor: only entries with id > `since`).
 */
export interface InternerDumpOp {
  interner_dump: string;
  since?: number;
}

/**
 * Register field NAMES in a repo's interner, returning the (name → id)
 * mapping. Idempotent. Wire form:
 *   `{ interner_touch: "main", names: ["age","name"] }`.
 * `interner_touch` has `#[serde(default = "default_repo")]` and NO skip —
 * it is ALWAYS present. `names` is always required (never skipped).
 */
export interface InternerTouchOp {
  interner_touch: string;
  names: string[];
}

// ── List ops (internally tagged: `#[serde(tag = "list")]`) ──────────

export type ListOp =
  | { list: 'databases' }
  | { list: 'repos' }
  | { list: 'tables'; repo: string }
  | { list: 'indexes'; table: string; repo: string }
  | { list: 'users' }
  | { list: 'functions'; folder?: string }
  | { list: 'validators' }
  | { list: 'function_folders'; parent?: string };

// ── HMAC-gated DDL ops ──────────────────────────────────────────────

export interface DropDbOp {
  drop_db: string;
  hmac: string;
  cascade?: true;
  if_exists?: boolean;
}

export interface DropRepoOp {
  drop_repo: string;
  hmac: string;
  cascade?: true;
  if_exists?: boolean;
}

export interface DropTableOp {
  drop_table: string;
  repo: string;
  hmac: string;
  if_exists?: boolean;
  cascade?: true;
}

export interface RenameTableOp {
  rename_table: string;
  to: string;
  repo?: string;
}

export interface RenameRepoOp {
  rename_repo: string;
  to: string;
}

export interface RenameDbOp {
  rename_db: string;
  to: string;
}

export interface RenameIndexOp {
  rename_index: string;
  to: string;
  table: string;
  repo?: string;
}

export interface DropIndexOp {
  drop_index: string;
  table: string;
  repo: string;
  hmac: string;
  unique?: true;
  if_exists?: boolean;
}

export interface StartMigrationOp {
  start_migration: string;
  repo: string;
  dst_repo: string;
  dst_engine: string;
  hmac: string;
  dst_path?: string;
}

export interface CommitMigrationOp {
  commit_migration: string;
  hmac: string;
}

export interface RollbackMigrationOp {
  rollback_migration: string;
  hmac: string;
}

// ── Union ───────────────────────────────────────────────────────────

/** Union of all DDL admin operations. */
export type DdlOp =
  | CreateDbOp
  | DropDbOp
  | CreateRepoOp
  | DropRepoOp
  | RenameRepoOp
  | RenameDbOp
  | CreateTableOp
  | DropTableOp
  | RenameTableOp
  | SetTableSchemaOp
  | AddSchemaRuleOp
  | RemoveSchemaRuleOp
  | GetTableSchemaOp
  | DescribeTableOp
  | CreateIndexOp
  | DropIndexOp
  | RenameIndexOp
  | SetBufferConfigOp
  | GetBufferConfigOp
  | AlterBufferConfigOp
  | MigrationStatusOp
  | StartMigrationOp
  | CommitMigrationOp
  | RollbackMigrationOp
  | CreateFunctionOp
  | DropFunctionOp
  | RenameFunctionOp
  | CreateValidatorOp
  | DropValidatorOp
  | RenameValidatorOp
  | BindValidatorOp
  | UnbindValidatorOp
  | ListValidatorsOp
  | CreateFunctionFolderOp
  | RenameFunctionFolderOp
  | SetRetentionOp
  | PurgeHistoryOp
  | ChangesSinceOp
  | InternerDumpOp
  | InternerTouchOp
  | ListOp;
