# Brief — #983 fix: `Bin` scalars are never comparable in the tree-eval filter path

Task: #983. **Root cause has already been found and confirmed** by the
orchestrating session — this brief hands you a precise, narrow fix, not a
fresh investigation. A previous `crush` run bisected the Rust-side codec
layers (all clean — see the test files already on disk, described below) and
left one genuinely failing test that pins the real bug. Read this brief in
full before touching anything.

## Confirmed root cause

`crates/shamir-engine/src/query/filter/resolve.rs`'s `compare_values<K>`
(~line 141), the canonical `Value<K>` vs `Value<K>` comparator used by the
WHOLE filter-evaluation stack, has **no `(Value::Bin(a), Value::Bin(b))`
arm at all**. It has a meticulous cross-type arm for every OTHER pair
(Int/F64/Dec/Big/Str all get exact or documented-approximate handling) but
Bin falls through to `_ => None` unconditionally — even `Bin == Bin` with
identical bytes.

Two downstream functions **explicitly claim to mirror `compare_values`
arm-for-arm** and both reproduce the same gap:

- `crates/shamir-types/src/record_view/scalar_ref.rs::scalar_ref_cmp`
  (~line 150) — its own doc comment says: *"Returns `None` for
  non-comparable pairs (mismatched type families that have no numeric
  bridge, containers, `Bin`)"* — i.e. the gap was **documented as
  intentional**, but it directly contradicts the type's own `PartialEq`
  impl 90 lines above it (`ScalarRef::eq`, line 64:
  `(ScalarRef::Bin(a), ScalarRef::Bin(b)) => a == b` — Bin **is** comparable
  there). This is the same "code contradicts its own doc comment" class of
  bug as #960 (P0-4) from the earlier release-blocker chain — the comment
  rationalizes an oversight rather than documenting a real design decision.
- `crates/shamir-types/src/record_view/scalar_ref.rs::scalar_ref_cmp_qv`
  (~line 180) — the `QueryValue` twin, same gap, same missing arm.

There is even an existing test that **locks in the bug as if it were
correct**:
`crates/shamir-types/src/record_view/tests/scalar_ref_cmp_tests.rs` ~line
222-229:

```rust
#[test]
fn bin_vs_anything_returns_none() {
    // Bin is a ScalarRef variant but compare_values has no Bin arm → None.
    assert_eq!(
        scalar_ref_cmp(ScalarRef::Bin(&[1, 2, 3]), &InnerValue::Bin(vec![1, 2, 3])),
        None,
    );
}
```

This test currently PASSES — proving the gap is real and current, not
already fixed.

## Why this is #983's "filter matches zero rows" symptom, confirmed empirically

