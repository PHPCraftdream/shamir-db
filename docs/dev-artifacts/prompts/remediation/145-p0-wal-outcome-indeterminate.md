# P0 stage A2a — p0-wal-outcome-indeterminate

## Goal

Card `RV-WAL-3B29232E` (Seal-time fsync failure fails an already-successful append whose frames survive — acked-failed tx resurrected on replay) in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md` (read it whole, including the 2026-10-24 revalidation paragraph). Status at HEAD: `partially-fixed`; evidence: crates/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:395; crates/shamir-engine/src/query/batch/exec/batch_execute.rs:929; note: CHANGE — OBSERVED: rotation errors now retain Unknown tickets and unresolved versions rather than aborting their trackers, but the first pass missed that WalRequiresReopen still becomes TransactionInfo::aborted in explicit transactional batches, preserving a definite-abort receipt for potentially replayable frames.

Binding design ruling (two rush reviewers + @oh agree): a commit whose WAL ticket comes back Unknown/Indeterminate after it was queued must be reported to the client as **indeterminate** — never `aborted`. Refusals at the admission gate (nothing was queued; `wal_requires_reopen`) stay `aborted` with `reason:"wal_requires_reopen"`. `WrittenNotDurable` stays `committed` with `error:"wal_durability"`.

Implementation outline (@oh, verify against HEAD code; line numbers are approximate):
- Add `TxError::WalOutcomeUnknown { commit_version }`. `wal_completion(gate, commit_version, r)` (commit_policy.rs ~:37-44) returns it for Unknown/Indeterminate; callers in legacy_and_lockfree.rs (~:351) and locked.rs (~:406).
- `into_transaction_info` (crates/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs ~:395) maps it to an `indeterminate` TransactionInfo carrying `commit_version`, `snapshot_version`, `error:"wal_outcome_unknown"` (or the stable code the existing contract docs prescribe — read docs TRANSACTION_OUTCOMES.md and the operations doc first and follow them; if they say the code stays `wal_requires_reopen` for the indeterminate case, use that and say so).
- batch_execute.rs (~:929-942): an indeterminate outcome takes the not-committed result branch (no receipts stamped).
- Update `receipt_error_code` (receipts/commit.rs), `validate()` in transaction_info.rs (accept ("indeterminate", None) when reason is none and commit_version is set), the Rust and TS client result types, and the docs that describe the mapping.
- Implicit single ops: `QueryError` with the stable code plus `commit_version`, no receipts.

Recovery contract to document: after reopen, WAL replay decides (CRC-valid frame = committed at its version; missing/torn = never written); the client reconciles by reading, never auto-retries.

Test oracles: unit test that Unknown/Indeterminate -> WalOutcomeUnknown -> `indeterminate` and passes validate(); integration with a small max_bytes WAL and an injected seal-fsync failure (add a fail flag to the existing test seam if needed): explicit batch returns indeterminate with commit_version; the next commit returns aborted/wal_requires_reopen; after reopen with the fault cleared the row is present; inverse case (write fault -> NotWritten) absent after reopen. Update existing tests that pinned the old aborted mapping (transactions_tests.rs etc.). Each new test must fail with only its own fix reverted.

Reviewer designs for reference (sections about RV-WAL-3B29232E):
=== REVIEWER A ===
## Verdict: **Do not approve A2 as a three-line rollback/error-mapping/unlink patch. Approve the designs below, subject to the stated CI oracles.**

| Card | Required decision |
|---|---|
| **RV-TX-2CB455A7** | Retain early write-intent publication, but add **ownership-aware failure reconciliation before abort settlement**. A bare compare-and-restore is insufficient; simply moving `publish_cell` after `history.transact` is also insufficient. |
| **RV-WAL-3B29232E** | A possibly accepted commit is **unknown**, never definitely aborted. Preserve `wal_requires_reopen`; explicit transaction metadata must represent unknown rather than encode it as aborted. Keep known-published `wal_durability` outcomes committed. |
| **RV-WAL-A025886E** | **Fail writable open/reuse if stale-sidecar invalidation fails.** Invalidation must survive restart before modifying the segment. Do not introduce quarantine or validity-marker machinery for this repair. |

**Evidence boundary:** OBSERVED below means directly inspected source/document contents, not a reproduced execution. Consequences and proposed interleavings are **INFERRED**. All three cards, including their 2026-10-24 revalidation paragraphs, were read in full.

## 1. TX-2CB455A7: reconcile ownership before settling failure

### Observed mechanism

- Single writes publish the cell before the fallible history batch: `crates/shamir-tx/src/mvcc_store/store.rs:536–553`. Batched writes and deletes do the same: `store.rs:659–676`, `store.rs:869–883`.
- The publication primitive only takes the maximum offered version; it retains no predecessor ownership or failure state: `crates/shamir-tx/src/mvcc_store/write_receipt/cell_publish.rs:14–26`.
- An ordinary armed `VersionGuard` drop marks both trackers aborted and advances the visibility floor: `crates/shamir-tx/src/version/version_guard.rs:232–245`.
- Exact-version reads interpret missing physical data as absence, without reconciling the cell: `store.rs:447–465`; `crates/shamir-tx/src/mvcc_store/history/version_entry.rs:404–417`.
- History scans select physical versions without checking whether they belong to an aborted write: `crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:34–39`. Streams similarly filter by floor, not commit fate: `history/version_entry.rs:190–217`.
- `Store::transact` can apply a prefix and then return an error: `crates/shamir-storage/src/core/types.rs:379–390`.

**INFERRED:** For committed `k::5 = A`, failure before writing `k::6` can leave the cell at 6 and the floor past 6; the exact lookup then returns absence. Failure after writing `k::6` but before its timestamp creates a second problem: advancing the floor past aborted 6 can expose its physical bytes through scans. Restoring the cell alone does not solve that.

### Required invariant

**Committed-value preservation and publication/fate coherence:**

> Every successful read at snapshot S returns the newest successfully published version of that key at or below S, including its tombstone semantics. A failed write neither hides that version nor overwrites a newer successful writer. No floor may cross a failed version until its physical residue and cell ownership have been reconciled, or that residue is authoritatively excluded from every read and recovery path.

GC must retain the committed predecessor required by the settled floor and every live or opening snapshot—not merely the predecessor of the latest attempted write.

### Smallest acceptable design

**Recommendation: keep bump-first intent, add a per-key owned-write reconciliation protocol.** Do not change the shared monotonic publication primitive into an unconditional rollback primitive.

For each affected key, retain:

- the stable successful predecessor;
- outstanding owned versions and their pending/successful/failed fate;
- a generation usable by readers to detect reconciliation;
- protection for predecessor history while reconciliation is outstanding.

Capture ownership and predecessor under the same per-entry update that installs the intent. The separate `current_version` lookup at `store.rs:536` is not an atomic predecessor capture.

A single conditional restore is necessary but not sufficient:

```text
base = 5
A publishes 6, remembering 5
B publishes 7, remembering 6
A fails; restore-if-current(6) does nothing
B fails; restore-if-current(7) restores failed 6
```

The restored predecessor must be the newest **nonfailed owned candidate**, not a remembered scalar. Preserve successful candidates until no outstanding operation can need them for reconciliation. Change only the owned version state; do not restore an old copy of `vacuum_anchor` or `reserved_by`.

**Why not just stage publication after history?** With today’s fast path, a reader can sample the old cell after newer history has landed but before the cell changes, and perform an exact old-version read without noticing the newer eligible version. GC can also inspect landed-but-unpublished rows. Delayed publication can be made correct, but requires read-generation validation/pending filtering and GC coordination too. It is not the smaller change to this architecture.

### Pseudocode at the affected seams

The following is conceptual pseudocode, not an implementation:

```text
store.rs:536–553 / :659–676 / :869–883

