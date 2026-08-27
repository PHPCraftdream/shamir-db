# shamir-types -- Concurrency & lock-free invariants

## Summary
The crate models CLAUDE.md's 5 pillars well on its shared-state surface: the interner is `DashMap(THasher)` + `ArcSwap` RCU reads + `AtomicU64` ids, hash-keyed structures are uniformly Fx (`TDashMap`/`TFxMap`/`TMap`), the single `std::sync::Mutex` is the registered F-9 category-3 first-touch-only exception with an inline contention-model comment and a documented data-loss rationale, there is no async/`.await` in the crate (so no guard-across-await risk), no `scc::*` usage (so no banned O(N) `scc::*::len()` calls), and `RecordId`'s RNG is per-thread (`thread_local!`). Findings below are therefore medium-and-below: two hidden-complexity/O(N²)-shaped hot-path issues against pillar 3, one documented-but-unguarded growth stall on the sanctioned mutex, transient forward-before-reverse publication windows, and stale README claims about the locking model.

## Findings

### 1. `RecordRef::for_each_field` (lens impl) is O(fields²) — repeated full-scan lookups plus double decode of every value
File: crates/shamir-types/src/record_view/record_ref.rs:338–346 (scan primitive: crates/shamir-types/src/record_view/lens.rs:1131–1168)
Severity: medium

Issue: The `RecordView` impl iterates `fields()` (a sequential walk over the map body) but discards each already-decoded value (`for (key, _val)`) and then calls `materialize_at(&[key])` per field. `materialize_at` → `value_bytes_at` restarts its cursor at offset 0 and linearly re-scans keys + `skip_value`s every preceding entry until it matches the target id. So a wide record costs Σ(i) skip/decode work = O(fields²) bytes walked, and each value's marker is processed twice (once thrown away in `fields()`, once re-skipped/re-parsed in `value_bytes_at`). This is exactly CLAUDE.md pillar 3's "hidden O(N)/O(N²) in helpers (full scans, repeated lookups)" pattern; there is no ack comment. Per the crate's own docs (`record_view/mod.rs`), this trait is consumed at 34+ call sites across `shamir-engine`, including `SELECT *` / full-record projection paths.

Failure scenario: per-row cost on projection-heavy workloads grows quadratically with record width; e.g. a 500-field record pays ~125k skip_value operations + double decodes per row, which shows up as throughput collapse rather than any error.
Suggested fix: collect (id → value-offset) once while walking `fields()` (the offset is already known when the key is read), or build `FieldIndex` via `index()` and materialize from indexed offsets; either way decode/skip each value exactly once and materialize directly from the recorded span (lens.rs even stores those offsets already in `FieldIndex`).

### 2. Header-driven unbounded preallocation in the zerocopy decoder and merge encoder (allocation abort on malformed input)
File: crates/shamir-types/src/codecs/interned/messagepack.rs:305 (`decode_array`: `Vec::with_capacity(len)`), :318 (`decode_map`: `new_map_wc(len)`), :581/:584 (`merge_storage_bytes`: `Vec::with_capacity(n_old)` + `TFxMap::with_capacity_and_hasher(n_old, …)`); header counts come straight from u16/u32 msgpack headers (:236–254, :680–718)
Severity: medium

Issue: element/entry counts are read from the wire header and passed to capacity allocation without a sanity cap. A `Map32`/`Array32` header can declare up to ~4.29e9 entries; `Vec::with_capacity`/IndexMap allocation then requests tens-to-hundreds of GB and `handle_alloc_error` aborts the process. This crate itself establishes the correct precedent — `SANE_PREALLOC_CAP = 4096` in types/value.rs:117–122 documents precisely this "multi-GB alloc / abort" hazard for the serde visitor path — and lens.rs is naturally safe here (its `skip_value` loops terminate at truncation). The tree decoder and merge path lack the cap while explicitly being "the WAL recovery decode target" and storage-bytes patcher, i.e. they run over bytes whose provenance is only checksum-protected.

Failure scenario: torn/corrupt (or hostile, if an id-msgpack body ever crosses a trust boundary — validate_keys.rs exists precisely because client-supplied ids do) msgpack with `Map32 0xFFFFFFFF` header → process abort during recovery/write-path decode, taking down all in-flight sessions instead of returning a `CodecError`.
Suggested fix: apply the same `min(header, SANE_PREALLOC_CAP)` clamp used by `ValueVisitor::visit_seq/visit_map` at the three sites (Vec/IndexMap still grow on demand for legitimately large records).

### 3. Sanctioned `reverse_write_lock` also held across the doubling-growth clone-forward — write-stall grows with spine length
File: crates/shamir-types/src/core/interner/interner.rs:216–233 (`set_reverse_slot`) and :243–260 (`grown_reverse`)
Severity: low

