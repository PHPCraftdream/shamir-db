# shamir-wal — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-wal/` — the write-ahead log: group-commit coordinator
(`WalGroupCommit`), segmented file store (`SegmentSet`/`WalSegment`), the V2
entry envelope (`WalEntryV2`/`WalOpV2`), the `.meta` seal sidecar
(`segment_meta`), and the sink abstraction with its in-memory variant
(`WalSink`/`MemSink`).

Review basis: the seven 2026-08-14 lens files under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`,
`error-handling-lifecycle.md`, `style-claude-md.md` — synthesized into one
document. Structure/tone/rigor calibrated against the two completed exemplars
(`../shamir-client-node/SUMMARY.md`, `../shamir-transport-ipc/SUMMARY.md`);
the workspace-wide `../SUMMARY.md` per-crate row and health-scorecard entry
for shamir-wal were consulted for the dedup convention only (its numbers are
pre-dedup lens-tagged counts). A handful of load-bearing file:line refs were
re-verified against source during synthesis (breaker exit, success exit,
CAS/park loop, `next_seq`, `parse_seg_seq`, `seal_and_rotate` in the Ok arm,
`lib.rs` exports, `segment_meta` inline tests — all confirmed as cited).
Read-only synthesis — no build/test/lint commands, no source modifications.

## Executive summary

The crate's design story is unusually strong — group commit genuinely
amortizes N committers into one `write()` + at most one `fsync()`, the #500
sidecar and its fallback matrix are a model of tested recovery engineering,
and the lock discipline matches every CLAUDE.md adjudication — but the crate
is **not shippable as-is: its worst defects are hang-class liveness failures
on the durability spine**, exactly the "hangs are bugs" class the repo
designates P0. Three things to fix before anything else ships from this
crate: (1) the group-commit **circuit-breaker exit strands parked committers
forever** after any write/fsync failure (a quiescent post-ENOSPC system hangs
the commit path); (2) **leader cancellation/panic wedges `flushing` forever**
(no RAII release on the inline leader task); (3) a **seal-time fsync failure
reports an already-written batch as failed while its frames survive to
replay** — "acked-failed" transactions resurrecting on restart, violating the
crate's own §1.6 all-or-nothing contract. After the liveness trio, the theme
is *error-path fidelity and coverage*: causes dropped to booleans, no
`thiserror` taxonomy, and the audit-closed §1.5/§2.4/§1.3-file fixes guarded
by vacuous or absent tests.

---

## 1. correctness-tdd

### 1.1 — high — Circuit breaker can strand parked appenders indefinitely (L1 violation, hang)
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:327-330` (breaker exit;
  re-verified), park loop `:189-197`, push+CAS `:176-186`, success exit
  `:272-277`; also flagged as **concurrency-lockfree #1** (deduped here).
- Issue: `lead_until_drained` implements the documented L1 pattern
  ("leadership is released under the SAME `pending` lock as `push` … no entry
  is ever stranded") only on the SUCCESS exit (`:272-277`: empty-check +
  `flushing.store(false)` inside the lock). The circuit-breaker exit (`:328`)
  does a plain `flushing.store(false)` with NO lock and NO re-check of
  `pending`, then returns. Any committer that pushed to `pending` during the
  leader's failed `append_batch`/`sync` await already lost its CAS (flushing
  was still true) and is parked on its `Waiter`; the leader returns without
  draining or failing that entry. Nobody will call `complete()` on it.
- Failure scenario: Tasks A and B commit concurrently. A wins the CAS and
  leads; B pushes during A's in-flight window and parks. The window's write
  (or fsync) fails (real ENOSPC / EIO; on the File sink this requires the
  rotate-and-retry-once at `segment_set.rs:265-270` to also fail). A's batch
  waiters get `ok=false`, then A hits the breaker and stores `flushing=false`.
  B is parked forever: no leader exists, but B is not appending — it is
  awaiting. On a quiescent system (exactly the post-disk-error state) B hangs
  indefinitely — a per-connection hang on the production commit path
  (`RepoWalManager::begin_grouped` → `WalGroupCommit::append`). B recovers
  only if some FUTURE append elects a new leader that drains B's stale entry,
  possibly long after B's caller timed out at a higher layer.
- Suggested fix: make the breaker exit symmetric with the success exit — under
  the `pending` lock, store `flushing=false` and `mem::take` any stragglers in
  the same critical section, then complete them with `ok=false` (surfacing the
  error beats hanging; a pusher arriving after the store sees
  `flushing==false` and leads itself). TDD (Red first): extend the `Mem` fault
  knob to fail ≥2 consecutive `append_batch`es, spawn A (leader, fails) and B
  (pushes behind it), assert B's `append` resolves (Err) inside
  `tokio::time::timeout` — pre-fix this test hangs/timeouts, post-fix it fails
  fast.

### 1.2 — medium — Vacuous §1.5 regression test: the restore-on-failed-fsync branch never executes
- File:line: `crates/shamir-wal/src/tests/wal_group_commit_tests.rs:294-332`
  (esp. `:316-331`); production branch `wal_group_commit.rs:394-403`; also
  flagged as **error-handling-lifecycle #7** (deduped here).
- Issue: `dirty_flag_restored_after_failed_fsync` claims to be the §1.5
  regression test, but its "failed sync" is performed by the test itself:
  `gc.set_dirty()` … `assert(gc.is_dirty())`. It re-implements the fix in the
  test body and asserts its own re-implementation. Deleting the restore at
  `wal_group_commit.rs:398` leaves this test green — there was never a Red,
  because the only fault knob (`arm_fail_next_append`) arms `append_batch`,
  not `sync`, and `sync_now` cannot be made to fail on either sink.
- Failure scenario: a refactor of `spawn_background_fsync` (early return on
  `Err`, or `take_dirty` moved after a successful-only sync) silently loses
  the unbounded data-at-risk retry that §1.5 closed; the suite stays green.
- Suggested fix: add a `#[cfg(test)] fail_next_sync: AtomicBool` knob to
  `WalSink` (mirroring `arm_fail_next_append`), arm it, run
  `spawn_background_fsync` with a short interval, assert the dirty flag is
  RE-set after the tick, then disarm and assert it clears on the next
  successful tick. Keep the existing happy-path assertions; delete the
  self-referential "simulate the error branch" lines.

### 1.3 — medium — §2.4 startup `PermissionDenied` hard-fail branch has zero coverage
- File:line: `crates/shamir-wal/src/wal_segment.rs:512-524`; placeholder test
  `src/tests/wal_segment_tests.rs:217-235`; also flagged as
  **error-handling-lifecycle #8 item 3** (deduped here).
- Issue: the audit-§2.4 fix (a `PermissionDenied` at startup must be a HARD
  error, not a silent `Ok(vec![])` that skips durable records) exists only as
  the `tolerate_permission_denied == false` arm of `replay_inner`. The only
  test touching the `_at_startup` variants exercises the healthy path; its own
  doc concedes "the test validates the API surface exists". Flipping the flag
  back to `true` at the startup call sites passes the suite.
- Failure scenario: a future edit reuses the tolerant `replay()`/
  `replay_sealed()` variants "for consistency"; an antivirus/ACL-held segment
  silently replays as empty at boot and recovery skips durable commits — the
  exact silent-data-skip §2.4 was filed against, with no test failure.
- Suggested fix: factor the open-error classification out of `replay_inner`
  into a pure helper (`classify_open_err(e, tolerate_perm_denied) ->
  OpenOutcome { NotFound, ToleratedDenied, HardDenied, Fatal }`) and unit-test
  all four kinds against all flag combinations (no filesystem needed);
  optionally a `#[cfg(unix)]` chmod-000 test for the real arm.

### 1.4 — medium — `repair_torn_tail` silently amputates the valid suffix on a complete-but-CRC-bad frame, mislabeled "torn tail"
- File:line: `crates/shamir-wal/src/wal_segment.rs:377-395` (break conditions),
  `:399-411` (truncate), `:419-423` (warn); contrast `replay_inner`'s
  sealed-CRC loud error at `:546-563`; also flagged as **security-crypto #1**
  (deduped here).
- Issue: the repair loop breaks identically on (a) an INCOMPLETE trailing
  frame (`frame_end > buf.len()` — a genuine crash tail; silent truncate is
  the design) and (b) a COMPLETE frame whose CRC mismatches (sub-frame-torn
  page-cache write or disk bit-rot). In case (b) it still truncates the file
  at that boundary at open time — permanently destroying every valid frame
  after it — and logs only a `warn` worded "truncated torn tail". This is the
  same bytewise condition that audit §1.8 made a LOUD operator-facing error
  for sealed segments; for the active segment (which can hold fsync-acked
  level-3 commits) it is silent, permanent, and mislabeled.
- Failure scenario: bit-rot flips one payload byte in the middle of an active
  segment. On next open, `repair_torn_tail` deletes the entire valid suffix
  (potentially many acked commits) before `replay` ever runs. No ERROR-level
  signal reaches the operator; the warn implies an expected crash artifact.
- Suggested fix: track WHY the loop broke. For the CRC-mismatch case keep the
  truncate (append-path correctness requires it — appends after a corrupt
  frame would be stranded on replay) but log at `error!` with "CRC mismatch
  (complete frame) — on-disk corruption in ACTIVE segment; valid suffix of N
  bytes truncated". The security reviewer went further (stop without mutating
  the file, mirroring `replay_sealed`); that conflicts with the append-stranding
  constraint above — if adopted it must also gate subsequent appends. Either
  way, add a regression test for the mid-file-CRC-mismatch case (current tests
  cover only a genuine partial tail and the clean no-op:
  `wal_segment_poison_tests.rs:48,124`).

