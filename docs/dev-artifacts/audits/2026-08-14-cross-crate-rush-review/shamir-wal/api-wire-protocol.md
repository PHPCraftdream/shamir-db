# shamir-wal — API & wire-protocol design

## Summary

The crate's versioned surfaces are genuinely well-engineered: the V2 entry envelope (`[magic][version][bincode]`) decodes both v1-legacy and v2 bodies with tests pinning each, the `.meta` sidecar has a documented fallback matrix (absent/corrupt/torn → replay) with a test per case, and the frame CRC / torn-tail / sealed-vs-active replay semantics are thoroughly covered. The weaknesses are concentrated in the unversioned and retired parts of the surface: segment-name parsing silently accepts non-canonical names (shadowing data), the append path accepts payloads that later brick replay, a serialized field (`idx_id`) is written as a constant with "deferred" semantics, and the public API still exports the retired F5c KV-marker wire protocol with docs that describe it as live. The builder-only query-construction rule is trivially compliant: no `serde_json` anywhere in the crate; WAL entries are typed structs (`WalEntryV2`/`WalOpV2`) assembled with builder-style `with_commit_version`.

## Findings

### 1. Segment-name parser accepts non-canonical names and can silently shadow WAL data
- **File:** `crates/shamir-wal/src/segment_set.rs:74-77` (`parse_seg_seq`), writer at `:68-70` (`seg_file_name`)
- **Severity:** medium
- **Issue:** `seg_file_name` writes the canonical `NNNNNNNN.wal` (zero-padded 8), but `parse_seg_seq` accepts *any* numeric stem (`"5.wal"`, `"0000001.wal"`, 9+ digits) and canonicalizes the seq back to the 8-digit path. The directory listing is this store's "wire", and the parser is more lenient than the writer.
- **Failure scenario:** a foreign or legacy file `5.wal` next to a real `00000005.wal`: both parse to seq 5, `seqs = [5, 5]`; one becomes "sealed" and the other "active", but *both* `SealedMeta.path` and the active path resolve to `00000005.wal`. `5.wal` is silently never replayed, never truncated, never mentioned — if it held un-drained commits, that is silent data loss at recovery.
- **Suggested fix:** in `parse_seg_seq`, require the canonical form exactly (stem length == 8, all digits, suffix `.wal`); treat non-canonical `.wal` names as a loud `open` error (or at minimum skip-with-log), and dedupe/reject duplicate seqs from the scan.

### 2. Append path accepts payloads that produce well-formed frames which then hard-fail replay
- **File:** `crates/shamir-wal/src/wal_segment.rs:195-234` (`append_batch` writes any `Vec<u8>` incl. empty), `:572` (`WalEntryV2::decode(payload)?` in `replay_inner`), `:377-394` (`repair_torn_tail` keeps the frame)
- **Severity:** medium
- **Issue:** the sink layer is byte-opaque by design, but nothing validates that a payload is a decodable entry. A zero-length payload is the sharp edge: its frame `[len=0][crc=0]` is *well-formed* (CRC32 of empty input is 0), so `repair_torn_tail` keeps it and every replay mode reaches `WalEntryV2::decode(&[])`, whose error propagates via `?` as a **hard error even in the tolerant active-segment/startup path**.
- **Failure scenario:** any caller of the public `WalSink::append_batch` / `WalGroupCommit::append` passing an empty (or truncated) `Vec<u8>` — the only production caller encodes a real `WalEntryV2` today, so this needs API misuse, but the API invites it — writes a frame that makes `SegmentSet::replay` → `replay_at_startup` return `Err`, so `RepoWalManager::recover()` fails on every subsequent open. Permanently, until manual repair; the "torn tail is discarded" contract does not cover it.
- **Suggested fix:** reject empty payloads at `WalGroupCommit::append`/`append_many` and `WalSink::append_batch` (`Err` before touching `next_seq`/file), and in `replay_inner` decide decode-failure policy per mode explicitly (e.g. treat as corrupt frame: break for active, loud `Err` for sealed) instead of an unconditional `?`.

