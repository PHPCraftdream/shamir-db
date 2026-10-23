# 131i - round 5 fixes after the final gate review of the PR8b-3 design (DEC-3)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Situation

Round 4 (brief 131h) is the HEAD commit of this branch. The final gate review (verbatim below) answered NOT READY with five P1 protocol gaps and a P2 citation list. The orchestrator has DECIDED every point; write exactly these decisions into the documents (same five files under `docs/dev-artifacts/prompts/remediation/`, documents only, SURGICAL edits - nothing that exists may be compressed or dropped, at most 1000 lines per file, every citation re-read against the checkout, imports of terms consistent across M = 131, E = 131a, S = 131b).

## Decisions

1. **Audit bookkeeping owner (review P1-1).** Introduce ONE resident `AuditBookkeeper` task per database, owned by the lifecycle module, alive for the process lifetime. It receives every `checkpoint_durable` ack-watermark notification and is the only writer, besides executors, of an operation row, restricted to the AUDIT FIELDS (`audit_pending`, `audit_acked_ordinal`, `audit_next_ordinal`): for EVERY loaded row with pending audit - including `Destroyed`, `Blocked` (boot-loaded without an executor) and parked rows - it removes the entries with `audit_seq <= ack_watermark` and advances `audit_acked_ordinal` in ONE `commit_durable` CAS on `record_version` (executors still own all state/phase transitions; a CAS conflict between the two is retried; the operation token excludes executors from each other, not the bookkeeper). Amend the sentences that call the executor "the only row writer" and the ack-hook text (M section 3e step 5, M 3b schema comments, S A.7) accordingly. A late submission of an acknowledged event id returns `AlreadyAcknowledged` (a result variant, no `DurableAck` reconstruction from an ordinal). Extend E cases 28/32 (use the real numbers in the file) to terminal and Blocked rows and to a restart after pending-entry removal.
2. **Ordinals, payload identity, cap exhaustion (review P1-2).** Transition ordinals start at 1; the row carries a durable monotonic counter `audit_next_ordinal: u32` (initial 1) incremented inside the same CAS that creates the pending entry; repeated `Blocked` transitions each consume one; at `u32::MAX` new transitions are refused (`Blocked(audit_ordinals_exhausted)`, practically unreachable, state it). The canonical payload is FROZEN at event creation and EXCLUDES the annotation `outcome in {emitted, replayed, acked}` (a record-level field outside the digest), so a replay never changes the digest. Cap exhaustion: the operation parks as `Blocked` with `error_class = AuditUnavailable` (add this variant to the `ErrorClass` list and its wire mapping `destruction_audit_unavailable` in S) and `failed_phase` = the current phase; operator recovery is the audited `destruction_retry` carrying an explicit budget extension: each extension adds `MAX_APPEND_ATTEMPTS` to the row's cap (`append_budget_extensions: u8`, durable, recorded as an audited operator act), so the stated bound becomes `duplicates <= (1 + extensions) * MAX_APPEND_ATTEMPTS - 1`; before extending, the retry first runs the read-only (c)-proof (a covering ack watermark resolves the entry WITHOUT any append). Reconcile E case 18 ("at most once per boot") with this: at most one replay append per boot per pending entry AND always within the current cap.
3. **Stage-2 R1 integration (review P1-3).** Stage 2 REUSES the `SubscriptionRemoval` record, the `destruction_bindings` index and the step boundaries of stage 1; it only RELAXES the completion predicate. Write the exact boundaries: (a) `Pending` + the incident references become durable in ONE `commit_durable` (together with the `destruction_bindings` row); (b) the generation-fenced subscription delete + `phase = RowDeleted` in ONE `commit_durable`; (c) `RowDeleted -> Completed` after verifying DURABLE PROTECTION for every surviving destination (an `Unresolved` incident row or a covering destruction operation exists and is readable back) instead of universal absence. Make the retry API (M ~465, S ~340) and the crash table STAGE-AWARE (stage 1: absence re-verification; stage 2: protection verification) with the recovery action for every durable prefix.
4. **Unknown-provenance incident (review P1-4).** Add an explicit `UnresolvedProvenance` incident variant: key `(cause, destination_name, binding_generation)`, `storage_object_id: None`, no receipt/bookmark baseline (those fields become `Option` or live in a variant-specific part of the schema), reservation by NAME (not by id). Its resolution is only (i) validated recovery once ownership of whatever occupies the name can be read, or (ii) explicit destruction R2' of that object; it never mints an ownership identity. State deduplication, the reservation semantics and the recovery rule; update the incident schema in S and the absent-destination rule in M section 4c stage 2.
5. **Generation bump is not a cure (review P1-5).** Replace every "different live binding generation => recovery landed, causes cured" shortcut (M ~324/326/333, S ~340) with: the old generation's protection is retired ONLY by (i) validated recovery of the old cause (the section 3d predicate) or (ii) a durable identity-bound transfer of that cause's protection; the generation fence keeps the NEW generation from being deleted; if neither is proved, the removal stays protected and incomplete. Cite `replication_seed/catalogue.rs` (re-read the lines) for the rebind behavior that preserves quarantine until a post-reseed Resume.
6. **P2 and detach/catalogue ambiguity.** Correct the verified-source inventory rows named in the review (E 125-138 control-held-across-await entries, E 123/145 lease quiescence wording, E 23/65 `follower_loop.rs` Hello-fence branches, S 158 / E 101 staging rename `:127`); re-read each against the checkout. In R2' define "detach" unambiguously as registry/handle detachment only and assign the catalogue deletion to ONE durable phase (Cataloguing), fixing M ~253.

