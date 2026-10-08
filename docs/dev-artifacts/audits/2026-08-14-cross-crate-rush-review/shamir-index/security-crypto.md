<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-index — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Hash-only identities and scalar dispatch remain privilege-qualified integrity risks. Exact locked FxHasher semantics now provide a full dual-hash numeric collision witness. Plain serialization and path joins are confirmed; NEON UB remains unverified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 6 | 0 | 0 | 1 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints are enforced on hash alone

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Exact rustc-hash 2.1.2 uses H←(H+x)K. On 64-bit, two Int leaves emit tag,x,tag,y. Tuples (0,0) and (t,signed(-tK² mod 2^64)) collide for both public-seeded streams because their suffix difference is tK³-tK³, independent of seed. Unique postings store only RecordId and reject a hit without value comparison. This needs an existing composite numeric index and write permission, not auth bypass. Published source: https://docs.rs/crate/rustc-hash/2.1.2/source/src/lib.rs; signed dispatch is confirmed by [Rust 1.94 Hash source](https://raw.githubusercontent.com/rust-lang/rust/1.94.0/library/core/src/hash/mod.rs). No experiment was run.

Evidence: [crates/shamir-index/src/base_index/index_keys.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_keys.rs#L72); [crates/shamir-index/src/base_index/index_keys.rs:191](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_keys.rs#L191); [crates/shamir-index/src/base_index/index_manager_unique.rs:384](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager_unique.rs#L384); [crates/shamir-index/src/base_index/index_manager_unique.rs:440](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/index_manager_unique.rs#L440); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007).

<a id="review-2"></a>

### Claim 2 — FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Token text becomes one unkeyed u64 and postings preserve no verification text. A colliding token shares membership/ranking identity; document writers influence inputs. Exact rustc-hash 2.1.2 source confirms non-cryptographic semantics, but the numeric dual-hash witness does not prove a chosen-token collision under tokenizer restrictions. Published source: https://docs.rs/crate/rustc-hash/2.1.2/source/src/lib.rs.

Evidence: [crates/shamir-index/src/tokenizer.rs:462](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tokenizer.rs#L462); [crates/shamir-index/src/fts_ranked_backend.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L88); [crates/shamir-index/src/fts_ranked_backend.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L123); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007).

<a id="review-3"></a>

### Claim 3 — `trusted_pure` scalar gate is documented here but not enforced at this crate's dispatch boundary

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

expr calls unrestricted resolver.call, whose user-first resolution can dispatch an unvouched replacement. Normal engine DDL checks is_indexable. Bypass requires host replacement, direct library construction, or persisted metadata manipulation; no arbitrary remote closure installation is shown. Check and invoke the same entry with existing arity rules.

Evidence: [crates/shamir-index/src/expr.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/expr.rs#L178); [crates/shamir-funclib/src/scalar_resolver.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-funclib/src/scalar_resolver.rs#L110); [crates/shamir-engine/src/table/table_manager_index_mgmt.rs:262](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_index_mgmt.rs#L262); [crates/shamir-index/src/functional_backend.rs:303](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/functional_backend.rs#L303).

<a id="review-4"></a>

### Claim 4 — External vector-backend API key persisted in cleartext inside the index-metadata blob

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

SecretString serialization is pass-through and External descriptors can be persisted through library APIs. The inspected engine handler constructs InProcessHnsw, and encrypted data/backup volumes are the explicit deployment obligation. This is credential-carrier hardening, not demonstrated remote secret disclosure.

Evidence: [crates/shamir-index/src/kind.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/kind.rs#L195); [crates/shamir-types/src/secret.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L54); [crates/shamir-index/src/persistence.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/persistence.rs#L97); [docs/guide-docs/security/data-protection.md:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/security/data-protection.md#L68).

<a id="review-5"></a>

### Claim 5 — Snapshot load joins persisted `basename`/`qbasename` into temp file paths unsanitized; manifest/sidecar carry no integrity check

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Manifest basename/qbasename feed path joins followed by File::create without validation. A metadata writer can select a writable destination with the appended .hnsw.graph/.hnsw.data suffix. CRC32 does not authenticate such an attacker, and no arbitrary remote metadata-write route is established.

Evidence: [crates/shamir-index/src/vector/snapshot.rs:907](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L907); [crates/shamir-index/src/vector/snapshot.rs:912](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L912); [crates/shamir-index/src/vector/snapshot.rs:983](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L983); [crates/shamir-index/src/vector/snapshot.rs:989](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/snapshot.rs#L989).

<a id="review-6"></a>

### Claim 6 — `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A long alphanumeric run allocates a gram vector and one owned string per emitted gram. Frequency planning deduplicates repeated hashes, so one posting per gram is false. Remote frames impose an outer bound; direct library inputs lack this index-local budget. Silent truncation would change search completeness.

Evidence: [crates/shamir-index/src/tokenizer.rs:152](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tokenizer.rs#L152); [crates/shamir-index/src/tokenizer.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/tokenizer.rs#L164); [crates/shamir-index/src/fts_ranked_backend.rs:84](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/fts_ranked_backend.rs#L84); [crates/shamir-connect/src/common/types.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/types.rs#L111).

<a id="review-7"></a>

### Claim 7 — NEON kernels read `u32` through `*const u8`-derived pointers — aligned-load safety contract violated (aarch64 only, untested on CI)

Status: `unverified`. Current risk: `nit`.

Prior-cycle decision: `unverified`.

The u8-derived u32 casts and lane loads exist, and chunk bounds cover four bytes. The exact Rust 1.94 intrinsic alignment contract was not resolved; available primary intrinsic documentation does not explicitly supply the claimed requirement. Pointer casting alone is not proof of UB. Do not substitute another compiler version or execute a reproduction.

Evidence: [crates/shamir-index/src/vector/simd.rs:920](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/simd.rs#L920); [crates/shamir-index/src/vector/simd.rs:932](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/simd.rs#L932); [crates/shamir-index/src/vector/simd.rs:1101](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/simd.rs#L1101); [crates/shamir-index/src/vector/simd.rs:1250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/vector/simd.rs#L1250); [rust-toolchain.toml:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/rust-toolchain.toml#L15).

<a id="review-8"></a>

### Claim 8 — `unreachable!` on a data-derived `IndexKind::Btree` descriptor panics at table open

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Persisted table-open descriptors are filtered for Btree before builder dispatch. Direct unsupported calls still panic, but the alleged hostile-blob boot path is positively excluded by pre-existing caller code.

Evidence: [crates/shamir-engine/src/table/table_manager.rs:661](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager.rs#L661); [crates/shamir-index/src/build_backend.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/build_backend.rs#L66).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

## Evidence and recipe corrections

- Replace the historical multiply-XOR description with the exact locked polynomial implementation. Constant-work simultaneous collisions are proven for the typed numeric witness, not for every arbitrary string/token scenario.
- No hash finding establishes tenant isolation or authorization bypass; scope is records reachable under the caller's existing privileges.
- Widening token identity requires changing the Vec&lt;u64&gt; query/token API and migrating persisted postings, not only extending posting value_bytes.
- A keyed hash requires a durable, consistently replicated key and format migration; a per-process random key would make persisted lookups incompatible.
- CRC cannot defend against a metadata-writing attacker. Path confinement is independently required.
- Replacing four-byte lane loads with vld1_u8 can overread unless the loop is restructured for eight readable bytes.
- The broad atomic-batch assurance holds for HNSW's override, not the VectorAdapter default inherited by BruteForce.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-index -- Security & crypto boundary

## Summary

`shamir-index` has no auth/HMAC/SCRAM/TLS surface (that lives in `shamir-connect`); its crypto-boundary exposure is (a) hash-based key derivation over **untrusted record values and FTS token streams**, (b) decoding of **persisted, potentially tampered blobs** (index metadata, HNSW snapshots, covering projections), (c) one `unsafe` SIMD module, and (d) one secret-bearing config field. The crate is unusually disciplined on input clamping (`MAX_TOPK` / `MAX_EF_SEARCH` / dim checks / bounds-checked decoders / CRC32-everywhere on snapshot chunks), but it systematically relies on the **non-keyed `FxHasher`** for adversarially controlled inputs — directly contradicting CLAUDE.md pillar 4's own justification ("we don't accept untrusted hash inputs here"), since document text and record values *are* untrusted. The `trusted_pure` scalar gate this crate documents is enforced only in `shamir-engine` at DDL time, never re-checked at this crate's eval/dispatch boundary. No critical/high findings; several medium integrity/DoS issues and low/nit items.

## Findings

### 1. Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints are enforced on hash alone
- **File:** `crates/shamir-index/src/base_index/index_keys.rs:186-240` (`compute_leaf_hashes` / `compute_lookup_hashes`), `crates/shamir-index/src/base_index/index_record_key.rs:25-29,62-81`, consumed at `index_manager.rs:2789-2883` (`lookup_by_index`) and `index_manager_unique.rs:350-421` (`check_unique_key`)
- **Severity:** medium
- **Issue:** The 25-byte posting key is `hash1 || hash2`, where h1 and h2 are both `rustc_hash::FxHasher` — the *same* non-keyed, non-cryptographic multiply-xor algorithm, one instance merely pre-seeded with the public constant `0x9E3779B97F4A7C15`. The doc calls this "collision resistance", but the two streams are correlated and FxHash's structure makes simultaneous multi-collions cheap to construct offline (far below the 2^128 brute-force bound; the seed is fixed and public, so an attacker solving one stream can adapt the final block to control the other). All hashed material (record field values via `ScalarRef::Str`/`Bin`, composite multi-field tuples) is client-controlled row data. Crucially, neither read path re-verifies values: `lookup_by_index` returns every record whose 25-byte key prefix matches with no value comparison, and `check_unique_key` treats any existing entry under the hash key as a duplicate — the hash **is** the constraint.
- **Failure scenario:** A tenant with INSERT/SELECT privilege (1) crafts values whose dual-FxHash collides with a victim value → `lookup_by_index` returns the attacker's records for queries on the victim's value (wrong results / data confusion across rows); (2) crafts a colliding value on a UNIQUE column → legitimate inserts of the victim value are rejected with `DuplicateKey` (availability); (3) inserts a flood of distinct values all colliding into one 25-byte key → the posting cache entry and prefix scan degrade to O(n) per lookup (classic hash-flooding DoS — exactly what `RandomState` exists to prevent and what CLAUDE.md pillar 4 dismisses by claiming no untrusted hash inputs).
- **Suggested fix:** For the unique family at minimum, re-verify on hit: store the (serialized) indexed values (or a strong digest) alongside the `RecordId` in the posting value and compare before returning `DuplicateKey`, mirroring how `extract_index_leaves` already exists for value comparison. Longer term, switch the value-hash to a keyed or cryptographic digest (e.g. SipHash-1-3 with a per-table key, or blake3 truncated to 16 bytes) — the tag-stable encoding scheme can stay, only the compression function changes. Add an adversarial-collision test; the current suite only checks that two *different* strings hash differently (`tests/tokenizer_tests.rs:51` analog, `byte_identity_tests.rs` corpus).

### 2. FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`)
- **File:** `crates/shamir-index/src/tokenizer.rs:462-469`, used at `fts_backend.rs:70-93,125-131,204-211` and `fts_ranked_backend.rs:80-99,150-156`
- **Severity:** medium
- **Issue:** Every token from user documents (`tokenize_record`) and from query strings (`tokenize_query`) is compressed to a `u64` via raw `FxHasher` and that `u64` **is** the token identity inside the posting key (`[index_id][FTS][hash8][record_id16]`). FxHash is trivially collidable on short attacker-chosen strings. The scan-side filter (`pk.index_id == self.descriptor.id && pk.type_tag == type_tag::FTS`) verifies the tag but never the token itself — a collision merges two tokens' posting lists with no runtime detection.
- **Failure scenario:** An attacker who can insert documents computes a token colliding with a victim term (e.g. a rival product name, another user's handle) and seeds documents with it; every subsequent FTS query for the victim term returns the attacker's records (search-result poisoning via the index), and BM25 ranking (`fts_ranked_backend.rs:313`) scores the injected postings as if they were the queried term. Conversely, colliding the tokens of two sensitive terms cross-contaminates result sets. A flood of colliding tokens also funnels all postings into one prefix scan (per-query O(total postings of the bucket)).
- **Suggested fix:** Widen the token identity to a 128-bit digest (the posting layout has room: `FIXED_OVERHEAD` already treats `value_bytes` as variable — store 16 bytes instead of 8), using a keyed or cryptographic hash (SipHash-1-3 keyed per-DB, or blake3-128). Keep `token_hash` only for non-adversarial internal uses. CLAUDE.md pillar 4's stated trade-off ("we don't accept untrusted hash inputs") should either be re-affirmed by an explicit threat-model note on `token_hash` (documents are attacker-controlled text) or the hash should be upgraded.

### 3. `trusted_pure` scalar gate is documented here but not enforced at this crate's dispatch boundary
- **File:** `crates/shamir-index/src/expr.rs:47-52,167-185` (`IndexExpr::Scalar` → `resolver.call(name, …)`), `crates/shamir-index/src/functional_backend.rs:52-66,95-105`, `crates/shamir-index/src/build_backend.rs:22-51`
- **Severity:** medium
- **Issue:** `expr.rs` states "Only `.trusted_pure()`-vouched scalars are allowed here — the name is persisted, the callable is per-process", and `shamir-funclib::registry` documents `is_indexable()` as "the index-safety gate". That gate is enforced exactly once, in `shamir-engine` (`table_manager_index_mgmt.rs:259`), at CREATE INDEX time. The eval path in *this* crate calls `resolver.call(name, ...)` against the **full** resolver — user layer first, then every builtin including non-vouched, non-deterministic ones (`uuid_v4`, `now`) — with no `is_indexable()` check. The scalar name travels in the bincode-persisted `IndexDescriptor`, and `build_index2_backend_with_resolver` re-arms evaluation from disk on every open.
- **Failure scenario:** (a) A tampered/legacy `__meta__/indexes` blob carrying `Scalar { name: "uuid_v4" }` (or any host-registered function) is loaded on open and evaluated on every write and lookup of the functional index — write-path and read-path hashes diverge, so the index silently returns wrong/empty results while appearing `Ready`, and an un-vouched host closure is dispatched from persisted data. (b) A scalar re-registered after CREATE without `.trusted_pure()` (same name, new behavior) is picked up on reopen with no gate. Both bypass a documented security property whose enforcement lives in a different crate and a different lifecycle moment than the dispatch.
- **Suggested fix:** In `IndexExpr::eval_with_scalars` (or `FunctionalBackend::eval_or_null`), resolve via `resolver.get(name)` and reject with `ExprError::ScalarError` unless `entry.is_indexable()`, then dispatch the entry directly. This makes the boundary self-defending regardless of which layer validated the DDL, at the cost of one map lookup per scalar-node eval.

### 4. External vector-backend API key persisted in cleartext inside the index-metadata blob
- **File:** `crates/shamir-index/src/kind.rs:189-200` (`VectorBackendRef::External { driver, url, api_key_secret }`), persisted via `crates/shamir-index/src/persistence.rs:93-106` → `meta_envelope.rs:50-52` (plain bincode in `MetaEnvelope`, no confidentiality or MAC)
- **Severity:** medium
- **Issue:** `SecretString` redacts `Debug` and zeroizes on drop (`shamir-types/src/secret.rs`), but its `Serialize` is pass-through, so `save_index2_metadata` writes the raw API key into the `system:_m.idx` record of the info store. The envelope provides versioning only — no encryption, no MAC. Anyone with read access to the info store (file-level access, a backup, a replicated/mis-scoped store) recovers the credential; the in-memory protections never engage for the at-rest copy.
- **Failure scenario:** Operator configures a vector index against an external service with an API key; a store snapshot/backup or an info-store read primitive (present or future) leaks the third-party credential in plaintext.
- **Suggested fix:** Either exclude `api_key_secret` from the persisted descriptor (like `VectorConfig::quantization` already does with `#[serde(skip)]` — resolve the secret at runtime from a keyring/env reference), or persist a *reference* (secret name) rather than the value. If at-rest encryption exists at the storage layer, document that contract at this field so future backends can't silently break it.

### 5. Snapshot load joins persisted `basename`/`qbasename` into temp file paths unsanitized; manifest/sidecar carry no integrity check
- **File:** `crates/shamir-index/src/vector/snapshot.rs:871-917` (f32 path: `load_dir.path().join(format!("{basename}.hnsw.graph"))` at 907-908) and `:971-1013` (u8 path: `qbasename` at 983-984); acknowledged no-CRC on manifest/sidecar at `snapshot.rs:694-697`
- **Severity:** low
- **Issue:** Every *chunk* is CRC32-verified, but the manifest and sidecar themselves are only magic/version-checked (the in-code comment admits "they carry no crc of their own"). `basename` is a free-form `String` read from that unverified manifest and interpolated into a filesystem path. On Windows/Unix alike, a basename containing `..`, `../`, or absolute-path components escapes the `TempDir` in `File::create` + `write_all` during load — an arbitrary-location overwrite (content: attacker-influenced graph bytes) at table open.
- **Failure scenario:** An attacker with write access to the info store (the same trust level the per-chunk CRCs already defend against) edits only the manifest (no chunk CRC to recompute, no envelope MAC to forge) to set `basename = "..\..\Users\Public\x"`; on next open, the load path writes `x.hnsw.graph` outside the temp dir.
- **Suggested fix:** Sanitize/validate `basename` on load (reject anything containing path separators, `..`, NUL, or a non-identifier character — the dump side only ever produces `"shamir"`/`"shamirq"` + uniquifier suffixes), or ignore the persisted name entirely and derive it locally. Optionally fold a CRC over the manifest/sidecar payloads to close the acknowledged gap.

### 6. `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token
- **File:** `crates/shamir-index/src/tokenizer.rs:110-170` (`NgramTokenizer` / `emit_ngrams`), consumed unbounded at `fts_ranked_backend.rs:80-99` and `plan_insert:163-185`
- **Severity:** low
- **Issue:** A single alphanumeric run of length L emits ~L owned `String` n-grams (each a fresh allocation), and every gram becomes a separate `SetPosting` op in the planned batch. There is no cap on field text length, tokens per document, or ops per `plan_insert` anywhere in the FTS path; `doc_len: u32` also saturates BM25 stats for absurd inputs.
- **Failure scenario:** A writer inserts a record whose indexed field is a multi-MB unbroken string (cheap for the attacker, one field) into an n-gram-indexed table: tokenization allocates thousands of small strings per KB of input, and the commit plans millions of posting ops — memory spike + store write amplification on the server.
- **Suggested fix:** Cap per-field token count / total gram count (tunable, e.g. 100k tokens) with truncation or a typed `IndexError`, and/or cap the grams emitted per word; the FTS write path is the right chokepoint since the value arrives as an opaque `&str`.

### 7. NEON kernels read `u32` through `*const u8`-derived pointers — aligned-load safety contract violated (aarch64 only, untested on CI)
- **File:** `crates/shamir-index/src/vector/simd.rs:932-933` (`weighted_bilinear_neon`), `:1101-1102` (`weighted_sq_diff_neon`), `:1250` (`weighted_linear_neon`)
- **Severity:** nit
- **Issue:** `vld1_lane_u32(xp.add(i) as *const u32, …)` loads a `u32` from a pointer obtained via `&[u8]::as_ptr()`, whose alignment guarantee is 1. The `std::arch` intrinsic requires the pointer to be valid for an *aligned* `u32` read; with a `Vec<u8>` code buffer at a non-4-aligned address this is formally UB. The module's own header concedes these kernels are never executed on the CI hosts ("NOT exercised on the x86_64 CI/dev hosts; verified by `cargo check --target aarch64-*`"), so the misalignment case is also untested. (The x86 paths correctly use `_mm_loadu_*` unaligned intrinsics; loop bounds are in-range.)
- **Failure scenario:** On aarch64 with a misaligned `vectors_u8` sub-slice or an allocator returning a 1-aligned buffer, execution hits an alignment violation — in practice hardware tolerates it, but the contract breach is real and is exactly the class of latent UB that surfaces under future compiler optimization.
- **Suggested fix:** Load with `vld1_u8`/`vld1q_u8` (unaligned-safe, as `dot_u8_neon_wide` right below already does) and widen with `vmovl_u8`, or use `read_unaligned` into a `u32` before `vdup_n_u32`. Add an aarch64 CI leg or at least an alignment-fuzz unit test on x86 via a scalar twin.

### 8. `unreachable!` on a data-derived `IndexKind::Btree` descriptor panics at table open
- **File:** `crates/shamir-index/src/build_backend.rs:66-68`
- **Severity:** nit
- **Issue:** `build_index2_backend_with_resolver` panics via `unreachable!("Btree indexes are handled by the base_index index manager")` when the *persisted* descriptor decodes with `kind = Btree`. Descriptors come from the bincode `__meta__/indexes` blob (untrusted-at-rest, as this crate's own forward-compat fallback chain treats it). CLAUDE.md's error rule permits panic only for invariant violations that mean a programmer bug; a hostile/corrupt blob is data.
- **Failure scenario:** A tampered metadata blob (or a future writer bug) yields one Btree descriptor → every subsequent open of that table panics inside the backend-builder loop — a persistent crash-loop rather than a typed error.
- **Suggested fix:** Return `IndexError::Backend("Btree kind must not reach the index2 builder")` (or skip + `set_failed`) and let the caller's R0-D fail-closed path mark the backend `Failed`, consistent with how the rest of the open path degrades.

---

**Areas examined and clean (for the record):** all `unsafe` is confined to `vector/simd.rs` with runtime feature detection and documented invariants (except finding 7); `PostingKeyRef::decode` and the sorted-index key decoders are bounds-checked and `Option`-returning; `posting_layout`, `ddl_op_log` (versioned), `decode_covering_projection`, `IndexInfo::decode_bytes`, and the snapshot chunk reassembly all fail closed on malformed input; vector search inputs are consistently clamped (`MAX_TOPK`, `MAX_EF_SEARCH`, dim checks, atomic batch dim validation); `FtsPostingValue` decode falls back rather than panicking; no secret-dependent comparisons or timing-sensitive branches exist in this crate (distances/keys are not secrets); `RecordId::system` key-collision hazards are explicitly documented and byte-verified at every new system key (persistence.rs:297-304, 426-437, index_manager.rs:1054-1072, sorted_index_manager.rs:680-685, 1232-1237); CRC32 (not a MAC) as the snapshot integrity mechanism matches CLAUDE.md's "Checksums everywhere" corruption-detection pillar rather than contradicting it.

</details>
