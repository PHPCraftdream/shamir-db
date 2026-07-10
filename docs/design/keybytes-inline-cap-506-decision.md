בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Task #506 decision: raise `KeyBytes::INLINE_CAP` or add a posting-key tier

Step 5 of `docs/design/record-key-128-migration-plan.md` §4, explicitly
scoped as "optional, measure-first" and "do NOT bundle with 1-4".

## Investigation

`INLINE_CAP = 23` (`crates/shamir-storage/src/key_bytes.rs`) covers the
three hot key shapes the migration targeted: `RecordId` (16 B),
`WalActiveKey` (21 B), and the 9-byte index prefix — all inline. The
41-byte legacy posting key (`index_key(25 B) ‖ record_id(16 B)`,
`crate::legacy::index_keys::build_posting_key`,
`crates/shamir-index/src/legacy/index_keys.rs:306`) exceeds the cap and
falls back to `KeyBytes`'s heap `Bytes` variant.

**Where the 41-byte posting key is actually used:** `build_posting_key`
is called on the WRITE path only — once per posting insert/update
(index maintenance on record insert/update/delete), confirmed by reading
its call sites. It is NOT on the hot read/lookup/comparison path: since
task #499 (posting-list `Arc<[RecordId]>` migration), a cache hit
returns the cached `Arc<[RecordId]>` slice directly — comparisons and
iteration happen over raw 16-byte `RecordId` values within that slice,
never over the 41-byte posting key itself. The posting key's only job is
to be the PHYSICAL STORAGE KEY for one `set()`/`remove()` call at write
time, matching the migration plan's own §3 landmine-4 note: "Posting
keys (41 B) stay heap unless the cap is raised — acceptable; they are
built once per posting write, not per comparison."

## Decision: defer, no change

Raising `INLINE_CAP` past 23 without growing `KeyBytes` past its 32-byte
budget requires an `unsafe` union/niche-layout trick (per
`key_bytes.rs`'s own doc comment on why 23, not the plan's nominal 30,
was chosen — `bytes::Bytes` on this target already consumes the full
24-byte heap-variant budget, so anything past `INLINE_CAP = 23` pushes
`size_of::<KeyBytes>()` to 40 bytes for a plain safe tagged enum). Given:

1. The posting key's ONLY hot-adjacent cost is one heap allocation per
   posting WRITE (not per read, not per comparison) — a cost this
   session's #499 work already confirmed is not on the dominant read
   path.
2. Adding a separate posting-key tier (a distinct small-buffer type sized
   for exactly 41 bytes) would introduce a SECOND key representation
   alongside `KeyBytes`, adding real complexity (another Eq/Ord/Hash
   surface to keep bug-free per landmine 1) for a write-path-only,
   already-acceptable cost.
3. Pursuing the `unsafe` union approach to raise `INLINE_CAP` carries
   real correctness risk (as the `key_bytes.rs` doc comment itself notes,
   deferring it "to a separate, measurement-driven follow-up rather than
   risk UB here") for the same modest, write-path-only benefit.

No measured signal currently justifies either approach. This is filed as
a conscious "won't fix without new evidence" decision, not a punt — if a
future workload profile shows posting-list WRITE throughput (not read)
is actually bottlenecked on this specific allocation (e.g. via a
dedicated `storage_fjall_pump`-style bench isolating posting-key
construction cost under bulk index-build load), re-open with that data.

No code change in this task.