allocate version guard(s)
install owned intents atomically per key
retain stable predecessor(s), including duplicate keys in a batch

await history.transact(all data + timestamps)

on success:
    install overlay and timestamp-index entries
    reconcile each intent as successful, preserving any newer owner
    publish all required successful state before settling guards
    commit guards; mark durable
    vacuum only with reconciled predecessor/anchor information

on error:
    retain ownership of every allocated version
    remove every possibly applied data key and timestamp key
    verify cleanup completion
    reconcile all failed intents, skipping failed predecessors
    invalidate stale reader generations
    only then settle those guards as aborted
    return original failure

on cleanup failure or uncertain continuing I/O:
    retain unresolved ownership; require reopen
    do not mark either tracker aborted
    do not advance the floor through those versions
```

For partial-write cleanup, remove **all attempted version keys**, not just operations believed to have executed. Keys are version-specific; cleanup must not remove another writer’s version.

Readers holding the failed cell version must not turn a subsequent missing lookup into `None`: validate the sampled generation and retry resolution when reconciliation changed it. Apply this to scalar and vectored exact paths, not just `resolve_read`.

GC must protect the stable predecessor until failure reconciliation finishes. Preserve the existing opening-snapshot barrier and completion-floor anchor obligations at `crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:62–98`, `:163–205`. Point reads/streams also need a stable read lifetime—an appropriate temporary snapshot/read pin or a validated retry protocol—not protection only for registered transactional readers.

**Scope boundary:** If cleanup cannot be guaranteed after cancellation, detached backend work, or restart, ordinary abort settlement is forbidden. Such writes need a retained unresolved/persistent fate mechanism or a fail-closed recovery policy. An in-memory “aborted versions” set alone cannot make restart safe.

### Deterministic CI oracles

Use a store with barriers before/after each mutation and independently injectable write/removal failures.

1. **Existing value:** establish exact bytes `A` at version 5; fail version 6 before its first mutation. After failure, scalar point, vectored point, snapshot and stream reads all return `A`.
2. **Late failure:** apply version-6 data, then fail its timestamp; repeat with a later operation in a multi-key batch. No failed bytes or tombstone may become visible after the floor advances.
3. **Two failures:** schedule 6 and 7 against the same key; fail them in both orders. Final cell resolves to 5, never failed 6.
4. **Newer success:** pause 6; successfully materialize 7; fail 6. Reconciliation must preserve 7. A snapshot pinned at 5 still returns `A`.
5. **Reader race:** pause an exact lookup after sampling failed version 6; finish rollback; release the lookup. It must retry and return `A`, not absence.
6. **GC race:** hold a snapshot-opening barrier while multiple writes/reconciliations and GC occur. The pinned predecessor remains readable.
7. **Cleanup failure:** fail residue removal. Assert unresolved ownership, reopen requirement and no tracker advancement through the failed version.
8. **Cancellation:** cancel at the history await and cleanup await. Assert that no ordinary guard drop falsely certifies an abort.

**Acceptance:** these apply to single set, delete, many, append-only-many and their no-wait variants. Fresh-key/first-write-error tests alone are insufficient.

## 2. WAL-3B29232E: unknown must remain unknown at the client boundary

### Observed mechanism and existing contract

- Physical append succeeds before threshold-triggered rotation can fail: `crates/shamir-wal/src/segment_set/segment_set.rs:255–265`.
- Seal fsync and new-active open are separate failure points: `segment_set.rs:380–406`.
- The coordinator conservatively treats a File-sink append error as indeterminate because it cannot prove rollback: `crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:313–322`. Its guard publishes Unknown and fail-stop notification: `:483–501`.
- Unknown tickets remain unresolved instead of being aborted: `crates/shamir-tx/src/version/version_guard.rs:203–209`.
- `wal_completion` maps Unknown/Indeterminate to `WalRequiresReopen`: `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/commit_policy.rs:26–44`.
- Nevertheless, `WalRequiresReopen` becomes `TransactionInfo::aborted`: `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:395–402`; explicit batches call that mapping at `crates/shamir-engine/src/query/batch/exec/batch_execute.rs:929`.

The documentation explicitly acknowledges this inconsistency:

- `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md:56–62` describes indeterminate WAL, then documents the current aborted mapping while warning that it does not prove disk absence.
- `TRANSACTION_OUTCOMES.md:364–382` gives the outcome/retry codes.
- `TRANSACTION_OUTCOMES.md:42–48` distinguishes committed `wal_durability`.
- `TRANSACTION_OUTCOMES.md:400–403` says visibility-prefix settlement does **not** establish whether an unknown transaction committed.

**Recommendation:** correct the representation, not merely the explanatory prose.

### Exact client-visible contract

Use **`status: "unknown"`** as the new metadata status; “indeterminate” describes its meaning. Do not introduce two synonymous statuses.

| Physical/publication knowledge | Implicit single operation | Explicit transactional batch / interactive commit |
|---|---|---|
| Possibly accepted; current rotation path reports `WalRequiresReopen` | Return the existing structured failure code **`wal_requires_reopen`**, not a success or an aborted receipt. | Return **`TransactionInfo.status = "unknown"`**, `reason = "wal_requires_reopen"`; no claimed committed version, materialization or visibility. |
| Proven not written / rejected before admission | Definite operation failure using its applicable code. | **Aborted**, with the applicable diagnostic/code. |
| Known published, but requested WAL durability failed | Committed receipt with **`error = "wal_durability"`**. | **Committed**, `error = "wal_durability"`, preserving actual version, materialization and visibility. |
| Requested durability positively achieved, publication completed | Normal committed receipt. | Normal committed metadata. |

The implicit path already preserves `wal_requires_reopen` through `BatchError::QueryError`: `crates/shamir-engine/src/query/batch/control/receipts/commit.rs:26–44`, `:67–73`. It presently returns no `TransactionInfo` for that failure; there is no need to redesign its envelope for A2. If metadata is attached to that error later, its status must be unknown.

One important distinction: `WalRequiresReopen` is also returned **before admission** when the repository is already stopped: `entry.rs:621–642`. That request was not attempted. Separate admission refusal from attempted-but-unknown internally; both may retain the operational code `wal_requires_reopen`, but only the attempted commit is unknown. Do not derive physical fate from the code alone.

### Smallest change

Keep the existing conservative WAL fail-stop behavior. A2 does **not** need to split append from housekeeping to close the false-abort receipt.

```text
entry.rs:368–402

