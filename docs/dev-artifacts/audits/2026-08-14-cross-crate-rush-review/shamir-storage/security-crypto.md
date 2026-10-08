<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-storage — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

No local cryptographic or unsafe boundary was found. Exact dependencies resolve name and hashing uncertainties: name-limit panics are captured, numeric keyspace directories contradict traversal, and deterministic hashing does not establish a practical remote collision attack.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 5 | 1 | 0 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Store names passed to the durable engine unvalidated

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Published fjall 3.1.6 src/keyspace/name.rs checks only nonempty and &lt;=255 bytes; src/db.rs asserts it. Shim blocking tasks convert panic to Internal rather than typed validation. Control/path characters are accepted, but src/keyspace/mod.rs uses numeric IDs for directories. Authorized DDL is required; traversal/namespace bypass is refuted by this mechanism.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L234); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L40); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

<a id="review-2"></a>

### Claim 2 — Fresh random 128-bit id claim behind skipping the insert collision probe is false

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

RecordId::new combines wall-clock-relative timestamp with a 64-bit thread-local Xoshiro tail. It is not uniformly random, monotonic or unique-by-construction. No auth secrecy requirement or exact recovery threshold is established.

Evidence: [crates/shamir-types/src/types/record_id.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L24); [crates/shamir-types/src/types/record_id.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L41); [crates/shamir-storage/src/storage_fjall.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L154).

<a id="review-3"></a>

### Claim 3 — User-influenced keys enter non-keyed FxHash maps despite the documented no untrusted hash inputs premise

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Caller keys/names reach THasher. Exact rustc-hash 2.1.2 uses deterministic hash_bytes compression, not merely the older multiply-xor description. Legacy postings encode hashes; sorted keys encode values. Practical collision sets and remote amplification remain unverified.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L154); [crates/shamir-collections/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L17); [crates/shamir-index/src/base_index/sorted_index_manager.rs:2687](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/sorted_index_manager.rs#L2687); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007).

<a id="review-4"></a>

### Claim 4 — Raw key bytes — including attacker-influenced indexed values — embedded in error messages

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Missing-key errors format the full numeric byte slice, allowing large diagnostic strings for direct long-key callers. Encoded sorted values can appear as numbers; cross-tenant delivery and decoded Unicode spoofing are not established.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:419](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L419); [crates/shamir-storage/src/storage_in_memory.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L140); [crates/shamir-storage/src/key_bytes.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L237).

<a id="review-4-bidi"></a>

### Claim 4.BiDi — Printable Unicode/BiDi characters survive raw-key Debug rendering

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Debug delegates to &[u8], printing numerical byte elements rather than UTF-8 characters. Registered Debug parity tests directly pin this rendering; the alleged control-character mechanism does not exist.

Evidence: [crates/shamir-storage/src/key_bytes.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L237); [crates/shamir-storage/src/key_bytes/tests/debug_tests.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes/tests/debug_tests.rs#L17).

<a id="review-5"></a>

### Claim 5 — Nit: KeyBytes::Deserialize allocates an unbounded blob before any size check

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Deserialize materializes ByteBuf before constructing/copying KeyBytes, without its own size cap. Exact serde_bytes 0.11.19 visit_seq caps initial reservation at 4096 but permits subsequent growth; byte-buffer dispatch depends on the decoder. WAL uses Bytes, so no direct remote KeyBytes decoder is proven.

Evidence: [crates/shamir-storage/src/key_bytes.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L310); [crates/shamir-storage/src/key_bytes.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L111); [crates/shamir-wal/src/wal_entry_v2.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L87); [Cargo.lock:3214](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3214).

<a id="review-nf-boundary"></a>

### Claim NF-boundary — No auth/crypto/TLS surface, no unsafe blocks, and no local secret comparison

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Local code has no auth/TLS primitive, unsafe block or demonstrated secret comparison. Ordinary key equality has no established constant-time obligation here; dependencies may contain unsafe code.

Evidence: [crates/shamir-storage/src/lib.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/lib.rs#L16); [crates/shamir-storage/src/key_bytes.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L250).

<a id="review-nf-hydration"></a>

### Claim NF-hydration — MirroredStore hydration re-filters classifier drift/tampered mirror entries

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Construction checks the classifier before primary publication and warns on rejection. Registered test checks a rejected Count key, valid control and warning count. The historical repair proves classification, not allowed-value authenticity or automatic inventory exhaustiveness.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:276](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L276); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:244](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L244); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:1029](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L1029).

## Evidence and recipe corrections