## Review findings (verbatim)

exit_reason= end_turn verdict= None dur_ms= 194895
--- FINAL
# NOT READY

The amendments close the original authorization and archival-release defects, but the normative set still permits materially different implementations of audit recovery and stage-2 R1. **Do not merge #196 until the P1 edits below land.**

References use:
- **M** = `docs/dev-artifacts/prompts/remediation/131-s5cx-destination-lifecycle.md`
- **E** = `docs/dev-artifacts/prompts/remediation/131a-s5cx-destination-lifecycle-evidence.md`
- **S** = `docs/dev-artifacts/prompts/remediation/131b-s5cx-destination-lifecycle-surface.md`

**OBSERVED** below denotes document or source text actually read; **INFERRED** denotes its implementation consequence—not an observed runtime failure.

## P1 — Must fix before merge

### 1. The audit acknowledgement lifecycle still has no defined owner after executor completion

**OBSERVED**
- The executor is the **only** operation-row writer; the checkpoint hook only posts notifications: **M:139, M:155, M:221**.
- `Destroyed` can retain pending audit entries, which must drain before retirement and reservation release: **M:229, M:270**.
- A boot-loaded `Blocked` operation has **no executor spawned**, while its outbox is nevertheless replayed: **M:350–352**.
- For an acknowledged event whose pending entry was removed, the only retained acknowledgement is `audit_acked_ordinal`: **M:107–110, M:225**. That ordinal does not retain the `seq` and `checkpoint_position` returned by `DurableAck`: **S:316**.

**INFERRED consequence:** two implementations are possible:
1. the executor exits on `Destroyed`/`Blocked`, leaving checkpoint notifications unable to durably drain the outbox; or
2. an undocumented resident executor continues handling audit bookkeeping.

Likewise, “return the recorded acknowledgement” after removal can mean either return an actual `DurableAck` or merely report already acknowledged.

**Minimal edit:** define one audit-bookkeeping owner that remains available for **every loaded row with pending audit**, including terminal and parked operations. Specify its lifetime and token ownership. Define whether late coalesced requests return an `AlreadyAcknowledged` result or require retained acknowledgement data; do not promise reconstruction of a full acknowledgement from an ordinal alone. Extend **E:28, E:32** to cover terminal/Blocked rows and a restart after pending-entry removal.

### 2. Audit ordinal allocation, replay payload identity, and attempt-cap recovery remain incomplete

**OBSERVED**
- `audit_acked_ordinal` starts at **0**, and any event with `k ≤ audit_acked_ordinal` is treated as acknowledged: **M:107–110, M:225**.
- The event ordinal is a `u32`, but no first ordinal or allocation/overflow rule is specified: **M:220; S:289, S:296**.
- Replay uses the same event id, and a different payload digest for that id is corruption: **S:326, S:328**. Yet the canonical payload contains `outcome ∈ { emitted, replayed, acked }`: **S:293, S:298**.
- The attempt cap parks the operation until operator action, while every rewrite remains forbidden at `append_attempts = 3`: **M:228; S:331**. No permitted operator transition changes that condition.
- **E:30** additionally requires at most one replay append per boot, whereas **M:228** permits further classified-ambiguous reconciliation attempts within the remaining budget.

