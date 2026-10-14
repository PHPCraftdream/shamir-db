# Runtime orphan repair

The background drainer repairs events from retained WAL entries and processes admitted work in ascending commit-version order. An entry may arrive late: the drainer window is bounded, and a later pass can recover dropped entries from the WAL. Transient acknowledgement or flush failures leave the orphan registered, preserve its evidence, increment failure metrics, and are logged for retry. Unknown WAL evidence and a closed or full changefeed writer use the conservative GAP fallback.

When an orphan is registered but absent from both the retained gate evidence and the drainer window, the empty-window path performs WAL recovery before deciding the outcome. If no evidence is recovered, it records a minimal GAP and settles only the next contiguous orphan after the durable watermark; it does not settle an ordinary pending or missing version. The GAP is recorded before marking the version durable and removing the orphan registration.

Repair runs on the background drainer, not the commit hot path. Recovery checkpoints may add two FIFO CURRENT-J checkpoints; normal commit behavior remains untouched. Recovered journal keys are processed in ascending order, and the completion watermark gate prevents a late physical enqueue from being skipped. Normal draining retains its existing two-flush flow.

The persistent crash tests cover truncation, seal/unlink and two reopen cycles, plus a bypass-gate negative control. These tests remain pending CI confirmation.
