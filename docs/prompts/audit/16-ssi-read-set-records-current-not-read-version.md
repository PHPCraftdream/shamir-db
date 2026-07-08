Task: HIGH-concurrency — SSI read-set records the cell's CURRENT version
instead of the version actually observed by the read, letting
Serializable transactions commit on stale data with no detected
conflict (audit finding A3, `docs/audits/2026-07-06-concurrency-engine.md`).

## Where

`crates/shamir-engine/src/table/table_manager_streaming.rs`:

- `read_one_tx` (~line 331-398): line 345-349
  ```rust
  let version = self
      .mvcc_store
      .as_ref()
      .map_or(0, |mvcc| mvcc.version_of(key.as_ref()));
  tx.record_read_shared(self.table_token(), key.clone(), version);
  // ... later, reads the SNAPSHOT value:
  match mvcc.get_at(key.as_ref(), tx.snapshot_version).await? { ... }
  ```
- `read_one_tx_bytes` (~line 410-451): the identical pattern at
  line 421-425 (`version_of` recorded) vs. line 441
  (`get_at(key, tx.snapshot_version)` — the actual value read).
- `record_scan_reads` (~line 244-267): same pattern at line 265-266
  inside a Serializable-only scan-read-tracking loop.

Check `crates/shamir-tx/src/tx_context.rs:530-541` (`validate_read_set`)
to confirm the exact comparison it performs (per the audit:
`current_version > version_seen` decides conflict) — read this before
writing the fix, since the fix must integrate cleanly with whatever
comparison this function actually does today (line numbers may have
drifted since 2026-07-06).

## Why this is HIGH

`version_of(key)` returns the cell's version **right now** at the
moment of the call — which can be STRICTLY NEWER than
`tx.snapshot_version` if a concurrent committer publishes a newer
version between the `version_of` call and the `get_at` call (or simply
because the cell was already ahead of this tx's snapshot before the
read even started). The value actually read, however, comes from
`get_at(key, tx.snapshot_version)` — the OLD, snapshot-consistent
value. Recording `version_of`'s current (newer) version into the
read-set means `validate_read_set` sees `current_version == version_seen`
(both equal to the newer version) and treats this as "no conflict",
even though the transaction actually read STALE data relative to what
it recorded having read.

Concrete interleaving from the audit:
1. B (Serializable, `snapshot_version = 10`) is about to read key `k`.
2. A commits `k` at version 11 (Phase 5a — `mvcc.publish_cell(k, 11)`).
3. B calls `version_of(k)` → returns `11` (A's commit already
   published) → `record_read(k, 11)`. B then calls
   `get_at(k, 10)` → returns the OLD value (v10), since `get_at` is
   snapshot-gated to `tx.snapshot_version = 10`.
4. B commits. `validate_read_set` compares `current_version(k) = 11`
   against `version_seen = 11` (what was recorded) → `11 > 11` is
   false → **no conflict detected, B commits**.
5. B has committed having read a value that was already stale by the
   time it read it (A's v11 was already visible), with the exact
   conflict-detection mechanism (SSI read-set validation) blind to it
   because the recorded version doesn't correspond to the value
   actually read. This breaks first-committer-wins even under fully
   serialized validation.

The audit also notes this is "проверено на практике": some existing
tests (e.g. `acceptance_tests.rs:495` area — check current location)
manually record the read as `tx.snapshot_version` (the CORRECT
semantics) rather than going through the actual production code path
(`version_of`), meaning the production bug has been masked by tests
that don't exercise the real call chain. Confirm this observation
still holds and note it in your report; do not "fix" the test to
match the buggy production behavior — the test's manual recording is
the CORRECT behavior, the production code is what's wrong.

## Fix

Per the audit's fix sketch, make the recorded version correspond to
the version of the VALUE ACTUALLY READ, not the cell's current version
at read time. Two viable approaches — pick the one that fits the
existing `get_at` API more cleanly (inspect `get_at`'s signature and
implementation in `shamir-tx`'s mvcc_store first):

1. **Conservative clamp**: record
   `version_of(key).min(tx.snapshot_version)` instead of raw
   `version_of(key)`. Since `get_at` never returns anything newer than
   `tx.snapshot_version`, clamping the recorded version to
   `min(current, snapshot)` ensures the read-set never claims to have
   seen a version newer than what could possibly have been read. If a
   later committer's version exceeds `tx.snapshot_version`, the min
   still records `tx.snapshot_version`, and `validate_read_set`'s
   `current > version_seen` check will then correctly detect
   `current_version(k) > tx.snapshot_version` as a conflict (since any
   post-snapshot commit means the cell's version now exceeds what this
   tx's read-set claims to have observed) — this makes the conflict
   detectable instead of masked.
2. **Return the resolved version from `get_at`**: change `get_at` (or
   add a variant) to return `(resolved_version, bytes)` instead of
   just `bytes`, so the caller records the EXACT version the returned
   bytes correspond to (which by construction is always
   `≤ tx.snapshot_version`). This is more precise but touches the
   `get_at` API surface and all its callers — check how many callers
   `get_at` has across the codebase before choosing this approach; if
   there are many unrelated callers that don't need the version, prefer
   approach 1 (the `.min()` clamp) as the more surgical fix.

State which approach you chose and why in your report. Apply the SAME
fix uniformly to all 3 call sites: `read_one_tx`, `read_one_tx_bytes`,
and `record_scan_reads`.

Do NOT touch `acquire_pessimistic_read_lock`, the I.4
read-your-own-writes staging-overlay check, or anything unrelated to
the read-set version recording — those are out of scope for this fix.

## TDD requirement

1. **Red**: write a `#[tokio::test]` (check existing test module
   structure in `crates/shamir-engine/src/table/tests/` or wherever
   Serializable-isolation SSI tests currently live — likely near
   `acceptance_tests.rs` or a dedicated `ssi_*`/`serializable_*` test
   file) that reproduces the exact interleaving above:
   - Start tx B at snapshot version 10 (Serializable isolation).
   - Have another tx A commit a write to the same key at version 11
     (published via the normal commit path, not a raw MVCC call, so
     the interleaving is realistic).
   - Have B read the key via `read_one_tx` (getting the stale v10
     value).
   - Attempt to commit B.
   - **Before the fix**: B commits successfully (bug — no conflict
     detected). **After the fix**: B's commit is rejected with a
     conflict/abort error (the correct SSI outcome — first-committer-
     wins should abort the later-validating transaction that read
     stale data).
2. **Green**: apply the fix.
3. Confirm existing SSI/Serializable/read-set tests still pass —
   particularly any test that already manually records
   `tx.snapshot_version` as the read version (per the audit's note
   about `acceptance_tests.rs:495`) should be unaffected since that was
   already the correct semantics.
4. Confirm `record_scan_reads`'s Serializable-scan path also gets
   equivalent coverage (extend an existing scan-based SSI test, or add
   a focused one, whichever fits the existing test organization better).

## Test scope command

```
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-tx
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Which fix approach you chose (conservative `.min()` clamp vs.
  `get_at` returning `(resolved_version, bytes)`) and why.
- Confirmation the fix was applied uniformly to all 3 call sites
  (`read_one_tx`, `read_one_tx_bytes`, `record_scan_reads`).
- The failing-then-passing test evidence for the exact interleaving
  described above (B commits when it shouldn't, before the fix; B's
  commit is correctly rejected after).
- Confirmation existing SSI/read-set tests (including any that already
  manually record `tx.snapshot_version`) still pass unchanged.
- Full test/gate results (exact commands + pass/fail).
