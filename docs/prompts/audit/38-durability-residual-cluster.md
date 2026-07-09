Task: MEDIUM-durability residual cluster — 9 independent findings from
`docs/audits/2026-07-06-durability-storage-wal-tx.md`, section 1 (1.5-1.9)
and section 2 (2.2-2.6). Task #494 in the ongoing audit-remediation
campaign.

These are INDEPENDENT findings — fix each on its own merits. Per this
campaign's established pattern, if any single finding turns out to be
genuinely high-complexity/structural beyond what's tractable here, STOP
on that ONE finding, document your investigation + a follow-up task
description, and continue with the others — do not let one hard finding
block the rest.

## Finding 1.5 — Buffered-commit 250ms data-at-risk window unbounded on fsync failure

`crates/shamir-wal/src/wal_group_commit.rs:315-330` (`spawn_background_fsync`,
confirm current line numbers): `take_dirty()` clears the dirty flag BEFORE
attempting fsync, and `let _ = g.sync_now()` swallows the error. After one
failed fsync, dirty state is lost with no retry until the next append; on
a quiescent system the loss window becomes UNBOUNDED with zero logging.

Fix: on `sync_now()` error, restore the dirty flag (`dirty_since_sync.store(true, ...)`),
log the error, and treat repeated fsync failures as cause for segment
rotation (related to a prior finding 1.3, already fixed in an earlier
campaign task if it exists — check git log/grep for prior 1.3 fixes
before assuming this is greenfield).

## Finding 1.6 — begin_grouped_many: partial append + "abort all" resurrects rejected transactions