WalDurability(actual published facts)
    -> committed_with_error("wal_durability")

AttemptedWalOutcomeUnknown
    -> unknown(tx_id, snapshot_version, reason="wal_requires_reopen")

RejectedBeforeWalBecauseReopenRequired
    -> aborted(tx_id, reason="wal_requires_reopen")

other proven precommit failures
    -> aborted(...)
```

Update `TransactionInfo` construction/validation and consumers together. Today validation rejects unknown outright: `crates/shamir-query-types/src/ops/batch/req/transaction_info.rs:111–117`. The existing test also explicitly requires the wrong aborted mapping: `crates/shamir-engine/src/query/batch/tests/executor/tx/transactions_tests.rs:128–133`.

Unknown must not fall through a client’s “not committed means safely aborted/retryable” branch. It carries no successful query results or committed receipt stamps. The existing explicit-batch noncommitted branch can return empty results, but must preserve unknown metadata: `batch_execute.rs:928–942`.

**Do not warn-and-return-Ok after failed seal fsync.** If later work separates physical append facts from housekeeping, acknowledgements must be based on positively achieved durability tiers—not assumptions about whether a later sync will fail.

### Recovery resolution

**OBSERVED:** recovery reads and sorts surviving WAL entries, replays them, persists interner state, then marks recovered versions materialized/durable: `crates/shamir-engine/src/tx/recovery/v2.rs:375–377`, `:427–471`. WAL entries have no commit/abort-marker distinction; replay treats surviving valid entries as committed: `:454–458`.

Therefore:

- A surviving complete valid entry resolves the unknown attempt to recovered committed state.
- A successfully completed recovery with authoritative evidence that the attempt was neither accepted nor already materialized can resolve it as not committed.
- Corruption or incomplete evidence must refuse recovery or retain uncertainty—not manufacture an abort.
- Absence from the **current WAL alone** is insufficient: committed entries can have been drained/truncated. Likewise, settled visibility is insufficient.
- Reconciliation must use repository-scoped transaction/attempt identity and authoritative recovered state. Do not promise a durable per-transaction outcome lookup that A2 has not implemented.

### Deterministic CI oracles

For Buffered and Synced, exercise implicit writes, explicit Execute and interactive TxCommit:

1. Complete real frame writing; fail seal fsync.
2. Complete seal fsync; fail new-active creation.
3. Assert code/status above, Unknown ticket, unresolved version and refusal of subsequent admission.
4. Reopen one fixture with complete frames retained: exact writes recover, with idempotent repeated reopen and no duplicate counter/index effects.
5. Reopen a controlled crash image without the unconfirmed frames: no write appears; no earlier response claimed definite abort or durability.
6. Separately fail explicit post-write sync: known-published `wal_durability` remains committed.
7. Round-trip unknown metadata through actual wire serializers and client decoders; verify no automatic retry/rollback classification.

**Acceptance:** no attempted Unknown ticket can produce an aborted receipt; no Synced success without positive durability evidence; all affected client/schema validators handle unknown explicitly.

## 3. WAL-A025886E: fail closed before modifying a reactivated segment

### Observed mechanism

- Removal errors are warned and swallowed: `crates/shamir-wal/src/entry/segment_meta.rs:164–172`.
- Open continues after that best-effort removal: `crates/shamir-wal/src/segment_set/segment_set.rs:196–206`.
- Poison rotation does likewise: `segment_set.rs:317–329`.
- On a later open, a CRC-valid sidecar maximum is trusted directly: `segment_set.rs:147–155`.
- Truncation deletes segments whose recorded maximum is at or below the durable threshold: `segment_set.rs:484–490`.

**INFERRED, conditional on the filesystem behavior described by the card:** readable meta max10 can survive failed invalidation while the segment accepts version60; later truncation at55 can delete version60.

### Required invariant and smallest design

**Metadata-generation validity:**

> A sidecar may influence truncation only while it describes the exact immutable segment contents from which it was generated. Before any repair, append or rewrite invalidates that relationship, restart-safe invalidation must complete.

Choose **strict invalidation and refusal**, not a quarantine naming scheme or separate validity marker.

```text
segment_meta.rs:164
invalidate_for_reuse(path) -> Result
    remove sidecar
    NotFound is success
    any other unlink error is failure
    establish restart-safe absence using the platform durability mechanism

