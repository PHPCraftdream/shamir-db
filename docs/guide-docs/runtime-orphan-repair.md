# Runtime orphan repair

The background drainer repairs events from retained WAL entries and processes admitted work in ascending commit-version order. An entry may arrive late: the drainer window is bounded, and a later pass can recover dropped entries from the WAL. Transient acknowledgement or flush failures leave the orphan registered, preserve its evidence, increment failure metrics, and are logged for retry. Unknown WAL evidence and a closed changefeed writer use the conservative GAP fallback; a full writer no longer does — see the channel-full repair below.

When an orphan is registered but absent from both the retained gate evidence and the drainer window, the empty-window path performs WAL recovery before deciding the outcome. If no evidence is recovered, it records a minimal GAP and settles only the next contiguous orphan after the durable watermark; it does not settle an ordinary pending or missing version. The GAP is recorded before marking the version durable and removing the orphan registration.

Repair runs on the background drainer, not the commit hot path. Recovery checkpoints may add two FIFO CURRENT-J checkpoints; normal commit behavior remains untouched. Recovered journal keys are processed in ascending order, and the completion watermark gate prevents a late physical enqueue from being skipped. Normal draining retains its existing two-flush flow.

## Channel-full journal repair (S5c-X PR5b)

When the changefeed journal channel (4096 slots) is full, the event is no longer dropped and gapped. The commit version is recorded in a lock-free in-memory repair-pending set (`shamir_tx::changefeed::RepairPending`: an scc map with the Fx hasher, an `AtomicUsize` depth gauge, and an `AtomicU64` min-version), and the background drainer re-enqueues the event rebuilt from the retained WAL entry (`event_from_wal_entry`) once the channel has room. The normal enqueue path is byte-for-byte unchanged; the commit hot path only does work in the rare channel-full branch, and that work is a single set insert.

Ordering (S5b/N1) is preserved by clamping: journal-writer barriers are clamped below the oldest repair-pending version (`clamp_barrier`), and the runtime clean-horizon checkpoint ceiling (PR2 `settle_and_truncate`) is clamped the same way, so no bounded reader can silently skip the missing version. The I2 retention gate retains the WAL segment holding a repair-pending version until the repaired event is durably checkpointed.

A durable GAP — with a reason counter — is declared only when repair is impossible: the WAL entry is no longer retained or not derivable (legacy format), or the journal writer is closed/dead. PR2's retention policy (`refuse_writes` / `declare_gap`) stays authoritative when the retention limit itself would have to be violated.

Repair is idempotent: repairing a version whose event is already in the journal produces no second put; a present-but-differing event is a loud error (counter + log), never an overwrite. Counters cover deferred (channel-full), repaired, and gap-declared events with their reasons, plus mismatch errors; a gauge exposes repair-pending depth. A `cfg(test)`-only seam disables the barrier/ceiling clamp so tests can prove the clamp is load-bearing.

This composes with the orphan replay above: an orphan repair whose enqueue finds the channel full now also lands in the repair-pending set instead of declaring a gap, and the drainer retries it on a later pass.

The persistent crash tests cover truncation, seal/unlink and two reopen cycles, plus a bypass-gate negative control. These tests remain pending CI confirmation.
