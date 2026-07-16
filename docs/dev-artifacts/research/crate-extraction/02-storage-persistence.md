# Crate-extraction research — storage & persistence group

**Scope:** `shamir-storage`, `shamir-wal`, `shamir-tx`, `shamir-index`
**Date:** 2026-07-16
**Method:** read-only survey of `src/` trees, `Cargo.toml` dependency graphs, and module
docs. LOC figures exclude `tests/` directories unless noted. Precedent: `bench-scale-tool`
(extracted + published 2026-07-07).

Extraction criteria applied per candidate:
1. domain-generic (not shamir-db-specific),
2. minimal/no dependency on other `shamir-*` crates,
3. nontrivial (not a 20-line utility).

---

## 1. `shamir-storage` (~3.8k LOC src, +3.4k tests)

**What it does.** The `Store` trait — an async, bytes-in/bytes-out KV abstraction
(`insert/set/get/get_many/remove/transact/iter_stream/scan_prefix_stream`) — plus concrete
backends: `InMemoryStore` (scc TreeIndex), `FjallStore` (LSM, feature-gated),
`CachedStore` (moka read-cache with sync/async write modes), `MemBufferStore` (moka
write-back buffer with dirty-set flusher). Also owns `DbError` and `KeyBytes`
(= `RecordKey`).

**Deps:** shamir-types, shamir-collections, shamir-tunables + tokio/fjall/moka/scc/
arc-swap/bytes/serde.

### Candidate 1a — `KeyBytes` → standalone crate (proposed name: `keybytes` or `sso-bytes`)

- **Module:** `crates/shamir-storage/src/key_bytes.rs` (316 LOC) + `key_bytes/tests/`
  (~660 LOC — size gate, inline-vs-heap Eq/Hash consistency, serde byte-identity vs
  `Bytes` under bincode and rmp-serde).
- **Scope:** a 32-byte small-string-optimized byte-key: ≤23 bytes inline (alloc-free),
  longer spills to `bytes::Bytes` (zero-copy `From<Bytes>` for long inputs).
  Representation is provably unobservable: hand-written `Eq`/`Ord`/`Hash`/`Debug`/serde
  all route through `as_slice()`; serde encoding is byte-identical to
  `serde_bytes`-encoded `Bytes` (wire/disk-format invariance).
- **Dependency footprint:** `bytes` + `serde` + `serde_bytes`. **Zero** shamir-* imports.
  Extraction is a file move + re-export (`pub type RecordKey = keybytes::KeyBytes;`).
- **FOR:** this is exactly the shape of thing the ecosystem keeps reinventing — `smol_str`
  covers strings, `smallvec`/`tinyvec` cover vectors, but there is no widely-adopted
  "SSO byte-string that interops zero-copy with `bytes::Bytes` and guarantees
  representation-transparent Eq/Ord/Hash **and serde encoding parity with `Bytes`**"
  (Meta's `minibytes` is the closest and is unpublished-ish/vendored). The test suite
  (size gate, forced-heap consistency proofs, cross-format serde identity) is the real
  asset — it encodes the #489-class landmine most home-grown versions ship with. Already
  fully decoupled; extraction cost is near zero and it would shed one reason other crates
  depend on all of `shamir-storage`.
- **AGAINST:** it is small (~1k LOC with tests) and the space is crowded at the edges
  (`minibytes`, `bytes-utils`, `smallbytes`); adoption is uncertain. `INLINE_CAP = 23` is
  tuned to shamir's key shapes (RecordId 16 B, WalActiveKey 21 B) — a public crate would
  face pressure for a const-generic cap, which currently requires the unsafe union layout
  the module deliberately avoided. As a private module it costs nothing where it is.
- **Verdict:** **strong candidate**, best effort/value ratio in this whole group.

### Candidate 1b — `MemBufferStore` / `CachedStore` (write-back & read-cache Store decorators)

- **Modules:** `storage_membuffer.rs` (1256 LOC), `storage_cached.rs` (623 LOC).
- These are genuinely generic *patterns* (moka-backed write-back buffer with dirty-set
  flusher + eviction-inline flush; FIFO-ordered async write worker), but they are written
  against the `Store` trait, `RecordKey`, `DbError`, and `shamir-tunables` config.
  Extracting them means first publishing the `Store` trait itself as a public abstraction
  — i.e. publishing shamir-storage's whole API surface, which is a product decision (an
  "async-kv-facade" crate), not a module extraction. moka itself already covers 80% of
  what an outside user would want.
- **Verdict:** not worth extracting now. Revisit only if the `Store` trait is ever
  published deliberately.

**Rest of the crate:** backends are glue around fjall/scc/moka — nothing else to extract.

---

## 2. `shamir-wal` (~2.4k LOC src, +2.0k tests)