segment_set.rs:174–204
    invalidate_for_reuse(active_path)?   // BEFORE repair_torn_tail
    open/repair active
    only then expose writable SegmentSet

segment_set.rs:317–329
    invalidate_for_reuse(sealed_path)?
    only then create/install replacement active
```

The ordering change matters: `repair_torn_tail` currently precedes invalidation at `segment_set.rs:182`. Repair is itself a rewrite.

A successful unlink without the required filesystem durability ordering is not enough for a power-loss guarantee. If the platform cannot establish that ordering, use replay-only trust rather than claiming durable invalidation.

**Compatibility requirement:** strict invalidation prevents newly created stale sidecars; it cannot certify already-sealed stale sidecars left by an older binary. For the “never trusted” guarantee across upgrade, invalidate trust in legacy WMT1 sidecars—e.g. treat them as replay-only and emit a new recognized format only under the corrected lifecycle. WMT1 contains no segment-generation/content binding: `segment_meta.rs:49–58`, `:138–154`.

No automatic quarantine is needed. Ignoring the sidecar only in memory is insufficient because the next process can trust it again.

### Deterministic CI oracles

Inject filesystem operations independently; do not rely on OS ACL timing:

1. Create readable CRC-valid meta max10. Force unlink failure while WAL writes and directory creation remain otherwise permitted.
2. Assert writable open fails **before repair or append**; segment bytes are unchanged.
3. Inject invalidation failure during poison rotation. Assert no unsafe rotation/retry proceeds; propagate uncertainty according to section 2.
4. Test NotFound as successful invalidation and successful removal permitting reuse.
5. Use crash-image barriers around invalidation durability, first rewrite, reseal and new-active creation. Every reopen either derives max60 by replay or refuses open; truncate55 must retain version60.
6. Seed a legacy valid stale sidecar on an already-sealed segment. Confirm it cannot bypass replay after upgrade.
7. Inject sidecar replacement failure during reseal. Confirm fallback cannot rediscover and trust an older sidecar.

**Acceptance:** non-NotFound invalidation errors are actionable failures; no writable handle escapes first; invalidation precedes every rewrite; restart and legacy-sidecar tests prohibit truncation based on stale maxima.

**Files changed:** none. **Builds/tests run:** none; read-only review only. All behavioral acceptance above remains for GitHub CI.

=== REVIEWER B ===
## Verdict: approve A2 only with three fail-closed designs

1. **TX-2CB455A7:** stage the write before publishing the `RecordCell`; **reject compare-and-restore as the primary fix**. Staging must also address partially applied `Store::transact` operations—not merely move one line.
2. **WAL-3B29232E:** preserve the existing WAL fail-stop/Unknown behavior and eliminate the **false aborted receipt**. An uncertain attempted commit is **indeterminate**, with stable code `wal_requires_reopen`; it is not a retryable abort.
3. **WAL-A025886E:** make sidecar invalidation mandatory **before making a segment mutable**. If invalidation cannot be made durable, **fail open/reuse**. An in-memory “ignore metadata” flag is insufficient.

**Evidence classification:** OBSERVED below means inspected source/documentation, not an executed reproduction. Consequences and proposed designs are INFERRED/recommended. I read all three cards, including their revalidation paragraphs: `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P0.md:2403`, `:2611`, `:2695`.

---

## 1. RV-TX-2CB455A7 — publish only backed, successful versions

### OBSERVED mechanism

- Single set publishes the cell at `crates/shamir-tx/src/mvcc_store/store.rs:539`, before the fallible history transaction at `:545–553`.
- The same ordering exists for batch set at `:663` / `:676`, append-only batch at `:789` / `:802`, and delete at `:871` / `:883`.
- A subsequent exact-version miss becomes absence, not the previous value: `crates/shamir-tx/src/mvcc_store/store.rs:456–464`; current point reads do the same at `crates/shamir-tx/src/mvcc_store/history/version_entry.rs:413–416`.
- The physical-history scan does **not** filter aborted versions: `crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:22–43`.
- Crucially, default `Store::transact` applies operations sequentially and can fail after earlier operations landed: `crates/shamir-storage/src/core/types.rs:379–390`. Its capability flag describes **visibility atomicity**, not an explicit “Err implies no mutation” guarantee: `:393–422`.

**INFERRED consequence:** restoring only the cell—or merely delaying publication—does not prevent a partially written failed version from later being selected by snapshot, stream, or cold-start scans.

### Required invariant: **backed publication and aborted-version exclusion**

For each key:

> `RecordCell.version` names the greatest successfully published version, backed by readable history or a committed overlay entry. Failed versions cannot affect any read, cold reconstruction, timestamp lookup, or GC anchor selection. Failure of one writer cannot regress another writer’s successful publication.

GC additionally retains the existing **settled-prefix floor and predecessor anchor**: versions protected by `min_alive`, and the predecessor needed by snapshots, remain readable. Those rules are explicit in `crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:34–41`, with the opening-snapshot barrier at `:62–83`.

### Smallest acceptable design

**Choose staged publication with an explicitly failure-atomic history-write requirement.** Before performing these direct writes, require an implementation/adapter guaranteeing that a returned error leaves **none** of the attempted data/timestamp entries installed. Unsupported history stores must be rejected before mutation, not silently treated as atomic.

This is the smallest sound design **if narrowing supported history backends is acceptable**. Do not treat the current `supports_atomic_transact()` flag alone as sufficient: its documented guarantee must also cover failure rollback.

If preserving arbitrary `Arc<dyn Store>` support is mandatory, A2 needs a larger design: a persistent successful-version/batch publication record and readers/GC/cold reconstruction that exclude uncommitted physical rows. **A cell-only patch cannot meet that contract.**

Recommended pseudocode against `store.rs:536–584`, applied to all four write families:

```text
require history transaction has failure-atomic semantics