### 1.5 — medium — §1.3 file-sink mid-flight failure → rollback → retry-once path untested; the whole File-sink error surface has no fault knob
- File:line: `crates/shamir-wal/src/segment_set.rs:255-274` (Err branch;
  contrast the tested pre-poisoned branch `:232-236`); related untested
  invariant `wal_segment.rs:214`; also flagged as
  **error-handling-lifecycle #8 items 1, 2, 4** (deduped here; that finding
  enumerates four paths).
- Issue: every File-sink poison test drives the PRE-poisoned branch via
  `mark_poisoned()`. Never executed by any test: (1) a live append FAILING
  mid-flight — the segment self-poisoning, rolling back via
  `set_len(pre_batch_offset)`, `rotate_after_poison`, and the batch being
  retried ONCE on the fresh file (no knob can fail a real `write_all`);
  (2) `rotate_after_poison` when opening the next segment fails
  (`segment_set.rs:317` — "poisoned segment stays active, every append fails
  fast" asserted nowhere); (3) sealed-CRC-as-loud-error composed through
  `SegmentSet::open`'s no-sidecar fallback (`segment_set.rs:154-156`); plus
  the untested consequences: `max_committed` being `fetch_max`-ed at
  `wal_segment.rs:214` BEFORE the write (sealed `SealedMeta.max_version` can
  exceed actual content — conservative over-retention, undocumented), and the
  retried `last_seq` referring to the NEW segment.
- Failure scenario: a regression in the rollback offset (truncate to 0, or to
  `bytes_written` after the failed `fetch_add`) or in the retry's seq/version
  accounting lands undetected; §1.3's fix claim rests on code reading alone.
- Suggested fix: add a test-only fault seam on the File path
  (`#[cfg(test)] fail_next_write/fail_next_sync` checked inside the
  `spawn_blocking` closure, mirroring the Mem knob) and a Red/Green test:
  first append fails (poison + rollback), retry lands on the fresh segment,
  replay shows exactly the retried frames, and the sealed poisoned segment's
  `max_committed` equals its actual content's max (or document the
  overstatement as intentional). Cover paths 2–3 with the same seam.

### 1.6 — low — *(primary: 5.6)* — per-segment seq counter restarts at 0 on reopen
- (Full write-up at 5.6; correctness #6 supplied the reopen-collision framing:
  reopened segments hand out seqs 0..k-1 again, aliasing pre-existing frames'
  seqs — inconsistent with `bytes_written` being seeded from disk at
  `wal_segment.rs:179`.)

### 1.7 — low — *(primary: 6.3)* — leader discards the underlying I/O error; a failed retry is logged nowhere
- (Full write-up at 6.3; correctness #7 adds the detail that
  `SegmentSet::append_batch`'s RETRY failure (`segment_set.rs:270`) propagates
  unlogged into the leader's boolean reduction — in the retry-failure case no
  log line anywhere carries the actual OS error.)

### 1.8 — low — *(primary: 3.4)* — corrupt length header overflows `usize` on 32-bit targets → panic in replay/repair
- (Full write-up at 3.4; correctness #8's framing: on 32-bit, release wraps /
  debug panics on overflow, and the subsequent slice indexes out of bounds —
  a corrupt-file-triggered `panic!`, which CLAUDE.md reserves for invariant
  violations. 64-bit builds are safe.)

### 1.9 — low — *(primary: 7.1)* — inline `#[cfg(test)] mod tests` in `segment_meta.rs`
- (Full write-up at 7.1; rated low by this lens, high by style, low by api —
  see the severity note there.)

