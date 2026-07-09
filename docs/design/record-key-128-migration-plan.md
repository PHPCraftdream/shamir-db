בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# RecordKey → inline small-key migration plan (task #491)

_Analysis for audit finding 3.1 of `docs/audits/2026-07-06-perf-radical-o-notation.md`
("`RecordKey = Bytes` — heap-аллокация и косвенность на каждый 16-байтовый ключ").
Read-only investigation, 2026-07-09. Implementation is delegated to follow-up /crush tasks._

---

## 1. Current state (with citations)

### 1.1 The type itself

- `crates/shamir-storage/src/types.rs:8` — `pub type RecordKey = Bytes;`
  A **type alias**, not a newtype. Every consumer sees `bytes::Bytes` directly and
  is free to use the full `Bytes` API.
- `crates/shamir-types/src/types/record_id.rs:21` — `pub struct RecordId(pub [u8; 16]);`
  The logical primary key of every data record. Layout: `[0..8] BE relative-µs
  timestamp | [8..16] random/seq tail` (`from_ts`, `from_ts_seq`). BE timestamp ⇒
  **lexicographic byte order == chronological order** — load-bearing for range scans.
- `record_id.rs:115-117` — `RecordId::to_bytes()` = `Bytes::copy_from_slice(&self.0)`
  → **one heap allocation per conversion**. This is the alloc the audit targets.

### 1.2 CRITICAL FACT: RecordKey is NOT always 16 bytes

The same `RecordKey` type keys *every* store behind the one `Store` trait
(`types.rs:30`), and the stores use materially different key shapes:

| Producer | Key shape | Citation |
|---|---|---|
| Data stores (`__data__<t>`) | `RecordId` — exactly 16 B | `table_manager_crud.rs:107,223,299`, `drainer.rs:431-435`, `recovery.rs:81-111` |
| Legacy index postings | `index_key(25 B) ‖ record_id(16 B)` = **41 B** | `crates/shamir-index/src/legacy/index_keys.rs:306-311` |
| Unique-index keys | `IndexRecordKey::to_bytes()` = **25 B** | `crates/shamir-index/src/legacy/index_record_key.rs:84-92` |
| Index prefix scans | **9 B** prefix | `index_record_key.rs:95-101` |
| WAL active markers | `"__wal_active_" ‖ BE u64` = **21 B** | `crates/shamir-wal/src/active_key.rs:15-39` |
| Vector-index snapshots | arbitrary strings (`"{ks}.manifest"`, `"{ks}.g{gen}.sidecar"`, `"{ks}.delta."`) | `crates/shamir-index/src/vector/snapshot.rs:164-168,1162` |
| Migration shadow keys | typed wrapper → Bytes | `crates/shamir-engine/src/migration/shadow_key.rs:38` |
| Info/meta stores | system `RecordId` (16 B, zero-prefix) | `recovery_marker.rs:49-67`, `validator/persistence.rs:28-40`, `buffer_config.rs:20` |
| Tests | 0-byte, 2-byte, 42-byte keys explicitly | `crates/shamir-tx/src/tests/staging_store_tests.rs:184-186` |

**Verdict on the audit's premise:** "фактический ключ данных — всегда RecordId" is
true only for *data* stores. The Store API carries variable-length keys from 0 to
41+ bytes plus arbitrary strings. A bare `Key128(u128)` **cannot replace**
`RecordKey`; it can only be an inline fast path inside a small-buffer key type,
or a data-store-only local type.

### 1.3 Where keys flow (blast radius)

- **Store trait** (`types.rs:30-262`): `insert/set/get/get_many/remove/*_many/transact`
  + three stream methods yielding `Vec<(RecordKey, Bytes)>`. Used as `Arc<dyn Store>`
  everywhere — the trait is **object-safe and non-generic**; a `Store<K>` redesign
  would break dyn dispatch across engine/tx/index/wal and is not on the table.
- **Backends**: `storage_in_memory.rs` (TreeIndex keyed by RecordKey),
  `storage_fjall.rs` (passes `&key[..]` to fjall — backend sees raw bytes only),
  `storage_cached.rs` (cache map keyed by RecordKey), `storage_membuffer.rs`.
