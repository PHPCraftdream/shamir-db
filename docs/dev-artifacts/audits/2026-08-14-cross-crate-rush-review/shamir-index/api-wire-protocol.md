<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-index — api-wire-protocol independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Reconstruction and genuine historical-byte compatibility defects remain. Hash-format and ordinal notes are compatibility/documentation debt rather than demonstrated current version corruption. Options and permissive prefix parsing are not intrinsically invalid APIs.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 10 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Persisted `VectorConfig.backend` is ignored on the reopen/rebuild path

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The builder always creates default in-process HNSW, including for External. Successful snapshots restore live graph parameters; absent/corrupt snapshots and compaction defaults lose tuning. Test nondefault m/ef through fallback reopen and compaction.

Evidence: [crates/shamir-index/src/build_backend.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/build_backend.rs#L52); [crates/shamir-index/src/vector/hnsw_adapter.rs:952](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L952); [crates/shamir-engine/src/table/table_manager.rs:665](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager.rs#L665).

<a id="review-2"></a>

### Claim 2 — SQ8 quantization opt-in has no durable carrier and is lost on most restarts

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

serde(skip) removes intent and from_parts sets None, disabling future fit. Fitted snapshots reconstruct Some(Sq8); therefore only pre-fit/absent/corrupt snapshot paths demonstrate intent loss, not 'most' restarts quantitatively.

Evidence: [crates/shamir-index/src/kind.rs:185](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/kind.rs#L185); [crates/shamir-index/src/vector/hnsw_adapter.rs:549](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L549); [crates/shamir-index/src/vector/hnsw_adapter.rs:618](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/hnsw_adapter.rs#L618).

<a id="review-3"></a>

### Claim 3 — Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Historical 3ed3069b^ manifest lacks q fields and sidecar lacks vectors_u8. Current positional bincode 1.3.3 decoding has no fallback. The registered test re-encodes current structs with version=1 and cannot detect historical layout failure. Published source: https://docs.rs/crate/bincode/1.3.3/source/src/de/mod.rs.

Evidence: [crates/shamir-index/src/vector/snapshot.rs:279](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L279); [crates/shamir-index/src/vector/snapshot.rs:723](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L723); [crates/shamir-index/src/vector/tests/quantization_snapshot_tests.rs:384](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/tests/quantization_snapshot_tests.rs#L384); [Cargo.lock:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L413).

<a id="review-4"></a>

### Claim 4 — Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dependency

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The caret declaration and format version omit algorithm/target identity. Exact rustc-hash 2.1.2 is pointer-width-sensitive; no current patch-output regression was demonstrated. This is prospective compatibility debt. Published source: https://docs.rs/crate/rustc-hash/2.1.2/source/src/lib.rs.

Evidence: [crates/shamir-index/Cargo.toml:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/Cargo.toml#L30); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007); [crates/shamir-index/src/persistence.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/persistence.rs#L37).

<a id="review-5"></a>

### Claim 5 — `flip_generation` never prunes the old generation's `qgraph`/`qdata` chunks

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Old manifest state retains only graph/data counts and flip removes only those sections plus sidecar. Successful quantized flips leave old q chunks. Assert exact old-generation namespace emptiness, not only manifest advancement.

Evidence: [crates/shamir-index/src/vector/vector_backend.rs:906](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/vector_backend.rs#L906); [crates/shamir-index/src/vector/snapshot.rs:1300](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L1300).

<a id="review-6"></a>

### Claim 6 — `MetaEnvelope::open` validates magic/version only after deserializing the payload

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A valid unsupported header with an incompatible/truncated payload yields Decode before UnsupportedVersion. Header-first dispatch can preserve the existing fixed byte layout if the exact bincode options are retained.

Evidence: [crates/shamir-index/src/meta_envelope.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/meta_envelope.rs#L54); [crates/shamir-index/src/meta_envelope.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/meta_envelope.rs#L64).

<a id="review-7"></a>

### Claim 7 — Snapshot load path can panic on corrupt-but-decodable persisted data; the sidecar carries no checksum

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Method is checked, but dim/mins/scales consistency is not. Changing only validly encoded QuantMeta lengths leaves graph-section CRC unchanged and reaches synchronous assertions. Tests must assert typed corruption/rebuild, not only malformed envelope rejection.

Evidence: [crates/shamir-index/src/vector/snapshot.rs:822](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L822); [crates/shamir-index/src/vector/snapshot.rs:973](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L973); [crates/shamir-index/src/vector/quant_meta.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/quant_meta.rs#L73); [crates/shamir-index/src/vector/sq8.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/sq8.rs#L91).

<a id="review-8"></a>

### Claim 8 — Bincode ordinal-stability contract is documented on some persisted enums but missing on others

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

IndexKind, TokenizerKind, and IndexExpr lack the notes present on sibling enums. Actual ordinal serialization is confirmed in bincode 1.3.3, but no enum reorder is shown. This is preventive documentation, not current wire corruption.

Evidence: [crates/shamir-index/src/kind.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/kind.rs#L11); [crates/shamir-index/src/kind.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/kind.rs#L25); [crates/shamir-index/src/expr.rs:21](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/expr.rs#L21); [Cargo.lock:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L413).

<a id="review-9"></a>

### Claim 9 — `IndexDescriptor.options` is dead public API

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The public field is explicitly opaque/default-empty and persisted unchanged. No interpreted tuning promise is violated. Removal would require migration; using it as a versioned carrier is an optional design.

Evidence: [crates/shamir-index/src/descriptor.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/descriptor.rs#L26); [crates/shamir-index/src/persistence.rs:212](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/persistence.rs#L212).

<a id="review-10"></a>

### Claim 10 — Corrupt FTS posting values are silently replaced with `tf=1, doc_len=1`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Empty legacy postings deliberately default, but nonempty bincode failures use the same fallback without signal. Corrupt nonempty values can alter ranking. An oracle must inject that branch and inspect scoring/error behavior.

Evidence: [crates/shamir-index/src/fts_ranked_backend.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L125); [crates/shamir-index/src/fts_ranked_backend.rs:128](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L128).

<a id="review-11"></a>

### Claim 11 — Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Failed is shipped while lifecycle still argues it is unnecessary. from_bytes intentionally reads a 25-byte prefix and has safe slice bounds; accepting a longer posting key and returning String are not independently proven correctness defects. Public with_values removal is API-breaking.

Evidence: [crates/shamir-index/src/lifecycle.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/lifecycle.rs#L31); [crates/shamir-index/src/state.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/state.rs#L73); [crates/shamir-index/src/base_index/index_record_key.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_record_key.rs#L104); [crates/shamir-index/src/base_index/index_record_key.rs:62](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_record_key.rs#L62).

## Evidence and recipe corrections

- Do not claim universal reopen tuning loss or quantify 'most' SQ8 restarts; fitted snapshot restoration is positive counter-evidence.
- The v1 sidecar already contained reserved quantization; only genuinely absent fields justify the positional-layout witness.
- Exact pinning alone does not make FxHasher cross-target or persisted-format stable.
- Appending an enum variant preserves old-into-new ordinal meanings, not new-variant readability by old readers.
- String error payloads and public deprecated helpers are API tradeoffs; length-equality changes must first distinguish logical key decoding from prefix parsing.
- The configured unwind policy means a data-driven panic need not abort the entire server process.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-index -- API & wire-protocol design

## Summary

The crate's query surface is clean and builder-only by construction: there is no `serde_json` dependency anywhere, all lookups go through the typed `IndexQuery` enum, and all write ops are typed `IndexWriteOp`s — the CLAUDE.md "never hand-assemble from raw JSON" rule is trivially satisfied. The base_index family's on-disk versioning (three-tier shadow-shape decodes, system-key collision analyses, `LEGACY_INDEX_FORMAT_VERSION` rebuild gate, versioned DDL op-log records) is exemplary. The serious problems concentrate in the index2 vector family: the persisted `VectorConfig` is silently ignored on every reopen/rebuild path, the SQ8 opt-in has no durable carrier at all, and the vector snapshot codec's advertised v1 back-compat is not backed by a working decode path (nor by a genuine v1-bytes test), while FxHasher output — which IS the persisted key format for three index families — rides on a caret-pinned dependency with no format-version coupling.

## Findings

### 1. Persisted `VectorConfig.backend` is ignored on the reopen/rebuild path
- **File:** `crates/shamir-index/src/build_backend.rs:52-65` (with `crates/shamir-index/src/kind.rs:189-200`)
- **Severity:** high
- **Issue:** `build_index2_backend_with_resolver` — documented as "Shared by `TableManager::create` (reopen path) and `replicate_index2_descriptors_from`" — never reads `cfg.backend`. It hardcodes `HnswConfig { max_elements: 100_000, m: 16, ef_construction: 200, ef_search: 50 }` for every vector descriptor. Grep confirms no production code in the crate ever reads `VectorBackendRef`: `InProcessHnsw { ef_construct, m }` is constructed only in tests, and `External { driver, url, api_key_secret }` is silently rebuilt as an in-process HNSW. The persisted wire discriminant says one thing; the restored backend is another.
- **Failure scenario:** (a) `CREATE ... VECTOR (ef_construct = 400, m = 32)`; crash before the first background snapshot (snapshots only trigger at `VECTOR_SNAPSHOT_DELTA_THRESHOLD` mutations); reopen takes the `NotFound`/rebuild branch → graph rebuilt with m=16/ef=200 → recall silently degrades, and the *next* snapshot persists the default params, making the loss permanent. (b) An `External` driver index reopens as in-process HNSW with no error. Existing tests mask this only by coincidence: every test descriptor uses `ef_construct: 200` (`vector_restore_tests.rs:110-111`, `crash_recovery_tests.rs:121-122`, `delta_log_tests.rs:93-94`), exactly matching the hardcoded value.
- **Suggested fix:** In the `IndexKind::Vector(cfg)` arm, map `cfg.backend` → `HnswConfig` (`InProcessHnsw { ef_construct, m }` → config; `External { .. }` → explicit error or a real external adapter), and thread `cfg.quantization` through `HnswAdapter::new_with_quantization` (see finding 2). Add a reopen test with non-default `ef_construct`/`m`.

### 2. SQ8 quantization opt-in has no durable carrier and is lost on most restarts
- **File:** `crates/shamir-index/src/kind.rs:168-186`; `crates/shamir-index/src/build_backend.rs:53-63`; `crates/shamir-index/src/vector/hnsw_adapter.rs:526-549` and `:1292-1296`
- **Severity:** high
- **Issue:** `VectorConfig::quantization` is `#[serde(skip)]`, so it is absent from every persisted `IndexDescriptor`; the kind.rs doc says the mode is "carried through the WIRE op … it is NOT persisted in #411 (snapshot codec for quantization is #412)". But #412 only round-trips a *fitted* quantizer through a v2 snapshot sidecar. The two restore paths that don't go through a quantized snapshot both construct unquantized adapters: `build_index2_backend` calls `HnswAdapter::new` (never `new_with_quantization`), and `HnswAdapter::from_parts` hardcodes `quantization: None` (hnsw_adapter.rs:549) — while `try_fit_and_rebuild` returns immediately when `quantization.is_none()` (:1293-1296). The `from_parts` doc's claim that a loaded adapter "starts un-fitted and will re-fit at the threshold on the next upserts" (":526-530") is therefore false: an adapter loaded via `from_parts` can never fit.
- **Failure scenario:** Create an SQ8 index; insert ≥256 vectors (fit fires, u8 graph live). Restart *before* a background snapshot exists (pre-threshold), or with a corrupt/version-mismatched snapshot → rebuild path → unquantized f32 adapter forever; the next dump is non-quantized, so every later restart stays f32. Result: silent ~4× memory regression and changed recall characteristics on a feature the user explicitly opted into. Even a *successful* restart from a snapshot taken pre-fit (below `FIT_THRESHOLD`) permanently loses the opt-in.
- **Suggested fix:** Persist the quantization mode durably — either replace `#[serde(skip)]` with a forward-compat carrier (e.g. encode it into the existing `IndexDescriptor.options` bytes, or bump `PersistedIndexes` with a shadow-shape fallback like `state` got in F-50), or reconstruct it in `from_parts`/`build_index2_backend` from a config source that survives restart. Also fix the `from_parts` doc comment.

### 3. Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous
- **File:** `crates/shamir-index/src/vector/snapshot.rs:95-105`, `:278-310`, `:206-255`; test at `crates/shamir-index/src/vector/tests/quantization_snapshot_tests.rs:363-406`
- **Severity:** medium
- **Issue:** `SNAPSHOT_SUPPORTED_VERSIONS = &[1, 2]` and the docs promise "A v1 dump loads on a v2 build via the back-compat path". But the v2 fields were *inserted into the middle* of the positional bincode layout — `SnapshotManifest` gained `qgraph_chunks`/`qdata_chunks` between `data_chunks` and `basename`, and `qbasename` between `basename` and `delta_applied_upto`; `SnapshotSidecar` gained `quantization`/`vectors_u8` before `sections_crc32` — guarded only by `#[serde(default)]`, which this workspace has *proven* (state.rs module doc, `index_state_compat_tests.rs`) does not rescue old bincode blobs (and mid-insertion misaligns regardless). Unlike `persistence::decode_persisted_indexes`, `IndexInfo::decode_bytes`, and `SortedIndexManager::load`, there is no v1 shadow-shape fallback. A genuine pre-#412 blob would fail inside `MetaEnvelope::open` → mapped by `map_meta_err` to `Corrupt` — never reaching the `SNAPSHOT_SUPPORTED_VERSIONS` check, so `1` in that array is dead. The regression test `migration_v1_snapshot_loads_back_compat` does not write v1-shaped bytes: it decodes a current-shape dump, sets `format_version = 1`, and re-encodes with the *current* struct — it validates only the version-label gate, not the layout.
- **Failure scenario:** Upgrade a deployment whose vector indexes have pre-#412 snapshots: every open logs "snapshot load failed … falling back to full rebuild" and pays a full O(N) re-index per vector index, while the code and `SNAPSHOT_SUPPORTED_VERSIONS` claim v1 loads are supported.
- **Suggested fix:** Either add a v1 shadow-shape decode (mirror `decode_persisted_indexes`) or stop advertising v1 support (drop it from the array + docs) and let `VersionMismatch`-style messaging surface. Add a test fixture that writes actual v1-shaped bytes (hand-built struct without the q* fields).
- **Caveat:** if the v1 writer already serialized the now-`#[serde(default)]` fields as zero/None (the sidecar doc hints reserved fields were pre-declared), the layout may in fact be compatible — but nothing in the code or tests demonstrates that, and the `#[serde(default)]` annotations argue otherwise. Either way the contract is currently unverified.

### 4. Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dependency
- **File:** `crates/shamir-index/src/tokenizer.rs:462-469` (`token_hash`); `crates/shamir-index/src/base_index/index_keys.rs:186-240` (`hash1`/`hash2`); `crates/shamir-index/src/functional_backend.rs:68-81`, `:131-175` (`hash_value`/`hash_inner`); `Cargo.toml:30` (`rustc-hash = "2.1"`) vs `Cargo.toml:47-51` (hnsw_rs exact pin)
- **Severity:** medium
- **Issue:** FxHasher-derived u64s are embedded in persisted keys for three families: FTS postings (`token_hash`), legacy regular/unique postings (`hash1`/`hash2`, whose tag scheme is explicitly documented as "part of on-disk index compatibility"), and functional postings (`FunctionalBackend::hash_value`, doc: "the tag scheme is part of on-disk index compatibility and must stay stable"). The *tags* are stable, but the hash function's output is not under this repo's control: `rustc-hash` gives no cross-version output-stability guarantee (and did change output at the 1.x→2.x boundary), yet it is caret-pinned and unmarked in any format version. The team already solved this exact class of risk for `hnsw_rs` (exact `=0.3.4` pin + `HNSW_RS_VERSION` check refusing foreign dumps at load) — `rustc-hash` got neither.
- **Failure scenario:** A routine `cargo update` bumps rustc-hash 2.1 → 2.2 with an algorithm tweak. Queries now hash tokens/values differently than the on-disk keys: all FTS/functional/hash-index lookups silently return empty/partial results. `legacy_indexes_need_rebuild` does not fire (the stored `_m.idx.lfv` still equals 2 — the *scheme* tag didn't change), so nothing rebuilds.
- **Suggested fix:** Exact-pin `rustc-hash` (mirroring the hnsw_rs pin comment) and/or fold the hash-function identity into `LEGACY_INDEX_FORMAT_VERSION` with a boot-time self-check (hash a fixed vector at startup and compare against a baked-in constant; bump the format version on mismatch to trigger rebuild).

### 5. `flip_generation` never prunes the old generation's `qgraph`/`qdata` chunks
- **File:** `crates/shamir-index/src/vector/snapshot.rs:1287-1330`; caller `crates/shamir-index/src/vector/vector_backend.rs:906-911`, `:955-963`
- **Severity:** medium
- **Issue:** The generation flip is the *only* prune mechanism (the dump path's comment "Wipe any PRIOR generation's chunks first" is followed by no code). `flip_generation` removes the old gen's `graph` chunks, `data` chunks, and sidecar — but not its `qgraph`/`qdata` chunks. The caller reads only `(m.gen, m.graph_chunks, m.data_chunks)` from the old manifest, so the API couldn't even express the q-chunk counts: the signature has `old_graph_chunks`/`old_data_chunks` but no `old_qgraph_chunks`/`old_qdata_chunks`.
- **Failure scenario:** A quantized index flips generation every `VECTOR_SNAPSHOT_DELTA_THRESHOLD` mutations; each flip leaks the previous u8-graph dump (a full second copy of the graph) in the info store forever. Unbounded storage growth with no correctness signal (the manifest points only at the new gen, so nothing ever reads the orphans).
- **Suggested fix:** Extend the manifest read + `flip_generation` signature with the old q-chunk counts and emit `KvOp::Remove` for `qgraph`/`qdata` chunk keys in the same transact.

### 6. `MetaEnvelope::open` validates magic/version only after deserializing the payload
- **File:** `crates/shamir-index/src/meta_envelope.rs:50-67`
- **Severity:** low
- **Issue:** The module doc says the envelope exists "so future migrations can dispatch on `version` without ambiguity", but `open` runs `bincode::deserialize::<MetaEnvelope<T>>` over the *whole* envelope first. A future version-2 envelope whose payload `T` changed shape fails inside the payload decode and surfaces as `MetaError::Decode` (mapped to `Corrupt` by `map_meta_err`) — `UnsupportedVersion` is observable only when the payload happens to still decode. This is precisely why every consumer (`decode_persisted_indexes`, `IndexInfo::decode_bytes`, sorted three-tier load) had to grow shadow-shape fallback chains instead of dispatching on the version field.
- **Suggested fix:** Split the envelope into a fixed header (`magic`, `version`, `written_at_nanos`) decoded first, with the payload decoded only after the version check (bincode supports this by deserializing the header from a reader, then the payload from the remainder).

### 7. Snapshot load path can panic on corrupt-but-decodable persisted data; the sidecar carries no checksum
- **File:** `crates/shamir-index/src/vector/quant_meta.rs:58-80`; `crates/shamir-index/src/vector/sq8.rs:89-93`; `crates/shamir-index/src/vector/snapshot.rs:694-706`
- **Severity:** low
- **Issue:** `QuantMeta::to_quantizer` `assert_eq!`s the method tag and the `mins`/`scales` lengths, and calls `Sq8Quantizer::fit`, which `assert!`s `dim > 0`. The method tag is guarded upstream (snapshot.rs:822), but a sidecar blob whose `dim`/`mins`/`scales` disagree (still-valid bincode — e.g. after a bit flip) panics during `restore_on_open`, i.e. a process crash driven purely by disk bytes. This input is effectively unverified: per `map_meta_err`'s own doc, "the manifest/sidecar bytes … carry no crc of their own, unlike the chunk bodies". CLAUDE.md's error-handling rule reserves panics for programmer-error invariants, not external data.
- **Suggested fix:** Make `to_quantizer` fallible (or validate lengths/dim in `load_snapshot` and return `SnapshotError::Corrupt`), and consider adding a crc32 over the sidecar/manifest envelope payloads.

### 8. Bincode ordinal-stability contract is documented on some persisted enums but missing on others
- **File:** `crates/shamir-index/src/kind.rs:10-40` (`IndexKind`, `TokenizerKind`); `crates/shamir-index/src/expr.rs:21-52` (`IndexExpr`)
- **Severity:** low
- **Issue:** `StemLanguage`, `VectorQuantization`, and `IndexState` each carry an explicit "# Bincode ordinal stability: append only / DO NOT MOVE" contract. `IndexKind`, `TokenizerKind`, and the entire persisted `IndexExpr` AST (inside `FunctionalConfig`, inside `IndexDescriptor.kind`) are serialized by the same ordinal-tagged bincode but carry no such note. `IndexExpr` in particular reads like an alphabetized list where inserting a variant mid-enum is an easy "innocent" refactor that silently corrupts every persisted functional index.
- **Suggested fix:** Replicate the ordinal-stability doc + `// ordinal N — DO NOT MOVE` markers on `IndexKind`, `TokenizerKind`, and `IndexExpr` variants.

### 9. `IndexDescriptor.options` is dead public API
- **File:** `crates/shamir-index/src/descriptor.rs:26-29`
- **Severity:** low
- **Issue:** Documented as "Opaque backend-specific tuning (bincode-friendly)… empty by default", but grep shows no writer (always `Vec::new()` via `IndexDescriptor::new`) and no reader anywhere in the crate — it is only round-tripped by persistence and asserted in a compat test. It misleads API consumers into thinking backend tuning has a persisted channel (it does not — see finding 1).
- **Suggested fix:** Either wire it into a backend (a natural home for the quantization mode of finding 2) or remove it before the format ossifies.

### 10. Corrupt FTS posting values are silently replaced with `tf=1, doc_len=1`
- **File:** `crates/shamir-index/src/fts_ranked_backend.rs:125-130`
- **Severity:** low
- **Issue:** `bincode::deserialize(&val_bytes).unwrap_or(FtsPostingValue { tf: 1, doc_len: 1 })` conflates legacy MVP-era empty postings with genuine value corruption. A corrupted posting value silently yields wrong BM25 tf/doc_len inputs (skewed rankings) instead of surfacing an error or at least a log line — contrary to the workspace's "checksums everywhere / surface corruption" stance.
- **Suggested fix:** Keep the empty-value fast path for legacy postings, but log at `warn` (or error) on a non-empty value that fails to decode.

### 11. Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts
- **File:** `crates/shamir-index/src/lifecycle.rs:31-57` vs `crates/shamir-index/src/state.rs:59-73`; `crates/shamir-index/src/base_index/index_record_key.rs:62-81`, `:104-120`
- **Severity:** nit
- **Issue:** lifecycle.rs still argues "Why no `Dropping` / `Failed` enum variant?" while `IndexState::Failed` shipped in R0-D (#1013) — the public lifecycle contract doc misdescribes the persisted enum. Separately, `IndexRecordKey::from_bytes` returns `Result<_, String>` (not a thiserror type, per CLAUDE.md) and silently accepts keys longer than 25 bytes; the deprecated `with_values` FxHasher helper remains fully `pub` "for tests only".
- **Suggested fix:** Update lifecycle.rs to describe `Failed` (and link #1013); tighten `from_bytes` (length equality + proper error type) and demote `with_values` to `#[cfg(test)]` or remove it.

---

**Builder-only rule compliance (theme checklist):** no violations found. The crate has no `serde_json` dependency; queries are constructed exclusively via the `IndexQuery` enum (`Point`/`Range`/`Fts`/`Vector`) and persisted ops via the typed `IndexWriteOp` re-export; the one raw-string parse surface (`StemLanguage::from_dsl`, `VectorQuantization::from_dsl`) returns `Option` rather than erroring, and covers both full names and ISO codes. Wire keys (`_m.idx*` system keys, posting-layout prefixes, `__vec_snap__<id>` keyspaces) are consistently documented with byte-level collision analyses.

</details>
