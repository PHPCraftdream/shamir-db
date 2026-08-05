# Brief — #983 fix (round 2): `FilterValue::Binary` misclassified as `String` on real wire input

Task: #983. **This is a follow-up to the just-landed fix** (compare_values/
scalar_ref_cmp/scalar_ref_cmp_qv Bin arms — already committed). That fix is
CORRECT and necessary but **not sufficient**: personally re-verifying the
prior fix end-to-end (fresh `cargo build --release -p shamir-server`, fresh
napi `.node` rebuild, real JS e2e run against both) still showed
`filter.eq('blob', filter.bin([1,2,3]))` returning zero rows against a real
server. This brief pins the SECOND, independent root cause found while
chasing that down. Read in full before touching anything.

## Confirmed root cause — empirically verified twice

`crates/shamir-query-types/src/filter/filter_value.rs`'s `FilterValue` enum:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(#[serde(with = "serde_bytes")] Vec<u8>),
    Array(Vec<FilterValue>),
    // ... FieldRef, QueryRef, FnCall, Expr, Cond, Param
}
```

`#[serde(untagged)]` tries each variant **in declaration order** and takes
the first that successfully deserializes. `String`'s `Deserialize` impl (the
one from the standard library, used here) is **lenient toward raw bytes**:
given a byte buffer that happens to be valid UTF-8, it succeeds via
`String::from_utf8` and produces a `String`, rather than erroring. Since
`String` is declared **before** `Binary`, ANY binary payload whose bytes
happen to form valid UTF-8 (e.g. `[1, 2, 3]` — every byte ≤ 0x7F, so
trivially valid UTF-8) gets silently captured by the `String` arm and NEVER
reaches `Binary` at all.

This is separate from (and independent of) the tree-eval comparison gap
just fixed — that fix corrects a real defect (`Bin` vs `Bin` comparison
returning `None`), but it can't help here because the VALUE never becomes
`FilterValue::Binary` in the first place; it becomes
`FilterValue::String("\u{1}\u{2}\u{3}")`, which then resolves to
`QueryValue::Str(...)` — a completely different type family that correctly
(and separately, per the existing `mismatched_type_families_none` tests)
returns `None` when compared against a record's `InnerValue::Bin` field.

### Empirical proof (reproduce exactly, do not skip this step)

1. Real JS client (`@msgpack/msgpack`, via `tests/e2e`'s
   `shamir-client-node` wrapper) encodes
   `Query.from('t').where(filter.eq('blob', filter.bin([1,2,3]))).build()`
   wrapped in a batch, producing (captured this session, verified byte-by-byte
   against the msgpack spec):
   ```
   ...a56669656c6491a4626c6f62a576616c7565c403010203
   ```
   The `value` field is unambiguously **`c4 03 01 02 03`** — a genuine
   msgpack **bin8** marker, length 3, payload `[1,2,3]`. The JS encode side
   is fully correct — do not re-investigate it.
2. Decoding just the `where` clause bytes
   (`83a26f70a26571a56669656c6491a4626c6f62a576616c7565c403010203`) as
   `Filter` via `rmp_serde::from_slice` currently produces:
   ```
   Eq { field: ["blob"], value: String("\u{1}\u{2}\u{3}") }
   ```
   — confirmed via a temporary scratch test this session (since removed;
   reproduce it yourself to confirm before fixing, using
   `rmp_serde::from_slice::<Filter>(&bytes)` on those exact bytes — build
   the byte vec from the hex string above, or capture fresh bytes yourself
   via a small `node -e` script using `@msgpack/msgpack`'s `encode()` on the
   query-builder output, whichever is faster).
3. **Confirmed the fix**: temporarily reordering a **local replica** enum
   (NOT the real `FilterValue` — verified via a disposable scratch type to
   avoid touching real source before writing this brief) so `Binary` is
   declared **before** `String` makes the exact same wire bytes decode to
   `Binary([1,2,3])` correctly. This is the fix.

## Why this exact symptom, and why it's data-dependent (important context)

This bug is **silent and payload-dependent** — it only manifests when the
binary payload's bytes happen to form valid UTF-8. A payload like
`[0xDE, 0xAD, 0xBE, 0xEF]` (invalid UTF-8) would correctly fall through
`String` and land on `Binary` even with the CURRENT buggy order — which is
almost certainly why earlier ad-hoc-style checks did not catch this widely:
whoever originally tested binary filter values likely used non-ASCII/
non-UTF8 test bytes and never hit the failure. Small, ASCII-range test
payloads (`[1,2,3]`, or anything resembling printable text/short integers)
are exactly the common case that trips this — which is presumably also
what the ORIGINAL #983 bug reporter used.

