<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-wal — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Leader cancellation and post-write rotation errors remain distinct High mechanisms. Error propagation is lossy; live replay undermines the startup-only assumption. The Result-returning best-effort directory helper is not itself a defect.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 14 | 0 | 0 | 1 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Cancellation or panic of the leader task wedges flushing forever — permanent WAL append hang

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Drop an elected append future while awaiting File work: neither normal release executes, flushing remains true and taken waiters disappear from pending without completion. 09d2e215 detached a different engine GroupCommit; cooperative server deadlines are counter-evidence to the claimed ordinary remote-timeout trigger.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:185](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L185); [crates/shamir-tx/src/repo_wal_manager.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-tx/src/repo_wal_manager.rs#L91); [crates/shamir-engine/src/repo/group_commit/group_commit.rs:74](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/repo/group_commit/group_commit.rs#L74); [crates/shamir-engine/src/query/batch/batch_execute.rs:779](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/batch_execute.rs#L779).

<a id="review-2"></a>

### Claim 2 — Seal-time fsync failure fails an already-successful append whose frames survive — acked-failed tx resurrected on replay

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A successful physical append crosses the threshold; seal fsync Err propagates, and the group reports failure without removing the written frames. commit.rs treats WAL failure as pre-commit abort, yet a later recovery can replay those frames. New-active open failure produces the same outcome even after successful old-segment fsync.

Evidence: [crates/shamir-wal/src/segment_set.rs:251](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L251); [crates/shamir-wal/src/segment_set.rs:367](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L367); [crates/shamir-wal/src/segment_set.rs:390](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L390); [crates/shamir-engine/src/tx/commit.rs:977](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/commit.rs#L977).

<a id="review-3"></a>

### Claim 3 — Group-commit layer discards the underlying error — waiters get a context-free generic Err

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Both append and sync causes are discarded through is_ok, and Waiter transports only booleans. Ordinary lower-layer failures have logs, but programmatic causes are unavailable.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:294](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L294); [crates/shamir-wal/src/wal_group_commit.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L311); [crates/shamir-wal/src/wal_group_commit.rs:261](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L261).

<a id="review-4"></a>

### Claim 4 — No thiserror error enum — stringly-typed errors, io::ErrorKind destroyed at the wrap boundary

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The shared thiserror DbError already offers Io and Codec, but WAL stringifies I/O into Storage and data decoding into Internal. Absence of a dedicated per-crate enum is not a rule violation; loss of typed causes is the actual issue.

Evidence: [crates/shamir-wal/src/wal_segment.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L155); [crates/shamir-wal/src/wal_segment.rs:315](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L315); [crates/shamir-storage/src/error.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L41).

<a id="review-5"></a>

### Claim 5 — SegmentSet::replay fronts sealed paths with a create-mode open — defeats documented tolerance and side-effect-creates files on a read path

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Between snapshot and sealed open, truncation can unlink the file; replay recreates it empty or errors on a denied open before any tolerant reader runs. Live gap recovery and background seeding reach this code. An existing valid positive sidecar means recreated files are not universally permanent zero pins.

Evidence: [crates/shamir-wal/src/segment_set.rs:435](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L435); [crates/shamir-wal/src/wal_segment.rs:150](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L150); [crates/shamir-engine/src/tx/drainer.rs:368](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L368); [crates/shamir-engine/src/tx/drainer.rs:1027](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L1027).

<a id="review-6"></a>

### Claim 6 — Error-path rollback target is captured outside the file lock — concurrent-append failure truncates a concurrent successful batch

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Rollback uses a pre-lock offset and runs after releasing the file guard; successful accounting is also outside the closure. Direct concurrent callers violate the ordinary single-leader precondition.

Evidence: [crates/shamir-wal/src/wal_segment.rs:220](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L220); [crates/shamir-wal/src/wal_segment.rs:248](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L248).

Grouping/duplicate: [concurrency-lockfree.md#2](concurrency-lockfree.md#review-2). This is not an additional independent defect.

<a id="review-7"></a>

### Claim 7 — §1.5 dirty-flag-restore regression test is tautological

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The test explicitly calls set_dirty to simulate the production action and never observes a failed background sync. Removing the production restore does not invalidate it.

Evidence: [crates/shamir-wal/src/tests/wal_group_commit_tests.rs:327](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_group_commit_tests.rs#L327).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — Untested error paths on the File sink

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Registration and bodies show no induced partial write/rollback failure, poison next-open failure, startup denial or missing-sidecar corrupt-sealed open.

Evidence: [crates/shamir-wal/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/mod.rs#L1); [crates/shamir-wal/src/tests/wal_segment_poison_tests.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_segment_poison_tests.rs#L48); [crates/shamir-wal/src/tests/segment_set_tests.rs:850](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/segment_set_tests.rs#L850).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-8-1"></a>

### Claim 8(1) — Real write-failure rollback, Windows fresh-handle workaround, and rollback-itself failure

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Manually appended tails and manually set poison do not execute write_all Err, fresh-handle rollback or nested set_len failure. A discriminating seam must write a real partial prefix first.

Evidence: [crates/shamir-wal/src/wal_segment.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L238); [crates/shamir-wal/src/wal_segment.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L249); [crates/shamir-wal/src/tests/wal_segment_poison_tests.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_segment_poison_tests.rs#L59).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-8-2"></a>

### Claim 8(2) — rotate_after_poison when opening the next segment fails

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

New-active open can fail before state publication, leaving the poisoned active unchanged. The registered poison test only exercises successful creation.

Evidence: [crates/shamir-wal/src/segment_set.rs:317](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L317); [crates/shamir-wal/src/tests/segment_set_tests.rs:882](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/segment_set_tests.rs#L882).

<a id="review-8-3"></a>

### Claim 8(3) — §2.4 startup PermissionDenied hard error

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The strict PermissionDenied branch exists; the healthy-file API smoke test cannot detect tolerant dispatch.

Evidence: [crates/shamir-wal/src/wal_segment.rs:512](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L512); [crates/shamir-wal/src/tests/wal_segment_tests.rs:218](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/wal_segment_tests.rs#L218).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-8-4"></a>

### Claim 8(4) — Sealed CRC loud error through SegmentSet::open without a sidecar

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Direct sealed CRC errors are tested, as are healthy absent-sidecar fallback and corrupt data with a valid sidecar. No test combines absent sidecar and corrupt sealed payload to require Err from SegmentSet::open.

Evidence: [crates/shamir-wal/src/segment_set.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L154); [crates/shamir-wal/src/tests/segment_set_tests.rs:557](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/segment_set_tests.rs#L557); [crates/shamir-wal/src/tests/segment_set_tests.rs:596](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/tests/segment_set_tests.rs#L596).

<a id="review-9"></a>

### Claim 9 — truncate_below partial failure discards progress and strands claimed files until restart

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

All candidates leave sealed before unlink. A hard error at path k returns immediately, discarding successful count and leaving k and later paths untracked until rescan. Already-durable data is not lost.

Evidence: [crates/shamir-wal/src/segment_set.rs:470](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L470); [crates/shamir-wal/src/segment_set.rs:511](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L511).

<a id="review-10"></a>

### Claim 10 — Corrupt on-disk data decoded as DbError::Internal — misclassifies corruption as a code bug

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Short input, bad magic, unknown versions and malformed bodies return Internal despite the existing Codec variant for serialization failures.

Evidence: [crates/shamir-wal/src/wal_entry_v2.rs:229](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L229); [crates/shamir-wal/src/wal_entry_v2.rs:252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_entry_v2.rs#L252); [crates/shamir-storage/src/error.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-storage/src/error.rs#L35).

<a id="review-11"></a>

### Claim 11 — fsync_parent_dir's DbResult&lt;()&gt; can never return Err

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Both implementations intentionally always return Ok after documented best-effort handling. Result does not promise that an Err case exists; narrowing its type is optional style. Failure handling does separately qualify unconditional Synced durability assurances.

Evidence: [crates/shamir-wal/src/wal_segment.rs:31](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L31); [crates/shamir-wal/src/wal_segment.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L67); [crates/shamir-wal/src/wal_segment.rs:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L71).

<a id="review-summary-startup-only-replay"></a>

### Claim Summary/startup-only-replay — Production replay is safe because its sole use is startup recovery

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The live drainer explicitly calls recover for an empty contiguous-prefix gap; its background task also recovers for initial seeding.

Evidence: [crates/shamir-engine/src/tx/drainer.rs:368](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L368); [crates/shamir-engine/src/tx/drainer.rs:1027](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L1027).

## Evidence and recipe corrections

- A flag-only Drop guard neither owns outstanding waiters nor waits for started blocking writes; double-drain is not automatically benign.
- Do not state that Synced callers necessarily fail on a poisoned segment: WalSegment::sync has no poison precheck and can return Ok.
- Differentiate seal-fsync failure, sidecar failure and new-active open failure. The latter need not poison the old active.
- A shared failure result needs owned/shared error transport: DbError contains io::Error and is not Clone. The historical clone-DbError slot recipe is incomplete.
- Swallowing hard unlink errors and returning Ok(success_count) can conceal reclaim failure. Preserve diagnostics/partial outcome and a retry policy.
- PermissionDenied is not uniquely a delete-pending signal. A live tolerant path needs a proven deletion/eligibility context, not blanket denial swallowing.
- Best-effort parent-directory synchronization prevents an unconditional power-loss guarantee when it fails; changing only the return signature does not address that contract.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wal -- Error handling & resource lifecycle

## Summary

The crate's happy-path `Result` discipline is good (`DbResult` everywhere, `?` propagation, a well-reasoned poison/quarantine model with rollback-on-write-failure, and carefully documented error tolerances), but two error-path correctness gaps stand out: a dropped/panicked group-commit leader permanently wedges the WAL append path, and a seal-time fsync failure converts an already-successful append into `Err` while its frames survive to replay (violating the crate's own §1.6 all-or-nothing contract). Error fidelity is weak across the board — the group-commit layer discards the underlying cause, and there is no thiserror error enum despite CLAUDE.md's mandate, forcing `io::ErrorKind` into strings and tests into substring matching. Several error paths (real file-sink write-failure rollback, rotate-on-poison open failure, §2.4 PermissionDenied hard-fail, open-time sealed CRC) have no test coverage, and the §1.5 dirty-flag-restore regression test is tautological — it re-implements the fix instead of driving a real failed fsync.

## Findings

### 1. Cancellation or panic of the leader task wedges `flushing` forever — permanent WAL append hang
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:180-186` (CAS + inline lead), `:273-275`, `:327-330` (the only two release points)
- **Severity:** high
- **Issue:** Leadership (`flushing == true`) is acquired by CAS in `append`/`append_many` and released only inside `lead_until_drained` — at the observed-empty exit (line 274) or the circuit-breaker exit (line 328). The leader runs inline on the caller's task (`self.lead_until_drained().await`, not spawned), so if that future is dropped at any internal `.await` (caller timeout, `select!` shutdown, runtime drop) or unwinds via panic (e.g. any of the `.lock().expect(...)` sites in `SegmentSet` firing while the leader holds leadership), `flushing` stays `true` with no `Drop` guard and no recovery path.
- **Failure scenario:** An engine committer task wrapped in `tokio::time::timeout` wins the CAS, then is cancelled while awaiting `sink.append_batch`. Every subsequent appender fails the CAS, parks on its `Waiter`, and is never notified — the commit path hangs until process restart. Entries already `mem::take`n from `pending` but not yet completed hang their waiters too. This is exactly the class of hang CLAUDE.md designates a bug ("hangs / SLOW / TIMEOUT are BUGS — hunt the root").
- **Suggested fix:** Release leadership via an RAII guard (a small struct borrowing the `AtomicBool` whose `Drop` does `store(false, Release)`); double-drain is benign because `mem::take` hands each entry to exactly one leader. Keep the under-lock release for the L1 no-stranded-pusher argument, and let the guard be the cancellation/panic backstop.

### 2. Seal-time fsync failure fails an already-successful append whose frames survive — "acked-failed" tx resurrected on replay
- **File:** `crates/shamir-wal/src/segment_set.rs:247-253` (`seal_and_rotate().await?` in the Ok arm), `:367` (`sealing.sync().await?`); waiters completed(false) at `wal_group_commit.rs:290-294`, `:302-306`
- **Severity:** high (narrow trigger: rotation boundary + fsync failure; but the resurrection is then the *expected* outcome of any later restart)
- **Issue:** In `SegmentSet::append_batch`, the write succeeds and only then does the size check fire `seal_and_rotate`, whose fsync failure propagates via `?` and turns the whole window's result into `Err`. The batch's bytes are already in the page cache, and the now-poisoned segment is *not* rolled back (rollback runs only on `write_all` failure) — it is later sealed-as-poisoned with its intact prefix replayable. The group-commit leader sees `write_ok == false` and completes **all** waiters — including `Buffered` ones whose level-2 tier was genuinely reached — with failure.
- **Failure scenario:** ENOSPC/eio fsync exactly when the active segment crosses `max_bytes`: the committer is told "wal group commit failed", aborts/reports failure to the client, yet recovery replays the entry as a committed tx. This is precisely the §1.6 property `append_many` documents ("no entry survives a partial write ... never replays a subset of a 'failed' batch") — violated at the rotation seam instead of the write seam. For `Synced` waiters an Err on a failed fsync is defensible (durability genuinely unknown); for `Buffered` waiters it contradicts the tier contract outright.
- **Suggested fix:** In the Ok arm, treat `seal_and_rotate` failure as a logged housekeeping error, not an append error: return `Ok(last_seq)` and let the poison flag force rotation on the next append (the sidecar-write failure two lines below is already swallowed for exactly this reason). Then `Buffered` waiters ack correctly, and `Synced` waiters still fail via their own subsequent `sink.sync()` against the poisoned segment — acks align with the actual tier outcome.

### 3. Group-commit layer discards the underlying error — waiters get a context-free generic `Err`
- **File:** `crates/shamir-wal/src/wal_group_commit.rs:290-294` (`append_batch(...).is_ok()`), `:309-315` (`sync(...).is_ok()`), `:198-202` / `:258-262` (`DbError::Storage("wal group commit failed")`)
- **Severity:** medium
- **Issue:** The leader reduces both the write and the fsync outcome to `bool`; the `DbError` (io kind, path, "poisoned" reason) is dropped without even a log at this layer, and waiters receive a fixed-string error with no cause chain. Upstream layers do log (`WalSegment` poison logs, `SegmentSet` retry log), so diagnosis depends on log scraping, and the engine→client error for the most critical write path in the DB cannot distinguish ENOSPC from EIO from a poisoned segment.
- **Failure scenario:** Operator sees every commit fail with "wal group commit failed"; the actual ENOSPC cause is only in server logs, and programmatic handling (retry-after-space vs fail-stop) is impossible from the error value.
- **Suggested fix:** Carry the error to the waiter (e.g. `err: Mutex<Option<DbError>>` on `Waiter`, completed by the leader on failure and returned/cloned to parked callers), and at minimum `log::error!` the dropped `Err` in `lead_until_drained` where it is currently silently discarded.

### 4. No thiserror error enum — stringly-typed errors, `io::ErrorKind` destroyed at the wrap boundary
- **File:** whole crate; e.g. `crates/shamir-wal/src/wal_segment.rs:155, 168, 236-263, 315, 519-525`; symptom in tests at `src/tests/wal_segment_poison_tests.rs:113-118`
- **Severity:** medium
- **Issue:** CLAUDE.md mandates `thiserror` for library error enums; shamir-wal defines none, hand-formatting everything into `DbError::Storage(String)`/`Internal(String)`. Notably `DbError::Io(#[from] std::io::Error)` already exists in shamir-storage, yet every io error here is stringified — `replay_inner` pattern-matches `ErrorKind::NotFound`/`PermissionDenied` *before* wrapping and then flattens the kind, so no downstream caller can ever match on it. The poison test's `msg.contains("poisoned")` assertion shows the practical cost: conditions are only identifiable by substring.
- **Failure scenario:** Engine/drainer code that needs to branch on "segment poisoned → rotate" or "NotFound → skip" cannot; each new consumer reinvents substring parsing, which breaks silently when messages are reworded.
- **Suggested fix:** Cheapest: wrap io errors with `DbError::Io` (lossless kind) and add a dedicated variant/message for poison; proper: a `WalError` thiserror enum (`SegmentPoisoned { path }`, `WriteFailed { path, #[from] io::Error }`, `SealedFrameCorrupt { path, offset }`, ...) converted to `DbError` at the crate boundary.

### 5. `SegmentSet::replay` fronts sealed paths with a create-mode open — defeats the documented NotFound/PermissionDenied tolerance and side-effect-creates files on a read path
- **File:** `crates/shamir-wal/src/segment_set.rs:435` (`WalSegment::open(meta.path.clone()).await?`) vs. the tolerance machinery at `src/wal_segment.rs:507-524` and the rationale at `:432-451`
- **Severity:** medium
- **Issue:** `WalSegment::replay`'s doc justifies its NotFound/Windows-delete-pending (`PermissionDenied`) tolerance by "a concurrent `truncate_below` can unlink one of the snapshot's paths between the snapshot capture and our open here" — but `SegmentSet::replay` calls `WalSegment::open`, which uses `OpenOptions::create(true)` (wal_segment.rs:150-155): (a) a *missing* sealed segment is silently re-created as an empty `.wal` (a filesystem mutation on a recovery/read path, which later reopens treat as a `max_version == 0` PIN segment that is never reclaimable, I5); (b) a Windows delete-pending path makes the *create-mode open itself* fail `PermissionDenied`, surfaced as a hard `DbError::Storage` — the exact error the one-layer-down tolerance was built to absorb. Grep confirms the tolerant non-startup variants (`replay()`, `replay_sealed()`) have no production callers at all — the tolerance machinery is unreachable through the only real path (`SegmentSet::replay` → `_at_startup` variants). Production is currently safe only because replay is startup-only (sole caller `RepoWalManager::recover`), which makes the tolerance's placement dead weight and its docs misleading about the live contract.
- **Failure scenario:** An operator/archival step removes a sealed `.wal` between the list snapshot and replay (or the pub API is used concurrently with the pub `truncate_below`): replay either fabricates an empty segment file or hard-fails with a confusing open error, instead of the documented skip.
- **Suggested fix:** Open sealed segments for replay read-only (`File::open`-based constructor, no `create`), letting the existing kind-matching in `replay_inner` govern; delete or actually wire the tolerant variants.

### 6. Error-path rollback target is captured outside the file lock — concurrent-append failure truncates a concurrent successful batch
- **File:** `crates/shamir-wal/src/wal_segment.rs:220` (`pre_batch_offset` read), `:249-252` (`set_len(pre_batch_offset)` rollback)
- **Severity:** medium (guarded in production by the single-leader model, but the repo's own committed bench deliberately violates that model)
- **Issue:** `pre_batch_offset` is read from `bytes_written` before entering `spawn_blocking`, not under the `file` mutex that serializes the actual writes. Two concurrent `append_batch` calls on the same segment: B captures a stale offset from before A's write; if B's `write_all` later fails, its rollback `set_len` truncates the file past A's already-acked, successfully-written frames. `WalSegment`'s doc claims "single-writer BY CONSTRUCTION", yet `benches/segment_set_lock.rs` explicitly stresses N tasks calling `SegmentSet::append_batch` (→ same `WalSegment`) concurrently and bills its `raw_append` scenario as a *correctness* stress test — that correctness claim only holds while no write fails.
- **Failure scenario:** Under the sanctioned bench shape (or any future non-leader caller), one ENOSPC failure destroys another committer's acked level-2 frames — silent loss of acked WAL data.
- **Suggested fix:** Derive the rollback boundary inside the file-lock critical section (maintain a last-good-offset field updated only while holding `file`), or fall back to `metadata().len()` under the lock on the error path — one extra syscall on an already-failing path is free.

### 7. §1.5 dirty-flag-restore regression test is tautological — the real restore branch is never executed by any test
- **File:** `crates/shamir-wal/src/tests/wal_group_commit_tests.rs:293-332` (restore simulated by the test itself at `:327`); untested branch at `src/wal_group_commit.rs:394-403`
- **Severity:** medium
- **Issue:** `dirty_flag_restored_after_failed_fsync` cannot trigger a real sync failure (its own comment admits this), so it "verifies" the fix by calling `gc.set_dirty()` — re-implementing the restore rather than exercising it. Deleting the `store(true)` restore inside `spawn_background_fsync` would leave this test green; the actual error branch (log + restore on `sync_now()` Err) has zero coverage.
- **Failure scenario:** A refactor drops the restore (e.g. returns early, or moves `take_dirty` after the sync): CI stays green, and quiescent systems again get the unbounded data-at-risk window the fix closed — with no test noticing.
- **Suggested fix:** Make the fsync failure injectable on a real path (extend the `Mem`/`File` fault knob to `sync`, behind `#[cfg(test)]`) and assert the flag is re-armed after a genuinely failed background tick.

### 8. Untested error paths on the File sink
- **File:** multiple (below)
- **Severity:** medium
- **Issue:** Four concrete error paths have no coverage anywhere in `src/tests/`:
  1. **Real write-failure rollback** (`wal_segment.rs:238-263`): no test drives an actual `write_all` failure on a file segment — the `set_len(pre_batch_offset)` rollback, the fresh write-mode-handle Windows workaround, and the rollback-itself-failed "unknown state" path are all untested; poison behavior is only simulated via `mark_poisoned()`, which skips rollback entirely (`wal_segment_poison_tests.rs:90-119`).
  2. **`rotate_after_poison` when opening the next segment fails** (`segment_set.rs:317`): untested — the "poisoned segment stays active, every append fails fast" behavior is asserted nowhere.
  3. **§2.4 startup `PermissionDenied` hard error** (`wal_segment.rs:512-524`): the `tolerate_permission_denied == false` branch with an actual denial is unexecuted — acknowledged in the test's own comments (`wal_segment_tests.rs:200-222`, "we cannot easily force PermissionDenied portably").
  4. **Sealed CRC loud error through `SegmentSet::open`** (`segment_set.rs:154-156`): `wal_segment_tests.rs:156-198` proves `replay_sealed()` errors on a corrupt sealed segment, but no test covers corrupt sealed data with *no sidecar* reaching `open` and failing loudly — the composition (fallback → `replay_sealed_at_startup` → `Err` out of `open`) is unverified.
- **Suggested fix:** Add a `#[cfg(test)]` fault-injection knob for the file `write_all`/`sync_all`/open of a chosen path (the `MemSink::fail_next_append` pattern already shows the house style), then cover 1–4; even a platform-gated (`#[cfg(unix)]` chmod-based) test for 3 would be stronger than the current API-surface smoke test.

### 9. `truncate_below` partial failure discards progress and strands claimed files until restart
- **File:** `crates/shamir-wal/src/segment_set.rs:511-515`
- **Severity:** low
- **Issue:** Claim-then-delete removes entries from `sealed` under the lock *before* unlinking; if the unlink of file #k fails with a hard (non-NotFound, non-PermissionDenied) error, the `Err` return discards the count of already-deleted files, and files #k+1..n — claimed but never unlinked — are untracked for the rest of the process lifetime (only a reopen rescans them). The comment block documents the data-safety rationale (idempotent replay), which holds, but the *caller-visible* semantics on partial failure are lossy: the drainer learns nothing about what was reclaimed.
- **Suggested fix:** On a hard unlink error, log and continue the loop (counting successes) instead of returning early — a leaked file is already documented as harmless, so failing the whole call buys nothing; or return the partial count alongside the error.

### 10. Corrupt on-disk data decoded as `DbError::Internal` — misclassifies corruption as a code bug
- **File:** `crates/shamir-wal/src/wal_entry_v2.rs:229, 232-235, 241, 252-255`
- **Severity:** low
- **Issue:** `WalEntryV2::decode` failures (bad magic, short input, unknown version, bincode errors) are returned as `DbError::Internal("wal_v2 decode: ...")`. These originate from *persisted bytes*, i.e. data corruption or format drift — the existing `DbError::Codec`/`Storage` taxonomy fits; `Internal` signals a programmer bug and will misroute operator triage (file a bug vs. restore from backup), especially since these errors surface from `replay` where a CRC check has already passed (a frame whose payload is a valid CRC but garbage bytes decodes to this path).
- **Suggested fix:** Return `DbError::Codec` (or a typed corruption variant per finding 4) from `decode`; reserve `Internal` for encode-side/logic failures.

### 11. `fsync_parent_dir`'s `DbResult<()>` can never return `Err`
- **File:** `crates/shamir-wal/src/wal_segment.rs:41-68`
- **Severity:** nit
- **Issue:** Both failure modes are logged at `warn` and swallowed (a documented, reasonable degradation decision), so the signature promises an error path that does not exist — and on filesystems where directory fsync returns EINVAL, every new segment creation emits a warn, a potential log-spam source on network mounts.
- **Suggested fix:** Return `()` and keep the doc comment, or downgrade the EINVAL-family log to `debug` with a once-per-path latch.

</details>