`crates/shamir-engine/src/query/filter/filter_node.rs` (~line 494) computes
every `CompareOp` (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`) via
`scalar_ref_cmp_qv(a, b) == Some(Ordering::Equal)` (or the `Greater`/`Less`
equivalents) with **no special-case for `Bin` before that call**. Since
`scalar_ref_cmp_qv` always returns `None` for any `Bin` pair, **every
Eq/Ne/Gt/Gte/Lt/Lte filter comparison against a binary field returns `false`
via this evaluation path**, regardless of whether the bytes are actually
equal.

This has ALREADY been reproduced on disk, right now, without your help:
run

```
./scripts/test.sh -p shamir-engine -- test_eq_bin
```

and observe `test_eq_bin_match` FAIL with:
`assertion left == right failed: bytes-eval (true) disagrees with normal
eval (false) for Eq { field: ["bin"], value: Binary([1, 2, 3]) }` — the
bytes-fast-path (`matches_msgpack_bytes`, a SEPARATE evaluator over raw
msgpack bytes that already handles `Bin` correctly — see
`crates/shamir-engine/src/query/filter/eval_bytes.rs` ~487,
`RawScalar::Bin` vs `FilterValue::Binary`) says `true`; the tree-eval path
(`compiled.matches(...)`, which is what runs whenever a filter is NOT
served by the bytes fast-path — e.g. any in-memory/decoded-record
evaluation) says `false`. This exact contradiction is the root of the
original bug report's "filter.eq('blob', filter.bin(payload)) matches ZERO
rows" symptom.

## What is ALREADY on disk — verify, do not redo

A previous session (this orchestrating agent, directly, not delegated)
already:
1. Added `crates/shamir-types/src/codecs/interned/tests/bin_roundtrip_tests.rs`
   (wired into `tests/mod.rs`) — 6 tests pinning that the storage
   encode/decode layers (serde round-trip, tree path, wire path, lens path)
   all correctly preserve `Bin`. **All 6 currently PASS.** This proves the
   corruption is NOT in the codec/storage layer — do not re-investigate it.
2. Added `test_eq_bin_match` / `test_eq_bin_no_match` to
   `crates/shamir-engine/src/query/filter/tests/eval_bytes_tests.rs`.
   `test_eq_bin_no_match` PASSES; `test_eq_bin_match` currently **FAILS**
   (see above) — this is your target regression test, already written,
   TDD-red.

Run `git status --short` and `git diff` yourself first to see exactly what
is already staged uncommitted before you start — these files are real,
already on disk, not hypothetical.

## Required fix

### 1. `compare_values` (`crates/shamir-engine/src/query/filter/resolve.rs`)

Add, right before the final `_ => None` arm:

```rust
(Value::Bin(a), Value::Bin(b)) => Some(a.cmp(b)),
```

`Vec<u8>: Ord` gives lexicographic byte-wise ordering — this supports
`Eq`/`Ne` (byte equality) and also makes `Gt`/`Lt`/`Gte`/`Lte` against a
binary field well-defined (lexicographic), consistent with how `Str` is
already ordered in the same function. Do not special-case Eq-only; use the
same `Some(a.cmp(b))` shape the `Str`/`Dec`/`Big` arms already use.

### 2. `scalar_ref_cmp` and `scalar_ref_cmp_qv`
(`crates/shamir-types/src/record_view/scalar_ref.rs`)

Add the matching arm to BOTH functions (mirroring #1 exactly, per their own
"mirrors `compare_values` arm-for-arm" doc contract):

```rust
(ScalarRef::Bin(a), InnerValue::Bin(b)) => Some(a.cmp(b.as_slice())),
```
```rust
(ScalarRef::Bin(a), QueryValue::Bin(b)) => Some(a.cmp(b.as_slice())),
```

### 3. Fix the misleading doc comments

- `scalar_ref_cmp`'s doc comment (~line 141): remove `, Bin` from *"Returns
  `None` for non-comparable pairs (mismatched type families that have no
  numeric bridge, containers, `Bin`)"* — Bin IS now comparable; only
  containers (Map/List/Set) and genuinely mismatched families remain
  `None`.
- `ScalarRef`'s top doc comment (~line 24-27) and any other place in this
  file/module that states or implies Bin is excluded from comparison —
  search for "Bin" in `scalar_ref.rs`'s comments and correct every stale
  claim, not just the one quoted above.

### 4. Fix the test that locks in the bug

`crates/shamir-types/src/record_view/tests/scalar_ref_cmp_tests.rs`
~line 222-229: `bin_vs_anything_returns_none` currently asserts the BUG.
Replace it with tests asserting the CORRECT behaviour — mirror the file's
own existing style (see `int_int_equal`, the Str tests, etc. earlier in the
file) with at least:
- `bin_bin_equal` — identical bytes → `Some(Ordering::Equal)`.
- `bin_bin_not_equal` — different bytes → `Some(Ordering::Less)` or
  `Some(Ordering::Greater)` per actual lexicographic order (pick a concrete
  example and assert the concrete direction, don't just assert `!=
  Some(Equal)`).
- Keep (or add fresh) a **genuinely** non-comparable Bin case if one exists
  (e.g. `ScalarRef::Bin` vs `InnerValue::Str` — mismatched families still
  return `None`) so the "containers/mismatched families → None" contract
  still has coverage.

Also add the `QueryValue` counterpart cases somewhere sensible — either
extend this same file with a `_qv` suffix section, or create a sibling
`scalar_ref_cmp_qv_tests.rs` (check `tests/mod.rs` in this directory first;
wire in whichever you add, following this repo's test-organisation
convention: `tests/mod.rs` is re-exports only).

## Investigate the SEPARATE "Uint8Array becomes plain object" claim

The original #983 report described TWO symptoms:
1. `filter.eq('blob', filter.bin(payload))` matches zero rows — **this is
   the bug you are fixing above.**
2. Reading the row back shows `blob` as `{"0":0,"1":1,...}` instead of a
   `Uint8Array`.

**Do not assume #2 is a real bug without checking.** `JSON.stringify()` of a
REAL `Uint8Array` in JavaScript produces EXACTLY this shape
(`{"0":0,"1":1,...}`) — Uint8Array has no custom `toJSON` and
`JSON.stringify` walks its numeric indices as plain enumerable properties.
It is entirely possible the original bug report's author inspected the
value via `JSON.stringify`/`console.log` and mistook completely correct
behaviour for corruption.

Write a focused test — in
`crates/shamir-client-ts/src/__tests__/` (vitest) or `tests/e2e/tests/`
(pick whichever this repo's convention favours for a client round-trip
check; look at `tests/e2e/tests/02-basic-crud.test.js` for the existing
style) — that inserts a `Uint8Array` field and asserts on the READ result
using `instanceof Uint8Array` (or `Buffer.isBuffer` — check what the actual
client returns, Node's msgpack decode may hand back a `Buffer`, which
IS a `Uint8Array` subclass) — **not** a JSON/string comparison. Report
which of these is true:
- The client already returns a real `Uint8Array`/`Buffer` and symptom #2 was
  a `JSON.stringify` misdiagnosis in the original report — say so plainly,
  do not invent a client-side fix that isn't needed.
- The client genuinely returns a plain object (fails `instanceof
  Uint8Array`) — in that case, find and fix the actual client-side
  conversion bug, and say exactly where it was.

## Scope discipline

- Do NOT touch the codec/storage layer files verified clean by the existing
  `bin_roundtrip_tests.rs` (layer 1/2) — the bug is confirmed to be in the
  comparison functions listed above, nowhere else in Rust.
- Do NOT add Bin support to `set_contains_coercing` or any `InSet`/Set-based
  path beyond what's already there (line 99 of `filter_node.rs` already
  handles `ScalarRef::Bin` for set membership correctly — that path is NOT
  broken, only the plain `Eq`/`Ne`/`Gt`/etc. `Compare` path is).
- Do NOT change `eval_bytes.rs` / `matches_msgpack_bytes` — it already
  handles Bin correctly and is out of scope.
- If the TS investigation finds symptom #2 is real, fix ONLY the minimal
  actual defect — do not refactor the client's decode pipeline.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-types -p shamir-engine -p shamir-db --full
```

If you touch the TS client:

```
cd crates/shamir-client-ts && npm run build && npx vitest run
```

If you add/confirm a JS e2e test:

```
cd tests/e2e && npm test
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the test
run, then commits. Only edit/create files and run read-only / test / gate
commands.

## What to report back

- Confirm `test_eq_bin_match` now PASSES (paste the exact test-runner
  line).
- The diff for all three comparison functions + the doc-comment fixes.
- The rewritten `scalar_ref_cmp_tests.rs` section (before/after for the
  flipped test).
- Your finding on the "Uint8Array becomes plain object" question — real bug
  or misdiagnosis — with the exact test that proves it either way.
- Exact gate command output.