## Required fix

### 1. Reorder `FilterValue`'s variants

In `crates/shamir-query-types/src/filter/filter_value.rs`, move `Binary`
to be declared **before** `String`:

```rust
pub enum FilterValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Binary(#[serde(with = "serde_bytes")] Vec<u8>),
    String(String),
    Array(Vec<FilterValue>),
    // ... unchanged
}
```

Verify this is sufficient by re-running the exact reproduction from the
"Empirical proof" section above against the REAL `FilterValue` type (not a
replica) after the reorder — decode the captured real-wire bytes and
confirm `Binary([1,2,3])`.

**Investigate before assuming this is the complete fix**: check whether a
GENUINE string payload that happens to be encoded via `serde_bytes`-style
bytes anywhere in the wire protocol could now be misclassified as `Binary`
in the OTHER direction. This should NOT happen — a real JS string always
encodes as msgpack `str8`/`fixstr`/`str16`/`str32` (a distinct msgpack type
from `bin8`/`16`/`32`), and `serde_bytes`'s `Deserialize` for `Vec<u8>`
does NOT accept `visit_str`/`visit_string` (unlike the reverse — the
standard `String`'s `Deserialize` DOES accept `visit_bytes`/`visit_byte_buf`
as a fallback, which is the actual cause of the ambiguity). Confirm this
asymmetry holds (a real string literal filter value still decodes as
`FilterValue::String` correctly after the reorder) with a test — do not
just assume it.

### 2. Check `Array` too — same class of question

`Array(Vec<FilterValue>)` comes right after these two. Msgpack arrays and
raw bytes are structurally distinct wire types (array markers vs bin8/16/32
markers), so an `Array` vs `Binary` collision is much less likely than
`String` vs `Binary` — but VERIFY this rather than assuming, since this
whole bug class is exactly "assumed distinct, wasn't." A quick test asserting
a real `[1,2,3]` JS ARRAY (not `Uint8Array`) still decodes as
`FilterValue::Array([Int(1),Int(2),Int(3)])` (not `Binary`) after the
reorder is sufficient due-diligence.

### 3. Search for the SAME pattern elsewhere in this codebase

`FilterValue` is not the only `#[serde(untagged)]` enum with adjacent
`String`/`Binary`-shaped variants in this workspace. Grep for
`#[serde(untagged)]` across `crates/*/src` and check every hit for a
`String`-before-`Binary` (or equivalent bytes-newtype) ordering hazard.
Report every additional site found, whether or not you fix it in this pass
(if you find MORE than 1-2 additional sites, or any site outside
`shamir-query-types`, treat that as a signal to STOP and report rather than
silently fixing an unbounded set — flag it for a follow-up task instead;
use judgement, but do not let this pass balloon into a repo-wide sweep
without saying so first).

## Required tests

Extend `crates/shamir-query-types/src/filter/tests/` (this repo's test-
organisation convention: `tests/mod.rs` is re-exports only; add to
`filter_enum_tests.rs` or `filter_value_conv_tests.rs`, whichever fits
better — check both first) with:

1. **The exact real-wire-bytes regression** — decode the captured hex bytes
   from the "Empirical proof" section (or freshly re-capture them via a
   `node -e` one-liner using this repo's actual TS query builder + a real
   `@msgpack/msgpack` encode, to avoid hand-transcription risk) as `Filter`
   and assert `Eq { field, value: FilterValue::Binary(vec![1,2,3]), .. }`
   — this is the test that FAILS without your fix and PASSES with it; state
   explicitly that you verified the fail→fix cycle.
2. A round-trip test for a `Filter::Eq` with a `Binary` value whose bytes
   are ALL valid-UTF8 range (like `[1,2,3]`) AND one with invalid-UTF8 bytes
   (like `[0xDE, 0xAD, 0xBE, 0xEF]`) — both must decode as `Binary`, proving
   the fix isn't accidentally UTF8-payload-specific.
3. A `Filter::Eq` with a real `String` filter value — must still decode as
   `FilterValue::String`, proving no direction-reversal regression.