- **shamir-tx**: `staging_store.rs:81` — `TMap<RecordKey, StagedOp>` (Fx-hashed);
  `tx_context.rs:548-577` — conflict detection over `(u64, &RecordKey)` sets.
  These are the *hottest* in-memory key consumers.
- **shamir-engine**: ~40 production call sites doing `rid.to_bytes()` /
  `RecordId::system(..).to_bytes()` (grep above); MVCC paths
  (`read_temporal.rs:98,294`, `read_index_scan.rs:115`) pass `&id.to_bytes()`.
- **shamir-wal**: `wal_entry_v2.rs` — data ops carry a typed `RecordId`
  (serialized as 16 raw bytes via serde); generic KV ops carry `Bytes` via
  `serde_bytes`. WAL format is defined in terms of these serde encodings.
- **Client/FFI**: `RecordId` crosses the wire as base58 string / 16-byte blob;
  `RecordKey` itself is **not** a client-visible type. TS client and napi bindings
  are unaffected by an internal representation change.
- **Raw-byte reach-ins**: range/prefix filtering compares `k.as_ref()` slices
  (`types.rs:307-317`), fjall converts via `&key[..]`, `WalActiveKey::parse(&[u8])`,
  `RecordId::try_from_bytes(&[u8])`. All go through `Deref/AsRef<[u8]>` — i.e. the
  raw-byte API surface is **slice-shaped, not Bytes-method-shaped**, with a handful
  of exceptions (`Bytes::from_static` in tests, `RecordKey::from(vec)`, `.clone()`).

### 1.4 On-disk / on-wire impact

fjall persists exactly the bytes we hand it; the WAL serializes `RecordId` as a
16-byte blob and generic keys through `serde_bytes`. **As long as the new key type
(a) produces byte-identical `[u8]` views and (b) serde-serializes as the same
byte-blob encoding, there is NO on-disk or on-wire format change and NO migration
tool or format-version bump is needed.** The existing
`crates/shamir-index/src/legacy/tests/index_manager_tests/byte_identity_tests.rs`
suite is the guard for the index side.

---

## 2. Feasibility verdict

- **Full `Key128(u128)` replacement: NOT feasible.** Variable-length keys are
  first-class citizens of the same Store API (§1.2).
