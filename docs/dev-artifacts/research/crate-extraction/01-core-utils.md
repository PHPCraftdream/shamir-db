# Crate-extraction research — batch 1: core utility crates

**Date:** 2026-07-16
**Scope:** `shamir-collections`, `shamir-types`, `shamir-tunables`, `shamir-numa`,
`shamir-bench-utils`, `shamir-query-builder-macros`, `shamir-sdk-macros`
**Question:** what, if anything, inside these crates should be extracted further —
(a) into a workspace-internal crate for testability/isolation, or (b) into a
published crates.io package (the `bench-scale-tool` precedent).

**Method:** read of each crate's `Cargo.toml`, full `src/` tree listing with LOC,
and the source of every candidate module. No code was modified.

---

## Verdict at a glance

| Candidate | Source | Proposed crate | Verdict |
|---|---|---|---|
| Whole crate `shamir-numa` | `crates/shamir-numa` | `numa-replicated` | **Strongest candidate** — publish as-is after removing one internal dep |
| Order-preserving key codec | `shamir-types/src/core/sort_codec.rs` | `sortable-key` (or similar) | Generic and clean, but **crowded niche** — publish only if differentiated |
| Concurrent interner | `shamir-types/src/core/interner/` | `arcswap-interner` | Technically extractable; **weak case** vs `lasso` |
| Clustered vector-dataset generator | `shamir-bench-utils/src/vector_data.rs` | `ann-bench-data` (or fold into `bench-scale-tool`) | Modest but real candidate — natural companion to the already-published harness |
| Everything else in scope | — | — | **Not worth extracting** (reasons per crate below) |

---

## 1. shamir-collections

**What it is.** A 63-line leaf crate of type aliases: `TMap`/`TSet`
(`IndexMap`/`IndexSet` over `FxHasher`), `TFxMap`/`TFxSet` (std `HashMap`/`HashSet`
over `FxHasher`), plus `new_*` constructor helpers. Deps: `indexmap`, `rustc-hash`
only. It exists precisely so guest-facing crates can share hasher policy without
pulling `shamir-types`.

**Extraction verdict: nothing to extract.** The crate *is already* the extraction —
a dependency-light leaf. As a crates.io package it would be ~40 lines of `pub type`
aliases, which any project writes in five minutes; there is no behavior, no tests
worth isolating, no community value beyond what `indexmap` + `rustc-hash` README
snippets already provide. Leave it exactly as it is.

---

## 2. shamir-types

**What it is.** The foundation crate (~5,700 LOC excl. tests; ~15,400 incl.): the
`Value`/`InnerValue`/`QueryValue` model, `RecordId`, base58 codec, the string
interner, MessagePack/bincode codecs (plain + interned-key variants), the zero-copy
`RecordView` lens, the `mpack!` literal macro, and the sort-key codec. Deps are
heavy (serde, rmp/rmpv, chrono, rust_decimal, num-bigint, dashmap, arc-swap, …).

Three sub-components merit individual analysis; the rest is domain glue.

### 2a. Candidate: `core/sort_codec.rs` → standalone `sortable-key`-style crate

- **Scope:** 154 LOC impl + `core/tests/sort_codec_tests.rs`. Pure functions:
  `encode_null/bool/i64/u64/f64/str/bytes` producing tag-prefixed, order-preserving,
  self-delimiting byte keys (sign-flip trick for i64, IEEE-754 total-order transform
  for f64, `0x00 0x01` escaping + `0x00 0x00` terminator for str/bytes, composable
  by concatenation).
- **Dependency footprint:** `thiserror` only. **Zero** `shamir-*` coupling — it does
  not even see `Value`; callers destructure into scalars first.
- **FOR:** This is exactly the kind of small, subtle, correctness-critical utility
  that benefits from standalone property-test hardening and that other KV/DB
  projects need (memcomparable encoding is a rite of passage every storage engine
  re-derives, usually with the string-terminator bug this file's doc-comment
  explains). It is fully generic, already documented to publication standard, and
  extraction costs one `use` rewrite in `shamir-index`/`shamir-engine`.
