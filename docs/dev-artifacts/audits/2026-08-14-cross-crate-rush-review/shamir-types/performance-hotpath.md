<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-types — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Live projection rescans are the clearest current asymptotic issue. Other allocation opportunities are structurally present but unmeasured; scratch handoff is intentional and aggregate validation is justified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 7 | 0 | 0 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Projection codec re-scans the whole record once PER selected field id — O(fields x selected) per row

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Each field_value_bytes call starts at body offset zero on live S-read projection, giving worst-case O(selected*fields) traversal plus subtree skipping. Registered tests do not detect scaling.

Evidence: [crates/shamir-types/src/codecs/interned/projection.rs:62](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/projection.rs#L62); [crates/shamir-types/src/record_view/lens.rs:1139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L1139); [crates/shamir-engine/src/table/read_exec.rs:1544](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/read_exec.rs#L1544).

<a id="review-2"></a>

### Claim 2 — `RecordRef::for_each_field` (lens impl) is O(f^2): one full map re-scan per field

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The already-read value is discarded and materialize_at rescans for every field. Only tests currently call this method; production SELECT * uses other paths.

Evidence: [crates/shamir-types/src/record_view/record_ref.rs:338](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/record_ref.rs#L338); [crates/shamir-engine/src/table/read_exec.rs:1537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/read_exec.rs#L1537).

<a id="review-3"></a>

### Claim 3 — Zerocopy msgpack decoder allocates an owned `String` per map KEY even though keys go straight into the interner

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Nonempty keys are copied before borrowed interner lookup. This custom decoder requires external string keys and has no current production caller found, so its alleged WAL hot-path impact is unsupported.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:139](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L139); [crates/shamir-types/src/codecs/interned/messagepack.rs:324](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L324).

<a id="review-4"></a>

### Claim 4 — `query_value_to_storage_bytes_into` defeats its own scratch-buffer purpose — capacity resets to 0 every call

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Consumed scratch and regrowth are explicit contracts with a registered capacity-zero test, introduced before the review. Bytes 1.11.1 published src/bytes.rs transfers the payload allocation; reserve replacement does not reuse it.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:882](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L882); [crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs:534](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/tests/storage_bytes_tests.rs#L534); [Cargo.lock:571](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L571).

<a id="review-5"></a>

### Claim 5 — Raced `touch_ind` cold-misses permanently leak interned ids — interner memory tracks racing touches, not distinct names

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

fetch_add occurs before entry arbitration, so Occupied losers permanently leave ID holes. Those affect spine sizing and delta recapture, but temporary String/Arc allocations are freed.

Evidence: [crates/shamir-types/src/core/interner/interner.rs:157](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L157); [crates/shamir-types/src/core/interner/interner.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L163); [crates/shamir-types/src/core/interner/interner.rs:554](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/core/interner/interner.rs#L554).

<a id="review-6"></a>

### Claim 6 — `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Replacement values and new entries are independently encoded and copied into output. Streaming could remove these payload temporaries without changing order/encoding; no significance was measured.

Evidence: [crates/shamir-types/src/codecs/interned/messagepack.rs:648](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L648); [crates/shamir-types/src/codecs/interned/messagepack.rs:661](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L661); [crates/shamir-types/src/codecs/interned/messagepack.rs:667](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L667).

<a id="review-7"></a>

### Claim 7 — Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Non-admin checks allocate a Vec and clone parent segments. FunctionFolder chains are caller-sized; preserving nearest-first awaited checks constrains a callback replacement.

Evidence: [crates/shamir-types/src/access.rs:524](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L524); [crates/shamir-types/src/access.rs:550](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L550); [crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:850](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/access_control.rs#L850).

<a id="review-8"></a>

### Claim 8 — Lazy aggregate cursors pay an eager full-subtree validation walk, then walk again when consumed

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The eager skip followed by lazy iteration is documented and establishes safe slice bounds. Nested consumption need not cost exactly twice the work.

Evidence: [crates/shamir-types/src/record_view/lens.rs:622](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L622); [crates/shamir-types/src/record_view/lens.rs:641](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L641).

<a id="review-9"></a>

### Claim 9 — `QueryValue::set_path` builds the error-message path prefix during successful traversals too

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

set_path updates walked before every branch, including successful traversal. This is avoidable copying, not proof of per-segment allocation or material runtime regression.

Evidence: [crates/shamir-types/src/types/value.rs:590](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L590).

## Evidence and recipe corrections

- FieldIndex stores starts, not complete spans, and trusts the map header for capacity. It is not a safe drop-in optimization without bounded construction and explicit accepted-input semantics.
- Using existing FieldIndex changes repeated-key lookup from first-wins to last-wins unless duplicate inputs are rejected or handled deliberately.
- Bytes 1.11.1 can allocate Shared bookkeeping when Vec length differs from capacity; zero-copy payload transfer is not an allocation-free guarantee.
- DashMap 6.1.0 published src/lib.rs::_len reads all shards; it is not an atomic/sharded-counter cardinality operation.
- A synchronous ancestors callback cannot directly replace authorization's asynchronous metadata checks.
- Numeric marker counts, throughput collapse, per-operation nanoseconds, and exact double-walk multipliers were not measured.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-types -- Performance & O(x->0)

## Summary

The crate is largely exemplary on pillar 3: the zero-copy `RecordView` lens with its amortizing
`FieldIndex`, the doubling-growth interner reverse spine (#501), the streaming storage encoders
(`SANE_PREALLOC_CAP`, `query_value_to_storage_bytes*`), and the single-pass byte-level
`merge_storage_bytes` all drive per-op cost toward O(1)/O(N). The residual findings are two hidden
re-scan patterns that quietly re-introduce O(N^2)-shaped products on per-row paths
(`record_view_to_id_msgpack`, `RecordRef::for_each_field` for the lens impl), one scratch-buffer
API whose stated capacity-reuse win is defeated by its own `std::mem::take`, and one avoidable
per-key allocation in the zerocopy decoder. Tests cover parity, not scaling, so none of these
would surface as a failure today.

## Findings

### 1. Projection codec re-scans the whole record once PER selected field id — O(fields x selected) per row
- **File:line:** `crates/shamir-types/src/codecs/interned/projection.rs:62-67`
  (loop calling `view.field_value_bytes(id.clone())`; the underlying linear scan is
  `crates/shamir-types/src/record_view/lens.rs:1131-1168`, `value_bytes_at`)
- **Severity:** high
- **Issue:** `record_view_to_id_msgpack` calls `field_value_bytes` for each id in
  `selected_ids`. Every call runs `value_bytes_at`, which restarts at body offset 0 and linearly
  walks key markers + `skip_value`s until it matches (or exhausts) the map. A projection of k ids
  over an f-field record therefore costs O(k*f) key comparisons/value skips *per row*. The crate's
  own lens docs state the design intent ("reading N fields is O(fields) amortised, not
  O(fields^2)" via `FieldIndex`) — this consumer bypasses exactly that primitive.
- **Failure scenario:** `shamir-engine/src/table/read_exec.rs:1538,1579` invokes this per row on
  the S-read path. `SELECT a, b, c, ... (20 cols)` over a 60-field record does ~1200 marker reads +
  value skips per row instead of ~80; the cost grows multiplicatively with record width and
  projection width, invisible to correctness tests (`projection_tests.rs` uses tiny fixtures).
- **Suggested fix:** Build a one-pass span index per view first (extend or reuse
  `FieldIndex::index()` so each entry also records the value's end offset / byte span), then emit
  each selected id by probing the map in O(1). Total becomes O(f + k). The existing
  `projection_tests` (byte-parity against `record_view_to_query_value`) will verify the output is
  unchanged.

### 2. `RecordRef::for_each_field` (lens impl) is O(f^2): one full map re-scan per field
- **File:line:** `crates/shamir-types/src/record_view/record_ref.rs:338-346`
  (per-field `materialize_at(&[key])` → `lens.rs:1123-1125` → full scan in `value_bytes_at`)
- **Severity:** medium
- **Issue:** The implementation iterates `fields()` (one full O(bytes) pass), then for EVERY field
  calls `materialize_at([id])`, which restarts the scan from offset 0 and skips forward to that
  field again. Sum of scan positions = O(f * total_bytes / 2); additionally the value already
  decoded inside `fields()` (`_val` is explicitly discarded) validates the same subtree a second
  time through `read_value`'s eager aggregate skip (`borrow_seq_body`/`borrow_map_body`).
  Documented as "Used for SELECT * / full-record projection" — the one cross-crate use case where
  quadratic behaviour bites hardest.
- **Failure scenario:** Currently only exercised by `record_view/tests/record_ref_tests.rs`
  (`for_each_field_parity`) — grep finds no shamir-engine caller yet, which is why this is not
  ranked above finding 1. When Stage-3 wires SELECT * onto it (the trait doc names exactly that),
  a 100-field record pays ~5,000 extra key decodes+skips per row vs 100.
- **Suggested fix:** Single pass capturing `(id, val_start, val_end)` spans while walking
  `fields()`' cursor positions, then `InnerValue::from_bytes(&body[span])` per field; or build the
  same span index as in fix #1 and share it between both call sites.

### 3. Zerocopy msgpack decoder allocates an owned `String` per map KEY even though keys go straight into the interner
- **File:line:** `crates/shamir-types/src/codecs/interned/messagepack.rs:130-140`
  (`read_str` always returns `Ok(s.to_string())`), consumed at `messagepack.rs:324-341`
  (`decode_map`: `read_str(cur, klen)?` then `intern_string_key(interner, &key_str)`)
- **Severity:** medium
- **Issue:** `decode_map` materialises each wire key as a heap `String` purely to hash it against
  the interner — whose hot fast path (`interner.rs:141-147`, `UserKey: Borrow<str>`,
  documented as "no String alloc on cache hits, the 99% case") exists precisely to make such
  lookups allocation-free. On any decode of stored form the field-name set is warm, so every row
  re-pays f allocations that buy nothing. Values must be owned anyway (the target is an owned
  `InnerValue` tree) — keys are the pure waste.
- **Failure scenario:** Whole-record decode paths (`msgpack_to_inner`, WAL recovery replays,
  benches `codec_msgpack.rs:106` as the reference decode) allocate one transient `String` per key
  per row; for a 30-field table at batch-recovery speed this is tens of thousands of short-lived
  allocs/sec feeding the allocator for data immediately discarded.
- **Suggested fix:** Make `read_str` borrow: `fn read_str<'a>(cur: &mut Cursor<&'a [u8]>, len) ->
  Result<&'a str, CodecError>` (validate UTF-8 on the borrowed subslice via
  `std::str::from_utf8`, then advance position — the payload outlives the mutable borrow).
  Pass `&str` to `intern_string_key`; no signature change needed there. Storage-bytes round-trip
  tests already pin behaviour.

### 4. `query_value_to_storage_bytes_into` defeats its own scratch-buffer purpose — capacity resets to 0 every call
- **File:line:** `crates/shamir-types/src/codecs/interned/messagepack.rs:890-910`
  (`Ok(Bytes::from(std::mem::take(scratch)))` at :909)
- **Severity:** medium
- **Issue:** Doc comment claims: "The caller keeps ownership of `scratch` for the loop variable;
  the key win over a bare `rmp_serde::to_vec` call is [capacity retention]" and "eliminates the
  +1 alloc + memcpy per row that was causing the L12 regression on N=1". But
  `std::mem::take(scratch)` moves the grown Vec into the returned `Bytes` and leaves `scratch`
  empty **with capacity 0**; from call #2 onward the writer reallocates from nothing and grows
  geometrically — steady-state allocation churn equals plain `to_vec`, only round 1 benefits.
- **Failure scenario:** `shamir-engine/src/query/batch/query_runner.rs:1792` and
  `shamir-engine/src/table/write_exec.rs:250` pass `&mut scratch` inside per-row loops exactly for
  the promised reuse; a batch INSERT of N rows still performs N independent growth sequences.
  No test asserts capacity retention across iterations
  (`codecs/interned/tests/storage_bytes_tests.rs` only checks output bytes), so the regression
  this function was written to fix is silently back.
- **Suggested fix:** Restore the buffer before returning, e.g. capture `let cap =
  scratch.capacity(); let v = std::mem::take(scratch); *scratch = Vec::new();
  scratch.reserve_exact(cap.max(64)); Ok(Bytes::from(v))` — or return the scratch along with
  `Bytes` sliced from it (`Bytes::copy_from_slice` reintroduces memcpy, so prefer the reserve
  pattern). Add a test asserting `scratch.capacity()` survives a second call.

### 5. Raced `touch_ind` cold-misses permanently leak interned ids — interner memory tracks racing touches, not distinct names
- **File:line:** `crates/shamir-types/src/core/interner/interner.rs:154-166`
  (`fetch_add` before the forward-map CAS; lost races "silently leaked"), reverse sizing at
  `interner.rs:243-260` (`grown_reverse` sized to max leaked id)
- **Severity:** low
- **Issue:** Documented and accepted ("small leaks are harmless", "monotonic and small"). Under a
  thundering-herd cold touch (many threads racing the FIRST touch of the same new field name —
  e.g. schema migration fan-out or replay burst), every loser burns a `fetch_add` slot: id space,
  the doubling-grown reverse spine (length >= max raced id), and one wasted `Arc<str>` alloc per
  race. Because ids never recycle and `generation()`/persistence bounds ride on them, growth is
  proportional to total touches including races, forever. Amortized cost per op stays O(1); this
  is an unbounded-buffering note within the theme, flagged because the code treats contention as
  rare without measuring how often concurrent cold misses occur in production startup bursts.
- **Failure scenario:** K threads cold-touch the same new field name → up to K-1 leaked slots;
  repeated across hundreds of racing names during recovery, the reverse vec inflates well past
  the distinct-name count, growing every later `entries_after` persist-delta scan.
- **Suggested fix:** Keep the leak for simplicity, but only retry the fetch_add after losing the
  CAS when truly unavoidable... practical cheap option: move the id reservation INSIDE the
  `Entry::Vacant` arm (reserved-but-unused ids then don't advance the high-water mark consumed by
  persistence scans), or document a measured bound.

### 6. `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer
- **File:line:** `crates/shamir-types/src/codecs/interned/messagepack.rs:658-671`
  (`rmp_serde::to_vec(key)` + `val.to_bytes()` allocated, then immediately
  `buf.extend_from_slice`)
- **Severity:** low
- **Issue:** The rest of the function is carefully allocation-free (verbatim span copies,
  capacity pre-estimate at :634), but each new-entry encode builds a throwaway heap Vec that is
  memcpy'd once and dropped — an allocation-in-loop for wide new-field updates. Same pattern as
  the bug `query_value_to_storage_bytes_into` was created to remove (#61/L12 history).
- **Failure scenario:** UPDATE adding many fields to a wide record allocates/frees 2 Vecs per new
  field; allocator churn scales with new-field count x batch size. Correctness unaffected.
- **Suggested fix:** Serialize directly into `buf` via `rmp_serde::encode::write(&mut buf, key)`
  and a small `Serialize` wrapper for values (or reuse `QvInternedRef`-style streaming).

### 7. Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path
- **File:line:** `crates/shamir-types/src/access.rs:504-546` (`parent()` clones db/store/table/
  name Strings per level), `access.rs:550-558` (`ancestors()` builds a `Vec<ResourcePath>`)
- **Severity:** low
- **Issue:** Every traversal decision re-materialises the ancestor chain: each level clones all
  path segments into brand-new Strings even though callers typically have the originals alive,
  plus the Vec. Depth is bounded (~<=5) so it is constant-factor, not asymptotic — but it runs
  per-op in front of `permits()` on a path the code itself calls "on the hot path"
  (`AccessError` doc, access.rs:626-628).
- **Failure scenario:** Per-request authorisation does ~4 Vector+String-set allocations that
  exist only to be pattern-matched and dropped; visible as allocator overhead under high rps,
  never as a wrong answer.
- **Suggested fix:** An `ancestors_with<&F>` closure-callback variant (walk without collecting),
  or borrow-based ancestors returning segment slices, letting callers skip building owned paths
  they only inspect.

### 8. Lazy aggregate cursors pay an eager full-subtree validation walk, then walk again when consumed
- **File:line:** `crates/shamir-types/src/record_view/lens.rs:625-659`
  (`borrow_seq_body`/`borrow_map_body` run `skip_value` over every element/pair just to bound the
  slice)
- **Severity:** nit
- **Issue:** `read_value` returns "lazy" `RawSeq`/nested `RecordView` cursors only AFTER eagerly
  skipping the entire subtree for truncation validation. Consumers that then iterate the cursor
  (`RawSeqIter`, nested `fields()`) decode the same bytes twice. This buys untrusted-input safety
  and exact slice bounds — a deliberate tradeoff worth recording here only so it isn't counted
  twice when profiling filter-heavy array workloads.
- **Failure scenario:** None (correctness-driven); ~2x bytes-touched per consumed aggregate.
- **Suggested fix:** Optionally cache validated spans, or document the double-walk in the module
  docs so future lens consumers budget for it.

### 9. `QueryValue::set_path` builds the error-message path prefix during successful traversals too
- **File:line:** `crates/shamir-types/src/types/value.rs:591-594` (`walked.push_str(segment)`
  executed on the success path for every intermediate segment)
- **Severity:** nit
- **Issue:** `walked` exists solely to render `ValueError::NotAMap`, but is incrementally built
  (allocation + copy per segment) even when traversal succeeds and no error is ever raised.
  Constant-factor waste in a mutation helper used by update-paths.
- **Failure scenario:** None.
- **Suggested fix:** Only assemble `walked` lazily on the error branches (collect segment stack,
  format on demand), or accept the byte-count and add a one-line comment stating the tradeoff.

---

*Test-coverage context:* module-level `tests/` dirs exist for every submodule per house style;
they are strong on parity/round-trip but use small fixtures, so the quadratic patterns in
findings 1-2 and the capacity-reset in finding 4 are invisible to the current suite (grep found
no scaling/capacity assertion). `clippy.toml` bans scc `len()`, not DashMap's — `Interner::len()`
(:308) is sharded-counter based and compliant.

</details>