- **`enum RecordKey { Id(u128), Raw(Bytes) }` (audit's alternative): feasible but
  wrong shape.** A tagged enum whose variants are semantic ("Id vs Raw") invites
  Eq/Ord/Hash divergence between a 16-byte `Raw` and the equivalent `Id` — exactly
  the #489-class landmine. The correct shape is a **representation-transparent
  small-buffer byte string** (SSO, like `SmallVec`/`smol_str` for keys): inline
  storage for keys ≤ N bytes, heap `Bytes` beyond, with Eq/Ord/Hash defined
  *solely* over the byte slice so inline-vs-heap is unobservable.
- **Tractability: good, because `RecordKey` is a pub alias.** Swapping the alias to
  a newtype is one compile-driven cutover; call sites that already treat keys as
  opaque comparable/slice-like values (the vast majority, §1.3) need zero or
  mechanical changes. The cutover commit is large-ish but mechanical; the risky
  logic (the type itself) lands earlier as a fully-tested, zero-call-site step —
  matching the campaign's #488→#499 scope-down pattern.
- **Win**: eliminates the alloc in every `rid.to_bytes()` (data-store point ops,
  drainer/recovery batches, MVCC lookups, staging-map keys), gives inline compare
  + Fx-hash over registers for ≤N-byte keys. Posting keys (41 B) stay heap unless
  the cap is raised — acceptable; they are built once per posting write, not per
  comparison.

## 3. Proposed type design

New file `crates/shamir-storage/src/key_bytes.rs` (one primary export per file):

```rust
/// Small-string-optimized key: inline for len <= INLINE_CAP, heap `Bytes` beyond.
/// Representation is UNOBSERVABLE: Eq/Ord/Hash/serde are defined over the byte
/// slice only. Total size 32 bytes = same as `bytes::Bytes` on x86-64.
pub struct KeyBytes(Repr);

const INLINE_CAP: usize = 30; // covers 16B RecordId, 21B WAL, 25B unique-index, 9B prefix

enum Repr {
    Inline { len: u8, buf: [u8; INLINE_CAP] },
    Heap(Bytes),
}
```

Required trait/API surface (drives the mechanical cutover):
- `Deref<Target = [u8]>`, `AsRef<[u8]>`, `Borrow<[u8]>` — covers `k.as_ref()`
  comparisons, fjall's `&key[..]`, `parse(&[u8])`, `try_from_bytes`.
- `Clone` (inline = memcpy 32 B; heap = refcount bump), `Debug`.
- `PartialEq/Eq/PartialOrd/Ord/Hash` — **delegating to `self.as_slice()`**, never
  derived over the enum. Plus `PartialEq<[u8]>` conveniences.
- `From<Bytes>`, `From<Vec<u8>>`, `From<&'static [u8]>` (inline-copies when it
  fits; `from_static` test call sites keep compiling via `From<Bytes>`),
  `impl From<KeyBytes> for Bytes` (heap: zero-copy; inline: one copy — used only
  at cold boundaries).
- `KeyBytes::from_slice(&[u8])` — the workhorse constructor; inline when it fits.
- `Serialize/Deserialize` via `serde_bytes`-style byte-blob — **byte-identical to
  the current `Bytes` encoding** (guarded by a round-trip-vs-Bytes test).
- `RecordId::to_key(&self) -> RecordKey` in shamir-types is NOT possible
  (dependency direction: storage depends on types). Instead add in shamir-storage:
  `impl From<&RecordId> for KeyBytes` + free fn `record_key(id: &RecordId)` — or,
  simpler, callers use `KeyBytes::from_slice(id.as_bytes())` (alloc-free).

Then flip the alias: `pub type RecordKey = KeyBytes;` (`types.rs:8`). The `Store`
trait, `KvOp`, streams, staging maps, overlay, caches all switch representation
with no signature changes. No `u128` appears anywhere — byte-array inline repr
sidesteps every endianness question (§5.2).

## 4. Migration sequence (each step = one /crush task, one commit)

1. **`KeyBytes` type + exhaustive tests, zero call-site changes.**
   Add `key_bytes.rs` + `tests/key_bytes_tests.rs` in shamir-storage. TDD targets:
   (a) property test — for random byte strings of len 0..64, `KeyBytes` and `Bytes`
   agree on Eq/Ord (total order = lexicographic) and produce equal `as_ref()`;
   (b) Hash consistency — inline and forced-heap constructions of the same bytes
   hash identically under `FxHasher` (construct heap variant of a short key via a
   test-only hook or `From<Bytes>` on a non-inlineable path — expose
   `#[cfg(test)] fn heap_for_test`); (c) serde: `bincode`/`rmp` encodings of
   `KeyBytes` == encodings of equivalent `Bytes`; (d) size assertion
   `size_of::<KeyBytes>() == 32`. Nothing else changes; independently green.
2. **Alias cutover: `type RecordKey = KeyBytes` + mechanical call-site fixes.**
   One workspace-wide compile-driven commit. Expected fix classes: test literals
   (`Bytes::from_static(b"k1")` → `RecordKey::from(..)`), explicit
   `Bytes` ↔ `RecordKey` boundary conversions in backends
   (`storage_fjall.rs`, `storage_in_memory.rs`, `storage_cached.rs`,
   `storage_membuffer.rs`), stream item construction, `active_key.rs:37-39`,
   `shadow_key.rs:38`, vector `snapshot.rs` string keys. Verification:
   `./scripts/test.sh --full` at orchestrator level; byte-identity suite
   (`byte_identity_tests.rs`) must stay green — it proves on-disk keys unchanged.
   NO behavioral change intended; the diff must contain no logic edits.
3. **Alloc-free hot-path constructors.** Replace `rid.to_bytes()` with
   `RecordKey::from_slice(rid.as_bytes())` (or a `record_key(id)` helper) at the
   hot engine/tx call sites: `table_manager_crud.rs`, `drainer.rs:431-435`,
   `tx/recovery.rs`, `read_temporal.rs`, `read_index_scan.rs`, `table.rs`,
   `table_manager_streaming.rs`, staging-store writers. Optionally deprecate
   `RecordId::to_bytes()` for key construction (keep for genuine Bytes needs).
   Bench: `engine_perf` + `storage_*_pump` + `posting_cache_hit` before/after
   (CARGO_TARGET_DIR bench isolation per CLAUDE.md).
4. **In-memory backend key maps.** Confirm `InMemoryStore`'s `TreeIndex<RecordKey,..>`
   and `CachedStore`'s map benefit automatically (they do once the alias flips);
   sweep for any remaining `Bytes::copy_from_slice(id.as_bytes())` (e.g.
   `storage_fjall.rs:102` insert path) and residual `.to_bytes()` in
   `interner_manager.rs` / `record_counter.rs` / meta paths (cold, but free).
5. **(Optional, measure first) Raise INLINE_CAP or add a 41-byte posting-key tier**
   if posting-write benches show the heap fallback matters. Separate task; do not
   bundle with 1–4.

Steps 1 and 2 are the gate; 3–4 are incremental per-crate wins that can land in
any order after 2. If step 2's diff turns out unexpectedly large, it can be split
per-crate only by temporarily giving `KeyBytes` `From/Into<Bytes>` shims — but the
alias makes an atomic cutover the cheaper path.

## 5. Landmines and mitigations

1. **Hash/Eq/Ord divergence between inline and heap representations** (the
   #489-class bug). If `#[derive(PartialEq, Hash)]` lands on the `Repr` enum, a
   16-byte key built inline ≠ the same bytes arriving as `Heap(Bytes)` (e.g. a key
   read back from a fjall scan) → staging-map misses, conflict-detection false
   negatives, cache-key duplication — silent data corruption. **Mitigation:**
   hand-written impls over `as_slice()` only; the step-1 property tests
   (inline-vs-heap Eq/Ord/Hash agreement) are mandatory red-first tests.
2. **Endianness / ordering.** `RecordId` BE-timestamp prefix and `WalActiveKey` BE
   txn_id make lexicographic byte order semantically load-bearing (`iter_range_stream`
   contract, `types.rs:216-217`; recovery ordering, `active_key.rs:8-10`). A
   `u128`-based repr compared numerically only matches byte order via
   `from_be_bytes` on big-endian... **Mitigation: no `u128` at all** — inline repr
   is `[u8; N]`, `Ord` is slice `Ord`. Ordering is byte-for-byte identical by
   construction.
3. **Serde format drift.** `KvOp`/keys reach bincode (WAL generic ops,
   `wal_entry_v2.rs:87-99` via `serde_bytes`) — if `KeyBytes` derives Serialize
   over the enum, the wire/disk encoding changes (tag byte + fixed array) and old
   WALs stop replaying. **Mitigation:** serialize as a plain byte blob identical
   to `serde_bytes` encoding of `Bytes`; step-1 test (c) compares encodings
   against `Bytes` byte-for-byte for both bincode and rmp-serde.
4. **Bytes-specific API reach-ins.** `Bytes::from_static` (tests),
   `RecordKey::from(vec)`, `slice()/split_off` (none found on keys in production
   code, but verify during step 2), zero-copy `Bytes::from(slice)` in the fjall
   read path (post-#486). **Mitigation:** mirror the needed `From` impls; for the
   fjall read path convert `fjall::Slice → Bytes → KeyBytes` — for ≤30 B keys the
   inline copy is cheaper than the refcount anyway; for >30 B it's `Heap(bytes)`
   zero-copy.
5. **Clone-cost inversion.** `Bytes::clone()` is a refcount bump regardless of
   length; inline clone is a 32 B memcpy (cheaper), but code that clones *large*
   keys in loops keeps `Heap` refcount semantics — no regression. Verify with
   `posting_cache_hit` + `storage_cached_pump` benches in step 3.
6. **`is_unique()` / other `Bytes` niche methods and `impl PartialEq<Bytes>`
   assumptions in tests** — sweep in step 2; tests comparing `RecordKey` against
   `Bytes` literals need `PartialEq<Bytes> for KeyBytes` (slice-delegating) to
   stay ergonomic.
7. **`RecordId` itself must NOT change** — it is client-visible (base58, serde
   16-byte blob, `record_id.rs:158-178`). This plan touches only the storage-layer
   key representation; `RecordId` stays `[u8; 16]`.
