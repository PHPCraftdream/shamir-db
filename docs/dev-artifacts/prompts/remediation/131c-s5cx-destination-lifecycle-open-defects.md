# 131c — PR8b-3 destination lifecycle: defect resolution log (rounds 4-5 — ALL DEFECTS CLOSED)

STATUS: all defects D1-D6 (rounds 3-4) and every round-4 review finding (131d:
P1-1..P1-8, P2-1..P2-9, P3) are resolved in
`131-s5cx-destination-lifecycle.md` (M), `131b-s5cx-destination-lifecycle-surface.md` (S)
and `131a-s5cx-destination-lifecycle-evidence.md` (E), kept mutually consistent,
against the MERGED #188/#189 code (master at 95c825f7). This file remains as the
resolution log only; it no longer lists open work.

Round-4 resolutions (D1-D6):

- D1 — resolved (round 4, re-proven round 5): db gate taken ONCE up front in its final
  mode (exclusive for kind-`Database` destructions; shared for repo-scope incl. CREATE
  REPO — mode decided by operation KIND), held to the end, never upgraded or
  released/reacquired; pair-by-pair deadlock-freedom proof (M §4e; S §7a).
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

Round-6 resolutions (adversarial review of round 5, K1-K3):

- K1(a) — resolved (round 6): seed-install boundary is option (i): a NEW 8b-3e wrapper
  acquires db gate SHARED + repo admission mutex + control BEFORE the installer body and
  passes them in; `install_seed_subscription` gains guard parameters instead of taking
  control itself (M §4e inventory item 8 + Class B proof; S §7b row 8; E case 14, §10).
- K1(b) — resolved (round 6): mechanical merged lock-site inventory (E Appendix 13, 29
  rows incl. test-only appendix) replaces the pair enumeration; the M §4e proof is redone
  over the complete table (Classes A-F); the only site needing a merged-code change is the
  seed install (= K1(a)).
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

Round-5 resolutions (131d P1/P2/P3) — see 131d's resolved log for the per-item map.

Round-6 pass-1 resolutions (adversarial review C1-C9) — **DECIDED (round 6)**:

- C1 (P1) — resolved (round 6): the R1 delete predicate is generation-fenced —
  `(name, binding_generation == record.binding_generation)` via an additional `Eq`
  filter (the `Filter::And` pattern of system_store.rs:330-352); a resumed removal
  whose live row's generation ≠ the record's fails closed → record `Completed`
  WITHOUT deleting (a recovery landed, causes cured), only the record generation's
  claims released (M §4c step 5 + crash table + merged-handler note; R1-stage-1 lock
  table row 4; S A.2d + Appendix B; E case 9).
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
- C6 (P2) — resolved (round 6): a challenge failure at Prepare (iii) leaves the row
  `Prepared` (reservations held, nothing deleted) and the remedy is to resubmit the
  SAME operation id; the challenge gates only the first submission by design (M §4a
  step 1 (iii); S §6a, §6c `DestructionTargetChanged` row).
- C7 (P2) — resolved (round 6): the minimal seed-install reservation check
  (`destination_name_reserved` lookups inside `install_seed_subscription`'s existing
  control section, catalogue.rs:161) lands in 8b-3c so I4 holds from the first
  install path that exists; the full preflight + boundary wrapper stay 8b-3e; the
  residual 8b-3a gap (no install path in staged code) is stated (M §1, I4; E §10
  8b-3a non-goals + 8b-3c scope).
- C8 (P2) — resolved (round 6): `destruction_archive` eligibility = `Destroyed` +
  fully-acked OR final-`Blocked` + fully-acked, with EXPLICIT release of the
  `Blocked` row's reservations (owner decision surface; reservations never lapse
  silently) (S Appendix B; M §3e item 7; E §11 risk 7).
- C9 (P2) — resolved (round 6): the 8b-3a boot skip-and-fence lands as a structural
  hook (empty reservation-view consult, inert until 8b-3b wires the view); acceptance
  = hook exists + a unit test drives it with a synthetic non-empty view (M §3a; E §10
  8b-3a).

Round-7 resolutions (verifiers A/B/C) — **DECIDED (round 7)**:

- C-1 (P2) — resolved (round 7): the minimal 8b-3c seed-install check is the §7c
  rebind exemption AND §7c item 4's reservation lookup — a destination reserved by a
  NON-TERMINAL destruction operation is refused with `destruction_in_progress` even
  for the rebind flow (one journal lookup; the destroyed-then-reinstall path is
  unaffected because its operation is `Destroyed`) (M §1/I4; E §9 case 15 + §10
  8b-3c; S §7b row 8).
- C-2 (P2) — resolved (round 7): the CREATE-path control section spans the
  reservation check, the mirror write AND the registry/catalogue publication (the
  insert runs inside the section — check-to-publish window closed by holding the
  section through publication); CREATE SUBSCRIPTION NEWLY acquires the design's
  admission gates (db gate + sorted repo mutexes for its bound destinations) BEFORE
  its merged control acquisition, outside control per the total order (M I13 + §4e
  item 12; S §7b rows 1-2/7/12).
- A-1 (SHOULD-FIX) — resolved (round 7): a `repo_create_locks` holder waits only
  DOWNWARD for control; no control holder ever takes a create mutex (cycle-free; the
  earlier "never waits for either" was mis-stated) (M §4e Class F; E §13 row 29).
- A-2/A-3 (NIT) — resolved (round 7): M §3e item 7 adds the F1b archival form;
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
