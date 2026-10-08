<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-storage — api-wire-protocol independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Object-safe public paths and registrations are intact, but scan-size, flag and atomicity contracts diverge. Envelope guardrails exist; future schema compatibility and unsupported legacy upgrades must be separated.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 6 | 0 | 2 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Persisted MemBufferConfig wire format has no versioning guardrails despite a stable wire-format claim

Status: `partially-fixed`. Current risk: `low`.

Prior-cycle decision: `partially-fixed`.

September source diff replaces raw bincode with MetaEnvelope. Golden/schema dispatch remains absent, but explicit alpha policy permits legacy rejection; defaults do not repair tuple EOF.

Evidence: [crates/shamir-engine/src/table/buffer_config.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/buffer_config.rs#L44); [crates/shamir-index/src/meta_envelope.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/meta_envelope.rs#L54); [CHANGELOG.md:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CHANGELOG.md#L13).

<a id="review-2"></a>

### Claim 2 — batch_size == 0 is unspecified: InMemoryStore yields empty batches forever; fjall/cached silently return zero results

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A nonempty InMemory full/prefix scan yields empty batches endlessly; Cached/Fjall take zero and terminate. MemBuffer still passes zero to inner. Test all dispatch paths, not just merge output.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L164); [crates/shamir-storage/src/storage_cached.rs:559](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L559); [crates/shamir-storage/src/storage_membuffer.rs:923](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L923).

<a id="review-3"></a>

### Claim 3 — set/remove created/existed flag precision varies by backend and write mode, with no capability disclosure

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Seed only inner, then MemBuffer set/remove reports local existence rather than actual effect. Fjall probe/mutation is TOCTOU. remove itself lacks the historically quoted explicit sentence, unlike remove_many.

Evidence: [crates/shamir-storage/src/types.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L36); [crates/shamir-storage/src/types.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L168); [crates/shamir-storage/src/storage_membuffer.rs:763](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L763).

<a id="review-4"></a>

### Claim 4 — Cross-crate wire-format literal duplicated privately: [0,0,0,0] system-record prefix re-encoded in storage_mirrored

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Private literals match and tests use RecordId::system, including truncation. The issue is future coupling; no current incorrect classification follows from duplication.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L44); [crates/shamir-types/src/types/record_id.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L98); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:244](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L244).

<a id="review-5"></a>

### Claim 5 — Public-API rustdoc drift: prefetch promise, phantom engines, stale KeyBytes status

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Prefetch promise is not implemented; removed engines, unflipped KeyBytes narrative and stale line reference remain. Actual RecordKey is KeyBytes.

Evidence: [crates/shamir-storage/src/types.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L291); [crates/shamir-storage/src/key_bytes.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L7); [crates/shamir-storage/src/storage_fjall.rs:655](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L655).

<a id="review-6"></a>

### Claim 6 — Shared backend-conformance suite skipped by CachedStore and MirroredStore

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Helper call sites cover only InMemory/MemBuffer/Fjall. Dedicated Mirrored routing tests do not assert every common flag, empty-input, get_many and reverse contract.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L38); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L168).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — Repo::store_get create-on-read semantics make typos durably materialize

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

store_get intentionally creates. stores_list permits nonmutating existence inspection; normal engine reads reject unconfigured table names before opening stores. Atomic open-existing would be optional additional semantics.

Evidence: [crates/shamir-storage/src/types.rs:465](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L465); [crates/shamir-storage/src/types.rs:475](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L475); [crates/shamir-engine/src/repo/repo_instance.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L337).

<a id="review-8"></a>

### Claim 8 — Interface polish nits

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Alias privacy/duplication, Bytes bounds, copy signature, repeated KeyExists prefix, banners and engine-domain errors remain. Expanded stream types already allow external implementations; polish is not a runtime defect.

Evidence: [crates/shamir-storage/src/types.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L11); [crates/shamir-storage/src/types.rs:336](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L336); [crates/shamir-storage/src/error.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L13).

<a id="review-nf-layout-and-serde"></a>

