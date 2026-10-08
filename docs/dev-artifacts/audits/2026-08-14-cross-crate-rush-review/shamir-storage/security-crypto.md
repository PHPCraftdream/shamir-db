<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-storage — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

No local crypto/unsafe implementation was found. Name handling and hash-input trust assumptions need qualification; raw-key rendering exists, but the alleged Unicode spoofing mechanism is positively refuted.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 4 | 1 | 0 | 1 | 1 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Store names passed to the durable engine unvalidated

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

The shim forwards names without validation, and authorized table DDL preserves caller names. However, whether pinned fjall accepts pathological names, panics, rejects them, or permits namespace/path exploitation is unverified. This is not an established remote traversal or authorization bypass.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:229](../../../../../crates/shamir-storage/src/storage_fjall.rs#L229); [crates/shamir-storage/src/storage_fjall.rs:247](../../../../../crates/shamir-storage/src/storage_fjall.rs#L247); [crates/shamir-db/src/shamir_db/execute/admin_table_index.rs:40](../../../../../crates/shamir-db/src/shamir_db/execute/admin_table_index.rs#L40); [crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:36](../../../../../crates/shamir-db/src/shamir_db/shamir_db/table_management.rs#L36); [crates/shamir-engine/src/table/table_config.rs:8](../../../../../crates/shamir-engine/src/table/table_config.rs#L8); [Cargo.lock:1332](../../../../../Cargo.lock#L1332).

<a id="review-2"></a>

### Claim 2 — Fresh random 128-bit id claim behind skipping the insert collision probe is false

Status: `confirmed-open`. Current risk: `low`.

The comments still claim random 128-bit IDs and approximately 2^-128 collisions. RecordId::new instead uses wall-clock timestamp bytes plus a 64-bit Xoshiro tail. No local authentication relies on secrecy; the report's exact PRNG-recovery threshold is unverified.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:154](../../../../../crates/shamir-storage/src/storage_fjall.rs#L154); [crates/shamir-storage/src/storage_fjall.rs:326](../../../../../crates/shamir-storage/src/storage_fjall.rs#L326); [crates/shamir-storage/benches/storage_fjall_pump.rs:96](../../../../../crates/shamir-storage/benches/storage_fjall_pump.rs#L96); [crates/shamir-types/src/types/record_id.rs:24](../../../../../crates/shamir-types/src/types/record_id.rs#L24); [crates/shamir-types/src/types/record_id.rs:41](../../../../../crates/shamir-types/src/types/record_id.rs#L41); [crates/shamir-types/src/types/record_id.rs:80](../../../../../crates/shamir-types/src/types/record_id.rs#L80).

<a id="review-3"></a>

### Claim 3 — User-influenced keys enter non-keyed FxHash maps despite the documented no untrusted hash inputs premise

Status: `confirmed-open`. Current risk: `low`.

dirty accepts caller-supplied keys and the repository map accepts caller-derived names through THasher. This contradicts the blanket trust premise. Legacy posting keys contain hashes rather than verbatim values; sorted keys contain encoded values. Collision-farming practicality and the claimed remote latency amplifier remain unverified.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:154](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L154); [crates/shamir-storage/src/storage_membuffer.rs:763](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L763); [crates/shamir-storage/src/storage_in_memory.rs:42](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L42); [crates/shamir-types/src/types/common.rs:8](../../../../../crates/shamir-types/src/types/common.rs#L8); [crates/shamir-collections/src/lib.rs:17](../../../../../crates/shamir-collections/src/lib.rs#L17); [crates/shamir-index/src/base_index/index_record_key.rs:102](../../../../../crates/shamir-index/src/base_index/index_record_key.rs#L102); [crates/shamir-index/src/base_index/sorted_index_manager.rs:2687](../../../../../crates/shamir-index/src/base_index/sorted_index_manager.rs#L2687); [Cargo.lock:3007](../../../../../Cargo.lock#L3007).

<a id="review-4"></a>

### Claim 4 — Raw key bytes — including attacker-influenced indexed values — embedded in error messages

Status: `confirmed-open`. Current risk: `low`.

Errors still include the entire key, making unbounded key disclosure/log volume a real hygiene concern. KeyBytes Debug renders numeric byte arrays, not decoded Unicode, so BiDi/newline spoofing through the alleged rendering mechanism is refuted. Cross-tenant disclosure was not demonstrated.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:419](../../../../../crates/shamir-storage/src/storage_fjall.rs#L419); [crates/shamir-storage/src/storage_in_memory.rs:140](../../../../../crates/shamir-storage/src/storage_in_memory.rs#L140); [crates/shamir-storage/src/storage_membuffer.rs:797](../../../../../crates/shamir-storage/src/storage_membuffer.rs#L797); [crates/shamir-storage/src/key_bytes.rs:233](../../../../../crates/shamir-storage/src/key_bytes.rs#L233); [crates/shamir-storage/src/key_bytes/tests/debug_tests.rs:17](../../../../../crates/shamir-storage/src/key_bytes/tests/debug_tests.rs#L17).

<a id="review-4-bidi"></a>

### Claim 4.BiDi — Printable Unicode/BiDi characters survive raw-key Debug rendering

Status: `refuted`. Current risk: —.

Debug explicitly delegates to &[u8] formatting, and a registered test compares its output to byte-slice Debug. UTF-8 bytes are printed as numeric elements, not terminal control characters.

Evidence: [crates/shamir-storage/src/key_bytes.rs:237](../../../../../crates/shamir-storage/src/key_bytes.rs#L237); [crates/shamir-storage/src/key_bytes/tests/debug_tests.rs:17](../../../../../crates/shamir-storage/src/key_bytes/tests/debug_tests.rs#L17); [crates/shamir-storage/src/key_bytes/tests/mod.rs:7](../../../../../crates/shamir-storage/src/key_bytes/tests/mod.rs#L7).

<a id="review-5"></a>

### Claim 5 — Nit: KeyBytes::Deserialize allocates an unbounded blob before any size check

Status: `confirmed-open`. Current risk: `nit`.

ByteBuf is fully deserialized before from_slice, with no key-size policy; long inputs are then copied into a second heap buffer. The alias has already flipped, but inspected WAL serialization still uses Bytes, so a direct remote KeyBytes decoder exposure was not established.

Evidence: [crates/shamir-storage/src/key_bytes.rs:308](../../../../../crates/shamir-storage/src/key_bytes.rs#L308); [crates/shamir-storage/src/key_bytes.rs:310](../../../../../crates/shamir-storage/src/key_bytes.rs#L310); [crates/shamir-storage/src/key_bytes.rs:111](../../../../../crates/shamir-storage/src/key_bytes.rs#L111); [crates/shamir-storage/src/types.rs:9](../../../../../crates/shamir-storage/src/types.rs#L9); [crates/shamir-wal/src/wal_entry_v2.rs:87](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L87); [crates/shamir-wal/src/wal_entry_v2.rs:121](../../../../../crates/shamir-wal/src/wal_entry_v2.rs#L121).

<a id="review-nf-boundary"></a>

### Claim NF-boundary — No auth/crypto/TLS surface, no unsafe blocks, and no local secret comparison

Status: `not-applicable`. Current risk: —.

These local observations hold. Ordinary byte equality is not a demonstrated secret-dependent security boundary in this crate.

Evidence: [crates/shamir-storage/src/lib.rs:16](../../../../../crates/shamir-storage/src/lib.rs#L16); [crates/shamir-storage/src/key_bytes.rs:43](../../../../../crates/shamir-storage/src/key_bytes.rs#L43); [crates/shamir-storage/src/key_bytes.rs:250](../../../../../crates/shamir-storage/src/key_bytes.rs#L250).

<a id="review-nf-hydration"></a>

### Claim NF-hydration — MirroredStore hydration re-filters classifier drift/tampered mirror entries

Status: `fixed`. Current risk: —.

Source reclassifies each hydrated key and skips/warns on rejection. Registered tests check excluded entries and captured warnings. This protects classification only, not authenticity or validity of allowed-key values; the tag inventory is manually maintained.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:276](../../../../../crates/shamir-storage/src/storage_mirrored.rs#L276); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:244](../../../../../crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L244); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:1029](../../../../../crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L1029); [crates/shamir-storage/src/tests/mod.rs:6](../../../../../crates/shamir-storage/src/tests/mod.rs#L6).

## Corrections and qualified non-findings

- Threat model: direct Store callers or callers with relevant DDL/write authorization, not an unauthenticated client.
- Name validation is O(name length), not O(1), when scanning characters.
- Do not describe timestamp-prefixed randomized IDs as monotonic or unique-by-construction.
- Legacy posting values are hashed, not stored verbatim; the classifier comment/test uses a different illustrative key shape.
- A post-allocation length rejection does not prevent the pre-allocation DoS alleged in finding 5.
- The safe-today claim based on KeyBytes being unused is stale; the inspected WAL/client boundary does not automatically deserialize RecordKey.

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
