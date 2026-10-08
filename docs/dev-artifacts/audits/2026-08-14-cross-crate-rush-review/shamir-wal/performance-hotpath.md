<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-wal — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Worst-case linear probes and corpus materialization are source-supported. Capacity guesses, queue allocation and full fallback decoding are tuning opportunities, not independently established performance defects.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 2 | 0 | 0 | 1 | 1 | 6 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — has_truncatable on the Mem sink is an O(frames) scan run on every drainer tick

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Mem any scans all frames when none qualifies, under frames; the idle and completed drainer paths invoke the probe. File any similarly scans sealed metadata. Backlog-dependent work is proven; the claimed CPU burden and Medium operational impact were not.

Evidence: [crates/shamir-wal/src/wal_sink.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L205); [crates/shamir-engine/src/tx/drainer.rs:337](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L337); [crates/shamir-engine/src/tx/drainer.rs:953](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L953).

<a id="review-2"></a>

### Claim 2 — WalEntryV2::encode starts from a 256-byte capacity guess that realistic entries overflow

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

A 256-byte body plus fields exceeds the reservation, but Vec growth is intended and the fixture does not establish workload prevalence or that double-pass sizing is faster. No exact reallocation count or regression is proved.

Evidence: [crates/shamir-wal/src/wal_entry_v2.rs:215](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L215); [crates/shamir-wal/benches/wal_startup_open.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/benches/wal_startup_open.rs#L37).

<a id="review-3"></a>

### Claim 3 — Per-window allocation churn in lead_until_drained: pending Vec regrows from capacity 0 every window

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

mem::take drops reusable queue capacity and fresh payload/meta vectors are allocated. This is a real source-level cost, but no violated allocation contract, measured regression or bottleneck distinguishes it from an intentional simple ownership design.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L277); [crates/shamir-wal/src/wal_group_commit.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L280).

<a id="review-4"></a>

### Claim 4 — Startup sidecar fallback decodes every entry to extract one u64

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Fallback decodes complete entries to derive maxima, as documented. That also validates payloads before metadata acceptance; header-only decoding would change failure behavior. Its optimization benefit is unmeasured and actual recovery still needs the entries.

Evidence: [crates/shamir-wal/src/segment_set.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L155); [crates/shamir-wal/src/wal_segment.rs:572](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L572).

<a id="review-5"></a>

### Claim 5 — replay materializes the entire WAL as decoded entries in one Vec

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Decoded entries from every segment accumulate before return; Mem additionally retains encoded frames. A large retained corpus therefore produces backlog-dependent peak memory, including live gap recovery. No specific OOM threshold is proved.

Evidence: [crates/shamir-wal/src/segment_set.rs:433](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L433); [crates/shamir-wal/src/wal_sink.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L163); [crates/shamir-engine/src/tx/drainer.rs:368](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L368).

<a id="review-6"></a>

### Claim 6 — Full window bytes memcpy'd into the coalescing buffer per append batch

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The explicit copy constructs contiguous framed output for write_all. Moving Vec ownership through pending and payloads does not itself copy contents. This is a deliberate trade, not a defect.

Evidence: [crates/shamir-wal/src/wal_segment.rs:232](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L232); [crates/shamir-wal/src/wal_group_commit.rs:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L283).

<a id="review-7"></a>

### Claim 7 — One Arc&lt;Waiter&gt; heap allocation per single append

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Arc&lt;Waiter&gt; is required by this ownership design and is an acknowledged cost. No current regression or mandatory replacement is established.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L175).

<a id="review-theme-core-amortization"></a>

### Claim Theme/core-amortization — Exactly one write and at most one fsync per window

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

One coordinator append request is not one guaranteed OS write; rotation fsync, sidecar fsync, retry and background synchronization are additional work.