allocate guard(s)
build data + timestamp operations without publishing cells
history.transact(operations)?
    // Err: no physical entries; cells/overlay/ts-index unchanged

without another suspension point:
    insert successful overlay entries
    insert successful timestamp-index entries
    publish_cell_sync(key, version) for each item
    commit all guards

mark durable after visibility settlement
vacuum using predecessor information from successful publication
acknowledge outside writer guards
```

Use the existing max-monotone synchronous publication body at
`crates/shamir-tx/src/mvcc_store/write_receipt/cell_publish.rs:14–27`. Preserve `reserved_by` and `vacuum_anchor`; do not replace the whole cell with a saved copy.

**Why not compare-and-restore?**

- A reader can already have copied the failed version before restoration and later perform the missing exact lookup.
- With two pending writers, restoring the immediate predecessor can restore another failed/pending version.
- Restoring an entire saved cell can overwrite newer reservation/GC bookkeeping.
- Even a version-conditional restore does not remove partially written history rows.

These are concurrency objections, not claims of executed failures.

### Deterministic CI oracles

Use a fault-injecting store with explicit entered/release barriers:

1. Seed `k → A`; fail the next set and delete. While history is paused and after failure, assert point, vectored, snapshot, and stream reads return the appropriate existing committed value—not absence.
2. Writer A pauses before failing; writer B successfully publishes a higher version. Release A. Assert the cell and current reads still select B.
3. Reverse successful completion order. Assert publication remains max-monotone and historical snapshots return their respective bytes.
4. Fail after the first physical data operation and at a later batch operation. The failure-atomic adapter must restore the complete pre-write physical state. An unsupported sequential store must be rejected **before its first mutation**.
5. Hold a snapshot-opening barrier across several successful writes and a failed write. Inspect retained physical versions and assert the snapshot’s predecessor survives GC.
6. Reconstruct a fresh `MvccStore` over the resulting history. Failed versions and their timestamps must not reappear.

**Acceptance:** no failed write changes logical contents; no successful concurrent write is rolled back; no failed row survives as eligible history; `durable_watermark ≤ visibility watermark`; existing snapshot-opening/floor protections remain intact.

---

## 2. RV-WAL-3B29232E — uncertainty must survive through the client boundary

### OBSERVED mechanism and counter-evidence

The WAL layer already conservatively handles the important ambiguity:

- Physical append succeeds before rotation can fail: `crates/shamir-wal/src/segment_set/segment_set.rs:255–265`.
- Seal sync and next-active open are separate failing operations: `:380–382`, `:406`.
- A file-sink append error exits without claiming rollback: `crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:318–322`.
- `LeaderGuard` completes affected tickets as Unknown and stops the group: `:483–500`.
- `wal_completion` sets requires-reopen for Unknown/Indeterminate: `crates/shamir-engine/src/repo/journal/backend/durability_policy/durability_policy/commit_policy.rs:37–44`.
- Unknown tickets remain unresolved rather than being marked aborted: `crates/shamir-tx/src/version/version_guard.rs:203–209`.

**The remaining contradiction is client-facing:** `WalRequiresReopen` becomes a reason string and then `TransactionInfo::aborted` at `crates/shamir-engine/src/tx/commit/pipeline/phase/commit/entry.rs:395–402`; explicit transactional batches expose that mapping at `crates/shamir-engine/src/query/batch/exec/batch_execute.rs:928–942`.

### Exact outcome contract

| Actual knowledge about this request | Required outcome |
|---|---|
| Proven not admitted/not written | **Aborted**; retain the appropriate refusal/error code. |
| Attempted WAL commit may have survived | **Indeterminate**; `wal_requires_reopen`; no claim of rollback, visibility, or durable acceptance. |
| Known published commit, durability failed | **Committed**, `error: "wal_durability"`; retain existing visibility/materialization metadata. |
| Requested Synced durability proved | Normal committed result; never manufacture it from an unsuccessful sync. |

**Recommended canonical status spelling: `indeterminate`.** Use one spelling consistently rather than introducing both `unknown` and `indeterminate`.

For the two requested execution surfaces:

- **Implicit single operation:** retain the existing error envelope and stable code **`wal_requires_reopen`**, with **no fabricated aborted receipt**. This mapping already exists at `crates/shamir-engine/src/query/batch/control/receipts/commit.rs:29–44`, `:67–73`. If metadata is attached, its status must be `indeterminate`.
- **Explicit transactional batch:** return `TransactionInfo.status = "indeterminate"` and `reason = "wal_requires_reopen"`; do not return successful write receipts or claim the batch was aborted. Preserve transaction/snapshot identity. Do not label an allocated candidate as a confirmed `commit_version`.
- **Interactive commit:** use the same classification, not an alternate “aborted on every error” mapping.

**Important distinction:** the same `WalRequiresReopen` variant is also returned **before admission** when the repository is already stopped, at `entry.rs:621–642`. That new request is definitely refused. The smallest truthful design splits the classification into **indeterminate attempt** versus **reopen-required admission refusal**; both may retain the public code `wal_requires_reopen`, but only the former has uncertain transaction fate.

### Existing documentation supports the semantics—but records the defective encoding

- `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md:56–62` explicitly says indeterminate, requires reopen, and no blind replay; it also documents the current misleading aborted mapping.
- The retry/error-code table says reopen/reconcile first: `:364–382`.
- Published durability failures are committed, not aborts: `:42–48`.
- Implicit receipts and explicit top-level metadata are distinguished at `:113–117`.
- A visibility observation proves settlement through a **position**, not whether an unknown transaction committed: `:400–403`.
- Operational guidance repeats that indeterminate WAL is not proof of absence: `docs/guide-docs/guide/07-operations.md:334–350`.

Adding the status is **not merely a local engine change**: the current wire validator accepts only committed/aborted at `crates/shamir-query-types/src/ops/batch/req/transaction_info.rs:111–117`. Update the validator, affected client decoders/types, and contract together; do not describe this as an automatically backward-compatible optional field.

### Minimal pseudocode

At the WAL-result classification and `entry.rs:368–402`:

```text
NotWritten:
    settle as aborted

