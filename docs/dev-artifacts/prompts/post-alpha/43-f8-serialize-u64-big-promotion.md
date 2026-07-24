# Brief for #798 (F-8) — QueryValueSerializer::serialize_u64 must promote to Big, not wrap

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (confirmed against the established contract, not just the review's claim)

`crates/shamir-engine/src/query/batch/query_value_serializer.rs`'s
`QueryValueSerializer::serialize_u64` (~line 142-144):

```rust
fn serialize_u64(self, v: u64) -> Result<QueryValue, QvSerError> {
    Ok(QueryValue::Int(v as i64))
}
```

The comment directly above it (~line 131-132) claims this "matches
`ValueVisitor::visit_u64` (`value as i64`)" — **this is factually wrong.**
Read `crates/shamir-types/src/types/value.rs`'s `ValueVisitor::visit_u64`
(~line 142-155) — it is the established, documented "Unified u64 contract":

```rust
fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
    // Unified u64 contract: values that fit in `i64` decode as a plain
    // `Int`; values above `i64::MAX` promote losslessly to `Big` (an
    // arbitrary-precision `BigInt`) instead of silently wrapping via
    // `value as i64` (which sign-flips `u64::MAX` to `-1`). ...
    if value <= i64::MAX as u64 {
        Ok(Value::Int(value as i64))
    } else {
        Ok(Value::Big(BigInt::from(value)))
    }
}
```

So the ACTUAL, established contract this module is supposed to mirror
(per its own module doc comment's stated goal — "Wire-shape parity with
the old msgpack round-trip", i.e. matching what
`rmp_serde::to_vec_named` + `ValueVisitor`-based decode used to produce)
promotes `u64` values above `i64::MAX` to `Big`. `serialize_u64`'s plain
`v as i64` cast instead silently sign-flips e.g. `u64::MAX` to `-1` —
exactly the bug `visit_u64`'s own doc comment says it exists to prevent.
This means any `u64`-typed field on `QueryRecord`/`QueryStats`/
`PaginationInfo`/etc. (the types this serializer converts — check which
ones actually carry raw `u64` fields large enough to exceed `i64::MAX` in
practice, e.g. `records_scanned`/`records_returned`/`total_count`-style
counters) that legitimately exceeds `i64::MAX` gets silently corrupted
into a negative `Int` instead of promoting to `Big`.

## The fix

```rust
fn serialize_u64(self, v: u64) -> Result<QueryValue, QvSerError> {
    if v <= i64::MAX as u64 {
        Ok(QueryValue::Int(v as i64))
    } else {
        Ok(QueryValue::Big(num_bigint::BigInt::from(v)))
    }
}
```

Match `ValueVisitor::visit_u64`'s exact bound check (`value <= i64::MAX as
u64`) and use the same `BigInt::from(v)` construction — check what crate
path `BigInt` is imported from elsewhere in this crate (`num_bigint::BigInt`
is used by `ValueVisitor`; confirm `shamir-engine`'s `Cargo.toml` already
depends on `num_bigint`, likely yes given `QueryValue::Big` exists) and add
the `use` import at this file's top per this repo's import convention.

Update the WRONG comment above `serialize_u64` (~line 131-132, currently
shared with the `u8`/`u16`/`u32` methods above it) to correctly describe
the split: `u8`/`u16`/`u32` can never exceed `i64::MAX` so they always
land in `Int` (that part of the comment is still correct for those three
methods) — split the comment so `serialize_u64` gets its own, ACCURATE
description of the Big-promotion behavior, referencing
`ValueVisitor::visit_u64`'s "Unified u64 contract" doc comment by name so
a future reader can find the canonical rationale.

## Check for the same pattern elsewhere in this file

`serialize_i8`/`i16`/`i32`/`i64` (~line 118-129) are all sign-preserving
casts into `i64` — these are correct as-is (no promotion needed; a
negative `i64` is already exactly representable). Skim the REST of this
file (sequence/map/struct serialization, ~line 160 onward) for any other
place that might independently re-derive a `u64`-like value and cast it
without the same bounds check — if you find one, apply the same fix;
otherwise state in your final report that `serialize_u64` was the only
site.

## Tests

Find or extend the existing differential test file
`crates/shamir-engine/src/query/batch/tests/query_value_serializer_tests.rs`
(referenced by this module's own doc comment as "the exhaustive
differential test... asserts `PartialEq` parity across every
representative shape") and add:

1. A `u64` value `<= i64::MAX` (e.g. `42u64`, or `i64::MAX as u64`) →
   `QueryValue::Int(...)`, unchanged from today.
2. A `u64` value `> i64::MAX` (e.g. `u64::MAX`, and `(i64::MAX as
   u64) + 1`) → `QueryValue::Big(BigInt::from(v))`, NOT a negative `Int`.
   Assert the `Big` value's exact numeric value round-trips correctly
   (e.g. via `.to_string()` or however this test file's existing
   assertions compare `Big` values — follow its established pattern).
3. If this file's differential-testing convention compares against the
   OLD msgpack-round-trip behavior (encode via `rmp_serde::to_vec_named`
   then decode via `ValueVisitor`) for parity, add the `u64`-overflow case
   to THAT comparison too, proving `to_query_value` and the old
   round-trip now agree (they didn't before this fix, for values above
   `i64::MAX`).
4. Find whichever real `QueryResult`/`QueryRecord`/`QueryStats`/
   `PaginationInfo` struct field is actually `u64`-typed and would flow
   through `serialize_u64` in practice (check the struct definitions in
   `shamir-query-types`/`shamir-engine`'s `query::read` module) and add
   one end-to-end test through `to_query_value` on that REAL struct with
   a huge value in that field, not just a synthetic bare-`u64` unit test —
   this proves the fix actually reaches the real call sites this
   serializer exists to serve, not just the primitive in isolation.

## Constraints

- Do NOT change any other `serialize_*` method's behavior.
- Do NOT change `ValueVisitor::visit_u64` itself — it's already correct;
  this task brings `serialize_u64` INTO alignment with it.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.
- Follow workspace conventions: `use` at file top, one primary export per
  file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```