Evidence: [crates/shamir-wal/src/wal_segment.rs:236](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L236); [crates/shamir-wal/src/segment_set.rs:367](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L367); [crates/shamir-wal/src/segment_meta.rs:103](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_meta.rs#L103).

<a id="review-theme-sidecar-startup-saving"></a>

### Claim Theme/sidecar-startup-saving — Sidecar removed O(total WAL bytes) startup replay

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Valid sidecars avoid sealed decoding during maximum discovery; the corrupt-data test discriminates that branch. Active repair and RepoWalManager recovery still read WAL data, so the benefit is not elimination of total database startup replay.

Evidence: [crates/shamir-wal/src/segment_set.rs:145](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L145); [crates/shamir-wal/src/tests/segment_set_tests.rs:557](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/segment_set_tests.rs#L557); [crates/shamir-tx/src/repo_wal_manager.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_wal_manager.rs#L135).

<a id="review-theme-measured-performance"></a>

### Claim Theme/measured-performance — Historical 4.4× scaling, 63× fsync dominance, and lock-cost measurements

Status: `unverified`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Benchmark targets are registered, but no measurements were reproduced. Sustained append amortizes spawning while still timing encoding/framing, blocking dispatch and serialized file writes; it does not isolate inner-mutex acquisition.

Evidence: [crates/shamir-wal/Cargo.toml:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/Cargo.toml#L26); [crates/shamir-wal/benches/segment_set_lock.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/benches/segment_set_lock.rs#L131).

## Evidence and recipe corrections

- Do not count unmeasured allocation-sizing/recycling/fallback changes as confirmed defects merely because an alternative representation exists.
- An O(1) minimum mirror must cover arbitrary version order, pinned entries and every append/truncate mutation. Taking a retained head is not a minimum proof.
- A merge of per-segment streams requires each stream to be commit-version sorted; physical append order is expressly not that order.
- The append/truncate benchmark's 1 GiB cap and ignored deletion count do not prove rotation or reclamation was exercised.
- The startup benchmark includes runtime construction, uses relatively small synthetic segments, and does not measure complete RepoInstance recovery.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wal -- Performance & O(x->0)

## Summary

The crate's core amortization story is sound and unusually well documented: group commit coalesces a window of N committers into exactly one `write()` and at most one `fsync()`, segment rotation is driven by an atomic byte counter (no `metadata()` syscall per append), and the `#500` sidecar removed the O(total WAL bytes) startup replay. No quadratic path was found. The remaining findings are linear scans labelled "cheap" whose N grows with the un-truncated backlog (`WalSink::has_truncatable` on the `Mem` variant), per-window/per-commit allocation churn on the commit hot path (pending-Vec regrowth from capacity 0, a wrong 256-byte encode capacity guess), and cold-path buffering that materializes the whole WAL in RAM during replay/recovery.

## Findings

### 1. `has_truncatable` on the `Mem` sink is an O(frames) scan run on every drainer tick
- **File:** `crates/shamir-wal/src/wal_sink.rs:200-208` (Mem arm, `frames.iter().any(..)` at 205); sibling `SegmentSet::has_truncatable` at `crates/shamir-wal/src/segment_set.rs:557-562`
- **Severity:** medium
- **Issue:** The method is documented and consumed as a "cheap probe" — `shamir-engine/src/tx/drainer.rs:730-743` calls it in `settle_and_truncate`, which runs at the end of *every* `drain_step` pass **and** on the `dur >= vis` early-return path, i.e. on every background drainer tick even when nothing new was committed. For `WalSink::File` the scan is over the sealed-segment list (8 MiB segments per `WAL_SEGMENT_MAX_BYTES`, so short — genuinely cheap). For `WalSink::Mem` it is a linear scan over *every frame appended since the last truncation*, under the `frames` std Mutex. The "N" here is commits-since-truncation, and it grows precisely when the truncation gate (`pending_unsafe` / interner A5 hwm) lags — a wedge that also makes the scan-per-tick fire repeatedly over the largest possible list.
- **Failure scenario:** An in-memory repo under sustained write load whose interner delta gate stalls (a long-lived `pending_unsafe` entry): frames grow to millions; every drainer tick then pays an O(millions) scan of a ~32-byte-per-element Vec (plus lock hold), turning the background loop into a constant CPU burner proportional to backlog size.
- **Suggested fix:** Maintain an O(1) mirror per CLAUDE.md pillar 3 (the `scc::len()` rule): e.g. an `AtomicU64` tracking the minimum non-pinned frame `max_version` (updated on `append_batch` when the list is empty / on `truncate_below` by re-deriving from the retained head), so the probe is a single load. The `File` variant can keep its short scan or mirror `min(sealed.max_version)` the same way (updated at seal/truncate under `inner`).

### 2. `WalEntryV2::encode` starts from a 256-byte capacity guess that realistic entries overflow
- **File:** `crates/shamir-wal/src/wal_entry_v2.rs:211-221` (`Vec::with_capacity(256)` at 215)
- **Severity:** low
- **Issue:** `encode()` runs once per committed transaction on the hottest path in the system. The comment claims "one alloc is the common case", but a V2 entry carries full inline record bodies (`WalOpV2::Put { body }`) — any entry over ~256 bytes (the crate's own startup bench uses a 256-byte *body*, which already overflows the guess once headers are added) climbs the geometric-growth ladder: 256→512→1024→…, each step a realloc plus memcpy of everything written so far. Amortized it is O(bytes) not O(N²), but it is avoidable allocator churn on every commit, and the justifying comment is wrong for the common case.
- **Failure scenario:** A workload of 4 KB records: ~5 reallocs + copies per commit, ~per-window, for the whole life of the database. Individually cheap (fsync dominates ~63× per the crate's own measurement), but it is pure waste on the non-fsync (`Buffered`/mem-sink) tiers where the bench shows throughput is coordination-bound.
- **Suggested fix:** Size exactly with `bincode::serialized_size(self)` (a second serialization pass but zero allocations) → `Vec::with_capacity(5 + size as usize)`, or cheaply pre-estimate from `ops` (`sum of body/key/value lens + fixed per-op overhead`) if the double pass is a concern. Also fix the comment.

### 3. Per-window allocation churn in `lead_until_drained`: pending Vec regrows from capacity 0 every window
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:270-287` (`std::mem::take` at 277; `payloads`/`metas` fresh Vecs at 280-281); `pending: Mutex<Vec<Pending>>` initialized bare at 158
- **Severity:** low
- **Issue:** `mem::take` replaces `pending` with a zero-capacity `Vec::new()`, so every window the queue re-climbs the allocation ladder (4→8→16→… elements) — for a 64-committer window that is ~5 reallocs of the pending Vec per window — and the drained Vec's hard-won capacity is simply dropped instead of recycled. On top of that, each window allocates two more Vecs (`payloads`, `metas`) plus `Arc::new(payloads)` in `SegmentSet::append_batch` (segment_set.rs:242), and lines 298/308 make two extra O(window) `.any()` scans that could be folded into the existing destructure loop at 283-287. All amortized O(1) per entry and measured non-binding (the mem sink scales 4.4× with concurrency), but it is steady allocator traffic in the innermost commit loop.
- **Failure scenario:** No cliff — this is constant per-window overhead (~8+ allocations per window) that shows up only on the no-I/O mem-sink tier and in allocator pressure under high TPS.
- **Suggested fix:** Keep a spare Vec: after draining, `swap` the emptied-but-capacity-carrying Vec back into a `Mutex<Vec<Pending>>`-adjacent slot (or `std::mem::replace(&mut *p, spare.take().unwrap_or_default())` pattern) so capacity survives windows; seed `WalGroupCommit::new`'s Vec with a small capacity (e.g. 64). Fold the `has_buffered`/`needs_fsync` computations into the destructure loop.

### 4. Startup sidecar fallback decodes every entry (allocating all ops/Bytes/Strings) to extract one `u64`
- **File:** `crates/shamir-wal/src/segment_set.rs:145-158` (`replay_sealed_at_startup().await` then `entries.iter().map(|e| e.commit_version).max()` at 155-156)
- **Severity:** low
- **Issue:** When a sealed segment lacks a valid `.meta` sidecar (pre-#500 segment, or interrupted sidecar write), `open` computes `max_version` by fully replaying: `replay_inner` does `read_to_end` of the whole file (up to 8 MiB per segment with no capacity hint — geometric growth again) and bincode-*decodes* every entry — materializing each `Vec<WalOpV2>`, every `Bytes` body, and every `InternerOverlayMerge` `String` — only for the caller to read one `commit_version` field and drop everything.
- **Failure scenario:** Cold start after a long downtime on a pre-sidecar database: startup time and transient RAM spike are O(total WAL bytes × decode cost) — exactly the cost the sidecar was added to avoid, paid in full on the fallback, multiplied by allocation churn the value extraction does not need.
- **Suggested fix:** Add a streaming frame walk that reads only each payload's `commit_version` (it is the 4th fixed-width `u64` in the bincode body, so a fixed-offset slice read with the existing CRC check suffices — or a minimal `Deserialize` impl on a header-only proxy type), without materializing ops. Also seed the `read_to_end` buffer from `metadata().len()`.

### 5. `replay` materializes the entire WAL as decoded entries in one Vec
- **File:** `crates/shamir-wal/src/segment_set.rs:423-446` (`out.extend(...)` over all sealed + active); `crates/shamir-wal/src/wal_segment.rs:527-530`; `wal_sink.rs:158-170` (Mem arm)
- **Severity:** low
- **Issue:** Recovery accumulates every decoded `WalEntryV2` from every segment into a single `Vec` before returning, so peak RAM is O(total un-truncated WAL bytes × decode-expansion factor), not O(one segment). On the Mem arm the same shape holds with the frames lock held across the whole decode loop (sync code, no `.await` — legal, but it blocks any concurrent appender for the full replay).
- **Failure scenario:** A large un-truncated backlog (wedged interner gate + power loss) on a memory-constrained host: recovery can OOM even though a streaming/callback-based replay (decode → hand to recovery consumer → drop) would run in O(largest segment) memory.
- **Suggested fix:** Offer a streaming variant (`replay_with(|entry| ...)` or an iterator/chunked API) alongside the Vec-returning one; the recovery consumer in `shamir-tx` sorts by `commit_version` anyway, which can be done with a two-pass (offsets then ordered decode) or an external-sort-free merge over per-segment streams.

### 6. Full window bytes memcpy'd into the coalescing buffer per append batch
- **File:** `crates/shamir-wal/src/wal_segment.rs:223-234` (`Vec::with_capacity(total)` + per-payload `extend_from_slice`)
- **Severity:** nit
- **Issue:** Every encoded payload is copied a second time (encode → pending Vec → payloads Vec → coalesce buf → kernel). The copy buys exactly one `write()` syscall instead of 3N, which is a sound trade (fsync dominates; and on Windows `File` does not advertise vectored writes, so `writev` would degrade to a loop anyway) — noted here for completeness, not as a defect.
- **Suggested fix:** If it ever shows in a profile: `Write::write_vectored` with an IoSlice chain on unix (guarded), or `bytes::Bytes`-backed payloads so frames can share allocation with the entry bodies.

### 7. One `Arc<Waiter>` heap allocation per single append
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:175` (`Arc::new(Waiter::new())`)
- **Severity:** nit
- **Issue:** Every `append` (i.e. every commit not using `append_many`) allocates a `Waiter` (two atomics + a `Notify`). The alternatives (oneshot channel, waiter slab/pool) were measured or reasoned to be worse — the crate's own docs record that the oneshot-per-append design of the reverted single-writer prototype cost ~+22% mem N=1 latency — so this is a documented, deliberate cost.
- **Suggested fix:** None required. If ever revisited, a lock-free intrusive waiter list reusing the caller's stack frame would remove the alloc without a channel round-trip.

## Theme-coverage note (no action)

The two sanctioned mutexes on the append path (`WalGroupCommit::pending`, `SegmentSet::inner`) are concurrency-theme items and were explicitly investigated and closed in CLAUDE.md (#1095/#1109, architectural single-writer argument) — not re-litigated here. `MemSink.frames` growing without bound until `truncate_below` is WAL-by-definition (cannot drop before durability), not a buffering bug. Test/bench coverage for this theme is good: `benches/wal_append.rs` (contention × tier), `benches/wal_startup_open.rs` (sidecar vs full-replay startup), `benches/segment_set_lock.rs` (lock cost), plus `synced_fsyncs_are_batched` / `buffered_only_window_issues_no_fsync` asserting the syscall amortization the design claims.

</details>