### Claim NF-layout-and-serde — Registered external test trees and KeyBytes byte-identity suite

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Both manifests are wired, with Fjall gated. Encoding equality/round trips use a local helper matching actual WAL source; bincode alone has bidirectional cross-decode. No execution result is inferred.

Evidence: [crates/shamir-storage/src/lib.rs:32](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/lib.rs#L32); [crates/shamir-storage/src/key_bytes/tests/serde_byte_identity_tests.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes/tests/serde_byte_identity_tests.rs#L129); [crates/shamir-wal/src/wal_entry_v2.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L118).

<a id="review-nf-atomic-capability"></a>

### Claim NF-atomic-capability — supports_atomic_transact discloses atomicity capability honestly

Status: `partially-fixed`. Current risk: `medium`.

Prior-cycle decision: `partially-fixed`.

False for InMemory/Mirrored is correct; cache publication remains per key despite true forwarding. AtomicTransactMock actually delegates to sequential InMemory transact, and native Fjall latest reads require separate qualification.

Evidence: [crates/shamir-storage/src/storage_cached.rs:675](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L675); [crates/shamir-storage/src/storage_membuffer.rs:1092](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1092); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:1352](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L1352).

Grouping/duplicate: [SUMMARY.md#NEW.2](SUMMARY.md#new-2). This is not an additional independent defect.

## Evidence and recipe corrections

- Exact bincode 1.3.3 published src/de/mod.rs dispatches structs as tuples of expected field count and propagates missing-field decode errors; serde defaults alone cannot supply appended fields.
- The version-mismatch test checks Codec, not specifically UnsupportedVersion. Because generic MetaEnvelope::open decodes T before header checks, a valid-payload fixture does not establish header-first dispatch.
- Preserve Store dyn compatibility; impl AsRef or generic prefix/range methods belong in extension conveniences, not required object-safe methods.
- The README's default stack is MemBuffer(Fjall), not unconditional Cached(MemBuffer(Fjall)); its claims of native forward ranges and universal bounded streaming are broader than current implementation.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage -- API & wire-protocol design

## Summary
The `Store`/`Repo` trait surface is unusually well documented for a KV abstraction: ordering guarantees across batches are stated as correctness contracts, `supports_atomic_transact` discloses atomicity capability honestly (F-77/F-85), and the `KeyBytes` serde layer carries a real byte-identity test suite against both bincode and rmp-serde. The main gaps are on serialization/versioning and per-backend contract fidelity: `MemBufferConfig` is declared "stable wire-format" yet persisted with no serde defaults or version field; the streaming API leaves `batch_size == 0` unspecified with divergent (one infinite-hang) behavior across backends; and the `set`/`remove` bool flags silently vary in precision by backend/mode without a capability query, despite the crate having already invented exactly that mechanism for transact. Test coverage is strong overall (~150 tests, correct `tests/` layout), but two of five `Store` implementations skip the shared backend-conformance suite.

## Findings

### 1. Persisted `MemBufferConfig` wire format has no versioning guardrails despite a "stable wire-format" claim
- **File:line:** `crates/shamir-storage/src/storage_membuffer.rs:92-126`
- **Severity:** high
- **Issue:** The doc comment says "Stable wire-format (serialized into `info_store` by the DDL layer)", and the struct is a plain `#[derive(Serialize, Deserialize)]` over 5 fields (`max_bytes`, `max_entries`, `ttl_ms`, `flush_interval_ms`, `flush_batch_size`) — no `#[serde(default)]`, no version/tag field, no golden-bytes test pinning today's encoding. Per `Cargo.toml`, this blob lands in the engine's info_store via bincode at the DDL boundary (i.e., on disk inside `__info__<t>`, mirrored through to fjall).
- **Failure scenario:** Any future change — adding a field (bincode reads old blobs as short → error or misaligned garbage), reordering fields, widening `flush_interval_ms`/`ttl_ms` types — breaks deserialization of every previously-written database's buffer config at open/DDL-reload time. There is no migration hook to catch it, so an existing deployment either fails to open or silently loses its buffer config.
- **Suggested fix:** Add `#[serde(default = "...")]` per field (cheap insurance even under bincode's self-describing-hostile format) plus an explicit envelope/version field before any further schema churn; add a round-trip golden test that serializes today's default config bytes and asserts byte-equality forever, so an accidental format change fails CI instead of production opens.

### 2. `batch_size == 0` is unspecified: InMemoryStore yields empty batches forever; fjall/cached silently return zero results
- **File:line:** trait contract `crates/shamir-storage/src/types.rs:310` (`iter_stream`), `:336` (`scan_prefix_stream`) — no stated precondition; broken divergent impls at `crates/shamir-storage/src/storage_in_memory.rs:161-169` and `:249-257`; silent-empty behavior in `storage_fjall.rs:596-646`/`:656-723` and `storage_cached.rs:540-584`/`:600-645`.
- **Severity:** medium
- **Issue:** Nothing documents that `batch_size` must be > 0. `InMemoryStore`'s stream loops do `take = min(batch_size, entries.len()); drain(..take); yield` — with `batch_size == 0` and a non-empty corpus this yields `Ok(vec![])` infinitely and never terminates. Fjall and CachedStore use `.take(batch_size)` (= 0 items → empty batch → break), so they end immediately having yielded nothing at all even though data exists. Only `merge_overlay_stream` defends itself (`storage_membuffer.rs:661`, `batch_size.max(1)`), which shows the hazard is known but handled inconsistently.
- **Failure scenario:** A caller derives batch size from a tunable/config that computes to 0: full-table scans on disk backends report "no rows" (silent wrong answer feeding index scans/posting lists), while the identical call on the in-memory backend never completes (a hang — classified as a bug by repo policy, but by the *caller*, who has no way to know 0 is invalid).
- **Suggested fix:** Either clamp defensively (`batch_size.max(1)` everywhere, matching `merge_overlay_stream`) or add `debug_assert!(batch_size > 0)` plus one sentence in the trait docs stating the precondition and the exhaustion semantics ("a final batch may be shorter; empty batches are never yielded").

### 3. `set`/`remove` created/existed flag precision varies by backend and write mode, with no capability disclosure
- **File:line:** contract `types.rs:36-39` (`set`: "Returns true if created"), `:66-67` (`remove`: "true if existed"); divergences in `storage_membuffer.rs:763-788` (best-effort, cache/dirty-only, "false (presumed new)" after eviction — acknowledged inline at `:764-768` but not in trait docs), `storage_cached.rs:435-464` and `:493-513` (Async-mode flags derived from cache state only).
- **Severity:** medium
- **Issue:** The trait documents one semantic for `bool`; implementations actually deliver three tiers: strict-but-TOCTOU (Fjall, documented inline at `storage_fjall.rs:358-364`), best-effort-local (MemBuffer — deliberately consults only dirty+cache, never inner, so an evicted-key update reports `created = true`), and mode-dependent (CachedStore Sync vs Async). This crate already established the correct pattern for exactly this problem — `supports_atomic_transact` (`types.rs:285-287`) was introduced because another undocumented capability gap caused MirroredStore to violate an overpromised contract (F-77) — but the flag precision was left as free-text commentary scattered in impl comments.
- **Failure scenario:** A caller using `set`'s return to decide insert-vs-update bookkeeping (e.g., bumping a counter once per genuinely-new key) gets wrong counts when running over MemBuffer-wrapped backends whenever moka evicted the key, with no compile-time or runtime signal that the guarantee differs from the Fjall build it was developed against.
- **Suggested fix:** Document the tier each backend delivers in the trait method doc (or mirror the Fjall precedent: note where the strictness boundary is); longer-term, extend the F-77-style pattern with e.g. `fn strict_exists_flags(&self) -> bool` if callers ever need to gate on it.

### 4. Cross-crate wire-format literal duplicated privately: `[0,0,0,0]` system-record prefix re-encoded in storage_mirrored
- **File:line:** `crates/shamir-storage/src/storage_mirrored.rs:41-48` (`SYSTEM_RECORD_PREFIX: [u8; 4] = [0,0,0,0]`, kept local because the canonical constant is private) vs `crates/shamir-types/src/types/record_id.rs:18` (private `const SYSTEM_RECORD_PREFIX: &[u8] = &[0, 0, 0, 0];`); consumed by the durability classifier at `storage_mirrored.rs:173-198`.
- **Severity:** medium
- **Issue:** A 4-byte wire constant that decides which keys survive a hybrid-table restart exists in two crates with no shared definition. The local copy is honestly annotated, and the exhaustiveness test (`storage_mirrored_tests.rs:244`, building keys via the real `RecordId::system`) would catch an encoding change indirectly — but the guard is a behavioral proxy, not the type-level guarantee the module's own care level implies.
- **Failure scenario:** The record-key migration plan this crate is mid-flight on (`docs/dev-artifacts/design/record-key-128-migration-plan.md`) touches exactly these encodings; if `RecordId::system`'s prefix/padding changes without the allowlist match set following, classifier hits go to zero and every durable-config key silently becomes ephemeral — table/index/buffer config stops surviving restart, logged only as hydration drift warnings (if any entries existed to warn about).
- **Suggested fix:** Export `pub const SYSTEM_RECORD_PREFIX: [u8; 4]` (or a `RecordId::system_prefix()` accessor) from shamir-types and use it here; keep the local copy only if the borrow direction (types must not depend on storage) forbids it — in which case add a cross-crate test asserting the two literals stay equal.

### 5. Public-API rustdoc drift: prefetch promise, phantom engines, stale KeyBytes status
- **File:line:** `types.rs:289-292` ("Uses concurrent prefetching: while yielding current batch, fetches next batch in background" — no implementation prefetches; every backend runs sequential cursor-resumed batches, and the crate README itself describes the design as lazy fetch-on-demand). Phantom backend references presented as live implementors/callers: `types.rs:96` ("sled, fjall, cached MUST override"), `:134-141`, `:185-188`, `:343-347`, `:374` (sled/redb/persy/nebari/canopy), `storage_in_memory.rs:80`, `storage_membuffer.rs:119-121`, `src/tests/types_tests.rs:149-150` — the crate README (`src/README.md:152-159`) explicitly documents these engines as removed/nonexistent. Stale self-description: `key_bytes.rs:4-9` still claims "`pub type RecordKey = Bytes;` alias is left untouched ... currently unused by production code", while `types.rs:9` is now `pub type RecordKey = KeyBytes;` and the entire trait surface uses it. Stale line-ref comment: `storage_fjall.rs:655` ("same pattern as iter_stream above (lines ~323)" — iter_stream is at :596).
- **Severity:** low
- **Issue:** For a new implementor of `Store` (the primary audience of trait rustdoc), the contract text describes behavior that does not exist (concurrent prefetch) and an engine ecosystem that does not exist, and the `KeyBytes` header contradicts the actual wiring status of the very type it documents.
- **Failure scenario:** Documentation-only: someone implementing a new backend budgets effort for background-prefetch machinery or compares against phantom engines; readers of `key_bytes.rs` header may wrongly assume the alias flip hasn't happened and that serde byte-identity is still prospective rather than load-bearing (it now guards every WAL/storage key).
- **Suggested fix:** One surgical rustdoc pass: delete the prefetch sentence (or reword to "batches are fetched lazily, one cursor-resumed range per batch"); replace phantom-engine name-drops with "buffering backends"; refresh the `KeyBytes` module header to reflect that `RecordKey = KeyBytes` shipped.

### 6. Shared backend-conformance suite skipped by CachedStore and MirroredStore
- **File:line:** suite defined in `src/tests/types_tests.rs:38` (`run_batch_store_tests` covering `insert_many`/`set_many`/`remove_many` flag ordering + empty-input, `get_many` order/miss mapping, post-`flush` visibility, reverse-range ordering); invoked only from `storage_fjall_tests.rs:125`, `storage_membuffer_tests.rs:33`, `storage_in_memory_tests.rs:77` — no call from `storage_cached_tests.rs` (33 tests) or `storage_mirrored_tests.rs` (18 tests).
- **Severity:** low
- **Issue:** Two of the five `Store` implementations — including CachedStore, whose overrides are non-trivial (async worker drain inside `flush`, flag-from-cache semantics) — never run the crate's own agnostic conformance checklist. Their bespoke suites cover wrapper-specific behaviors well but not the common contract (e.g., `set_many` flag vector ordering, `remove_many` mixed exists/missing results, reverse-range high→low assert).
- **Failure scenario:** A regression in a delegation path (e.g., CachedStore `flush` variant missing propagation, or a future override changing flag order) breaks the documented contract on those backends only; the safety net designed to catch precisely that does not exercise them.
- **Suggested fix:** Add two thin tests: `CachedStore::new_sync(InMemory)` → `run_batch_store_tests(...)`, and (with a toy classifier) a `MirroredStore` over `InMemoryRepo` doing the same; keep Fjall/MemBuffer coverage as-is.

### 7. `Repo::store_get` create-on-read semantics make typos durably materialize
- **File:line:** contract `types.rs:465-468` ("Retrieves a store by name. Creates it if it doesn't exist"); disk-side effect in `storage_fjall.rs:229-245` (fjall keyspace created via `KeyspaceCreateOptions::default` on every miss).
- **Severity:** low
- **Issue:** There is no open-without-create probe on the `Repo` trait, so a read-path caller can never validate existence without mutating durable layout.
- **Failure scenario:** A misspelled table name anywhere on a read path materializes a real, empty fjall keyspace in the repository directory — visible in `stores_list()`, occupying metadata/journal space until manually cleaned, and indistinguishable from an intentional empty table downstream.
- **Suggested fix:** Add a default-implemented non-mutating check (e.g., `store_exists(name) -> bool`, cheaply answerable by all current backends) and route pure-validation callers through it; optionally log when `store_get` creates a new keyspace so accidental creation surfaces in logs.

### 8. Interface polish nits
- **File:line / severity:** nit, grouped
- **Issue & suggested fix:**
  - `RecordStream` is `pub(crate)` (`types.rs:11`) but is the return type of required public trait methods — external implementors/consumers cannot name it, and the alias already had to be duplicated locally twice (`storage_membuffer.rs:628`, `src/tests/types_tests.rs:12`). Make it `pub`.
  - Mixed key vocabulary on scan APIs: keys are `RecordKey`/`KeyBytes` but prefix/range bounds take raw `Bytes` (`types.rs:336, 354-358`), forcing boundary conversions in every backend (`RecordKey::from(prefix.clone())`). Consider accepting `impl AsRef<[u8]>` uniformly.
  - `Repo::copy_store` takes `&str` (`types.rs:488`) while sibling methods take generic `AsRef<str> + Send` — signature inconsistency in the same trait.
  - Double-prefixed message: `DbError::KeyExists(format!("Key already exists: {:?}", key))` renders as "Key already exists: Key already exists: ..." since the variant Display adds the same prefix (`storage_in_memory.rs:111` vs `error.rs:13-14`).
  - Dangling `// ==== Tests ====` section headers left in impl files whose tests moved to `tests/` (`storage_in_memory.rs:260-262`, `storage_fjall.rs:726-728`, `storage_cached.rs:719-721`).
  - `error.rs` accumulates engine-domain variants (`Function`, `ValidatorRejected`, `ValidatorInvalid`, `IndexDrainInProgress`) in the lowest-layer crate; deliberate per its doc ("generic error"), but worth noting it couples every backend consumer to the engine's error taxonomy. Its `code()` wire mapping covers only 3 variants — fine, just ensure sibling reviewers know the mapping lives here.

## Test-coverage notes (for the record)
- Layout conforms to CLAUDE.md: per-module `tests/` dirs (`src/tests/`, `src/key_bytes/tests/`) with manifest-only `mod.rs`; no inline `#[cfg(test)] mod tests` in impl files; test-only seams properly `#[cfg(test)]`-gated (`membuffer_clear_race_hook.rs`).
- `key_bytes/tests/serde_byte_identity_tests.rs` is exemplary for a wire-format suite: bincode + rmp-serde byte-identity against a local mirror of the WAL encoder, spanning INLINE_CAP boundaries, round-trips, and cross-decode in both directions.
- Backend suites are deep on their specific hazards (overlay-scan merges, async-write FIFO ordering, flush error surfacing, mirror-first F-41/F-59 atomicity, classifier drift); the gap is only finding #6.

</details>
