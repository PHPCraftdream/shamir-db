# Release 1 deferred items

## `shamir-lang` transactional batch commit cancellation

A wall-clock timeout can drop the in-flight gateway `execute` future during `commit_tx`; the procedure may report a timeout even though the transaction committed. A timeout is therefore not proof of rollback. This risk applies to the single-`db_execute` `batch_tx()` scope and is not resolved by that feature.

**Deferred:** consider running the transactional engine call in a spawned task and joining it to completion rather than dropping it with the guest-call timeout. Do not detach the task: it must remain joined so transaction outcome and resource lifetime are accounted for. This conflicts with the current stage-2 no-detached-work rule and requires a separate design decision.
