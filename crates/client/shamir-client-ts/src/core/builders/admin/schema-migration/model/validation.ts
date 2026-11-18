/**
 * Typed pre-send validation of one schema-migration body.
 *
 * Mirrors the Rust model validation exactly:
 *   - `model/metadata.rs::SchemaMigrationMetadata::validate`
 *     (nonempty id, positive u64 ordinal, canonical_version == 1)
 *   - `model/body.rs::SchemaMigrationBody::validate`
 *     (nonempty + unique dependent function names, nonempty + unique
 *     `depends_on` names, per-operation validation; EMPTY `operations`
 *     list is allowed, exactly as in Rust)
 *   - `model/operation.rs::SchemaMigrationOp::validate`
 *     (only the 19 admitted kinds; functional-index builtin allowlist —
 *     exactly `lower | upper | trim | length`, the operations the server's
 *     functional-index backend builds directly — and refusal of ANY supplied
 *     `functional_args`; index type / vector metric / quantization families;
 *     exactly one of source/wasm for function/validator creation and check;
 *     16 MiB UTF-8 code-size bound; lang/optimize/security/visibility
 *     families)
 *   - `model/admission.rs`
 *     (schema rules: no registered scalar, no cross-field compare, no
 *     dynamic default, explicit null default disallowed, finite numeric
 *     bounds; query literals bounded at depth 64 / 65 536 nodes /
 *     16 MiB total including map keys)
 *
 * Everything here fails TYPED (`MigrationError('invalid_set', …)`) BEFORE
 * the body is encoded or sent — the client never puts an invalid set on
 * the wire.
 *
 * PLATFORM-AGNOSTIC.
 */

import type {
  SchemaMigrationBody,
  SchemaMigrationMetadata,
  SchemaMigrationOp,
  MigrationFieldRuleDto,
  MigrationNumBound,
  MigrationCreateIndexPayload,
} from '../../../../types/schema/migration-body.js';
import { isMigrationFloat } from './float.js';
import { MigrationError } from '../client/error.js';
import { createIndex } from '../../../ddl/ddl-index.js';

// ── Canonical bounds (mirror `model/admission.rs`) ──────────────────

const MAX_VALUE_DEPTH = 64;
const MAX_VALUE_NODES = 65_536;
const MAX_VALUE_BYTES = 16 * 1024 * 1024; // 16 MiB

/**
 * Functional-index builtin allowlist (`model/operation.rs`), narrowed to
 * exactly the operations the server's index backend builds directly
 * (`write_domain_index_admitted.rs`: the `"functional"` arm matches only
 * `lower | upper | trim | length`; anything else needs a trusted indexable
 * scalar the migration model cannot vouch for).
 */
const FUNCTIONAL_INDEX_OPS: ReadonlySet<string> = new Set([
  'lower',
  'upper',
  'trim',
  'length',
]);

const INDEX_TYPES: ReadonlySet<string> = new Set(['btree', 'fts', 'functional', 'vector']);
const VECTOR_METRICS: ReadonlySet<string> = new Set(['l2', 'cosine', 'dot']);
const OPTIMIZE_MODES: ReadonlySet<string> = new Set(['auto', 'now', 'never']);
const SECURITY_MODES: ReadonlySet<string> = new Set(['invoker', 'definer']);
const VISIBILITY_MODES: ReadonlySet<string> = new Set(['public', 'private']);

const MAX_U64 = 2n ** 64n - 1n;

/**
 * The MAX_I64 range for `bigint` literals (canonical format v1 encodes a
 * `bigint` as a signed Rust `i64`).
 */
const MAX_I64 = 2n ** 63n - 1n;
const MIN_I64 = -(2n ** 63n);

function fail(message: string): never {
  throw new MigrationError('invalid_set', message);
}

/** UTF-8 byte length (mirrors Rust `str::len`, no normalization). */
const utf8 = new TextEncoder();
function byteLength(value: string): number {
  return utf8.encode(value).length;
}

// ── Own-`__proto__`-key wire guard (migration inputs only) ──────────

/**
 * Root-only variant: reject an OWN `__proto__` key on this one container
 * (object or array, enumerable or not) without traversing its values.
 * Used where the nested values are guarded elsewhere (e.g. per-operation
 * inside the indexed validation loop) and a full recursive walk of a huge
 * raw array would be redundant. Same wire-loss diagnostic as the recursive
 * walker. Internal only.
 */
