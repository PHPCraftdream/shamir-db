# shamir-funclib -- API & wire-protocol design

## Summary

The public interface is coherent: one `ScalarRegistry`/`ScalarResolver` dispatch table over
`fn(&[QueryValue]) -> ScalarResult`, folder-qualified wire names (`math/abs`), a code-only
machine-error contract, and explicit purity/trust metadata (`FnEntry`) that the engine's
functional-index gate consumes. The bespoke canonical-hash serializer is unusually well
tested for order-independence and msgpack round-trip invariance, but its documented
key-ordering contract is factually wrong (ordering is by msgpack-*encoded* key bytes, not
UTF-8 names) and the byte stream carries no version tag, so any codec evolution silently
invalidates stored CAS hashes. Builder-only query-construction rule: **compliant** — the
crate constructs no queries; all `serde_json`/`rmp-serde` use is value-codec work
(`to_json`/`parse_json` scalars, canonical key bytes), and the bench uses
`bench_scale_tool::Harness` per convention.

## Findings

### 1. Query-reachable scalars allocate unbounded memory (`random_bytes`, `repeat`, `pad`)
**File:line:** `crates/shamir-funclib/src/gen.rs:70-77`; `crates/shamir-funclib/src/strings.rs:228-242` and `389-411`
**Severity:** high

**Issue:** `crypto.rs` was hardened per audit §2b with per-call caps and an aggregate
concurrency gate for `argon2id` (`crypto.rs:48-76`), but the identical exposure class was
left unbounded in two other categories that are equally query/WASM-guest-reachable:
- `gen/random_bytes(n)` rejects only `n < 0`, then does `vec![0u8; n as usize]` with `n` up to `i64::MAX`.
- `strings/repeat(s, n)` does `s.repeat(n)` with `n` up to `i64::MAX`.
- `strings/pad_*(_ , len, _)` allocates `target - cur` fill chars for any `len ≥ 0`.

All three are registered into `register_builtins` (`lib.rs:53,60`) and dispatched from
filters (`shamir-engine/src/query/filter/resolve.rs:368`), schema field rules
(`field_rule.rs:404`), and WASM guests (`shamir-wasm-host::builtin_scalars`).

**Failure scenario:** a single low-privileged query such as
`gen/random_bytes(9223372036854775807)` (or `strings/repeat('a', 10^18)`) attempts a
~10^18-byte allocation; Rust allocation failure aborts the process, killing every
connection. The argon2id hardening explicitly exists to prevent this class; these three
functions bypass it.

**Suggested fix:** cap each parameter (e.g. `random_bytes` ≤ 1 MiB, `repeat`/`pad` output
≤ ~2^20 chars) and return `"out_of_range"` beyond the cap, mirroring the `A2_MAX_*`
pattern; add cap-boundary tests next to `gen/tests/gen_tests.rs` and
`tests/strings_tests.rs`.

### 2. `canonical_hash` key ordering is msgpack-dependent but documented as name order; byte format has no version tag
**File:line:** `crates/shamir-funclib/src/canonical.rs:26-37` (module doc), `180-188` (`serialise_key` comment), `161` (sort), `199-219` (public API)
**Severity:** medium

**Issue:** The module doc says for string keys the sorted bytes "are the UTF-8 key name",
and the inline comment claims "string keys order exactly as their names do". False:
`rmp_serde::to_vec(key)` prepends a length tag (fixstr `0xa0|len` for len < 32; `0xd9`
str8 above), so ordering is (length-class, length, bytes) — e.g. `"b"` encodes to
`[0xa1,'b']` and sorts *before* `"aa"` (`[0xa2,'a','a']`) although `"aa" < "b"` by name.
The hash stays deterministic and insertion-order independent (well covered by
`canonical/tests`), but (a) the documented contract is wrong, and (b) any cross-language
reimplementation of the CAS hash must replicate rmp-serde's exact encoding, which is
specified nowhere. Additionally `canonical_bytes`/`canonical_hash` emit no magic/version
prefix, and the whole format is implicitly coupled to codec behavior (Dec/Big hashed as
`T_STR` because `Serialize` emits `to_string()`, `canonical.rs:52-58,102-116`; verified
true today in `shamir-types/src/types/value.rs:72-73`).

**Failure scenario:** a non-Rust client (or a future codec change — e.g. reactivating the
reserved `0x04/0x05` tags or changing Dec serialization) computes hashes by name-sorting
or with changed tags; stored `_prev_hash` chains then mismatch for logically identical
records, and there is no version field to detect or migrate the format change.