**What it does.** File-segment write-ahead log: `WalSegment` (append-only file with CRC
framing, two durability tiers — write()+flush = "survives process crash" vs fsync =
"survives power loss", dir-fsync on create), `SegmentSet` (directory of numbered
segments, rotation at `max_bytes`, watermark-based whole-segment truncation — the append
path and truncation path never touch the same file), `WalGroupCommit` (rotating-leader
group commit: one CAS-elected leader per window drains a queue, issues one batched
`write()` and at most one `fsync`, with documented L1/L2/L3 liveness proofs),
`WalEntryV2` (the shamir-specific entry format: table tokens, RecordId, interner delta),
`WalSink` (File | Mem enum), `WalActiveKey`.

**Deps:** shamir-types (RecordId), shamir-storage (only `DbError`/`DbResult` +
`RecordKey` in active_key) + bytes/serde/bincode/crc32fast/tokio.

### Candidate 2a — segmented WAL core → standalone crate (proposed name: `segwal` or `tierwal`)

- **Modules:** `wal_segment.rs` (580) + `segment_set.rs` (590) + `wal_group_commit.rs`
  (431) + `wal_sink.rs` (209) ≈ **1.8k LOC** + ~2k LOC of tests/benches
  (`wal_append`, `wal_startup_open`).
- **Coupling:** the only shamir-* imports are `DbError`/`DbResult` (trivially replaced by
  a crate-local error enum) and `WalEntryV2` — the one real cut point. `WalSegment`
  physically appends CRC-framed byte payloads and tracks a `max_committed: u64`
  watermark; the entry's *content* (RecordId, table tokens, interner delta) is
  domain-specific but the segment/rotation/truncation/group-commit machinery only needs
  `(payload: &[u8], commit_version: u64)`. Genericizing the frame to that pair severs the
  dependency cleanly. `WalGroupCommit` is already generic over `WalSink`.
- **FOR:** this is classic reinvented infrastructure. The differentiators over existing
  crates (`okaywal`, `simple-wal`, `wal`): (1) the explicit **two-tier durability
  contract** (Buffered = page-cache/process-crash-safe, Synced = fsync/power-loss-safe)
  surfaced per-append, with a window fsyncing IFF it contains a Synced waiter; (2) a
  rotating-leader group commit with a written liveness argument (no stranded committer /
  no lost wakeup / circuit-breaker); (3) watermark truncation that deletes whole sealed
  segments with zero writer↔truncator coordination; (4) dir-fsync correctness on segment
  creation (the ext4 gotcha most hobby WALs miss). It is async-native (tokio +
  spawn_blocking), which `okaywal` (sync) is not. The in-crate test suite (recovery,
  torn-tail, fault injection via MemSink) travels with it.
- **AGAINST:** real work, not a file move — the `WalEntryV2` genericization touches
  `SegmentSet::recover` (which today decodes entries to read `commit_version`) and every
  call site in shamir-tx/shamir-engine; the version-watermark truncation model
  (`max_version <= durable_watermark`) is shamir's MVCC drain contract and needs to be
  presented as a generic "release key" for outsiders. The WAL space on crates.io is
  littered with abandoned one-offs; standing out requires docs + maintenance commitment.
  Also `WalDurability::Buffered` semantics are subtle and easy for downstream users to
  misuse (data loss on power failure by design).
- **Verdict:** **the strongest community-value candidate in this group**, medium
  extraction cost. Keep `WalEntryV2`/`WalActiveKey` in shamir-wal (which would become a
  thin domain shim over the published core).

**Not candidates:** `wal_entry_v2.rs`, `active_key.rs`, `segment_meta.rs` — pure shamir
domain (RecordId, table tokens, interner deltas).

---

## 3. `shamir-tx` (~7.6k LOC src, +12.5k tests)

**What it does.** The MVCC/transaction layer: `MvccStore` (versioned KV over a history
version log with `<key>‖0xFF‖version_be` physical keys, snapshot reads, GC, retention,
key locks), `RepoTxGate` (commit serialization, write-footprint conflict detection, SSI),
`TxContext`, `StagingStore`, `VersionedOverlay` (lock-free `(key, version) → value`
window between ack and durable drain), `CompletionTracker` (lock-free contiguous
watermark over version states), `LayeredInterner`, `PredicateSet` (SSI phantom
protection), changefeed, WAL manager glue.

**Deps:** shamir-collections, shamir-tunables, shamir-types, shamir-storage, shamir-wal —
the most coupled crate of the four.

### Honest headline: the MVCC layer as a whole is NOT extractable

`MvccStore`/`RepoTxGate`/`TxContext` are welded to the `Store` trait, `RecordKey`, the
WAL entry format, the interner, and shamir's drain/watermark pipeline. This is the
database; extracting "an MVCC crate" from it would mean redesigning its API around a
storage abstraction that doesn't exist publicly. Not recommended.

