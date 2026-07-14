Task: MEDIUM performance — three independent findings from
`docs/dev-artifacts/audits/2026-07-06-perf-radical-o-notation.md`:
1. **2.4**: `IndexManager::create_index` fully materializes the ENTIRE
   table (as decoded `InnerValue`s) into a `Vec` before building the
   index, DESPITE a stale doc-comment claiming it streams in batches.
2. **3.3**: `FjallStore`'s point ops (`get`/`set`/`remove`) each pay a
   separate `spawn_blocking` dispatch (~1-5µs overhead + locality
   loss) even though batch APIs exist for `get_many`/`transact`.
3. **3.4**: TCP framing does an extra `memcpy` of the ENTIRE response
   payload into a scratch buffer just to prepend a 4-byte length
   before a single write.

These are independent — fix each on its own merits; do not couple them.

## Finding 2.4 — `create_index` materializes the whole table (LOW-MED complexity, fix fully)

- `crates/shamir-index/src/legacy/index_manager.rs`, `create_index`
  (~line 210-234, confirm current lines): the doc-comment (line ~207)
  CLAIMS "Использует потоковую обработку (stream) с батчами по 1000
  записей, чтобы избежать загрузки всех данных в память одновременно"
  (uses streaming with 1000-record batches to avoid loading everything
  into memory) — but the ACTUAL code does:
  ```rust
  let mut stream = self.data_store.iter_stream(FULL_SCAN_BATCH);
  let mut records: Vec<(RecordId, InnerValue)> = Vec::with_capacity(128);
  while let Some(batch_result) = stream.next().await {
      // ... decodes every record, pushes into `records` ...
  }
  ```
  Every batch's records are decoded and accumulated into ONE `records`
  Vec that grows across the ENTIRE stream, THEN the whole thing is
  passed to `create_index_from_records` in one call. For a 10M-row
  table, this means the FULL table sits in memory as decoded
  `InnerValue`s (the most expensive representation) at once — exactly
  the opposite of what the doc-comment claims. The stale comment
  itself should be corrected too (this is both a #482-style stale-
  comment issue AND the actual perf bug — fix the CODE, and correct
  the comment to describe the fixed behavior, do not just fix the
  comment to match the old code).

### Fix — 2.4

1. Change `create_index` to build/flush the index INCREMENTALLY,
   batch-by-batch, instead of accumulating everything first. Per the
   audit's fix sketch: for EACH batch yielded by the stream, build that
   batch's posting-writes and flush them via `set_many` (or whatever
   the existing per-batch write primitive in `create_index_from_records`
   is), rather than waiting for the whole table.
2. Investigate whether `create_index_from_records`'s existing signature
   (which takes a `Vec<(RecordId, InnerValue)>` per the current
   call site) needs to change to accept a stream/batch-at-a-time
   interface instead, or whether `create_index` should just call a
   PER-BATCH variant of the index-building logic directly (inlining
   the equivalent of `create_index_from_records`'s body per batch
   instead of delegating to one big-batch call). Check whether
   `create_index_from_records` is called from OTHER places too (grep
   the workspace) — if so, avoid breaking those other callers; either
   add a new incremental variant alongside the existing one, or adapt
   the existing one to accept an async batch stream if that's cleanly
   achievable.
3. Per the audit's SECONDARY suggestion: `build_index_key_from_record`
   works off a `RecordRef` — investigate whether decoding into a FULL
   `InnerValue` per record is even necessary, or whether a zero-copy
   `RecordView` (if one exists in this codebase — grep for it) could
   extract just the indexed field's value without a full decode. This
   is a nice-to-have on top of the incremental-batching fix; attempt it
   if straightforward, but the PRIMARY fix (incremental batching,
   bounding memory to O(batch) instead of O(table)) is the required
   deliverable — do not skip that for the sake of also chasing the
   zero-copy-decode nice-to-have.
4. Correct the stale doc-comment to accurately describe the NEW
   (genuinely incremental) behavior.

## Finding 3.3 — fjall per-op `spawn_blocking` overhead (MED-HIGH complexity — investigate, likely scope down)