export function assertNoOwnProtoRootKey(value: unknown, what: string): void {
  if (value === null || typeof value !== 'object') return;
  if (Object.hasOwn(value, '__proto__')) {
    fail(`${what} carries an own '__proto__' key, which would not survive the wire`);
  }
}

/**
 * Migration-input recursive guard: reject any object in the input that
 * carries an OWN `__proto__` key (enumerable or not) — such a key would not
 * survive the wire (the canonical encoder emits a Rust map/struct, where a
 * `__proto__` entry either serializes as a bogus field or is dropped by
 * the legacy marker-free encoder's assignment — the key never reaches the
 * Rust map/struct). Internal only — this is a construction-input guard,
 * never a substitute for the canonical-side check.
 *
 * Cycle-safe: iterative worklist with a `WeakSet` of visited containers.
 * Each real container (plain object, array) is enumerated once with
 * `getOwnPropertyNames`, so non-enumerable own `__proto__` keys are
 * rejected too. Leaf byte/string containers (Uint8Array, MigrationFloat)
 * are NOT enumerated — a 16 MiB Uint8Array has millions of indexed own
 * key strings — they only get the cheap root-level own-key check before
 * their values are skipped.
 *
 * The walk itself throws only `MigrationError`; READING an own value can
 * still throw anything (a hostile accessor on a minted payload), which the
 * caller remains responsible for wrapping into a typed error.
 */
export function assertNoOwnProtoKeys(root: unknown, what: string): void {
  const seen = new WeakSet<object>();
  const stack: unknown[] = [root];
  while (stack.length > 0) {
    const value = stack.pop();
    if (value === null || typeof value !== 'object') continue;
    const obj = value as object;
    if (seen.has(obj)) continue;
    seen.add(obj);
    // Own `__proto__` first (cheap hasOwn check — no enumeration), then
    // skip leaf byte/string containers WITHOUT enumerating their own keys
    // (a 16 MiB Uint8Array would materialize millions of key strings);
    // they carry no admissible nested maps. Real containers are enumerated
    // ONCE, queueing every own value — INCLUDING non-index own properties
    // of arrays (a hostile `arr['__proto__']`-adjacent key's value must be
    // traversed too).
    assertNoOwnProtoRootKey(obj, what);
    if (obj instanceof Uint8Array || isMigrationFloat(obj)) continue;
    const keys = Object.getOwnPropertyNames(obj);
    for (const key of keys) {
      stack.push((obj as Record<string, unknown>)[key]);
    }
  }
}

// ── Metadata (`model/metadata.rs`) ──────────────────────────────────

function validateOrdinal(ordinal: number | bigint, where: string): void {
  if (typeof ordinal === 'bigint') {
    if (ordinal <= 0n) fail(`${where}: ordinal must be positive`);
    if (ordinal > MAX_U64) fail(`${where}: ordinal exceeds the u64 range`);
    return;
  }
  if (typeof ordinal !== 'number' || !Number.isInteger(ordinal)) {
    fail(`${where}: ordinal must be an integer (number or bigint)`);
  }
  if (ordinal <= 0) fail(`${where}: ordinal must be positive`);
  if (!Number.isSafeInteger(ordinal)) {
    fail(`${where}: ordinal above 2^53 must be given as bigint`);
  }
}

function validateMetadata(metadata: SchemaMigrationMetadata): void {
  if (typeof metadata.id !== 'string' || metadata.id.trim() === '') {
    fail('migration id must not be empty');
  }
  validateOrdinal(metadata.ordinal, `migration ${metadata.id}`);
  if (typeof metadata.display_name !== 'string') {
    fail('migration display name must be a string');
  }
  if (metadata.canonical_version !== 1) {
    fail(
      `unknown canonical version ${String(metadata.canonical_version)}; only 1 is defined`,
    );
  }
}

// ── Query-value literal bounds (`model/admission.rs`) ───────────────

interface ValueBudget {
  nodes: number;
  bytes: number;
}

function countNode(budget: ValueBudget, what: string): void {
  budget.nodes += 1;
  if (budget.nodes > MAX_VALUE_NODES) {
    fail(`${what} exceeds canonical node bound (${MAX_VALUE_NODES})`);
  }
}

function countBytes(budget: ValueBudget, size: number, what: string): void {
  budget.bytes += size;
  if (budget.bytes > MAX_VALUE_BYTES) {
    fail(`${what} exceed the 16 MiB canonical size bound`);
  }
}

