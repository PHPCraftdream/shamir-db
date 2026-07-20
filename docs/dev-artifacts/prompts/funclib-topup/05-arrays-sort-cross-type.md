# Funclib top-up 4e — fix `arrays/sort` cross-type + add `arrays/sort_desc`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fifth P0 item of "Этап 4 — v0.10 funclib top-up"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
per report 10 (`docs/dev-artifacts/research/2026-07-17-release-audit/10-release-readiness-v0.10.md`,
~line 157):

> Fix `arrays/sort` to use cross-type `compare()` (keep numeric fast path),
> add `arrays/sort_desc`. Sorting a string array is a day-one operation;
> the current `type_mismatch` will read as a bug.

## The bug

`crates/shamir-funclib/src/arrays.rs`'s `sort` registration (~lines
176-192):

```rust
reg.register(
    "sort",
    FnEntry::pure(
        |a| {
            let arr = arg_list(a, 0)?;
            // Numeric sort by decimal value; non-numeric element -> type_mismatch.
            let mut keyed: Vec<(Decimal, QueryValue)> = Vec::with_capacity(arr.len());
            for i in 0..arr.len() {
                keyed.push((arg_dec(arr, i)?, arr[i].clone()));
            }
            keyed.sort_by(|x, y| x.0.cmp(&y.0));
            Ok(v_list(keyed.into_iter().map(|(_, v)| v).collect()))
        },
        1,
        Some(1),
    ),
);
```

`arg_dec(arr, i)?` forces EVERY element through a `Decimal` conversion —
sorting `["banana", "apple"]` fails with `type_mismatch` instead of
producing `["apple", "banana"]`. Meanwhile `crates/shamir-funclib/src/math.rs`'s
`min`/`max`/`clamp`/`between` already establish the cross-type pattern this
whole workspace uses for exactly this kind of thing:
`crate::compare::compare` (the canonical total order over `QueryValue`,
already handling Null/Bool/numeric-cross-subtype/Str/Bin/List/Set/Map —
see `crates/shamir-funclib/src/compare.rs`'s own module doc comment for the
full rank table).

## The fix

Replace the `Decimal`-forcing sort with a direct `compare::compare`-based
sort:

```rust
reg.register(
    "sort",
    FnEntry::pure(
        |a| {
            let arr = arg_list(a, 0)?;
            let mut out = arr.to_vec();
            out.sort_by(compare::compare);
            Ok(v_list(out))
        },
        1,
        Some(1),
    ),
);
reg.register(
    "sort_desc",
    FnEntry::pure(
        |a| {
            let arr = arg_list(a, 0)?;
            let mut out = arr.to_vec();
            out.sort_by(|x, y| compare::compare(x, y).reverse());
            Ok(v_list(out))
        },
        1,
        Some(1),
    ),
);
```

(Check the exact import path — `arrays.rs` may need to add
`use crate::compare::compare;` if it isn't already imported; check how
`math.rs` imports it, same crate-relative path.) This is not a
"keep a fast path AND a slow path" two-branch design — `compare::compare`
ITSELF already has same-type numeric fast paths internally
(`compare_numeric`'s `Int vs Int`/`Dec vs Dec`/`F64 vs F64` arms return
immediately without any cross-type promotion when both sides are the same
numeric subtype), so a single `sort_by(compare::compare)` call is
simultaneously correct for mixed-type arrays AND fast for the common
homogeneous-numeric-array case — read `compare.rs`'s `compare_numeric`
yourself to confirm this before assuming you need a separate special-cased
branch; the report's "(keep numeric fast path)" parenthetical is asking
you to verify this fast path still exists after the fix, NOT to write a
second bespoke numeric-only branch alongside the `compare`-based one.

Update the module's own doc comment (near the top of `arrays.rs` — check
whether it lists the registered function names, mirroring `math.rs`'s doc
comment style) to include `sort_desc` if such a list exists.

## Tests

1. `sort` on a `Str` array (`["banana", "apple", "cherry"]`) succeeds and
   returns lexicographic order — this is the exact bug: it previously
   failed with `type_mismatch`.
2. `sort` on a mixed-type array (e.g. `[Int(2), Str("a"), Int(1)]`) sorts
   by `compare`'s cross-type rank ordering (numeric rank < Str rank, so
   both ints sort before the string) — don't assert a `type_mismatch`
   error here, assert the actual cross-type-ordered result.
3. Regression: `sort` on a homogeneous numeric array (mixing
   `Int`/`Dec`/`F64` subtypes) still sorts by VALUE, not by rank-then-
   subtype — e.g. `[Int(3), Dec(1.5), F64(2.0)]` → `[Dec(1.5), F64(2.0),
   Int(3)]` (all numeric-rank, compared by value via `compare_numeric`'s
   cross-subtype arms).
4. `sort_desc` on the same numeric array returns the exact reverse order
   of `sort`.
5. `sort_desc` on a `Str` array returns reverse-lexicographic order.
6. Regression: existing all-Int/all-Dec `sort` tests (if any exist in
   `crates/shamir-funclib/src/arrays/tests/`) continue to pass with the
   same VALUES in the same order (only the mechanism changed, not the
   observable numeric-array behavior).

## Out of scope

- Do NOT touch `arrays.rs`'s OTHER functions (`distinct`, `join`, `sum`/
  `min`/`max`/`avg` reducers) — this brief is scoped to `sort`/`sort_desc`
  only.
- Do NOT touch any OTHER Этап 4 P0 item (null functions, agg wire params/
  distinct, datetime format/parse, uuid_v4 — all done in earlier stages of
  this same campaign; `parse_json`/`to_json` — separate, final leaf task).
- Do NOT implement the P1-tier array set-ops (`reverse`/`concat`/`union`/
  `intersect`/`difference`) mentioned elsewhere in the work plan — separate,
  capacity-contingent decision made later.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-funclib --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-funclib`,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Confirm explicitly: `compare::compare`'s existing same-subtype numeric
  fast paths (read `compare_numeric` in `compare.rs`) are what keep the
  numeric case fast — you did NOT add a second, separate numeric-only
  sort branch alongside the `compare`-based one.
