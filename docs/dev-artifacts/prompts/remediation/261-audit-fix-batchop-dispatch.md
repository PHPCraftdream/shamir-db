# 261 — audit-fix-batchop-dispatch (P1 audit follow-up: H5 / RV-QUERY-TYPES-1EC04E32)

Source: post-merge audit of PR #323 + design decision by @oh (2026-10-29, binding). Crate: `shamir-query-types` only. Read first: `CLAUDE.md`, `crates/shamir-query-types/src/ops/batch/ops/batch_op.rs` (Deserialize ~380-570: `assert_nested_batch_caps` ~380, `DISCRIMINATORS.contains` ~382-391, MessagePack re-encode ~402-407, 76-branch `has()` chain ~413-568), `crates/shamir-types/src/types/value.rs` (~71, `Big` encoding), the dispatch tests (`batch_op_dispatch_tests.rs`, `batch_limits_tests.rs`), `ops/batch/ops/query_entry.rs`.

## Defects
1. `BatchOp` decoding buffers the map, RE-ENCODES it to MessagePack and decodes again, then runs a linear 76-way discriminator chain: O(n_ops x 76) plus an extra encode/decode per op.
2. Latent bug: a typed `u64` field in an op body above `i64::MAX` (`expected_version` in `Update`/`Delete`, nested `limits.max_total_work`) is rejected today, because `Big` is re-encoded as a string and the `u64` field refuses it.

## Change (behaviour otherwise unchanged)
1. Single buffer per op. Add a crate-private deserializer over the owned buffered value (`QueryValueDeserializer`) built on the public `serde::de::value::{MapDeserializer, SeqDeserializer}`; decode the selected op DIRECTLY from it. No MessagePack re-encode. It must mirror what rmp_serde presents to a visitor: `Big` -> `visit_u64` (it only ever holds a u64 above `i64::MAX`), `Bin` -> `visit_byte_buf`, `deserialize_option` maps `Null` to `visit_none`, enums accept a string (unit variant) or a one-entry map, `is_human_readable()` returns `false`. No new dependency (serde `Content` is private and unstable; `rmpv` is not a dependency).
2. O(1) dispatch. Replace `DISCRIMINATORS.contains` and the `has()` chain with `fn op_kind(&str) -> Option<OpKind>` (a `match` on the key) and one `match` over `OpKind`. Count discriminators over the BUFFERED map keys so duplicate keys still collapse to last-wins as today. Keep: the `update`+`set` exemption (`Update` wins), the exact existing error strings, partial-limit handling, ambiguous-discriminator rejection, and `assert_nested_batch_caps` BEFORE typed decoding.
3. Layout: `ops/batch/ops` goes from 5 to 7 entries (the limit) - put `OpKind` and the deserializer in new files and keep `mod.rs` declarations-only; `batch_op.rs` is 951 lines and must end up smaller than before. A new `impl` may require an entry in `.rust-guard/owned_impl.toml` (read how existing entries look; add one only if the structure gate needs it, with `owner` and `reason`).

## Tests (must fail on the old code)
- `Update` and `Delete` with `expected_version = u64::MAX` (and i64::MAX+1) decode exactly; a nested sub-batch with `limits.max_total_work = u64::MAX` decodes.
- Every discriminator round-trips to its own variant (table-driven over all of them; derive the list from `OpKind`).
- Duplicate keys: last wins as before. `update`+`set` still decodes as `Update`. Two other discriminators still error with the existing message.
- Existing dispatch/limits tests stay green. Do NOT add `deny_unknown_fields` (a later stage does).

## Rules
Touch only `shamir-query-types` (+ `.rust-guard/owned_impl.toml` only if required). Do NOT touch `read/filter/**` (another executor works on `FilterValue`). No `#[allow]`, no visibility widening beyond `pub(crate)`, imports at top, `mod.rs` re-exports only, tests in `tests/` dirs, JSON literals in tests multi-line.

## Local gates (only these)
`cargo fmt -p shamir-query-types -- --check`; `cargo clippy -p shamir-query-types --all-targets -- -D warnings`; `cargo check -p shamir-query-builder -p shamir-engine-query -p shamir-engine -p shamir-server --all-targets`; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). One cargo process at a time, CARGO_BUILD_JOBS=2. Report real exit codes.
