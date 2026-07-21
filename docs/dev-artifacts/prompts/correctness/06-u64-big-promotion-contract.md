# FG-1: Unified u64 contract — promote to `Big` instead of wrapping/clamping (all levels, both builders)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## DECIDED CONTRACT (user, 2026-07-21) — do not re-litigate

`u64 <= i64::MAX` → `Value::Int`/`QueryValue::Int` (unchanged). `u64 >
i64::MAX` → promote to the EXISTING `Value::Big(BigInt)` /
`QueryValue::Big(BigInt)` variant (lossless), not a wrapping cast (current
bug #1: silent sign inversion) and not a clamp to `i64::MAX` (current bug
#2: silent data corruption to a wrong-but-plausible value). This is the
single most severe class of silent data corruption found in this
campaign's second review — implement it precisely, at every site listed
below, no more and no less.

## Context — already investigated exhaustively, do not re-derive

### Fix site 1 (primary) — `crates/shamir-types/src/types/value.rs:142`

```rust
fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
    Ok(Value::Int(value as i64))
}
```
This is `ValueVisitor::visit_u64`, the self-describing msgpack decode entry
point for BOTH `Value<String>` (`UserValue`/`QueryValue`) AND
`Value<InternerKey>` (`InnerValue`) — it is generic over `Key`, so fixing
it here fixes the decode path for every concrete `Value` instantiation,
including the "tree decoder" (`InnerValue::from_bytes` via `rmp_serde`,
which dispatches through this same visitor). Fix:
```rust
fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
    if value <= i64::MAX as u64 {
        Ok(Value::Int(value as i64))
    } else {
        Ok(Value::Big(BigInt::from(value)))
    }
}
```
(`BigInt` is already imported in this file — `num_bigint::BigInt`.)

### Fix site 2 — `crates/shamir-types/src/types/value.rs:207` (secondary, test-fixture path)

Inside the SAME file's `visit_map` (the `UserValue`/`Key=String` prefixed-key
parsing branch, used by test fixtures / non-wire construction — `UserValue`
is `#[deprecated]`, tests-only):
```rust
Some("u") => map.next_value::<u64>().map(|v| Value::Int(v as i64))?,
```
Apply the SAME `<= i64::MAX` → `Int` / `else` → `Big` fix here too, for
consistency (this file already has a working `Some("big") => ...` branch
a few lines below using a `BigIntSource` helper type — read it to confirm
the exact `BigInt::from_str`/construction idiom already established in
this file, and reuse it rather than inventing a second way to build a
`BigInt`).

### Fix site 3 — `crates/shamir-types/src/types/value.rs:620` — `impl From<u64> for Value<String>`