Unknown / Indeterminate:
    retain unresolved ticket and entry
    stop repository
    return IndeterminateAttempt(identity, wal_requires_reopen)

already stopped before admission:
    return AdmissionRefused(wal_requires_reopen)

WrittenNotDurable followed by publication:
    return committed metadata with error wal_durability
```

At `batch_execute.rs:929`:

```text
match classified outcome:
    committed       => stamp committed receipts
    aborted         => aborted transaction metadata
    indeterminate   => indeterminate transaction metadata; no success receipts
```

### Recovery contract

`RepoWalManager::recover` replays surviving WAL entries; `commit(txn_id)` is a no-op, not a separate durable commit marker: `crates/shamir-tx/src/tx/repo_wal_manager.rs:152–163`.

Therefore:

- A complete surviving commit entry is replayed idempotently and its projections restored.
- A missing/incomplete active-tail entry must not be invented from retained in-memory intent.
- Recovery failure/corruption remains an error—not evidence of abortion.
- Client reconciliation requires transaction-specific evidence or application idempotency/state checks. **Do not promise that `WaitForVisibility` resolves commit fate.**

### Deterministic CI oracles and acceptance

Inject faults **after real frame writes**, at seal sync and next-active creation, for Buffered and Synced:

1. Barrier confirms append completion; trigger the rotation fault.
2. Assert attempted transaction is indeterminate, stable code survives both execution surfaces, trackers remain unresolved, and later commits are refused.
3. Reopen using a controlled persisted image containing the complete entry: assert exactly-once logical replay.
4. Reopen an image lacking the uncertain entry: assert no logical application.
5. Separately test proven NotWritten, pre-admission reopen refusal, and known published `wal_durability`.
6. Round-trip all new metadata through real wire/client validators.

**Acceptance:** no response asserting definite abort can later have its attempted commit replayed; no Synced success without durability proof; no blind automatic retry for indeterminate attempts.

---

## 3. RV-WAL-A025886E — invalidate durably before mutation, or refuse reuse

### OBSERVED mechanism

- Removal errors are swallowed at `crates/shamir-wal/src/entry/segment_meta.rs:164–172`.
- Active reuse continues after that helper at `crates/shamir-wal/src/segment_set/segment_set.rs:196–204`.
- Worse, torn-tail repair already occurs at `:182`, **before** invalidation.
- Poison rotation also continues after best-effort removal at `:317–329`.
- A sealed segment’s valid sidecar is trusted directly at `:147–155`.
- That maximum determines deletion eligibility at `:484–490`.
- Sidecar CRC validates only its own contents, not correspondence to a segment generation: `crates/shamir-wal/src/entry/segment_meta.rs:138–154`.

### Required invariant: **metadata belongs to an immutable segment incarnation**

> A trusted sidecar must describe the exact immutable WAL incarnation being considered for truncation. Before any append, repair, truncation, or rewrite can change that incarnation, previous metadata must be durably invalidated.

### Smallest design: mandatory invalidation, no quarantine subsystem

Change the invalidation operation to return a result. Missing is success; other unlink errors are fatal **for mutable reuse**.

Move invalidation **before `repair_torn_tail()`**, not merely before the first append:

```text
select highest segment as candidate active
durably_invalidate_sidecar(candidate)?
    unlink .meta; NotFound is acceptable
    establish required directory-metadata durability
    failure => fail open before repair or append