**INFERRED consequence:** an implementer can start ordinals at zero and silently classify the first event as acknowledged; change `outcome` during replay and trigger corruption; or implement a retry API that cannot recover a capped operation without inventing a budget-reset policy.

**Minimal edit:**
- Start emitted ordinals at **1**; define durable monotonic allocation, including repeated Blocked transitions and overflow refusal.
- Freeze the canonical payload at event creation. State explicitly whether replay/ack status is outside that payload.
- Define the cap-exhaustion transition using existing `Phase`/`ErrorClass` values and a concrete operator recovery rule that preserves the claimed duplicate bound.
- Reconcile **E:30** with the chosen per-boot rewrite policy.

These are protocol decisions, not implementation-detail choices.

### 3. Stage-2 R1 is not integrated with the SubscriptionRemoval transaction/phase protocol

**OBSERVED**
- Stage-1 R1 explicitly creates `Pending` plus `destruction_bindings` atomically, then deletes the subscription plus writes `RowDeleted` atomically: **M:283–315; S:259**.
- Its crash table subsequently requires absence verification before completion: **M:324–328**.
- Stage-2 R1 separately specifies incident handoff, lease drain, subscription delete plus flush, and reservation release, but never states where the `SubscriptionRemoval` record is created, where incident references are persisted, or how its phases advance: **M:335–340**.
- Generic retry documentation still defines `RowDeleted` resume as **destination-absence** re-verification: **M:465; S:340**—which stage 2 intentionally no longer requires.

**INFERRED consequence:** an implementer can either retain the stage-1 absence requirement, defeating stage 2, or bypass the durable removal record/phase transaction when implementing stage 2.

**Minimal edit:** state that stage 2 reuses the removal record and binding index, then give its exact transaction boundaries and recovery predicates:
- when `Pending` and incident references become durable;
- subscription delete plus `RowDeleted` in one `commit_durable`;
- `RowDeleted → Completed` after verifying durable protection for surviving destinations, rather than universal absence;
- recovery after each incident-copy prefix and each removal-record phase.

Update the retry API and crash table to be explicitly stage-aware.

### 4. The “absent, never destroyed / provenance unknown” incident cannot be represented by the incident schema

**OBSERVED**
- A removal destination can have `storage_object_id: None`, expressly meaning never-destroyed/unresolved provenance: **M:295–306; S:257**.
- Stage-2 R1 must nevertheless create an unresolved incident for an absent destination without a covering destruction operation: **M:337**.
- An incident requires a concrete `[u8;16]` storage-object id, a complete receipt/source/bookmark baseline, and uniqueness by `(cause, storage_object_id)`: **M:169–186; S:273–275**.
- Neither the removal record nor the absent-destination rule defines where those mandatory values come from when provenance is unresolved.

**INFERRED consequence:** implementations must invent an identity/baseline, reject a required handoff, or weaken the incident schema independently.

**Minimal edit:** define a concrete identity/evidence representation for unknown-provenance protection. Either require recoverable historical identity and baseline before this handoff, or introduce an explicit unresolved-provenance variant with defined reservation, deduplication, recovery, and destruction semantics. Do **not** silently mint a replacement ownership identity to fill the gap.

### 5. A generation bump is still treated as proof of cure, contrary to the documented rebind mechanism

**OBSERVED**
- Resumed R1 declares a different live binding generation to mean “recovery landed, causes cured,” and completes while releasing the old generation’s claims: **M:324, M:326, M:333; S:340**.
- The cited rebind implementation bumps the generation while preserving the quarantined row; its comment explicitly says **only a post-reseed Resume may lift quarantine**:
  `crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:213–225, :227–251`.
- I11 and the exact recovery predicate require validated recovery or destruction—not a generation change—to clear protection: **M:57, M:188; S:275**.

**INFERRED consequence:** the new generation is correctly protected from accidental deletion, but the design incorrectly authorizes retiring old-generation protection based on generation inequality alone.

**Minimal edit:** retain the generation-fenced non-delete behavior, but replace “generation changed ⇒ cured” with verification of either:
- validated recovery for the old cause; or
- a durable, identity-bound transfer of that cause’s protection.

If neither is proved, keep the removal protected and incomplete.

## P2 — Correct the verified-source inventory