```rust
impl From<u64> for Value<String> {
    fn from(v: u64) -> Self {
        Value::Int(v as i64)
    }
}
```
This is the conversion a Rust query-builder caller hits when constructing
a value/literal from a plain `u64` (e.g. `.value(some_u64)` sugar sites
that rely on `Into<Value<String>>`). Apply the same fix. Do NOT touch the
neighboring `From<u32>`/`From<usize>` impls — `u32::MAX` and (on any
realistic target) `usize` values used in this codebase never approach
`i64::MAX`, those casts are not bugs (verified: `u32::MAX` fits in `i64`
with headroom; leave them alone, this is explicitly out of scope, don't
"fix" something that isn't broken).

### Fix site 4 — `crates/shamir-query-types/src/read/query_record.rs:103-108`

```rust
fn visit_u64<E: de::Error>(self, v: u64) -> Result<QueryRecord, E> {
    // u64 > i64::MAX cannot be represented losslessly in QueryValue::Int;
    // clamp to i64::MAX as a safe approximation (u64 > i64::MAX saturates).
    Ok(QueryRecord::Direct(QueryValue::Int(
        v.min(i64::MAX as u64) as i64
    )))
}
```
Fix to the same `<= i64::MAX` → `QueryValue::Int` / `else` →
`QueryValue::Direct(QueryValue::Big(BigInt::from(v)))` contract. Remove the
now-inaccurate "clamp... saturates" comment.

### Fix site 5 — `crates/shamir-query-types/src/filter/filter_value.rs:68` — `lit_u64` (Rust query-builder)

```rust
/// This is an explicit lossy escape-hatch for values that may exceed
/// `i64::MAX`. Values above `i64::MAX` will wrap silently.
/// For all other integer widths, use `lit(v)` (which goes through
/// `From<i8/i16/i32/i64/u8/u16/u32>`).
pub fn lit_u64(v: u64) -> FilterValue {
    FilterValue::Int(v as i64)
}
```
`FilterValue` (`crates/shamir-query-types/src/filter/filter_value.rs`) has
**NO `Big` variant** — verified, its variants are `Null`/`Bool`/`Int(i64)`/
`Float(f64)`/`String(String)`/`Binary`/`Array`/`FieldRef`/`QueryRef`/
`FnCall` (and a few more — read the full enum before editing). Adding a
brand-new `FilterValue::Big` wire variant (with its own serde tag,
comparison-resolution wiring, and a matching TS type) is a materially
bigger change than this task's scope — **DO NOT add one.**

Instead: `Value::Big` ALREADY serializes to the wire as a plain decimal
STRING (`Value::Big(b) => serializer.serialize_str(&b.to_string())` in
`value.rs`'s `Serialize` impl — confirmed, this is pre-existing,
established behavior for `Big` generally, not something this task
changes). And `QueryValue::Big` vs `QueryValue::Str` cross-type EQUALITY
is ALREADY implemented (`crates/shamir-engine/src/query/read/
hashable_query_value.rs`, `(QueryValue::Big(x), QueryValue::Str(y)) | ...`
branch, from the earlier "Dec/Big comparison layer" work this same
campaign already did). So the coherent fix, reusing this EXISTING bridge
rather than inventing a new one:
```rust
pub fn lit_u64(v: u64) -> FilterValue {
    if v <= i64::MAX as u64 {
        FilterValue::Int(v as i64)
    } else {
        FilterValue::String(v.to_string())
    }
}
```
Update the doc comment to describe the new, lossless contract (values
above `i64::MAX` are represented as their exact decimal string, matching
how `Value::Big` itself serializes — no more silent wrapping).

**MANDATORY empirical verification (do not skip):** write a test proving
an `Eq` filter built via `lit_u64(large_value)` against a stored field
whose value round-tripped to `QueryValue::Big` (i.e. was originally
inserted as a raw u64 > `i64::MAX` and decoded via fix site 1) actually
MATCHES that record — i.e. confirm the filter-resolution path really does
feed `FilterValue::String` into a comparison against `QueryValue::Big` via
the existing cross-type bridge, end to end, not just in isolated unit
logic. **If this does NOT work** (the bridge doesn't apply at the actual
filter-evaluation call site for some structural reason), STOP, do not
force a workaround — report the exact failure precisely (what actually
happened, where the comparison diverges) so this can be re-scoped as its
own follow-up rather than silently shipping a filter that doesn't match.

### Fix site 6 — `crates/shamir-client-ts/src/core/builders/filter.ts:489` — `litU64` (TS query-builder)

```ts
/**
 * Explicit lossy escape-hatch for full u64 values. Mirrors Rust
 * `lit_u64` (`filter_value.rs:68`) — values above `i64::MAX` wrap
 * silently (Rust casts `v as i64` without bounds checks).
 *
 * Accepts `bigint` for ergonomics (callers may already hold a bigint),
 * but ALWAYS returns `number` via `Number(v)`. Values above `2^53` lose
 * precision — this is the JS analogue of Rust's lossy cast. The result
 * is a msgpack-safe integer (no bigint on the wire).
 *
 * NO runtime range checks are added — no throw.
 */
export function litU64(v: bigint | number): number {
  return typeof v === 'bigint' ? Number(v) : v;
}
```
This explicitly mirrors fix site 5's OLD (buggy) behavior — fix it to
mirror site 5's NEW behavior instead: values representable in `i64`
(`<= 9223372036854775807`) stay a `number` (unchanged, msgpack-safe int);
values above that become the exact decimal STRING (matching
`FilterValue::String(v.to_string())` on the Rust side), returned as a
`string`, NOT a further-lossy `Number(v)` call. Change the return type to
`bigint | number | string` — hold on, actually since the function's own
job is to produce something usable as a `FilterValue` wire literal, the
cleanest signature is `litU64(v: bigint | number): number | string`,
returning a plain decimal `string` for the overflow case (JS strings are
exact for arbitrary-precision decimal text — no BigInt needed on this
side at all for correctness, since the value never needs arithmetic in
JS, only wire representation). Check how this function's return value is
actually consumed downstream (what `FilterValue`-shaped TS type accepts
here) and adjust that type definition if it currently requires `number`
specifically — search `crates/shamir-client-ts/src/core/types/` for the
`FilterValue`-equivalent TS type/interface and widen the relevant field's
type if needed. Update the doc comment to describe the new lossless
contract, matching site 5's updated comment in spirit.

### Explicitly OUT OF SCOPE — `record_view`'s lens (`crates/shamir-types/src/record_view/lens.rs`)

`uint_to_record_value` (~line 614) ALREADY correctly handles `u64 >
i64::MAX` losslessly — via `RecordValue::Str(Cow::Owned(u.to_string()))`,
NOT truncation. `RecordValue` (the zero-copy record view type) has NO
`Big`/`BigInt` variant BY DESIGN (it exists specifically for cheap
borrowed views over raw msgpack bytes; `BigInt` isn't a borrowed/zero-copy
type, so adding one there would defeat the type's whole purpose — this is
a deliberate, already-documented architectural choice, not a bug). **Do
NOT touch `lens.rs` or `uint_to_record_value`.** After this task's fixes
land, the "tree" decoder (fix site 1, now `Big`) and the "lens" decoder
(unchanged, `Str(decimal)`) will represent the SAME lossless value two
different ways in two different type systems — this is expected and
correct, not a new divergence to resolve.

**However**, `crates/shamir-types/src/record_view/tests/parity_tests.rs`
(around lines 380-425) and `crates/shamir-types/src/record_view/tests/
deintern_parity_tests.rs` (around line 243) contain tests that
EXPLICITLY assert the OLD tree behavior (`Int` via truncation) as
"expected" and document the lens/tree divergence in those terms ("tree
truncates via visit_u64... lens maps to Str for overflow... known
divergence, not a bug in the lens"). **These tests MUST be updated** to
assert the NEW tree behavior (`Big`, not truncated `Int`) and reword the
divergence comments to describe "tree → `Big`, lens → `Str(decimal)`,
both lossless, two different representations" — do not leave stale
comments describing behavior that no longer exists after this fix.

## Grep sweep (MANDATORY — confirm no other truncating u64→i64 casts remain)

Two candidate sites were checked and found to be FALSE POSITIVES (do not
touch, document why in your summary if you re-encounter them so the next
reader doesn't re-flag them):
- `crates/shamir-engine/src/query/filter/eval_bytes.rs:377` —
  `read_u64_be(bytes, after)? as i64` inside the `0xd3` (msgpack **int64**,
  signed) branch. This is a correct BIT-PATTERN reinterpretation of an
  already-signed value read via a u64-returning byte helper, not a
  truncation of an actual unsigned value. The genuinely unsigned `0xcf`
  (uint64) branch a few lines above already correctly returns a dedicated
  `RawScalar::U64(v)`, no cast at all. No fix needed here.
- Everywhere `Value::Big`/`QueryValue::Big` is already CONSUMED (compared,
  hashed, ordered) — `crates/shamir-engine/src/query/filter/resolve.rs`,
  `crates/shamir-engine/src/query/read/hashable_query_value.rs`,
  `crates/shamir-engine/src/query/read/order.rs`,
  `crates/shamir-engine/src/query/read/aggregate.rs`,
  `crates/shamir-engine/src/query/batch/fk_on_update.rs`,
  `crates/shamir-engine/src/query/batch/fk_restrict.rs` — ALL already
  handle `Big` correctly (cross-type comparison/hash/sort/FK-cascade
  already wired from earlier campaign work). Do NOT modify any of these —
  they are already correct and exist purely to CONSUME `Big` values that,
  after this task's fixes, will simply appear MORE OFTEN (previously
  wrapped/clamped values now correctly arrive as `Big` instead of a wrong
  `Int`). Confirm this with a quick read of each, but no code changes
  expected here.

Beyond the 6 fix sites above and the 2 confirmed false positives, run your
own targeted grep across `crates/shamir-types/src`, `crates/shamir-query-types/src`,
and `crates/shamir-engine/src` for any OTHER `as i64` cast whose input is a
`u64` value that could plausibly exceed `i64::MAX` in real usage (ignore
`u32`/`u16`/`u8`/small-range casts — those can never overflow `i64`) and
fix any genuine finds using the same contract. Report exactly what you
found (including "nothing else found" if that's the case) in your summary.

## Tests (MANDATORY — every level named in the task)

1. **Rust unit** — `crates/shamir-types/src/types/tests/value_tests.rs` (or
   wherever `ValueVisitor`/`Value::from` is already tested — find the
   sibling convention): decode a msgpack `uint64` payload at exactly
   `i64::MAX` (must stay `Int`), `i64::MAX as u64 + 1` (must become
   `Big`), and `u64::MAX` (must become `Big` with the exact correct
   decimal value, not `-1` and not a clamped `i64::MAX`). Also test
   `Value::from(u64::MAX)` directly (fix site 3).
2. **Rust unit** — `crates/shamir-query-types/src/read/tests/` (find the
   sibling test file for `query_record.rs`, or create one per this repo's
   `tests/` convention if none exists) — same three boundary values for
   `QueryRecord`'s `visit_u64`.
3. **Rust unit** — `filter_value.rs`'s test module — `lit_u64` boundary
   values (`i64::MAX` stays `FilterValue::Int`, `i64::MAX + 1` becomes
   `FilterValue::String` with the exact decimal text).
4. **Rust integration/e2e** — the mandatory cross-cutting proof from fix
   site 5: insert a record via the real server with a field holding a raw
   u64 > `i64::MAX` (find how an existing e2e test constructs a
   raw-msgpack-encoded write bypassing the builder's own `i64`-typed
   surface — check `crates/shamir-client/tests/` or `crates/shamir-server/
   tests/` for a precedent of writing a raw/custom-encoded value), read
   it back and confirm the exact value round-trips as `Big` (not
   corrupted), then run an `Eq` filter via `lit_u64(that_value)` and
   confirm the record matches.
5. **TS e2e** — mirror test 4 in `crates/shamir-client-ts/src/__tests__/`
   (check the `describe.skipIf(!SERVER_AVAILABLE)` self-skip convention
   already established in `e2e-harness.ts` from RI-5 — follow it): write
   a value via a raw path if the TS builder's typed surface can't express
   a raw u64 > `i64::MAX` directly, read it back, confirm no precision
   loss, and confirm `litU64(largeValue)` in an `Eq` filter matches it.
6. **Regression on sort/index of Big values**: a test confirming a
   collection containing BOTH ordinary `Int` values and promoted `Big`
   values sorts/orders correctly relative to each other (the comparison
   infra already exists per the grep sweep above — this test PROVES it
   stays correct once `Big` values become reachable via normal u64
   ingestion, not just via manual construction in existing tests).
7. Update the two stale-assertion tests named above
   (`record_view/tests/parity_tests.rs`, `deintern_parity_tests.rs`) to
   match the new tree-decoder behavior.

## Docs

Add a short, precise note to
`docs/guide-docs/client-server-protocol-spec/` (find the file documenting
numeric wire semantics — check `AUTH_PROTOCOL.md`/`IMPLEMENTATION_GUIDE.md`
or a dedicated wire-format doc; if none covers this, the most relevant
existing file) stating the contract: `u64 <= i64::MAX` decodes as a plain
integer; `u64 > i64::MAX` decodes as an arbitrary-precision value
(`Value::Big` server-side) and is represented as an exact decimal STRING
at wire/filter-literal boundaries (both `lit_u64` and `litU64`) — no
silent wrapping or clamping anywhere in the stack.

`CHANGELOG.md`: one `[Unreleased]` bullet — this is a correctness fix for
silent data corruption (two DIFFERENT prior bugs: sign-flip wrap and
value-clamp), not a breaking wire-format change (the wire bytes for a
u64 field are unchanged; only the DECODED Rust/TS-side representation for
previously-corrupted values is now correct).

## Out of scope

- Do NOT add a `FilterValue::Big` wire variant (see fix site 5's
  rationale — reuse the existing String/Big comparison bridge instead).
- Do NOT touch `record_view/lens.rs`'s `uint_to_record_value` (already
  correct, different but equally lossless representation, deliberate
  design).
- Do NOT touch `From<u32>`/`From<usize>` conversions (never overflow
  `i64` in practice).
- Do NOT touch `eval_bytes.rs`'s `0xd3` int64 bit-pattern decode (false
  positive, not a bug).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @types --full` green.
- `./scripts/test.sh -p shamir-query-types --full` green.
- `./scripts/test.sh @engine --full` green.
- Any TS unit/e2e tests you touched/added pass (`npm test` in
  `crates/shamir-client-ts`, per this repo's established TS test
  convention — check `package.json` scripts).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above, plus the outcome of
  fix site 5's mandatory empirical filter-match verification (worked as
  expected, or a precise failure report if not).