- Exact name behavior is available at https://docs.rs/crate/fjall/3.1.6/source/src/keyspace/name.rs. Its character-restriction comment is broader than its implementation; only byte length/nonempty is checked.
- Do not canonicalize existing accepted names or impose ASCII-only policy without compatibility and authorization analysis. Validate actual backend limits and distinguish captured task panic from process termination.
- Exact FxHasher source is https://docs.rs/crate/rustc-hash/2.1.2/source/src/lib.rs. Deterministic hashing supports the trust-premise concern, not the historical claim of trivially manufactured practical collisions.
- A post-ByteBuf length check cannot prevent earlier allocation. Decoder limits and public constructor policy are separate; neither a universal allocation bypass nor universal protection is established.
- Classifier tests use a manually maintained tag inventory; illustrative posting/sorted-key shapes do not match all actual production encodings.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage -- Security & crypto boundary

## Summary

The crate holds no auth / crypto / TLS surface (SCRAM, HMAC and session handling live in `shamir-connect` / `shamir-server`) and contains **zero `unsafe` blocks** (crate-wide grep: the only hit is a doc sentence in `key_bytes.rs` explaining why an `unsafe` union layout was *rejected*). The real untrusted-input boundary it owns is tampered on-disk mirror content at hydration, which is handled well (`MirroredStore::new` re-runs the allowlist classifier against every streamed entry and skips+warns on drift/tampering -- `storage_mirrored.rs:276-284`; backed by a classifier-exhaustiveness test and a hydration-drift test). Residual risks found are premise gaps rather than standalone exploits: unvalidated store names handed to the durable engine, a load-bearing "random 128-bit id" comment that the referenced implementation contradicts, and user-influenced keys entering non-keyed-FxHash maps despite the workspace's stated "no untrusted hash inputs" assumption. Timing side-channels: nothing in this crate compares secret material, so its non-constant-time slice equality (`KeyBytes`) is not exploitable as written.

## Findings

### 1. Store names passed to the durable engine unvalidated
**File:** `crates/shamir-storage/src/storage_fjall.rs:229-245` (`FjallRepo::store_get`), `:247-262` (`store_delete`)
**Severity:** medium (conditional -- see below)
**Issue:** Both methods forward `name.as_ref()` straight into `Database::keyspace(&table_name, ...)` (and `delete_keyspace`) with no validation whatsoever: empty string, whitespace-only, control characters, absurd length, delimiter/path-flavored characters, or names engineered to collide with the engine's composed prefixes (`__data__<t>` / `__info__<t>` / `__history__<t>`) are all accepted and become durable on-disk artifacts. `copy_store` (types.rs:488) composes onto this unchecked too. Whether fjall internally rejects pathological names was not verified; the crate neither relies on nor documents any guarantee, so the boundary simply trusts every caller.
**Failure scenario:** If a client-controlled DDL table name reaches `Repo` uncanonicalized (validation lives outside this crate), one request can mint or delete persistent storage artifacts under manipulated names, alias across composed store namespaces after a rename cycle, or wedge `stores_list()` consumers with invisible/control-character names.
**Suggested fix:** Validate once at the `Repo` boundary -- reject empty, over-length, and non-printable/non-ASCII names with `DbError::Validation`; canonicalize before calling fjall. Cheap O(1) guard on a cold path, converts a transitive trust assumption into a checked invariant.

### 2. "Fresh random 128-bit id" claim behind skipping the insert collision probe is false (timestamp + 64-bit PRNG tail)
**File:** `crates/shamir-storage/src/storage_fjall.rs:152-156` (`exec_insert`) and `:324-329` (`Store::insert`); same claim in `benches/storage_fjall_pump.rs:96-101`
**Severity:** low
**Issue:** Both comments justify dropping the pre-insert `contains_key` probe with "`RecordId::new()` is a fresh random 128-bit id ... ~2^-128". The referenced implementation says otherwise (`shamir-types/src/types/record_id.rs:24-54`, `:80-90`): bytes `[0..8]` are wall-clock microseconds (fully predictable), bytes `[8..16]` come from a thread-local **Xoshiro256++** -- deliberately *not* a CSPRNG, seeded once per thread from OS RNG. So predictability is 50% of the id and the random part carries 64 bits from an xoshiro stream that is computationally invertible/predictable after ~32 observed consecutive outputs per thread. The engineering *decision* (skip the probe) remains sound -- distinct-microsecond timestamps dominate separation and the same-microsecond tail birthday bound is ample -- but the written security argument overstates it by 2^64 and by PRNG strength, and other files repeat it.
**Failure scenario:** Today nothing authenticates on id unguessability, so impact is latent. If any future feature starts treating record keys as opaque unguessable tokens (share links, presigned-style record URLs, lottery-on-key), this comment will have waved the design through under a "128-bit random" justification that the implementation does not provide.
**Suggested fix:** Correct the comments in place: "monotonic-ts-prefixed id with a 64-bit Xoshiro256++ tail; unique-by-construction for insert, **not a secret / not CSPRNG-backed**". One-line edits, keeps the perf decision intact while deleting the misleading premise.

