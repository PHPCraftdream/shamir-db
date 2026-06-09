/**
 * Call-operation wire types — type-only mirror of
 * `crates/shamir-query-types/src/call/`.
 *
 * Pure type declarations; the constructor code that assembles these
 * shapes lives in `../../builders/call.ts`.
 *
 * Serde notes encoded here (so the builder emits the exact wire shape):
 *   - `params` is `#[serde(default, skip_serializing_if = "Vec::is_empty")]`
 *     → OMITTED when empty.
 *   - `repo` is `#[serde(default = "default_repo")]` WITHOUT skip → ALWAYS
 *     present (default "main").
 *
 * PLATFORM-AGNOSTIC.
 */

import type { FilterValue } from './filter.js';

/**
 * Stored-procedure / callable-function batch operation.
 * Mirrors `CallOp` in `crates/shamir-query-types/src/call/mod.rs`.
 */
export interface CallOp {
  /** Name of the function to invoke. */
  call: string;
  /** Positional parameters (omitted when empty). */
  params?: FilterValue[];
  /** Repository context — always emitted, defaults to "main". */
  repo: string;
}