/**
 * Narrow the scalar arms shared by query literals and schema defaults.
 * Mirrors the Rust `Value` grammar: `bool` stays a bool (a plain JS `number`
 * check would wrongly reject it), numbers must be finite and — when integral
 * — safe integers (above 2^53 a `bigint` is required), `bigint` is a signed
 * Rust `i64`.
 */
function validateScalarLiteral(
  value: unknown,
  what: string,
  budget: ValueBudget,
): boolean {
  if (value === null || typeof value === 'boolean') return true;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) {
      fail(`non-finite ${what} float is not canonical`);
    }
    if (Number.isInteger(value) && !Number.isSafeInteger(value)) {
      fail(`${what} integer above 2^53 must be given as bigint`);
    }
    return true;
  }
  if (typeof value === 'bigint') {
    if (value > MAX_I64 || value < MIN_I64) {
      fail(`${what} bigint exceeds the signed i64 range`);
    }
    return true;
  }
  if (isMigrationFloat(value)) {
    if (!Number.isFinite(value.value)) {
      fail(`non-finite ${what} float is not canonical`);
    }
    return true;
  }
  if (value instanceof Uint8Array) {
    countBytes(budget, value.byteLength, what);
    return true;
  }
  return false;
}

/**
 * Validate a list of query literals (`one_of` values, functional-index
 * arguments). Mirrors `validate_query_values`: depth ≤ 64, nodes ≤ 65 536,
 * total literal bytes ≤ 16 MiB including map keys.
 */
function validateQueryValues(values: readonly unknown[]): void {
  const budget: ValueBudget = { nodes: 0, bytes: 0 };
  for (const value of values) {
    validateQueryValue(value, 0, budget, 'query value');
  }
}

function validateQueryValue(
  value: unknown,
  depth: number,
  budget: ValueBudget,
  what: string,
): void {
  countNode(budget, what);
  if (depth > MAX_VALUE_DEPTH || budget.nodes > MAX_VALUE_NODES) {
    fail(`${what} exceeds canonical nesting or node bound (${MAX_VALUE_DEPTH}/${MAX_VALUE_NODES})`);
  }
  if (validateScalarLiteral(value, what, budget)) return;
  if (typeof value === 'string') {
    countBytes(budget, byteLength(value), what);
    return;
  }
  if (Array.isArray(value)) {
    for (const nested of value) {
      validateQueryValue(nested, depth + 1, budget, what);
    }
    return;
  }
  if (typeof value !== 'object' || value === null) {
    fail(`unsupported ${what} literal is not canonical`);
  }
  const proto = Object.getPrototypeOf(value);
  if (proto !== Object.prototype && proto !== null) {
    fail(`unsupported ${what} literal: plain-object maps only`);
  }
  // Plain-object map: keys count toward the byte budget (Rust counts
  // `QueryValue::Map` key lengths), values recurse.
  for (const key of Object.keys(value)) {
    countBytes(budget, byteLength(key), what);
    validateQueryValue((value as Record<string, unknown>)[key], depth + 1, budget, what);
  }
}

/**
 * Validate a schema-rule default. Mirrors `validate_filter_value`: the
 * default must be a LITERAL — explicit null is disallowed (indistinguishable
 * from an absent default on the wire) and the `$`-tagged computed-expression
 * forms (fn/ref/queryRef/expr/cond/param) are dynamic and not admitted.
 */
function validateDefault(value: unknown, depth: number, budget: ValueBudget): void {
  countNode(budget, 'schema default');
  if (depth > MAX_VALUE_DEPTH) {
    fail(`schema default nesting exceeds canonical bound (${MAX_VALUE_DEPTH})`);
  }
  if (value === null) {
    fail('an explicit null default is indistinguishable from an absent default on the wire');
  }
  if (validateScalarLiteral(value, 'schema default', budget)) return;
  if (typeof value === 'string') {
    countBytes(budget, byteLength(value), 'schema default');
    return;
  }
  if (Array.isArray(value)) {
    for (const nested of value) {
      validateDefault(nested, depth + 1, budget);
    }
    return;
  }
  // A schema default must be a LITERAL: no plain-object maps (this also
  // covers the `$`-tagged dynamic expression forms), nothing else.
  fail('schema defaults must be literal values; maps and dynamic expressions are not admitted');
}

// ── Schema rules (`model/admission.rs`) ─────────────────────────────