- **AGAINST (honest):** crates.io already has `memcomparable` (RisingWave, actively
  maintained, serde-integrated), `ordcode`, and `bytekey`-descendants covering the
  same ground with more types (decimal, datetime) and more eyes. A new 150-line
  entrant differentiates on nothing except "ours". Publishing would be maintenance
  surface for near-zero adoption. **Recommendation:** do NOT publish; optionally
  move it into `shamir-collections`-style leaf status only if a future crate
  (e.g. a standalone index library) needs it without `shamir-types` — today no
  such consumer exists, so leave in place.

### 2b. Candidate: `core/interner/` → standalone `arcswap-interner`

- **Scope:** ~820 LOC impl (`interner.rs` 561, `interned_key.rs` 148, `user_key.rs`
  66, `touch_ind.rs` 33) + a tests dir. A two-way `str ↔ u64` interner: forward
  direction sharded `DashMap`, reverse direction `ArcSwap<Vec<OnceLock<Arc<str>>>>`
  with doubling growth and a single-writer mutex for spine mutation — 100% lock-free
  reads. Includes hydration (`with_state`) and delta-extraction (`entries_after`,
  `entries_in_id_range`) used by WAL checkpointing.
- **Dependency footprint:** `dashmap`, `arc-swap`, `rustc-hash` (via
  `types::common::TDashMap` — a one-line alias, trivially inlined), `serde` on the
  key types. No deep `shamir-*` coupling; `InternerKey`/`UserKey` are self-contained
  newtypes living in the same module.
- **FOR:** The lock-free-read reverse-lookup design (ArcSwap spine + OnceLock leaves,
  documented race analysis for why the fully-lock-free grow was abandoned) is
  genuinely interesting and battle-tested here; the write-up in the struct doc is
  publication-quality. Extraction would also decouple `shamir-types`' most
  concurrency-sensitive component from its serde-heavy neighbors for isolated
  loom/stress testing.
- **AGAINST (honest):** `lasso` (ThreadedRodeo) is the established concurrent
  interner with a large user base, arena allocation, and `Spur` keys; `string-interner`
  covers the single-threaded case. Our differentiators — persistent-state hydration,
  monotonic u64 ids stable across restarts, delta extraction for checkpointing — are
  precisely the *database-shaped* parts, i.e. the parts a generic user does not
  want and `lasso` users do not miss. Stripped of those, we would be publishing a
  worse `lasso`. Testability-in-place is also fine: the module already has its own
  `tests/` dir and no upward deps. **Recommendation:** do not extract now. Revisit
  only if the WASM guest side ever needs the interner without `shamir-types`' heavy
  dep graph (chrono/decimal/bigint) — that would justify a workspace-internal
  `shamir-interner` leaf, not a crates.io publication.

### 2c. Non-candidates inside shamir-types (checked, rejected)

- **`record_view/` (RecordView lens, ~2,100 LOC)** — a zero-copy MessagePack lens
  is generically appealing on paper, but this one is welded to the interned-key
  storage format: map keys MUST be msgpack `bin`-encoded `InternerKey` LE bytes,
  ext markers carry Shamir type tags, and error semantics mirror the canonical
  decoder. Making it generic means parameterizing the key-matching strategy and
  ext handling — a redesign, not an extraction. Against: `rmpv`/`msgpacker` users
  who want zero-copy views have `mp_serde`-adjacent options; the coupling makes
  the honest answer "no".
