# 260 — audit-fix-filter-value-decoder (P1 audit follow-up: H5 / RV-QUERY-TYPES-FF6DD2A7)

Source: post-merge audit of PR #323 + design decision by @oh (2026-10-29, binding). Crate: `shamir-query-types` only. Read first: `CLAUDE.md`, `crates/shamir-query-types/src/read/filter/filter_value.rs` (untagged derive ~47-80, RU7 notes ~15,45), its tests `read/filter/tests/filter_value_conv_tests.rs` (~17-75), and the producers: `crates/shamir-query-builder` (FilterValue builders) and `crates/shamir-client-ts/src/builders/read/filter.ts` (~367-409: `param`, `queryRef`, `ref`, `fn`, `expr`).

## Defect
`FilterValue` derives `#[serde(untagged)]`: trial decoding lets map variants fall back into each other. Observed: `{"$ref":42,"$query":"q"}` decodes as a query ref and `{"$query":"q","$ref":["x"]}` decodes as a field ref. No producer emits such maps (the engine `is_marker_map` in `shamir-engine/src/query/batch/runner/param_subst.rs:112-117`, the planner `ops/batch/plan/planner.rs:451-456` and `shamir-query-builder/src/batch/batch_refs.rs:336-349` all treat a marker as exactly one reserved key, or `$query` plus `path`), so the fallback is not part of the wire contract and must become an error.

## Change
Replace the untagged derive with a hand-written `Deserialize` (a visitor over the serde data model as produced by rmp_serde AND serde_json: nil/bool/i64/u64/f64/str/bytes/seq/map) with deterministic dispatch:
- Non-map cases keep their CURRENT result exactly, including the RU7 rule: a u64 above `i64::MAX` stays exact (read the current code and tests first and preserve the representation), never a lossy float.
- Map form: exactly ONE `$`-prefixed discriminator key (take the exact set from the current variants: `$param`, `$ref`, `$query`, `$fn`, `$expr`, ...), plus `path` ONLY together with `$query`, in ANY key order; `path: nil` allowed (the Rust serializer omits it when unset). Zero discriminators, two or more discriminators, a discriminator with an invalid value, `path` without `$query`, or a stray extra key => a typed, short error naming the keys involved (no fallback to another variant).
- Serialization is unchanged (keep the derived or hand-written `Serialize` byte-identical; add a round-trip test per variant).

## Tests (existing tests dir, a new file if needed: one file = one topic)
One positive test per accepted encoding (each variant, both rmp_serde named maps and serde_json), reversed `path`/`$query` order, `path: null`, RU7 boundary values (i64::MAX, i64::MAX+1 as u64, u64::MAX). Negative tests that FAIL ON THE OLD CODE: `{"$ref":42,"$query":"q"}`, `{"$query":"q","$ref":["x"]}`, `{"$ref":["a"],"path":["b"]}`, `{"$query":"q","extra":1}`, empty map `{}`. Existing tests in `filter_value_conv_tests.rs` stay green.

## Rules
Touch only `shamir-query-types`. No new dependency, no `#[allow]`, no visibility widening, layout limits (<=7 entries/folder, <=1000 lines/file), one file = one primary export (put the visitor in its own file next to `filter_value.rs`), imports at top, `mod.rs` re-exports only, tests in `tests/` dirs. Do NOT touch `batch_op.rs` or `ops/batch/**` (another executor works there) and do not add `deny_unknown_fields` anywhere (a later stage does).

## Local gates (only these)
`cargo fmt -p shamir-query-types -- --check`; `cargo clippy -p shamir-query-types --all-targets -- -D warnings`; `cargo check -p shamir-query-builder -p shamir-engine-query -p shamir-server --all-targets`; `python -I scripts/ci/check_layout.py` (the layout checker ignores untracked files - count direct entries of touched folders yourself with ls). One cargo process at a time, CARGO_BUILD_JOBS=2. Report real exit codes.