Issue: The `std::sync::Mutex<()>` is correctly registered as the F-9 category-3 exception ("first-touch-only population"), and amortized total work is documented (#501 geometric series). However, the lock's instantaneous critical section includes the full `grown_reverse` sweep — cloning every existing cell of the spine while holding the mutex. With N distinct fields interned, one unlucky first touch stalls ALL concurrent first touches (~N Arc refcount bumps) inside the lock. Reads stay unaffected (ArcSwap), and first-touch frequency is once-per-distinct-field-name-ever, so contention is nil in steady state; the exposed window is cold-start/WAL-hydration bursts that add many names to a large existing interner.

Failure scenario: hydration of ~1M persisted field names into a live-ish interner serializes all writers behind N successive O(spine-length) clones; nothing corrupts, but latency spikes cluster at growth boundaries (doubling events).
Suggested fix: keep the design (its data-loss justification is sound), but (a) note the growth-boundary stall inline next to the exception comment so future audits see it, and (b) if hydration ever becomes hot, revisit the doc's own sketch of a seqlock-style generation counter around grows — the struct doc already identifies the exact mechanism the CAS protocol lacked.

### 4. Transient forward-before-reverse publication window (and rollback window) visible to racing third-party readers
File: crates/shamir-types/src/core/interner/interner.rs:168–177 (`touch_ind`: DashMap insert commits before `set_reverse_slot`), :402–447 (`touch_with_id`: forward insert committed, then reverse slot decided under lock; collision path removes the forward entry afterward while the reverse slot keeps the other name)
Severity: low

Issue: Between a writer's forward-map insert and its reverse-slot population (or after `touch_with_id`'s collision rollback), another thread that resolved `get_ind(name)` in that window and immediately calls `get_str(id)` observes `None` — surfacing upstream as `CodecError::Decode("Interned key not found")`. It self-heals in nanoseconds and the owning caller never sees it (both directions are populated before `touch_ind` returns `New`), and `entries_after`'s gap semantics are thoroughly documented/tested; but this read-side transient is not called out anywhere a codec caller would look, so consumers may treat it as permanent corruption instead of retrying. The `touch_ind` vs concurrent `touch_with_id` same-id race is acknowledged in code (interner.rs:200–207) as out-of-scope under "recovery does not run concurrently with live traffic" — flagging here because that assumption is enforced by convention cross-crate, not in-code.

Failure scenario: a reader thread interleaves between another thread's forward insert and reverse set → de-intern error logged / row rejected once under load; `touch_with_id` rollback window additionally exposes a name→id mapping that then disappears.
Suggested fix: one sentence on the public `get_str`/`get_ind` docs ("a just-touched key may briefly resolve forward-only under concurrency; `None` is transient") and/or a retry-at-the-call-site helper in codecs::interned::common; optionally encode the recovery-vs-live exclusion in code (an AtomicBool probe epoch) rather than convention.

### 5. README describes an obsolete locking model for `Interner`
File: crates/shamir-types/src/core/README.md:64–67 and :83–86, :128–131
Severity: nit

Issue: `core/README.md` states the reverse map is `TDashMap<InternerKey, UserKey>` and "Current ID: `Mutex<u64>`", and summarizes thread safety as "lock-free concurrent reads, fine-grained writes via DashMap". The actual model is `ArcSwap<Vec<OnceLock<Arc<str>>>>` + `AtomicU64` + the single-writer `reverse_write_lock: std::sync::Mutex<()>` gate (see interner.rs:57–80). Since CLAUDE.md's F-9 exception regime depends on accurate per-site documentation, a stale doc claiming a `Mutex` counter where there is none (and vice versa) actively misleads audits.

Failure scenario: a future reviewer trusts the README, misses the real sanctioned-mutex site or hunts a phantom `Mutex<u64>`.
Suggested fix: refresh §Interner / Thread Safety sections to name `ArcSwap` spine, `OnceLock` slots, `AtomicU64` id counter, and the F-9-cited `Mutex<()>` write gate with a pointer to the struct doc.

---
Notes (no violations found): the sole blocking lock is the registered exception (#3); zero `async fn`/`.await` in the crate so no guard-across-await exists; no `parking_lot`; no `std::collections`/`RandomState` anywhere — every hash structure goes through `THasher`; `DashMap::len()` (not scc, not clippy-banned) backs `Interner::len/is_empty`; `QUERY_VALUE_NULL` (`LazyLock`) is immutable; test coverage for the concurrency-sensitive core is strong (`test_concurrent_growth_no_lost_touches_no_dup_ids` 32×400 stress asserting both directions + unique ids, gap-semantics and trailing-capacity regressions).
