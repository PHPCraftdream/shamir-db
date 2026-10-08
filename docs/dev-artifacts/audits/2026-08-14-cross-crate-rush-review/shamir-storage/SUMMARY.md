<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-storage — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Most concrete shim defects remain. Exact cached dependency bytes resolve several earlier uncertainties and expose native batch visibility/error-path gaps. The legacy-config rejection is documented alpha policy, not an unsupported-upgrade defect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 64 | 50 | 4 | 2 | 3 | 0 | 5 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — MemBufferStore::get_many cache-fill can poison a Tombstone over a concurrent write

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Pause an inner miss, acknowledge set(K,V), then release the miss: unconditional Tombstone fill masks V, including after dirty drains.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1246](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1246); [crates/shamir-storage/src/storage_membuffer.rs:1258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1258).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — MemBufferStore::transact post-commit cache republish clobbers a concurrent writer's fresher value

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A concurrent differing dirty value survives cleanup but is hidden by unconditional transaction cache publication; the regression checks only real_inner.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1048); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:832](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L832).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — InMemoryStore::iter_range_stream resumes inclusive + blind-skip instead of Bound::Excluded

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Emit A, remove A between pulls, then resume: the unseen successor B is skipped.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:196](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L196); [crates/shamir-storage/src/storage_in_memory.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L205).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — MemBufferStore::remove/remove_many misreport existed for inner-only keys

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Seed inner before wrapping: removal reports false yet deletes on flush. Normal manager pre-read warms the cache; bookkeeping exposure needs a subsequent eviction/expiry/race.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:886](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L886); [crates/shamir-engine/src/table/table_manager_crud.rs:456](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager_crud.rs#L456).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — CachedStore and MirroredStore never run the backend-agnostic batch contract suite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Only InMemory, MemBuffer and Fjall invoke the helper; dedicated Mirrored routing assertions cover a subset.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L38); [crates/shamir-storage/src/tests/mod.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/mod.rs#L3).

Grouping/duplicate: [api-wire-protocol.md#6](api-wire-protocol.md#review-6). This is not an additional independent defect.

<a id="review-1-6"></a>

### Claim 1.6 — FjallStore write-ordering claim holds only per handle-instance; store_get creates fresh workers

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Fresh handles own lazy workers; set/remove bypass them. Drop joins workers, so ordering overstatement and conditional churn are supported, not a leak.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L240); [crates/shamir-storage/src/storage_fjall.rs:494](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L494).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-1-7"></a>

### Claim 1.7 — InMemoryStore::set can resurrect an older value under concurrent same-key writers

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The update exposes a transient absence and ignores duplicate reinsertion. Overlapping completion order alone is not a lost-write oracle.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L129); [crates/shamir-storage/src/storage_in_memory.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L130).

Grouping/duplicate: [concurrency-lockfree.md#3](concurrency-lockfree.md#review-3). This is not an additional independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — CachedStore size-counter drift: increment on rejected duplicate insert

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

A lazy fill can insert a newly created inner key before insert resumes; ignored duplicate insertion still increments size.

Evidence: [crates/shamir-storage/src/storage_cached.rs:422](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L422); [crates/shamir-storage/src/storage_cached.rs:480](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L480).

Grouping/duplicate: [correctness-tdd.md#8](correctness-tdd.md#review-8). This is not an additional independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — Repo::copy_store has no from == to self-copy guard

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

set_many preserves original keys; quiescent self-copy does not double records. A no-op guard is optional.

Evidence: [crates/shamir-storage/src/types.rs:500](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L500).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

<a id="review-1-10"></a>

### Claim 1.10 — Stale narration of the retired #535 mechanism in race-hook docs

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Batch-hook and test comments still narrate boolean clear/republish, although production uses dirty_count.

Evidence: [crates/shamir-storage/src/membuffer_clear_race_hook.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/membuffer_clear_race_hook.rs#L61); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:845](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L845).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — CachedStore unordered, non-atomic cache mutation leaves cache behind inner

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Backing A then B, cache B then A leaves lasting disagreement. Async queued set can also execute after a later directly delegated transact.

Evidence: [crates/shamir-storage/src/storage_cached.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L431); [crates/shamir-storage/src/storage_cached.rs:671](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L671).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-2-2"></a>

### Claim 2.2 — MemBufferStore::get_many missing the #539 tombstone-poisoning guard

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Post-inner fills overwrite concurrent Live/Tombstone publication; ordinary shared get_many tests do not schedule that window.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1258).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-2-3"></a>

### Claim 2.3 — InMemoryStore::set remove/reinsert swallows a racing Duplicate

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Remove/reinsert gap remains. Published scc 3.8.4 proves upsert_sync exists; a transient NotFound is a discriminating oracle, completion-last is not.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L124); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

Grouping/duplicate: [concurrency-lockfree.md#3](concurrency-lockfree.md#review-3). This is not an additional independent defect.

<a id="review-2-4"></a>

### Claim 2.4 — FjallStore::submit blocks tokio when its 1024-slot queue fills

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Full queue blocks the executing runtime thread inside async submit. Saturation and latency were not measured.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L92); [crates/shamir-storage/src/storage_fjall.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L199).

Grouping/duplicate: [concurrency-lockfree.md#4](concurrency-lockfree.md#review-4). This is not an additional independent defect.

<a id="review-2-5"></a>

### Claim 2.5 — moka cache uses default hasher instead of THasher

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Published moka 0.12.15 confirms std RandomState. This is doctrine/performance-policy debt, not a measured speed regression.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:255](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L255); [Cargo.lock:2221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2221).

Grouping/duplicate: [concurrency-lockfree.md#5](concurrency-lockfree.md#review-5). This is not an additional independent defect.

<a id="review-2-6"></a>

### Claim 2.6 — InMemoryStore streams eagerly materialize all results under one Guard

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

All result handles are collected before stream construction; Guard ends before yielding, but collection pins reclamation during the traversal.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L153); [crates/shamir-storage/src/storage_in_memory.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L240).

Grouping/duplicate: [performance-hotpath.md#2](performance-hotpath.md#review-2). This is not an additional independent defect.

<a id="review-2-7"></a>

### Claim 2.7 — CachedStore::reload is non-atomic clear/refill against live traffic

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Clear/reset/refill is unordered with live mutations; stale populated hits do not fall through to inner.

Evidence: [crates/shamir-storage/src/storage_cached.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L311); [crates/shamir-storage/src/storage_cached.rs:471](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L471).

Grouping/duplicate: [concurrency-lockfree.md#7](concurrency-lockfree.md#review-7). This is not an additional independent defect.

<a id="review-2-8"></a>

### Claim 2.8 — Cross-path Fjall ordering rests on journal arrival order

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Published fjall 3.1.6 confirms journal locking, but it orders arrival, not intent across worker and blocking-pool routes.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L337); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — Store names passed to the durable engine unvalidated

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `unverified`.

fjall 3.1.6 asserts nonempty names of at most 255 bytes; the shim converts the resulting blocking-task panic to Internal. Control/path characters are accepted, but numeric directories refute traversal.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L234); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Fresh random 128-bit id collision-probe justification is false

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

IDs contain wall-clock timestamp plus 64-bit Xoshiro output, not 128 random bits or guaranteed uniqueness.

Evidence: [crates/shamir-types/src/types/record_id.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L41); [crates/shamir-storage/src/storage_fjall.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L154).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — User-influenced keys enter non-keyed FxHash maps

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Public keys/names reach deterministic hashing. rustc-hash 2.1.2 source confirms fixed-seed byte hashing, not the historical simplistic collision/latency narrative.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L154); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007).

Grouping/duplicate: [security-crypto.md#3](security-crypto.md#review-3). This is not an additional independent defect.

<a id="review-3-4"></a>

### Claim 3.4 — Raw key bytes embedded in error messages

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Entire byte arrays are formatted without a length cap; Unicode spoofing and cross-tenant disclosure are not established.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:419](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L419); [crates/shamir-storage/src/key_bytes.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L237).

