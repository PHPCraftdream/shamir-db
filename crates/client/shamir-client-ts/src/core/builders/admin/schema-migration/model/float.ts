/**
 * Migration-specific tagged float bound.
 *
 * The canonical format v1 (`canonical/ddl.rs::encode_num`) encodes a rule
 * bound as EITHER `NumDto::Int(i64)` (tag byte 0) OR `NumDto::F64(f64)`
 * (tag byte 1 + 8 big-endian IEEE-754 bits). The two are different canonical
 * bytes for the same mathematical value (`2` vs `2.0`), and the f64 form is
 * bit-exact including negative zero — a plain JS `number` cannot express
 * "this is a float bound" (`2.0` collapses to `2`, and `-0 === 0`).
 *
 * `MigrationFloat` is the explicit tag: wrap a bound with
 * {@link migrationFloat} to force the bit-exact f64 encoding. The class
 * holds the value WITHOUT normalization (so `-0` stays `-0` for
 * `DataView.setFloat64` in the canonical encoder).
 *
 * PLATFORM-AGNOSTIC.
 */

export class MigrationFloat {
  /** The bound value, stored verbatim (no normalization; `-0` preserved). */
  readonly value: number;

  constructor(value: number) {
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      throw new TypeError(
        'migrationFloat: bound must be a finite number ' +
          '(canonical format v1 rejects non-finite f64 bounds)',
      );
    }
    this.value = value;
  }

  toString(): string {
    // Object.is distinguishes the negative zero for an honest debug string.
    return Object.is(this.value, -0) ? 'MigrationFloat(-0)' : `MigrationFloat(${this.value})`;
  }
}

/**
 * Tag a migration field-rule bound as a bit-exact f64 (`NumDto::F64`).
 * Necessary whenever the bound is not an integer or must serialize as a
 * float on purpose (e.g. `-0`, `2.0`, `0.5`).
 */
export function migrationFloat(value: number): MigrationFloat {
  return new MigrationFloat(value);
}

/**
 * True when `v` is a `MigrationFloat` (structural guard for the canonical
 * encoder and validators; class identity across module instances holds
 * because the class is only ever imported from this module).
 */
export function isMigrationFloat(v: unknown): v is MigrationFloat {
  return v instanceof MigrationFloat;
}
