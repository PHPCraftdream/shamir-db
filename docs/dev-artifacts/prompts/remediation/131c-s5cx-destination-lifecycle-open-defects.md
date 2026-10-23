# 131c — PR8b-3 destination lifecycle: defect resolution log

**STATUS: HISTORICAL, NON-NORMATIVE.** This file records past review findings and their resolutions. The current normative contract is 131/131a/131b as amended by [DEC-3 Q3](../../design/dec3-owner-rulings-2026-10-21.md#q3---pr8b-3-design-and-draft-196). Where a historical resolution below conflicts with the DEC-3 amendments (notably A1 authorization atomicity, A2 archival no-release), the DEC-3 amendments win.

## Superseded by DEC-3

- **C6 — SUPERSEDED by DEC-3 A1.** The round-6 remedy "resubmit the SAME operation id to claim the stranded `Prepared`" is DELETED: a challenge failure at Prepare writes NO row (the challenge DELETE-IF-MATCHES and the `Prepared` PUT-IF-ABSENT are ONE system-store transaction); the remedy is re-preview + a fresh operation id (or `destruction_retry` only if a prior authorized row genuinely exists).
- **C8 — SUPERSEDED by DEC-3 A2.** The round-6 archival form "FINAL-`Blocked` + fully-acked with EXPLICIT release of the `Blocked` row's reservations" is DELETED: `destruction_archive` NEVER releases protection (eligible only Repo/Database `Destroyed` + fully acked, and SubscriptionRemoval `Completed`; tombstone replaces the row; operation ids never reusable; final Blocked exits only via `destruction_retry` → `Destroyed`; quota on concurrent non-terminal operations with backpressure refusal).
- **F1b — SUPERSEDED by DEC-3 A2.** The round-6 "RowDeleted + Blocked + acked-drained archival-eligible WITH explicit reservation release" form is DELETED: a Blocked SubscriptionRemoval at `RowDeleted` whose re-verification found data converts that data into an R2' target or an incident, NEVER a release.

## Round 9 corrections (historical log; current normative contract in 131/131a/131b)

- OPEN1 — selected a new authority-only `SeedAuthorityReader::open_existing` + snapshot check for 8b-3c, not reordered `init_seed`/`ShamirDb::init`. Read only the existing system store through a required read-only adapter; reconstruct the validated §3c/§4d union, ready only after full load, fail closed on unavailable/corrupt/dirty/unsupported authority, and refuse before candidate access or logical publication. See 131b §8b and 131 I4.
- OPEN2 — 8b-3b is primitive-only; real publisher/control serialization wiring and race tests are 8b-3c. See 131a case 5 and §10, 131b §7b, 131 I13.


Round 8 R1: the merged installer exists; 8b-3c's read-only reservation check runs after maintenance-lock acquisition and before `find_complete`, without control lock, relying on the lock through install. The authoritative catalogue-tail recheck keeps the same predicates/refusal typing. R2 replaces unsafe try_lock cleanup with strong_count==1 under the synchronized registry lock and no bypass handles. R3 separates merged Appendix 13 inventory from proposed composition and proof; CREATE DATABASE is exclusive, CREATE REPO shared.



Round-4 resolutions (D1-D6):

- D1 — corrected round 8: the former proof described proposed paths as if merged leaf status were sufficient. M §4e now separates lock facts and proposed compositions; see R3 correction above.
- D2 — resolved (rounds 4-5): ambiguous-append reconciliation via the `AuditSink`
  seam (`contains`/`durable_watermark`) over the DURABLE log tail or
  `checkpoint_durable`'s ack record — never `AuditChain::snapshot`; contiguous-prefix
  watermark, chain poison, sync-error-never-proof (M §3e items 0/0b/6; S A.7a/A.7b; E risk 8).
- D3 — resolved (rounds 4-5): destruction completion writes `Resolved(by destruction)`
  in the SAME durable write as `Destroyed`; R1 records NOTHING for an absent destination
  with a completed destruction; an absent NEVER-destroyed destination still gets an
  Unresolved incident + reservation (M §4a step 6, §4c (a); S §7c, A.3/A.4; E case 15).
- D4 — resolved (rounds 4-5): `destruction_preview` + digest/challenge binding; tag
  `0x0A` DROPPED (the digest 0x0B commits to the list); challenge single-use via the
  `destruction_challenges` delete-if-matches table; actor server-recomputed from the
  session (M §4a step 0; S §6a/§6b, A.2a, A.6; E case 12).
- D5 — resolved (round 4, amended rounds 5-6): commit-before-flush deletion is UNKNOWN on
  flush failure; the re-read happens AFTER a forced successful flush; **round 6 (K3): the
  resume is keyed by the `SubscriptionRemoval` OPERATION RECORD — lookup key
  `(subscription name, binding generation)`, NEVER the row** (M §4c stage 1 + crash table;
  S A.2d/Appendix B; E case 9/crash points).

Round-6 resolutions below are historical; corrected R1-R3 decisions above take precedence.


- K1(a) — corrected round 8: the wrapper remains proposed for 8b-3e, but it is not the only required lock-path change. Proposed CREATE and lifecycle compositions are detailed in M §4e/S §7b; the old seed-only change claim is superseded.
- K1(b) — corrected in round 8: Appendix 13 is explicitly merged-code inventory only. The prior claim that only seed install needed a merged-code change was too narrow: proposed composed paths also add gates to CREATE DATABASE, CREATE REPO, generic add-repo, CREATE SUBSCRIPTION/RESUME, remove/rename/destroy/R1. M §4e and S §7b now separate observed leaves from proposed composition and prove order for those paths.
- K2 — resolved (round 6): the "retry fsync as proof" branch is removed; the closed
  durability-proof rule ((a) own successful sync / (b) fresh same-event_id append /
  (c) checkpoint_durable ack record; found-but-unconfirmed tail NEVER adopted) is stated
  byte-consistently in M §3e item 6, S A.7a/A.7b and E risk 8.
- K3 — resolved (round 6): D5's binding-keyed resume actually specified — the
  `SubscriptionRemoval` operation record (S A.2d), the retry endpoint lookup by
  `(name, generation)`, the R1-stage-1 crash/ambiguity table and the merged
  `handle_drop_subscription` `existed:false` silent-no-op fix (M §4c/§4f; S Appendix B).
- D6 — resolved (round 5): single-writer executor epoch model; claims via CAS
  `(record_version, executor_epoch)`; executor spawned before the handler returns;
  stranded `Prepared` claimed by same-id / retry / periodic pass, every claim re-running
  full preflight (M §3b, §3e item 3, §4b/§4d/§4f; S Appendix B; E cases 2/5).

- R2 (P2) — corrected round 8: the unsafe try_lock-probe reclamation claim is withdrawn. S §7a gives the strong_count == 1 invariant under the same registry lock used by every get/clone, forbids weak/external bypass clones, and specifies safe capacity backpressure. Historical resolution wording is not evidence that the old policy was sufficient.

## Round-6 pass-1 resolutions (historical; C7 superseded by round 8)

- C1 (P1) — resolved (round 6): the R1 delete predicate is generation-fenced —
  `(name, binding_generation == record.binding_generation)` via an additional `Eq`
  filter (the `Filter::And` pattern of system_store.rs:330-352); a resumed removal
  whose live row's generation ≠ the record's fails closed → record `Completed`
  WITHOUT deleting (a recovery landed, causes cured), only the record generation's
  claims released (M §4c step 5 + crash table + merged-handler note; R1-stage-1 lock
  table row 4; S A.2d + Appendix B; E case 9). **SUPERSEDED (round 9, brief 131i,
  P1-5): the "generation changed ⇒ recovery landed, causes cured ⇒ record
  `Completed` WITHOUT deleting" shortcut is RETRACTED — a generation bump is not a
  cure; the record stays protected and INCOMPLETE until validated recovery of the
  old cause (§3d predicate) or a durable identity-bound protection transfer is
  proved (see the current M §4c crash table).**
- C2 (P2→P1) — resolved (round 6): `Rejected` rows are excluded from boot's
  non-empty-`audit_pending` load, and the `destruction_rejected` event is emitted
  best-effort DIRECTLY, never enqueued (M §4d steps 1-2, §3b, §3e item 2; S A.2/A.5;
  E case 3).
- C3 (P1) — resolved (round 6): poison admits exactly one append class — the
  reconciliation RE-WRITE of the failed event id — and unpoisons on that entry's own
  successful write+sync; other appends get `Poisoned`; poison never silently cleared
  (M §3e item 0b; S A.7a/A.7b; E risk 8, case 3).
- C4 (P1) — resolved (round 6): Prepare (i) is a durable PUT-IF-ABSENT keyed by
  `operation_id` (loser short-circuits to the step-0 id-lookup path); the INITIAL
  executor spawn passes through the same `(record_version, executor_epoch)` CAS (M
  §4a step 1, §4f, §3b; S Appendix B).
- C5 (P2) — resolved (round 6): Appendix 13 row 30 added — every remaining
  production admit() site re-grepped, grouped into verified site families (incl. the
  migration `admit_pair` fixed ascending-token order); the inventory scope is stated
  honestly (control/lease sites exhaustive; admit() classes with representatives)
  (E §13 preamble + row 30; M §4e Class F + inventory item note).
- C6 (P2) — resolved (round 6) — **SUPERSEDED by DEC-3 A1**: a challenge failure at Prepare (iii) leaves the row
  `Prepared` (reservations held, nothing deleted) and the remedy is to resubmit the
  SAME operation id; the challenge gates only the first submission by design (M §4a
  step 1 (iii); S §6a, §6c `DestructionTargetChanged` row). The resubmission-as-remedy
  is deleted; see the Superseded by DEC-3 section above.
- C7 (P2) — corrected round 8: the merged installer exists. 8b-3c adds a read-only precheck after maintenance-lock acquisition and before find_complete, with consistent predicates and typed refusal at catalogue.rs:161. No 8b-3e wrapper required; applies to rebind and subscription-free install; nonterminal destruction refused before open/mutation. Maintenance-lock exclusion is required. CI asserts refusal before open/import/publish/register.
- C8 (P2) — resolved (round 6) — **SUPERSEDED by DEC-3 A2**: `destruction_archive` eligibility = `Destroyed` +
  fully-acked OR final-`Blocked` + fully-acked, with EXPLICIT release of the
  `Blocked` row's reservations (owner decision surface; reservations never lapse
  silently) (S Appendix B; M §3e item 7; E §11 risk 7). The Blocked-row archival
  release is deleted; see the Superseded by DEC-3 section above.