- `crates/shamir-storage/src/storage_fjall.rs`: every `get`/`set`/
  `remove` wraps its body in a separate `task::spawn_blocking` call —
  a pool dispatch + task migration (~1-5µs per op) plus loss of
  locality. Batch APIs (`get_many`, `transact`) exist but even
  `get_many`'s INTERNAL implementation does sequential point-gets
  inside ONE `spawn_blocking`, not a genuinely batched fjall
  operation.

### Fix — 3.3 (investigate; scope down per audit's own "средняя-высокая" complexity if not cleanly achievable)

1. Investigate the audit's two suggested approaches:
   - (a) For read-mostly workloads: check whether fjall 3.x has any
     NON-blocking read path when data is resident in the memtable/
     block-cache (i.e., does fjall expose a sync, non-blocking `get`
     that's safe to call from an async context WITHOUT spawn_blocking
     when it's guaranteed not to touch disk, or is EVERY fjall read
     potentially blocking regardless of cache state, making
     spawn_blocking unconditionally necessary for correctness)? This
     requires reading fjall's actual API/docs — do not guess.
   - (b) A sharded worker-loop with MPSC batching of point-ops
     (amortizing the spawn_blocking dispatch cost across a batch of
     queued point-ops) — this is a more invasive architectural change
     (a background worker pool + channel-based dispatch) that would
     touch `FjallStore`'s internal execution model significantly.
2. **Scope-down escape valve**: per the audit's own complexity rating
   ("средняя-высокая", medium-high — the HIGHEST complexity rating
   among findings covered by tasks #486-490 except the explicitly-
   deferred structural ones), if investigation reveals fjall has NO
   safe non-blocking read path (making option (a) infeasible) and
   option (b)'s worker-loop rearchitecture is too large a change for
   a single surgical PERF task, **STOP and defer this finding
   entirely**. Document: what you found about fjall's actual
   blocking-read semantics, and a follow-up task description for the
   MPSC-batching worker-loop approach if that's the recommended path
   forward. This mirrors the successful deferral pattern from tasks
   #488 (3.2) and #489 (2.1/2.3) in this campaign.
3. If a genuinely safe, LOW-RISK partial win is found (e.g., fjall
   DOES have a documented non-blocking fast-path for warm-cache reads
   that's safe to call directly), implement just that narrow win and
   report it — do not attempt the full worker-loop rearchitecture
   unless it's clearly tractable within this task's scope.

## Finding 3.4 — TCP framing extra memcpy (LOW complexity, fix fully)

- `crates/shamir-transport-tcp/src/framing.rs`, `write_frame_into`
  (~line 190-205, confirm current lines):
  ```rust
  let len = payload.len() as u32;
  scratch.clear();
  scratch.reserve(4 + payload.len());
  scratch.extend_from_slice(&len.to_be_bytes());
  scratch.extend_from_slice(payload);
  writer.write_all(scratch).await?;
  writer.flush().await?;
  ```
  This copies the ENTIRE `payload` slice into `scratch` just to
  prepend a 4-byte length prefix before ONE `write_all` call — an
  extra full-response memcpy. For large SELECT responses (megabytes),
  this is a real, measurable cost.

### Fix — 3.4

1. Per the audit's fix sketch: avoid the second copy by reserving the
   4-byte length prefix SPACE directly in the buffer that the response
   is ALREADY being serialized into (upstream of `write_frame_into` —
   find the actual call site, likely in a request-handling loop in
   `shamir-server` or wherever responses are msgpack-encoded before
   being framed), i.e.:
   - Before msgpack-encoding the response, push 4 placeholder zero
     bytes (`buf.extend_from_slice(&[0u8; 4])`) into the SAME buffer
     that will hold the encoded response.
   - Encode the response DIRECTLY into that buffer (appending after
     the 4 placeholder bytes) — no separate payload buffer needed.
   - After encoding, PATCH the first 4 bytes in-place with the actual
     length (`buf[0..4].copy_from_slice(&(actual_len as
     u32).to_be_bytes())`, where `actual_len = buf.len() - 4`).
   - Call `writer.write_all(&buf)` directly — ONE write, no
     `write_frame_into`-style second copy.
2. This requires changing the CALLER of `write_frame_into` (the
   response-serialization path), not necessarily `framing.rs` itself —
   investigate where responses are actually built (grep for
   `write_frame_into`'s callers) and figure out whether the
   msgpack-encoding call site can be restructured to write directly
   into a length-prefixed buffer, OR whether a new framing helper
   function (e.g. `write_frame_prereserved` or similar) needs to be
   added to `framing.rs` that accepts an already-length-prefixed
   buffer and just does the write (skipping the internal copy
   `write_frame_into` currently does). Choose whichever is cleaner
   given the actual call site's structure; report your choice.
3. Confirm this doesn't break framing correctness for ANY existing
   caller of `write_frame_into` that ISN'T the large-response path
   (e.g., small control messages, close signals) — if `write_frame_into`
   itself is changed, every caller must still produce correctly-framed
   output; if a NEW helper is added instead, `write_frame_into` can stay
   unchanged for callers that don't need the optimization.

## Performance verification requirement (MANDATORY for whichever findings are fixed — this is a PERF task)

Per this repo's `/opti` methodology and this campaign's established
precedent:
1. For 2.4: bench `create_index` on a table with, e.g., 100k-1M rows,
   measuring PEAK MEMORY (not just wall-clock) before/after — the
   audit's complaint is about memory (O(table) → O(batch)), so a
   memory-focused measurement (or at minimum, a proxy like "does the
   fixed version's memory stay flat as table size grows" via a
   parameterized bench at increasing N) matters as much as wall-clock
   here. If a memory-measurement harness isn't readily available in
   this repo's bench tooling, at minimum report qualitatively (e.g. via
   `/usr/bin/time -v` or platform equivalent, or a manual
   allocation-count assertion) that the fix bounds memory to O(batch).
2. For 3.3 (if fixed, even partially): bench point-op latency
   (get/set/remove) before/after.
3. For 3.4: bench `write_frame_into`'s (or the new helper's) cost on a
   LARGE payload (e.g. 1-10MB, simulating a big SELECT response)
   before/after — the memcpy elimination should show a clear,
   payload-size-proportional win.