### 3. Retired F5c KV-marker wire protocol still exported as public API, with docs describing it as live
- **File:** `crates/shamir-wal/src/lib.rs:54` (`pub use active_key::WalActiveKey`), `src/wal_entry_v2.rs:1-16` (module doc), `:259-264` (`looks_like_v2`), `src/active_key.rs` (whole file), `src/wal_segment.rs:3` (`[`crate::WalManager`]`)
- **Severity:** medium
- **Issue:** `lib.rs`'s own architecture doc says the F5c/F6 cutover retired the KV-marker design ("production no longer uses such markers"), and the engine removed `shamir_wal::WalManager` + the V1 codec in F5c (`shamir-engine/src/table/table_manager_crud.rs:359`). Yet the crate still exports `WalActiveKey` and `WalEntryV2::looks_like_v2` — a workspace-wide grep shows **zero code consumers** outside their own tests (only comments). Meanwhile `wal_entry_v2.rs`'s module doc asserts "Coexists with the V1 [`super::wal_entry::WalEntry`]" (broken intra-doc link — no `wal_entry` module exists in this crate) and "Both V1 and V2 entries live under the same `WalActiveKey` prefix in info_store; recovery distinguishes them by sniffing the magic prefix (stage 0.8 will wire this)" — a description of a wire protocol that no longer exists. `wal_segment.rs:3` references `[`crate::WalManager`]` (also a broken link), and `looks_like_v2`'s doc cites the removed `WalManager` as its user. `looks_like_v2_sniff` in the test suite even asserts on hypothetical V1 bincode bytes.
- **Failure scenario:** a consumer reads the crate docs, concludes V1 entries may be present under `__wal_active_` keys, and builds dispatch/repair tooling around `looks_like_v2`/`WalActiveKey` — dead code paths for a format that can no longer occur. (No runtime failure; this is public-surface misinformation on a durability component.)
- **Suggested fix:** delete `active_key.rs` + its tests and `looks_like_v2` + its test (or at minimum `#[deprecated]` + `#[doc(hidden)]` with a pointer to the F5c cutover), and rewrite `wal_entry_v2.rs`'s module doc to the segment-store reality; fix the two broken intra-doc links.

### 4. `WalOpV2::IndexPut`/`IndexDel` serialize `idx_id` as a constant 0 with semantics "deferred"
- **File:** `crates/shamir-wal/src/wal_entry_v2.rs:69-100` (field + invariant doc); sole producer `shamir-engine/src/tx/commit.rs:320-330` hardcodes `idx_id: 0`
- **Severity:** medium
- **Issue:** a wire-format field is written but meaningless — the producer always emits 0, consumers must decode the real index id from the `key` byte prefix (`[idx_id_be: 4][rest]`), and the doc says the reconciliation decision (thread it through vs. keep 0 forever) is "deferred to the recovery implementation". Cross-crate confirmation: `shamir-tx/src/index_write_op.rs:58` calls it "currently-unpopulated … for a FUTURE wire-level identity scheme".
- **Failure scenario:** whichever way the deferral lands, the on-disk corpus is locked in: if real ids start being emitted later, every future reader must forever special-case `idx_id == 0` as "decode from key prefix" — and a table that legitimately has index id 0 is indistinguishable from the legacy encoding. A new consumer trusting the field (it *looks* populated in the schema) misroutes postings.
- **Suggested fix:** resolve the decision now while the corpus is young: either remove `idx_id` from the wire struct (bump `WAL_V2_VERSION` to 3 with a legacy decode path, mirroring the existing v1 pattern), or thread real ids through and document `0`-means-prefix-decode as a permanent invariant in both this doc and the recovery code.

