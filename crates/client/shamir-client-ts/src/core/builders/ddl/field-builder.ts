/**
 * Fluent `field()` builder for `FieldRuleDto` schemas. Split out of the
 * former monolithic `builders/ddl.ts`.
 */

import type {
  FieldRuleDto,
  ConstraintsDto,
  ForeignKeyDto,
  FkAction,
} from '../../types/schema/ddl.js';


// ── field() fluent API ──────────────────────────────────────────────

/**
 * Fluent builder for a single `FieldRuleDto`. Mirrors the Rust
 * `shamir_query_builder::ddl::field()` API.
 *
 * ```ts
 * field(["email"]).string().max(255).required()
 * field(["age"]).int().min(0).max(150)
 * ```
 */
export class FieldBuilder {
  private _path: string[];
  private _type = '';
  private _constraints: ConstraintsDto = {};

  constructor(path: string[]) {
    this._path = path;
  }

  // ── type setters ────────────────────────────────────────────────
  string(): this { this._type = 'string'; return this; }
  int(): this { this._type = 'int'; return this; }
  f64(): this { this._type = 'f64'; return this; }
  dec(): this { this._type = 'dec'; return this; }
  bool(): this { this._type = 'bool'; return this; }
  bin(): this { this._type = 'bin'; return this; }
  list(): this { this._type = 'list'; return this; }
  map(): this { this._type = 'map'; return this; }
  any(): this { this._type = 'any'; return this; }
  typeTag(tag: string): this { this._type = tag; return this; }

  // ── constraint setters ──────────────────────────────────────────
  required(): this { this._constraints.required = true; return this; }
  nullable(): this { this._constraints.nullable = true; return this; }
  unsigned(): this { this._constraints.unsigned = true; return this; }
  min(v: number): this { this._constraints.min = v; return this; }
  max(v: number): this { this._constraints.max = v; return this; }
  len(v: number): this { this._constraints.len = v; return this; }
  maxLen(v: number): this { this._constraints.max_len = v; return this; }
  minLen(v: number): this { this._constraints.min_len = v; return this; }
  arrayOf(tag: string): this { this._constraints.array_of = tag; return this; }

  // ── Phase B constraint setters ──────────────────────────────────

  /**
   * Phase B — scalar-bridge: validate the field by calling the named
   * registered scalar as a predicate.
   */
  scalar(name: string): this { this._constraints.scalar = name; return this; }

  /**
   * Allowed-value set (enum constraint).
   */
  oneOf(values: import('../../types/write/write.js').WireValue[]): this {
    this._constraints.one_of = values;
    return this;
  }

  /**
   * ③.2c — default value (literal or expression) stamped on INSERT for an
   * absent field (extends Phase ②.4b literal-only to expression).
   *
   * - **Literal** forms (null/bool/number/string/array/object) route through
   *   the fast `apply_defaults` path (②.4c behaviour is unchanged).
   * - **Expression** `ComputedExpr` forms (`$fn` / `$ref` / etc.) route
   *   through `apply_transforms` → `eval_write_value` → `builtin_scalars()`
   *   at admission-time. User scalars are NOT available here (same boundary
   *   as inline `$fn` write-field expressions).
   *
   * Accepts `WriteValue` (superset of `WireValue` + `ComputedExpr`). Mirrors
   * the Rust builder's `.default(impl Into<FilterValue>)`.
   */
  default(value: import('../../types/write/write.js').WriteValue): this {
    this._constraints.default = value;
    return this;
  }

  /**
   * Phase C3 — unique constraint.
   * The field value must not duplicate any existing row in the same table.
   */
  unique(): this { this._constraints.unique = true; return this; }

  /**
   * Phase B — named format check (`"email"` / `"url"` / `"uuid"` / `"date"`).
   */
  format(kind: string): this { this._constraints.format = kind; return this; }

  /**
   * Phase B — cross-field comparison against another path.
   * `op` is the comparison operator string (`"<"`, `"<="`, `"=="`, `"!="`,
   * `">="`, `">"`).
   */
  compare(other: string[], op: string): this {
    this._constraints.compare = { other, op };
    return this;
  }

  /**
   * ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
   * this field on **every** write (INSERT and UPDATE). The server clock is
   * always authoritative — any caller-supplied value is overwritten.
   *
   * Typical usage: `updated_at` field. Mirrors the Rust `.auto_now()` builder.
   */
  autoNow(): this { this._constraints.auto_now = true; return this; }

  /**
   * ③.2d — server-stamping: stamp the server wall-clock nanoseconds onto
   * this field on **INSERT** only, and only when the field is absent.
   * An explicitly-supplied value (including explicit `null`) is preserved.
   *
   * Typical usage: `created_at` field. Mirrors the Rust `.auto_now_add()` builder.
   */
  autoNowAdd(): this { this._constraints.auto_now_add = true; return this; }

  /**
   * Phase C2 — forward-only foreign-key reference.
   * The field value must exist in `refTable.refField`.
   * `onDelete` defaults to `'restrict'` (matching the Rust builder); pass
   * `'cascade'` / `'set_null'` / `'no_action'` to override.
   * `onUpdate` defaults to `'no_action'` (Phase ②.2a — surface only;
   * enforcement in ②.2b; additive — existing callers keep current behavior).
   */
  foreignKey(
    refTable: string,
    refField: string,
    opts?: { onDelete?: FkAction; onUpdate?: FkAction },
  ): this {
    const fk: ForeignKeyDto = {
      ref_table: refTable,
      ref_field: refField,
      on_delete: opts?.onDelete ?? 'restrict',
    };
    const onUpdate = opts?.onUpdate ?? 'no_action';
    if (onUpdate !== 'no_action') {
      fk.on_update = onUpdate;
    }
    this._constraints.foreign_key = fk;
    return this;
  }

  /** Finalize into a wire-ready `FieldRuleDto`. */
  build(): FieldRuleDto {
    const dto: FieldRuleDto = {
      path: this._path,
      type: this._type,
    };
    // Spread only defined constraint keys (mirrors serde skip_serializing_if).
    for (const [k, v] of Object.entries(this._constraints)) {
      if (v !== undefined) {
        (dto as unknown as Record<string, unknown>)[k] = v;
      }
    }
    return dto;
  }
}

/** Start building a `FieldRuleDto` for the given path segments. */
export function field(path: string[]): FieldBuilder {
  return new FieldBuilder(path);
}