function validateNumBound(bound: MigrationNumBound | undefined, what: string): void {
  if (bound === undefined) return;
  if (isMigrationFloat(bound)) {
    if (!Number.isFinite(bound.value)) {
      fail(`non-finite numeric ${what} constraint is not canonical`);
    }
    return;
  }
  if (typeof bound === 'number') {
    if (!Number.isFinite(bound)) {
      fail(`non-finite numeric ${what} constraint is not canonical`);
    }
    if (Number.isInteger(bound) && !Number.isSafeInteger(bound)) {
      fail(`numeric ${what} constraint above 2^53 must be given as bigint or MigrationFloat`);
    }
    return;
  }
  if (typeof bound === 'bigint') {
    if (bound > MAX_I64 || bound < MIN_I64) {
      fail(`numeric ${what} constraint exceeds the signed i64 range`);
    }
    return;
  }
  fail(`unsupported numeric ${what} constraint type`);
}

/** Validate one migration field rule (`validate_schema_rules`). */
function validateSchemaRule(rule: MigrationFieldRuleDto): void {
  if (rule.scalar !== undefined) {
    fail('registered scalar constraints are not admitted in schema migrations');
  }
  if (rule.compare !== undefined) {
    fail('cross-field schema comparisons are dynamic and not admitted');
  }
  const budget: ValueBudget = { nodes: 0, bytes: 0 };
  if (rule.default !== undefined) {
    validateDefault(rule.default, 0, budget);
  }
  if (rule.one_of !== undefined) {
    validateQueryValues(rule.one_of);
  }
  validateNumBound(rule.min, 'min');
  validateNumBound(rule.max, 'max');
}

function validateSchemaRules(rules: readonly MigrationFieldRuleDto[]): void {
  for (const rule of rules) {
    validateSchemaRule(rule);
  }
}

// ── Function/validator code rules (`model/operation.rs`) ────────────

const MAX_CODE_BYTES = 16 * 1024 * 1024; // 16 MiB

function validateFunctionCodeSize(source: string | undefined, wasm: string | undefined): void {
  if (
    (source !== undefined && byteLength(source) > MAX_CODE_BYTES) ||
    (wasm !== undefined && byteLength(wasm) > MAX_CODE_BYTES)
  ) {
    fail('function source or module exceeds the 16 MiB canonical size bound');
  }
}

function validateFunctionDefaults(
  lang: string | undefined,
  hasWasm: boolean,
  optimize: string | undefined,
  security: string | undefined,
  visibility: string | undefined,
): void {
  const validLanguage =
    lang === undefined ||
    (hasWasm && lang === 'wasm') ||
    (!hasWasm && (lang === 'rust' || lang === 'shamir'));
  if (!validLanguage) {
    fail('function language must match its source/module representation');
  }
  if (optimize !== undefined && !OPTIMIZE_MODES.has(optimize)) {
    fail('unsupported function optimization mode is not admitted');
  }
  if (security !== undefined && !SECURITY_MODES.has(security)) {
    fail('unsupported function security mode is not admitted');
  }
  if (visibility !== undefined && !VISIBILITY_MODES.has(visibility)) {
    fail('unsupported function visibility is not admitted');
  }
}

// ── create_index validation ─────────────────────────────────────────

/**
 * Functional-option guards for one migration `create_index` payload, in the
 * exact landed Rust order (`SchemaMigrationOp::validate`, `CreateIndex` arm):
 * wrong/omitted family first; then — only for `index_type: 'functional'` —
 * missing op, name outside the built-in allowlist (`lower | upper | trim |
 * length`, what the server's index backend builds directly), then ANY
 * supplied `functional_args` (any value, including `[]`: the server never
 * reads them, so they must not enter the canonical checksum).
 *
 * Internal only (not re-exported through the barrel): shared verbatim by
 * `validateMigrationIndex` and the DDL-wrapper module, which calls it BEFORE
 * the plain `createIndex` builder so the diagnostics and their order match.
 */
export function validateFunctionalIndexOptions(
  index_type: string | undefined,
  functional_op: string | undefined,
  functional_args: readonly unknown[] | undefined,
): void {
  if (index_type !== 'functional' && (functional_op !== undefined || functional_args !== undefined)) {
    fail("functional_op/functional_args are only valid for 'functional' indexes");
  }
  if (index_type === 'functional') {
    if (functional_op === undefined) {
      fail('functional index requires an explicit built-in operation');
    }
    if (!FUNCTIONAL_INDEX_OPS.has(functional_op)) {
      fail('index function is outside the built-in functional-index allowlist');
    }
    if (functional_args !== undefined) {
      fail('functional_args are not admitted because functional indexes do not use them');
    }
  }
}