`crates/shamir-tx/src/repo_wal_manager.rs:84-97` (append one-by-one, `?`
mid-loop) + `crates/shamir-engine/src/tx/group_commit.rs:299-319` ("WAL
begin failed — nothing durable", aborts all participants).

Scenario: batch leader writes 5 entries, #3 fails (disk full) — entries
#1-2 are ALREADY durable in the segment; all 5 clients get an error,
versions marked Aborted, but on restart recovery treats "durable =
committed" (`recovery.rs:262-264`) and replays #1-2 — transactions the
client was explicitly told failed get materialized anyway.

Fix: make the batch-append atomic at the window level (one `append` call
carrying all payloads via the group commit interface, which nearly
supports this already) — investigate `WalGroupCommit`'s actual API before
assuming a rewrite is needed.

## Finding 1.7 — Recovery marker regression: parallel commits can write a stale commit_version

`crates/shamir-engine/src/tx/commit_phases.rs:521-529`: each committer
writes its OWN version, not the running maximum — parallel Phase 6.5
writers can write 9 after 10 already landed, regressing the marker.

Fix: write `gate.last_committed()` (the monotonic maximum) instead of the
raw `commit_version`, or CAS-max when writing the marker.

## Finding 1.8 — Replay silently discards a valid sealed-segment tail after one CRC failure

`crates/shamir-wal/src/wal_segment.rs:231-238` (confirm lines): for a
SEALED segment (per invariant I4, fully fsynced, torn tail impossible by
construction), a CRC mismatch mid-segment means disk corruption — but the
code logs a `warn!` and discards ALL subsequent valid frames. The frame
format `[len][payload][crc]` has no magic/seq, so resynchronization after
a single corrupt frame is impossible in principle.

Fix: for sealed segments, CRC failure must be a LOUD recovery error (an
operator decision), not a silent warn. Adding magic+seq to the frame
format to allow skipping a single corruption is a LARGER, format-changing
fix — if that part looks like it needs a WAL format version bump,
implement JUST the "loud error instead of silent warn" half here, and
defer the magic+seq resync capability as its own follow-up task (name it
explicitly in your report) rather than attempting a format change in this
pass.

## Finding 1.9 — No fsync of the WAL directory after segment create/rotation (Linux)

`crates/shamir-wal/src/segment_set.rs:214` (rotation opens a new file),
`wal_segment.rs:80-106`. `sync_all()` on the file doesn't guarantee the
directory ENTRY is durable — on ext4/xfs after power loss a freshly
created segment can be missing from the directory listing entirely, so
even Synced-acked writes in it are lost and replay won't even see the
file existed.

Fix: after segment creation/rotation, fsync the parent directory
(unix: `File::open(dir)?.sync_all()`). This is platform-specific (no-op
or harmless on Windows) — `#[cfg(unix)]` gate it, confirm it compiles and
is inert on Windows (this repo's primary dev platform per CLAUDE.md).

## Finding 2.2 — MemBuffer background flush silently swallows errors; scans silently lose dirty tail

`crates/shamir-storage/src/storage_membuffer.rs:263` — `let _ =
Self::drain_once(...)` with zero logging (disk full → dirty grows
unboundedly, zero signal). Worse: `iter_stream`/`scan_prefix_stream`/range
streams (lines ~529-532, 546-551, 569-574, 592-597, confirm current
lines) do `drain_once(...).unwrap_or(0)` — on flush error the stream
silently serves stale data from `inner` WITHOUT the undrained dirty tail:
full scans (index rebuilds, doctor, copy_store during RENAME) silently
see stale data.

Fix: log the error in the background flusher (add a telemetry counter if
one exists in this codebase's convention, e.g. an AtomicU64 error
counter — check for an existing pattern before inventing one); in the
streams, propagate the drain error as a stream item/error instead of
swallowing it via `unwrap_or(0)`.

## Finding 2.3 — MemBufferStore::transact loses a concurrent write via dirty.remove(&k) instead of remove_if

`storage_membuffer.rs:643-649` (confirm lines): after `inner.transact`,
the key is removed from `dirty` UNCONDITONALLY. A concurrent `set(k)`
landing between `inner.transact` and the `remove` call puts a NEW value
into `dirty` — which then gets removed anyway, so it never reaches the
inner store; after a cache eviction or restart, durable state is the OLD
value. The nearby `drain_once` handles this correctly already (snapshot +
`remove_if`) — use that as the reference pattern.

Fix: either drop the unconditional removal entirely (drain_all at the
start already emptied these keys) or use `remove_if` with a value
comparison, matching `drain_once`'s existing correct pattern.

## Finding 2.4 — WalSegment::replay: PermissionDenied silently becomes Ok(vec![]) even at startup

`crates/shamir-wal/src/wal_segment.rs:202-210` (confirm lines). The
rationale (delete-pending during a concurrent truncate) is only valid for
a race in an already-running process. The SAME code path runs during
`SegmentSet::open`/recovery at STARTUP: a real ACL denial, or a file held
by an antivirus/backup process, becomes a silent "empty WAL" — recovery
silently skips durable records.

Fix: only tolerate `PermissionDenied` when truncation is genuinely
concurrent (a flag in `SegmentSet` marking "this path is claimed for
deletion"); on open/recovery, it must be a hard error.

## Finding 2.5 — Swallowed errors in recovery replay

`crates/shamir-engine/src/tx/recovery.rs:141` (confirm line) — broadcast
IndexPut: `let _ = tbl.info_store().set(...)` (the neighboring IndexDel
DOES propagate its error — asymmetric); `recovery.rs:192-199` (confirm
lines) — InternerOverlayMerge: `if let Ok(...)` on interner resolution,
`let _ = interner.touch_ind(...)`, `let _ = repo_interner.persist()` — an
interner-merge failure during recovery is silently swallowed.

Fix: propagate these errors; for broadcast branches across multiple
tables, collect the FIRST Err after attempting all tables (mirror
whatever existing pattern this codebase uses for that, e.g. a
`flush_buffers`-style "attempt all, return first error" helper — grep for
one before inventing a new pattern).

## Finding 2.6 — Interner load treats corruption as "skippable"

`interner_manager.rs:163-167` (confirm lines, a corrupt legacy blob
becomes "empty dictionary") and `:182-197` (a corrupt chunk is skipped;
a scan error just `break`s). Continuing with a truncated dictionary means
minting new ids over already-occupied ones = silent corruption of every
old record referencing those ids. Chunks have no checksum (the "checksums
everywhere" project goal isn't honored here — integrity is delegated to
fjall, but a decode failure is still treated as "skip it").

Fix: any decode failure while loading the interner must be a FATAL open
error, not a skip-and-continue. NOTE: this repo's earlier campaign work
(A5/A8/A11 in the concurrency-isolation cluster) built crash-safety
guarantees around `entries_after`/`last_persisted_len` — read that code
and its tests BEFORE changing failure behavior here, to confirm this
change doesn't interact badly with those existing invariants. If it does,
scope this one down and document why, per this task's general escape
valve.

## General verification requirement

For EACH finding you fix: add or confirm a regression test that would
FAIL without the fix (not vacuous). Where a finding requires touching
crash-recovery paths, check for and reuse existing test infrastructure in
`crates/shamir-wal/src/*/tests/`, `crates/shamir-engine/src/tx/tests/`,
`crates/shamir-storage/src/*/tests/` rather than inventing new harnesses.

## Test scope

```
./scripts/test.sh -p shamir-wal
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-storage
```

## Gate

```
cargo fmt -p shamir-wal -p shamir-tx -p shamir-engine -p shamir-storage -- --check
cargo clippy -p shamir-wal -p shamir-tx -p shamir-engine -p shamir-storage --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

For EACH of the 9 findings (1.5, 1.6, 1.7, 1.8, 1.9, 2.2, 2.3, 2.4, 2.5,
2.6 — note there are actually 10 numbered items since 1.5-1.9 is 5 items
and 2.2-2.6 is 5 items):
```
[Finding X.Y] Status: fixed / deferred
  > What changed + regression test added (if fixed)
  > OR: deferral reason + follow-up task description (if deferred)
```
Full test/gate results (exact commands + pass/fail) for whichever crates
were actually touched.
