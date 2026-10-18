# PR8b-3: destination lifecycle — incident authority, destruction, reservations

**Status:** design brief (round 3). Supersedes nothing; supplies the exit that PR8b-2 (#188) truthfully documents as missing. The implementation will be done later by rush executors **from this document alone**; it is the durable artefact if every other artefact is lost. Split across three files:

- **This file** — §1-5: goal, invariants, data model, protocols (R2', R1 stage 1 + stage 2, boot replay, locks, cancellation), backend contract.
- [`131b-s5cx-destination-lifecycle-surface.md`](131b-s5cx-destination-lifecycle-surface.md) — §6-8: DDL/wire/builder/client surface incl. the injective canonical confirmation encoding, the complete creation exclusion matrix (incl. the NamespaceAdmission guard and publication-API inventory), seed install changes; plus the durable-schema appendix (all record schemas, types, bounds) and the administrative inspect/retry API.
- [`131a-s5cx-destination-lifecycle-evidence.md`](131a-s5cx-destination-lifecycle-evidence.md) — §9-12: stage-specific CI evidence, PR staging, risks, PR8b-4 appendix, corrections + verified-fact appendices.

All `OBSERVED` file:line citations were re-verified in this worktree at origin/master @ b67a3c46 during the round-2/round-3 rewrites; items that live only on the #188 branch are marked **REQUIRES #188**.

Notation: **OBSERVED** = read in this checkout (with file:line). **DECIDED** = design decision made by the owner or by this document as author; do not re-litigate in implementation PRs. **INFERRED** = consequence derived from observed facts. **REQUIRES #188** = depends on the PR8b-2 branch; the relied-on contract is stated inline.

## 1. Goal and non-goals

**Goal.** Give quarantined replication destinations a safe, crash-recoverable, source-independent exit: explicit physical destruction of a destination's storage, release of its reserved name, and removal of the (otherwise immortal) subscription row — without ever serving destroyed or half-destroyed data, and without ever deleting anything but the frozen target set.

**DECIDED — authority staging (stated once here, applied everywhere).**

- **Stage 1 (PRs 8b-3a..3c)** introduces storage-object ids + ownership descriptors, the backend lifecycle contract, the destruction journal, name reservations, NamespaceAdmission guard (round 3: ONE mechanism with the per-target lifecycle exclusion — surface file §7a), the DDL/error surface, the audit Prepared-barrier + in-record durable outbox, and creation exclusion. Incident authority **remains** the #188 terminal/quarantined subscription-row coverage. R1-stage-1 is gated on destination **absence** (not live, not catalogued, no owned component) — a deliberately temporary precondition.
- **Stage 2 (8b-3d)** moves incident authority to the **incarnation**: a catalogue incident record (`destination_incidents`) plus repository-carried versioned consistency evidence (`MetaKey::ConsistencyIncidents`); subscriptions are reduced to provenance. R1-stage-2 (subscription deletion) then proceeds by the **ordered incident handoff** (§4c) and no longer depends on destination absence; R1-stage-1 collapses into ordinary destruction completion.
- **Permanent from stage 1:** storage-object id format, ownership descriptor format, destruction journal schema + location, reservation semantics, NamespaceAdmission guard, backend lifecycle contract, the ten typed error codes, the DDL surface, the v2 canonical confirmation encoding, the audit outbox semantics. **Deliberately replaceable:** the R1-stage-1 absence precondition (relaxed in 8b-3d by the incident handoff); quarantine coverage derivation from subscription rows; the journal's `incident_ref` starts empty and is filled by 8b-3d. **No throwaway work:** every stage-1 artefact is consumed unchanged by stage 2 — descriptors become the incarnation's component list, reservations become incident-derived, and the executor/boot-replay/audit-outbox machinery is identical in both stages.

**DECIDED — what stage 1 makes permanent vs what stage 2 replaces (the F2 table).**

| Row | Stage 1 (8b-3a..3c) | Stage 2 (8b-3d) | Permanence |
|---|---|---|---|
| Incident authority | Terminal/quarantined subscription-row coverage (**REQUIRES #188** coverage set) | `destination_incidents` row **and/or** repo-carried `MetaKey::ConsistencyIncidents`; subscription row is provenance | Stage-2 form permanent |
| Reservation source | Anchor coverage + non-`Destroyed` operation targets | Same union, anchor contribution derived from incidents (unresolved incident ⇒ its cause still reserves) | Source list permanent; anchor entry re-derived |
| R1 precondition | Every bound destination absent, pending work resolved, lease drained (§4b) | Ordered handoff (§4c): incident durable → lease drained → row delete → reservation release; absence not required | Stage-2 protocol permanent |
| Journal `incident_refs` | `Vec<IncidentRef>`, always empty | Refs (`cause {subscription, generation}`, `storage_object_id`, `incident_id`) of the causes the destruction resolves | Field permanent, populated in stage 2 |
| Coverage derivation | Boot rebuild from terminal subscription rows (core.rs:525-527 routine) | Boot rebuild from terminal rows **and** incident rows **and** repo-carried evidence; either unresolved copy blocks | Union permanent |

**Non-goals (this series).**

- Identity-preserving RENAME of replication destinations; splitting source coordinates from destination identity. That is the PR8b-4 series (§12 in the evidence file). R3 stays a restriction; the `replication_destination_bound` message must say the restriction is a **format limitation**, not permanent.
- Ordinary (non-destructive) DROP/RENAME becoming crash-atomic catalogue transactions. Ordinary DROP/RENAME must respect reservations and storage ownership, but a full atomic-DDL rewrite is out of scope.
- In-place reseeding, multi-target seed recovery, Windows seed installation (8b-3e documents them as unsupported; destruction + name release ARE supported on Windows).
- In-place replacement of a destroyed destination's storage into the CURRENT incarnation. **DECIDED:** this series supports exactly ONE recovery path — destroy (R2') → R1 → fresh install into the ABSENT target as a NEW incarnation (surface file §8); in-place replacement is listed as a follow-up.
- Online migration; enterprise/tamper-evident audit subsystems.

## 2. Invariants

Numbered, each independently testable (CI assertions in the evidence file §9; stage attribution there too).

1. **I1 — Protection outlives configuration deletion.** A destination whose consistency is unresolved stays protected (unreadable, unreservable, uncreatable-over) until its data is durably destroyed or a validated recovery clears quarantine. Deleting a subscription row never by itself clears protection — in stage 2 the row delete is preceded by a durable incident write precisely so this holds (§4c).
2. **I2 — Authorization precedes destruction and outlives partial action.** No destructive step runs before (a) a `Prepared` destruction record (operation id, frozen target set, storage-object ids + component descriptors, actor, reason, nonce) is persisted **and flushed**, and (b) the `destruction_prepared` audit event is durably appended (§3e). **DECIDED (round 3):** an audit failure at the barrier NEVER leads to `Rejected` by inference — the strict append is `write_all` + `flush` + `sync_all` (**OBSERVED** audit_appender.rs:498-519 `append_strict`), so a failed final sync does NOT prove the write is absent. The operation STAYS `Prepared` with reservations UNPUBLISHED (reservations publish only after a SUCCESSFUL barrier) and nothing deleted; the caller receives the NEW retryable wire code `destruction_audit_unavailable` (surface §6c — a TENTH code), and the SAME event id is retried per the §3e reconciliation-read rule. The intent (journal + audit chain) remains the recovery authority across every crash, cancellation and restart, until the record reaches `Destroyed` or `Blocked`.
3. **I3 — Boot loads intents before any repository loader.** `ShamirDb::init` reads (a) all non-`Destroyed`/non-`Rejected` rows AND (b) ANY row with NON-EMPTY `audit_pending` (ANY state, incl. `Destroyed`, for outbox replay) from `destruction_operations` BEFORE the repository-open loop; a target covered by an open operation is never opened, recovered, or published.
4. **I4 — Reserved names reject creation on every publication path.** While a terminal/quarantined anchor or a non-`Destroyed` **and non-`Rejected`** operation — or a `Destroyed` operation with non-empty `audit_pending` (released when the pending set drains, §3e/§4d) — covers a name, CREATE DATABASE / CREATE REPO / config add-repo / engine publication / seed install / subscription binding / rename-into are all rejected with `destination_name_reserved` (full matrix + NamespaceAdmission guard: surface file §7). Nothing is ever "born quarantined".
5. **I5 — Destruction is complete and verified.** Completion requires every component in the ownership descriptor (repository store, interner/meta, transaction state, changelog, identity/bookmarks, seed receipts, the **sibling `.shamirwal` WAL directory and its segments**, persistent tiers of cache/hybrid backends, seed staging remnants per surface-file §8) to be destroyed and verified absent.
6. **I6 — Retry never deletes anything but the frozen target set.** All destructive work is bound to storage_object_id + recorded descriptors, never to "whatever occupies the name now". A retry against a live replacement is an error (`unsafe_storage_ownership`), never a deletion.
7. **I7 — No control lock across slow deletion.** The replication control lock is held only for short critical sections (§4e lock order): validation, reservation publication, state transitions, the final destructive catalogue write. Component destruction runs under a per-operation executor token plus per-target lifecycle exclusion.
8. **I8 — R1 never orphans protection.** R1-stage-1 permits DROP SUBSCRIPTION of an anchor only when **every** bound destination is absent and no incomplete operation covers any of them. R1-stage-2 permits it only after every bound destination's incident evidence is durably handed off (§4c), **and no unresolved pending terminal cause for any referencing generation**. The deletion itself NEVER resolves an incident — only validated recovery (the §3d predicate) or destruction completion does. In both stages the union of protections is non-increasing until the handoff/destruction completes.
9. **I9 — Destination-by-destination and cause-by-cause.** Multi-destination subscriptions are destroyed destination by destination; there is no implicit graph-wide destruction, and surviving destinations keep their protection. Incident causes are independent: one incident per (cause, storage_object_id); clearing a cause clears exactly that cause's copies.
10. **I10 — Audit is a barrier at Prepare, an in-record outbox afterwards; the journal is authority.** The `Prepared` transition requires a durable audit append (I2). Later transitions enqueue their audit event INTO THE OPERATION RECORD in the SAME durable CAS write as the state change (§3e); an un-drained `audit_pending` only withholds retirement of the operation and the lapse of the reservations it anchors — it never rolls back a transition.
11. **I11 — Only validated recovery or destruction completion clears quarantine on surviving data.** Operator acknowledgement, subscription deletion, rename, and destruction of a *sibling* destination never clear quarantine on a surviving incarnation; only a replacement verified against the incident's recorded baseline (§3d predicate) or destruction completion resolves an incident (stage 2: the handoff writes the incident BEFORE the row delete, so deletion is provably never the cure).
12. **I12 — No clean checkpoint required.** Destruction never requires successfully checkpointing or opening corrupt data; it requires workers stopped and handles closed.
13. **I13 — Admission closes the check-vs-publish race.** Every publication path takes the admission guard for its (db, repo) namespace before validation and holds it until publication or abort (RAII; cancellation-safe; no durable trace); reservation mutation takes the SAME guard, so there is no check/use gap. The same object serves as the per-target lifecycle exclusion — admission guards and lifecycle exclusions are ONE mechanism (§4e; surface file §7a). **DECIDED (round 3):** the round-2 generation-counter fallback is REMOVED entirely — no generation token, no second invalidation path; holding the guard across check-and-publish is the only serialization.
14. **I14 — Unknown is not success.** Ambiguous I/O (a write/flush that errored but may be durable) is reconciled by re-reading durable state; until reconciled, conservative fencing (reservations + fail-closed reads for the target) stays in force. A failed acknowledgement is never a rollback (§4f).

## 3. Data model

All new durable state lives in the Fjall-backed system repo at path `<data_dir>/shamir_db_meta.redb` — a Fjall directory despite the file-style suffix — **OBSERVED** (system_store.rs:57-64 `SystemStoreConfig`, :103 `SystemStoreConfig::Fjall → BoxRepoFactory::fjall`; installer.rs:118-120; the "redb single-writer" comments at access_tree.rs:150 / doctor.rs:216 are stale code comments), except the repository-carried records noted per artefact. Full schemas, concrete types and bounds for every record: **surface file, Appendix A**.

### 3a. Storage-object id + ownership descriptor

**DECIDED.**

- New per-repository durable meta record `MetaKey::StorageOwnership` in the repo info store, codec versioned like `MetaEnvelope` in `crates/shamir-engine/src/meta/repo_identity.rs:79-117` (version 1; NotFound at open → mint lazily only via a controlled admission path, never during destruction).
  Format v1: `{ storage_object_id: [u8;16], durable: bool, components: Vec<ComponentDescriptor>, created_at: u64, format_version: u16 }` where `ComponentDescriptor = { component_id: u32, kind: ComponentKind, path: Option<PathBuf>, bytes_repr: Option<String> }`; `component_id` is stable per descriptor (assigned once at descriptor creation, never renumbered) and is what `destruction_blocked` diagnostics and the record's `blocked_component` reference — **DECIDED** (round 2; an index into the frozen `components` vector is not stable across retry-time re-derivation, so the id lives in the descriptor). `ComponentKind` enumerates: PrimaryStore, Interner, Meta, TransactionState, Changelog, IdentityBookmarks, SeedReceipts, Wal{dir, segments}, CacheTier, HybridDurableTier, MemoryInstance, SeedStagingDir, SeedInstallMarker, SeedReceiptStore (the last three per surface-file §8; all seed remnants of a seeded target are owned components).
- **Mirror:** the system-catalogue `repositories` row gains `storage_object_id: [u8;16]` and `ownership_digest: [u8;32]` (hash of the descriptor). On open, record vs row disagreement fails closed with `unsafe_storage_ownership`. This catches catalogue/live aliasing (RENAME keeps the old directory while CREATE derives paths from names — **OBSERVED** admin_db_repo.rs:266, :291 `create_dir_all` from name-derived dirs; db_management.rs rename re-keys rows best-effort; **INFERRED**: without the mirror, a renamed-then-recreated repo is indistinguishable from the old storage).
- `durable:false` for `in_memory`: id minted per boot, recorded only in the catalogue row; no on-disk components exist and no fs absence proof is possible. **DECIDED:** the storage-object id must NOT reuse `RepoIdentity::lineage_id`: **OBSERVED** repo_identity.rs:24-34 — `in_memory` AND `hybrid` keep `__tx__` in RAM only, so every process start mints a new lineage at epoch 0 by design; a hybrid's lineage_id is unstable across boots and would make journal/destruction identity meaningless. StorageOwnership instead is written into the **durable metadata/interner tier** for hybrid (open risk in §11 of the evidence file).
- Writers: only the repository open/admission path (mint), the destruction executor (record retirement), and boot replay. Nobody else may mutate.

### 3b. Destruction operation record

**DECIDED.** New system-catalogue table `destruction_operations` (a new table on the `SYSTEM_REPO` Fjall system repo, alongside `subscriptions` — **OBSERVED** system_store.rs:55 `TABLE_SUBSCRIPTIONS` (comment :52-54), table registration at :119), keyed by `operation_id` (caller-generated string, bounds and reuse rules in §4f / surface-file Appendix A). Format version 1, row:

```text
{ operation_id: String(≤64, [0-9A-Za-z._-]+), format_version: u16 = 1,
  kind: Repo | Database,
  targets: Vec<TargetRef>,              // frozen at Prepare:
                                        //   { db_name, repo_name, storage_object_id: [u8;16],
                                        //     ownership_digest: [u8;32],
                                        //     subscription_anchors: Vec<{ name: String, generation: u64 }> }
  components: Vec<ComponentDescriptor>, // frozen union of descriptor components (3a, with component_id)
  anchor_subscriptions: Vec<String>,    // provenance summary of targets[].subscription_anchors
  actor: String(≤255), reason: String(1..=512),
  incident_refs: Vec<IncidentRef>,      // empty in stage 1; filled by 8b-3d (IncidentRef: surface A.3)
  nonce: [u8;16],                       // caller's confirmation nonce, persisted with the operation
  state: Prepared | Quiescing | Destroying | Cataloguing | Destroyed | Blocked | Rejected,
  failed_phase: Option<Phase>,          // set on Blocked/Rejected
  error_class: Option<ErrorClass>,      // ExternalLock | PermissionDenied | UnsupportedBackend
                                        //   | OwnershipMismatch | UnknownIo
                                        //   (wire codes: surface file §6c)
  safe_restart_phase: Option<Phase>,    // the phase to resume from on retry
  blocked_component: Option<u32>,       // component_id from 3a
  last_error: Option<String>,
  audit_pending: Vec<PendingAuditEvent>, // in-record outbox (§3e); PendingAuditEvent =
                                        //   { event_id: {operation_id, transition_ordinal}, kind,
                                        //     payload_digest, audit_seq: Option<u64> }; entries whose
                                        //   audit_seq ≤ ack_watermark are removed by the ack hook (§3e step 5)
  record_version: u64,                  // optimistic-concurrency CAS token; NOT part of the event id
  created_at, updated_at: u64 }         // ns since UNIX_EPOCH
```

Round-2 additions and why — **DECIDED**:

- `targets[].subscription_anchors` carries **generations**, not just names: anchor coverage and incident attribution are generation-scoped (surface-file §8 pending-cause model, `MetaKey`-level attribution); a name alone cannot distinguish a replaced binding.
- `audit_pending` implements the **in-record outbox** (§3e, DECIDED round 3): each transition enqueues its audit event INTO the operation record in the SAME durable CAS write as the state change; an append success writes the chain seq into the pending entry (diagnostic); the periodic checkpoint ack hook REMOVES entries whose `audit_seq ≤ ack_watermark` (one durable CAS batch per checkpoint). The row-level `audit_seq_emitted`/`audit_seq_acked` fields are DROPPED. Boot/recovery selects replay work by NON-EMPTY `audit_pending` (never by seq comparison), and retirement requires `state = Destroyed` AND empty `audit_pending`.
- `record_version` is the optimistic-concurrency token for executor self-replacement after crash. **Round 3:** it is NO LONGER part of the audit event id — `event_id = (operation_id, transition_ordinal)`, independent of `record_version` (§3e).
- `Rejected` is a new **terminal** state reserved for DEFINITIVE PRE-WRITE REFUSALS (authorization, validation, reservation conflict) BEFORE any audit append. No reservation was ever published (reservations publish only after the barrier succeeded), no destructive step ever ran, and the only audit event is `destruction_rejected` (best-effort, exempt from the retirement gate). **An audit failure at the barrier does NOT produce `Rejected`** — the operation stays `Prepared` (I2).

State meanings and durability rules — **every transition persists the new row and flushes before the next step starts**:

| State | Meaning | Durability rule |
|---|---|---|
| `Prepared` | Authorized; targets, ids, descriptors frozen; reservations published; `destruction_prepared` audit event durably appended (barrier, §3e — its pending entry stays in `audit_pending` until the ack hook drains it). | Row + flush, then barrier append, BEFORE any destructive action (I2). |
| `Quiescing` | Admission closed, leases drained, workers/followers stopping. | Persisted on entry, with the `destruction_quiesced` event enqueued in the SAME write; re-entering is idempotent. |
| `Destroying` | Components being destroyed (subset may be gone). | Persisted on entry; per-component progress optional, absence re-checked on retry. The `Destroying` transition emits no dedicated audit event (it is covered by the durable row + the subsequent catalogued/destroyed events); intermediate events are optional. |
| `Cataloguing` | Physical destruction done; catalogue rows being removed with error propagation. | Persisted on entry. |
| `Destroyed` | Terminal: no live object, no catalogue rows, all components verified absent. | Persisted + flushed; retirement only when `audit_pending` is empty (§3e). Anchors now removable via R1. |
| `Blocked` | External error (sharing violation, permission, unsupported backend, ownership mismatch, unknown I/O). Reservation retained; retryable; carries `failed_phase`, `error_class`, `safe_restart_phase`, `blocked_component`. | Persisted + flushed with diagnostics. |
| `Rejected` | Terminal; definitive pre-write refusal (authorization, validation, reservation conflict) BEFORE any audit append. | Persisted + flushed; no reservation was ever published (reservations publish only after the barrier succeeded); no destructive step; `destruction_rejected` is best-effort and exempt from the retirement gate. An audit failure never lands here (I2). |

Writers: the single per-operation executor only, guarded by `record_version` compare-and-swap.

### 3c. Name reservations (in-memory authoritative view)

**DECIDED.** Reservations are an **in-memory view**, derived at boot and maintained under the control lock — **not** separately persisted. Members: (i) every target name covered by #188 terminal/quarantined subscription-row coverage (**REQUIRES #188**: the coverage set derived from terminal rows as in `repl_binding/coverage.rs` — stage 2: derived additionally from `destination_incidents` rows and repo-carried evidence); (ii) every target of a non-`Destroyed` **and non-`Rejected`** `destruction_operations` row; (iii) every target of a `Destroyed` row whose terminal audit event is unacked (released when the ack lands, §3e/§4d). Rationale: the two persisted sources are already the authority; persisting reservations a third time would create a third durable truth that can disagree after partial writes. The view is rebuilt identically at boot from those sources, so it is reconstructible by construction.

Key normalization (surface file §7 gives the guard that consumes it): a reservation key is `(db: String, repo: String)` with `repo = "*"` for database-scope (a database destruction op, or an anchor with `db: None`, covers every `(db, *)`); lookup normalizes exact-match first, then `repo = "*"` (the same two-step pattern as the existing quarantine cache `reason_for` lookup, **OBSERVED** quarantine_cache.rs:39-53).

**Hierarchical admission (DECIDED round 3):** namespace admission is TWO-LEVEL: a per-database RW-style gate (repo-scope operations take it SHARED; database-scope operations EXCLUSIVE) plus per-(db, repo) mutexes beneath it, always acquired in SORTED order for multi-target operations; the per-target lifecycle exclusion is the SAME merged guard object (§4e). Reservation mutation takes the SAME guard structure, so there is no check/use gap. The round-2 generation-counter fallback is REMOVED explicitly — no generation token, no second invalidation path; holding the guard across check-and-publish is the only serialization (I13). Separate note: the quarantine cache gates READS only (**OBSERVED** db_execute.rs:76-90 reads-only gate; :131-150 walks `BatchOp::Read`); lifecycle WRITE fencing uses the engine write-admission seam (`RepoWriteDomain`/`RepoWritePermit`, **OBSERVED** db_instance.rs:79-96) — the cache never fences writes.

### 3d. Incident record (8b-3d — designed now, implemented in stage 2)

- Catalogue table `destination_incidents` (system store, format v1), concrete types — **DECIDED** (round 2):

```text
{ incident_id: [u8;16],
  storage_object_id: [u8;16],
  db_name: String(≤255), repo_name: String(≤255),
  cause: { subscription: String(≤255), generation: u64 },
  state: Unresolved | Cleared,
  cleared_by: Option<{ evidence_digest: [u8;32], cleared_at_ns: u64 }>,
  created_at_ns: u64,
  reason: String(1..=512),
  // recovery baseline (DECIDED round 3) — a replacement is verified against EXACTLY these:
  receipt_artifact_digest: [u8;32],
  source_lineage_id: [u8;16],
  source_history_epoch: u64,
  bookmark_floor: u64 }                  // the bookmark at quarantine time
```

- One incident per `(cause, storage_object_id)` — a cause that binds multiple destinations produces one incident per destination; a destination covered by multiple causes holds one incident per cause (I9). `incident_id` is a fresh random 16 bytes at creation; retry/re-execution of the handoff **reuses** the existing incident id for the same `(cause, storage_object_id)` pair (idempotent by cause pair, §4c).
- Repository-carried `MetaKey::ConsistencyIncidents` (MetaEnvelope v1, same codec discipline as 3a): versioned list of unresolved copies. **DECIDED (round 3):** the repo-carried record holds the FULL incident copy — ALL fields above, including the recovery baseline (`receipt_artifact_digest`, `source_lineage_id`, `source_history_epoch`, `bookmark_floor`), not only id + digest — so catalogue rows can be REBUILT from it; **boot rebuilds missing catalogue rows from the repo copy when the repo is openable.** **Either** unresolved copy (catalogue row or repo-carried record) blocks exposure; disagreement is unresolved state, not permission to pick the convenient copy. **DECIDED (F2 boot derivation):** at boot, the quarantine/reservation union is derived as: an incident row with `state = Unresolved` for a **live** `storage_object_id` quarantines reads for that (db, repo); repo-carried unresolved evidence quarantines **even without a catalogue row** (a repo opened whose `ConsistencyIncidents` is non-empty is fail-closed quarantined and its incident list is re-published into the catalogue as Unresolved rows if missing); either copy blocks.
- **Recovery-clear predicate (exact; normative form in surface file A.3):** an incident is resolved ONLY when a replacement incarnation is verified against the recorded baseline — fresh `storage_object_id` ≠ the incident's, installed receipt `artifact_digest == receipt_artifact_digest`, `lineage_id == source_lineage_id`, `history_epoch == source_history_epoch`, and the new bookmark starts at `bookmark_floor + 1`; the clear rewrites exactly that incident's copies (row + repo record) by `incident_id`. **Anything else — operator acknowledgement, subscription deletion, rename, sibling destruction — never clears (deleting a subscription NEVER resolves an incident).**
- Stage 1 reserves only the schema names and the journal's `incident_ref` field. The #188 subscription-row anchor remains the authority until 8b-3d lands.

### 3e. Audit events, the barrier, and the durable outbox

**OBSERVED:** `AuditAppender` (`crates/shamir-connect/src/server/audit_chain.rs:341-349`) returns nothing from `append_entry`/`checkpoint`; `AuditChainWriter::append` (:406+) returns the entry, not an ack; strict mode syncs per entry, BATCHED buffers ≤5 s (`crates/shamir-server/src/audit_appender.rs:1-20`: strict = write + `sync_all` per entry; batched = in-memory buffer, background flush ≤5 s; checkpoints always `SyncAll`). **OBSERVED (round 3):** the strict append is `write_all`, `flush`, `sync_all` (audit_appender.rs:498-519, `append_strict`) — a failed final sync does NOT prove the write is absent. The periodic scheduler task calls `chain.checkpoint()` and the void-returning `appender.checkpoint()` (crates/shamir-server/src/scheduler.rs:204-216), which logs a failed buffer flush and STILL writes the checkpoint (audit_appender.rs:706-713); `AuditChain::checkpoint()` returns `(next_seq, prev_hmac)` where `next_seq` is the NEXT seq to assign (audit_chain.rs:185-223), not an inclusive acknowledged seq; `checkpoint_now` (audit_chain.rs:436-441) is a manual force, NOT a durability barrier. **INFERRED:** existing audit cannot certify "intent is durably recorded" nor "everything through seq N is durable".

**DECIDED (round 3 — in-record outbox; replaces the round-2 seq-field design).**

1. **New appender call.** `append_entry_durable(&AuditEntry) -> Result<AuditAck>` where `AuditAck = { seq: u64, checkpoint_position: [u8;32] }`. Guarantees: in **strict mode**, the entry is written and `sync_all`ed before return (the existing strict behaviour, audit_appender.rs:13-15/:498-519, wrapped so the sync result is observed). In **batched mode**, THIS call alone flushes the buffer synchronously (write + `sync_all` of the pending batch containing this entry) before returning — "flush-before-return for this call only"; the background ≤5 s flusher is unchanged for other callers. An `Err` return means **durability unknown** — the entry MAY be durable (see item 6).
2. **Transition event list — DECIDED (round 3), stated once, everywhere:** `Prepared`→`destruction_prepared` (barrier, item 3); Quiescing→`destruction_quiesced`; **Destroying→(NONE — no dedicated event)**; Cataloguing→`destruction_catalogued`; Destroyed→`destruction_completed` (terminal, retirement-gating); Blocked→`destruction_blocked` (final Blocked, retirement-gating); Rejected→`destruction_rejected` (best-effort, enqueued but EXEMPT from the retirement gate). The name `destruction_destroyed` is not used anywhere.
3. **Prepared barrier.** In order: (i) control lock: persist + flush the `Prepared` row, RELEASE control; (ii) the durable audit barrier `append_entry_durable(destruction_prepared)` — audit I/O with NO control lock held (§4e); (iii) control lock: persist pending-entry bookkeeping and publish reservations LAST (only after the barrier succeeded). **If (ii) fails, the operation STAYS `Prepared`** — reservations unpublished, nothing deleted; it does NOT flip to `Rejected` (a failed strict sync does not prove the write absent — OBSERVED above); the caller receives `destruction_audit_unavailable` and the SAME event id is retried per item 6. `Rejected` exists ONLY for definitive pre-write refusals (authorization, validation, reservation conflict) BEFORE any audit append. The barrier exists so the audit chain independently proves the intent existed even if the journal were damaged.
4. **In-record outbox.** Each transition's durable CAS write (on `record_version`) atomically includes the new `audit_pending` entry: `PendingAuditEvent = { event_id: {operation_id, transition_ordinal}, kind, payload_digest, audit_seq: Option<u64> }` (the top-level replay order is `transition_ordinal` from the event id; it is not duplicated as a separate field) — INDEPENDENT of `record_version` (which still increments on every CAS write but is no longer part of the id). A later append success writes the returned seq into the pending entry (diagnostic). **A failure or ambiguous outcome in the append NEVER rolls back the state change.**
5. **Acknowledgement source — DECIDED (round 3):** specify a result-returning `checkpoint_durable() -> Result<AckWatermark, AuditError>` on the appender/writer that (1) flushes + fsyncs the log FIRST and FAILS if the flush failed (unlike the OBSERVED `appender.checkpoint()`, which logs and continues), then (2) writes the checkpoint keyspace durably (SyncAll) and returns `AckWatermark = next_seq - 1` — the highest assigned seq covered by the durable log + checkpoint. The scheduler path switches to `checkpoint_durable`; on success a hook REMOVES pending entries whose `audit_seq ≤ ack_watermark` from every loaded operation row (one durable CAS batch per checkpoint). An outbox event is acknowledged only when `ack_watermark >= event.audit_seq`.
6. **Ambiguous append / duplicate event ids.** The audit chain has NO dedup index (evidence file risk 8). **DECIDED:** a duplicate event id is a benign repeat. After an AMBIGUOUS append (an error whose durability is unknown — including a failed final sync), the executor first runs a RECONCILIATION READ of the chain tail (scan the in-memory chain entries via the `AuditChain::snapshot` + `verify_against_checkpoint` path) for an entry whose payload hash equals the pending entry's `payload_digest`; if found, adopt that entry's seq as `audit_seq`; if not found, re-append. The chain then contains a bounded number of duplicates — one per ambiguous append — which is acceptable and documented; **the journal, not the chain, is the recovery authority** (I10).
7. **Replay and retirement.** Boot/recovery selects operations by NON-EMPTY `audit_pending` (ANY state, incl. `Destroyed`) — NEVER by seq comparison — and replays every pending entry in `transition_ordinal` order, idempotently (same event id; item 6 governs duplicates). Retirement: an operation record (and its name reservations) is retired ONLY when `state = Destroyed` AND `audit_pending` is empty (the `Rejected` row's best-effort event is exempt; a final `Blocked` retires only via owner-approved archival). **Retention:** `destruction_operations` rows are retained forever unless an owner-approved archival API removes them (no automatic expiry — reservations must never silently lapse, I1); reservations derived from an operation may lapse only when its `audit_pending` has drained via the ack hook.
8. Emission content unchanged: `destruction_prepared/catalogued/completed/blocked/rejected/quiesced` carry actor, reason, operation id, storage-object ids, affected subscriptions/generations, outcome. The audit event is correlated with the journal but the **journal remains the recovery authority** (I10).

### 3f. Generation-attributed pending terminal evidence (round 2)

**OBSERVED:** the quarantine cache keys are destination-only `(db, repo) -> reason` with a `("*", "*")` global sentinel; `publish()` prepublishes keys and tracks them in `pending` on persistence failure; `install()` replaces contents from a confirmed snapshot, keeping prepublished keys until the snapshot confirms them (**OBSERVED** quarantine_cache.rs:39-53 `reason_for` incl. sentinel, :55-64 publish, :65-77 install).

**DECIDED (F7-v).** Pending entries become generation-attributed: `(db, repo) -> Map<cause: (subscription_name: String, generation: u64), reason>`. Semantics:

- **Who writes:** the terminal marking path (`mark_subscription_terminal`, **OBSERVED** crates/shamir-db/src/shamir_db/execute/admin_replication.rs:860-876 — takes the control lock at :866) `publish()`es one pending cause per covered destination, attributed to the marking subscription + its binding generation.
- **Who retires:** the post-flush cache refresh (`install()`-equivalent) retires a pending cause **only** when the durable snapshot confirms that generation's terminal row write is durable; per-cause retention replaces the current whole-key retention. In stage 2, the incident handoff (§4c step a) is itself durable evidence: the incident row for a cause counts as that cause's confirmed copy for coverage purposes.
- **Effect on protocols:** R2'/R1 preflight refuses while ANY pending cause for a referencing generation is unresolved, with the coded error `quarantine_persistence_pending` (a terminal row alone is not proof its flush completed — the pending set exists precisely because of that window).

## 4. Protocols

### 4a. R2' — explicit destruction step table

Dispatched from `DROP REPOSITORY/DATABASE … DESTROY QUARANTINED DATA` (surface file §6). Columns: step / action / durable state after / crash-cancel result / retry.

| # | Step | Durable state after step | Crash / cancel result | Retry behaviour |
|---|---|---|---|---|
| 0 | **Preflight** (control lock, §4e): authorize; enumerate live **and catalogued** targets; verify every referencing subscription row is quarantined/terminal with **no unresolved pending cause for a referencing generation** (§3f, **REQUIRES #188** predicates); resolve ownership descriptors for every component incl. sibling `.shamirwal` (**OBSERVED** repo_instance.rs:963-985 — the WAL is a sibling `<repo>.shamirwal/` directory); reject unknown ownership, path aliases, incomplete operations overlapping the targets. Rejection → `Rejected` row (3b); no reservation, no destructive step. | `Rejected` row on refusal; none otherwise | A crash mid-persist of a `Rejected` row is harmless by design: best-effort, non-barrier, no reservation ever existed; a half-written row reconciles as `Rejected`. | Retry preflight. A terminal row alone is not proof its flush completed — check pending causes (§3f), not just the row. Repeat; rejects are coded (`quarantined_subscription_destinations_remain`, `quarantine_persistence_pending`, `unsafe_storage_ownership`, `destruction_operation_conflict`). |
| 1 | **Prepare**: the admission guards for every target namespace are ALREADY HELD from preflight start (merged admission/lifecycle exclusion, §4e — acquired before step 0 began). Then IN ORDER: (i) control lock: persist + flush the `Prepared` row, RELEASE control; (ii) the **barrier**: `append_entry_durable(destruction_prepared)` — audit I/O with NO control lock held (§4e); failure leaves the row `Prepared` (never `Rejected`, I2), caller receives `destruction_audit_unavailable`; (iii) control lock: persist pending-entry bookkeeping + publish reservations LAST (only after the barrier succeeded), release. The request handler **returns the operation id here** and the executor continues detached (§4g). | `Prepared` row + reservations + audit event appended (pending entry until acked) | A failed prepare barrier prevents any deletion (I2). Restart loads the row BEFORE repo loaders (I3); targets skipped fail-closed; executor resumes. | Idempotent by operation_id + identical frozen parameters. |
| 2 | **Quiesce** (no control lock; per-op executor + per-target lifecycle exclusion; lease gate inside target scope as needed): invalidate generation leases + drain apply gates (**REQUIRES #188** `lease.rs` write gate), close repo admission, stop/join workers, WAL timers, follower tasks for the target; revoke downstream deliveries. Outbox event (§3e). | `Quiescing` | Intent remains; restart skips the target and retries quiesce. Cancellation cannot restore serving. | Closure/drain repeated idempotently. |
| 3 | **Detach + close**: conditionally unregister via `DbInstance::remove_repo` (db_instance.rs:161-163 — entry removal only) / `ShamirDb::remove_repo`/`remove_db` (**on this tree public no-guard helpers**, db_management.rs:519+, :82-100; **REQUIRES #188** the `_locked`-with-typed-guard shape — the R2' path calls the guarded variants with the lifecycle capability); verify registry absence; close backend via `close_for_destruction`. Catalogue rows and intent kept. | `Quiescing`/`Destroying` on entry | Restart still skips opening it. A conflicting live replacement is `unsafe_storage_ownership`, never deleted. | Missing registration succeeds only after identity (storage_object_id) check. |
| 4 | **Destroy components**: backend `destroy(components)` — repo store, interner, meta, tx state, changelog, identity/bookmarks, seed receipts, **the sibling `<repo>.shamirwal` directory + segments** (descriptor component list), durable tiers of cache/hybrid composites, seed remnants per surface §8; namespace durability barriers. Bug #1370 fixed in 8b-3c so `DROP DATABASE` cascade targets `op.drop_db`, not the session db (`drop_table_cleaning_validators` mixed-coordinate call, **OBSERVED** admin_db_repo.rs:134 for drop-db, :398 for drop-repo cascade). | `Destroying` | Any subset may be gone; **no ordinary recovery/open is attempted** (I3). External failures → `Blocked` + `destruction_blocked` with `failed_phase`/`error_class`/`blocked_component`. | Absence re-checked per component; missing components are success; remaining deleted from the recorded descriptor. |
| 5 | **Catalogue removal** (control lock re-acquired, §4e): remove table rows, `repositories` rows, and for database destruction the `databases` row — errors **propagated** (contrast **OBSERVED** best-effort warn-only removal, db_management.rs:90-96, :523-534). Performed even if live registration was already absent. **Commit/flush window (round-2 fix):** the existing deletion primitive **commits an implicit transaction and only then flushes** — **OBSERVED** system_store.rs:192-213 `delete_via_implicit_tx` → repo_instance.rs:1274-1298 `run_implicit_batch_tx`, which commits at :1296 before returning; `remove_repository` flushes AFTER (system_store.rs:330-352, flush at :350-351) and `remove_database` currently does **not** flush at all (:262-275). **DECIDED:** keep the existing primitive; the executor treats "commit ok, flush unknown" as **UNKNOWN** (I14) and reconciles by re-reading the catalogue after a flush error; 8b-3c additionally adds the missing flush to `remove_database`. A crash between commit and flush is resolved at boot by the same re-read reconciliation before the step is retried. | `Cataloguing` | Leftover rows cannot be reopened: intent prevents loaders (I3). | Re-removal of absent rows succeeds. |
| 6 | **Complete**: revalidate no live object, no relevant catalogue rows, all components absent (`verify_absent`). Persist + flush `Destroyed`. Subscription anchors remain; reservations from the operation are released only when the terminal audit ack lands (§3e); anchor reservations persist until R1. Return operation id + remaining anchors. | `Destroyed` | Crash before flush → `Cataloguing`; replay from step 6 checks. | Completed retry with same id → current status. Old-op retry against a recreated repo must not touch it (I6). The two conflict codes are distinct: a retry whose recorded target set mismatches the live/catalogued state (recreated repo ⇒ different storage_object_id) fails preflight with `destruction_operation_conflict`; an object that MATCHES the recorded storage_object_id but fails ownership verification → `Blocked(OwnershipMismatch)` / `unsafe_storage_ownership` (see the `Destroying` row in §4b). |

**IF EXISTS** (DropDbOp/DropRepoOp `if_exists`, early exits **OBSERVED** admin_db_repo.rs:106-112, :367-368) must first inspect incomplete operations covering the target and return `destruction_in_progress` instead of a silent no-op.

### 4b. Full state machine: crash, cancellation, retry, visibility (F4)

One row per state; "recovery action" is what the boot executor does; "cancellation" is what happens when the caller's future is dropped at an await point inside this state (executor-owned work continues — §4g).

| State | What is durable | Crash-recovery action (exact) | Cancellation behaviour | Retry behaviour | Status API shows |
|---|---|---|---|---|---|
| `Prepared` | Row + reservations (in-memory, rebuilt at boot) + barrier-appended `destruction_prepared` (pending entry until acked). | Load before repo loaders (I3); publish reservations; on resume the executor FIRST completes the audit barrier append (idempotent) before any destructive action (I2); spawn/continue executor from Quiesce. | Executor continues to next durable boundary (Quiescing entry); caller's HTTP future is already gone (it returned at step 1). | Same-id identical params → status; different params → `destruction_operation_conflict`. | state, targets, components, actor, reason, created_at, audit state per `audit_pending` (drained = acked). |
| `Quiescing` | Row (state, versions, `audit_pending`). | Re-run quiesce idempotently (drain joins repeated). | Executor continues; drain cannot be undone by a dropped future. | Re-enter step 2. | + failed/safe-restart fields if previously Blocked. |
| `Destroying` | Row; per-component progress is optional and derived, not durable. | Re-derive component absence via `verify_absent`; destroy what remains from the recorded descriptor; a live conflicting object → `Blocked(OwnershipMismatch)` / `unsafe_storage_ownership`, never deletion (I6). This is the ownership-mismatch branch of the §4a step 6 disambiguation: `destruction_operation_conflict` is for a mismatched recorded target set (different storage_object_id), this one for an object that matches the recorded id but fails ownership verification. | Executor continues into Cataloguing or Blocks; no rollback of deleted components. | Resume from `safe_restart_phase`. | + blocked component id, error_class. |
| `Cataloguing` | Row. | Re-read catalogue (I14 reconciliation, step 5); remove remaining rows; errors propagate → `Blocked(UnknownIo)` with re-read evidence attached. | Executor continues to Complete. | Resume step 5→6. | + progress per component. |
| `Destroyed` | Terminal row + acked terminal event (or pending outbox replay). | Outbox replay if unacked (§3e); then retirement allowed. | n/a (terminal). | Same-id retry → current status, no re-execution. | terminal, anchors remaining. |
| `Blocked` | Row with `failed_phase`, `error_class`, `safe_restart_phase`, `blocked_component`, `last_error`; reservations retained. | Boot re-publishes reservations; executor stays idle until `destruction_retry` or external cause clears and an admin retries. | n/a (executor parked). | `destruction_retry(operation_id)` resumes from `safe_restart_phase` under a fresh executor token. | blocked diagnostics, retry hint. |
| `Rejected` | Terminal row. | Nothing; no reservation was ever published; no destructive step ever ran. | n/a. | New operation id required. | terminal, rejection reason. |

**Ambiguous I/O rule (I14, applies at every step).** A flush/write that returned an error but whose data MAY be durable ⇒ the outcome is **UNKNOWN**: reconcile by RE-READING durable state (catalogue rows, component absence, incident copies); until reconciled, conservative fencing (reservations for the target + fail-closed reads) stays in force. A failed acknowledgement — audit or client — is NEVER a rollback.

### 4c. R1 — subscription removal: stage 1 and stage 2 (F2)

**R1-stage-1 (today + 8b-3a..3c).** DROP SUBSCRIPTION of a quarantined anchor is permitted only under **all** of (order fixed):

1. Authorize under the control lock; reservations for the **whole binding's** destination set are taken/published before leaving the lock (I8 covers every bound destination, not just one).
2. Resolve pending terminal work: no unresolved pending cause for any referencing generation (§3f); drain the generation lease write gate (**REQUIRES #188**).
3. Require **every** bound destination absent: not live (`DbInstance::get_repo` **OBSERVED** db_instance.rs:156-158), not catalogued, and `verify_absent(components)` true on the journal/absence evidence recorded by the completed operations. Incomplete operations covering any target → reject `destruction_in_progress`.
4. Delete the subscription row + durability barrier (flush system store).
5. Rebuild coverage (quarantine cache refresh — **OBSERVED** boot rebuild point core.rs:525-527; same refresh routine used at runtime).
6. Release the operation-derived reservations of the binding (only after the terminal ack per §3e if operations were involved).

Steps 1-3 and 5-6 make no durable change of their own; step 4 is the single durability barrier. Any failure before step 4's flush leaves the anchor intact and retry re-runs the protocol from step 1. Source availability is NOT required at any step (upstream may be gone — `validate_seed_resume`'s network probe, **OBSERVED** control.rs:218-300 with `probe_seed_upstream` at :283, is never invoked on this path).

**R1-stage-2 (8b-3d) — the ORDERED HANDOFF.** Each step with its crash behaviour:

- **(a) Write the incident record durably and VERIFY the write.** For every bound destination: upsert the `destination_incidents` row for `(cause, storage_object_id)` (state `Unresolved`, reusing the existing incident id for the same cause pair — idempotent) ALWAYS; write the repo-carried `MetaKey::ConsistencyIncidents` copy ONLY if the repo is openable (for an absent destination the catalogue row alone is authoritative and sufficient); then VERIFY what was written: read back both copies and pass a flush barrier on each store. Crash after (a) incomplete (row written, evidence write failed): retry is safe — the upsert is idempotent by cause pair and incident ids are **reused**; the row alone already quarantines (§3d boot derivation). Crash after (a) complete: protection has moved to the incarnation; the subscription row is now redundant.
- **(b) Invalidate + drain the subscription lease** (REQUIRES #188 lease write gate; generation-scoped to this binding's generation).
- **(c) Delete the subscription row + durability barrier** (flush system store). Crash after (c): the quarantine union rebuild **no longer derives protection from the deleted row**; it derives it from `destination_incidents` + repo-carried evidence instead (exact boot derivation in §3d: unresolved incident row over a live storage_object_id quarantines reads; repo-carried unresolved evidence quarantines even without a catalogue row — either copy blocks). This is precisely why (a) precedes (c).
- **(d) Release reservations for this binding** (in-memory; the incident-derived entries remain while incidents are unresolved).

**Overlapping / multi-destination independence.** One incident per `(cause, storage_object_id)`; clearing a subscription clears only the incident whose `cause` is THAT `(subscription, generation)`; validated recovery clears exactly one incident **by incident id** and rewrites only that incident's copies (row + repo-carried record). A destination with two causes keeps the second incident after the first clears (I9, I11).

### 4d. Boot-time replay ordering

Exact insertion in `ShamirDb::init` (**OBSERVED** core.rs: control lock field :55, built :221; repository-open loop :296-317 with `seed_boot_gate` :303 and publication via `add_repo_with_durability` :316):

1. Load (a) all non-`Destroyed`/non-`Rejected` rows AND (b) ANY row with NON-EMPTY `audit_pending` (ANY state, incl. `Destroyed`) — (b) is for outbox replay only; their reservations stay published until the pending set drains via the ack hook (§3e step 5) — from `destruction_operations` **before** the repository-open loop. Selection is NEVER by seq comparison.
2. Publish their targets (and the #188 anchor coverage — stage 2: plus incident rows and repo-carried evidence per §3d) into the reservation view (§3c). Boot publishes reservations BEFORE the server accepts requests, while the admission registry is empty (no contention); post-boot the executor acquires admission locks normally (§7a of the surface file).
3. Run the audit-outbox boot replay for every pending entry in `transition_ordinal` order (§3e step 7); the periodic `checkpoint_durable` ack hook subsequently removes pending entries whose `audit_seq ≤ ack_watermark` (§3e step 5).
4. Repository loop (:296-317): skip any target covered by an open operation **fail-closed** — log, do not open, do not recover, do not publish; the `seed_boot_gate` check (:303) stays as-is for non-reserved targets.
5. Spawn/continue the executor for `Blocked`/`Quiescing`/`Destroying`/`Cataloguing` — **and `Prepared`** (round-2 fix: the round-1 list omitted it) — resuming from the recorded state per 4b; run the outbox replay pass before retirement considerations.
6. Quarantine-cache rebuild stays at the END, :525-527.

**INFERRED justification:** today a partially destroyed target would be opened and recovered at :316 before any quarantine state exists; replay-before-open is what makes crash residue safe.

### 4e. Lock order and lock table (F5)

**DECIDED ORDER (round 3 — ONE merged schedule, single global order, never crossed in reverse):**

`operation token` (outermost; a single executor per operation) → `db-level admission gate` (per-database RW-style gate: repo-scope ops take it SHARED, database-scope ops EXCLUSIVE — §3c) → `repo-level admission mutexes` (per (db, repo), SORTED order — deadlock-freedom) → `per-target lifecycle exclusion` (**THE SAME admission-guard object — admission and lifecycle exclusion are ONE merged mechanism**, held from preflight start until `Destroyed`/`Rejected`/`Blocked`-release) → `subscription lease gates` (**REQUIRES #188**, sorted by subscription name) → `replication control lock` (innermost; short catalogue/validation sections only; NEVER across slow deletion or audit I/O).

Lock table (R2'; "target exclusions" = the merged repo-level admission mutex / per-target lifecycle exclusion, held in EVERY step 0→6):

| Step (4a) | op token | db gate | target exclusions | lease gate | control lock | Across awaits? |
|---|---|---|---|---|---|---|
| 0 Preflight | yes | shared | **yes (sorted; acquired here, held until 6)** | no | yes (validation only) | no (short section) |
| 1 Prepare | yes | shared | yes | no | yes for (i) and (iii); **NOT held for (ii)** — the barrier append is audit I/O | **YES — the Prepare barrier append** (awaited without the control lock) |
| 2 Quiesce | yes | shared | yes (sorted) | yes, inside target scope | **no** | **YES (drain joins)** |
| 3 Detach | yes | shared | yes | no | no | yes |
| 4 Destroy | yes | shared | yes | no | **no** (slow fs deletion, I7) | **YES (fs deletion)** |
| 5 Catalogue | yes | **exclusive** (destructive catalogue write) | yes | no | yes (re-acquired; write + revalidation) | **YES (async tx + flush)** |
| 6 Complete | yes | exclusive | yes (released only after verify_absent + the `Destroyed` transition) | no | yes (final transition + release bookkeeping) | **YES (revalidation probes)** |

The db gate is RELEASED before the step-5 catalogue write and RE-ACQUIRED EXCLUSIVE for steps 5-6 (the per-target admission guards are held independently throughout and are not affected by the db-gate release), then released after step 6; no other shared holder can exist at that point because destructive catalogue writes require the operation token, and the token is unique per operation.

**R1 stage-1 lock schedule** — drain-OUTSIDE / revalidate-INSIDE, mandatory because **`mark_subscription_terminal` takes the control lock** (**OBSERVED** admin_replication.rs:860-876, control lock at :866): any callback or marking that might run during drain must not be able to block on, or be blocked by, the deleter's control lock.

| R1-stage-1 step | db gate | target exclusions | lease gate | control lock | Across awaits? |
|---|---|---|---|---|---|
| 1 Authorize + reservations for the whole binding's destination set published | shared | yes (sorted) | no | yes (validation + publication), released before drain | no |
| 2 Pending-terminal resolution + lease drain (§4c steps 2) | shared | yes | yes | **no** — drain runs OUTSIDE control | **YES (drain joins)** |
| 3 Absence verification (§4c step 3: not live, not catalogued, `verify_absent`) | shared | yes | no | **no** (verification is I/O) | **YES** |
| 4 Row delete + durability barrier (§4c step 4) — RE-ACQUIRE control and REVALIDATE first: (i) row still exists, same name + state; (ii) binding generation unchanged (**REQUIRES #188**); (iii) every destination still absent (stage 1) / every incident still durably in place (stage 2); (iv) no new operation covers the targets; (v) reservations still held | shared | yes | no | yes (revalidate + delete + flush) | **YES (flush)** |
| 5 Coverage rebuild + operation-reservation release (§4c steps 5-6) | shared | yes (released last) | no | no | no |

**R1 stage-2 lock schedule** — same pattern, handoff steps of §4c:

| R1-stage-2 step | db gate | target exclusions | lease gate | control lock | Across awaits? |
|---|---|---|---|---|---|
| (a) Incident write + VERIFY both copies (§4c (a)) | shared | yes | no | no (incident I/O, not a control section) | **YES (durable writes + read-back + flush)** |
| (b) Lease invalidate + drain (generation-scoped) | shared | yes | yes | **no** — drain OUTSIDE control | **YES** |
| (c) Subscription-row delete + barrier — RE-ACQUIRE control, REVALIDATE (predicates (i)-(v) above, stage-2 form of (iii)) | shared | yes | no | yes | **YES (flush)** |
| (d) Reservation release for the binding (incident-derived entries remain) | shared | yes (released) | no | no | no |

**Acquisition-site inventory (must all match the order; merged mechanism):** (1) request handler: token → db gate → (sorted) target exclusions (acquired at preflight, held) → control lock for the short preflight/Prepare sections only (the barrier append happens with NO control lock); (2) executor quiesce: token + db gate + target exclusions held, lease gate acquired/released inside target scope; (3) executor catalogue/complete: same outer stack, control lock re-acquired; (4) `mark_subscription_terminal`/terminal marking: control lock only (LEAF — never called while a token holder waits for control *from inside* the control section; the drain-then-revalidate pattern above guarantees this); (5) R1 deleter: drain outside control lock, re-acquire + revalidate inside; (6) reservation mutation: takes the SAME db gate + repo-level admission mutexes it reserves (sorted) — **the round-2 generation-counter fallback is REMOVED** (I13); (7) wire CREATE REPO path: db gate (shared) → repo-level admission mutex → per-db `repo_create_locks` mutex → control lock (the create-path order, surface §7b row 6). No path acquires against the order; site (4) is a leaf and sites (1)-(3),(5)-(7) acquire strictly outermost→innermost.

### 4f. Cancellation, duplicates, bounds

- The request handler **spawns the executor and returns the operation id immediately after Prepare**; the HTTP/wire future can be cancelled freely at any point. The executor task owns the work and is detached: when the caller's future is dropped, the executor continues to the next durable boundary. External cancellation is implicit: when a caller's future is dropped the executor continues to the next durable boundary; there is no admin-cancel API in this series (`destruction_retry` refuses terminal rows (`Destroyed` → status; `Rejected` → a new operation is required) and concurrent re-execution, per surface-file Appendix B; it does not cancel).
- Ambiguous I/O per I14 (§4b). Audit failure per §3e (withholds retirement, never rolls back).
- Duplicate submit: same operation id + identical frozen parameters → return current status (idempotent); same id with different parameters → `destruction_operation_conflict`.
- Field bounds (normative, full table in surface-file Appendix A): reason 1..=512 chars, printable UTF-8, no NUL/control; names ≤ 255 bytes; paths ≤ 4096 bytes; operation_id ≤ 64 chars matching `[0-9A-Za-z._-]+`; storage_object_id 16 bytes; generation u64; timestamps u64 ns since UNIX_EPOCH. Validation rejects NUL and C0 control chars in `reason`/`operation_id`/`actor` (also enforced by the v2 confirmation encoding, surface §6).

## 5. Backend contract

**DECIDED.** New lifecycle contract in `shamir-storage`/engine, implemented by every backend factory (`BoxRepoFactory`, repo_types.rs:421-534 composites incl. MemBuffer wrappers :480-492; hybrid composite per repo_types.rs:518):

```rust
pub struct OwnedComponent { pub kind: ComponentKind, pub component_id: u32, pub path: Option<PathBuf> }
pub trait DestructibleBackend {
    fn describe_components(&self) -> Result<Vec<OwnedComponent>>;
    fn close_for_destruction(&self) -> Result<()>;               // stop workers, close handles; NO checkpoint required
    fn destroy(&self, components: &[OwnedComponent]) -> Result<Vec<OwnedComponent>>; // idempotent; returns destroyed
    fn verify_absent(&self, components: &[OwnedComponent]) -> Result<ComponentAbsence>; // Absent | Present(Vec) | Unknown
}
```

- **Raw fjall:** components are the repo directory contents plus the **sibling `<repo>.shamirwal/`** directory with its `NNNNNNNN.wal` segments (**OBSERVED** repo_instance.rs:963-985 — deleting only the repo dir leaves the WAL; I5).
- **In-memory:** revoke handles; no fs absence proof — no on-disk components exist; the descriptor records `durable:false` and completion relies on registry + catalogue absence.
- **Cache/buffer/hybrid composites:** destroy the durable tiers via the inner backend; hybrid's durable fjall metadata/interner tier is a real component (repo_types.rs:518); ephemeral tiers need only revocation.
- **Seed remnants are owned components** when the target is seeded (component kinds SeedStagingDir, SeedInstallMarker, SeedReceiptStore; ownership + deletion rules in surface-file §8) — **DECIDED** round 2.
- **Clean shutdown is not this API:** `shutdown_clean` requires admission already stopped and flushes/checkpoints — a preservation path (**OBSERVED** crates/shamir-engine/src/repo/clean_horizon/shutdown_impl.rs:14-63), not close-for-destruction (I12: never require a clean checkpoint of corrupt data to authorize destruction).

**Path-safety rules** enforced inside the filesystem destruction primitive (below the DDL): delete only the actual persisted location from the descriptor (never name-reconstructed paths — **OBSERVED** CREATE derives paths from names, admin_db_repo.rs:266/:291, while rename preserves the old location via registry re-key, db_instance.rs:176-184); no `..`, symlink/junction/reparse-point traversal; no overlap with the system-store directory (the Fjall tree at `shamir_db_meta.redb`) or another ownership record; handle-relative/no-follow deletion or platform-equivalent check/use protection; namespace durability barriers on affected parents; string-prefix matching and one-time `canonicalize` are explicitly insufficient. Arbitrary out-of-root backend locations fail preflight unless a separately reviewed backend capability owns them. The maintenance lock (`crate::maintenance_lock::acquire(&data_dir)`, **OBSERVED** installer.rs:113-115 — exclusive instance-level file lock) is reused for repo-scoped destructive maintenance at the instance level where the installer path is involved (§8 in the surface file); the per-operation executor does not depend on it for ordinary R2'.

Continue to the surface file, sections 6-8 + appendices: [`131b-s5cx-destination-lifecycle-surface.md`](131b-s5cx-destination-lifecycle-surface.md) — then the evidence file, sections 9-12: [`131a-s5cx-destination-lifecycle-evidence.md`](131a-s5cx-destination-lifecycle-evidence.md).