4. Follow this repo's `bench-scale-tool::Harness` convention (check
   `crates/shamir-index/benches/`, `crates/shamir-storage/benches/`,
   `crates/shamir-transport-tcp/benches/` for existing patterns, or the
   benches added by tasks #486-489 for the general shape).
5. Report exact baseline vs. after numbers with speedup ratios,
   honestly, per this campaign's established precedent (report flat/
   no-improvement results honestly with root-cause analysis, matching
   tasks #486/#487/#489).

## TDD/regression requirement

1. For 2.4: confirm CREATE INDEX still produces an IDENTICAL index
   (same postings) before/after — this must be a pure memory/
   incrementality optimization, not a behavior change.
2. For 3.3 (if fixed): confirm point-op correctness (get/set/remove
   semantics) is unchanged.
3. For 3.4: confirm framed output is byte-identical before/after (the
   length prefix + payload bytes on the wire must be EXACTLY the same,
   just constructed with fewer copies) — add or extend a round-trip
   test (encode via the new path, decode via the existing frame-reader,
   confirm the payload matches).

## Test scope command

```
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-storage
./scripts/test.sh -p shamir-transport-tcp
./scripts/test.sh -p shamir-server
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-index -p shamir-storage -p shamir-transport-tcp -p shamir-server -- --check
cargo clippy -p shamir-index -p shamir-storage -p shamir-transport-tcp -p shamir-server --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly, for EACH of the three findings:
```
[Finding 2.4] Status: fixed / deferred
  > Baseline / After / Δ (memory + wall-clock, if fixed)
  > OR: deferral reason + follow-up description (if deferred)

[Finding 3.3] Status: fixed / partially-fixed / deferred
  > Baseline / After / Δ (if fixed)
  > OR: deferral reason + follow-up description (if deferred)

[Finding 3.4] Status: fixed / deferred
  > Baseline / After / Δ (if fixed)
  > OR: deferral reason + follow-up description (if deferred)
```
- Full test/gate results (exact commands + pass/fail) for whichever
  crates were actually touched.
- Confirmation of byte-identical wire framing for 3.4 if fixed.
- Confirmation of identical index-build results for 2.4 if fixed.