open/repair candidate
expose writable SegmentSet
```

Apply the same prerequisite to any path that would leave an untrusted old sidecar associated with changed bytes, including poison rotation at `segment_set.rs:317–329`.

**A process-local flag is rejected:** the next process still sees the readable CRC-valid stale sidecar. Likewise, successful unlink without the filesystem durability needed to prevent crash resurrection is not a complete crash-safety argument.

If portable durable invalidation cannot be established, the alternative is a **persistent generation/validity protocol** checked on every metadata read; failures to establish its invalid state must still block mutation. That is larger than the recommended fix.

Quarantine is unnecessary for A2. Renaming only the WAL or ignoring it during recovery risks discarding recoverable data. A safe quarantine would have to preserve replay and truncation protection across restart.

Best-effort cleanup can remain only where a leftover sidecar cannot become authoritative for different bytes. Do not globally turn harmless cleanup failures into fatal database errors.

### Deterministic CI oracles

Use an injected filesystem operation seam, not platform ACL timing:

1. Create readable, valid `.meta` with max 10. Inject unlink failure while WAL read/write and directory creation remain permitted.
2. Attempt active open. Assert failure **before repair or append**, unchanged WAL bytes, unchanged metadata, and no new active segment.
3. Repeat with NotFound: open succeeds.
4. Inject invalidation durability failure after unlink. Assert mutable reuse is still refused.
5. Exercise poison rotation with invalidation failure. Assert no unsafe transition and propagate the uncertain WAL outcome according to section 2.
6. Positive lifecycle: invalidate, append version 60, force sidecar-write failure during reseal, reopen, invoke `truncate_below(55)`. Assert version 60 remains replayable and its segment is retained.
7. Model restart at each invalidation/mutation boundary, including the filesystem’s allowed crash image. A stale max-10 sidecar must never authorize deletion of rewritten version-60 bytes.

**Acceptance:** invalidation failure blocks mutation; restart cannot resurrect authoritative stale metadata; metadata-write failure falls back to replay safely; truncation never relies on a maximum below the segment’s true maximum.

---

**Files changed:** none. **Builds/tests run:** none; read-only review only. All proposed oracles and acceptance checks require GitHub CI verification.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD: for every card add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`). Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code; Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows.
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