**Suggested fix:** (a) sort string keys by raw UTF-8 bytes (matching the documented
contract), keeping the msgpack encoding only for non-string keys — note this changes
hash outputs, so pair it with (b); (b) prefix the canonical byte stream with a 1-byte
format version and document the encoding as frozen.

### 3. Machine error-code vocabulary is free-form and inconsistent across categories
**File:line:** `crates/shamir-funclib/src/registry.rs:17-29` (code-only `ScalarError`); `strings.rs:427` (`"bad_regex"`) vs `validate.rs:342` (`"bad_pattern"`); `registry.rs:212-222` (`"out_of_range"`) vs `cast.rs:111-131` (`"cast_failed"`) for the identical fractional/overflow-to-int rejection; `datetime.rs:54,122,155` (one code `"parse"` covering malformed input, invalid pattern, and non-matching input)
**Severity:** medium

**Issue:** The frontend localises by code (`registry.rs:8-9`), making the code set part of
the wire contract, yet it exists only as scattered string literals with no single
catalogue. Sibling functions implementing the same condition emit different codes (the
`strings` regex family vs `validate/matches` compiling the same kind of user-supplied
pattern; `cast/to_int` vs the shared `arg_i64` extractor), and `parse` is overloaded for
three distinct failure kinds.

**Failure scenario:** a client that localises `"bad_regex"` shows a raw, unlocalised code
for `validate/matches` failures; telemetry/UX keyed to one code silently misses its twin.

**Suggested fix:** declare the code set as `pub const`s (or an enum with `as_str`) in
`registry.rs`, deduplicate the twin codes, and add a registry-level test asserting every
`ScalarError::new(...)` literal in the crate is a known code.

### 4. Module docs advertise plain unqualified names; the wire protocol dispatches folder-qualified names
**File:line:** `arrays.rs:1-6`, `cast.rs:3`, `crypto.rs:3-4`, `datetime.rs:4-8`, `encode.rs:3-6`, `math.rs:4-6`, `object.rs:3-4`, `strings.rs:6-10`, `text.rs:6-8`, `validate.rs:3-5`, `value_nav.rs:4-5` (all say "plain names, no folder prefix"); `lib.rs:12-13` (stale "remaining categories are stubs"); correct in `gen.rs:3-4` and `null.rs:3-4`
**Severity:** medium

**Issue:** `register_builtins` (`lib.rs:49-66`) folder-qualifies every category, and the
production wire contract uses `math/abs`-style names (confirmed in
`shamir-query-types/src/read/select.rs:102`, the TS client builders, and
`docs/guide-docs/guide/05-functions.md`). Eleven of thirteen category headers still claim
plain names — an embedder reading `math.rs` would call `"abs"` and get
`"unknown_function"`. Additionally, the per-category behavioural suites register modules
*without* a folder and assert plain names (`math/tests/registry_tests.rs:21-25`,
`tests/encode_tests.rs:6-10`, etc.), i.e. they exercise names that do not exist in the
production registry; only `tests/register_builtins_tests.rs` (one sample per category) and
the gen/null/canonical wiring tests cover the qualified spellings.

**Failure scenario:** docs steer embedders into dead names; a future regression in
`register`/`in_folder` prefixing for one category would not be caught by that category's
behavioural suite.

**Suggested fix:** update the 11 headers to the folder-qualified names, delete the stale
"stubs" sentence in `lib.rs`, and build each category's test registry via
`in_folder("<cat>", <mod>::register)` so behavioural tests cover the production spelling.

### 5. Same public name `get_path` in two folders with opposite miss semantics
**File:line:** `crates/shamir-funclib/src/object.rs:94-121` vs `crates/shamir-funclib/src/value_nav.rs:26-37,98-140`
**Severity:** low

**Issue:** `object/get_path` errors `"missing_key"` on any miss and accepts only `Str`
steps; `value_nav/get_path` returns `Null` on any miss, accepts `Int`/`Str` steps (with
negative indexing), and errors `"type_mismatch"` only on a malformed step. `lib.rs:5-8`
documents the folder mechanism as a collision fix but not that the colliding names are
semantically different functions.