/**
 * Validate one migration `create_index` payload (internal — NOT re-exported
 * through the barrel; the migration wrappers call this through
 * `validateMigrationBody`'s own `create_index` arm, and the DDL-wrapper
 * module calls it directly).
 *
 * Three layers:
 * 1. The functional-option guards, in the landed Rust order, via
 *    {@link validateFunctionalIndexOptions}.
 * 2. The unrelated index families (`model/operation.rs`).
 * 3. The existing plain `createIndex` builder's cross-family rules (unique/sorted
 *    combination, empty fields, option-per-type families, sorted single-field,
 *    include rules) — REUSED by calling the builder on the payload's options,
 *    never copied; a plain `Error` from the builder is re-wrapped as a typed
 *    {@link MigrationError} (`'invalid_set'`).
 */
export function validateMigrationIndex(payload: MigrationCreateIndexPayload): void {
  // 1. Functional-option guards (exact Rust order).
  validateFunctionalIndexOptions(
    payload.index_type,
    payload.functional_op,
    payload.functional_args,
  );

  // 2. Unrelated index families (`model/operation.rs`).
  if (payload.index_type !== undefined && !INDEX_TYPES.has(payload.index_type)) {
    fail('unsupported index type is not admitted');
  }
  if (payload.vector_metric !== undefined && !VECTOR_METRICS.has(payload.vector_metric)) {
    fail('unsupported vector metric is not admitted');
  }
  if (
    payload.vector_quantization !== undefined &&
    payload.vector_quantization.toLowerCase() !== 'sq8'
  ) {
    fail('unsupported vector quantization is not admitted');
  }

  // 3. Reuse the plain builder's validation without copying its rules.
  try {
    createIndex(payload.create_index, payload.table, payload.fields, {
      unique: payload.unique,
      sorted: payload.sorted,
      repo: payload.repo,
      index_type: payload.index_type,
      fts_tokenizer: payload.fts_tokenizer,
      fts_language: payload.fts_language,
      functional_op: payload.functional_op,
      vector_dim: payload.vector_dim,
      vector_metric: payload.vector_metric,
      vector_quantization: payload.vector_quantization,
      include: payload.include,
      if_not_exists: payload.if_not_exists,
    });
  } catch (error) {
    fail(
      error instanceof Error
        ? `migration create_index: ${error.message}`
        : 'migration create_index: invalid index payload',
    );
  }
}

// ── Per-operation validation (`model/operation.rs`) ─────────────────

function validateOp(op: SchemaMigrationOp): void {
  // Cheap shape guards FIRST: null/undefined or non-object ops and ops
  // without a non-null plain payload must fail TYPED here, not throw a raw
  // `TypeError` deeper in the per-arm walkers.
  if (op === null || typeof op !== 'object') {
    fail('migration operation must be a non-null object');
  }
  if (op.payload === null || typeof op.payload !== 'object') {
    fail('migration operation payload must be a non-null object');
  }
  switch (op.operation) {
    case 'create_table': {
      if (op.payload.schema !== undefined) validateSchemaRules(op.payload.schema);
      return;
    }
    case 'set_table_schema': {
      validateSchemaRules(op.payload.schema);
      return;
    }
    case 'add_schema_rule': {
      validateSchemaRules([op.payload.rule]);
      return;
    }
    case 'create_index': {
      // All semantic checks (families, allowlist, `functional_args`
      // refusal, explicit-op requirement) live in `validateMigrationIndex`, shared with the
      // DDL-wrapper module so both paths validate identically.
      validateMigrationIndex(op.payload);
      return;
    }
    case 'create_function': {
      const payload = op.payload;
      if ((payload.source !== undefined) === (payload.wasm !== undefined)) {
        fail('function creation requires exactly one source or module');
      }
      validateFunctionCodeSize(payload.source, payload.wasm);
      validateFunctionDefaults(
        payload.lang,
        payload.wasm !== undefined,
        payload.optimize,
        payload.security,
        payload.visibility,
      );
      return;
    }
    case 'check_function': {
      const spec = op.payload.check_function;
      if ((spec.source !== undefined) === (spec.wasm !== undefined)) {
        fail('function check requires exactly one source or module');
      }
      validateFunctionCodeSize(spec.source, spec.wasm);
      validateFunctionDefaults(spec.lang, spec.wasm !== undefined, spec.optimize, undefined, undefined);
      return;
    }
    case 'create_validator': {
      const payload = op.payload;
      if ((payload.source !== undefined) === (payload.wasm !== undefined)) {
        fail('validator creation requires exactly one source or module');
      }
      validateFunctionCodeSize(payload.source, payload.wasm);
      return;
    }
    case 'drop_table':
    case 'rename_table':
    case 'drop_index':
    case 'rename_index':
    case 'remove_schema_rule':
    case 'drop_function':
    case 'rename_function':
    case 'alter_function':
    case 'drop_validator':
    case 'rename_validator':
    case 'bind_validator':
    case 'unbind_validator': {
      // No migration-specific constraints (`model/operation.rs` no-op arms).
      return;
    }
    default:
      fail('unsupported schema migration operation');
  }
}