### 3. User-influenced keys enter non-keyed FxHash maps despite the documented "no untrusted hash inputs" premise
**File:** `crates/shamir-storage/src/storage_membuffer.rs:154` (field) and `:298-299` (construction): `dirty: DashMap<RecordKey, Slot, THasher>`; `crates/shamir-storage/src/storage_in_memory.rs:18,24`: `stores: TDashMap<String, _>` keyed by store name
**Severity:** low
**Issue:** CLAUDE.md pillar 4 trades away RandomState DOS protection because "we don't accept untrusted hash inputs here". Two structures in this crate break that premise in spirit: `dirty` is keyed by *caller-supplied* `RecordKey`s, and secondary-index posting keys embed indexed field values verbatim (per `storage_mirrored.rs:165-172`'s own key-shape description), i.e. attacker-chosen bytes on an ingestion path; `InMemoryRepo.stores` is keyed by table/store names that ultimately originate in DDL. FxHash is a multiply-xor construction whose collisions are trivially mass-manufactured, unlike SipHash.
**Failure scenario:** A writer flooding crafted colliding posting keys while the write-back buffer sits undrained (default 500 ms tick) concentrates those entries into one dashmap shard; subsequent `set`/`get` probes and `snapshot_overlay_sorted`'s clone+sort of the overlay (`storage_membuffer.rs:576-595`) skew superlinearly on that shard -- a bounded but measurable remote write-path latency amplifier. `InMemoryStore.data`'s `scc::TreeIndex` (ordered B+-tree) is unaffected; only the two hash maps above are exposed.
**Suggested fix:** Either (a) document why the premise still holds (e.g. prove posting values are canonicalized/length-capped upstream such that collision farming is pointless), or (b) give just these two externally-influenced maps a keyed BuildHasher (SipHash/RandomState) -- their access patterns are buffered/write-side, not the ultra-hot lock-free reads pillar 4 optimizes for.

### 4. Raw key bytes -- including attacker-influenced indexed values -- embedded in error messages
**File:** `crates/shamir-storage/src/storage_fjall.rs:419`; `crates/shamir-storage/src/storage_in_memory.rs:111,140`; `crates/shamir-storage/src/storage_membuffer.rs:797,810`
**Severity:** low
**Issue:** `DbError::NotFound(format!("record not found: {:?}", key))` (and `KeyExists`) interpolate the full key via `Debug`. Posting keys carry indexed field values of *other* columns' data; these error strings flow up through engine/wire layers and log aggregation. Rust's `escape_debug` escapes `\n`/`\r`/controls (so single-line logs hold and forgery-via-newline is blocked), but it leaves printable Unicode intact -- including BiDi overrides (U+202E etc.) and zero-width characters -- so log/terminal spoofing and cross-record value leakage via error text are both possible.
**Failure scenario:** A query touching a missing posting key surfaces fragments of some record's indexed value inside an error string shown to a different tenant/console; or renders convincingly-reversed log lines via injected BiDi characters inside a crafted indexed value.
**Suggested fix:** For these specific call sites, render keys as bounded hex (`hex(&key[..min(key.len(), 16)])` style helper) instead of `{:?}` -- one small local formatter, no API change.

### 5. Nit: `KeyBytes::Deserialize` allocates an unbounded blob before any size check
**File:** `crates/shamir-storage/src/key_bytes.rs:308-313`
**Severity:** nit
**Issue:** Deserialization goes through `serde_bytes::ByteBuf::deserialize`, materializing the entire input allocation before `from_slice` runs; there is no maximum-length guard. Today safe (callers are WAL/bincode/rmp-serde boundaries that own frame limits, and the type is unused by production per module docs), but plan doc section 5.3 anticipates flipping `RecordKey` to `KeyBytes` across the WAL/client-wire paths -- at that point a hostile frame chooses the pre-allocation size subject only to upstream framing.
**Suggested fix:** When the alias flip lands, gate the constructor: deserialize, then reject `len > MAX_RECORD_KEY_BYTES` (tie to schema/tunable constants) returning a `de::Error::invalid_length`.

</details>