### Candidate 3a — `CompletionTracker` + `VersionedOverlay` as a "version-watermark toolkit" (proposed name: `version-watermark` or fold into a future `mvcc-primitives`)

- **Modules:** `completion_tracker.rs` (103 LOC — AtomicU64 watermark + scc::HashMap of
  above-watermark states; "highest V such that all versions ≤ V are Materialized or
  Aborted", with compaction) and `versioned_overlay.rs` (309 LOC — lock-free
  `scc::TreeIndex<(K, u64), Bytes>` overlay with O(1) atomic byte/count mirrors,
  `newest_visible` range scans, GC below a watermark).
- **Coupling:** CompletionTracker needs only `scc` + a hasher (THasher = FxHasher —
  replaceable by a default type param). VersionedOverlay needs `scc` + `bytes` +
  `RecordKey` — generic over `K: Ord + Clone` with one line of change.
- **FOR:** the "contiguous completion watermark over out-of-order finishers" is a real
  recurring primitive (WAL truncation points, Kafka-style low-watermarks, out-of-order
  commit pipelines) and the lock-free formulation here (CAS-advance + sparse
  above-watermark map + compaction) is tested and clean. Zero extraction friction.
- **AGAINST:** together they are ~400 LOC — near the "20-line utility" floor scaled up.
  Alone, CompletionTracker is a weekend crate anyone could write (the value is the tested
  edge cases, e.g. mark-below-watermark races). VersionedOverlay without the surrounding
  drain protocol is just "a sorted map keyed by (key, version)" — its interesting
  invariants live in `MvccStore`, not in the module. Publishing risks a
  maintenance-burden-to-value ratio worse than keeping them in-tree.
- **Verdict:** **marginal** — worth it only if bundled with the WAL extraction (2a) as a
  small companion, since the two watermark models are the WAL's natural truncation
  driver. Do not extract standalone.

**Other modules checked and rejected:** `version_codec` (66 LOC — too small),
`predicate_set` (217 LOC — SSI-shaped but engine-contract-specific), `key_lock` (90 LOC),
`layered_interner` (needs shamir's `Interner` trait), `changefeed` (658 LOC — bound to
`Actor`, RecordId, repo semantics), `id_remap` (pure domain).

---

## 4. `shamir-index` (~14.6k LOC src, +17k tests)

**What it does.** Index subsystem: FTS (tokenizers + stemmers, BM25 ranking, posting
layouts), functional indexes, legacy sorted/unique index managers, and a vector-index
stack (HNSW via pinned `hnsw_rs =0.3.4`, brute force, SQ8 scalar quantization, SIMD
distance kernels, KV-store snapshot codec with CRC + generations).

**Deps:** six shamir-* crates + hnsw_rs, rust-stemmers, scc, smallvec, etc.

### Candidate 4a — SIMD distance kernels + SQ8 quantizer (proposed name: `simd-dist` or `sq8-dist`)

- **Modules:** `vector/simd.rs` (962 LOC — runtime-dispatched f32 `dot_product` /
  `l2_squared` over AVX-512F / AVX2+FMA / NEON / autovectorizing scalar, plus `dot_u8`
  and a per-lane-weighted `weighted_bilinear_f32` kernel) + `vector/sq8.rs` (376 LOC —
  SQ8 per-dimension asymmetric quantizer with precomputed `scale²`/`min·scale` tables and
  an exact-on-dequantized approximate dot/L2/cosine) + optionally
  `vector/quantized_dist.rs` (424 LOC — an `hnsw_rs::Distance<u8>` impl over the
  quantizer). ≈ **1.4–1.8k LOC** + dedicated benches (`sq8_hot_path`) and an invariant
  test suite (all dispatch paths agree within FMA rounding).
- **Coupling:** `simd.rs` has **zero** imports of any kind — pure std + intrinsics.
  `sq8.rs` likewise pure. `quantized_dist.rs` needs `VectorMetric` (a 3-variant enum,
  trivially internalized) and binds to `hnsw_rs` (would be an optional feature).
  Everything is currently `pub(crate)` — an extraction just flips visibility.
- **FOR:** every vector-search project re-writes exactly these kernels; the combination
  of (a) OnceLock-cached widest-SIMD dispatch including AVX-512F, (b) a documented-math
  SQ8 quantizer whose approx dot is *exactly* the dot of the dequantized vectors
  (constant/linear/bilinear decomposition), and (c) the weighted-bilinear kernel that
  makes per-dimension scales SIMD-able, is genuinely useful and better-documented than
  most of what's published. The cross-path agreement test suite is the moat.