4. Whatever `Array`-vs-`Binary` test point 2 above concluded was needed.
5. **An end-to-end JS e2e test** — the Rust-level tests above prove the wire
   deserialization is fixed, but this bug was ONLY caught by testing against
   a REAL running server with a REAL client. Extend
   `tests/e2e/tests/05-filters.test.js` — the `#983 eq matches identical
   bytes (tree-eval Bin arm)` and `#983 ne excludes identical bytes` tests
   ALREADY EXIST there (from the prior fix round) and currently FAIL against
   a real server (confirmed this session, both with and without the prior
   fix, and with fully fresh `shamir-server.exe` + `.node` binding rebuilds)
   — after YOUR fix, they must PASS. Do not add new e2e tests for this;
   make the EXISTING ones in that file pass.

## Rebuild discipline for verifying the e2e tests — READ CAREFULLY

This repo's `tests/e2e` uses TWO separately-built artifacts that do **not**
rebuild automatically:
- `target/release/shamir-server.exe` — via `cargo build --release
  -p shamir-server` **but this repo has a GLOBAL `CARGO_TARGET_DIR` env var
  set** (verify with `echo $CARGO_TARGET_DIR` yourself — this session found
  it pointed at `D:\dev\rust\.cargo-target`, NOT the repo's own `target/`).
  If that's still the case for you, a plain `cargo build --release
  -p shamir-server` will NOT update `target/release/shamir-server.exe` (the
  path `tests/e2e/helpers/server.js` hardcodes) — it updates
  `$CARGO_TARGET_DIR/release/shamir-server.exe` instead. You MUST either:
  (a) copy the freshly built binary from `$CARGO_TARGET_DIR/release/` into
  the repo's `target/release/`, or (b) run the build with
  `CARGO_TARGET_DIR=target cargo build --release -p shamir-server`
  (repo-relative) so it lands in the expected place directly. Verify
  `target/release/shamir-server.exe`'s mtime is fresh (after your edits)
  before trusting an e2e run.
- The napi native client — `crates/shamir-client-node/shamir-client.win32-
  x64-msvc.node` (or the equivalent for your platform) — is a SEPARATE
  build, via `cd crates/shamir-client-node && npx napi build --platform
  --release`. `shamir-client-node` is excluded from the default workspace
  (see this repo's `Cargo.toml` — MSVC-only, built separately) so a
  workspace-level cargo build never touches it. It does NOT need rebuilding
  for THIS specific fix (the fix lives in `shamir-query-types`, and the
  client binding statically links that crate too — check whether your fix
  actually requires it; if in doubt, rebuild it anyway, it only takes a few
  minutes and this session's investigation shows skipping it produces a
  false "still broken" signal that costs much more time to debug than the
  rebuild itself).

Do not report the e2e gate green without having personally confirmed BOTH
artifacts' mtimes are newer than your source edits.

## Scope discipline

- Do NOT touch the `compare_values`/`scalar_ref_cmp`/`scalar_ref_cmp_qv` fix
  already landed — it is correct and orthogonal, do not revert or alter it.
- Do NOT redesign `FilterValue` into a tagged enum or add a custom
  `Deserialize` impl — the task's own investigation (this brief) confirms a
  plain variant reorder is sufficient and minimal; do not over-engineer.
- If your search in step 3 ("search for the same pattern elsewhere") finds
  more than a couple of extra sites, or anything outside
  `shamir-query-types`, STOP, do not fix them all silently — report findings
  and let the orchestrator decide scope for a follow-up.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-types -p shamir-engine -p shamir-types -p shamir-db --full
```

Then, personally, with fresh binaries per the rebuild discipline above:

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
commands. (Scratch/temporary files you create for your OWN investigation
should simply be deleted with a plain file-delete before finishing, not
`git rm`/`git checkout` — those files won't be tracked yet anyway if you
never staged them.)

## What to report back

- Confirm the exact real-wire-bytes test FAILS before your fix and PASSES
  after — paste both test-runner outcomes.
- The full list of `#[serde(untagged)]` enums you found elsewhere in the
  workspace, and which (if any) you fixed vs. flagged for follow-up.
- The diff for `filter_value.rs` (variant reorder) + all new tests.
- Full JS e2e `npm test` output showing the previously-failing `#983 eq
  matches identical bytes` / `#983 ne excludes identical bytes` tests now
  PASS, with explicit confirmation of both binaries' fresh mtimes.
- Exact gate command output.