### 5. Frame format has no per-frame magic/seq and segment files have no header or format version
- **File:** `crates/shamir-wal/src/wal_segment.rs:228-234` (frame layout `[u32 len LE][payload][u32 crc32 LE]`), `:546-563` (the in-code acknowledgment: "no magic/seq for resync … deferred follow-up … requires a WAL format version bump")
- **Severity:** low (acknowledged, documented debt — recorded here because it shapes the whole recovery API)
- **Issue:** the `.wal` file is a bare frame stream: no file magic, no format version (the 17-byte `.meta` sidecar has both, the segment itself has neither), no per-frame sequence. Consequences already visible in the API: a single corrupt frame mid-*active* segment silently discards the entire valid tail (`replay` warn+break), while the same corruption in a sealed segment is a hard operator error (`replay_sealed`) — the format cannot resync, so these coarse policies are forced.
- **Suggested fix:** when the deferred version bump happens anyway (see finding 4), add a small segment header (`[magic "WSEG"][version]`) and per-frame `[seq]` so (a) future layout changes are detectable, (b) single-frame-skip resync becomes possible, narrowing the silent-tail-loss window.

### 6. `append_batch` returns a "seq" that is per-segment, non-persisted, and resets to 0 on every open
- **File:** `crates/shamir-wal/src/wal_segment.rs:177` (`next_seq: AtomicU64::new(0)` even when reopening a non-empty file), `:212-213`; surfaced publicly at `segment_set.rs:222` ("Returns the seq assigned to the last entry") and `wal_sink.rs:109`
- **Severity:** low
- **Issue:** the `u64` return value is: relative to the current segment only, never written to disk, restarted at 0 by every `WalSegment::open` (fresh or reopened), and — per workspace grep — consumed by nothing in production (`WalGroupCommit::lead_until_drained` discards it via `.is_ok()`; only tests read it, e.g. `wal_segment_tests.rs:42`). The docs read like a global sequence, inviting LSN-style misuse.
- **Suggested fix:** either drop the return value from the public signatures (`-> DbResult<()>`) or make it a real durable global sequence (persisted in the frame per finding 5); at minimum document that it is a per-segment, per-open in-memory counter.

### 7. No library error enum: all wire failures collapse into `DbError::Internal(String)` / `DbError::Storage(String)`
- **File:** `crates/shamir-wal/src/wal_entry_v2.rs:219-256` (encode/decode), plus 41 `DbError::` sites crate-wide (see grep in review)
- **Severity:** low
- **Issue:** CLAUDE.md's error-handling rule calls for `thiserror` library error enums; this crate instead reuses `shamir_storage::error::DbError` string variants for everything. Consequences at the API level: "bad magic", "unsupported version", "corrupt bincode body", "spawn_blocking join" and "ENOSPC on write" are all indistinguishable programmatically — a caller cannot branch on *corrupt WAL* vs *transient I/O*, and message-matching on strings (as `wal_segment_tests.rs:184-187` already does: `err_msg.contains("CRC mismatch")`) becomes the de-facto contract.
- **Suggested fix:** introduce a `thiserror` `WalError` (variants `BadMagic`, `UnsupportedVersion { got }`, `Decode { source }`, `Io { path, source }`, …) converted into `DbError` at the engine boundary; keep `DbResult` as the return alias if the workspace Result type must be preserved.

