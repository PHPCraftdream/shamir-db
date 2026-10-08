<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-wal — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The circuit-breaker hang and specified coverage gaps remain. CRC repair remains destructive. The claimed dirty-flag safety is false, while failed-fsync counting is consistent with an attempts/issued counter.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 14 | 9 | 0 | 1 | 2 | 0 | 2 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Circuit breaker can strand parked appenders indefinitely (L1 violation, hang)

Status: `confirmed-open`. Current risk: `high`.

Failure releases flushing without locking or draining pending. Followers queued during the failed await already lost election and only await their own Notify; without a later append they never complete.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:176](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L176); [crates/shamir-wal/src/wal_group_commit.rs:189](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L189); [crates/shamir-wal/src/wal_group_commit.rs:277](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L277); [crates/shamir-wal/src/wal_group_commit.rs:327](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L327).

<a id="review-2"></a>

### Claim 2 — Vacuous §1.5 regression test — the restore-on-failed-fsync branch never executes

Status: `confirmed-open`. Current risk: `medium`.

The registered test manually calls set_dirty to simulate restoration and never starts the background error branch. Removing production restoration would not invalidate its assertions. Mem sync always succeeds; no sync fault seam exists.

Evidence: [crates/shamir-wal/src/tests/mod.rs:4](../../../../../crates/shamir-wal/src/tests/mod.rs#L4); [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:294](../../../../../crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L294); [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:327](../../../../../crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L327); [crates/shamir-wal/src/wal_group_commit.rs:394](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L394); [crates/shamir-wal/src/wal_sink.rs:142](../../../../../crates/shamir-wal/src/wal_sink.rs#L142).

<a id="review-3"></a>

### Claim 3 — §2.4 startup PermissionDenied hard-fail branch has zero coverage

Status: `confirmed-open`. Current risk: `medium`.

Startup variants pass false and correctly return an error on PermissionDenied, but the registered API-surface test only reads a healthy file. It cannot detect changing either startup flag to true.

Evidence: [crates/shamir-wal/src/wal_segment.rs:479](../../../../../crates/shamir-wal/src/wal_segment.rs#L479); [crates/shamir-wal/src/wal_segment.rs:486](../../../../../crates/shamir-wal/src/wal_segment.rs#L486); [crates/shamir-wal/src/wal_segment.rs:512](../../../../../crates/shamir-wal/src/wal_segment.rs#L512); [crates/shamir-wal/src/tests/wal_segment_tests.rs:218](../../../../../crates/shamir-wal/src/tests/wal_segment_tests.rs#L218); [crates/shamir-wal/src/tests/mod.rs:6](../../../../../crates/shamir-wal/src/tests/mod.rs#L6).

<a id="review-4"></a>

### Claim 4 — repair_torn_tail silently amputates the valid suffix on a complete-but-CRC-bad frame, mislabeled as torn tail

Status: `confirmed-open`. Current risk: `medium`.

Active open invokes repair before replay. Complete CRC mismatch and incomplete tail both break to set_len(pos), then emit the same torn-tail warning. A valid suffix can be permanently removed without an error returned.

Evidence: [crates/shamir-wal/src/segment_set.rs:173](../../../../../crates/shamir-wal/src/segment_set.rs#L173); [crates/shamir-wal/src/wal_segment.rs:391](../../../../../crates/shamir-wal/src/wal_segment.rs#L391); [crates/shamir-wal/src/wal_segment.rs:402](../../../../../crates/shamir-wal/src/wal_segment.rs#L402); [crates/shamir-wal/src/wal_segment.rs:419](../../../../../crates/shamir-wal/src/wal_segment.rs#L419).

<a id="review-5"></a>

### Claim 5 — §1.3 file-sink mid-flight append failure → rotate → retry-once path is untested

Status: `confirmed-open`. Current risk: `medium`.

Registered file tests manually poison or append a torn tail; none induces write_all failure. Rollback, error-arm rotation/retry, and conservative pre-write max_committed overstatement remain unasserted.

Evidence: [crates/shamir-wal/src/wal_segment.rs:214](../../../../../crates/shamir-wal/src/wal_segment.rs#L214); [crates/shamir-wal/src/wal_segment.rs:236](../../../../../crates/shamir-wal/src/wal_segment.rs#L236); [crates/shamir-wal/src/segment_set.rs:255](../../../../../crates/shamir-wal/src/segment_set.rs#L255); [crates/shamir-wal/src/tests/wal_segment_poison_tests.rs:101](../../../../../crates/shamir-wal/src/tests/wal_segment_poison_tests.rs#L101); [crates/shamir-wal/src/tests/segment_set_tests.rs:876](../../../../../crates/shamir-wal/src/tests/segment_set_tests.rs#L876).

<a id="review-6"></a>

### Claim 6 — Per-segment seq counter restarts at 0 on reopen — returned last_seq can collide with pre-existing frames

Status: `confirmed-open`. Current risk: `low`.

Open seeds byte length but initializes next_seq to zero. Returned ordinals repeat across handles; they are not persisted frame identifiers. Production discards them, so this is API ambiguity rather than demonstrated live record loss.

Evidence: [crates/shamir-wal/src/wal_segment.rs:177](../../../../../crates/shamir-wal/src/wal_segment.rs#L177); [crates/shamir-wal/src/wal_segment.rs:179](../../../../../crates/shamir-wal/src/wal_segment.rs#L179); [crates/shamir-wal/src/wal_segment.rs:213](../../../../../crates/shamir-wal/src/wal_segment.rs#L213); [crates/shamir-wal/src/wal_group_commit.rs:294](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L294).

Grouping/duplicate: `api-wire-protocol.md#6`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Leader discards the underlying I/O error; a failed retry-on-fresh-segment is logged nowhere

Status: `confirmed-open`. Current risk: `low`.

Leader still erases errors into booleans and returns generic Storage text. The blanket logging allegation is too strong: ordinary retry write_all failures are logged inside WalSegment; early poison rejection and join failures can propagate without that log.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:294](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L294); [crates/shamir-wal/src/wal_group_commit.rs:311](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L311); [crates/shamir-wal/src/wal_group_commit.rs:201](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L201); [crates/shamir-wal/src/wal_segment.rs:277](../../../../../crates/shamir-wal/src/wal_segment.rs#L277); [crates/shamir-wal/src/segment_set.rs:270](../../../../../crates/shamir-wal/src/segment_set.rs#L270).

Grouping/duplicate: `error-handling-lifecycle.md#3`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Corrupt length header overflows usize on 32-bit targets → panic in replay/repair

Status: `confirmed-open`. Current risk: `low`.

Both walkers add a disk-controlled u32 length using unchecked usize arithmetic. This can overflow on 32-bit and subsequently invalidate slicing; ordinary 64-bit targets do not have this length-induced overflow.

Evidence: [crates/shamir-wal/src/wal_segment.rs:380](../../../../../crates/shamir-wal/src/wal_segment.rs#L380); [crates/shamir-wal/src/wal_segment.rs:384](../../../../../crates/shamir-wal/src/wal_segment.rs#L384); [crates/shamir-wal/src/wal_segment.rs:535](../../../../../crates/shamir-wal/src/wal_segment.rs#L535); [crates/shamir-wal/src/wal_segment.rs:539](../../../../../crates/shamir-wal/src/wal_segment.rs#L539).

Grouping/duplicate: `security-crypto.md#4`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — Inline #[cfg(test)] mod tests in segment_meta.rs violates the documented test layout

Status: `confirmed-open`. Current risk: `low`.

Five tests remain in the implementation module, reachable through the exported segment_meta module under cfg(test). This is structural debt, not a runtime High.

Evidence: [crates/shamir-wal/src/segment_meta.rs:175](../../../../../crates/shamir-wal/src/segment_meta.rs#L175); [crates/shamir-wal/src/lib.rs:47](../../../../../crates/shamir-wal/src/lib.rs#L47); [crates/shamir-wal/src/tests/mod.rs:1](../../../../../crates/shamir-wal/src/tests/mod.rs#L1).

Grouping/duplicate: `style-claude-md.md#1`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — sync_now counts failed fsyncs as issued fsyncs

Status: `refuted`. Current risk: —.

The documented counter counts fsyncs issued, not successful durability events. Both leader and sync_now count attempts consistently. The named tests use healthy sinks and do not infer success from a failed attempt.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:148](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L148); [crates/shamir-wal/src/wal_group_commit.rs:310](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L310); [crates/shamir-wal/src/wal_group_commit.rs:355](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L355); [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:151](../../../../../crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L151).

<a id="review-notes-notify-and-success-l1"></a>

### Claim Notes/Notify-and-success-L1 — Notify enable-before-check, successful leadership release, and append_many queue atomicity are correct

Status: `not-applicable`. Current risk: —.

These source-level guarantees hold: subscription precedes done-check, empty release occurs under pending lock, and append_many pushes its entire batch under one lock. They do not establish cancellation safety or failure-path liveness.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:189](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L189); [crates/shamir-wal/src/wal_group_commit.rs:237](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L237); [crates/shamir-wal/src/wal_group_commit.rs:272](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L272).

<a id="review-notes-dirty-flag-interleavings"></a>

### Claim Notes/dirty-flag-interleavings — Background-fsync dirty-flag interleavings are safe

Status: `refuted`. Current risk: —.

The file mutex protects the filesystem operation, not the coordinator's later false store. A newer Buffered write can set dirty after the fsync and before that store, losing the next timer flush.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:300](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L300); [crates/shamir-wal/src/wal_group_commit.rs:356](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L356); [crates/shamir-wal/src/wal_group_commit.rs:358](../../../../../crates/shamir-wal/src/wal_group_commit.rs#L358).

Grouping/duplicate: `concurrency-lockfree.md#4`. This row is not another independent defect.

<a id="review-notes-memsink-truncation-parity"></a>

### Claim Notes/MemSink-truncation-parity — MemSink batch-max tagging and engine truncation coverage

Status: `not-applicable`. Current risk: —.

Frames carry batch maximums and are retained above the durable ceiling; zero-tag batches remain pinned. Registered engine tests assert WAL collapse and retained readable values. This is conservative positive-version batching, not per-entry exact reclamation.

Evidence: [crates/shamir-wal/src/wal_sink.rs:136](../../../../../crates/shamir-wal/src/wal_sink.rs#L136); [crates/shamir-wal/src/wal_sink.rs:188](../../../../../crates/shamir-wal/src/wal_sink.rs#L188); [crates/shamir-engine/src/tx/tests/mod.rs:38](../../../../../crates/shamir-engine/src/tx/tests/mod.rs#L38); [crates/shamir-engine/src/tx/tests/truncation_tests.rs:120](../../../../../crates/shamir-engine/src/tx/tests/truncation_tests.rs#L120).

<a id="review-notes-stale-sidecar-handling"></a>

### Claim Notes/stale-sidecar-handling — Stale-sidecar handling is thoroughly tested and safe

Status: `partially-fixed`. Current risk: `medium`.

Successful removal and absent/corrupt/torn fallbacks have sensitive registered assertions. Removal errors are nevertheless swallowed, so a stale valid sidecar can survive the alleged fix; these failure cases are not covered.

Evidence: [crates/shamir-wal/src/tests/segment_set_tests.rs:791](../../../../../crates/shamir-wal/src/tests/segment_set_tests.rs#L791); [crates/shamir-wal/src/tests/segment_set_tests.rs:890](../../../../../crates/shamir-wal/src/tests/segment_set_tests.rs#L890); [crates/shamir-wal/src/tests/segment_set_tests.rs:924](../../../../../crates/shamir-wal/src/tests/segment_set_tests.rs#L924); [crates/shamir-wal/src/segment_meta.rs:164](../../../../../crates/shamir-wal/src/segment_meta.rs#L164).

## Corrections and qualified non-findings

- Count five inline segment_meta tests, not four.
- Do not describe all retry I/O failures as unlogged: WalSegment logs failed write_all and fsync outcomes.
- The dirty-interleaving non-finding contradicts the actual coordinator clear race.
- A merely synchronous failing Mem append does not deterministically create the stranded-follower interleaving; a test needs an explicit leader pause/release seam.
- All proof here is source-level; no test execution or historical Red phase was verified.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wal — Correctness & TDD-coverage

## Summary
Happy paths and the task-#500 sidecar lifecycle are unusually well tested (each sidecar
crash window has a dedicated regression test), but the fault paths are not: the
group-commit circuit breaker releases leadership outside the `pending` lock and can
strand parked committers forever — a direct violation of the module's own documented L1
liveness invariant and of CLAUDE.md's "hangs are bugs" rule. Two audit fixes (§1.5
dirty-flag restore, §2.4 startup `PermissionDenied`) are guarded only by tests that
simulate the fix instead of executing it, and the §1.3 file-sink mid-flight-failure
retry path has no test at all because no fault knob exists for `write_all`/`sync` on the
`File` sink. Remaining edge-case hazards: `repair_torn_tail` conflates crash tails with
mid-file corruption, and frame-end arithmetic can overflow `usize` on 32-bit targets
given a corrupt length header.

## Findings

### 1. Circuit breaker can strand parked appenders indefinitely (L1 violation, hang)
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:327-330` (breaker exit), park loop at
  `:189-197`, push+CAS at `:176-186`
- **Severity:** high
- **Issue:** `lead_until_drained` implements the documented L1 pattern ("leadership is
  released under the SAME `pending` lock as `push` … no entry is ever stranded") only on
  the SUCCESS exit (`:272-277`: empty-check + `flushing.store(false)` inside the lock).
  The circuit-breaker exit (`:328`) does a plain `flushing.store(false)` with NO lock and
  NO re-check of `pending`, then returns. Any committer that pushed to `pending` during
  the leader's failed `append_batch`/`sync` await already lost its CAS (flushing was
  still true), so it is parked on its `Waiter`; the leader returns without draining or
  failing that entry. Nobody will call `complete()` on it.
- **Failure scenario:** Tasks A and B commit concurrently. A wins the CAS and leads; B
  pushes during A's in-flight window and parks. The window's write (or fsync) fails (real
  ENOSPC / fsync error; on the File sink this requires the rotate-and-retry-once at
  `segment_set.rs:265-270` to also fail). A's batch waiters get `ok=false`, then A hits
  the breaker and stores `flushing=false`. B is parked forever: `flushing` is false so no
  leader exists, but B is not appending — it is awaiting. On a quiescent system (exactly
  the post-disk-error state) B hangs indefinitely — a per-connection hang on the
  production commit path (`RepoWalManager::begin_grouped` → `WalGroupCommit::append`).
  B only recovers if some FUTURE append happens to elect a new leader that drains B's
  stale entry (possibly succeeding, long after B's caller timed out at a higher layer).
- **Suggested fix:** Make the breaker exit symmetric with the success exit — under the
  `pending` lock, store `flushing=false` and `mem::take` any stragglers in the same
  critical section, then complete them with `ok=false` (surfacing the error beats
  hanging; a pusher that arrives after the store sees `flushing==false` and leads
  itself):

  ```rust
  if !write_ok || (needs_fsync && !sync_ok) {
      let stranded = {
          let mut p = self.pending.lock().await;
          self.flushing.store(false, Ordering::Release);
          std::mem::take(&mut *p)
      };
      for (_, _, _, w) in stranded { w.complete(false); }
      return;
  }
  ```

  TDD (Red first): extend the `Mem` fault knob to fail ≥2 consecutive `append_batch`es,
  spawn A (leader, fails) and B (pushes behind it), and assert B's `append` resolves
  (Err) inside `tokio::time::timeout` — pre-fix this test hangs/timeouts, post-fix it
  fails fast.

### 2. Vacuous §1.5 regression test — the restore-on-failed-fsync branch never executes
- **File:** `crates/shamir-wal/src/tests/wal_group_commit_tests.rs:294-332` (esp.
  `:316-331`); production branch `wal_group_commit.rs:394-403`
- **Severity:** medium
- **Issue:** `dirty_flag_restored_after_failed_fsync` claims to be the §1.5 regression
  test, but its "failed sync" is performed by the test itself: `gc.set_dirty()` …
  `gc.set_dirty()` ("mirrors `dirty_since_sync.store(true, …)` on Err") … `assert
  (gc.is_dirty())`. It re-implements the fix in the test body and asserts its own
  re-implementation. Deleting the restore at `wal_group_commit.rs:398` leaves this test
  green — it proves nothing about the production branch, violating the Red/Green/Refactor
  discipline (there was never a Red: no failing test could be written, because the only
  fault knob, `arm_fail_next_append`, arms `append_batch`, not `sync`, and `sync_now`
  cannot be made to fail on either sink).
- **Failure scenario:** A refactor of `spawn_background_fsync` (e.g. returning early on
  `Err`, or moving `take_dirty` after a successful-only sync) silently loses the
  unbounded data-at-risk retry that §1.5 closed; the suite stays green.
- **Suggested fix:** Add a `#[cfg(test)] fail_next_sync: AtomicBool` knob to `WalSink`
  (mirroring `arm_fail_next_append`), arm it, run `spawn_background_fsync` with a short
  interval, and assert the dirty flag is RE-set after the tick (poll on `is_dirty()`),
  then disarm and assert it clears on the next successful tick. Keep the existing
  happy-path assertions; delete the self-referential "simulate the error branch" lines.

### 3. §2.4 startup `PermissionDenied` hard-fail branch has zero coverage
- **File:** `crates/shamir-wal/src/wal_segment.rs:512-524`; placeholder test
  `tests/wal_segment_tests.rs:217-235`
- **Severity:** medium
- **Issue:** The audit-§2.4 fix (a `PermissionDenied` at startup must be a HARD error,
  not a silent `Ok(vec![])` that skips durable records) exists only as the
  `tolerate_permission_denied == false` arm of `replay_inner`. The only test touching the
  `_at_startup` variants (`startup_replay_api_surface_exists`) exercises the healthy
  path on both variants; its own doc concedes "the test validates the API surface
  exists". The regression the audit closed (empty-WAL-on-ACL-denial) is therefore
  untested: flipping the flag back to `true` at the startup call sites passes the suite.
- **Failure scenario:** A future edit to `SegmentSet::replay`/`open` reuses the tolerant
  `replay()`/`replay_sealed()` variants "for consistency"; an antivirus/ACL-held segment
  silently replays as empty at boot and recovery skips durable commits — the exact
  silent-data-skip §2.4 was filed against, with no test failure.
- **Suggested fix:** Factor the open-error classification out of `replay_inner` into a
  pure helper — e.g. `fn classify_open_err(e: &io::Error, tolerate_perm_denied: bool) ->
  OpenOutcome { NotFound, ToleratedDenied, HardDenied, Fatal }` — and unit-test all four
  kinds against all flag combinations (no filesystem needed). Optionally add a
  `#[cfg(unix)]` chmod-000 test for the real arm.

### 4. `repair_torn_tail` silently amputates the valid suffix on a complete-but-CRC-bad frame, mislabeled as "torn tail"
- **File:** `crates/shamir-wal/src/wal_segment.rs:377-395` (break conditions), `:399-411`
  (truncate), `:419-423` (warn); contrast `replay_inner`'s sealed-CRC loud error at
  `:546-563`
- **Severity:** medium
- **Issue:** The repair loop breaks identically on (a) an INCOMPLETE trailing frame
  (`frame_end > buf.len()` — a genuine crash tail; silent truncate is the design) and
  (b) a COMPLETE frame whose CRC mismatches (sub-frame-torn page-cache write or disk
  bit-rot). In case (b) it still truncates the file at that boundary at open time —
  permanently destroying every valid frame after it — and logs only a `warn` worded
  "truncated torn tail". This is the same bytewise condition that audit §1.8 made a
  LOUD operator-facing error for sealed segments; for the active segment (which can hold
  fsync-acked level-3 commits) it is silent, permanent, and mislabeled. Note the
  truncation itself is necessary (appends after a corrupt frame would be stranded on
  replay) — the gap is the missing distinction and escalation, not the truncate.
- **Failure scenario:** Bit-rot flips one payload byte in the middle of an active
  segment. On next open, `repair_torn_tail` deletes the entire valid suffix (potentially
  many acked commits) before `replay` ever runs — `replay_inner`'s warn-and-keep-prefix
  path for the active segment can never observe it (repair runs first at open, and
  `SegmentSet::replay` is startup-only per §2.4). No ERROR-level signal reaches the
  operator; the warn says "torn tail", implying an expected crash artifact.
- **Suggested fix:** Track WHY the loop broke. For the CRC-mismatch case keep the
  truncate (append-path correctness requires it) but log at `error!` with "CRC mismatch
  (complete frame) — on-disk corruption in ACTIVE segment; valid suffix of N bytes
  truncated" so the data loss is operator-visible and distinguishable from a crash tail.

### 5. §1.3 file-sink mid-flight append failure → rotate → retry-once path is untested (no fault knob for `File` `write_all`)
- **File:** `crates/shamir-wal/src/segment_set.rs:255-274` (Err branch); contrast the
  tested pre-poisoned branch at `:232-236`; related untested invariant
  `wal_segment.rs:214`
- **Severity:** medium
- **Issue:** Every File-sink poison test drives the PRE-poisoned branch via
  `mark_poisoned()` (`poison_rotation_sheds_stale_sidecar`,
  `poisoned_segment_rejects_further_appends`). The other branch — a live append FAILING
  mid-flight, the segment self-poisoning + rolling back via `set_len(pre_batch_offset)`,
  `rotate_after_poison`, and the batch being retried ONCE on the fresh file — is never
  executed by any test, because `arm_fail_next_append` exists only on the `Mem` sink and
  no knob can fail a real `write_all`. Untested consequences include: (a) the
  rollback-truncate-to-`pre_batch_offset` path (only its AFTERMATH — a hand-appended torn
  tail — is tested), (b) `max_committed` being `fetch_max`-ed at `wal_segment.rs:214`
  BEFORE the write, so the poisoned sealed segment's `SealedMeta.max_version` can exceed
  its actual content (conservative — over-retention — but undocumented in
  `append_batch`'s doc and unasserted anywhere), and (c) the returned `last_seq` after a
  retry referring to the NEW segment.
- **Failure scenario:** A regression in the rollback offset (e.g. truncating to 0, or to
  `bytes_written` AFTER the failed `fetch_add`) or in the retry's seq/version accounting
  lands undetected; §1.3's "breaks the next-leader-writes-to-the-same-poisoned-segment
  bug" claim rests on code reading alone.
- **Suggested fix:** Add a test-only fault seam on the File path (e.g.
  `#[cfg(test)] fail_next_write: AtomicBool` checked inside `WalSegment`'s
  `spawn_blocking` before `write_all`, mirroring the Mem knob) and a Red/Green test:
  first append fails (segment poisons + rolls back), retry lands on the fresh segment,
  replay shows exactly the retried frames and nothing of the failed batch, and the sealed
  poisoned segment's `max_committed` equals its actual content's max (or document the
  overstatement as intentional).

### 6. Per-segment seq counter restarts at 0 on reopen — returned `last_seq` can collide with pre-existing frames
- **File:** `crates/shamir-wal/src/wal_segment.rs:177` (`next_seq: AtomicU64::new(0)` in
  `open`, not seeded from existing frames — unlike `bytes_written` at `:179`);
  `wal_sink.rs:130` (same pattern in `MemSink`)
- **Severity:** low
- **Issue:** `append_batch` documents "Returns the seq assigned to the LAST entry", and
  `append_then_replay_roundtrips` asserts `last_seq == 2` — but only on a FRESH segment.
  Reopening a segment that already holds k frames hands out seqs 0..k-1 again, colliding
  with the pre-existing frames' seqs; `WalSegment::open` deliberately seeds
  `bytes_written` from the on-disk length, so the asymmetry is inconsistent. No
  production consumer today (`WalGroupCommit::lead_until_drained` discards the value via
  `.is_ok()`; `RepoWalManager` never sees it), which makes this a latent API trap rather
  than a live bug.
- **Failure scenario:** A future caller uses the returned seq as a per-segment entry
  handle (e.g. for targeted truncation or dedup); after any reopen the handle aliases an
  older frame.
- **Suggested fix:** Seed `next_seq` from the frame count computed during
  `repair_torn_tail`/open (or at minimum document on `append_batch` that seq is
  per-handle, not per-file, and stable only within one process's ownership).

### 7. Leader discards the underlying I/O error; a failed retry-on-fresh-segment is logged nowhere
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:290-294` (`is_ok()` drops the
  write `Err`), `:311` (same for sync), `:201`/`:261` (generic "wal group commit failed"
  surfaces to the caller); `segment_set.rs:270` (retry `?` propagates silently)
- **Severity:** low
- **Issue:** `SegmentSet::append_batch` logs the FIRST failure
  (`segment_set.rs:261-264`) but its retry's failure (`:270`, `active.append_batch(...).await?`)
  propagates unlogged into `lead_until_drained`, which reduces it to a boolean. The
  committer receives `"wal group commit failed"` with no cause; in the retry-failure
  case no log line anywhere carries the actual OS error (ENOSPC detail, path). This
  erodes CLAUDE.md's error-handling intent (propagate with `?`, keep the cause) even
  though the `DbError` type is correct.
- **Suggested fix:** Capture the `Err` in the leader (`match` instead of `is_ok()`),
  `log::error!` it once, and (optionally) thread it into the `Waiter` so `append`
  returns `Err(e)` rather than a string clone.

### 8. Corrupt length header overflows `usize` on 32-bit targets → panic in replay/repair
- **File:** `crates/shamir-wal/src/wal_segment.rs:380` and `:535`
  (`let frame_end = pos + 4 + len + 4;` with `len` up to `u32::MAX as usize`)
- **Severity:** low
- **Issue:** On a 32-bit target, `pos + 8 + len` can exceed `usize::MAX` (wraps in
  release, panics on overflow-check in debug). A wrapped-small `frame_end` passes the
  `frame_end > buf.len()` guard and the subsequent slicing indexes out of bounds — a
  corrupt-file-triggered `panic!`, which CLAUDE.md reserves for invariant violations
  only. 64-bit builds are safe (no overflow possible for `len ≤ u32::MAX`).
- **Failure scenario:** A flipped length header in a segment file on a 32-bit deployment
  crashes the process during `open`/`replay` instead of treating the frame as a torn
  tail.
- **Suggested fix:** Compute in `u64` (`(pos as u64) + 8 + len as u64`) or use
  `checked_add` and break on `None`/overflow — treat it exactly like a torn tail.

### 9. Inline `#[cfg(test)] mod tests` in `segment_meta.rs` violates the documented test layout
- **File:** `crates/shamir-wal/src/segment_meta.rs:175-218`
- **Severity:** low
- **Issue:** CLAUDE.md's test organisation rules ("One `tests/` directory per module",
  "`tests/mod.rs` is a manifest only", "Never embed `#[cfg(test)] mod tests { … }` inline
  inside implementation files") are followed by every other module in this crate
  (`src/tests/{active_key,segment_set,wal_entry_v2,wal_group_commit,wal_segment*}_tests.rs`
  + manifest). `segment_meta.rs` is the lone exception — its four decode tests are inline
  in the impl file (the doc even says "Split out so tests can exercise it directly",
  which a sibling test file satisfies equally well).
- **Suggested fix:** Move them to `src/tests/segment_meta_tests.rs` and add
  `pub mod segment_meta_tests;` to `src/tests/mod.rs`. Mechanical, no behavioural change.

### 10. `sync_now` counts failed fsyncs as issued fsyncs
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:355-362` (counter incremented at
  `:360` regardless of `res`)
- **Severity:** nit
- **Issue:** `fsync_count` is documented as "Count of fsyncs this coordinator issued
  (for the batching test)" but `sync_now` increments it even when the fsync fails, so the
  batching assertions (`synced_fsyncs_are_batched`, `background_fsync_fires_for_buffered`)
  conflate attempts with successes. Harmless today (tests only need ≥ thresholds), but a
  future strict-equality assertion would be wrong on failure paths.
- **Suggested fix:** Increment only on `res.is_ok()`, or document the counter as
  "attempts".

## Notes / non-findings (verified while reading, no action requested)
- `WalGroupCommit`'s Notify park loop (enable-before-check) and the L1 release-under-lock
  on the SUCCESS exit are correct; `append_many`'s one-lock-push → single-window
  atomicity argument holds (a `mem::take` cannot observe a partial batch).
- The background-fsync dirty-flag interleavings are safe: `WalSegment`'s file mutex
  serializes write/fsync, and `dirty.store(true)` happens after the write completes.
- `MemSink`'s batch-max frame tagging makes `truncate_below` drop exactly when every
  frame in the batch is ≤ durable (conservative otherwise) — parity with the File sink is
  correct; the Mem path is covered end-to-end by `shamir-engine`'s `truncation_tests.rs`
  even though it has no crate-local unit test.
- Stale-sidecar handling (re-activation shed in `open`, poison-rotation shed, corrupt /
  torn / absent fallbacks) is thoroughly tested — task #500's coverage is a model of the
  Red/Green discipline this review found lacking on the §1.5/§2.4/§1.3-file paths.

</details>