I re-read **more than 20 distinct cited sites**, including the four `db_management.rs` control acquisitions, all seven replication-admin acquisitions, both DROP acquisitions, supervisor reconcile/start/lease/teardown, follower identity/apply sites, and installer/staging boundaries.

### Incorrect or materially misleading citations found

| Document reference | OBSERVED correction |
|---|---|
| **E:125–131**, inventory rows 5–11: each says control is not held across await | These handlers acquire control and subsequently await catalogue/validation work. Examples: `crates/shamir-db/src/shamir_db/execute/admin_db/admin_replication.rs:65–80`, `:132–150`, `:170–179`, `:245–253`, `:295–303`, `:400–429`, `:454–460`. All seven “no” entries need correction. |
| **E:132–133**, rows 12–13: DROP control sections say no await | Binding checks await while control remains held: `crates/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs:118–135`, `:433–450`. |
| **E:134, E:136–138**, rows 14/16–18: removal/rename control sections say no await | The acquired guard remains live over awaited calls: `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:87–88`, `:165–170`, `:553–554`, `:622–626`. All four entries need correction. |
| **E:123, E:145** describe lease quiescence as a “drain join” | It waits for the apply-gate write lock and drops it; it does not join a task: `crates/shamir-db/src/repl_binding/lease.rs:127–140`. Preserve the distinction from supervisor task joins, which DEC-3 explicitly requires outside control. |
| **E:23, E:65** cite `follower_loop.rs:407` as the production Hello-fence site | `:407` is the override-info branch; production initializes that override to `None` and follows `fence_hello_recorded` at `:416–423`: `crates/shamir-server/src/sync/replication/follower_loop.rs:399–423`. Cite both branches, with the production branch identified. |
| **S:158, E:101** locate the published rename at staging `:121` | `:121` is the function declaration; the rename syscall is `crates/shamir-server/src/sync/replication_seed/install/staging.rs:127`. This is the actual publication boundary. |

**Confirmed citation locations:** the four db-management control acquisitions; supervisor `reconcile:311`, control `:314`, `stop_all:389`, start `:428`, control `:434`, lease take `:459`, teardown control `:965`; follower Pull fence `:641`, apply-read acquisition `:832`, guard drop `:940`; installer Unix refusal `:92–94`, maintenance acquisition `:115`, candidate search `:117`, init `:118–121`, target check `:122`, import `:142`, publish call `:170`, registration `:176`, subscription persistence `:179`; staging candidate factory open `:60`.

## Cross-document gate assessment

- **Authorization:** the atomic challenge consumption plus Prepared insertion now matches DEC-3 A1: **M:219, M:251; S:27**.
- **Archival:** no-release eligibility, permanent tombstone, and permanent binding lookup are now aligned: **M:229, M:314; S:259, S:344**.
- **Staging/A14:** the private 8b-3b executor and delayed 8b-3c dispatch are consistently stated: **M:50, M:246; S:100; E:36–42**.
- **Error-code count:** **S:76–86** contains eleven lifecycle codes. The format and authority-unavailable diagnostics are explicitly separate: **S:90, S:184**. However, `Blocked(audit_unavailable)` still needs the concrete schema mapping required in P1-2.
- **CI numbering:** cases **16–20** match the audit topics referenced by **S:331**. Their oracles need the corrections above; numbering itself is not a blocker.
- **R2′ catalogue timing:** **M:253** describes destructive detach as removing the catalogue row, then says the operation’s catalogue rows go in step 5. Resolve this in the transaction-boundary edit: detach should unambiguously mean registry/handle detachment, with catalogue deletion assigned to one durable phase.

## Minimal path to MERGE-READY

1. Close audit bookkeeping ownership, ordinal/payload identity, and capped-retry semantics.
2. Unify stage-2 R1 with the removal-record phases and atomic transactions.
3. Make unknown-provenance incidents representable.
4. Remove generation-change-as-cure shortcuts.
5. Correct the verified inventory and the detach/catalogue phase ambiguity; adjust the corresponding existing CI cases.

**Files changed:** none. **Builds/tests run:** none. Read-only review only; the final observed Git status remained clean and detached.
--- REVIEW

--- WARN None

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

Final message (mandatory): for each decision 1-6 the file and section where it is written; the list (file:line you re-read) of at least 25 citations; anything you could not do exactly, with the reason. Never end a turn while a child agent is outstanding. Stop editing at 130 minutes and report exactly what is done and what is not.