- **AGAINST:** the space has heavyweights — `simsimd` (many metrics, many dtypes, C
  core), `wide`/`pulp`/`std::simd` for portable SIMD — and a small hand-rolled kernel
  crate competes on trust, which takes CI across x86/aarch64 and fuzzing the unsafe
  intrinsic paths. The SQ8 quantizer is the more defensible half (simsimd does not do
  trained asymmetric per-dimension SQ8 scoring), but alone it's ~400 LOC. Pinning to
  `hnsw_rs =0.3.4` for the Distance impl leaks shamir's snapshot-format constraint into a
  public API.
- **Verdict:** **good candidate**, second priority after 2a/1a. Publish as
  "SQ8 quantizer with SIMD scoring" (quantizer as the headline, kernels as the engine)
  rather than as a generic SIMD crate competing with simsimd.

### Candidate 4b — HNSW snapshot codec (`vector/snapshot.rs`, 1395 LOC)

- Chunked (1 MiB) dump/load of `hnsw_rs` graphs into any KV store, crc32 per chunk +
  cross-section crc, generation manifest, atomic transact write. This solves a real pain
  (hnsw_rs persistence is raw file dumps with awkward lifetimes — the `Box::leak(HnswIo)`
  pattern here is the documented workaround), and hnsw_rs users genuinely lack it.
- **AGAINST (decisive):** the sidecar serializes `HnswAdapter`'s private maps (rid_map,
  tombstones, vectors) — i.e. it snapshots the *adapter*, not just the graph; it writes
  through the shamir `Store` trait; and it is pinned to `hnsw_rs =0.3.4` because hnsw_rs
  does not guarantee dump-format stability across patch releases — a public crate would
  inherit that fragility as its core value proposition. Extraction would require
  redesigning the sidecar around a user-supplied payload and publishing a storage trait.
- **Verdict:** not extractable in current shape. The *idea* is community-worthy; the code
  is not decoupled enough.

### Candidate 4c — tokenizers + BM25 (`tokenizer.rs` 469 LOC, `bm25.rs` 93 LOC)

- Nearly dependency-free (TFxSet → FxHashSet, StemLanguage enum, rust-stemmers), enum-
  dispatched, zero-copy on ASCII. But: tantivy's tokenizer ecosystem and several existing
  `bm25` crates already own this space, and 93 LOC of BM25 is textbook formula code.
- **Verdict:** not worth publishing; no in-tree testability problem either.

**Also checked:** `legacy/` index managers (shamir-domain, scheduled for replacement by
the backend system — extraction would be embalming), `posting_layout`/`fts_ranked_backend`
(bound to RecordId/posting key layout), `meta_envelope.rs` (68 LOC — too small, though
note it is now duplicated conceptually in wal_entry_v2's envelope; an *internal*
consolidation into shamir-types might be worth a chore task).

---

## Summary ranking

| # | Candidate | Source | New crate (proposal) | ~LOC | Non-shamir deps | Extraction cost | Community value |
|---|-----------|--------|----------------------|------|-----------------|-----------------|-----------------|
| 1 | Segmented two-tier WAL + group commit | `shamir-wal` (wal_segment, segment_set, wal_group_commit, wal_sink) | `tierwal` / `segwal` | ~1.8k + tests | tokio, bytes, crc32fast | Medium (genericize frame payload, replace DbError) | **High** — durability-tiered async WAL with liveness-proved group commit; classic reinvented infra |
| 2 | SSO byte-key | `shamir-storage/src/key_bytes.rs` | `keybytes` | ~1k with tests | bytes, serde, serde_bytes | **Trivial** (zero shamir imports) | Medium-high — Bytes-interop SSO key with serde parity guarantees |
| 3 | SQ8 quantizer + SIMD kernels | `shamir-index/src/vector/{simd,sq8,quantized_dist}.rs` | `sq8-dist` | ~1.4–1.8k | none (opt: hnsw_rs) | Low (flip pub(crate), internalize VectorMetric) | Medium — defensible via trained SQ8 scoring; kernel-only space is crowded (simsimd) |
| 4 | Completion watermark (+ versioned overlay) | `shamir-tx/{completion_tracker,versioned_overlay}.rs` | companion to #1 only | ~0.4k | scc, bytes, fxhash | Trivial | Low alone; medium bundled with WAL |

**Explicit non-findings:** `shamir-tx`'s MVCC core, `shamir-storage`'s cache decorators,
the HNSW snapshot codec, FTS tokenizers/BM25, and everything in `shamir-index/legacy/`
are either too coupled, too small, or outclassed by existing crates — extracting them
would be motion, not progress. None of the four crates has an in-place *testability*
problem justifying extraction on criterion (a): all already follow the `tests/`-directory
convention with substantial suites (e.g. key_bytes ships its own serde-identity gate,
the WAL ships fault-injection recovery tests) that run without cross-crate scaffolding.