// ── Function dependencies (`model/body.rs`) ─────────────────────────

function validateFunctionDependencies(
  dependencies: readonly { function: string; depends_on: string[] }[] | undefined,
): void {
  if (dependencies === undefined) return;
  const functionNames = new Set<string>();
  for (const dependency of dependencies) {
    if (typeof dependency.function !== 'string' || dependency.function.trim() === '') {
      fail('dependent function name must not be empty');
    }
    if (functionNames.has(dependency.function)) {
      fail('duplicate function dependency metadata');
    }
    functionNames.add(dependency.function);
    const dependsOn = new Set<string>();
    for (const name of dependency.depends_on) {
      if (typeof name !== 'string' || name.trim() === '' || dependsOn.has(name)) {
        fail('function dependency names must be nonempty and unique');
      }
      dependsOn.add(name);
    }
  }
}

// ── Public entry point ──────────────────────────────────────────────

/**
 * Validate one migration body, mirroring `SchemaMigrationBody::validate`
 * (Rust `model/body.rs` + `model/metadata.rs` + `model/operation.rs` +
 * `model/admission.rs`). An EMPTY `operations` list is allowed, exactly as
 * in Rust. Throws a typed {@link MigrationError} (`'invalid_set'`) before
 * the body is encoded or sent; returns normally (void) on success.
 */
export function validateMigrationBody(body: SchemaMigrationBody): void {
  // Cheap shape guards BEFORE the semantic walkers: a null/non-object body,
  // a missing metadata record or a non-array `operations` list must surface
  // as a typed `invalid_set`, not a raw `TypeError`.
  if (body === null || typeof body !== 'object') {
    fail('migration body must be a non-null object');
  }
  if (body.metadata === null || typeof body.metadata !== 'object') {
    fail('migration body must carry a metadata object');
  }
  if (!Array.isArray(body.operations)) {
    fail('migration body operations must be an array');
  }
  // Root-container wire guard: an own `__proto__` key set DIRECTLY on the
  // body or on the operations array itself must be rejected too — the
  // recursive walker below starts at each operation and never visits these
  // containers. Same wire-loss diagnostic as the recursive guard.
  assertNoOwnProtoRootKey(body, 'migration body');
  assertNoOwnProtoRootKey(body.operations, 'migration body operations');
  // Own-`__proto__` guard on the trusted metadata and dependency records
  // (the per-operation payloads are guarded inside the indexed loop below,
  // inside the same try/catch as `validateOp`).
  assertNoOwnProtoKeys(body.metadata, 'migration body metadata');
  assertNoOwnProtoKeys(body.function_dependencies, 'migration function_dependencies');
  validateMetadata(body.metadata);
  validateFunctionDependencies(body.function_dependencies);
  // Rust allows an empty `operations` list — so do we. Each operation's
  // own-`__proto__` wire guard and its semantic validation run inside one
  // indexed try/catch, so ANY non-`MigrationError` throw from the (already
  // provenance-checked) payload still surfaces as a typed, indexed
  // `invalid_set` — never as a raw `TypeError` and never by trusting the
  // thrown error's message.
  for (let index = 0; index < body.operations.length; index += 1) {
    const op = body.operations[index];
    try {
      assertNoOwnProtoKeys(op, `operation ${index}`);
      validateOp(op);
    } catch (error) {
      if (error instanceof MigrationError) {
        throw error;
      }
      fail(`operation ${index} could not be validated`);
    }
  }
}