- C9 (P2) — resolved (round 6): the 8b-3a boot skip-and-fence lands as a structural
  hook (empty reservation-view consult, inert until 8b-3b wires the view); acceptance
  = hook exists + a unit test drives it with a synthetic non-empty view (M §3a; E §10
  8b-3a).

Round-7 resolutions (historical; amended by round 8 corrections above):

- C-1 (P2) — historical, superseded round 8: old C7 called for a minimal catalogue-tail check. The chosen early check is instead in installer.rs before find_complete; it refuses nonterminal destruction despite rebind and uses maintenance-lock exclusion. C-1's no-delete-generation-fence behavior in the catalogue path remains separate.
- C-2 (P2) — corrected round 8: the prior CREATE gate note did not state database-scope mode. Proposed path uses EXCLUSIVE `(db, *)` for CREATE DATABASE and SHARED db + repo mutex for CREATE REPO; both hold control through publication. M §4e/S §7b distinguish these paths from merged facts.
- A-1 (SHOULD-FIX) — resolved (round 7): a `repo_create_locks` holder waits only
  DOWNWARD for control; no control holder ever takes a create mutex (cycle-free; the
  earlier "never waits for either" was mis-stated) (M §4e Class F; E §13 row 29).
- A-2/A-3 (NIT) — resolved (round 7): M §3e item 7 adds the F1b archival form — **SUPERSEDED by DEC-3 A2** (see above);
  M I4's operation clause is kind-scoped to `kind: Repo | Database`.
- B-1..B-11 — resolved (round 7): no occurrences of the stale claims exist in M/E/S/
  C/R for B-1 (uncoded Validation), B-2 (public `_locked`), B-5 (deleted tx files),
  B-6 (monolith admin_replication cites), B-7 (flush_seed_catalogue), B-9
  (supervisor :519-538), B-10 (confirmed.rs/may_start_follower), B-11 (old paths) —
  those occur only in the `.scratch/panel/` docs, which are outside this file set.
  Fixed where present: B-3 (#1370 already fixed at admin_db_repo.rs:174/:482 on this
  tree — M §4a step 4 + E fact sheet), B-4 (quarantine_cache publish :71-96 is
  owner-scoped claims — M §3f + E fact sheet), B-8 (follower_loop local-lookup cite
  E §12 → :501/:362/:593).
