# P34-07b bounded-memory vector snapshot — stages A–E current status (CI pending)

No production generation algorithm changes were made by this follow-up. No tests or builds were run locally under repository policy; the orchestrator will report the local formatting and lint gates from their actual outputs. CI must verify acceptance tests; no measured memory numbers are claimed. No dependencies were added. Production generation/sweep/prune logic remains unchanged; the only non-test production edit is moving `File`/`Write` imports to `load.rs` file scope.

## Original design investigation — retained historical evidence

This section records the baseline findings and target from the initial design review (source parent `f7144200`), not assertions that the old implementation is still in the current branch.

At parent `f7144200`, `codec.rs` whole-section reads/chunk accumulation were at the old dump body and `slice_into_chunk_ops` (the historical design recorded the original source ranges as codec.rs 368–371, 440–444, 592–594, 600 onward; chunk wire type at 135–145; sidecar iteration/encoding at 476–542). `load.rs` retained returned chunk records and reassembled whole sections before temp-file writes (historical ranges 150–172, 193–227, helper 445–471; temp files 240–298), and cloned sidecar map pairs (262–267) before rebuilding maps (385–408). These parent-commit locations document the original memory profile; line positions can shift in the current implementation.

The bounded target is fixed-size chunks and bounded batches, incrementally updating the same per-chunk and cross-section CRC order. Load writes one validated chunk at a time to temp files. This is O(chunk × fixed batch) for section transport plus HNSW/sidecar state. The sidecar remains a single persisted record; encoding and decoded sidecar state impose a record-sized memory floor, so strict O(chunk) independent of sidecar size is not promised.

The manifest remains the publication commit record. Dump writes chunks and sidecar/integrity before manifest; failure between integrity and manifest under the default sequential `Store::transact` may leave the old manifest with a newer integrity record, making the old metadata fail closed on load. Recovery requires rebuilding from source records; the old snapshot is not asserted loadable in that boundary. After a successful publication, prune failure may leave stale generation keys while the new manifest remains loadable.

The verified-active orphan sweep is deliberately conservative: only after metadata verifies sufficiently to identify the active generation, scan the keyspace prefix, remove only exact recognized generation-record forms (sidecar or six-digit graph/data/qgraph/qdata chunk keys), in bounded batches, and never remove active-generation keys. Missing/corrupt/untrusted metadata does not authorize deletion; skip the sweep. The active generation is protected; a requested generation equal to it rolls to the checked successor. Unrelated and delta keys are outside the sweep.

## Stage A — complete reference parity oracle

`crates/shamir-index/src/vector/snapshot/tests/streaming_acceptance/parity.rs` has an independent test-local old dumper: whole-file f32 and fitted-u8 reads, 1 MiB chunking, unchanged `ChunkHeader` bincode, per-chunk CRC, graph/data/qgraph/qdata section CRC order, sidecar fields (including `vectors_u8` and `QuantMeta`), manifest, and integrity. It collects the complete `iter_stream` result and compares every `(RecordKey, Bytes)` record, sorted by bytewise key order; no records are omitted. Since `MetaEnvelope::new` timestamps with `SystemTime`, only bytes 6..14 of expected manifest and sidecar are normalized to actual timestamps, then expected integrity is recomputed and checked against the stored integrity record. The non-quantized fixture asserts a section exceeds `CHUNK_SIZE`; the fitted-quantized fixture covers the old empty f32 section records and u8 sections. Vendored `hnsw_rs/src/hnswio.rs` serializes points by layer and point index, not by generated `RecordId` ordering.

## Stage B — successful-allocation high-water oracle

The existing one global allocator preserves the thread-local cumulative window and independently tracks a process-wide largest successful allocation request via atomics, including `spawn_blocking` allocations. It observes successful `alloc`, `alloc_zeroed`, and `realloc` only; failed realloc is not recorded. This measures a largest-single-allocation signal, not outstanding live bytes. Dump/load tests explicitly skip unless `NEXTEST` is present. Their bound derives from CHUNK_SIZE, twice the actual encoded sidecar length, adapter-vector capacity floor, and 256 KiB reserve; a separate `file_dump` supplies actual file sizes, requires a 4× section-to-bound margin, and the old whole-file read is measured to prove it exceeds the same bound. The memory fixture constructs a private adapter via existing `HnswAdapter::from_parts`: it calls `Hnsw::set_keeping_pruned(true)` before putting the graph in `Arc`, with empty, correctly hashed maps and `next_id=0`, then inserts fixture vectors normally. Vendored `hnsw.rs` documents the flag's purpose and retains pruned candidates; this prevents default neighbor-selection pruning from undermining density. The fixture uses 8,192 points and M=128. The runtime file-size assertion remains the acceptance check; no file-size measurement is claimed here.

## Stage C — sequential failure and real restore

`streaming_tests::failure_after_integrity_before_manifest_is_detected_and_rebuildable` uses the trait default `Store::transact` (no wrapper override). Its wrapper delegates integrity `set`, then fails the manifest `set`; it verifies that the old manifest bytes remain, integrity changed, and `load_snapshot` returns exactly `Corrupt("snapshot metadata integrity mismatch")`.

