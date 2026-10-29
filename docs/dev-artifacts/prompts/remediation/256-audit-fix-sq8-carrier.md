# 256 — audit-fix-sq8-carrier (P1 audit follow-up: F9 RV-INDEX-E69B5ACD durable SQ8 opt-in)

Source: post-merge audit of PR #322 (P1 F9) + @oh design decision (2026-10-29). Problem: `VectorConfig.quantization: Option<VectorQuantization>` is `#[serde(skip)]` (`crates/shamir-index/src/model/kind.rs` ~160-190) because bincode 1.3.3 ignores `#[serde(default)]` for skipped fields and any layout change would break every persisted `IndexDescriptor`. So an unfitted SQ8 index loses its opt-in on restart. Read first: `CLAUDE.md`, `model/kind.rs`, `model/meta/{descriptor.rs,persistence.rs,registry.rs}` (descriptor `options: Vec<u8>` at descriptor.rs ~26-29; written at ~59 and persistence.rs ~292; nothing reads it), `vector/hnsw_adapter/adapter.rs` (`from_parts` ~280, hardcoded `None` ~300), `vector/vector_backend/{backend.rs (restore_on_open ~732),build_backend.rs (~72)}`, `vector/hnsw_adapter/fit.rs` (~286 FIT_THRESHOLD), `vector/tests/.../vector_restore_tests.rs` (~177, 261, 321), engine `index2_persistence_tests.rs`.

## Design (decided - implement exactly)
Use the EXISTING `IndexDescriptor::options` byte field - NO layout change.
- **Format:** `[0x01][rmp_serde::to_vec_named(IndexOptionsV1)]` with `IndexOptionsV1 { #[serde(default)] vector_quantization: Option<String> }` holding `"sq8"`. A string (never a bincode ordinal), a NAMED msgpack map (later fields via `#[serde(default)]`, unknown fields ignored). `rmp-serde` is already a dependency (see Cargo.toml). Empty `options` = no options (nothing non-empty was ever stored).
- **Who writes/reads:** ONLY the persistence layer; new file `crates/shamir-index/src/model/meta/index_options.rs` (folder 4 -> 5 entries).
  - Writer: `save_index2_metadata_with_pending` (persistence.rs ~122), per descriptor: vector kind with `quantization: Some` -> write v1; `None` and empty options -> leave empty (non-SQ8 descriptors stay byte-identical); options with an unknown version byte -> leave the bytes untouched and log a warning.
  - Reader: `decode_persisted_indexes` (~215-244), both decode tiers, sets `cfg.quantization`: empty -> `None`; v1 with unknown string -> warning + `None`; v1 whose payload fails to decode -> `DbError::Codec` (fail closed); unknown version -> warning + `None`, bytes kept.
- **Old binaries** decode the new data (same layout) and save the opaque bytes back verbatim via `all_descriptors` clone (`registry.rs` ~498): the setting survives downgrade+upgrade.
- **Reconciliation on reopen:** `from_parts` gets a `quantization` parameter; add `load_snapshot_with_intent` and keep `load_snapshot` as the `None` wrapper (about 12 test callers must not change). `restore_on_open` passes the descriptor's setting.
  | descriptor | snapshot on disk | result |
  | Some | fitted SQ8 | `from_parts_with_quantization`, unchanged |
  | Some | f32 only / missing / corrupt (rebuild via `build_backend.rs` ~72, which now receives `Some`) | unfitted, fits at FIT_THRESHOLD on the next write |
  | None | fitted (legacy indexes before this carrier) | `Some(Sq8)` - a fitted snapshot proves opt-in |
  | None | f32 only / missing | `None` |
- **Self-heal the legacy case:** add trait method `IndexBackend::persisted_descriptor()` with default `descriptor().clone()`; `VectorBackend` overrides it to overlay `adapter.quantization_mode()`; `registry.rs` ~498 calls it (same pattern as the existing state/name overlay there).
- **CF86432E interaction:** `descriptor_matches_ignoring_credential` (persistence.rs ~171-187) runs bincode over the whole descriptor incl. `options`: `normalized()` MUST also clear `copy.options`, otherwise a legacy External index with an inline credential could fail to save after options appear. Do NOT implement the secret-reference idea (F10) - only mention in the doc comment of `index_options.rs` that `api_key_ref` will live in this map later.

## Tests
- `index_options` unit tests: round trip, empty, unknown version byte kept verbatim, malformed v1 -> `Codec`, unknown string -> `None`+warn.
- Golden bytes of a v1 descriptor decoded with a TEST-LOCAL mirror of the pre-change struct (master cbd8e853 shape), proving old binaries read it; the existing blob-without-`state` compatibility test stays green.
- In `vector_restore_tests.rs`, each doing save -> `load_index2_metadata` -> `build_index2_backend` -> `restore_on_open`: SQ8 with fewer than FIT_THRESHOLD vectors and no snapshot -> `quantization_mode() == Some(Sq8)`, then insert to the threshold -> `is_quantized()`; the same with an f32 snapshot; the same with a corrupt snapshot; legacy fitted snapshot with empty options -> after save the options decode to `Some`; `None` + f32 snapshot never fits.
- A legacy External inline-credential descriptor still saves (CF86432E guard).
- Engine side: an `sq8` case in `index2_persistence_tests.rs`.
- `kind_tests.rs` ~62-84 stays (the field is still `serde(skip)`); refresh the doc comment at `kind.rs` ~133-186.

## Rules
Touch `shamir-index` (+ the one engine test file). No new dependencies, no `#[allow]` (remove the `#[allow(dead_code)]` the audit flagged at `vector/hnsw_adapter/fit.rs` ~416 if the new code makes the item used; otherwise leave), layout limits, one file = one primary export, imports at top, `mod.rs` re-exports only.

## Local gates (only these)
`cargo fmt -p shamir-index -p shamir-engine -- --check`; `cargo clippy -p shamir-index --all-targets -- -D warnings`; `cargo check -p shamir-engine --all-targets`; `cargo check -p shamir-db --all-targets`; `python -I scripts/ci/check_layout.py`. Real exit codes.