- **`macros/mpack.rs` (353 LOC)** — a `json!`-style literal macro, but it emits
  `QueryValue` (Shamir's own value enum). Generic only if the value type were
  generic, which it is not. Reject.
- **`types/record_id.rs` + `types/base.rs`** — a 16-byte time-ordered id with
  custom epoch 2026-01-31, system-record prefix convention, base58 text form.
  ULID/UUIDv7 crates own this space; the custom epoch and system-prefix rules are
  domain policy. Reject.
- **Codecs (`codecs/basic`, `codecs/interned`)** — thin `rmp-serde`/`bincode`
  wrappers plus the interned-key encoder, which is inseparable from the interner
  and the record model. Reject.

---

## 3. shamir-tunables

**What it is.** 236 LOC: namespaced `const` knobs (`store_defaults`,
`instance_defaults`) + a 76-line `RuntimeTunables` struct of atomics initialized
from those consts.

**Extraction verdict: nothing to extract, plainly.** The constants ARE the domain
(WAL segment size, HNSW compaction ratios, connection limits) — meaningless outside
shamir-db. The `RuntimeTunables` pattern (atomic-backed runtime-overridable consts)
is a nice idiom but is ~20 lines of pattern per field; as a generic crate it would
need a derive macro to be worth anything, and crates like `arc-swap` + a struct
already serve that. Not a candidate.

---

## 4. shamir-numa — **the strongest extraction candidate in this batch**

**What it is.** ~950 LOC impl + ~250 LOC tests + a Linux integration test + a
real README. A NUMA-topology abstraction (`Topology` trait, `LinuxTopology` /sys
probe + `sched_setaffinity`, `FallbackSingleNodeTopology`, `MockTopology` DI test
double, `parse_cpulist`, `detect()`) plus `NodeReplicated<T>` — one cache-padded
`ArcSwap<T>` per NUMA node with node-local `load_local()` reads and CAS-linearised
copy-on-write `rcu()` writes, degrading to a bare-ArcSwap-equivalent single replica
on UMA machines. Consumed today by `shamir-index` (`IndexInfo`,
`SortedIndexManager`).

**Candidate: publish the whole crate** as e.g. `numa-replicated` or
`node-replicated-arcswap`.

- **Scope:** the entire crate — nothing shamir-specific in any file except naming
  and doc references.
- **Dependency footprint:** `arc-swap`, `thiserror`, `libc` (Linux-only). The single
  internal dep — `shamir-collections` used in `linux.rs` for the CPU→node Fx map —
  is a house-policy alias replaceable by a direct `rustc-hash` `HashMap` in one line.
  After that, zero `shamir-*` deps.
- **FOR:** This is the same shape as the `bench-scale-tool` success: self-contained,
  generically useful, already documented like a public crate (README with citations,
  three-tier test strategy including a QEMU CI harness in
  `scripts/ci-qemu-numa-test.sh`). `NodeReplicated<T>` fills a real gap: crates.io
  has hwloc bindings (`hwloc2`) and `libnuma` FFI for *topology*, and academic
  `node-replication` (the NR paper implementation, operation-log based, heavier
  semantics), but nothing lightweight offering "per-node ArcSwap replicas of
  read-mostly state with single-node degradation." The DI-mock topology trait also
  makes NUMA-aware code testable without hardware — an underserved pain point.
  Publishing forces the clean seam (no shamir deps) and gives the QEMU harness a
  second life as public CI.
- **AGAINST (honest):** The crate is young — Фаза 1b just landed; `NodeReplicated`
  has *no measured perf numbers on real multi-socket hardware yet* (README says so),
  and the eventual-consistency window between node-0 commit and mirror stores is a
  sharp edge a public audience will misuse. `CachePadded` duplicates
  `crossbeam-utils::CachePadded` (ours is 54 lines; a published version should just
  re-export or depend on crossbeam's). Windows/macOS topology detection is a stub
  (`detect()` always falls back to single-node), which public users will file
  issues about. **Recommendation:** extract when Фаза 2 (real consumers migrated)
  proves the API under load — publishing before that risks freezing a v0 API we
  still expect to bend. Concretely: swap `shamir-collections` → `rustc-hash` now
  (one-line, removes the last internal dep), keep the crate publication-shaped, and
  publish after multi-socket validation.

---

## 5. shamir-bench-utils

**What it is.** 490 LOC, two modules: `peak_mem` (feature-gated wrapper over the
`peak_alloc` global allocator — reset/measure/measure_async helpers, 110 LOC) and
`vector_data` (seeded clustered vector-dataset generator for ANN benchmarks:
deterministic LCG, K centroids in `[-1,1]^dim`, Box-Muller Gaussian noise,
round-robin balanced clusters, 363 LOC). Used by `shamir-index` and
`shamir-engine` dev-deps.

> **Housekeeping note:** CLAUDE.md's bench section claims `shamir_bench_utils`
> "predates the migration and is gone." It is not gone — it survived the
> bench-scale-tool migration in reduced form (its own lib.rs says so) and is still
> a dev-dep of two crates. CLAUDE.md should be corrected.

### 5a. Candidate: `vector_data.rs` → `ann-bench-data` (or a `bench-scale-tool` companion)

- **Scope:** ~363 LOC — `Lcg` (Numerical Recipes constants) + the clustered
  generator. Zero dependencies at all (not even `rand`).
- **FOR:** Reproducible, seedable, *clustered* (not uniform — the doc-comment
  correctly explains why uniform clouds flatter ANN recall) synthetic datasets are
  something every HNSW/IVF implementor needs for benches, and the standard
  alternative (SIFT/GIST HDF5 downloads a la ann-benchmarks) is heavyweight and
  awkward in `cargo bench`. Zero-dep, no_std-adjacent, deterministic across runs —
  publication-cheap. It also pairs naturally with the already-published
  `bench-scale-tool` as a second bench-infrastructure crate under the same
  maintenance umbrella.
- **AGAINST (honest):** It is small and niche; the audience is "Rust developers
  writing ANN benches who don't want real datasets" — dozens, not thousands. The
  cross-target f32 determinism caveat (relies on platform `ln`/`sqrt`) is a real
  limitation for its own headline feature. And the LCG constant is deliberately
  matched to shamir-index contract-test fixtures — a published crate must promise
  stream stability forever or break that lineage. **Recommendation:** borderline —
  worth publishing only if the maintainer wants to grow a small bench-tooling
  family around `bench-scale-tool`; otherwise leave in place.

### 5b. `peak_mem` — not a candidate

A thin convenience wrapper over the already-published `peak_alloc` crate
(reset + closure + read = ~30 lines of substance). The `#[global_allocator]`
inside a library is also a footgun that limits composability. Anyone who needs
this uses `peak_alloc` directly. Reject.

---

## 6. shamir-query-builder-macros

**What it is.** 1,546 LOC proc-macro crate: `filter!` (natural boolean expressions
→ `Filter` constructors, 19 predicate forms) and `q!` (SQL-like DSL → builder DTOs
for read/insert/update/delete/upsert/call).

**Extraction verdict: nothing extractable.** The macros exist to emit
fully-qualified `::shamir_query_builder::...` paths — the coupling to the builder
API is the entire product; the crate header itself notes it must not depend on the
builder only to avoid a cycle. The parsing layer (`query_parse.rs`, 1,019 LOC) is
a decent hand-rolled `syn` recursive-descent SQL-ish parser, but "SQL-like DSL
parser that lowers to *someone's* AST" is not separable from the AST it lowers to
without inventing a trait-abstracted target IR — a new project, not an extraction.
For testability it is already fine: proc-macros test via `trybuild`/expansion tests
in the consumer crate. No action.

---

## 7. shamir-sdk-macros

**What it is.** 571 LOC, one file: the `#[validator]` (and function-SDK) attribute
macro that rewrites an author's async fn into the ShamirDB WASM guest ABI —
`shamir_alloc`/`shamir_call` exports, msgpack param decode, `Validation` encode.

**Extraction verdict: nothing extractable.** Every emitted token references the
proprietary guest ABI (`shamir_call` signature, param map shape, `Ctx`,
`Validation`). The generic idea — "attribute macro that wraps an async fn in a
WASM export with serialized params" — is what `wit-bindgen`/`wasm-bindgen` already
own with actual standards behind them. Also note: the macro `panic!`s/`assert!`s on
author mistakes instead of emitting `syn::Error` spanned diagnostics — worth an
internal quality task someday, but not an extraction rationale. No action.

---

## Summary of recommendations

1. **`shamir-numa` → publish as `numa-replicated`** after (a) replacing its
   `shamir-collections` dep with direct `rustc-hash` (one line, `linux.rs`),
   (b) considering re-export of `crossbeam-utils::CachePadded` instead of the local
   copy, and (c) Фаза-2 validation on real multi-socket hardware. This is the only
   candidate in this batch with the same profile `bench-scale-tool` had: generic,
   self-contained, README-ready, fills a real crates.io gap.
2. **`shamir-bench-utils::vector_data` → optional publication** as a
   `bench-scale-tool` companion (`ann-bench-data`); zero-dep and cheap to publish,
   small audience. Maintainer's call.
3. **`sort_codec` and the interner:** clean, decoupled, and technically
   extractable — but crates.io incumbents (`memcomparable`, `lasso`) are better at
   the generic version of each. Keep in-tree; both already test well in place.
4. **No extraction** from `shamir-collections`, `shamir-tunables`,
   `shamir-query-builder-macros`, `shamir-sdk-macros` — trivial, pure-domain, or
   coupled-by-design respectively.
5. **Docs fix:** CLAUDE.md incorrectly states `shamir_bench_utils` is gone; it
   still exists and is a dev-dep of `shamir-index` and `shamir-engine`.