`vector/tests/backend/vector_restore_tests.rs::sequential_metadata_failure_triggers_backend_rebuild_from_source_records` repeats that publication window with a source data store containing the serialized `InnerValue` records expected by backend rebuild. It verifies the real `VectorBackend::restore_on_open` path reports `rebuild_count == 1`, recovered IDs equal the source IDs, and returned search results are among those IDs. Existing `integrity_tests::dump_and_flip_publish_companion_before_bounded_prune` remains in place for the post-publication prune-failure boundary.

## Stage D — source hygiene

`load.rs::stream_section_to_file` imports `std::fs::File` and `std::io::Write` at file scope. No other production code was changed.

## Stage E — generation caller audit and explicit-gen contract

Searched `dump_snapshot` and `dump_snapshot_with_gen` throughout `crates/` and inspected caller contexts. The requested generation is not a guaranteed published value: callers requiring the selected generation read the manifest. Fresh keyspaces keep their supplied initial generation; if the request equals the verified active generation, the writer uses the checked successor.

- `shamir-index/src/vector/vector_backend/maintenance.rs::run_background_snapshot`: reads verified metadata, obtains a candidate from `unused_generation`, dumps, rereads actual manifest values, and then flips/prunes. The backend uses its single-flight snapshot guard.
- `shamir-engine/examples/persisted_hnsw.rs`: `dump_snapshot` initializes the example's fresh keyspace before restore.
- `shamir-index/src/vector/snapshot/tests/streaming_tests.rs`: direct generation zero calls initialize new stores; the failure test reads active manifest and requests successor.
- `shamir-index/src/vector/snapshot/tests/streaming_acceptance/{parity,memory}.rs`: parity uses a fresh keyspace; memory fixtures each use fresh stores.
- `shamir-index/src/vector/snapshot/tests/integrity_tests.rs`: fixture and initial dump calls use fresh stores; the follow-up generation now derives from `old.gen + 1`.
- `shamir-index/src/vector/snapshot/tests/legacy_tests.rs`: initial legacy-compatibility dump uses a fresh keyspace; later manifest rewrites deliberately exercise legacy format.
- `shamir-index/src/vector/tests/backend/vector_restore_tests.rs`: standalone restore snapshots use fresh stores/keyspaces; sequential failure reads the active generation and requests `active.gen + 1`.
- `shamir-index/src/vector/tests/lifecycle/delta_log_tests.rs`: initial dump calls seed fresh per-test keyspaces. Follow-up generations now derive from `read_manifest` (`gen_zero.gen + 1`, `current.gen + 1`). The legacy crash fixture directly rewrites the manifest and removes its integrity record to model the pre-companion state; that is not a call relying on a dump's selected generation.
- `shamir-index/src/vector/tests/lifecycle/index_durability_tests.rs`: base dump is fresh; explicit next requests use `m0.gen + 1`.
- `shamir-index/src/vector/tests/lifecycle/snapshot_tests.rs`: normal dumps seed fresh stores; the explicit-generation prune test already requests `old.gen + 1`.
- `shamir-index/src/vector/tests/lifecycle/crash_recovery_tests.rs`: `dump_fresh_snapshot` creates the snapshot for each recovery fixture's fresh store/keyspace; other dumps likewise seed fresh fixture stores.
- `shamir-index/src/vector/tests/prune/delta_prune_tests.rs`: `seed_base_and_open` initializes a fresh keyspace before restore; subsequent generation selection is through `snapshot_now` / manifest-driven maintenance.
- `shamir-index/src/vector/tests/quant/quantization_snapshot_tests.rs`: each direct `dump_snapshot` call starts from a test-local store for quantization round-trip coverage.
- `shamir-index/src/vector/tests/lifecycle/compaction_tests/quantized.rs::vr6_sq8_compaction_snapshot_is_v2`: dumps the compacted quantized adapter to a newly created in-memory store; generation zero is initial.

The remaining literal generation-zero uses are initial/fresh-keyspace fixtures and `dump_snapshot`'s initial request. Follow-up literals 1/2 in delta/index durability fixtures were changed to read the active manifest first. No direct `shamir-db` snapshot caller was found. Corrupt/missing metadata still does not authorize a sweep; existing publication ordering, exact-key bounded sweep, active-gen protection, and prune implementation remain unchanged.

## Verification

`git diff --check` passed during the follow-up. `cargo fmt -p shamir-index` and `cargo clippy -p shamir-index --all-targets -- -D warnings` completed successfully. Tests/builds were not run; CI must verify acceptance behavior and measured allocation assertions.

## Historical source notes

Old source evidence: `codec.rs` whole reads and whole `Vec<KvOp>` path; `load.rs` full section reassembly and sidecar cloning; `manifest.rs` had no orphan sweep before streaming batches. These were the motivation for retaining bounded streaming and conservative sweep rules above. See the initial remediation prompt and the historical design version at the parent commit for the full original file:line investigation and crash discussion.