### 1.10 — nit — `sync_now` counts failed fsyncs as issued fsyncs
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:355-362` (incremented
  at `:360` regardless of `res`).
- Issue: `fsync_count` is documented as "count of fsyncs this coordinator
  issued (for the batching test)" but increments even when the fsync fails, so
  batching assertions conflate attempts with successes. Harmless today (tests
  need only ≥ thresholds); a future strict-equality assertion would be wrong
  on failure paths.
- Suggested fix: increment only on `res.is_ok()`, or document the counter as
  "attempts".

## 2. concurrency-lockfree

The crate's three locks are each inline-justified and match CLAUDE.md's closed
adjudications (`SegmentSet::inner` per #1095/#1109, `WalSegment::file` locked
only inside `spawn_blocking`, `WalGroupCommit::pending` as the sanctioned async
exception). No lock is ever held across an `.await`; no `parking_lot`/`RwLock`;
atomics are ordered correctly; pillars 4/5 are N/A (no hash-keyed structures).
The one substantive defect is the liveness hole deduped at 1.1.

### 2.1 — high — *(primary: 1.1)* — circuit-breaker exit strands parked waiters indefinitely
- (Full write-up at 1.1. The concurrency lens adds: the existing failure test
  `append_many_write_failure_leaves_zero_entries`,
  `wal_group_commit_tests.rs:377-424`, is single-caller and does not cover the
  stranded-follower case; and the workspace-wide SUMMARY explicitly calls this
  hang out as appearing in 3 sibling themes — the sibling-but-distinct leader
  wedge is 6.1.)

### 2.2 — medium — `WalSegment::append_batch` rollback and `bytes_written` accounting are unsafe under concurrent callers, while the bench claims correctness coverage of exactly that shape
- File:line: `crates/shamir-wal/src/wal_segment.rs:213-220` (seq `fetch_add` +
  `pre_batch_offset` load before the blocking write), `:248-252`
  (`set_len(pre_batch_offset)` rollback), `:270` (`bytes_written.fetch_add`);
  bench `crates/shamir-wal/benches/segment_set_lock.rs:100-153, 235-308`; also
  flagged as **error-handling-lifecycle #6** (deduped here).
- Issue: `pre_batch_offset` is loaded *before* the `spawn_blocking` write, so
  under two concurrent appenders on one segment, B's failed `write_all` rolls
  the file back to an offset captured before A's concurrent successful write —
  `set_len` would truncate away A's already-acked frames. Safe today only
  because of the documented single-leader model (`wal_segment.rs:82-94`).
  However, `benches/segment_set_lock.rs` deliberately runs N concurrent
  `append_batch` callers against the shared active segment and labels
  `raw_append` a "lock CORRECTNESS stress test… proving `append_batch` stays
  correct under worst-case, all-N-contend concurrency" — a claim the bench
  never verifies: it injects no failures, so the racy rollback path is never
  exercised, and there is no post-run replay/CRC check.
- Failure scenario: only if the single-writer model is ever relaxed (or the
  bench is cited as evidence concurrent append is supported): a mid-bench
  ENOSPC under `sustained_append` would silently discard other tasks' acked
  frames.
- Suggested fix: either (a) document on `WalSegment::append_batch` that
  concurrent calls are correctness-unsupported and soften the bench's claim to
  "no-failure concurrency smoke", or (b) make the path actually safe — derive
  the rollback boundary inside the file-lock critical section on the blocking
  thread (maintain a last-good-offset updated only under `file`, or fall back
  to `metadata().len()` under the lock on the error path — one extra syscall
  on an already-failing path is free), keeping the lock held only there, never
  across `.await`. Severity note: the concurrency reviewer rated this low
  (guarded by the single-writer model); error-handling rated the same defect
  medium precisely because the repo's own committed bench violates that model
  — carried at medium.

### 2.3 — low — Leader tenure is unbounded under sustained arrival — the leader's own append future cannot resolve
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:269-331`.
- Issue: `lead_until_drained` returns only when it observes `pending` empty.
  Under sustained load (arrival rate ≥ service rate; fsync dominates a durable
  append ~63× per the crate's own measurement), every empty-check can see new
  entries, so the current leader is pinned indefinitely and its caller's
  commit future never resolves even though its own entry is physically
  durable. Degraded service time for one committer at a time, not a deadlock —
  hence low.
- Suggested fix: bound consecutive windows per tenure and hand leadership off
  — but only AFTER 1.1 is fixed, since a non-draining handoff would strand
  waiters exactly the same way (a handoff must either drain or complete the
  leftovers). Alternatively, once the leader's own waiter is `done`, release
  leadership under the `pending` lock with the same leftover-completion guard.

### 2.4 — nit — `dirty_since_sync` boolean has a clear-race against concurrent Buffered completions
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:299-301` (set per
  Buffered window), `:355-362` (`sync_now`: fsync then `store(false)`),
  `:386-410` (timer `take_dirty` → `sync_now`).
- Issue: the background timer's `sync_now` does `sink.sync().await` and only
  then `store(false)`; a Buffered window completing between the fsync's
  completion and the store (or between `take_dirty()` and the fsync) has its
  dirty flag wiped without its bytes being flushed. The timer's promised
  "bounded power-loss window" silently reopens until the next Buffered append
  re-sets the flag. The `Buffered` tier's page-cache-only ack contract is not
  violated — nit.
- Suggested fix: replace the boolean with an epoch pair: `write_epoch:
  AtomicU64` (`fetch_add` per Buffered window) and `synced_epoch: AtomicU64`
  (`fetch_max(epoch_at_fsync_start)` on successful fsync); dirty ⇔
  `write_epoch > synced_epoch`. Race-free without any lock.

### 2.5 — nit — `MemSink` holds a `std::sync::Mutex` across O(N) CPU work inside async fns
- File:line: `crates/shamir-wal/src/wal_sink.rs:161-169` (`replay` decodes
  every frame under the lock), `:185-191` (`truncate_below` `retain`),
  `:203-207` (`has_truncatable` scan).
- Issue: the `Mem` arm of `WalSink::replay` holds `frames` across
  bincode-decoding the entire WAL (O(total bytes)) inside an async fn. The
  lock is never held across an `.await` (compliant), but a concurrent append
  on a multi-threaded runtime blocks a worker thread for the whole replay.
  Rare paths (recovery / drainer gate) on the in-memory sink only, and the
  struct's inline comment sanctions the lock — nit.
- Suggested fix: in `replay`, clone (or `mem::take` into a fresh Vec) the
  frames under the lock and decode after releasing it — replay is a snapshot
  anyway.

## 3. security-crypto

No authentication/HMAC/TLS or secret-handling surface exists (zero crypto
deps, zero `unsafe`, no secret comparisons). The entire security-relevant
surface is the recovery-time untrusted-input boundary: replay frame walkers,
envelope decode, sidecar decode, key parse. Integrity rests solely on
**unkeyed CRC32** — adequate against torn writes/bit-rot, transparent to any
adversary who can write to the data directory. `segment_meta::decode`
(`segment_meta.rs:138-155`) is exemplary untrusted-input parsing; finding 1.4
is the one place that rigor was not carried through.

### 3.1 — medium — *(primary: 1.4)* — silent, irreversible truncation on a mid-file CRC mismatch in the ACTIVE segment
- (Full write-up at 1.4. The security lens frames it as the trust-boundary
  issue: CRC32 is unkeyed, so tampering is indistinguishable from bit-rot, and
  the active-segment path destroys data with no error surfaced to
  `SegmentSet::open`'s caller — contradicting the crate's own §1.8 philosophy
  that corrupt-but-complete frames are corruption, not crash tails.)

### 3.2 — low — bincode 1.x decodes recovery data with no allocation bounds — a small crafted/corrupt frame can OOM the process
- File:line: `crates/shamir-wal/src/wal_entry_v2.rs:240,251`
  (`bincode::deserialize`); `crates/shamir-wal/Cargo.toml:12`
  (`bincode = "1.3.3"`); amplified by unbounded `read_to_end` at
  `wal_segment.rs:527-529, 370-374`.
- Issue: bincode 1.x targets *trusted* input — collection/`ByteBuf` length
  prefixes are read as raw `u64`s and used to size allocations before the
  reader validates that many bytes exist. A frame that passes the CRC gate
  reaches `WalEntryV2::decode` with an inner length claiming ~2^60
  elements/bytes → huge speculative allocation → abort/OOM at recovery from a
  ~40-byte file. Separately, `replay_inner`/`repair_torn_tail` `read_to_end`
  the whole segment file with no size cap — nothing at open validates that a
  `.wal` file on disk respects the rotation threshold.
- Failure scenario: corrupt or planted WAL file with a forged (CRC32 unkeyed —
  trivially recomputed) bincode body containing a `u64` length of
  `0x0100_0000_0000_0000`: recovery attempts a multi-exabyte `Vec` allocation
  and the process aborts — a DoS on database open.
- Suggested fix: (a) cap per-frame `len` against a max-entry-size tunable
  before slicing/decoding; (b) prefer a bounded deserializer on this boundary
  (bincode 2 `with_limit`, or postcard) — the envelope already carries a
  version byte so a bounded path can be gated on it; (c) check
  `metadata().len()` against a segment-size bound before `read_to_end`. A
  small `cargo-fuzz` target over `WalEntryV2::decode` + the frame walker would
  lock this boundary in cheaply.

### 3.3 — low — Unauthenticated WAL replay is an injection surface; the "data directory is trusted" assumption is implicit and undocumented
- File:line: `crates/shamir-wal/src/segment_set.rs:74-77, 100-104, 128-129`
  (`parse_seg_seq` accepts any numeric `.wal`; highest seq becomes the append
  tail); `wal_entry_v2.rs:49-110` (replayed ops);
  `segment_set.rs:470-477` (truncation trusts sidecar/filename-derived
  `max_version`).
- Issue: recovery replays whatever `NNNNNNNN.wal` files exist — no manifest,
  no seq-contiguity check, no per-entry MAC. `WalOpV2::Put.body` is arbitrary
  bytes replayed verbatim into data_store; a planted/edited segment (payload +
  recomputed CRC32) silently injects, deletes, or rewrites records, and a
  forged `.meta` sidecar can drive `truncate_below` to delete segments whose
  data is not yet durable in history. Sound **iff** the data directory is
  writable only by the DB's own OS user — a trust boundary nowhere stated.
- Failure scenario: multi-user host or shared/network-mounted WAL directory
  (NFS, shared container volume): any principal with write access owns the
  database's post-crash state — silent record injection/erasure with zero
  tamper evidence.
- Suggested fix: document the threat boundary explicitly in `lib.rs` /
  `segment_set.rs` ("WAL files are trusted input; directory permissions are
  the security boundary"). If WAL storage is ever exposed to weaker trust, add
  a keyed checksum (HMAC-SHA256 over each frame, per-repo secret held outside
  the WAL dir) — naturally done at the already-planned WAL format-version bump
  (see 5.5).

### 3.4 — low — Frame-length arithmetic can wrap on 32-bit targets → panic on a crafted 4-byte header
- File:line: `crates/shamir-wal/src/wal_segment.rs:532-539` (`replay_inner`)
  and `:377-384` (`repair_torn_tail`); also flagged as **correctness-tdd #8**
  and **api-wire-protocol #9** (deduped here — three lenses, one defect).
- Issue: `let len = u32::from_le_bytes(..) as usize; let frame_end = pos + 4 +
  len + 4;` — where `usize == u32`, a header of `0xFFFF_FFFF` wraps
  `frame_end` to a small value (release) or panics on the overflow check
  (debug); the wrapped `frame_end` passes the `frame_end > buf.len()` guard
  and the subsequent `&buf[pos+4..pos+4+len]` slice panics. Untrusted on-disk
  input causing a library panic violates the CLAUDE.md error rule; not
  reachable on production 64-bit targets (and `wasm32` is a declared target
  per CLAUDE.md's "WASM-first"), hence low.
- Failure scenario: a 32-bit/wasm32 build opens a segment whose first 4 bytes
  are `FF FF FF FF`: recovery panics instead of returning `Err`/stopping at
  the torn tail.
- Suggested fix: compute with checked arithmetic
  (`pos.checked_add(4)?.checked_add(len)?.checked_add(4)`) and treat overflow
  exactly like a torn tail (`break`). The fuzz target from 3.2 would catch
  this class permanently.

### 3.5 — nit — A single CRC-valid-but-undecodable frame aborts the entire recovery (version skew == corruption)
- File:line: `crates/shamir-wal/src/wal_segment.rs:572`
  (`out.push(WalEntryV2::decode(payload)?)` — the `?` aborts the whole
  `SegmentSet::replay`).
- Issue: fail-closed on undecodable data is the right instinct (and loudly
  so), but the error conflates corruption with **version skew**: an entry
  written by a newer build (same envelope version byte, evolved bincode schema)
  makes the entire database unopenable after a downgrade, with an `Internal`
  error that doesn't name the skew. The envelope exists precisely to dispatch
  migrations (`wal_entry_v2.rs:20-23`) yet an in-version schema change has no
  distinct signal. (Shares the unconditional-`?` abort mechanism with 5.2's
  empty-payload defect; distinct failure modes, listed separately.)
- Suggested fix: distinguish decode-failure kinds (unsupported/foreign payload
  vs. truncation) in the error, and bump `WAL_V2_VERSION` on any in-body
  schema change as a documented rule so skew is at least self-identifying.

### 3.6 — nit — Error strings embed absolute filesystem paths
- File:line: e.g. `crates/shamir-wal/src/wal_segment.rs:155, 201-204, 519-523,
  556-563`; `segment_set.rs:92, 512`.
- Issue: `DbError::Storage(format!("... {path:?} ..."))` bakes server-side
  absolute paths into error text. Whether this crosses the network depends on
  how `shamir-server`/`shamir-connect` map `DbError` onto wire responses (out
  of this crate's view); if any path reaches a client, it discloses host
  directory layout.
- Suggested fix: keep paths in `tracing`/`log` output; return path-free (or
  basename-only) messages in the `DbError` variants that can reach callers
  outside the process.

## 4. performance-hotpath

The core amortization story is sound and unusually well documented: group
commit coalesces a window of N committers into exactly one `write()` and at
most one `fsync()`; rotation is driven by an atomic byte counter (no
`metadata()` syscall per append); the #500 sidecar removed the O(total WAL
bytes) startup replay. No quadratic path was found. Test/bench coverage for
the theme is good (`benches/wal_append.rs`, `benches/wal_startup_open.rs`,
`benches/segment_set_lock.rs`, plus the fsync-batching tests).

### 4.1 — medium — `has_truncatable` on the `Mem` sink is an O(frames) scan run on every drainer tick
- File:line: `crates/shamir-wal/src/wal_sink.rs:200-208` (Mem arm,
  `frames.iter().any(..)` at 205); sibling `SegmentSet::has_truncatable` at
  `crates/shamir-wal/src/segment_set.rs:557-562`.
- Issue: documented and consumed as a "cheap probe" —
  `shamir-engine/src/tx/drainer.rs:730-743` calls it in `settle_and_truncate`,
  which runs at the end of *every* `drain_step` pass and on the
  `dur >= vis` early-return path, i.e. every background drainer tick even when
  nothing new was committed. For `WalSink::File` the scan is over the
  short sealed-segment list (genuinely cheap). For `WalSink::Mem` it is a
  linear scan over *every frame appended since the last truncation*, under the
  `frames` std Mutex. "N" grows precisely when the truncation gate
  (`pending_unsafe` / interner A5 hwm) lags — a wedge that also makes the
  scan-per-tick fire repeatedly over the largest possible list.
- Failure scenario: an in-memory repo under sustained write load whose
  interner delta gate stalls: frames grow to millions; every drainer tick pays
  an O(millions) scan of a ~32-byte-per-element Vec (plus lock hold), turning
  the background loop into a constant CPU burner proportional to backlog size.
- Suggested fix: maintain an O(1) mirror per CLAUDE.md pillar 3 (the
  `scc::len()` rule): e.g. an `AtomicU64` tracking the minimum non-pinned
  frame `max_version` (updated on `append_batch` when the list is empty / on
  `truncate_below` by re-deriving from the retained head), so the probe is a
  single load. The `File` variant can keep its short scan or mirror
  `min(sealed.max_version)` the same way.

### 4.2 — low — `WalEntryV2::encode` starts from a 256-byte capacity guess that realistic entries overflow
- File:line: `crates/shamir-wal/src/wal_entry_v2.rs:211-221`
  (`Vec::with_capacity(256)` at 215).
- Issue: `encode()` runs once per committed transaction on the hottest path.
  The comment claims "one alloc is the common case", but a V2 entry carries
  full inline record bodies — any entry over ~256 bytes (the crate's own
  startup bench uses a 256-byte *body*, which already overflows once headers
  are added) climbs the geometric-growth ladder, each step a realloc plus
  memcpy. Amortized O(bytes), not O(N²), but avoidable churn on every commit —
  and the justifying comment is wrong for the common case. Pure waste on the
  non-fsync (`Buffered`/mem-sink) tiers where throughput is coordination-bound.
- Failure scenario: a workload of 4 KB records: ~5 reallocs + copies per
  commit for the life of the database.
- Suggested fix: size exactly with `bincode::serialized_size(self)` (second
  pass, zero allocations) → `Vec::with_capacity(5 + size as usize)`, or
  cheaply pre-estimate from `ops`. Fix the comment.

### 4.3 — low — Per-window allocation churn in `lead_until_drained`: pending Vec regrows from capacity 0 every window
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:270-287`
  (`std::mem::take` at 277; `payloads`/`metas` fresh Vecs at 280-281);
  `pending: Mutex<Vec<Pending>>` initialized bare at 158;
  `Arc::new(payloads)` at `segment_set.rs:242`.
- Issue: `mem::take` replaces `pending` with a zero-capacity `Vec::new()`, so
  every window the queue re-climbs the allocation ladder (~5 reallocs for a
  64-committer window) and the drained Vec's capacity is dropped instead of
  recycled; each window also allocates `payloads`/`metas`, and lines 298/308
  make two extra O(window) `.any()` scans that could fold into the destructure
  loop. All amortized O(1) per entry and measured non-binding (mem sink scales
  4.4× with concurrency), but steady allocator traffic in the innermost commit
  loop. No cliff.
- Suggested fix: keep a spare Vec (`std::mem::replace(&mut *p,
  spare.take().unwrap_or_default())` pattern) so capacity survives windows;
  seed `WalGroupCommit::new`'s Vec with a small capacity (e.g. 64); fold the
  `has_buffered`/`needs_fsync` computations into the destructure loop.

### 4.4 — low — Startup sidecar fallback decodes every entry (allocating all ops/Bytes/Strings) to extract one `u64`
- File:line: `crates/shamir-wal/src/segment_set.rs:145-158`
  (`replay_sealed_at_startup().await` then
  `entries.iter().map(|e| e.commit_version).max()` at 155-156).
- Issue: when a sealed segment lacks a valid `.meta` sidecar (pre-#500
  segment, or interrupted sidecar write), `open` computes `max_version` by
  fully replaying: `read_to_end` of the whole file (up to 8 MiB, geometric
  growth, no capacity hint) and bincode-decoding every entry — materializing
  each `Vec<WalOpV2>`, every `Bytes` body, every `InternerOverlayMerge`
  `String` — to read one `commit_version` field and drop everything.
- Failure scenario: cold start after a long downtime on a pre-sidecar
  database: startup time and transient RAM spike are O(total WAL bytes ×
  decode cost) — exactly the cost the sidecar was added to avoid, paid in full
  on the fallback.
- Suggested fix: a streaming frame walk that reads only each payload's
  `commit_version` (fixed-offset slice read with the existing CRC check, or a
  minimal `Deserialize` impl on a header-only proxy type), without
  materializing ops. Seed the `read_to_end` buffer from `metadata().len()`.

### 4.5 — low — `replay` materializes the entire WAL as decoded entries in one Vec
- File:line: `crates/shamir-wal/src/segment_set.rs:423-446` (`out.extend(...)`
  over all sealed + active); `wal_segment.rs:527-530`; `wal_sink.rs:158-170`
  (Mem arm).
- Issue: recovery accumulates every decoded `WalEntryV2` from every segment
  into a single `Vec` before returning, so peak RAM is O(total un-truncated
  WAL bytes × decode-expansion factor), not O(one segment). On the Mem arm the
  lock is held across the whole decode loop (legal, no `.await`, but blocks
  any concurrent appender for the full replay).
- Failure scenario: a large un-truncated backlog (wedged interner gate + power
  loss) on a memory-constrained host: recovery can OOM even though a
  streaming/callback-based replay would run in O(largest segment) memory.
- Suggested fix: offer a streaming variant (`replay_with(|entry| ...)` or
  chunked API) alongside the Vec-returning one; the recovery consumer in
  `shamir-tx` sorts by `commit_version` anyway, doable with a two-pass or a
  merge over per-segment streams.

### 4.6 — nit — Full window bytes memcpy'd into the coalescing buffer per append batch
- File:line: `crates/shamir-wal/src/wal_segment.rs:223-234`
  (`Vec::with_capacity(total)` + per-payload `extend_from_slice`).
- Issue: every encoded payload is copied a second time (encode → pending Vec →
  payloads Vec → coalesce buf → kernel). The copy buys exactly one `write()`
  instead of 3N — a sound trade (fsync dominates; Windows `File` does not
  advertise vectored writes anyway) — noted for completeness, not as a defect.
- Suggested fix: if it ever shows in a profile: `Write::write_vectored` with an
  IoSlice chain on unix (guarded), or `bytes::Bytes`-backed payloads so frames
  share allocation with entry bodies.

### 4.7 — nit — One `Arc<Waiter>` heap allocation per single append
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:175`
  (`Arc::new(Waiter::new())`).
- Issue: every `append` allocates a `Waiter` (two atomics + a `Notify`). The
  alternatives were measured/reasoned worse — the crate's own docs record that
  the reverted single-writer prototype's oneshot-per-append cost ~+22% mem N=1
  latency — so this is a documented, deliberate cost.
- Suggested fix: none required. If ever revisited, a lock-free intrusive waiter
  list reusing the caller's stack frame would remove the alloc.

## 5. api-wire-protocol

The versioned surfaces are genuinely well-engineered: the V2 envelope
(`[magic][version][bincode]`) decodes both v1-legacy and v2 bodies with
pinning tests; the `.meta` sidecar has a documented fallback matrix with a
test per case; frame CRC / torn-tail / sealed-vs-active replay semantics are
thoroughly covered. The weaknesses concentrate in the unversioned and retired
parts of the surface. Builder-only query-construction rule: trivially
compliant — no `serde_json` anywhere; WAL entries are typed structs assembled
with builder-style `with_commit_version`.

### 5.1 — medium — Segment-name parser accepts non-canonical names and can silently shadow WAL data
- File:line: `crates/shamir-wal/src/segment_set.rs:74-77` (`parse_seg_seq`;
  re-verified: `stem.parse::<u64>().ok()` accepts any numeric stem), writer at
  `:68-70` (`seg_file_name`).
- Issue: `seg_file_name` writes the canonical `NNNNNNNN.wal` (zero-padded 8),
  but `parse_seg_seq` accepts *any* numeric stem (`"5.wal"`, `"0000001.wal"`,
  9+ digits) and canonicalizes the seq back to the 8-digit path. The directory
  listing is this store's "wire", and the parser is more lenient than the
  writer.
- Failure scenario: a foreign or legacy file `5.wal` next to a real
  `00000005.wal`: both parse to seq 5; one becomes "sealed" and the other
  "active", but both `SealedMeta.path` and the active path resolve to
  `00000005.wal`. `5.wal` is silently never replayed, never truncated, never
  mentioned — if it held un-drained commits, that is silent data loss at
  recovery.
- Suggested fix: require the canonical form exactly (stem length == 8, all
  digits, suffix `.wal`); treat non-canonical `.wal` names as a loud `open`
  error (or at minimum skip-with-log), and dedupe/reject duplicate seqs from
  the scan.

### 5.2 — medium — Append path accepts payloads that produce well-formed frames which then hard-fail replay
- File:line: `crates/shamir-wal/src/wal_segment.rs:195-234` (`append_batch`
  writes any `Vec<u8>` incl. empty), `:572` (`WalEntryV2::decode(payload)?` in
  `replay_inner`), `:377-394` (`repair_torn_tail` keeps the frame).
- Issue: the sink layer is byte-opaque by design, but nothing validates that a
  payload is a decodable entry. A zero-length payload is the sharp edge: its
  frame `[len=0][crc=0]` is *well-formed* (CRC32 of empty input is 0), so
  `repair_torn_tail` keeps it and every replay mode reaches
  `WalEntryV2::decode(&[])`, whose error propagates via `?` as a hard error
  even in the tolerant active-segment/startup path.
- Failure scenario: any caller of the public `WalSink::append_batch` /
  `WalGroupCommit::append` passing an empty (or truncated) `Vec<u8>` — the only
  production caller encodes a real `WalEntryV2` today, so this needs API
  misuse, but the API invites it — writes a frame that makes
  `RepoWalManager::recover()` fail on every subsequent open. Permanently, until
  manual repair; the "torn tail is discarded" contract does not cover it.
- Suggested fix: reject empty payloads at `WalGroupCommit::append`/
  `append_many` and `WalSink::append_batch` (`Err` before touching
  `next_seq`/file), and in `replay_inner` decide decode-failure policy per mode
  explicitly (treat as corrupt frame: break for active, loud `Err` for sealed)
  instead of an unconditional `?`.

### 5.3 — medium — Retired F5c KV-marker wire protocol still exported as public API, with docs describing it as live
- File:line: `crates/shamir-wal/src/lib.rs:54` (`pub use
  active_key::WalActiveKey`; re-verified), `src/active_key.rs` (whole file),
  `src/wal_entry_v2.rs:1-16` (module doc) and `:259-264` (`looks_like_v2`),
  `src/wal_segment.rs:3` (`[`crate::WalManager`]`); also flagged as
  **style-claude-md #2 and #4** (deduped here).
- Issue: `lib.rs`'s own architecture doc says the F5c/F6 cutover retired the
  KV-marker design ("production no longer uses such markers"), and the engine
  removed `shamir_wal::WalManager` + the V1 codec in F5c
  (`shamir-engine/src/table/table_manager_crud.rs:359`). Yet the crate still
  exports `WalActiveKey` and `WalEntryV2::looks_like_v2` — a workspace-wide
  grep shows **zero code consumers** outside their own tests. Meanwhile
  `wal_entry_v2.rs`'s module doc asserts "Coexists with the V1
  [`super::wal_entry::WalEntry`]" (broken intra-doc link — no `wal_entry`
  module exists in this crate) and "Both V1 and V2 entries live under the same
  `WalActiveKey` prefix in info_store; recovery distinguishes them by sniffing
  the magic prefix (stage 0.8 will wire this)" — a description of a wire
  protocol that no longer exists. `wal_segment.rs:3` references
  `[`crate::WalManager`]` (broken link); `looks_like_v2`'s doc cites the
  removed `WalManager` as its user; `active_key.rs`'s doc claims the encoding
  "lives in one place instead of being recomputed at three callsites" serving
  recovery's scan flow — all belonging to the retired design. `looks_like_v2_sniff`
  in the tests even asserts on hypothetical V1 bincode bytes. (Style's framing:
  doctests are banned crate-wide and rustdoc is not in the pre-commit gate, so
  the broken links and false claims are never surfaced mechanically.)
- Failure scenario: a consumer reads the crate docs, concludes V1 entries may
  be present under `__wal_active_` keys, and builds dispatch/repair tooling
  around `looks_like_v2`/`WalActiveKey` — dead code paths for a format that can
  no longer occur; or reconstructs the wrong storage model top-down and
  "corrects" recovery/append code toward the retired design. (No runtime
  failure; public-surface misinformation on a durability component.)
- Suggested fix: delete `active_key.rs` + its tests and `looks_like_v2` + its
  test (or at minimum `#[deprecated]` + `#[doc(hidden)]` with a pointer to the
  F5c cutover; if deliberately retained as a legacy on-disk-format decoder,
  rewrite the doc to say exactly that), rewrite `wal_entry_v2.rs`'s module doc
  to the segment-store reality (past tense: "retired by the F5c/F6 cutover"),
  and fix the two broken intra-doc links.