Grouping/duplicate: [security-crypto.md#4](security-crypto.md#review-4). This is not an additional independent defect.

<a id="review-3-5"></a>

### Claim 3.5 — KeyBytes::Deserialize allocates before any size check

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

ByteBuf materializes bytes before KeyBytes copies long inputs. Decoder-specific limits can constrain allocation; no direct remote KeyBytes path was demonstrated.

Evidence: [crates/shamir-storage/src/key_bytes.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L310); [crates/shamir-wal/src/wal_entry_v2.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L87).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — Reverse range streams drain entire ranges into RAM

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Default reverse collects before first yield; lookup_max requests one entry. Exact scc 3.8.4 reverse support is available.

Evidence: [crates/shamir-storage/src/types.rs:397](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L397); [crates/shamir-index/src/base_index/sorted_index_manager.rs:2174](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/base_index/sorted_index_manager.rs#L2174).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — InMemoryStore eagerly materializes corpus/prefix matches before first yield

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Synchronous collection scales with matches; front draining additionally shifts remaining entries. Payload duplication is not implied.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L153); [crates/shamir-storage/src/storage_in_memory.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L164).

Grouping/duplicate: [performance-hotpath.md#2](performance-hotpath.md#review-2). This is not an additional independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — MemBufferStore::transact drains the entire dirty buffer

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Every nonempty transaction drains all dirty entries using unbounded-size snapshots. Optimization must also repair outstanding-drain ordering.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1037](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1037); [crates/shamir-storage/src/storage_membuffer.rs:600](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L600).

Grouping/duplicate: [performance-hotpath.md#3](performance-hotpath.md#review-3). This is not an additional independent defect.

<a id="review-4-4"></a>

### Claim 4.4 — CachedStore Async uses an unbounded write-behind channel

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

No admission bound exists; queued historical Bytes handles retain allocations when producers outrun the worker.

Evidence: [crates/shamir-storage/src/storage_cached.rs:242](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L242).

Grouping/duplicate: [performance-hotpath.md#4](performance-hotpath.md#review-4). This is not an additional independent defect.

<a id="review-4-5"></a>

### Claim 4.5 — Trait-default range filter scans past the upper bound

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Filtering continues to exhaustion despite ascending keys; Cached inherits this unnecessary traversal.

Evidence: [crates/shamir-storage/src/types.rs:426](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L426).

Grouping/duplicate: [performance-hotpath.md#5](performance-hotpath.md#review-5). This is not an additional independent defect.

<a id="review-4-6"></a>

### Claim 4.6 — FjallStore::submit blocks the async caller on full queue

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

SyncSender::send can block on the bounded queue inside async submit.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L199).

Grouping/duplicate: [concurrency-lockfree.md#4](concurrency-lockfree.md#review-4). This is not an additional independent defect.

<a id="review-4-7"></a>

### Claim 4.7 — Minor allocations/clones on batched paths

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Miss-key vector cloning and fixed 256 initial scan capacities remain; exact speed/reallocation counts are unmeasured.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1245](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1245); [crates/shamir-storage/src/storage_fjall.rs:617](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L617).

Grouping/duplicate: [performance-hotpath.md#7](performance-hotpath.md#review-7). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — Persisted MemBufferConfig has no versioning guardrails

Status: `partially-fixed`. Current risk: `low`.

Prior-cycle decision: `partially-fixed`.

Envelope guards exist; schema evolution/golden fixtures remain absent. Legacy rejection is intentional and compatible with the declared unsupported-alpha-upgrade policy.

Evidence: [crates/shamir-engine/src/table/buffer_config.rs:34](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/buffer_config.rs#L34); [CHANGELOG.md:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CHANGELOG.md#L13).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — batch_size == 0 is unspecified and divergent

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

InMemory full/prefix streams repeatedly drain zero entries; Cached/Fjall terminate empty. MemBuffer forwards zero inward despite clamping output.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L164); [crates/shamir-storage/src/storage_membuffer.rs:923](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L923).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — set/remove flag precision varies without capability disclosure

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Strict set/batch contracts coexist with local-only and TOCTOU flags; no advertised precision capability resolves that mismatch.

Evidence: [crates/shamir-storage/src/types.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L36); [crates/shamir-storage/src/storage_membuffer.rs:763](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L763).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Private system-record prefix literal duplicated across crates

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Constants agree today; constructor-based tests catch current classifier drift. This is maintenance coupling, not demonstrated loss.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L44); [crates/shamir-types/src/types/record_id.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L18).

Grouping/duplicate: [api-wire-protocol.md#4](api-wire-protocol.md#review-4). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — Public-API rustdoc drift: prefetch promise and phantom engines

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Required-method docs promise prefetch absent from implementations and name removed backends.

Evidence: [crates/shamir-storage/src/types.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L291); [crates/shamir-storage/Cargo.toml:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/Cargo.toml#L16).

Grouping/duplicate: [api-wire-protocol.md#5](api-wire-protocol.md#review-5). This is not an additional independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — Shared backend-conformance suite skipped by CachedStore and MirroredStore

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Registered wrapper modules do not call the common batch helper.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L38).

Grouping/duplicate: [api-wire-protocol.md#6](api-wire-protocol.md#review-6). This is not an additional independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — Repo::store_get create-on-read makes typos durable

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Creation is intentional; stores_list is nonmutating and engine table reads first check the catalogue.

Evidence: [crates/shamir-storage/src/types.rs:475](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L475); [crates/shamir-engine/src/repo/repo_instance.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L337).

Grouping/duplicate: [api-wire-protocol.md#7](api-wire-protocol.md#review-7). This is not an additional independent defect.

<a id="review-5-8"></a>

### Claim 5.8 — Interface polish bundle

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Bytes bounds, private naming, copy signature and error-text/taxonomy observations remain optional polish; generic Store methods would break dyn compatibility.

Evidence: [crates/shamir-storage/src/types.rs:336](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L336); [crates/shamir-storage/src/error.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L13).

Grouping/duplicate: [api-wire-protocol.md#8](api-wire-protocol.md#review-8). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — CachedStore::flush hangs if the async worker dies

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

An unwinding inner panic abandons pending jobs without decrement or death notification; later flush waits indefinitely.

Evidence: [crates/shamir-storage/src/storage_cached.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L106); [crates/shamir-storage/src/storage_cached.rs:243](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L243).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Blocking SyncSender::send on tokio executor threads

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Bounded synchronous send remains in async submit.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L199).

Grouping/duplicate: [concurrency-lockfree.md#4](concurrency-lockfree.md#review-4). This is not an additional independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — MemBufferStore::Drop silently discards dirty data

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Drop has neither drain completion nor loss warning. A waiting flusher may perform one post-wake batch; loss is possible, not inevitable.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L339); [crates/shamir-storage/src/storage_membuffer.rs:621](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L621).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — Missing error-path tests and unread flush telemetry

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

flush_errors has no reader; registered tests do not force the specified MemBuffer drain, closed-channel, submit or partial-copy branches.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:355](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L355); [crates/shamir-storage/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/mod.rs#L1).

Grouping/duplicate: [error-handling-lifecycle.md#4](error-handling-lifecycle.md#review-4). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — Cache deletion committed before fallible backing acknowledgment

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Read-through can refill the old value before a delayed successful delete; that stale cache entry survives completion and flush.

Evidence: [crates/shamir-storage/src/storage_cached.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L487); [crates/shamir-storage/src/storage_cached.rs:476](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L476).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-6-6"></a>

### Claim 6.6 — copy_store leaves partially populated destination on failure

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Earlier destination batches survive later errors; overwrite retries leave extras. No rollback/fresh-destination contract is documented.

Evidence: [crates/shamir-storage/src/types.rs:500](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L500); [crates/shamir-engine/src/repo/repo_instance.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L593).

Grouping/duplicate: [error-handling-lifecycle.md#6](error-handling-lifecycle.md#review-6). This is not an additional independent defect.

<a id="review-6-7"></a>

### Claim 6.7 — Fresh FjallStore handles have fragile per-instance worker lifecycle

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Worker-using fresh handles cause creation/join churn. Exact fjall source confirms deleted point-write handles return KeyspaceDeleted rather than freely remaining writable.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L240); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-6-8"></a>

### Claim 6.8 — Flattened error sources and panicking thread-spawn failure

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

String conversion loses sources; OS thread creation can fail environmentally and panic. The latter is a real error-path issue, not an invariant.

Evidence: [crates/shamir-storage/src/error.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L94); [crates/shamir-storage/src/storage_fjall.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L98).

Grouping/duplicate: [error-handling-lifecycle.md#8](error-handling-lifecycle.md#review-8). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Function-local imports violate Imports at the top

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Thirteen listed imports remain in function bodies without applicable policy exceptions; placement has no demonstrated runtime impact.

Evidence: [crates/shamir-storage/src/types.rs:395](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L395); [crates/shamir-storage/src/storage_fjall.rs:451](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L451).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — KeyBytes module doc claims unused RecordKey = Bytes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The module says unused RecordKey=Bytes despite the production KeyBytes alias.

Evidence: [crates/shamir-storage/src/key_bytes.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L7); [crates/shamir-storage/src/types.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L9).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — Orphaned Tests ending banners

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Three ending banners remain; actual external tests are registered.

Evidence: [crates/shamir-storage/src/storage_cached.rs:719](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L719); [crates/shamir-storage/src/tests/mod.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/mod.rs#L3).

Grouping/duplicate: [style-claude-md.md#3](style-claude-md.md#review-3). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — Duplicate private RecordStream aliases

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Internal duplicates could import the existing crate-visible alias; public naming is optional ergonomics.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:628](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L628); [crates/shamir-storage/src/tests/types_tests.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L12).

Grouping/duplicate: [style-claude-md.md#4](style-claude-md.md#review-4). This is not an additional independent defect.

<a id="review-7-5"></a>

### Claim 7.5 — Shared conformance suite missing CachedStore/MirroredStore

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Cached/Mirrored bespoke tests do not invoke all common contract assertions.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L38).

Grouping/duplicate: [api-wire-protocol.md#6](api-wire-protocol.md#review-6). This is not an additional independent defect.

<a id="review-7-6"></a>

### Claim 7.6 — MemBuffer fixture topics remain nested inline modules

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Three topics remain nested in an external registered test file; organizational debt, not orphan tests.

Evidence: [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:728](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L728).

Grouping/duplicate: [style-claude-md.md#6](style-claude-md.md#review-6). This is not an additional independent defect.

<a id="review-7-7"></a>

### Claim 7.7 — Drifted hard-coded line-number reference

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Prefix scan refers to approximately line 323, while iter_stream begins at 596.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:655](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L655).

Grouping/duplicate: [style-claude-md.md#7](style-claude-md.md#review-7). This is not an additional independent defect.

<a id="review-2-pillar-verdict"></a>

### Claim 2.pillar-verdict — Concurrency pillar verdict

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Direct production lock absence holds, but DashMap and dependency internals use locks; separate atomic counters do not prove compound linearizability.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L154); [crates/shamir-storage/src/storage_membuffer.rs:778](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L778).

Grouping/duplicate: [concurrency-lockfree.md#NF-pillar-compliance](concurrency-lockfree.md#review-nf-pillar-compliance). This is not an additional independent defect.

<a id="review-3-boundary-verdict"></a>

### Claim 3.boundary-verdict — Security boundary verdict and hydration filtering

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Hydration reclassifies and warns on rejected keys; no local crypto/unsafe boundary. This historical fix predates the audit and does not authenticate allowed values.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:276](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L276); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:1029](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L1029).

Grouping/duplicate: [security-crypto.md#NF-hydration](security-crypto.md#review-nf-hydration). This is not an additional independent defect.

<a id="review-4-non-findings"></a>

### Claim 4.non-findings — Flag-free Fjall paths and eviction-safe dirty retention

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

No-flag writes omit probes and dirty survives cache eviction. Neither a memory bound nor drain-order correctness follows.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:394](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L394); [crates/shamir-storage/src/storage_membuffer.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L143).

Grouping/duplicate: [performance-hotpath.md#NF-dirty-retention](performance-hotpath.md#review-nf-dirty-retention). This is not an additional independent defect.

<a id="review-5-surface-verdict"></a>

### Claim 5.surface-verdict — Honest supports_atomic_transact capability

Status: `partially-fixed`. Current risk: `medium`.

Prior-cycle decision: `partially-fixed`.

InMemory/Mirrored false is honest; wrapper per-key caches invalidate forwarding. Exact pinned raw Fjall reads also bypass the batch publication watermark.

Evidence: [crates/shamir-storage/src/storage_cached.rs:684](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L684); [crates/shamir-storage/src/storage_fjall.rs:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L505); [Cargo.lock:2014](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2014).

Grouping/duplicate: [SUMMARY.md#NEW.2](SUMMARY.md#new-2). This is not an additional independent defect.

<a id="review-5-test-coverage-notes"></a>

### Claim 5.test-coverage-notes — Registered layout and KeyBytes byte-identity coverage

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Registration and byte-equality assertions hold; local helper matches current WAL source. Only bincode asserts bidirectional cross-decode.

Evidence: [crates/shamir-storage/src/key_bytes/tests/serde_byte_identity_tests.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes/tests/serde_byte_identity_tests.rs#L129); [crates/shamir-wal/src/wal_entry_v2.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L118).

Grouping/duplicate: [api-wire-protocol.md#NF-layout-and-serde](api-wire-protocol.md#review-nf-layout-and-serde). This is not an additional independent defect.

<a id="review-6-panic-verdict"></a>

### Claim 6.panic-verdict — All production panics are unreachable invariant violations

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Worker spawn expect can fail environmentally; invalid Fjall names also panic inside spawn_blocking before conversion to Internal.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L98); [crates/shamir-storage/src/storage_fjall.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L234).

Grouping/duplicate: [error-handling-lifecycle.md#NF-panic-surface](error-handling-lifecycle.md#review-nf-panic-surface). This is not an additional independent defect.

<a id="review-6-dirty-cleanup-verdict"></a>

### Claim 6.dirty-cleanup-verdict — Guarded dirty cleanup retains errors and concurrent writes

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Errors precede cleanup and differing current slots survive remove_if. This narrow historical repair does not serialize outstanding drain writes.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:527](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L527); [crates/shamir-storage/src/storage_membuffer.rs:554](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L554).

Grouping/duplicate: [error-handling-lifecycle.md#NF-dirty-cleanup](error-handling-lifecycle.md#review-nf-dirty-cleanup). This is not an additional independent defect.

<a id="review-6-mirror-first-verdict"></a>

### Claim 6.mirror-first-verdict — Mirror-first primary error atomicity

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Returned mirror errors precede both primary subsets. Actual mirror rollback/durability is not proven by mocks or this ordering.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:595](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L595); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:608](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L608).

Grouping/duplicate: [error-handling-lifecycle.md#NF-mirror-first](error-handling-lifecycle.md#review-nf-mirror-first). This is not an additional independent defect.

<a id="review-6-flush-verdict"></a>

### Claim 6.flush-verdict — Cached flush propagates inner flush and reports background errors once

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Completed waits are followed by inner.flush even after background Err; latest stored failure is consumed once. Worker death/cancellation remains separate.

Evidence: [crates/shamir-storage/src/storage_cached.rs:346](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L346); [crates/shamir-storage/src/storage_cached.rs:397](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L397).

Grouping/duplicate: [error-handling-lifecycle.md#NF-cached-flush](error-handling-lifecycle.md#review-nf-cached-flush). This is not an additional independent defect.

<a id="review-6-notify-verdict"></a>

### Claim 6.Notify-verdict — Create-Notified-before-check pattern

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Pinned Tokio confirms notify_waiters observes creation; no missed-wakeup defect in this pattern while the worker survives.

Evidence: [crates/shamir-storage/src/storage_cached.rs:385](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L385); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

Grouping/duplicate: [error-handling-lifecycle.md#NF-notify](error-handling-lifecycle.md#review-nf-notify). This is not an additional independent defect.

<a id="review-7-structure-verdict"></a>

### Claim 7.structure-verdict — Manifest and external-test structural conformance

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Manifest declarations, feature gates, external test wiring and thiserror are present; no implementation-local test bodies were found.

Evidence: [crates/shamir-storage/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/mod.rs#L1); [crates/shamir-storage/src/key_bytes.rs:315](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L315).

Grouping/duplicate: [style-claude-md.md#NF-structure](style-claude-md.md#review-nf-structure). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 22 | 21 | 0 | 1 | 0 | 0 | 0 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Vectored recheck/test remain absent. A recheck narrows the race; generation validation or publication ordering is needed to close the await gap.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1258).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No shared backing/cache ordering protocol exists. scc upsert is available but cannot alone order commits, read fills, transactions and reload.

Evidence: [crates/shamir-storage/src/storage_cached.rs:403](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L403); [crates/shamir-storage/src/storage_cached.rs:671](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L671); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Worker-death handling is absent. A job guard cannot account for queued abandoned jobs; use terminal worker state and wake all flush waiters, without busy polling.

Evidence: [crates/shamir-storage/src/storage_cached.rs:243](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L243); [crates/shamir-storage/src/storage_cached.rs:383](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L383).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `partially-fixed`.

Prior-cycle decision: `partially-fixed`.

Envelope portion is complete; golden/schema fixtures are not. Respect intentional unsupported-alpha legacy rejection. Header-first dispatch and explicit version schemas are safer than bincode field defaults.

Evidence: [crates/shamir-engine/src/table/buffer_config.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/buffer_config.rs#L44); [crates/shamir-index/src/meta_envelope.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-index/src/meta_envelope.rs#L54); [CHANGELOG.md:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CHANGELOG.md#L13).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

All scan mechanisms remain. Exact scc 3.8.4 supports reverse Range; use exclusive bounds, short Guard scopes and defined zero handling. Incremental scans change current eager snapshot behavior and need an explicit contract.

Evidence: [crates/shamir-storage/src/storage_in_memory.rs:196](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L196); [crates/shamir-storage/src/types.rs:397](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L397); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Whole drain remains. Touched-key cleanup alone is unsafe while older background snapshots remain outstanding; serialize/version their backing application before reducing drain scope.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:353](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L353); [crates/shamir-storage/src/storage_membuffer.rs:1037](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1037).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Queue remains unbounded. Reserve admission before publishing cache/pending state; cancellation at bounded send must not leave an acknowledged-looking cache mutation without a job.

Evidence: [crates/shamir-storage/src/storage_cached.rs:437](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L437); [crates/shamir-storage/src/storage_cached.rs:446](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L446).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Blocking send remains. Async bounded mpsc is viable; semaphore permits must cover queued jobs until dequeue/completion, not be released just after send or by cancelled callers.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L92); [crates/shamir-storage/src/storage_fjall.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L199).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Republish and remove/reinsert remain. Published scc upsert is verified; preserve flag semantics separately and test absence/coherence, not later-completion wins. A dirty comparison alone still races async insertion.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1048); [crates/shamir-storage/src/storage_in_memory.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L129); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No uniform zero or strict existence policy exists. Release-visible handling is required; flag disclosure must not silently weaken callers needing exact bookkeeping.

Evidence: [crates/shamir-storage/src/types.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L36); [crates/shamir-storage/src/storage_in_memory.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_in_memory.rs#L164); [crates/shamir-storage/src/storage_membuffer.rs:886](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L886).

<a id="plan-p1-11"></a>

### Plan P1.11 — P1.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No acknowledged MemBuffer shutdown or readable error counter exists. Drop warnings are observability, not durability; failure tests must exercise real drain/worker seams.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:621](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L621); [crates/shamir-storage/src/storage_membuffer.rs:355](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L355).

<a id="plan-p1-12"></a>

### Plan P1.12 — P1.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Wrapper helper calls remain absent. Preserve immediate-read contract checks; inserting flushes before Async delete assertions would conceal the actual resurrection defect.

Evidence: [crates/shamir-storage/src/tests/types_tests.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/types_tests.rs#L97); [crates/shamir-storage/src/tests/storage_cached_tests.rs:247](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_cached_tests.rs#L247).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Default hashing remains. moka 0.12.15 supports custom hashing, but state/build return types must become Cache&lt;K,V,THasher&gt;; reassess untrusted-input policy before weakening keyed hashing.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:142](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L142); [crates/shamir-storage/src/storage_membuffer.rs:233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L233); [Cargo.lock:2221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2221).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`.

Prior-cycle decision: `unverified`.

Exact fjall source resolves name semantics: empty/&gt;255-byte names assert; numeric directories prevent the alleged traversal. Add typed validation for actual backend limits, not destructive canonicalization of currently accepted names. store_exists is optional.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:234](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L234); [crates/shamir-storage/src/types.rs:475](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L475); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Duplicated prefix remains. Exporting the canonical constant is safe maintenance work; current constructor-based tests already establish agreement.

Evidence: [crates/shamir-storage/src/storage_mirrored.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_mirrored.rs#L44); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:244](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L244).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Narration and diagnostic size policy remain. Avoid claiming monotonic/unique IDs, proven collisions or BiDi output. Enforce any allocation bound in the actual decoder path, not after ByteBuf allocation.

Evidence: [crates/shamir-types/src/types/record_id.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/record_id.rs#L41); [crates/shamir-storage/src/key_bytes.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L237); [crates/shamir-storage/src/key_bytes.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L310).

<a id="plan-p2-17"></a>

### Plan P2.17 — P2.17

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Partial-copy policy remains absent. Self-copy guard is optional; unconditional destination deletion would destroy pre-existing caller-owned contents. Specify fresh staging/ownership before rollback.

Evidence: [crates/shamir-storage/src/types.rs:488](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L488); [crates/shamir-engine/src/repo/repo_instance.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L593).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Overbroad ordering comment remains. Shared-worker architecture is optional and must handle deletion/recreation generations; exact fjall point writes reject deleted handles.

Evidence: [crates/shamir-storage/src/storage_fjall.rs:289](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L289); [crates/shamir-storage/src/storage_fjall.rs:494](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L494); [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332).

<a id="plan-p2-19"></a>

### Plan P2.19 — P2.19

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No negative marker or reload quiescence contract exists. Order deletion confirmations/read fills and publish reload consistently; insertion counter increments must follow successful publication.

Evidence: [crates/shamir-storage/src/storage_cached.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L487); [crates/shamir-storage/src/storage_cached.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L311); [crates/shamir-storage/src/storage_cached.rs:423](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L423).

<a id="plan-p2-20"></a>

### Plan P2.20 — P2.20

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Flattening/spawn expect remain. Prefer a fallible initialization result; Option alone loses the useful spawn error. Preserve DbError codes and avoid backend-type leakage across feature configurations.

Evidence: [crates/shamir-storage/src/error.rs:94](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L94); [crates/shamir-storage/src/storage_fjall.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L98).

<a id="plan-p2-21"></a>

### Plan P2.21 — P2.21

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Listed cleanup remains. Separate optional API redesign and test relocation from mechanical docs/import cleanup; retain object safety and all test registrations. No commit authority is implied.

Evidence: [crates/shamir-storage/src/types.rs:395](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/types.rs#L395); [crates/shamir-storage/src/key_bytes.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/key_bytes.rs#L7); [crates/shamir-storage/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/mod.rs#L1).

<a id="plan-p2-22"></a>

### Plan P2.22 — P2.22

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Miss-key clone and fixed scan capacities remain. Optimization is optional; no measured benefit justifies a prescribed implementation or urgency.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:1245](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1245); [crates/shamir-storage/src/storage_fjall.rs:617](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L617).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 5 | 0 | 0 | 0 | 0 | 1 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="new-1"></a>

### Observation NEW.1 — Buffer-config envelope retrofit rejects previously valid persisted raw configurations

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The rejection and table-open Codec propagation are real and intentional. CHANGELOG explicitly disclaims alpha format compatibility and supported in-place upgrades; source documents the break and a registered test pins it. Treat this as an operational upgrade consequence, not an open violation of a promised legacy-upgrade contract.

Evidence: [CHANGELOG.md:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CHANGELOG.md#L13); [crates/shamir-engine/src/table/buffer_config.rs:34](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/buffer_config.rs#L34); [crates/shamir-engine/src/table/tests/buffer_config_tests.rs:334](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/tests/buffer_config_tests.rs#L334); [crates/shamir-engine/src/table/table_manager.rs:632](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/table_manager.rs#L632).

<a id="new-2"></a>

### Observation NEW.2 — Cache wrappers forward whole-batch visibility atomicity without atomic cache publication

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

With two preloaded keys, pause after publishing the first transaction cache entry: reads can observe new A and old B despite true forwarding. The capability mock delegates transact to non-atomic InMemoryStore; it proves flag plumbing, not atomicity. Native Fjall has a separate latest-read defect described below.

Evidence: [crates/shamir-storage/src/storage_cached.rs:675](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_cached.rs#L675); [crates/shamir-storage/src/storage_membuffer.rs:1048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1048); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:1352](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L1352).

<a id="observation-new-3"></a>

### Observation NEW.3 — Concurrent MemBuffer drains can overwrite a newer successfully flushed value with an older snapshot

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

Background D1 snapshots K=old and stalls before its backing write. A writer acknowledges K=new; foreground flush D2 writes new, removes the matching dirty slot and flushes successfully. D1 then writes old and finds no dirty entry to remove. After cache eviction, reads expose old. Background drain, drain_all and transact have no shared application fence; remove_if protects ownership of dirty entries, not write order. Default buffered info/history paths provide production exposure. The discriminating oracle is raw backing plus wrapper reads after eviction once both drains finish, not merely dirty emptiness. Parent caller qualification: the default Fjall worker orders submitted jobs. The production witness pauses the old drainer after snapshot capture but before submission, allowing the newer flush to submit first; it does not assume a newer queued job overtakes an older job already in that worker.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:353](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L353); [crates/shamir-storage/src/storage_membuffer.rs:504](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L504); [crates/shamir-storage/src/storage_membuffer.rs:527](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L527); [crates/shamir-storage/src/storage_membuffer.rs:554](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L554); [crates/shamir-storage/src/storage_membuffer.rs:600](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L600); [crates/shamir-storage/src/storage_membuffer.rs:1002](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L1002); [crates/shamir-engine/src/repo/repo_instance.rs:433](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/repo_instance.rs#L433).

<a id="observation-new-4"></a>

### Observation NEW.4 — Pinned Fjall raw reads bypass the publication watermark used to advertise native batch visibility atomicity

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

Checksum-matching fjall 3.1.6 src/batch/mod.rs applies individual memtable entries then publishes snapshot_tracker. Its Keyspace::get calls tree.get(key, SeqNo::MAX), and range likewise passes SeqNo::MAX rather than the opened nonce instant. Exact lsm-tree 3.1.6 Tree get and Memtable get use that bound without clamping to the published watermark. A worker preempted between Set(A,new) and Set(B,new) permits raw get_many/range to observe new A and old B while commit is still applying. FjallStore advertises true and uses these raw APIs. Higher MVCC gates may shield some consumers, but do not repair this public storage guarantee. Sources: https://docs.rs/crate/fjall/3.1.6/source/src/batch/mod.rs and https://docs.rs/crate/lsm-tree/3.1.6/source/src/tree/mod.rs.

Evidence: [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332); [Cargo.lock:2014](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2014); [crates/shamir-storage/src/storage_fjall.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L175); [crates/shamir-storage/src/storage_fjall.rs:405](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L405); [crates/shamir-storage/src/storage_fjall.rs:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L505); [crates/shamir-storage/src/storage_fjall.rs:528](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L528).

<a id="observation-new-5"></a>

### Observation NEW.5 — Pinned Fjall WriteBatch discards journal encoding/write errors before reporting successful application

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

Published fjall 3.1.6 src/batch/mod.rs explicitly ignores journal_writer.write_batch's Result, then applies every in-memory item and returns Ok if later persistence succeeds. src/journal/writer.rs returns errors from batch start/item/end writes. Default Database::batch sets Buffer persistence, which catches persistent I/O failures but can succeed after a transient earlier partial-batch failure; missing remaining items/end marker are not regenerated by flushing. The shim maps only commit's returned error and thus can acknowledge an incompletely journaled batch. This is not a claim that every disk-full error succeeds, or that end-to-end WAL recovery necessarily loses every such write. Required oracle: inject a transient journal failure before all batch items/end are emitted, allow subsequent Buffer persistence, and check returned error plus replayed batch—not a fail-before-delegation mirror mock. Source: https://docs.rs/crate/fjall/3.1.6/source/src/batch/mod.rs.

Evidence: [Cargo.lock:1332](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1332); [crates/shamir-storage/src/storage_fjall.rs:169](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L169); [crates/shamir-storage/src/storage_fjall.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_fjall.rs#L178); [crates/shamir-storage/src/tests/storage_mirrored_tests.rs:341](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_mirrored_tests.rs#L341).

<a id="observation-new-6"></a>

### Observation NEW.6 — Persisted MemBuffer max_entries configuration is never enforced despite its hard-cap documentation

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

MemBufferConfig documents a hard entry limit and stores/applies the field, but build_cache explicitly treats it as informational and only configures weighted max_bytes. With max_entries=1 and ample byte capacity, two small live entries are not constrained by the entry-count setting, even after maintenance. Existing capacity tests target bytes rather than independently enforcing entries. This is a concrete configuration-contract divergence, separate from an overall dirty-memory bound.

Evidence: [crates/shamir-storage/src/storage_membuffer.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L100); [crates/shamir-storage/src/storage_membuffer.rs:233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L233); [crates/shamir-storage/src/storage_membuffer.rs:386](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/storage_membuffer.rs#L386); [crates/shamir-storage/src/tests/storage_membuffer_tests.rs:244](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/tests/storage_membuffer_tests.rs#L244).

## Evidence and recipe corrections

- Replace dependency-unavailable qualifications with exact published-source evidence. Checked fjall/lsm-tree 3.1.6, scc 3.8.4 and moka 0.12.15 archives match lock checksums.
- Do not retain a medium open compatibility defect solely for explicitly unsupported legacy alpha upgrades; distinguish the missing prospective schema/golden plan from the intentional fail-closed policy.
- Normal non-MVCC manager deletion pre-reads through the wrapper, and default RepoInstance attaches MVCC and unwraps the data store. The inner-only removal witness is not by itself proof of default-server bookkeeping loss.
- Native Fjall batch atomicity and error propagation must not be assumed from batch naming or the mirror mocks; exact dependencies reveal additional failures.
- Existing fixed NF rows describe narrow historical repairs predating the August audit, not source remediation performed in either October review.
- No fixed 500 ms crash-loss bound or hard overall MemBuffer memory bound follows from the idle interval, weighted cache capacity or healthy-I/O assumption.

## Module scope and limitations

Coverage: 8 assigned documents, 133 current claim rows, 22 plan rows, 2 pre-existing observation rows; 4 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [SUMMARY.md](SUMMARY.md); [api-wire-protocol.md](api-wire-protocol.md); [concurrency-lockfree.md](concurrency-lockfree.md); [correctness-tdd.md](correctness-tdd.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [performance-hotpath.md](performance-hotpath.md); [security-crypto.md](security-crypto.md); [style-claude-md.md](style-claude-md.md).

- Read-only source, documentation, dependency-archive and history inspection only. No builds, tests, benchmarks, reproductions, project programs, downloads, edits or child agents were run.
- HEAD matched the required base; inspected source paths had no working-tree differences. Assigned reports identify their earlier source snapshot as 92ad58266bf57ddea1fa3c8a47affba1a3a9a096.
- Scheduling witnesses are derived from source, not experimentally reproduced. Production exploitability, workload saturation, latency, RSS and crash-loss thresholds were not measured.
- No unauthenticated traversal, cross-tenant disclosure, practical FxHash collision attack or numerical Xoshiro state-recovery threshold was established.
- This is complete assigned-claim revalidation, not an exhaustive fresh storage or dependency audit. No TASK_GROUPS.md was present in the assigned directory.

## Guarantee checks

- **Root reliability guidance: storage owns durability and flush establishes a strict boundary.** — `diverges`. MemBuffer explicitly flushes inner, but independent background and foreground drains can write snapshots out of order. Successful foreground flush does not fence an older outstanding drain. Reference: AGENTS.md:34; crates/shamir-storage/src/types.rs:85; crates/shamir-storage/src/storage_membuffer.rs:1002.
- **Store scans preserve ascending ordering and exclusive continuation.** — `diverges`. Cached/Fjall use exclusive bounds; InMemory range resumes inclusively and blindly skips, losing an untouched successor when the previous cursor is deleted. The explicit Excluded wording belongs to prefix scans, not the range method. Reference: crates/shamir-storage/src/types.rs:293; crates/shamir-storage/src/types.rs:316.
- **Streaming uses constant memory per batch regardless of dataset size.** — `diverges`. InMemory full/prefix scans collect all matches synchronously; default reverse collects the entire range. MemBuffer additionally snapshots all matching dirty entries. Bytes payload sharing does not eliminate result-vector growth. Reference: crates/shamir-storage/src/README.md:351; crates/shamir-storage/src/storage_in_memory.rs:153; crates/shamir-storage/src/types.rs:397.
- **set and remove_many flags describe actual creation/removal.** — `diverges`. MemBuffer ignores inner-only existence. Fjall probes separately from mutation. TableManager normally pre-reads before non-MVCC deletion, so its bookkeeping failure requires additional eviction, expiry or concurrency. Reference: crates/shamir-storage/src/types.rs:36; crates/shamir-storage/src/types.rs:168.
- **supports_atomic_transact reports whole-batch visibility atomicity.** — `diverges`. Cache wrappers publish per key. Additionally, checksum-matching published fjall 3.1.6 and lsm-tree 3.1.6 sources show raw Keyspace get/range using SeqNo::MAX while WriteBatch inserts memtable entries individually before publishing its watermark. Native raw reads can also observe a partial batch. Reference: crates/shamir-storage/src/types.rs:256; crates/shamir-storage/src/storage_fjall.rs:505; Cargo.lock:1332; Cargo.lock:2014.
- **Exact pinned dependencies were unavailable for API and implementation verification.** — `diverges`. Actual CARGO_HOME contained published .crate archives for fjall 3.1.6, lsm-tree 3.1.6, moka 0.12.15 and scc 3.8.4. Their SHA-256 hashes matched Cargo.lock. Sources were inspected through stdout without extraction. Reference: Cargo.lock:1332; Cargo.lock:2014; Cargo.lock:2221; Cargo.lock:3123.
- **scc supports single-key upsert and native reverse ranges.** — `supported`. https://docs.rs/crate/scc/3.8.4/source/src/tree_index.rs contains upsert_sync and DoubleEndedIterator for Range. Leaf upsert publishes replacement metadata with CAS. This removes the shim's explicit remove/reinsert gap, not cross-layer publication races or strict concurrent flags. Reference: Cargo.lock:3123; scc 3.8.4 published src/tree_index.rs and src/tree_index/leaf.rs.
- **moka uses RandomState by default and supports a custom hasher.** — `supported`. https://docs.rs/crate/moka/0.12.15/source/src/future/builder.rs constructs std RandomState in build and returns Cache&lt;K,V,S&gt; from build_with_hasher. Existing Cache&lt;K,V&gt; fields must change with the hasher; the historical one-line recipe is incomplete. Reference: Cargo.lock:2221; moka 0.12.15 published src/future/builder.rs.
- **MemBuffer max_entries is a hard entry-count cap.** — `diverges`. build_cache explicitly treats max_entries as informational and configures only weighted max_bytes. Dirty retention has no admission cap. Reference: crates/shamir-storage/src/storage_membuffer.rs:100; crates/shamir-storage/src/storage_membuffer.rs:233.
- **Persisted buffer configuration has format guardrails and supported legacy upgrades.** — `supported`. Magic/version validation and intentional legacy rejection exist. Alpha upgrades are explicitly unsupported. Header-first migration dispatch and golden compatibility fixtures remain prospective obligations, not an existing supported-upgrade guarantee. Reference: crates/shamir-engine/src/table/buffer_config.rs:34; crates/shamir-index/src/meta_envelope.rs:54; CHANGELOG.md:13.
- **KeyBytes preserves representation-independent bytes and WAL encoding.** — `supported`. Equality, ordering, hashing and serialization use the byte slice. Registered tests compare bincode/rmp encoding to a currently equivalent local WAL helper; cross-decoding is asserted only for bincode. Reference: crates/shamir-storage/src/key_bytes.rs:250; crates/shamir-storage/src/key_bytes.rs:302; crates/shamir-wal/src/wal_entry_v2.rs:118.
- **Notify creation before checking pending avoids missed notify_waiters wakeups.** — `supported`. https://docs.rs/crate/tokio/1.49.0/source/src/sync/notify.rs explicitly makes notify_waiters observable from future creation. This does not detect worker death. Reference: Cargo.lock:4195; crates/shamir-storage/src/storage_cached.rs:383; tokio 1.49.0 published src/sync/notify.rs.
- **Registered tests cover the actual production mechanisms claimed.** — `diverges`. Unit trees are registered and selected by the lib runner, with Fjall feature gating and doctests disabled. However, the deleted-cursor test deletes before scanning, the atomic mock only advertises atomicity, and the concurrent transact test checks dirty preservation rather than wrapper-read coherence. Reference: crates/shamir-storage/src/lib.rs:32; crates/shamir-storage/src/tests/mod.rs:1; scripts/test.sh:177; crates/shamir-storage/Cargo.toml:145.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- Material evidence correction: the current reports say pinned scc/fjall/lsm-tree/moka sources were unavailable. Exact published archives were available under actual CARGO_HOME; four checked archive hashes matched Cargo.lock. upsert, reverse-range, default-hasher, name-validation and deletion semantics are now source-verified.
- Verdict change: security-crypto.md#1 and SUMMARY.md#3.1 change unverified to confirmed-open/low for the missing typed name-limit boundary. Actual numeric keyspace directories refute the proposed path-traversal mechanism; invalid-name panics are captured as Internal.
- Verdict change: SUMMARY.md#NEW.1 changes confirmed-open/medium to not-applicable/none as a defect. Legacy rejection remains a real documented operational consequence, but CHANGELOG explicitly rejects the supposed supported-alpha-upgrade obligation.
- Severity qualification: api-wire-protocol.md#1 and SUMMARY.md#5.1 remain partially-fixed but low; envelope work is proven by the September source diff, while missing future schema/golden work does not establish a current medium upgrade defect.
- Recipe correction: exact moka build_with_hasher returns a differently parameterized Cache, so the historical one-line replacement is incomplete. Exact scc supports both upsert_sync and reverse Range, but neither proves cross-layer ordering or completion-last wins.
- Reachability correction: the current inner-only removal narrative overlooks TableManager's pre-read warming and default MVCC data-store unwrapping. Direct library divergence is confirmed; bookkeeping loss needs additional cache eviction/expiry/concurrency or a different configured seam.
- Oracle correction: AtomicTransactMock is not genuinely atomic; its transact delegates to InMemory. ConcurrentWriterInner discards the original transaction ops. Their existing assertions prove flag forwarding and differing-dirty retention respectively, not native atomicity or wrapper-read coherence.
- The three additional high mechanisms are source-derived, not experimental results: unordered outstanding drains, native latest-read watermark bypass, and ignored journal batch errors. They challenge broad existing assurances without counting assigned cache-wrapper or cleanup claims twice.
- Existing narrow historical fixes and refutations are supported, including numeric-byte Debug, key-preserving self-copy, registered ordinary get_many coverage, mirror-first primary error ordering, unconditional Cached inner.flush after a completed wait, and Tokio notify_waiters creation semantics.

## Current follow-up order

1. Establish one authoritative ordering/ownership protocol for outstanding MemBuffer drains, transaction publication and read fills; test wrapper reads, backing state and post-eviction visibility.
2. Resolve exact pinned native Fjall visibility and journal-error behavior before relying on supports_atomic_transact or mirror batch guarantees; distinguish raw-library exposure from higher MVCC/WAL protection.
3. Repair Cached backing/cache ordering, including Async jobs versus direct transact and delete/read-fill resurrection; use controlled same-key interleavings.
4. Make Cached flush observe terminal worker death and bound admission with cancellation-safe async backpressure; remove Fjall runtime-blocking submission.
5. Fix exclusive range resumption and define zero-size behavior across every stream path; then implement verified native reverse/incremental scans with explicit concurrent-read semantics.
6. Repair or accurately disclose flag precision, wrapper atomicity and max_entries behavior; add missing shared and failure-path oracles without weakening immediate-read assertions.
7. Define acknowledged dirty shutdown and destination ownership/failure policy. Keep intentional unsupported-alpha legacy rejection explicit rather than treating migration as an existing guarantee.
8. Perform lower-priority name-error typing, diagnostics, hashing-policy and style/documentation cleanup without speculative exploitation or performance claims.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-storage — Consolidated 7-lens review (synthesis of the 2026-08-14 cross-crate sweep)

Crate: `crates/shamir-storage/` — the storage spine: the `Store`/`Repo` KV abstraction
(`types.rs`), five backends/wrappers (`InMemoryStore`, `FjallStore` on the fjall LSM,
`MemBufferStore` write-back buffer + moka cache, `CachedStore` write-through/write-behind,
`MirroredStore` mirror-first hybrid), and the `KeyBytes` record-key type. Read-only
synthesis pass — no build/test/lint commands were run and no source file was modified.

Review basis: the seven 2026-08-14 lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — read in full and merged. Structure/tone/dedup conventions
calibrated on the two finished exemplars:
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`. The raw
lens-tagged counts below (53) match this crate's row in the workspace
`SUMMARY.md` breakdown table (context only).

Dedup convention: where the same root-cause defect was flagged by multiple lenses, the
full write-up lives once under its primary lens; the other lenses carry a
`*(primary: X.Y)*` stub. A deduped defect's severity of record is the **highest**
severity any lens assigned to it (the range is noted in the entry). Spot-checks during
synthesis (`storage_membuffer.rs:1243-1260`, `storage_in_memory.rs:184-231`,
`storage_cached.rs:242`, `storage_fjall.rs:92-98`, `types.rs:9` vs `key_bytes.rs:1-9`)
confirmed the load-bearing file:line references; nothing new was found worth adding.

## Executive summary

The crate's foundations are unusually good for a storage layer — contract-grade trait
docs, pillar-clean concurrency primitives (`THasher` on `dirty`, ArcSwap hot-swap,
Release/Acquire mirror comments), deep race-regression suites where it has been attacked
(#539/#535, F-41/F-49/F-59/F-77), and zero `unsafe` — but it is **not shippable as-is**:
two unguarded cache-fill/republish races (`MemBufferStore::get_many` missing the proven
#539 tombstone guard; `CachedStore`'s three unordered remove+insert mutation sites) can
permanently mask acked writes, and `CachedStore::flush()` can park forever if the async
write-worker dies. Fix those three liveness/silent-loss defects first (P0 items 1–3),
then put versioning guardrails on the persisted `MemBufferConfig` blob before any further
schema churn (P0 item 4) — an unversioned change there breaks every existing database at
open. The O(range)-memory scan class (reverse streams, eager whole-corpus materialization
on InMemoryStore) is the top P1 theme.

---

## 1. correctness-tdd

### 1.1 — high — `MemBufferStore::get_many` cache-fill can poison a Tombstone over a concurrent write — the #539 bug class survives in the vectored read
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:1243-1260` (fill loop; ungated
  dirty probe at :1231), contrasted with the guarded single-key path at :840-880 (guard
  rationale documented at :817-867). *(primary: also flagged by concurrency-lockfree — §2.2)*
- Issue: Single-key `get()` documents (#539 "tombstone-poisoning guard") that after an
  `inner.get()` round-trip it MUST re-check `dirty` before touching moka, because moka gives
  no ordering between two independent tasks' inserts to the same key beyond last-physical-
  write-wins, and a stale reader-inserted `Tombstone` landing after a writer's `Live`
  republish masks the write on every subsequent get until evicted or overwritten — a LASTING
  mask (default config has no TTL). `get_many` performs the identical miss →
  `inner.get_many` → `cache.insert(k, Slot::Tombstone/Live)` sequence with **no dirty recheck
  at all** (:1252-1258 insert unconditionally — verified). Per the concurrency lens: the
  window is not even narrowed to the single-call tail as in fixed `get()` — it spans the
  entire batched backend round-trip for every key in the batch, and covers writer `Live`
  values, not just tombstones. Additionally `get_many` has zero coverage in
  `storage_membuffer_tests.rs` (no `get_many` test anywhere in that suite).
- Failure scenario: Reader task R issues `get_many([K])`; R's probe of `dirty` misses (stale/
  invisible `dirty_count==0` per #539's accepted window). Writer task W completes
  `set(K, v)` (dirty insert + cache `Live` republish). R's slow `inner.get_many(K)` returns
  `None` (pre-flush); R then inserts `cache[K] = Tombstone` after W's republish. Every
  subsequent `get(K)` short-circuits on the cache-first branch and returns `NotFound`
  indefinitely — read-your-write broken permanently for that key until capacity eviction or
  another write.
- Suggested fix: Port the `get()` guard verbatim into the fill loop: before each
  `cache.insert(k, slot)`, re-check `dirty_count > 0 && dirty.get(&k).is_some()` and skip the
  fill when raced. Port `drain_clear_race_does_not_mask_acked_write`'s hook-based pattern
  into a `get_many` regression test (pause hook injected into the `inner.get_many` window).

### 1.2 — medium — `MemBufferStore::transact` post-commit cache republish clobbers a concurrent writer's fresher value — lasting stale read
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:1043-1084` (esp. unconditional
  `cache.insert(k, Live(v))` at :1048 and `Tombstone` at :1071; the `remove_if` guard
  protects only `dirty`, not the cache).
- Issue: The audit §2.3 fix correctly guards `dirty` with `remove_if(slot == snapshot)`, so a
  concurrent `set` landing during `inner.transact` keeps its dirty entry. But the cache
  update is unconditional: the call re-inserts its own (now older) value into moka with no
  comparison against current state. By the module's own #539 reasoning, ordering between the
  two tasks' moka inserts is undefined, so the transact's stale value can land in the cache
  after the concurrent writer's fresh one. Reads hit cache-first and return the pre-concurrent
  value; `dirty` holds the newer entry (so it eventually reaches inner), but the stale cache
  entry wins every read until eviction/overwrite/TTL (None by default).
- Failure scenario: T1 runs `transact([Set K v1])`; during T1's I/O-length `inner.transact`,
  T2 completes `set(K, v2)` (dirty + cache = v2). T1's post-commit loop inserts
  `cache[K] = v1`. All subsequent `get(K)` return v1 although v2 was ACKed and sits in `dirty`.
- Suggested fix: Same family as 1.1 — guard the republish (skip when `dirty.get(&k)` holds a
  different slot than this op's value), or make the guard symmetric for both layers via one
  helper. Add a wrapper-injected concurrency test mirroring
  `transact_does_not_lose_concurrent_set` but asserting the READ side too (the current test
  only checks what reaches `real_inner`).

### 1.3 — medium — `InMemoryStore::iter_range_stream` resumes inclusive + blind-skip instead of the mandated `Bound::Excluded` cursor — silent record drop under concurrent delete/update mid-scan
- File:line: `crates/shamir-storage/src/storage_in_memory.rs:184-231` (skip-first hack at
  :200-208, inclusive resume construction at :195-197 — verified); contract violated:
  `types.rs:316-336` ("each batch resumes strictly past the previous batch's last key
  (`Bound::Excluded`) — every implementor MUST uphold"); also contradicted by
  `src/README.md:366-369`, which claims InMemoryStore uses the same Excluded pattern as
  CachedStore/Fjall.
- Issue: Batches ≥ 2 seek `range(resume..)` — INCLUSIVE of the previous batch's last key —
  and compensate by unconditionally skipping the first yielded item
  (`skip_first = !first_batch`). That equivalence only holds if the resume key still exists
  when the next batch queries. If it was removed between batches (a plain `remove`, or
  `set`'s own remove→insert update window at `storage_in_memory.rs:120-131`), the iterator
  starts at the successor S and the blind skip consumes S — one unseen record silently
  vanishes from the scan. (Interaction note for 4.1: this same body is what the perf lens
  proposes as the template for the missing reverse-stream overrides — fix the resume
  discipline first so the template is sound.)
- Failure scenario: A range scan over posting/version keys with `batch_size < matches`; the
  batch-boundary key is deleted or updated concurrently while the consumer awaits the next
  batch. The successor of that key is dropped from results — no error, wrong query output.
  Reachable through `MirroredStore` (primary IS InMemoryStore) and
  `MemBufferStore::iter_range_stream` delegating to an InMemoryStore inner.
- Suggested fix: Mirror the sibling implementations exactly: carry `last_key` and query
  `(Bound::Excluded(last), Unbounded)` like `storage_cached.rs:553-555` does against the same
  scc type. Add a regression test interleaving a boundary-key removal between batch pulls.

### 1.4 — medium — `MemBufferStore::remove`/`remove_many` misreport the existed flag for keys resident only in `inner` — violates the `Store::remove` contract, unobservable to the shared test
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:884-894` (flag computed from
  dirty/cache only — no inner fallback), :1173-1191 (same for `remove_many`); contract:
  `types.rs:66`, `types.rs:168-171`; test gap: `types_tests.rs:97-104`
  (`run_batch_store_tests` remove section). Related API-level framing: §5.3 (distinct
  disclosure issue, not deduped).
- Issue: `existed` is derived exclusively from `dirty` + cache. A key that exists durably in
  `inner` but has never been read/written through the buffer reports `false` while being
  removed — the opposite direction of failure versus `FjallStore::remove` (which does the
  real lookup) and `CachedStore::Sync` (which delegates). The shared batch suite never
  catches this because its removal targets always pass through `insert_many`/`set_many`
  first and are therefore always present in dirty/cache — the "clean-key removal" branch is
  vacuously uncovered for this backend. Per inline comments, engine callers consume these
  flags (`delete_returning_version` per `storage_fjall.rs:343-344`), so a "delete didn't
  exist" answer for a row that did exist is caller-visible. (`set`'s best-effort flag is
  explicitly documented at :763-768; `remove` carries only a pointer to that rationale, and
  none of its justification applies — set's fallback errs toward the common case, remove's
  omission errs against reality.)
- Failure scenario: Engine opens a hybrid table (`MemBuffer(Fjall)`); deletes row K never
  touched this session; the buffer-layer delete returns `Ok(false)` though Fjall held and
  lost K; any logic branching on the flag (stats, conditional deletes, dedup) sees "nothing
  deleted".
- Suggested fix: Either fall back to the actual effect (consult a cheap existence check, or
  forward to `inner.remove_no_flag` + a `contains_key` equivalent via `get`), or amend the
  trait doc to mark MemBuffer's flag as advisory and add a red test proving the intended
  semantics so the divergence is at least deliberate.

### 1.5 — low — TDD gap: `CachedStore` (and `MirroredStore`) never run the backend-agnostic batch contract suite
- File:line: suite at `crates/shamir-storage/src/tests/types_tests.rs:38`
  (`run_batch_store_tests`); call sites only `storage_in_memory_tests.rs:77`,
  `storage_membuffer_tests.rs:33`, `storage_fjall_tests.rs:125`; nothing in
  `storage_cached_tests.rs` (33 tests) or `storage_mirrored_tests.rs` (18 tests) invokes it.
  *(primary: also flagged by api-wire-protocol §5.6 and style-claude-md §7.5)*
- Issue: The crate's central Red/Green instrument — asserting `insert_many`/`set_many`
  flags, empty-input behavior, `get_many` order-and-None semantics, post-flush consistency,
  and `iter_range_stream_reverse` high→low ordering — excludes the two backends that most
  need it: `CachedStore` overrides the very methods these defaults loop over (`set`/`remove`
  per-mode branches) and has the mode (Async) whose whole point is deferred durability;
  `MirroredStore` inherits defaults through delegation (per the api lens; its bespoke suites
  cover mirror atomicity but not the common batch contract). The cross-backend invariant
  sweep — including "flags preserve input order" through the Async enqueue path and
  reverse-range default-impl composition — has never been asserted against either.
- Failure scenario: A future refactor of `CachedStore::set`'s Async branch (e.g. moving the
  `pending_writes.fetch_add`) or a delegation-path regression in either wrapper breaks flag
  ordering, empty-batch, or reverse-order contracts with no failing test — the safety net
  designed to catch precisely that does not exercise them.
- Suggested fix: Add `cached_sync_passes_full_batch_suite` /
  `cached_async_passes_full_batch_suite` over an in-memory inner, and (with a toy
  classifier) a `MirroredStore` over `InMemoryRepo` doing the same; keep Fjall/MemBuffer
  coverage as-is.

### 1.6 — low — `FjallStore` write-ordering claim holds only per handle-instance; `Repo::store_get` hands out a new instance per call (each lazily spawns its own OS worker)
- File:line: `crates/shamir-storage/src/storage_fjall.rs:240-244` (fresh `FjallStore` per
  `store_get`), :309-319 (`OnceLock` worker per instance), :494-495 ("ordered against every
  other point-write on this store"), :34-46 (`set`/`remove` bypass the worker by design).
  *(primary: also flagged by concurrency-lockfree §2.8 — the worker-vs-`spawn_blocking`
  cross-path interleave — and error-handling-lifecycle §6.7 — the per-instance worker
  lifecycle/churn facet)*
- Issue: Three facets of one structural fact. (a) Ordering (correctness lens): two handles to
  the same keyspace (the engine refetches stores; e.g. the `__tx__` marker store per commit)
  each lazily spawn their OWN worker once they submit anything, so total order across
  `insert`/`transact` vs `set`/`remove` vs the other instance's worker does not exist —
  fjall's journal-writer mutex serializes execution but not intent order; the :494-495
  comment overstates the guarantee. (b) Cross-path interleave (concurrency lens): even within
  one handle, worker-routed ops (FIFO-submission-ordered) and `spawn_blocking`-routed ops
  (`set`/`set_no_flag`/`remove`/`remove_no_flag`, arbitrary pool scheduling) reach fjall in
  whatever order each path acquires the journal mutex — the comment never names this
  interleave; §B13's TableManager serialization neutralizes it in practice, but only by
  prose. (c) Lifecycle (error lens): unlike `InMemoryRepo` (which caches `Arc<dyn Store>`
  per name in a `TDashMap`), every `store_get` builds a new `FjallStore` with a fresh
  `OnceLock`; the design leans entirely on the documented convention that short-lived
  instances must never issue `insert`/`transact`, else each call spawns an OS-thread write
  worker just to abandon it (spawn+join churn per transaction); outstanding handles also keep
  operating on a keyspace after a concurrent `store_delete` (backend-dependent errors).
  Correctness is preserved today (each op atomic; §B13 assumes no concurrent same-key
  writers) — all three lenses rate this low/nit.
- Failure scenario: A future commit path submits one `insert` through the per-commit marker
  store → silent OS-thread create/join storm on the hot path (invisible until
  bench/flamegraph); DDL churn multiplies idle threads (one per store instance ever used for
  insert/transact); and the documented "ordered against every other point-write" invariant is
  silently false across handles, so a routing change can widen the race unnoticed.
- Suggested fix: Reword the invariant to scope it to a single handle and name the
  worker-vs-`spawn_blocking` interleave in the §B13 block; share the worker per-keyspace via
  `Arc<Database>` + a name-keyed `Arc` registry (mirroring `InMemoryRepo`) if the guarantee
  matters — that also removes the churn hazard. Document the thread-per-handle cost next to
  the lazy-spawn rationale.

### 1.7 — low *(as tagged here)* — *(primary: 2.3)* — `InMemoryStore::set` update path can resurrect an older value under concurrent same-key writers
- The concurrency lens carries the full write-up (it rated the same defect **medium** — the
  severity of record; the correctness lens rated it low, noting the inline "single-session
  in-memory backend" acknowledgment and §B13 consistency).

### 1.8 — nit — `CachedStore` size-counter drift: increment on rejected duplicate insert
- File:line: `crates/shamir-storage/src/storage_cached.rs:420-424` (size incremented even if
  `insert_sync` rejects a duplicate — practically unreachable given fresh `RecordId`s).
- Issue/fix: Increment only on `is_ok()` in `insert`; guard the O(1) counter mandated by
  CLAUDE.md §O(x→0). (The finding's other half — non-atomic `reload()` at :306-327 — is the
  same defect as §2.7 and is counted once there.)

### 1.9 — nit — `Repo::copy_store` default has no `from == to` self-copy guard
- File:line: `crates/shamir-storage/src/types.rs:488-503`. Related (distinct defect, not
  deduped): §6.6 partial-failure orphaning on the same method.
- Issue: Passing equal names streams the source onto itself (every record doubled; the
  monotonic cursor prevents an infinite loop but not the corruption). RENAME TABLE passes
  distinct names today.
- Suggested fix: Early-return `DbError::Validation("copy_store: from == to")`.

### 1.10 — nit — Stale narration of the retired #535 mechanism in race-hook docs
- File:line: `crates/shamir-storage/src/membuffer_clear_race_hook.rs:1-19` and
  `storage_membuffer_tests.rs:845-866` still narrate the boolean sentinel's "store(false)" /
  "verify-after-clear restore" flow that #539's `dirty_count` redesign removed (the hook
  module itself admits the exercised interleaving "no longer reproduces a masked write").
  (The finding's other half — mid-body `use` statements — is the same defect as §7.1 and is
  counted once there.)
- Suggested fix: Trim the dead mechanism narrative to one line pointing at #539.

---

## 2. concurrency-lockfree

Pillar verdict (from the lens, kept for calibration parity): production code contains zero
`std::sync::Mutex`/`RwLock`/`parking_lot` (only test-fixture Mutexes, correctly cloned-out
before every `.await`), no `scc::*::len()`, `THasher` on the `dirty` DashMap, ArcSwap RCU
for the moka hot-swap, and exemplary Release/Acquire-pairing comments on the atomic
cardinality mirrors. The real findings are semantic races in multi-step mutation sequences.

### 2.1 — high — CachedStore: unordered, non-atomic cache-mutation sites can leave the cache permanently behind `inner` (silent acked-write loss, size-counter drift)
- File:line: `crates/shamir-storage/src/storage_cached.rs:403-411` (`cache_upsert`),
  :427-467 (`set`), :469-485 (`get` lazy fill), :151-168 (`CacheAction::apply`), plus
  :306-327 (`reload`).
- Issue: Every cache mutation is a two-step `remove_sync` + `insert_sync` pair, and there are
  three independent mutation sites (`cache_upsert`, lazy-fill `insert_sync`, transact-
  populate `apply`) with no shared ordering primitive between them. Consequences under
  concurrent same-key traffic:
  1. **Silent lost update returning `Ok`.** In `WriteMode::Sync`, thread A runs
     `inner.set(K,vA)` then threads B/A race their `cache_upsert`s: A's `remove_sync(K)`
     succeeds, B's returns `false` (key transiently absent), B bumps `size` believing K new;
     B's `insert_sync(K,vB)` wins, A's re-insert hits `Duplicate` and is discarded by
     `let _ = self.cache.insert_sync(key, value)` — A's write vanishes from the cache while
     `inner` holds B. Whichever upsert lands last, not whichever committed to `inner` last,
     owns the cache.
  2. **Persistent read-your-write violation.** Cache-update order is decoupled from
     `inner`-update order: A(`inner.set` vA) → B(`inner.set` vB) → B(`cache_upsert` vB) →
     A(`cache_upsert` vA) leaves cache=vA vs inner=vB **indefinitely** — every subsequent
     `get` serves the stale value (cache-first, :471). Same shape via `transact`'s
     post-commit populate racing a standalone `set`.
  3. **Counter drift:** each such interleaving bumps `size` for one logical entry;
     `cache_size()` telemetry creeps upward (correctness-neutral — nothing gates eviction on
     it).
  No lock-free fix exists inside scc for "replace and keep order", but
  `TreeIndex::upsert_sync` (present in vendored scc 3.8.4, `tree_index.rs:393`) at least
  removes the removal window and the swallowed-Duplicate loss; #616 pt.2 already
  demonstrated the ordered-worker pattern for exactly this class of bug in Async mode.
- Failure scenario: Engine calls through `BoxRepoFactory::cached(inner, Sync)` while an
  online index build and live writes touch the same posting keys; a writer gets
  `Ok(created=…)` but its value never becomes visible to readers until some later write or
  reload of that key.
- Suggested fix: Route both Sync and Async-mode cache mutations through the single ordered
  worker (or collapse remove+insert into `upsert_sync` and make fill/populate go through one
  helper); accept-and-document remaining flag TOCTOU like Fjall §B13 does; derive `size`
  updates only inside that helper. Add a same-key concurrent-writer regression test
  (`test_cached_concurrent_access` currently uses 50 *distinct* keys, so this family is
  untested).

### 2.2 — high — *(primary: 1.1)* — `MemBufferStore::get_many` missing the #539 tombstone-poisoning guard
- Same root defect as §1.1 (full write-up there, including this lens's additions: the window
  spans the whole backend round-trip, covers `Live` values too, and `get_many` has zero test
  coverage).

### 2.3 — medium — InMemoryStore::set: remove+re-insert update path built on a false premise ("no update-in-place API") — swallows a racing Duplicate so a later-completing `set` can return Ok while its value never lands
- File:line: `crates/shamir-storage/src/storage_in_memory.rs:115-135`; contradicting
  evidence: `src/storage_mirrored.rs:563-566` (lists `upsert_sync` among TreeIndex
  mutations) and vendored `scc-3.8.4/src/tree_index.rs:393` (`pub fn upsert_sync(&self, key:
  K, val: V)`); crate dep is `scc = "3.8"`. *(primary: also flagged by correctness-tdd §1.7,
  rated low there)*
- Issue: The comment claims "`scc::TreeIndex` has no update-in-place API so 2 traversals are
  unavoidable" — false for scc 3.8.x, which provides single-traversal `upsert_sync`. Beyond
  the cost claim being wrong, the Err branch (`remove_sync(&k); let _ = insert_sync(k, v)`)
  has a multi-writer interleaving where BOTH concurrent setters take the Err branch, A
  removes, B's remove no-ops, A inserts its value, and B's final `insert_sync` fails
  Duplicate — discarded silently — so **B completes `Ok(updated=true)` but its value never
  reaches the store** (A's, the earlier caller's, wins). This is distinct from (and worse
  than) the acknowledged "brief absence" window in :126-128. Exposure is bounded by the same
  engine-level serialization Fjall §B13 leans on ("the engine never issues two concurrent
  `set` calls for the same key"), but unlike §B13 that acknowledgment is absent here and the
  safer primitive is one call away. Note: `MirroredStore::set`'s error-atomicity argument
  relies on primary writes being infallible (which holds), but nothing there orders two
  concurrent setters either.
- Failure scenario: Any non-engine/tooling caller issuing two overlapping `set`s on one
  hybrid-table key; the second call acks success yet restart-time hydration (which streams
  the primary/mirror) resurrects only the first value.
- Suggested fix: Replace the Err branch with `self.data.upsert_sync(k, v)` (keep the
  `insert_sync` fast-path for the fresh-key case if desired); flag accuracy degrades to
  documented-best-effort (parity with Fjall/MemBuffer semantics), value-loss disappears.
  Correct the comment; add a two-task same-key set test asserting the completing-later write
  wins.

### 2.4 — medium — FjallStore::submit blocks the tokio executor thread when the 1024-slot worker queue fills
- File:line: `crates/shamir-storage/src/storage_fjall.rs:92-93` (`sync_channel(1024)` —
  verified), :188-201/:194-208 (`tx.send(...)` called directly in `async fn submit`), call
  sites :330-334 (`insert`) and :496-500 (`transact`). *(primary: also flagged by
  performance-hotpath §4.6 — rated low there — and error-handling-lifecycle §6.2)*
- Issue: Pillar 2 requires I/O-bound ops to be async / CPU-blocking work off the runtime.
  Here the enqueue onto the OS-thread worker is a synchronous
  `std::sync::mpsc::SyncSender::send` executed on the async caller's task. The comment says
  "a full queue simply parks the submitting task" (:90-91) — it actually **parks the runtime
  worker thread** running that task (blocking send, not task park): every other task
  scheduled on that core-thread stalls too. Under sustained commit pressure (batch commits
  are slow disk ops on the drain thread) ≥N_workers blocked submitters parked across
  different tokio workers removes the executor from service until the single fjall drain
  thread catches up — a latency cliff/live-lock shape rather than backpressure. FIFO
  ordering and backpressure intent are sound (contrast: the deliberate 1024 bound prevents
  OOM); the wait mechanism is what violates the model.
- Failure scenario: A 4096-task fan-in bulk import (`transact`/`insert` storm while the
  worker drains fsync-bound commits): 3072 tasks sit blocked on `send` occupying tokio
  worker threads; remaining runtime capacity starves — unrelated timer/IO tasks on those
  workers stop firing until the drain drains; SLOW/TIMEOUT-class symptoms under load.
- Suggested fix: Keep the single OS-thread worker + bounded channel, but gate submission
  with a `tokio::sync::Semaphore` acquired via `.await` before the blocking `send`
  (preserves the no-extra-hop property and bounds in-flight submitters to the queue depth);
  or `try_send` first and hop the rare blocking send into `spawn_blocking` (SyncSender is
  Clone/Send); or switch the front half to `tokio::sync::mpsc::channel(1024)` with
  `.send().await` consumed by the dedicated thread via a small adapter — preserves the
  measured worker wins, removes the sync-block-in-async case.

### 2.5 — low — moka cache built with default `RandomState` instead of workspace `THasher` (pillar 4)
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:232-255` (`build_cache` ends in
  plain `.build()`; the `THasher` import is already present at :84 for the dirty map).
- Issue: CLAUDE.md pillar 4 names `THasher` the default for every hash-keyed structure.
  `MemBufferStore` sits directly on the default disk stack (`BoxRepoFactory`:
  MemBuffer→Fjall), so every hot op hashes RecordKey twice+ (cache get + insert; weigher
  lookups) via SipHash-class hashing instead of FxHash. moka 0.12 supports this
  (`CacheBuilder::build_with_hasher`, verified in vendored moka 0.12.x builder docs); no API
  obstacle.
- Suggested fix: `builder.build_with_hasher(shamir_collections::THasher::default())`.

### 2.6 — low *(as tagged here)* — *(primary: 4.2)* — `InMemoryStore` iter_stream/scan_prefix_stream eagerly materialize the whole result set under one pinned epoch Guard
- The performance lens carries the full write-up (it rated the same defect **high** — the
  severity of record). This lens's added facet: the `scc::Guard` pinned across the whole
  collect delays EBR reclamation of nodes removed concurrently during large scans.

### 2.7 — low — CachedStore::reload is non-atomic clear→refill against live traffic
- File:line: `crates/shamir-storage/src/storage_cached.rs:306-327`. *(primary: also flagged
  by correctness-tdd §1.8 — the reload half of its size-counter-drift nit)*
- Issue: `clear()` + `size.store(0)` followed by a streamed refill: concurrent Async-mode
  `cache_upsert`s can repopulate keys mid-refill whose values then lose the insert race
  against older streamed rows (stale refill overwrite of a fresher write persists until the
  next same-key write/reload); `size` accounting drifts while readers/writers interleave
  with the refill counter bumps. Reads are safe (fall-through to inner). Likely DDL-time-only
  usage today, hence low.
- Suggested fix: Document reload as requiring quiescence (it is a resync/debug surface), or
  snapshot-diff into the live tree rather than clear-first.

### 2.8 — nit — *(primary: 1.6)* — Cross-path write ordering inside FjallStore rests only on fjall's internal journal mutex arrival order
- Folded into §1.6 facet (b); the standalone nit record is preserved there
  (`storage_fjall.rs:337-389` `set`, :550-580 `remove`, :17-47 worker rationale).

---

## 3. security-crypto

Boundary verdict (kept for calibration parity): no auth/crypto/TLS surface lives here and
the crate contains **zero `unsafe` blocks** (the only grep hit is a doc sentence in
`key_bytes.rs` explaining why an `unsafe` union layout was *rejected*). The one untrusted
input it owns — tampered on-disk mirror content at hydration — is handled well
(`MirroredStore::new` re-runs the allowlist classifier against every streamed entry and
skips+warns on drift — `storage_mirrored.rs:276-284`, backed by classifier-exhaustiveness
and hydration-drift tests). Timing side-channels: nothing here compares secret material, so
`KeyBytes`' non-constant-time slice equality is not exploitable as written.

### 3.1 — medium — Store names passed to the durable engine unvalidated
- File:line: `crates/shamir-storage/src/storage_fjall.rs:229-245` (`FjallRepo::store_get`),
  :247-262 (`store_delete`); `copy_store` (types.rs:488) composes onto this unchecked too.
- Issue: Both methods forward `name.as_ref()` straight into
  `Database::keyspace(&table_name, ...)` (and `delete_keyspace`) with no validation
  whatsoever: empty string, whitespace-only, control characters, absurd length,
  delimiter/path-flavored characters, or names engineered to collide with the engine's
  composed prefixes (`__data__<t>` / `__info__<t>` / `__history__<t>`) are all accepted and
  become durable on-disk artifacts. Whether fjall internally rejects pathological names was
  not verified; the crate neither relies on nor documents any guarantee, so the boundary
  simply trusts every caller. (Related, distinct: §5.7 create-on-read semantics.)
- Failure scenario: If a client-controlled DDL table name reaches `Repo` uncanonicalized
  (validation lives outside this crate), one request can mint or delete persistent storage
  artifacts under manipulated names, alias across composed store namespaces after a rename
  cycle, or wedge `stores_list()` consumers with invisible/control-character names.
- Suggested fix: Validate once at the `Repo` boundary — reject empty, over-length, and
  non-printable/non-ASCII names with `DbError::Validation`; canonicalize before calling
  fjall. Cheap O(1) guard on a cold path, converts a transitive trust assumption into a
  checked invariant.

### 3.2 — low — "Fresh random 128-bit id" claim behind skipping the insert collision probe is false (timestamp + 64-bit PRNG tail)
- File:line: `crates/shamir-storage/src/storage_fjall.rs:152-156` (`exec_insert`) and
  :324-329 (`Store::insert`); same claim in `benches/storage_fjall_pump.rs:96-101`.
- Issue: Both comments justify dropping the pre-insert `contains_key` probe with
  "`RecordId::new()` is a fresh random 128-bit id ... ~2^-128". The referenced
  implementation says otherwise (`shamir-types/src/types/record_id.rs:24-54`, :80-90): bytes
  `[0..8]` are wall-clock microseconds (fully predictable), bytes `[8..16]` come from a
  thread-local **Xoshiro256++** — deliberately *not* a CSPRNG, seeded once per thread from
  OS RNG. Predictability is 50% of the id and the random part carries 64 bits from an
  xoshiro stream that is computationally invertible/predictable after ~32 observed
  consecutive outputs per thread. The engineering *decision* (skip the probe) remains sound
  — distinct-microsecond timestamps dominate separation and the same-microsecond tail
  birthday bound is ample — but the written security argument overstates it by 2^64 and by
  PRNG strength, and other files repeat it.
- Failure scenario: Today nothing authenticates on id unguessability, so impact is latent.
  If any future feature starts treating record keys as opaque unguessable tokens (share
  links, presigned-style record URLs, lottery-on-key), this comment will have waved the
  design through under a "128-bit random" justification the implementation does not provide.
- Suggested fix: Correct the comments in place: "monotonic-ts-prefixed id with a 64-bit
  Xoshiro256++ tail; unique-by-construction for insert, **not a secret / not
  CSPRNG-backed**". One-line edits, keeps the perf decision intact while deleting the
  misleading premise.

### 3.3 — low — User-influenced keys enter non-keyed FxHash maps despite the documented "no untrusted hash inputs" premise
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:154` (field) and :298-299
  (construction): `dirty: DashMap<RecordKey, Slot, THasher>`; `storage_in_memory.rs:18,24`:
  `stores: TDashMap<String, _>` keyed by store name.
- Issue: CLAUDE.md pillar 4 trades away RandomState DoS protection because "we don't accept
  untrusted hash inputs here". Two structures in this crate break that premise in spirit:
  `dirty` is keyed by *caller-supplied* `RecordKey`s, and secondary-index posting keys embed
  indexed field values verbatim (per `storage_mirrored.rs:165-172`'s own key-shape
  description), i.e. attacker-chosen bytes on an ingestion path; `InMemoryRepo.stores` is
  keyed by table/store names that ultimately originate in DDL. FxHash is a multiply-xor
  construction whose collisions are trivially mass-manufactured, unlike SipHash.
- Failure scenario: A writer flooding crafted colliding posting keys while the write-back
  buffer sits undrained (default 500 ms tick) concentrates those entries into one dashmap
  shard; subsequent `set`/`get` probes and `snapshot_overlay_sorted`'s clone+sort of the
  overlay (`storage_membuffer.rs:576-595`) skew superlinearly on that shard — a bounded but
  measurable remote write-path latency amplifier. `InMemoryStore.data`'s `scc::TreeIndex`
  (ordered B+-tree) is unaffected; only the two hash maps above are exposed.
- Suggested fix: Either (a) document why the premise still holds (prove posting values are
  canonicalized/length-capped upstream such that collision farming is pointless), or (b)
  give just these two externally-influenced maps a keyed BuildHasher (SipHash/RandomState) —
  their access patterns are buffered/write-side, not the ultra-hot lock-free reads pillar 4
  optimizes for.

### 3.4 — low — Raw key bytes — including attacker-influenced indexed values — embedded in error messages
- File:line: `crates/shamir-storage/src/storage_fjall.rs:419`;
  `storage_in_memory.rs:111,140`; `storage_membuffer.rs:797,810`.
- Issue: `DbError::NotFound(format!("record not found: {:?}", key))` (and `KeyExists`)
  interpolate the full key via `Debug`. Posting keys carry indexed field values of *other*
  columns' data; these error strings flow up through engine/wire layers and log aggregation.
  Rust's `escape_debug` escapes `\n`/`\r`/controls (so single-line logs hold and
  forgery-via-newline is blocked), but it leaves printable Unicode intact — including BiDi
  overrides (U+202E etc.) and zero-width characters — so log/terminal spoofing and
  cross-record value leakage via error text are both possible.
- Failure scenario: A query touching a missing posting key surfaces fragments of some
  record's indexed value inside an error string shown to a different tenant/console; or
  renders convincingly-reversed log lines via injected BiDi characters inside a crafted
  indexed value.
- Suggested fix: For these specific call sites, render keys as bounded hex
  (`hex(&key[..min(key.len(), 16)])` style helper) instead of `{:?}` — one small local
  formatter, no API change.

### 3.5 — nit — `KeyBytes::Deserialize` allocates an unbounded blob before any size check
- File:line: `crates/shamir-storage/src/key_bytes.rs:308-313`.
- Issue: Deserialization goes through `serde_bytes::ByteBuf::deserialize`, materializing the
  entire input allocation before `from_slice` runs; there is no maximum-length guard. Today
  safe (callers are WAL/bincode/rmp-serde boundaries that own frame limits, and the type is
  unused by production per module docs — see §7.2 for why that doc claim itself is stale),
  but plan doc section 5.3 anticipates flipping `RecordKey` to `KeyBytes` across the WAL/
  client-wire paths — at that point a hostile frame chooses the pre-allocation size subject
  only to upstream framing.
- Suggested fix: When the alias flip lands, gate the constructor: deserialize, then reject
  `len > MAX_RECORD_KEY_BYTES` (tie to schema/tunable constants) returning a
  `de::Error::invalid_length`.

---

## 4. performance-hotpath

Theme verdict (kept for calibration parity): the disk-tier backends (fjall, membuffer,
cached) show clear evidence of the 2026-07-06 audit fixes — incremental cursor scans,
zero-copy reads, bounded write-worker channel — but the same fixes were never carried into
`InMemoryStore`, and test coverage (~93 tests) asserts nothing about laziness or memory
bounds on exactly the paths below, which is how these survived.

### 4.1 — high — Reverse range streams drain the ENTIRE range into RAM before reversing; not overridden by InMemory/Cached/Mirrored
- File:line: `crates/shamir-storage/src/types.rs:376-384` (trait default) + :391-412
  (`default_reverse`); missing overrides at `storage_in_memory.rs:103-258` (implements
  `iter_range_stream` at :171 but no reverse) and `storage_cached.rs:414-717` (no
  `iter_range_stream*` overrides at all); inherited through `storage_mirrored.rs:431-439`.
- Issue: The default `iter_range_stream_reverse` composes the forward range stream with
  `default_reverse`, which `extend`s every batch into one `Vec` before yielding ("Memory ~ N
  items" per its own doc). Consumers of reverse order are precisely the early-exit workloads
  — `lookup_last_k`, `lookup_max`, `ORDER BY ... DESC LIMIT K` (named in the method's own
  doc) — yet each pays a full-range drain plus an O(range) resident allocation even for K=1.
  Because `scc::TreeIndex::Range` implements `DoubleEndedIterator` (`next_back`, verified
  against scc 3.8 docs), InMemoryStore can drive it natively, and
  `storage_fjall.rs:430-483` already proves the incremental reverse-cursor pattern this
  crate prefers. (Fix-order note: §1.3 — the InMemory forward resume body violates the
  `Bound::Excluded` contract — should be fixed first so the pattern being mirrored is
  sound.)
- Failure scenario: A hybrid table (`MirroredStore`, primary = InMemoryStore) or cached
  table with millions of sorted-index postings receives `lookup_last_k(k=10)`/DESC page
  requests; each request clones and holds all matching entries before returning one batch.
  Memory spikes scale with total range size, latency is linear in N for constant-K reads,
  and concurrent such reads multiply the transient allocations.
- Suggested fix: Override `iter_range_stream_reverse` in `InMemoryStore` with a per-batch
  guarded `.range(..).rev()` walk that seeks to the last key in-bound and resumes downward
  past it (`Bound::Excluded(last)`); have `CachedStore` do the same over its TreeIndex cache
  (its `scan_prefix_stream`:587 already shows the repeated-bounded-requery shape). Keep the
  trait default as documented fallback only.

### 4.2 — high — InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole corpus before the first yield
- File:line: `crates/shamir-storage/src/storage_in_memory.rs:153-159` (`iter_stream`),
  :240-247 (`scan_prefix_stream`). *(primary: also flagged by concurrency-lockfree §2.6 —
  rated low there, adding that the pinned `scc::Guard` across the whole collect delays EBR
  reclamation of concurrently-removed nodes)*
- Issue: Both methods `collect()` ALL matching `(key, value)` pairs into a `Vec` while
  holding an epoch guard, then hand the vec to the stream, which just drains it in
  batch_size chunks. This is exactly the eager-collect anti-pattern audit
  `2026-07-06-perf-radical-o-notation` §1.3 removed from `CachedStore`
  (`storage_cached.rs:521-526` documents the fix) and from fjall/membuffer — but it was
  never applied to the in-memory backend. A consumer wanting only the first batch
  (LIMIT-style pulls, `copy_store`'s early error paths) still pays O(N)/O(matches) clones +
  a single large allocation up front; `TreeIndex::iter` under a fresh guard each round-trip
  would keep memory O(batch_size). Note the SAME FILE's `iter_range_stream` (:171-232)
  implements the correct short-lived-guard + resume-key incremental pattern — the
  inconsistency is within one impl block (and §1.3 shows even that body needs its resume
  discipline corrected).
- Failure scenario: `MirroredStore::new` hydration streams via `mirror.iter_stream` (fine),
  but any later `scan_prefix_stream` over the hybrid primary (e.g.
  `SortedIndexManager::rekey_postings` re-scans, index lookup warming) allocates the full
  match-set twice transiently (collect vec + per-batch drained vecs) regardless of consumer
  appetite; under concurrent scans these snapshots compound, and large scans pin EBR garbage.
- Suggested fix: Convert both methods to the :184-231 pattern: open a guard, collect up to
  `batch_size` items starting after a resume key (for prefix: lower bound = max(resume,
  prefix), stop when a key exits the prefix), drop guard, yield, repeat. Total work
  unchanged; peak memory drops to O(batch_size) and early-exit consumers pay only what they
  drain. Align the README claim or scope it to Fjall/Cached.

### 4.3 — medium — MemBufferStore::transact drains the ENTIRE dirty buffer before every transact
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:1037`
  (`self.drain_all().await?` inside `transact`; `drain_all` itself at :598-618 also
  snapshots with `batch_size = usize::MAX`, :600/:612).
- Issue: Only op-touched keys need flushing before delegating to `inner.transact` (a pending
  dirty value v1 for key k MUST land before the batch writes v2 directly to inner, else the
  next drain revives stale v1). Instead the code flushes every dirty entry in the buffer —
  unrelated point-writes included — to disk synchronously inside each transact. That is
  write amplification proportional to total unflushed traffic, not to |ops|: the same
  read/write-triggered-drain class audit §2.3 (task #530) removed for scans ("a full flush
  is no longer required just to read" — yet a transact forces one). Additionally `drain_all`
  calls `drain_once(usize::MAX)`, snapshotting the whole dirty DashMap (keys + cloned values)
  into RAM in one shot rather than in `flush_batch_size` chunks.
- Failure scenario: A table behind `CachedStore -> MemBufferStore -> fjall` mixes steady
  single-row `set`s (buffered, flushed on the 500 ms tick) with a moderate tx rate calling
  `transact`. Every transact flushes whatever happens to be pending — potentially hundreds
  of MiB of accumulated buffered rows (64 MiB default `max_bytes` worth of cache alongside)
  — turning one small batch commit into a full write-back of unrelated data, defeating the
  fsync batching the buffer exists to provide, and stalling the commit for the drain
  duration.
- Suggested fix: Pre-drain ONLY the keys appearing in `ops` (build the key set, snapshot
  those dirty entries via `dirty.get`, apply their values with `set_many`/`remove_many`
  targeting just those keys, then CAS-clean via the same `remove_if(slot == snapshot)`
  discipline used at :1060-1081). Chunk the flush-path snapshots in `drain_once(batch)`
  units so even legitimate full drains don't spike resident memory to O(all dirty).

### 4.4 — medium — CachedStore WriteMode::Async uses an UNBOUNDED write-behind channel
- File:line: `crates/shamir-storage/src/storage_cached.rs:242`
  (`mpsc::unbounded_channel::<CacheWriteJob>()` — verified); jobs carry owned values:
  `CacheWriteJob::Set { key, value }` at :55, enqueued at :450/:503.
- Issue: Async-mode `set`/`remove` enqueue onto a single worker through an unbounded channel
  with no high-watermark, cap, or admission signal. `pending_writes` counts the backlog but
  nothing acts on growth; each queued `Set` holds its full `Bytes` value in addition to the
  copy already upserted into the cache at :437. One serialized worker draining at `inner`'s
  write rate is the only relief. Contrast `storage_fjall.rs:85-93`, which deliberately chose
  `sync_channel(1024)` for this identical pattern with the explicit rationale "a pathological
  fan-out can't OOM the queue" — the lesson was applied at the disk worker but not at the
  cache wrapper sitting above it.
- Failure scenario: Data-tier store in Async mode against a backing store that slows (cold
  cache, compaction stall, network volume): sustained producer rate > single-worker drain
  rate grows the queue without limit; memory rises by two copies per pending op until OOM.
  No telemetry surfaces depth except polling `pending_writes`.
- Suggested fix: Bound the queue (e.g. `async_channel`-style bounded or
  `tokio::sync::mpsc` with capacity ~ the fjall worker's 1024) and make submitters await
  send (async-aware backpressure), or keep std channel but route the rare-full case through
  `try_send` + async wait. Optionally expose a high-watermark log/metric off the existing
  `pending_writes`.

### 4.5 — medium — Trait-default range filter scans PAST the upper bound forever
- File:line: `crates/shamir-storage/src/types.rs:419-447` (`default_range_filter` loop keeps
  consuming batches and filtering after keys exceed `end`).
- Issue: The input stream is contractually ascending (`Store::iter_stream` ordering
  guarantee, types.rs:293-302), so once a key exceeds `end_inclusive` every subsequent key
  does too — but the filter keeps draining the stream to the end, discarding everything. Any
  backend relying on this default pays O(pos(end)..N) wasted traversal instead of stopping at
  the boundary. Concrete victim in-crate: `CachedStore` has no `iter_range_stream` override,
  so upper-bounded range/order queries on the cached tier run the default filter over its
  incremental full-store cursor; `InMemoryStore` is unaffected (native override). No
  correctness issue; pure O(x→0) miss.
- Failure scenario: An upper-bounded `iter_range_stream(Some(start), Some(end))` on a
  CachedStore covering the top slice of a large store walks and clones-checks every key
  beyond `end` — cost proportional to what lies ABOVE the requested window's end, growing
  with corpus size for a fixed-size query.
- Suggested fix: Track a `done: bool`; inside the filter closure return-based exit isn't
  enough across element boundaries — set `done` when the first out-of-window key is seen
  (`k > end`), then break the outer batch loop instead of pulling further batches.

### 4.6 — low — *(primary: 2.4)* — FjallStore::submit blocks the async caller thread when the bounded queue fills
- Same root defect as §2.4 (the perf lens rated it low, noting bulk parallel loaders can
  plausibly cross 1024 outstanding `insert`s since every submitter parks until its own job
  completes).

### 4.7 — nit — Minor allocations/clones on batched paths
- Files: `crates/shamir-storage/src/storage_membuffer.rs:1245` (`miss_keys.clone()` — extra
  key-vector clone per `get_many` containing misses; keys are `RecordKey`, mostly inline, so
  cheap but avoidable by zipping `miss_idxs` against kept-key references);
  `storage_fjall.rs:617` and `:682` (`Vec::with_capacity(256)` hardcoded instead of
  `min(batch_size, 256)` — over-allocates for small batches, reallocates once for larger).
- Issue/fix: Cosmetic constant-factor cleanups on batch paths; no behavioral risk either
  way. Listed for completeness, not as debt demanding action.

Theme-relevant non-findings preserved from the perf lens (for the record): `FjallStore::
set`/`remove` double LSM lookups are a bench-adjudicated trade-off with sanctioned
flag-free fast paths (`storage_fjall.rs:394-403`/:585-593) — not a finding; MemBuffer's
dirty-map value duplication (:143-154) is documented, bounded-ish by the flusher, with its
flush-failure residual acknowledged in-code (:187-192) — §4.3 addresses the amplification
side, and the writer-side high-watermark question is adjacent, deliberately not raised.

---

## 5. api-wire-protocol

Surface verdict (kept for calibration parity): the `Store`/`Repo` trait documentation is
unusually strong — ordering guarantees stated as correctness contracts, honest capability
disclosure via `supports_atomic_transact` (F-77/F-85), and an exemplary `KeyBytes`
byte-identity suite against bincode and rmp-serde. The gaps are serialization/versioning and
per-backend contract fidelity.

### 5.1 — high — Persisted `MemBufferConfig` wire format has no versioning guardrails despite a "stable wire-format" claim
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:92-126`.
- Issue: The doc comment says "Stable wire-format (serialized into `info_store` by the DDL
  layer)", and the struct is a plain `#[derive(Serialize, Deserialize)]` over 5 fields
  (`max_bytes`, `max_entries`, `ttl_ms`, `flush_interval_ms`, `flush_batch_size`) — no
  `#[serde(default)]`, no version/tag field, no golden-bytes test pinning today's encoding.
  Per `Cargo.toml`, this blob lands in the engine's info_store via bincode at the DDL
  boundary (i.e., on disk inside `__info__<t>`, mirrored through to fjall).
- Failure scenario: Any future change — adding a field (bincode reads old blobs as short →
  error or misaligned garbage), reordering fields, widening `flush_interval_ms`/`ttl_ms`
  types — breaks deserialization of every previously-written database's buffer config at
  open/DDL-reload time. There is no migration hook to catch it, so an existing deployment
  either fails to open or silently loses its buffer config.
- Suggested fix: Add `#[serde(default = "...")]` per field (cheap insurance even under
  bincode's self-describing-hostile format) plus an explicit envelope/version field before
  any further schema churn; add a round-trip golden test that serializes today's default
  config bytes and asserts byte-equality forever, so an accidental format change fails CI
  instead of production opens.

### 5.2 — medium — `batch_size == 0` is unspecified: InMemoryStore yields empty batches forever; fjall/cached silently return zero results
- File:line: trait contract `types.rs:310` (`iter_stream`), :336 (`scan_prefix_stream`) — no
  stated precondition; broken divergent impls at `storage_in_memory.rs:161-169` and
  :249-257; silent-empty behavior in `storage_fjall.rs:596-646`/`:656-723` and
  `storage_cached.rs:540-584`/`:600-645`.
- Issue: Nothing documents that `batch_size` must be > 0. `InMemoryStore`'s stream loops do
  `take = min(batch_size, entries.len()); drain(..take); yield` — with `batch_size == 0` and
  a non-empty corpus this yields `Ok(vec![])` infinitely and never terminates. Fjall and
  CachedStore use `.take(batch_size)` (= 0 items → empty batch → break), so they end
  immediately having yielded nothing at all even though data exists. Only
  `merge_overlay_stream` defends itself (`storage_membuffer.rs:661`, `batch_size.max(1)`),
  which shows the hazard is known but handled inconsistently.
- Failure scenario: A caller derives batch size from a tunable/config that computes to 0:
  full-table scans on disk backends report "no rows" (silent wrong answer feeding index
  scans/posting lists), while the identical call on the in-memory backend never completes (a
  hang — classified as a bug by repo policy, but by the *caller*, who has no way to know 0
  is invalid).
- Suggested fix: Either clamp defensively (`batch_size.max(1)` everywhere, matching
  `merge_overlay_stream`) or add `debug_assert!(batch_size > 0)` plus one sentence in the
  trait docs stating the precondition and the exhaustion semantics ("a final batch may be
  shorter; empty batches are never yielded").

### 5.3 — medium — `set`/`remove` created/existed flag precision varies by backend and write mode, with no capability disclosure
- File:line: contract `types.rs:36-39` (`set`: "Returns true if created"), :66-67 (`remove`:
  "true if existed"); divergences in `storage_membuffer.rs:763-788` (best-effort,
  cache/dirty-only, "false (presumed new)" after eviction — acknowledged inline at :764-768
  but not in trait docs), `storage_cached.rs:435-464` and :493-513 (Async-mode flags derived
  from cache state only). Related concrete bug: §1.4 (remove's flag is *wrong* for inner-only
  keys — a distinct defect from this disclosure gap).
- Issue: The trait documents one semantic for `bool`; implementations actually deliver three
  tiers: strict-but-TOCTOU (Fjall, documented inline at `storage_fjall.rs:358-364`),
  best-effort-local (MemBuffer — deliberately consults only dirty+cache, never inner, so an
  evicted-key update reports `created = true`), and mode-dependent (CachedStore Sync vs
  Async). This crate already established the correct pattern for exactly this problem —
  `supports_atomic_transact` (`types.rs:285-287`) was introduced because another
  undocumented capability gap caused MirroredStore to violate an overpromised contract
  (F-77) — but flag precision was left as free-text commentary scattered in impl comments.
- Failure scenario: A caller using `set`'s return to decide insert-vs-update bookkeeping
  (e.g., bumping a counter once per genuinely-new key) gets wrong counts when running over
  MemBuffer-wrapped backends whenever moka evicted the key, with no compile-time or runtime
  signal that the guarantee differs from the Fjall build it was developed against.
- Suggested fix: Document the tier each backend delivers in the trait method doc (or mirror
  the Fjall precedent: note where the strictness boundary is); longer-term, extend the
  F-77-style pattern with e.g. `fn strict_exists_flags(&self) -> bool` if callers ever need
  to gate on it.

### 5.4 — medium — Cross-crate wire-format literal duplicated privately: `[0,0,0,0]` system-record prefix re-encoded in storage_mirrored
- File:line: `crates/shamir-storage/src/storage_mirrored.rs:41-48`
  (`SYSTEM_RECORD_PREFIX: [u8; 4] = [0,0,0,0]`, kept local because the canonical constant is
  private) vs `crates/shamir-types/src/types/record_id.rs:18` (private
  `const SYSTEM_RECORD_PREFIX: &[u8] = &[0, 0, 0, 0];`); consumed by the durability
  classifier at `storage_mirrored.rs:173-198`.
- Issue: A 4-byte wire constant that decides which keys survive a hybrid-table restart
  exists in two crates with no shared definition. The local copy is honestly annotated, and
  the exhaustiveness test (`storage_mirrored_tests.rs:244`, building keys via the real
  `RecordId::system`) would catch an encoding change indirectly — but the guard is a
  behavioral proxy, not the type-level guarantee the module's own care level implies.
- Failure scenario: The record-key migration plan this crate is mid-flight on
  (`docs/dev-artifacts/design/record-key-128-migration-plan.md`) touches exactly these
  encodings; if `RecordId::system`'s prefix/padding changes without the allowlist match set
  following, classifier hits go to zero and every durable-config key silently becomes
  ephemeral — table/index/buffer config stops surviving restart, logged only as hydration
  drift warnings (if any entries existed to warn about).
- Suggested fix: Export `pub const SYSTEM_RECORD_PREFIX: [u8; 4]` (or a
  `RecordId::system_prefix()` accessor) from shamir-types and use it here; keep the local
  copy only if the borrow direction (types must not depend on storage) forbids it — in which
  case add a cross-crate test asserting the two literals stay equal.

### 5.5 — low — Public-API rustdoc drift: prefetch promise and phantom engines
- File:line: `types.rs:289-292` ("Uses concurrent prefetching: while yielding current batch,
  fetches next batch in background" — no implementation prefetches; every backend runs
  sequential cursor-resumed batches, and the crate README itself describes the design as
  lazy fetch-on-demand). Phantom backend references presented as live implementors/callers:
  `types.rs:96` ("sled, fjall, cached MUST override"), :134-141, :185-188, :343-347, :374
  (sled/redb/persy/nebari/canopy), `storage_in_memory.rs:80`, `storage_membuffer.rs:119-121`,
  `src/tests/types_tests.rs:149-150` — the crate README (`src/README.md:152-159`) explicitly
  documents these engines as removed/nonexistent. (The finding's other halves — the stale
  `key_bytes.rs` header and the drifted line-ref at `storage_fjall.rs:655` — are the same
  defects as §7.2 and §7.7 and are counted once there.)
- Issue: For a new implementor of `Store` (the primary audience of trait rustdoc), the
  contract text describes behavior that does not exist (concurrent prefetch) and an engine
  ecosystem that does not exist.
- Failure scenario: Documentation-only: someone implementing a new backend budgets effort
  for background-prefetch machinery or compares against phantom engines.
- Suggested fix: One surgical rustdoc pass: delete the prefetch sentence (or reword to
  "batches are fetched lazily, one cursor-resumed range per batch"); replace phantom-engine
  name-drops with "buffering backends".

### 5.6 — low — *(primary: 1.5)* — Shared backend-conformance suite skipped by CachedStore and MirroredStore
- Folded into §1.5 (the api lens's framing — "two of five `Store` implementations never run
  the crate's own agnostic conformance checklist; a delegation-path regression breaks the
  documented contract on those backends only" — is preserved there).

### 5.7 — low — `Repo::store_get` create-on-read semantics make typos durably materialize
- File:line: contract `types.rs:465-468` ("Retrieves a store by name. Creates it if it
  doesn't exist"); disk-side effect in `storage_fjall.rs:229-245` (fjall keyspace created
  via `KeyspaceCreateOptions::default` on every miss). Related, distinct: §3.1 name
  validation.
- Issue: There is no open-without-create probe on the `Repo` trait, so a read-path caller
  can never validate existence without mutating durable layout.
- Failure scenario: A misspelled table name anywhere on a read path materializes a real,
  empty fjall keyspace in the repository directory — visible in `stores_list()`, occupying
  metadata/journal space until manually cleaned, and indistinguishable from an intentional
  empty table downstream.
- Suggested fix: Add a default-implemented non-mutating check (e.g. `store_exists(name) ->
  bool`, cheaply answerable by all current backends) and route pure-validation callers
  through it; optionally log when `store_get` creates a new keyspace so accidental creation
  surfaces in logs.

### 5.8 — nit — Interface polish bundle
- Remaining items (the RecordStream and Tests-banner bullets are deduped into §7.4/§7.3):
  - Mixed key vocabulary on scan APIs: keys are `RecordKey`/`KeyBytes` but prefix/range
    bounds take raw `Bytes` (`types.rs:336, 354-358`), forcing boundary conversions in every
    backend (`RecordKey::from(prefix.clone())`). Consider accepting `impl AsRef<[u8]>`
    uniformly.
  - `Repo::copy_store` takes `&str` (`types.rs:488`) while sibling methods take generic
    `AsRef<str> + Send` — signature inconsistency in the same trait.
  - Double-prefixed message: `DbError::KeyExists(format!("Key already exists: {:?}", key))`
    renders as "Key already exists: Key already exists: ..." since the variant Display adds
    the same prefix (`storage_in_memory.rs:111` vs `error.rs:13-14`).
  - `error.rs` accumulates engine-domain variants (`Function`, `ValidatorRejected`,
    `ValidatorInvalid`, `IndexDrainInProgress`) in the lowest-layer crate; deliberate per
    its doc ("generic error"), but it couples every backend consumer to the engine's error
    taxonomy. Its `code()` wire mapping covers only 3 variants — fine, just ensure consumers
    know the mapping lives here.

Test-coverage notes preserved from the api lens: layout conforms to CLAUDE.md (per-module
`tests/` dirs with manifest-only `mod.rs`; test-only seams `#[cfg(test)]`-gated);
`key_bytes/tests/serde_byte_identity_tests.rs` is exemplary for a wire-format suite
(bincode + rmp-serde byte-identity against a local mirror of the WAL encoder, spanning
INLINE_CAP boundaries and cross-decode in both directions); the gap is only §5.6/§1.5.

---

## 6. error-handling-lifecycle

Discipline verdict (kept for calibration parity): error plumbing is broadly strong and
battle-tested — every fallible op returns `DbResult`, backend errors map into `DbError`
variants, production panics are confined to commented invariant violations (all verified as
genuinely unreachable: `storage_in_memory.rs:130,227`; `storage_fjall.rs:98,112,123,126,145`;
`storage_membuffer.rs:688,704`; `storage_cached.rs:160,409,422,449,502`), and
`drain_once` retains dirty entries on error, the §2.3 `remove_if` guard is regression-covered,
`MirroredStore` mirror-first ordering delivers honest error atomicity (thoroughly tested with
injected failures), and `CachedStore::flush` surfaces background failures exactly once
(#1082). The weaknesses are concentrated in resource lifecycle and untested error branches.

### 6.1 — high — `CachedStore::flush()` can hang forever if the async write-worker task dies before draining
- File:line: `crates/shamir-storage/src/storage_cached.rs:68-113` (worker loop), :243-249
  (`tokio::spawn`, handle discarded), :383-399 (`wait_for_async_writes`).
- Issue: In `WriteMode::Async`, `CachedStore` increments `pending_writes` per enqueued job
  and relies exclusively on the worker task to (a) `fetch_sub` after each job completes and
  (b) call `notify.notify_waiters()`. The task is launched with a discarded `JoinHandle`; if
  any `inner.set/remove` (an `Arc<dyn Store>` this crate does not control — it can be any
  wrapper, a foreign impl in tests/tooling, or scc/moka hitting an allocation panic) panics
  inside the worker task, or the runtime drops the task at shutdown mid-queue, the decrement
  + notify for every queued job never happens. `wait_for_async_writes` then loops:
  `pending_writes != 0` forever, and since `notify_waiters()` will never fire again, every
  subsequent `flush()` parks indefinitely. A durability-path deadlock of unbounded length;
  under house rules ("hangs are bugs") this is a defect even though the trigger requires an
  inner panic/cancellation. (The `Notify` before-check pattern itself follows tokio's
  documented race-free shape — given the worker stays alive.)
- Failure scenario: one failing/panicking inner write during a bulk load → all later
  `flush()` calls (the graceful-shutdown flush included) stall permanently instead of
  returning `Err`.
- Suggested fix: make the decrement+notify panic-safe — wrap each job iteration so
  decrement/notification run on unwind (a Drop guard over `pending_writes`), and/or await
  the worker's `JoinHandle` alongside (abort-on-death → surface
  `Err(DbError::Internal("async write worker died"))` from `flush()`), optionally add a
  bounded recheck with a plain atomic loop as a backstop.

### 6.2 — medium — *(primary: 2.4)* — Blocking `SyncSender::send` executed directly on tokio executor threads
- Folded into §2.4 (the error lens's framing — pillar-2 violation with SLOW/TIMEOUT-class
  symptoms under commit storms, and the suggested `spawn_blocking`/tokio-mpsc routes — is
  preserved there).

### 6.3 — medium — `MemBufferStore::Drop` silently discards a non-empty dirty buffer — zero observability
- File:line: `crates/shamir-storage/src/storage_membuffer.rs:621-626` (`Drop`), dirty-buffer
  contract :49-52 (module doc).
- Issue: Dropping the store sets `shutdown` and wakes the flusher, which exits *before*
  draining; whatever is still in `dirty` (values not yet applied to `inner`) is dropped
  without any log, count check, or accessor. The crate itself established in audit §2.2
  (:348-360) that buffered writes dying silently is unacceptable ("dirty grows unboundedly
  with zero signal") and added a counter + log for the flusher case — but the drop path loses
  the same data class with *less* signal than the bug §2.2 fixed. A `Drop` cannot `.await`,
  but observing the loss costs nothing.
- Failure scenario: a consumer recreates/replaces a MemBuffer-wrapped store outside
  `apply_config`'s drain-first path (the only documented safe path); all ACKed-but-unflushed
  writes vanish while `inner` keeps stale values — undiagnosable afterwards.
- Suggested fix: in `Drop`, when `dirty_count > 0`, emit a `log::warn!` naming the store and
  entry count (and/or expose `dirty_count()` for callers/tests to assert orderly shutdown);
  document explicitly that drop-with-dirty = data loss by contract.

### 6.4 — medium — Missing error-path tests; audit-§2.2 telemetry is written but never read
- File:line: `storage_membuffer.rs:192,355` (`flush_errors` — no reader anywhere, not even a
  `#[cfg(test)]` accessor); `storage_cached.rs:446-462,499-510` (worker-channel-closed
  fallbacks); `storage_fjall.rs:199-207` (both `DbError::Internal` mappings in `submit`);
  `types.rs:488-503` (`Repo::copy_store` default partial-failure).
- Issue: No test constructs any of these states:
  - MemBufferStore background-drain failure: the §2.2 behavior (counter increment, error
    log, dirty retained + retried next tick) is completely uncovered, and `flush_errors` has
    no accessor, so the counter cannot ever be observed — dead telemetry, unverifiable
    claim.
  - CachedStore `set`/`remove` send-failure branch ("worker gone, write dropped":
    pending-count undo + loud log).
  - FjallStore `submit` error shapes (`Internal("write worker channel closed"/"dropped
    reply")`) mapped to match the old `spawn_blocking` semantics.
  - `Repo::copy_store`: nothing tests what state remains when `src.iter_stream` or
    `dst.set_many` fails mid-copy (relevant to RENAME TABLE — see §6.6).
- Failure scenario: regressions in exactly these branches (e.g. removing the pending-count
  undo, changing the retry discipline, breaking retained-dirty-on-error) land silently green.
- Suggested fix: failing-inner wrappers (already idiomatic in this suite: `FailingStore`,
  `FailingTransactMirror`) cover the first three cheaply; add a `#[cfg(test)]
  dirty_error_count()` accessor (and a failing-backend test asserting `flush_errors` bumps
  and dirty survives a failed drain).

### 6.5 — low — Cache eviction/deletion committed before the fallible backing op is acknowledged
- File:line: `storage_cached.rs:487-515` (`remove` evicts cache before `inner.remove`
  resolves — both modes), :427-467 (`set` Async branch populates cache before enqueue result
  known).
- Issue: On `Err` from the backing store the cache mutation is already durably applied
  locally. Sync mode self-heals (next `get()` read-through re-caches what `inner` still
  holds), but Async remove is worse: after the one-shot flush error (#1082 semantics), the
  key cache-misses into `inner`, which still holds the old value — the deleted key silently
  resurrects on later reads with no further signal, and reload/hydration makes it permanent.
- Failure scenario: backing store outage during Async-mode deletes → caller sees one `Err`
  from `flush()`, then reads resurrect every tombstoned key with no diagnostic.
- Suggested fix: hold a sticky negative marker (or re-tombstone on read-through hit of a
  failed-remove key) until the removal is confirmed, or at minimum log on the resurrection
  path; document the divergence window in the module doc.

### 6.6 — low — `Repo::copy_store` default impl leaves a partially-populated destination on failure
- File:line: `types.rs:488-503`. Related, distinct: §1.9 self-copy guard on the same method.
- Issue: Copy-then-orphan rename streams batches into `dst.set_many` with no compensating
  cleanup: a mid-stream error returns `Err` leaving a half-copied destination store that
  persists on disk and appears in `stores_list` forever. Retry convergence relies on
  overwrite-by-key idempotency, which breaks if source rows were removed between attempts
  (stale extras survive in dst). None of this is documented on the method.
- Failure scenario: RENAME TABLE fails partway → phantom `__data__<t>`-shaped store
  accumulates; a successful later copy over different src content merges stale keys.
- Suggested fix: either best-effort `store_delete(to)` on the error path (documented, orphan
  disposition matches DROP TABLE) or spell out the convergence/idempotency contract callers
  must honor.

### 6.7 — low — *(primary: 1.6)* — `FjallRepo::store_get` returns a fresh `FjallStore` per call — fragile per-instance worker lifecycle
- Folded into §1.6 facet (c) (spawn+join churn per transaction, handles outliving
  `store_delete`, the name-keyed `Arc` cache fix).

### 6.8 — nit — Error-source chains flattened; thread-spawn failure panics instead of `DbResult`
- File:line: `error.rs:92-96` (`From<CodecError>` → `err.to_string()`); most variants carry
  `String` rather than a typed source; `storage_fjall.rs:95-98`
  (`.expect("spawn fjall write worker thread")`).
- Issue: CLAUDE.md asks for `thiserror` with `#[from]` where natural; `std::io::Error` gets
  it, but `CodecError` (and fjall/DB errors generally) degrade to display strings, losing
  `source()` chains for diagnostics. Thread-spawn failure in `WriteWorker::spawn` panics the
  calling async context via `OnceLock::get_or_init` (can't propagate an error through it);
  defensible as near-fatal, but inconsistent with the crate's otherwise strict no-panic
  surface.
- Suggested fix: consider `#[source]`/typed variants where ergonomic (esp. `Codec`); make
  `WriteWorker::spawn` return `Option<WriteWorker>` handled as `DbError::Storage`/`Internal`
  if a hard dependency on graceful degradation matters.

---

## 7. style-claude-md

Structure verdict (kept for calibration parity): `src/tests/mod.rs` and
`src/key_bytes/tests/mod.rs` are re-export manifests only; no inline `#[cfg(test)] mod
tests` in implementation files (both test trees wired via the prescribed `#[cfg(test)] mod
tests;` pointers at `key_bytes.rs:315-316` and `lib.rs:32-33`; `membuffer_clear_race_hook`
keeps its logic in a cfg-gated sibling file); `lib.rs` is declarations + docs only;
one-file-one-primary-export holds (Repo+Store pairings are closely-coupled groups; private
helper enums serve exactly their owning store); `thiserror` used for `DbError`. The two
real clusters are function-local imports and stale residual comments.

### 7.1 — medium — Function-local imports violate the mandatory "Imports at the top" rule
- File:line: types.rs:395, :424, :489, storage_cached.rs:218, :307, storage_fjall.rs:451,
  :610, :673; also tests: storage_membuffer_tests.rs:426, storage_cached_tests.rs:321,
  storage_in_memory_tests.rs:264, storage_mirrored_tests.rs:1289,
  key_bytes/tests/hash_consistency_tests.rs:52. *(primary: also flagged by correctness-tdd
  §1.10's second half)*
- Issue: CLAUDE.md ("Imports at the top") requires all `use` statements in the file (or
  enclosing module) header, allowing only three documented exceptions (`use super::*`
  inside cfg(test) test modules; collision-justified single-method trait imports;
  cfg-gated/macro bodies). None of these apply here:
  - `use futures::StreamExt;` inside fn bodies/closures: types.rs (in `default_reverse`,
    `default_range_filter`, `Repo::copy_store`), storage_cached.rs (in `new_with_mode`,
    `reload`). No name is in collision scope, and sibling files already hoist exactly this
    import at top level (types_tests.rs:7, storage_cached_tests.rs:11,
    storage_mirrored.rs:36), so even the repo's own precedent contradicts the local
    placement.
  - `use std::ops::Bound;` inside the three `spawn_blocking` closures of storage_fjall.rs
    (`iter_range_stream_reverse`, `iter_stream`, `scan_prefix_stream`) — not cfg-gated,
    hoistable.
  - Test files: `use tokio::task::JoinSet;` mid-`#[tokio::test]` (x2),
    `use crate::storage_cached::CachedStore;` mid-test at storage_mirrored_tests.rs:1289
    (top-level imports there already include other storage modules, so no collision),
    `use futures::StreamExt;` inside the `collect_stream` helper,
    `use std::hash::DefaultHasher;` inside a test body where the sibling header import line
    (`std::hash::{BuildHasher, Hash, Hasher}`) could simply be extended.
- Failure scenario: none behavioral; it defeats the rule's purpose (single-glance dependency
  inventory per file, diff hygiene), and because enforcement is manual, each new stream/range
  method tends to copy the nearest local `use` rather than the header.
- Suggested fix: hoist all thirteen imports to their file headers (module headers for the
  nested test mods). Mechanical, zero-risk diff.

### 7.2 — medium — Stale module doc in key_bytes.rs claims the type is unused and that `RecordKey = Bytes`
- File:line: `crates/shamir-storage/src/key_bytes.rs:4-9` (module doc — verified during
  synthesis against `types.rs:9`). *(primary: also flagged by api-wire-protocol §5.5,
  rated low there as part of its rustdoc-drift bundle)*
- Issue: The doc says step 1 landed "with zero call-site changes anywhere else", that
  `types.rs`'s "`pub type RecordKey = Bytes;` alias is left untouched", and that KeyBytes is
  "currently unused by production code". Since then the alias flip happened: `types.rs:9`
  reads `pub type RecordKey = KeyBytes;`, making `KeyBytes` *the* production record key
  across every backend.
- Failure scenario: A maintainer reading only the module doc would believe `RecordKey` is
  still `bytes::Bytes` and that inline-vs-heap behavior is dormant/unexercised in production
  — e.g. reasoning wrongly about allocation costs, or "deferring" work that has actually
  shipped, or trusting serialization properties of `Bytes` that no longer apply on these
  paths (the serde byte-identity suite now guards every WAL/storage key).
- Suggested fix: update the doc's framing to describe state-after-step-2 (alias flipped;
  representation-invariance guarantees now load-bearing everywhere `RecordKey` flows),
  keeping the history references.

### 7.3 — low — Orphaned "// ===== Tests =====" banner comments left behind after tests moved out
- File:line: storage_in_memory.rs:260-262, storage_cached.rs:719-721,
  storage_fjall.rs:726-728. *(primary: also flagged by api-wire-protocol §5.8's bundle)*
- Issue: All three files end with the empty banner block marking where inline tests used to
  live. The tests now follow the documented layout in `src/tests/*.rs`; these residues imply
  inline tests should follow and invite re-adding them there (the exact anti-pattern
  CLAUDE.md's test-organisation section bans).
- Failure scenario: a contributor appends a new test under the banner, recreating an inline
  test block in an impl file.
- Suggested fix: delete the three dangling banners.

### 7.4 — low — Duplicate private `RecordStream` alias re-declared instead of importing the canonical one
- File:line: storage_membuffer.rs:628 (vs. canonical `pub(crate) use` target at
  types.rs:11-12); same duplication in tests/types_tests.rs:12-13. *(primary: also flagged
  by api-wire-protocol §5.8's bundle, which adds the API-surface argument)*
- Issue: `type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>,
  DbError>> + Send>>;` is re-declared locally although `crate::types::RecordStream` is
  `pub(crate)` and already imported cross-module by storage_mirrored.rs:33. The api lens
  adds the deeper point: `RecordStream` is `pub(crate)` yet is the return type of required
  **public** trait methods, so external implementors/consumers cannot name it at all — which
  is *why* the alias had to be duplicated twice. Two copies can drift independently (e.g. if
  the item type ever grows a third field or the error type narrows).
- Failure scenario: a future signature change to the canonical alias silently leaves
  MemBuffer's copy inconsistent, surfacing as a compile break or — worse if both happen to
  still unify — unnoticed divergence.
- Suggested fix: `use crate::types::RecordStream;` in storage_membuffer.rs (and in
  tests/types_tests.rs); delete the local aliases — and make the canonical alias `pub` so
  external `Store` implementors can name their return type.

### 7.5 — low — *(primary: 1.5)* — Shared batch/conformance suite never runs against CachedStore or MirroredStore
- Folded into §1.5 (the style lens's framing — "the two wrappers whose correctness depends
  on faithfully preserving/inheriting default batch semantics through delegation" — is
  preserved there).

### 7.6 — nit — storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files
- File:line: tests/storage_membuffer_tests.rs:728 (`mod audit_2_3`), :867
  (`mod clear_race_535`), :974 (`mod batch_insert_republish_535`).
- Issue: The test-organisation section prescribes splitting by topic into one file per
  related group within `tests/` (the pattern `key_bytes/tests/` follows properly). Here
  three self-contained fixture groups (~380 lines including mock `Store` impls) sit as
  inline submodules of one 1075-line file. Their imports are correctly placed at each
  submodule's header (the documented exception pattern), so this is purely about file
  granularity.
- Failure scenario: continued accretion; the next audit fixture gets a fourth inline mod
  instead of a file.
- Suggested fix: promote each `mod` to its own `audit_2_3_tests.rs` / `clear_race_tests.rs` /
  `batch_pause_tests.rs` under `src/tests/`, registered in `tests/mod.rs`.

### 7.7 — nit — Drifted line-number reference in a comment
- File:line: storage_fjall.rs:655. *(primary: also flagged by api-wire-protocol §5.5)*
- Issue: `scan_prefix_stream`'s doc says the resume pattern matches "iter_stream above (lines
  ~323)" — `iter_stream` now sits around line 596; hard-coded line numbers rot on every edit
  above them.
- Failure scenario: reader chases a stale pointer, wastes time, distrusts nearby comments.
- Suggested fix: drop the parenthetical line reference; name the method only.

---

## Finding counts

Raw lens-tagged findings (as filed across the 7 reports, matching the workspace SUMMARY.md
row: 0 crit / 7 high / 17 med / 19 low / 10 nit = 53):

| Lens | crit | high | med | low | nit | total |
|---|---|---|---|---|---|---|
| correctness-tdd | 0 | 1 | 3 | 3 | 3 | 10 |
| concurrency-lockfree | 0 | 2 | 2 | 3 | 1 | 8 |
| security-crypto | 0 | 0 | 1 | 3 | 1 | 5 |
| performance-hotpath | 0 | 2 | 3 | 1 | 1 | 7 |
| api-wire-protocol | 0 | 1 | 3 | 3 | 1 | 8 |
| error-handling-lifecycle | 0 | 1 | 3 | 3 | 1 | 8 |
| style-claude-md | 0 | 0 | 2 | 3 | 2 | 7 |
| **total (lens-tagged)** | **0** | **7** | **17** | **19** | **10** | **53** |

Deduplicated distinct-defect census (each root-cause defect counted once; deduped severity =
highest severity any lens assigned; nit bundles counted as 1 per the workspace counting
note; partial overlaps inside §1.8/§1.10/§5.5/§5.8 leave their non-shared halves as their
own defects):

| Severity | Lens-tagged findings | Distinct defects | Dedup groups / findings counted once |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 7 | 6 | 1.1 + 2.2 (get_many guard — one defect, two lenses); 2.1, 4.1, 4.2 (eager materialize — dedup of 2.6), 5.1, 6.1 |
| medium | 17 | 16 | 2.3 (dedup of 1.7) · 2.4 (dedup of 4.6 + 6.2 — one defect, three lenses) · 7.1 (dedup of 1.10's import half) · 7.2 (dedup of 5.5's key_bytes half); singles: 1.2, 1.3, 1.4, 3.1, 4.3, 4.4, 4.5, 5.2, 5.3, 5.4, 6.3, 6.4 |
| low | 19 | 13 | 1.5 (dedup of 5.6 + 7.5 — one gap, three lenses) · 1.6 (dedup of 2.8 + 6.7 — one structural fact, three lenses) · 2.7 (dedup of 1.8's reload half) · 7.3 (dedup of 5.8 bullet) · 7.4 (dedup of 5.8 bullet); singles: 2.5, 3.2, 3.3, 3.4, 5.5 (remainder), 5.7, 6.5, 6.6 |
| nit | 10 | 9 | 7.7 (dedup of 5.5's line-ref half); singles: 1.8 (remainder), 1.9, 1.10 (remainder), 3.5, 4.7, 5.8 (remainder), 6.8, 7.6 |
| **total** | **53** | **44** | 53 lens-tagged findings → 44 distinct defects |

---

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Port the #539 dirty-recheck guard into `get_many`'s fill loop** (re-probe
   `dirty_count`/`dirty.get(&k)` before each `cache.insert`, mirroring `get()` at
   `storage_membuffer.rs:840-880`) and add the hook-based `get_many` regression test.
   Closes **1.1 / 2.2** — the permanent read-your-write break under ordinary concurrent
   vectored reads.
2. **Give `CachedStore` one ordered cache-mutation helper** (single ordered worker for both
   modes, or collapse remove+insert into `upsert_sync` with fill/populate routed through the
   helper; derive `size` only inside it) + a same-key concurrent-writer test. Closes
   **2.1** — silent acked-write loss; the workspace's top-ranked storage risk.
3. **Make the Async write-worker panic-safe** (Drop guard over `pending_writes`, awaited
   `JoinHandle` surfacing `Err` from `flush()`, bounded recheck backstop). Closes **6.1** —
   durability-path flush hang.
4. **Version the persisted `MemBufferConfig`**: per-field `#[serde(default)]`, explicit
   version/envelope field, golden round-trip test pinning today's bytes. Closes **5.1**
   before any further schema churn bricks existing database opens.

**P1 — soon**

5. **Fix the scan-laziness class**: implement incremental reverse-cursor overrides for
   InMemoryStore/CachedStore (4.1); convert `iter_stream`/`scan_prefix_stream` to the
   per-batch guarded resume pattern (4.2, closing 2.6's guard-pinning facet with the same
   edit). Fix **1.3** (inclusive-resume → `Bound::Excluded`) first so the template body is
   sound.
6. **Stop `transact` from force-draining the whole dirty buffer** — pre-drain only op keys
   with the `remove_if(slot == snapshot)` CAS discipline; chunk `drain_once` snapshots.
   Closes **4.3**.
7. **Bound the CachedStore Async write-behind queue** with async-aware backpressure and a
   high-watermark signal. Closes **4.4**.
8. **Make `FjallStore::submit` async-safe** (semaphore-gated or `try_send`+`spawn_blocking`
   or tokio mpsc front half). Closes **2.4 / 4.6 / 6.2** (one defect, three lenses).
9. **Republish-guard family**: guard `MemBufferStore::transact`'s post-commit cache republish
   (1.2) via the same helper as item 1; replace `InMemoryStore::set`'s Err branch with
   `upsert_sync` and correct its comment (2.3, closing 1.7).
10. **Contract fidelity**: clamp/document `batch_size == 0` (5.2); fix or explicitly amend
    `remove`'s existed-flag contract for inner-only keys (1.4) and document the per-backend
    flag tiers (5.3) — same doc/test sweep.
11. **Lifecycle observability**: warn on drop-with-dirty + `dirty_count()` accessor (6.3);
    add the missing error-path tests and a readable `flush_errors` (6.4).
12. **Run the shared batch suite over CachedStore (both modes) and MirroredStore** (1.5,
    closing 5.6 / 7.5).

**P2 — backlog**

13. `THasher` for the moka cache (`build_with_hasher`). Closes **2.5**.
14. Validate store names at the `Repo` boundary (3.1); add `store_exists` non-mutating probe
    + creation logging (5.7).
15. Export `SYSTEM_RECORD_PREFIX` from shamir-types (or add the cross-crate literal-equality
    test). Closes **5.4**.
16. Security hygiene: correct the "random 128-bit id" comments (3.2); document or re-key the
    two untrusted-input FxHash maps (3.3); bounded-hex key rendering in error strings (3.4);
    `MAX_RECORD_KEY_BYTES` gate when the KeyBytes wire flip lands (3.5).
17. copy_store hardening: `from == to` guard (1.9) + best-effort destination cleanup or a
    documented convergence contract (6.6).
18. FjallStore worker model: scope the ordering invariant to a single handle, name the
    worker-vs-`spawn_blocking` interleave, share the worker per keyspace. Closes
    **1.6 / 2.8 / 6.7** (one structural fact, three lenses).
19. Sticky negative marker (or log) for Async-mode remove resurrection (6.5); document
    `reload()` as quiescence-required or snapshot-diff it (2.7, with 1.8's reload half);
    increment `size` only on successful insert (1.8 remainder).
20. Error taxonomy: `#[source]`/typed `Codec` variant; graceful `WriteWorker` spawn failure
    (6.8).
21. Docs sweep, one commit: hoist the thirteen function-local imports (7.1, with 1.10's
    import half); refresh `key_bytes.rs` module doc (7.2, with 5.5's half); delete the three
    Tests banners (7.3, with 5.8's bullet); import/share `RecordStream` and make it `pub`
    (7.4, with 5.8's bullet); drop the prefetch/phantom-engine rustdoc (5.5 remainder); fix
    the `:655` line ref (7.7); trim the retired-#535 narration (1.10 remainder); split the
    membuffer test mods into topic files (7.6); apply 5.8's remaining API-polish bullets.
22. Cosmetic batch-path allocations (`miss_keys.clone()`, hardcoded 256) (4.7).

</details>
