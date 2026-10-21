# PR8b-3: destination lifecycle — incident authority, destruction, reservations

**Status:** normative design baseline amended by [DEC-3 Q3](../../design/dec3-owner-rulings-2026-10-21.md#q3---pr8b-3-design-and-draft-196). This file §§1–5, 131a §§9–11 and 131b §§6–8/Appendices A–B form the single normative set. 131c and 131d are historical, non-normative. Observed checkout citations are centralized in 131a Appendix 13. #188/#189 are merged; no unmet dependency markers remain.

## 1. Goal, staging and scope

Provide a crash-safe, source-independent exit for quarantined replication destinations: destroy only frozen, owned storage; release names only when protection is legitimately resolved; remove the subscription row without serving destroyed or partial data.

Stage 1 (8b-3a..3c) introduces ownership, destruction operations, reservation/admission controls, backend lifecycle, DDL and audit guarantees. Stage-1 R1 requires every destination absent. Stage 2 (8b-3d) makes incarnation evidence (`destination_incidents` plus repo-carried `MetaKey::ConsistencyIncidents`) authoritative; R1 proceeds by ordered incident handoff and no longer requires absence. Stage-1 subscription-row coverage is superseded; schemas and mechanisms remain in use.

Destination RENAME remains restricted as a format limitation pending PR8b-4. Ordinary DROP/RENAME are not made crash-atomic here. In-place reseed and multi-target seed recovery are out of scope. Recovery is destroy → R1 → fresh install as a new incarnation. Windows reseed is refused before any install mutation in this series and certified by Windows CI case 15; Windows destruction and name release remain required. Windows seed installation is a mandatory end-state follow-up, not a permanent product limitation.

## 2. Invariants

1. **I1 — Protection survives configuration deletion.** Unresolved destinations remain unreadable and unpublishable until validated recovery or durable destruction. Subscription deletion never clears protection; stage-2 incident handoff precedes deletion.
2. **I2 — Authorization precedes action.** Atomic authorized Prepare and durable `destruction_prepared` barrier precede destructive work. Barrier failure leaves `Prepared` and reservations held, returning `destruction_audit_unavailable`; a failed sync never proves absence.
3. **I3 — Replay before open.** Load operations, reservations and incident evidence before repository open. Reserved targets are skipped, not recovered or published.
4. **I4 — Reserved-name exclusion/staged reachability.** Every publisher checks reservations. 8b-3c seed authority reader runs under exclusive maintenance lock before `find_complete` and holds it through install. 8b-3b executor and authority are `pub(crate)` and not wire/admin dispatchable; destructive dispatch is enabled only in 8b-3c together with all publisher wiring.
5. **I5 — Complete verified destruction.** Destroy and verify every owned component, including sibling WAL, hybrid durable tiers and seed remnants.
6. **I6 — Frozen targets only.** Use recorded storage identity and component descriptors, never whatever occupies a name later. Replacement/foreign ownership fails closed.
7. **I7 — No control lock across slow work.** Never join tasks or delete filesystem components while control is held.
8. **I8 — R1 never orphans protection.** Stage 1 requires absence; stage 2 requires ordered incident handoff and no unresolved pending terminal cause.
9. **I9 — Per-destination, per-cause.** Destinations and incident causes resolve independently.
10. **I10 — Durable audit outbox.** Prepare audit is a barrier; later state/outbox event are one durable write. Journal remains recovery authority.
11. **I11 — Only validated recovery or destruction completion clears incident protection on surviving data.** Acknowledgement, deletion, rename or sibling destruction never clears it.
12. **I12 — No clean checkpoint required.** Destruction works when data cannot open or checkpoint; workers and handles must be closed.
13. **I13 — Admission closes check/use races.** Acquire namespace admissions before control/write locks. 8b-3b tests primitives only; 8b-3c wires and race-tests every publisher. Migration is interlocked in 8b-3c: acquire both namespace admissions in global `(db,repo)` order, revalidate identity, reject reserved targets; migration-during-reserved-name is mandatory CI.
14. **I14 — Unknown is not success.** Ambiguous writes/flushes require forced flush then reread. Reservations and fail-closed reads persist until outcome is resolved.

## 3. Data model and durability

Durable lifecycle state resides in the Fjall-backed system store; repository-carried records remain in their repository. Schemas and bounds: 131b Appendix A. Verified source citations: 131a Appendix 13.

### 3a. Storage identity and ownership

`StorageOwnership` v1 has a stable 16-byte `storage_object_id`, durability flag, creation time and stable-id component descriptors. Components include primary store, metadata/interner, transaction state, changelog, identity/bookmarks, seed receipts, WAL, cache/hybrid durable tiers, and seed remnants. The system catalogue mirrors the full descriptor, allowing destruction when a repository cannot open. Mirror writes propagate errors; disagreement fails `unsafe_storage_ownership`. An intact catalogue mirror may authorize destruction when the repo ownership record cannot be read; if both are absent/unreadable, refuse. Boot skips and fences covered repositories that cannot open.

Storage ownership is distinct from replication lineage. Hybrid stores ownership in its durable fjall tier; hybrid replication lineage remains per-boot. Mint ownership only on controlled creation and publish durably before exposure. Missing/corrupt ownership returns `unsafe_storage_ownership`, refusing open or delete; never mint lazily. Pre-8b-3a directories return typed `storage_format_unsupported`; no backfill or migration.

### 3b. Operation record and authorization

One system-store `destruction_operations` table is keyed by non-reusable `operation_id`. It stores kind (`Repo`, `Database`, `SubscriptionRemoval`), frozen targets/storage ids/ownership digests/components, generations, actor/reason/nonce/digest, state/diagnostics, outbox, `record_version`, and `executor_epoch`. Full schema is in 131b Appendix A.

**Authorization is atomic:** at Prepare, under held admissions, one system-store transaction performs challenge DELETE-IF-MATCHES `(challenge_id,digest)` and `Prepared` PUT-IF-ABSENT; one flush follows. Challenge failure writes no row and authorizes no execution (an error is sufficient; any Rejected row must be insert-if-absent). No resubmission can substitute for authorization. An existing row may be returned/claimed only because its original transaction proves challenge consumption and insertion succeeded. The operation token provides exclusion; epochs only detect stale work.

`Rejected` means definitive refusal before authorization; never reserves or overwrites an existing id, has empty outbox, and emits best-effort rejection directly. Unsupported backend is refused during preflight; it is not an operation-row `ErrorClass` variant.

Event identity is `(operation_id, transition_ordinal)`, independent of record version and bound to SHA-256 of canonical payload. Same id/different digest is corruption: poison plus incident, never deduplicate. Claims conditionally CAS the observed `(record_version,executor_epoch)` pair; winner increments both and spawns immediately without an await; losers return status. Executor checks epoch before every destructive durable step. Operation token excludes; epoch only detects stale work. Ack hook notifies the sole executor and never writes operation rows.

### 3c. Reservations and retirement

Reservations are an in-memory view rebuilt at boot and maintained under control, not a separately persisted truth. Sources: terminal/quarantined anchors; nonterminal Repo/Database operation targets (including barrier-failed Prepared); Destroyed rows until terminal outbox acknowledgement; stage-2 unresolved incident/repo evidence; boot-loaded SubscriptionRemoval Pending/RowDeleted destinations until resume completes. Rejected never reserves. Database scope uses `(db,"*")` with exact-match then wildcard lookup; this normalization belongs to lifecycle reservations, not the exact-key quarantine cache.

Retire a Repo/Database operation only after `Destroyed` and empty outbox; retire SubscriptionRemoval only at `Completed`. No subsequent archival or expiry releases protection; final Blocked operations and blocked RowDeleted removals retain reservations until retry resolves them. Newly discovered data becomes an R2' target or incident, never a release. No automatic expiry. Apply quota/backpressure to concurrent nonterminal operations; refuse new destruction at capacity and never evict protected state. Operation ids are never reusable. Registry entries may be reclaimed only under the same synchronized map/shard lock as get-or-create/Arc cloning and at strong_count 1 without temporary clone; no weak/external bypass handles; capacity refuses rather than evicts.

### 3d. Incidents and pending terminal evidence

One incident per `(cause=(subscription,generation), storage_object_id)`; retries reuse incident id. The catalogue `destination_incidents` and repo-carried `MetaKey::ConsistencyIncidents` each hold full incident copies; either unresolved copy blocks exposure and disagreement fails closed. Boot rebuilds missing catalogue rows from repo evidence when openable. Recovery clears one incident only when new storage id, receipt digest, source lineage/history epoch and bookmark floor+1 are validated. Destruction completion atomically marks incidents Cleared/ByDestruction with Destroyed. Stage-1 assertions prohibit other clearing paths; predicate tests start in 8b-3d.

Pending terminal coverage is keyed by destination and `(subscription,generation)` cause. Publish before row mutation; retire only after durable snapshot confirmation. R2'/R1 refuse while any referencing generation has pending terminal evidence (`quarantine_persistence_pending`). Stage-2 verified handoff is durable cause evidence.

### 3e. Durable mutation and audit contract

Every lifecycle transition writing two or more system-store records uses ONE transaction plus ONE flush via typed sole lifecycle API `SystemStore::commit_durable(batch) -> DurableReceipt`. Commit-success/flush-failure is typed `DurabilityUnknown`; resolve via forced flush then reread. `remove_database` missing flush is fixed in 8b-3a, not 8b-3c. Boundary recovery table:

| Durable prefix | Meaning | Boot/retry action |
|---|---|---|
| No Prepare transaction | No authorized operation | No row; new valid challenge required |
| Prepare committed+flushed | Challenge consumed, Prepared and reservation durable | Publish reservation; append/verify audit barrier before destructive work |
| Prepare flush unknown | Transaction outcome unknown | Force flush and reread row and challenge; execute only if both effects confirmed, otherwise fail closed |
| Prepared durable, barrier unknown | Authorized intent but no destructive authority | Keep reservation; fresh same-event append; failure reports `destruction_audit_unavailable` |
| Quiescing/Destroying/Cataloguing durable | Phase intent durable, physical work may be partial | Skip target at boot; resume idempotently and verify ownership/absence |
| Catalogue delete flush unknown | Catalogue durability unknown | Force flush, reread; absent advances, present retries |
| Complete durable | Destruction and incident resolutions atomic | Verify terminal state; reservation remains until audit retirement |

Add `AuditSink` in shamir-db, implemented by server and injected before init/replay. Durable barrier I/O runs on `spawn_blocking` with bounded timeout. Flush-before-return applies in strict and batched modes; group commit only when each ack covers its sequence. Expose latency metric. Event/state outbox writes are atomic. `checkpoint_durable()` syncs log before checkpoint and returns highest contiguous acknowledged sequence; ack hook notifies executor, which alone removes acknowledged outbox entries.

Event framing is length+checksum. On open truncate incomplete or bad-checksum final record before append; middle corruption fails closed. Exactly one serialized writer, at most one duplicate per ambiguous append. Proof of durability is only (a) original append's successful sync, (b) fresh successful append of same event id, or (c) durable checkpoint ack whose watermark covers sequence. Found-but-unconfirmed tail, `contains`, in-memory snapshots or retrying fsync after sync error are never proof. Restart replays by ordinal and never adopts a found tail. Same event id/different payload digest is poison+incident, never dedup.

Synchronous append failure attributed to an event poisons the chain. While poisoned, only reconciliation rewrite of that exact id is allowed; unpoison only on that rewrite's own successful write+sync. Background-flusher failure is not attributed poison: retain failed batch, mark degraded and do not advance contiguous watermark. Prepare barrier failure leaves Prepared/reservations held. Transition names: Prepared=`destruction_prepared`; Quiescing=`destruction_quiesced`; Destroying=no event; Cataloguing=`destruction_catalogued`; Destroyed=`destroyed`; final Blocked=`destruction_blocked`; Rejected=`destruction_rejected` direct, best-effort, not outboxed.

### 3f. Reservation view

Lifecycle reservations use exact `(db,repo)` keys and database-scope `(db,"*")`; check exact first, then wildcard. This is a lifecycle-layer design, not a claim that the merged quarantine cache uses a wildcard.

## 4. Protocols

### 4a. R2' destruction

Dispatch destructive DROP before ordinary handlers take control. `IF EXISTS` first checks overlapping incomplete operations and returns `destruction_in_progress`, not silent no-op.

| Step | Action | Durable state/recovery |
|---|---|---|
| Preflight | Authorize; an existing row can only return/claim an originally authorized row. Resolve targets; require a quarantined referencing subscription unless explicit unbound-repo route; verify preview digest, ownership/components/overlap/backend and reservations. Acquire namespace admissions in global `(db,repo)` order before control/write locks and revalidate identity. | Challenge failure writes no row/no authority; unsupported backend refuses before Prepare. |
| Prepare | Under control, one transaction DELETE-IF-MATCHES challenge + PUT-IF-ABSENT Prepared + reservation publication; flush once. Release control; append durable barrier. | Barrier failure/timeout leaves Prepared fenced, no deletion; retry same event, never bypass challenge. |
| Quiesce/drain | `drain_targets(targets,generations)->DrainReceipt`: under control invalidate/drain affected leases via `quiesce_subscription_lease_locked`, mark subscriptions `retiring` so reconcile/start skip them; release control; stop/join only affected loops with bounded timeout (timeout→Blocked); retake control, verify generations unchanged and no lease remains. | Receipt required before detach. Never join while control held. |
| Detach/close | Short control section; opaque operation-scoped authority applies only to frozen targets; unregister/remove catalogue association with propagated errors; close backend without checkpoint. | Intent causes boot skip. Only destruction path can bypass binding guard, only for frozen targets. |
| Destroy | Retire admission, drain permits, destroy recorded component paths incl sibling WAL/hybrid durable tiers/seed remnants, verify absence. | Partial deletion resumes from descriptors; no ordinary open/recovery. |
| Catalogue | Remove rows transactionally, one flush, propagate errors; `remove_database` flush fix in 8b-3a. | Unknown flush→force flush+reread; absent advances, present retries. |
| Complete | One durable transaction marks Destroyed and clears destroyed-incarnation incidents as ByDestruction; flush and verify no object/rows/components remain. | Operation reservation remains until terminal audit drains; anchor remains until R1. |

### 4b. State/retry rules

`Prepared`: no destruction until audit barrier succeeds; boot publishes reservation and retries barrier. `Quiescing`: repeat targeted drain idempotently. `Destroying`: verify component absence then delete remainder from frozen descriptor. `Cataloguing`: force flush and reread before retry. `Destroyed`: status-only retry; retire when outbox empty. `Blocked`: retain reservation and retry from safe restart phase. `Rejected`: no reservation/action and new id required. Unknown writes are reconciled by forced flush then reread; never rollback on failed acknowledgement.

### 4c. R1 subscription removal

**Stage 1:** require all bound destinations absent, unregistered and uncatalogued, pending terminal causes resolved, and no incomplete operation. Persist `SubscriptionRemoval` keyed by `(subscription,binding_generation)` with frozen targets and `Pending`; delete with predicate on name AND recorded generation; commit/flush, force flush+reread on uncertainty; mark `RowDeleted`; verify frozen targets absent; rebuild coverage under control, release only this binding's reservation, set `Completed`. Resume uses record, never deleted/rebound row. Different live generation is never deleted; complete old record and release only its claims. RowDeleted re-verification failure becomes Blocked; discovered data becomes R2' target or incident, never released by archive.

| Durable prefix | Recovery |
|---|---|
| No record | First submission; row must exist |
| Pending, row present | Revalidate frozen set/generation; delete matching generation only |
| Delete flush unknown | Force flush+reread; absent→RowDeleted; same generation→retry; different generation→complete without deleting |
| RowDeleted | Reverify frozen set; complete or block fail-closed |
| Completed | Return status; never reexecute |

**Stage 2 ordered handoff:** for every destination upsert/reuse incident keyed by cause+storage id and durably verify catalogue copy; also persist/verify full repo copy if openable. Absent with completed destruction gets no incident; absent without covering destruction gets unresolved catalogue incident and remains reserved. Then targeted drain, release control during joins, revalidate generation/lease under control, delete row with durable flush/reread, release binding reservation only; incident protections remain. Retry is idempotent by cause/id.

### 4d. Boot replay

1. Before repository open load nonterminal Repo/Database rows and all nonempty outboxes. Load SubscriptionRemoval for phase-based resume, not audit replay.
2. Rebuild reservations from anchors, nonterminal operations, Destroyed rows with pending terminal audit, stage-2 incidents/repo evidence, and Pending/RowDeleted removal destinations. Rejected never reserves.
3. Replay pending audit by transition ordinal; executor owns ack updates.
4. Skip/fence reserved targets before any open/recovery/publication.
5. Resume each operation by its state/phase; periodically claim authorized stranded Prepared rows and retry barrier. Quarantine rebuild cannot weaken reservations.

### 4e. Lock order, drains and migration

Observed lock facts and full inventory: 131a Appendix 13. Decided order: operation token → database gate at final mode (shared repo scope, exclusive db scope) → sorted `(db,repo)` admission mutexes → `repo_create_locks` if needed → control → lease gate → write admission. No reverse acquisition. CREATE DATABASE exclusive; CREATE REPO shared plus target/create mutex. Migration acquires both namespace admissions globally ordered before control/write locks, revalidates identity and refuses reserved targets; case mandatory in 8b-3c. Targeted drain always releases control before joining; revalidate under control after joins. `stop_all` is rejected for destruction.

### 4f. Cancellation and duplicate submission

Handler returns only after authorized Prepare and successful audit barrier; epoch CAS precedes immediate executor spawn. Detached work survives caller cancellation. Every claimant conditionally CASes its observed `(record_version,executor_epoch)`; losers return status. Executor checks epoch before each destructive durable step. Existing id lookup may return/claim only an already-authorized frozen row; failed challenge creates no row and cannot be remedied by same-id submission. Changed frozen params conflict. `destruction_retry` can resume a blocked or already-authorized stranded Prepared record; it cannot create authorization. Common bounds are in 131b Appendix A.

## 5. Backend contract

Every backend implements lifecycle describe/close/destroy/verify-absent. Close stops workers and handles without requiring checkpoint. Destroy is idempotent and acts only on recorded owned components; absence is `Absent|Present|Unknown`. In-memory completion uses registry/catalogue absence. Durable components include repository store, metadata/interner, transaction state, changelog, identities/bookmarks, receipts, sibling `<repo>.shamirwal`, hybrid durable tiers and seed remnants.

Path-safe deletion uses recorded paths, never reconstructed names; reject traversal, aliases, symlink/junction/reparse traversal and system-store overlap. Use no-follow/handle-relative protections or equivalent check/use exclusion and parent durability barriers. Arbitrary out-of-root components require reviewed backend capability. Installer maintenance lock applies where installer path is involved; ordinary R2' does not depend on it.

**Retire fence:** `retire` linearizes by setting retired flag under repo admission mutex, then bounded-drains permits; timeout fails closed. Every `admit`, `admit_with_reservation`, `admit_pair` checks retired flag under same mutex; `repo_wal()` creation requires live permit. Retire → drain → unlink. The full admission inventory is in 131a Appendix 13; it supersedes the earlier three-site claim. Stale clones cannot recreate WAL after retire. Fjall stale-handle behavior and Windows delete-pending absence are CI probes; until verified, any recreated path is Present/Blocked.