### 5.4 — medium — `WalOpV2::IndexPut`/`IndexDel` serialize `idx_id` as a constant 0 with semantics "deferred"
- File:line: `crates/shamir-wal/src/wal_entry_v2.rs:69-100` (field + invariant
  doc); sole producer `shamir-engine/src/tx/commit.rs:320-330` hardcodes
  `idx_id: 0`.
- Issue: a wire-format field is written but meaningless — the producer always
  emits 0, consumers must decode the real index id from the `key` byte prefix
  (`[idx_id_be: 4][rest]`), and the doc says the reconciliation decision is
  "deferred to the recovery implementation". Cross-crate confirmation:
  `shamir-tx/src/index_write_op.rs:58` calls it "currently-unpopulated … for a
  FUTURE wire-level identity scheme".
- Failure scenario: whichever way the deferral lands, the on-disk corpus is
  locked in: if real ids start being emitted later, every future reader must
  forever special-case `idx_id == 0` as "decode from key prefix" — and a table
  that legitimately has index id 0 is indistinguishable from the legacy
  encoding. A new consumer trusting the field (it *looks* populated in the
  schema) misroutes postings.
- Suggested fix: resolve the decision now while the corpus is young: either
  remove `idx_id` from the wire struct (bump `WAL_V2_VERSION` to 3 with a
  legacy decode path, mirroring the existing v1 pattern), or thread real ids
  through and document `0`-means-prefix-decode as a permanent invariant in both
  this doc and the recovery code.