### 8. Group-commit waiter transport discards the underlying error
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:89-108` (`Waiter` carries only `done: AtomicBool` + `ok: AtomicBool`), `:198-202` / `:258-262` (return `DbError::Storage("wal group commit failed")` / `"... batch failed")`)
- **Severity:** low
- **Issue:** the leader observes the real `DbError` from `sink.append_batch`/`sync` but the waiter protocol can only carry a bool, so `append`/`append_many` return a causeless generic error. The root cause is only recoverable from `log::error!` output (the segment code does log it), not from the API. Compounds finding 7: the engine commit path surfaces "wal group commit failed" to callers with no ENOSPC/EIO detail.
- **Suggested fix:** extend `Waiter` with a cheap error slot (e.g. `Mutex<Option<DbError>>` or an `ArcSwapOption<DbError>` set before `notify_one`); leaders store the first error, waiters clone it into the returned `Err`.

### 9. Frame-length arithmetic can overflow `usize` on 32-bit / wasm32 targets
- **File:** `crates/shamir-wal/src/wal_segment.rs:377-380` (`repair_torn_tail`) and `:532-535` (`replay_inner`): `let frame_end = pos + 4 + len + 4;` where `len` is an untrusted `u32` read from disk
- **Severity:** low
- **Issue:** on a 64-bit host this is safe (length bounded, `frame_end > buf.len()` breaks the loop), but on a 32-bit `usize` target (wasm32 — and CLAUDE.md declares the project "WASM-first") a corrupt length near `u32::MAX` makes `pos + 4 + len + 4` wrap (release) or panic on overflow (debug), after which the subsequent `buf[pos + 4..pos + 4 + len]` slice can panic on out-of-range bounds — a corrupt file becomes a crash instead of a controlled `break`/`Err`.
- **Suggested fix:** compute the frame end with checked arithmetic (`pos.checked_add(4)?.checked_add(len)?.checked_add(4)`) and treat overflow exactly like a torn tail.

### 10. `WalSegment::mark_poisoned` is an un-gated public test hook
- **File:** `crates/shamir-wal/src/wal_segment.rs:339-344`
- **Severity:** nit
- **Issue:** the doc says "exposed (pub(crate)-ish) for tests", but it is fully `pub` in the public API, unlike every other test hook in the crate which is properly `#[cfg(test)] pub(crate)` (`SegmentSet::active_segment_for_test`, `WalSink::arm_fail_next_append`, `WalGroupCommit::{fsync_count,is_dirty,set_dirty}`). Any downstream user can silently quarantine a production segment.
- **Suggested fix:** gate it `#[cfg(test)] pub(crate)` (the poison tests live in this crate's `tests/`), or at least `#[doc(hidden)]` with the fault-injection rationale.

### 11. Wire-format tests for `segment_meta` are inline in the implementation file
- **File:** `crates/shamir-wal/src/segment_meta.rs:175-218` (`#[cfg(test)] mod tests { ... }`)
- **Severity:** low (test-organization rule; noted here because the subject under test is the sidecar wire format — flagging for whichever sibling review owns test layout)
- **Issue:** CLAUDE.md mandates "Never embed `#[cfg(test)] mod tests { ... }` inline inside implementation files. Move them to the `tests/` directory." This is the only violation in the crate — every other module correctly uses `src/tests/` with a manifest-only `tests/mod.rs`.
- **Suggested fix:** move the four encode/decode tests to `src/tests/segment_meta_tests.rs` and add the module to `tests/mod.rs`.

### 12. Anonymous tuple types in the public wire structs
- **File:** `crates/shamir-wal/src/wal_entry_v2.rs:105` (`InternerOverlayMerge { entries: Vec<(u64, String)> }`), `:159` (`interner_delta: Vec<(u64, String, u64)>`)
- **Severity:** nit
- **Issue:** the positional triples `(table_token, field_name, intern_id)` and pairs `(id, name)` are part of the serialized schema and public API but carry meaning only via doc comments; `.0/.1/.2` access at every consumer is a standing misindexing hazard, and adding a field later forces a wire-breaking tuple→struct change anyway.
- **Suggested fix:** introduce tiny named structs (`InternerDeltaEntry { table_token, field_name, intern_id }`) at the next version bump, mirroring the v1→v2 legacy-decode pattern for old corpora.

---

**Test-coverage note (context for the findings above):** wire-format coverage is a strength — `wal_entry_v2_tests.rs` pins the envelope (magic, version byte, short/bad-magic/unknown-version rejection, v2 round-trip with delta, v1-legacy decode, size bound); `wal_segment_tests.rs` pins frame semantics (round-trip, torn tail, CRC detection, sealed-loud vs active-tolerant, per-segment seq); `segment_set_tests.rs` covers the full sidecar matrix (seal-writes, valid-skips-replay, absent/corrupt/truncated → replay fallback, crash-between-fsync-and-sidecar, truncate-removes, reactivation/poison sheds stale sidecar); `wal_group_commit_tests.rs` covers the `WalDurability` tier contract including `append_many` atomicity under injected write failure. The gaps that matter for this theme are exactly findings 1–2: no test for non-canonical segment names, and no test for an empty/undecodable payload entering the frame stream.