**Failure scenario:** a query author picks the wrong namesake; the miss surfaces as a
swallowed `"missing_key"` (the engine's `.ok()` silent-miss path at
`resolve.rs:368`) instead of the expected `Null`, or vice versa — filters silently
misbehave.

**Suggested fix:** align the miss semantics (both Null or both error), or rename one;
document the divergence at both definition sites regardless.

### 6. `trusted_pure` gate: pub fields make the "explicit opt-in" convention-only; docs claim indexability the gate forbids
**File:line:** `crates/shamir-funclib/src/registry.rs:54-65,94-104`; `arrays.rs:28`; `cast.rs:18-19`; consumer check at `shamir-engine/src/table/table_manager_index_mgmt.rs:250-262`
**Severity:** low

**Issue:** All `FnEntry` fields — including `trusted_pure` — are `pub`, so the documented
"set via `.trusted_pure()`" vouch workflow is bypassable by struct literal; the gate's
enforcement is only that `register_builtins` never vouches. Separately, the `arrays.rs` /
`cast.rs` headers say their functions "(indexable)" / "may back a functional index", but
every built-in is registered via `FnEntry::pure` (`trusted_pure = false`) and the engine
rejects non-vouched entries — `is_indexable()` is false for the entire built-in library,
and functional indexes are documented (engine side) as user-scalar-only.

**Failure scenario:** an embedder follows the module doc, tries to back an index with
`cast/to_int`, and gets a rejection whose message ("Call .trusted_pure() …") contradicts
the doc they just read.

**Suggested fix:** make the metadata fields private with the builder as sole setter (or at
least `trusted_pure`), and reword the two headers to "pure + deterministic;
functional-index use requires an explicit `.trusted_pure()` vouch".

### 7. `f64 → i64` extraction accepts values above `i64::MAX` due to a float-rounded bound
**File:line:** `crates/shamir-funclib/src/registry.rs:217-223` (`arg_i64`); same logic duplicated in `cast.rs:120-126`
**Severity:** low

**Issue:** The guard `*f <= i64::MAX as f64` compares against `2^63` (the nearest f64 to
`i64::MAX`), so `f == 2^63` passes; the saturating `*f as i64` then silently yields
`i64::MAX`.

**Failure scenario:** `cast/to_int(9223372036854775808.0)` returns
`Int(9223372036854775807)` instead of `"cast_failed"` — a silent wrong-value conversion on
a public conversion API (and on every category that funnels through `arg_i64`).

**Suggested fix:** bound with `*f < 9_223_372_036_854_775_808.0`, or route through the
exact `Decimal` path already used for `Dec`.

### 8. `ScalarError` has no structured detail slot
**File:line:** `crates/shamir-funclib/src/registry.rs:17-29`
**Severity:** nit

**Issue:** The code-only design is deliberate (localisation by code), but errors cannot
carry *machine-safe* detail — which argument index failed, expected type, `[min,max]`
arity — so clients cannot distinguish "arg 2 was bad" from "arg 0 was bad". Detail is not
human text and would not violate the stated no-human-text contract.

**Suggested fix:** add `pub detail: Option<ScalarErrorDetail>` (enum: `Arity { min, max }`,
`ArgType { index, expected }`, …) while keeping `code` and `Display` unchanged.

### 9. `ScalarRegistry::register` collision policy undocumented
**File:line:** `crates/shamir-funclib/src/registry.rs:126-136` vs `agg.rs:72-75`
**Severity:** nit

**Issue:** `AggRegistry::register` documents "last-wins on collision";
`ScalarRegistry::register` inserts silently with no stated policy. Given this registry
just migrated away from plain-name collisions (#118), the overwrite policy should be
explicit.

**Suggested fix:** one doc line ("duplicate names: last-wins") plus a debug log on
overwrite.

---

Test-coverage verdict (skimmed): every module has a `tests/` directory per the repo layout
convention and coverage of the *documented conventions* is strong — the agg empty-input
table has ~60 tests (`agg/tests/agg_tests.rs`), canonical hashing covers key-order
independence, top-level-only `_prev_hash` exclusion, and Dec/Big msgpack round-trip hash
invariance (`canonical/tests/canonical_tests.rs:25-241`), and argon2id has known-answer
tests plus a concurrency-cap regression (`crypto/tests/crypto_tests.rs:207-269`). Gaps: no
bound test for `random_bytes`/`repeat`/`pad` (finding 1), and the plain-name registration
in per-category suites leaves the production name spelling only sample-covered (finding 4).