### 5.5 — low — Frame format has no per-frame magic/seq and segment files have no header or format version
- File:line: `crates/shamir-wal/src/wal_segment.rs:228-234` (frame layout
  `[u32 len LE][payload][u32 crc32 LE]`), `:546-563` (the in-code
  acknowledgment: "no magic/seq for resync … deferred follow-up … requires a
  WAL format version bump").
- Issue: the `.wal` file is a bare frame stream: no file magic, no format
  version (the 17-byte `.meta` sidecar has both, the segment itself has
  neither), no per-frame sequence. Consequences already visible in the API: a
  single corrupt frame mid-*active* segment silently discards the entire valid
  tail (1.4), while the same corruption in a sealed segment is a hard operator
  error — the format cannot resync, so these coarse policies are forced.
- Suggested fix: when the deferred version bump happens anyway (see 5.4), add a
  small segment header (`[magic "WSEG"][version]`) and per-frame `[seq]` so
  (a) future layout changes are detectable, (b) single-frame-skip resync
  becomes possible, narrowing the silent-tail-loss window.

### 5.6 — low — `append_batch` returns a "seq" that is per-segment, non-persisted, and resets to 0 on every open
- File:line: `crates/shamir-wal/src/wal_segment.rs:177`
  (`next_seq: AtomicU64::new(0)` even when reopening a non-empty file;
  re-verified), `:212-213`; surfaced publicly at `segment_set.rs:222`
  ("Returns the seq assigned to the last entry") and `wal_sink.rs:109`; also
  flagged as **correctness-tdd #6** (deduped here).
- Issue: the `u64` return value is: relative to the current segment only,
  never written to disk, restarted at 0 by every `WalSegment::open` (fresh or
  reopened) — colliding with the pre-existing frames' seqs after any reopen,
  in asymmetric contrast to `bytes_written` being seeded from on-disk length
  at `:179` — and consumed by nothing in production (`lead_until_drained`
  discards it via `.is_ok()`; only tests read it). The docs read like a global
  sequence, inviting LSN-style misuse; a latent API trap rather than a live
  bug.
- Failure scenario: a future caller uses the returned seq as a per-segment
  entry handle (targeted truncation, dedup); after any reopen the handle
  aliases an older frame.
- Suggested fix: either drop the return value from the public signatures
  (`-> DbResult<()>`) or make it a real durable global sequence (persisted in
  the frame per 5.5); at minimum seed `next_seq` from the frame count computed
  during `repair_torn_tail`/open, or document that it is a per-segment,
  per-open in-memory counter.

### 5.7 — low — *(primary: 6.4)* — no library error enum: all wire failures collapse into `DbError::Internal(String)`/`Storage(String)`
- (Full write-up at 6.4. The api lens adds the wire angle: "bad magic",
  "unsupported version", "corrupt bincode body", "spawn_blocking join" and
  "ENOSPC on write" are programmatically indistinguishable, and message-matching
  on strings — as `wal_segment_tests.rs:184-187`'s
  `err_msg.contains("CRC mismatch")` — becomes the de-facto wire contract.)

### 5.8 — low — *(primary: 6.3)* — group-commit waiter transport discards the underlying error
- (Full write-up at 6.3. The api lens supplies the mechanism and fix shape:
  `Waiter` (`wal_group_commit.rs:89-108`) carries only `done`/`ok` atomics;
  extend it with a cheap error slot — `Mutex<Option<DbError>>` or
  `ArcSwapOption<DbError>` set before `notify_one` — so leaders store the first
  error and waiters clone it into the returned `Err`.)

### 5.9 — low — *(primary: 3.4)* — frame-length arithmetic overflow on 32-bit/wasm32 targets
- (Full write-up at 3.4. The api lens adds: CLAUDE.md declares the project
  "WASM-first", making the 32-bit `usize` case more than theoretical.)

### 5.10 — nit — `WalSegment::mark_poisoned` is an un-gated public test hook
- File:line: `crates/shamir-wal/src/wal_segment.rs:339-344`.
- Issue: the doc says "exposed (pub(crate)-ish) for tests", but it is fully
  `pub` in the public API, unlike every other test hook in the crate which is
  properly `#[cfg(test)] pub(crate)` (`SegmentSet::active_segment_for_test`,
  `WalSink::arm_fail_next_append`, `WalGroupCommit::{fsync_count,is_dirty,set_dirty}`).
  Any downstream user can silently quarantine a production segment.
- Suggested fix: gate it `#[cfg(test)] pub(crate)` (the poison tests live in
  this crate's `tests/`), or at least `#[doc(hidden)]` with the
  fault-injection rationale.

### 5.11 — low — *(primary: 7.1)* — wire-format tests for `segment_meta` inline in the implementation file
- (Full write-up at 7.1; flagged by this lens because the subject under test
  is the sidecar wire format.)

### 5.12 — nit — Anonymous tuple types in the public wire structs
- File:line: `crates/shamir-wal/src/wal_entry_v2.rs:105`
  (`InternerOverlayMerge { entries: Vec<(u64, String)> }`), `:159`
  (`interner_delta: Vec<(u64, String, u64)>`).
- Issue: the positional triples `(table_token, field_name, intern_id)` and
  pairs `(id, name)` are part of the serialized schema and public API but
  carry meaning only via doc comments; `.0/.1/.2` access at every consumer is
  a standing misindexing hazard, and adding a field later forces a
  wire-breaking tuple→struct change anyway.
- Suggested fix: tiny named structs (`InternerDeltaEntry { table_token,
  field_name, intern_id }`) at the next version bump, mirroring the v1→v2
  legacy-decode pattern for old corpora.

## 6. error-handling-lifecycle

Happy-path `Result` discipline is good (`DbResult` everywhere, `?`
propagation, a well-reasoned poison/quarantine model with
rollback-on-write-failure), but two error-path correctness gaps stand out
(6.1, 6.2), error fidelity is weak across the board, and several error paths
have no test coverage (see 1.2, 1.3, 1.5).

### 6.1 — high — Cancellation or panic of the leader task wedges `flushing` forever — permanent WAL append hang
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:180-186` (CAS + inline
  lead), `:273-275`, `:327-330` (the only two release points).
- Issue: leadership (`flushing == true`) is acquired by CAS in
  `append`/`append_many` and released only inside `lead_until_drained` — at
  the observed-empty exit or the circuit-breaker exit. The leader runs inline
  on the caller's task (`self.lead_until_drained().await`, not spawned), so if
  that future is dropped at any internal `.await` (caller timeout, `select!`
  shutdown, runtime drop) or unwinds via panic (e.g. any of the
  `.lock().expect(...)` sites in `SegmentSet` firing while the leader holds
  leadership), `flushing` stays `true` with no `Drop` guard and no recovery
  path.
- Failure scenario: an engine committer task wrapped in `tokio::time::timeout`
  wins the CAS, then is cancelled while awaiting `sink.append_batch`. Every
  subsequent appender fails the CAS, parks on its `Waiter`, and is never
  notified — the commit path hangs until process restart. Entries already
  `mem::take`n from `pending` but not yet completed hang their waiters too.
- Relation to 1.1: sibling defects sharing the release-point analysis, but
  distinct failure modes and distinct fixes — 1.1 is the breaker exit after a
  *completed but failed* window leaving stragglers; this is the leader task
  vanishing mid-window with no release at all. (The workspace SUMMARY's
  "3 sibling themes" callout covers this cluster.)
- Suggested fix: release leadership via an RAII guard (a small struct borrowing
  the `AtomicBool` whose `Drop` does `store(false, Release)`); double-drain is
  benign because `mem::take` hands each entry to exactly one leader. Keep the
  under-lock release for the L1 no-stranded-pusher argument, and let the guard
  be the cancellation/panic backstop.

### 6.2 — high — Seal-time fsync failure fails an already-successful append whose frames survive — "acked-failed" tx resurrected on replay
- File:line: `crates/shamir-wal/src/segment_set.rs:247-253`
  (`seal_and_rotate().await?` in the Ok arm; re-verified), `:367`
  (`sealing.sync().await?`); waiters completed(false) at
  `wal_group_commit.rs:290-294, 302-306`.
- Issue: in `SegmentSet::append_batch`, the write succeeds and only then does
  the size check fire `seal_and_rotate`, whose fsync failure propagates via
  `?` and turns the whole window's result into `Err`. The batch's bytes are
  already in the page cache, and the now-poisoned segment is *not* rolled back
  (rollback runs only on `write_all` failure) — it is later sealed-as-poisoned
  with its intact prefix replayable. The group-commit leader sees
  `write_ok == false` and completes **all** waiters — including `Buffered`
  ones whose level-2 tier was genuinely reached — with failure.
- Failure scenario: ENOSPC/EIO fsync exactly when the active segment crosses
  `max_bytes`: the committer is told "wal group commit failed",
  aborts/reports failure to the client, yet recovery replays the entry as a
  committed tx. This is precisely the §1.6 property `append_many` documents
  ("no entry survives a partial write … never replays a subset of a 'failed'
  batch") — violated at the rotation seam instead of the write seam. For
  `Synced` waiters an Err on a failed fsync is defensible (durability
  genuinely unknown); for `Buffered` waiters it contradicts the tier contract
  outright.
- Suggested fix: in the Ok arm, treat `seal_and_rotate` failure as a logged
  housekeeping error, not an append error: return `Ok(last_seq)` and let the
  poison flag force rotation on the next append (the sidecar-write failure two
  lines below is already swallowed for exactly this reason). Then `Buffered`
  waiters ack correctly, and `Synced` waiters still fail via their own
  subsequent `sink.sync()` against the poisoned segment — acks align with the
  actual tier outcome.

### 6.3 — medium — Group-commit layer discards the underlying error — waiters get a context-free generic `Err`
- File:line: `crates/shamir-wal/src/wal_group_commit.rs:290-294`
  (`append_batch(...).is_ok()`), `:309-315` (`sync(...).is_ok()`),
  `:198-202` / `:258-262` (`DbError::Storage("wal group commit failed")` /
  `"... batch failed"`); `Waiter` carries only bools at `:89-108`; retry
  failure propagates unlogged from `segment_set.rs:270`; also flagged as
  **correctness-tdd #7** and **api-wire-protocol #8** (deduped here).
- Issue: the leader reduces both the write and the fsync outcome to `bool`;
  the `DbError` (io kind, path, "poisoned" reason) is dropped without even a
  log at this layer, and waiters receive a fixed-string error with no cause
  chain. Upstream layers do log (`WalSegment` poison logs, `SegmentSet`
  first-attempt retry log at `segment_set.rs:261-264`), but the retry's
  failure (`:270`) propagates unlogged — in the retry-failure case no log line
  anywhere carries the actual OS error (ENOSPC detail, path). The engine→client
  error for the most critical write path in the DB cannot distinguish ENOSPC
  from EIO from a poisoned segment.
- Failure scenario: operator sees every commit fail with "wal group commit
  failed"; the actual cause is only in server logs, and programmatic handling
  (retry-after-space vs fail-stop) is impossible from the error value.
- Suggested fix: extend `Waiter` with a cheap error slot
  (`Mutex<Option<DbError>>` or `ArcSwapOption<DbError>` set before
  `notify_one`); leaders store the first error, waiters clone it into the
  returned `Err`; `match` instead of `is_ok()` and `log::error!` the dropped
  `Err` in `lead_until_drained`; log the retry's failure in
  `SegmentSet::append_batch` as well.

### 6.4 — medium — No thiserror error enum — stringly-typed errors, `io::ErrorKind` destroyed at the wrap boundary
- File:line: whole crate; e.g. `crates/shamir-wal/src/wal_segment.rs:155, 168,
  236-263, 315, 519-525`; `wal_entry_v2.rs:219-256`; symptom in tests at
  `src/tests/wal_segment_poison_tests.rs:113-118`; also flagged as
  **api-wire-protocol #7** (deduped here).
- Issue: CLAUDE.md mandates `thiserror` for library error enums; shamir-wal
  defines none, hand-formatting everything into
  `DbError::Storage(String)`/`Internal(String)`. Notably
  `DbError::Io(#[from] std::io::Error)` already exists in shamir-storage, yet
  every io error here is stringified — `replay_inner` pattern-matches
  `ErrorKind::NotFound`/`PermissionDenied` *before* wrapping and then flattens
  the kind, so no downstream caller can ever match on it. The poison test's
  `msg.contains("poisoned")` assertion shows the practical cost: conditions
  are only identifiable by substring.
- Failure scenario: engine/drainer code that needs to branch on "segment
  poisoned → rotate" or "NotFound → skip" cannot; each new consumer reinvents
  substring parsing, which breaks silently when messages are reworded.
- Suggested fix: cheapest: wrap io errors with `DbError::Io` (lossless kind)
  and add a dedicated variant/message for poison; proper: a `WalError`
  thiserror enum (`SegmentPoisoned { path }`, `WriteFailed { path, #[from]
  io::Error }`, `SealedFrameCorrupt { path, offset }`, `BadMagic`,
  `UnsupportedVersion { got }`, `Decode { source }`, …) converted to `DbError`
  at the crate boundary; keep `DbResult` as the return alias if the workspace
  Result type must be preserved.

### 6.5 — medium — `SegmentSet::replay` fronts sealed paths with a create-mode open — defeats the documented NotFound/PermissionDenied tolerance and side-effect-creates files on a read path
- File:line: `crates/shamir-wal/src/segment_set.rs:435`
  (`WalSegment::open(meta.path.clone()).await?`) vs the tolerance machinery at
  `src/wal_segment.rs:507-524` and the rationale at `:432-451`.
- Issue: `WalSegment::replay`'s doc justifies its NotFound/Windows-delete-pending
  tolerance by "a concurrent `truncate_below` can unlink one of the snapshot's
  paths between the snapshot capture and our open here" — but
  `SegmentSet::replay` calls `WalSegment::open`, which uses
  `OpenOptions::create(true)` (`wal_segment.rs:150-155`): (a) a *missing*
  sealed segment is silently re-created as an empty `.wal` (a filesystem
  mutation on a recovery/read path, which later reopens treat as a
  `max_version == 0` PIN segment that is never reclaimable, I5); (b) a Windows
  delete-pending path makes the *create-mode open itself* fail
  `PermissionDenied`, surfaced as a hard `DbError::Storage` — the exact error
  the one-layer-down tolerance was built to absorb. Grep confirms the tolerant
  non-startup variants (`replay()`, `replay_sealed()`) have no production
  callers at all — the tolerance machinery is unreachable through the only
  real path. Production is currently safe only because replay is startup-only
  (sole caller `RepoWalManager::recover`), which makes the tolerance's
  placement dead weight and its docs misleading about the live contract.
- Failure scenario: an operator/archival step removes a sealed `.wal` between
  the list snapshot and replay (or the pub API is used concurrently with the
  pub `truncate_below`): replay either fabricates an empty segment file or
  hard-fails with a confusing open error, instead of the documented skip.
- Suggested fix: open sealed segments for replay read-only (`File::open`-based
  constructor, no `create`), letting the existing kind-matching in
  `replay_inner` govern; delete or actually wire the tolerant variants.

### 6.6 — medium — *(primary: 2.2)* — error-path rollback target is captured outside the file lock
- (Full write-up at 2.2; the error-handling lens framed the same defect as
  "concurrent-append failure truncates a concurrent successful batch".)

### 6.7 — medium — *(primary: 1.2)* — §1.5 dirty-flag-restore regression test is tautological
- (Full write-up at 1.2.)

### 6.8 — medium — *(primary: 1.5)* — untested error paths on the File sink
- (Full write-up at 1.5; error-handling #8's enumeration — real write-failure
  rollback incl. the fresh-write-mode-handle Windows workaround and the
  rollback-itself-failed "unknown state" path; `rotate_after_poison` open
  failure; §2.4 `PermissionDenied`; sealed-CRC-through-`open` — is folded
  there.)

### 6.9 — low — `truncate_below` partial failure discards progress and strands claimed files until restart
- File:line: `crates/shamir-wal/src/segment_set.rs:511-515`.
- Issue: claim-then-delete removes entries from `sealed` under the lock
  *before* unlinking; if the unlink of file #k fails with a hard (non-NotFound,
  non-PermissionDenied) error, the `Err` return discards the count of
  already-deleted files, and files #k+1..n — claimed but never unlinked — are
  untracked for the rest of the process lifetime (only a reopen rescans them).
  The data-safety rationale (idempotent replay) holds, but the caller-visible
  semantics on partial failure are lossy: the drainer learns nothing about
  what was reclaimed.
- Suggested fix: on a hard unlink error, log and continue the loop (counting
  successes) instead of returning early — a leaked file is already documented
  as harmless, so failing the whole call buys nothing; or return the partial
  count alongside the error.

### 6.10 — low — Corrupt on-disk data decoded as `DbError::Internal` — misclassifies corruption as a code bug
- File:line: `crates/shamir-wal/src/wal_entry_v2.rs:229, 232-235, 241, 252-255`.
- Issue: `WalEntryV2::decode` failures (bad magic, short input, unknown
  version, bincode errors) are returned as `DbError::Internal("wal_v2 decode:
  ...")`. These originate from *persisted bytes*, i.e. data corruption or
  format drift — the existing `DbError::Codec`/`Storage` taxonomy fits;
  `Internal` signals a programmer bug and will misroute operator triage (file
  a bug vs. restore from backup), especially since these errors surface from
  `replay` where a CRC check has already passed. (Adjacent to 3.5's
  version-skew conflation; distinct classification defect.)
- Suggested fix: return `DbError::Codec` (or a typed corruption variant per
  6.4) from `decode`; reserve `Internal` for encode-side/logic failures.

### 6.11 — nit — `fsync_parent_dir`'s `DbResult<()>` can never return `Err`
- File:line: `crates/shamir-wal/src/wal_segment.rs:41-68`.
- Issue: both failure modes are logged at `warn` and swallowed (a documented,
  reasonable degradation decision), so the signature promises an error path
  that does not exist — and on filesystems where directory fsync returns
  EINVAL, every new segment creation emits a warn, a potential log-spam source
  on network mounts.
- Suggested fix: return `()` and keep the doc comment, or downgrade the
  EINVAL-family log to `debug` with a once-per-path latch.

## 7. style-claude-md

The crate's skeleton largely conforms: `lib.rs`/`tests/mod.rs` are
re-export-only manifests, modules are topic-split with one primary export
each, tests live in `src/tests/` as fixture-only topic files, imports are
otherwise hoisted (including the sanctioned cfg-gated import in
`wal_sink.rs`). The clear structural breach is 7.1; the dominant
comment-discipline problem is stale documentation describing retired designs
as current (7.2/7.3/7.4 → deduped at 5.3 and 7.3).

### 7.1 — high — Inline `#[cfg(test)] mod tests` in an implementation file
- File:line: `crates/shamir-wal/src/segment_meta.rs:175-218` (re-verified);
  also flagged as **correctness-tdd #9 (low)** and **api-wire-protocol #11
  (low)** (deduped here — one defect, three lenses, divergent severities).
- Issue: CLAUDE.md test-organisation rule 5: "Never embed `#[cfg(test)] mod
  tests { ... }` inline inside implementation files." `segment_meta.rs`
  carries a 44-line inline test module (5 tests: `roundtrip_encode_decode`,
  `decode_rejects_bad_magic`, `decode_rejects_bad_version`,
  `decode_rejects_bad_crc`, `decode_rejects_wrong_length`). Every other module
  in this crate correctly puts its tests in `src/tests/` (6 topic files behind
  the manifest-only `tests/mod.rs`); this is the sole outlier, and
  `src/tests/segment_meta_tests.rs` does not exist. `segment_meta`'s private
  fns (`encode`/`decode`) are reachable from a sibling test file via
  `crate::segment_meta::…` or `pub(crate)` shims, same pattern other test
  files already use for `pub(crate)` knobs.
- Failure scenario: the next contributor adding sidecar tests appends to the
  inline module (the visible local precedent inside this file), and the
  crate's test layout forks; test discovery by file no longer works for this
  module.
- Severity note: the style lens rated this high (bright-line breach);
  correctness and api rated the identical defect low. The workspace SUMMARY
  itself observes this rating divergence inflates shamir-wal's lens-tagged
  high count. Carried at high as the primary lens's rating; after dedup it is
  one discipline item, not a correctness risk.
- Suggested fix: move the five tests to
  `crates/shamir-wal/src/tests/segment_meta_tests.rs` (marked `pub mod
  segment_meta_tests;` in `tests/mod.rs`), widening `encode`/`decode` to
  `pub(crate)` only if needed. Mechanical, no behavioural change.

### 7.2 — medium — *(primary: 5.3)* — module docs describe the retired KV-marker design as current, with broken intra-doc links
- (Full write-up at 5.3 — style #2 is the docs-comment side of the same
  defect: `wal_entry_v2.rs:1-16` and `wal_segment.rs:3-7` preambles, the
  broken `[`super::wal_entry::WalEntry`]` and `[`crate::WalManager`]` links,
  and the stale `looks_like_v2` doc.)

### 7.3 — medium — `segment_set.rs` module doc claims it is unwired scaffold ("wired into nothing yet")
- File:line: `crates/shamir-wal/src/segment_set.rs:15-16`.
- Issue: the doc says: "PURELY ADDITIVE (F6a): wired into nothing yet —
  production still runs a single [`WalSegment`] via `WalSink::File`. F6b cuts
  `repo_instance` over." This is false on two counts: (a)
  `shamir-engine/src/repo/repo_instance.rs:800-801` already calls
  `shamir_wal::SegmentSet::open(...)` and wraps it in `WalSink::File(segset)` —
  the cutover landed; (b) `WalSink::File` holds a `SegmentSet`
  (`wal_sink.rs:86`), not a single `WalSegment` — the type shape the comment
  describes no longer exists. It directly contradicts sibling docs
  (`wal_group_commit.rs:65-68` "Wired in … production commit path (W3/W4
  landed)"; `wal_segment.rs:15-18` "Live production primitive"; `lib.rs`
  architecture section).
- Failure scenario: a reviewer or contributor assessing whether `SegmentSet`
  is safe to change skips impact analysis on the commit path, believing
  production bypasses it; or removes it as dead scaffold.
- Suggested fix: replace the paragraph with the current truth (production sink
  since F6b; constructed by `repo_instance.rs`) or delete it outright.

### 7.4 — medium — *(primary: 5.3)* — `WalActiveKey`: exported, documented-as-live module with zero production callers
- (Full write-up at 5.3 — style #4 is the export side of the same defect;
  its suggested fix (owner decision: delete module + `lib.rs` exports +
  `active_key_tests.rs`, or rewrite the doc as "retained to parse pre-F5c
  corpora; no live callers") is folded there.)

### 7.5 — low — Mid-function `use` statements in tests (imports-at-top rule)
- File:line: `crates/shamir-wal/src/tests/wal_group_commit_tests.rs:222, 253,
  270, 447`.
- Issue: four test bodies each open with a local `use std::time::Duration;`.
  CLAUDE.md "Imports at the top" bans `use` inside function bodies unless one
  of three documented exceptions applies — none does here; there is no name
  collision, `Duration` is used freely elsewhere in the same file.
  Inconsistently, the file header does *not* import `Duration` and instead
  spells it fully-qualified inside the shared `poll_until` helper (lines 42,
  51).
- Suggested fix: add `use std::time::Duration;` to the header import block and
  delete the four local imports (also shortening `poll_until`'s signatures).

### 7.6 — low — `pub mod segment_meta` exports nothing public
- File:line: `crates/shamir-wal/src/lib.rs:47`; `segment_meta.rs:62, 89, 120,
  164`.
- Issue: the module is declared `pub` but every item in it is `pub(crate)`
  (`meta_path_for`, `write_blocking`, `read_blocking`, `remove_blocking`), so
  the crate's public API contains an empty module. Side effect: the module
  doc's intra-doc links point from a public doc into private items, which
  rustdoc flags as "public documentation links to private item" whenever docs
  are built.
- Suggested fix: demote to `mod segment_meta;` in `lib.rs` (internal helper
  module of `segment_set`).

### 7.7 — nit — Vestigial, unexplained `#[allow(dead_code)]` on a public type
- File:line: `crates/shamir-wal/src/wal_segment.rs:108, 132`.
- Issue: `#[allow(dead_code)]` sits on `pub struct WalSegment` and its `impl`.
  Public items in a library crate cannot be dead code (reachable via the
  public API), so the attributes do nothing today — but they would silently
  mask genuinely dead private helpers if visibility ever narrows, and they
  carry no inline justification, contra the workspace's justified-allow
  convention.
- Suggested fix: delete both attributes; if one was load-bearing in the
  pre-extraction `shamir-engine` location, that history stayed behind.

### 7.8 — nit — `wal_sink.rs` carries two public types with separate impl blocks (borderline)
- File:line: `crates/shamir-wal/src/wal_sink.rs:17, 82`.
- Issue: the only src file with two public types (`WalSink` enum + `MemSink`
  struct, each with its own `impl`, plus a separate `Default` impl).
  Defensible as a "closely-coupled group" — `MemSink` exists solely as
  `WalSink::Mem`'s payload and mirrors its interface — flagged as borderline,
  not a violation. Relatedly, `wal_sink.rs` is the only src module without a
  `//!` module-level doc, so the coupling rationale lives only in scattered
  item docs.
- Suggested fix: optional: move `MemSink` to `mem_sink.rs`, or just add the
  module doc stating the enum-not-trait ("no dyn dispatch on the hot path")
  design and leave the layout as a documented coupled pair.

---

## Finding counts

| Severity | Lens-tagged findings | Deduped distinct defects | Dedup groups (each counts once) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 5 | 4 | 1.1 + 2.1 (circuit-breaker strand) |
| medium | 19 | 14 | 1.2 + 6.7 · 1.3 + 6.8(3) · 1.4 + 3.1 · 1.5 + 6.8(1,2,4) · 2.2 + 6.6 · 5.3 + 7.2 + 7.4 · 6.3 + 1.7 + 5.8 · 6.4 + 5.7 |
| low | 23 | 14 | 3.4 + 1.8 + 5.9 · 5.6 + 1.6 |
| nit | 12 | 12 | — |
| **total** | **59** | **44** | 15 lens-duplicates absorbed |

Lens-tagged reconciliation: 0 critical · 5 high · 19 medium · 23 low · 12 nit
= 59, matching the workspace SUMMARY per-crate row for shamir-wal (pre-dedup,
as expected). Deduplicated defect census: **0 critical, 4 high, 14 medium,
14 low, 12 nit = 44 distinct defects** (59 lens-tagged).

Notes on the dedup: (a) the circuit-breaker hang cluster spans 3 sibling
themes (correctness / concurrency / error-handling) per the workspace SUMMARY;
after dedup it is 1.1, with 6.1 (leader cancellation wedging `flushing`) kept
as a *distinct* defect — same release-point analysis, different failure mode
and fix; (b) the high-severity count drops 5 → 4 because the inline-test
violation (7.1) was double-tagged high/low/low across its three lenses — the
workspace SUMMARY itself flags that divergence as inflating this crate's
lens-tagged high count; (c) where two lenses rated one defect differently
(2.2/6.6: low vs medium; 7.1 cluster: high vs low), the higher rating is
carried and the divergence noted inline.

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Fix the circuit-breaker exit to uphold L1**: under the `pending` lock,
   `flushing.store(false)` + `mem::take` the stragglers in the same critical
   section and complete them with `ok=false` (Err beats hang). Red test first:
   Mem fault knob failing ≥2 consecutive appends, follower raced behind a
   failing leader, assert resolution inside `tokio::time::timeout`. Closes
   **1.1 + 2.1**.
2. **Make leadership cancellation-safe**: RAII guard whose `Drop` releases
   `flushing` (under-lock release kept for the L1 argument; guard as the
   cancellation/panic backstop). Closes **6.1**.
3. **Decouple seal-rotation failure from append success**: in the Ok arm, log
   `seal_and_rotate` failure as housekeeping and return `Ok(last_seq)`; let
   poison force the next rotation. Add a rotation-boundary + fsync-failure
   test asserting `Buffered` waiters ack and nothing resurrects. Closes
   **6.2**.

**P1 — soon**
4. **Distinguish CRC-mismatch from torn tail in `repair_torn_tail`** and log
   the corruption case at `error!` with the truncated-suffix size; regression
   test for mid-file CRC mismatch. Closes **1.4 / 3.1**.
5. **Error fidelity end-to-end**: error slot on `Waiter`, `match` instead of
   `is_ok()` in the leader, `log::error!` the dropped/retry `Err`s. Closes
   **6.3 (+1.7, 5.8)**.
6. **Introduce the `WalError` thiserror enum** (or minimally `DbError::Io`
   wrapping + a poison variant), reclassify decode failures as
   `Codec`-class. Closes **6.4 (+5.7), 6.10**.
7. **Harden the segment-name parser** to canonical `NNNNNNNN.wal` exactly;
   loud error (or skip-with-log) on non-canonical names; reject duplicate
   seqs. Closes **5.1**.
8. **Reject empty payloads at the append APIs** and make `replay_inner`'s
   decode-failure policy per-mode explicit. Closes **5.2** (and defuses
   3.5's unconditional-`?` abort).
9. **Resolve the retired KV-marker surface**: delete or deprecate
   `WalActiveKey` + `looks_like_v2` (+ tests), rewrite the stale module docs
   (`wal_entry_v2.rs`, `wal_segment.rs`, `segment_set.rs`'s "wired into
   nothing yet"), fix the broken intra-doc links. Closes **5.3 (+7.2, 7.4),
   7.3**.
10. **Move `segment_meta`'s inline tests** to `src/tests/segment_meta_tests.rs`.
    Closes **7.1 (+1.9, 5.11)**.
11. **Real fault knobs for the audit-fixed paths**: `fail_next_sync` knob +
    rewrite of the §1.5 test; `classify_open_err` helper unit tests for §2.4.
    Closes **1.2, 1.3 (+6.8 item 3)**.
12. **File-sink fault seam** (`fail_next_write` in the `spawn_blocking`
    closure) + tests for the four enumerated error paths (mid-flight failure →
    rollback → retry; `rotate_after_poison` open failure; §2.4 real arm;
    sealed-CRC-through-`open`). Closes **1.5 (+6.8 items 1, 2, 4)**.
13. **Open sealed segments read-only for replay** (no `create`), delete or
    wire the dead tolerant variants. Closes **6.5**.
14. **O(1) `has_truncatable` mirror** (atomic min-non-pinned-version per
    pillar 3). Closes **4.1**.

**P2 — backlog**
15. **Rollback boundary under the file lock** (last-good-offset maintained in
    the critical section) — or document single-writer-only on
    `append_batch` and soften the bench's "correctness stress test" claim.
    Closes **2.2 (+6.6)**.
16. **Fix `seq` semantics**: seed from frame count, drop the return value, or
    document per-segment/per-open scope. Closes **5.6 (+1.6)**.
17. **Resolve `idx_id`** while the corpus is young (remove via version bump,
    or document `0`-means-prefix-decode as permanent). Closes **5.4**.
18. **WAL format bump**: segment header (`[magic "WSEG"][version]`) +
    per-frame seq, enabling resync and detectable layout changes. Closes
    **5.5** (narrows 1.4's blast radius).
19. **Untrusted-input hardening**: per-frame length cap, bounded deserializer
    at the recovery boundary, `read_to_end` size check, `cargo-fuzz` target
    over `WalEntryV2::decode` + frame walker; checked frame-end arithmetic for
    32-bit/wasm32. Closes **3.2, 3.4 (+1.8, 5.9)**.
20. **Document the trust boundary** (directory permissions are the security
    boundary) in `lib.rs`/`segment_set.rs`. Closes **3.3**.
21. **Bound leader tenure** (hand off after N windows) — only after item 1
    lands. Closes **2.3**.
22. **Epoch-pair dirty tracking** and MemSink decode-outside-lock. Closes
    **2.4, 2.5**.
23. **Allocation churn batch**: exact-size `encode`, spare pending Vec +
    seeded capacity, streaming startup `max_version` walk, streaming replay
    variant. Closes **4.2, 4.3, 4.4, 4.5**.
24. **`truncate_below` continues on hard unlink errors** (log + count).
    Closes **6.9**.
25. **Nits batch**: `sync_now` counter semantics (1.10); path-free error text
    (3.6); gate `mark_poisoned` (5.10); named wire structs for the tuples
    (5.12); `fsync_parent_dir` signature (6.11); delete vestigial
    `#[allow(dead_code)]` (7.7); `wal_sink.rs` module doc (7.8); hoist the
    four test-file `Duration` imports (7.5); demote `pub mod segment_meta`
    (7.6).
