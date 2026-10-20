# 131d — PR8b-3 destination lifecycle: review round-4 findings — RESOLVED LOG (round 5)

STATUS: ALL items below are closed in `131-s5cx-destination-lifecycle.md` (M),
`131b-s5cx-destination-lifecycle-surface.md` (S) and
`131a-s5cx-destination-lifecycle-evidence.md` (E), resolved against the MERGED
#188/#189 code (master at 95c825f7). This file is retained as the per-item map only.

## D1-D6

- D1 — resolved (round 5): ONE total order derived from merged code; every #188/#189
  control site inventoried; dispatch split point before admin_db_repo.rs:118/:433
  (M §4e order + tables + inventory; S §7a/§7b; E §10 R5-1..R5-3).
- D2 — resolved (round 5): `AuditSink` seam + contiguous-prefix durability watermark +
  poison + fresh-write reconciliation (M §3e items 0/0b/6; S A.7b; E case 3, risk 8).
- D3 — resolved (rounds 4-5): never-destroyed absent destinations keep their Unresolved
  incident + reservation; S degraded-copy rule aligned with M §4c (a) (M §4c (a);
  S §7c items 2/4, A.4; E case 15).
- D4 — resolved (round 5): tag 0x0A dropped (digest 0x0B commits); `destruction_challenges`
  delete-if-matches single use; actor server-recomputed; preview authorized + session-keyed;
  v2 scope = drop confirmations only (S §6a/§6b, A.2a/A.6; M §4a step 0).
- D5 — resolved (rounds 5-6): post-flush-error re-read happens after a forced successful
  flush; **round 6 (K3): resume keyed by the `SubscriptionRemoval` OPERATION RECORD —
  lookup key `(subscription name, binding generation)`, NEVER the row; merged
  `handle_drop_subscription`'s `existed:false` silent no-op is fixed by routing through
  the record** (M §4c stage 1 + crash table, §4f; S A.2d, Appendix B; E crash points).
- D6 — resolved (round 5): epoch CAS claims, single writer, ack posted to executor;
  claims enumerated (M §3b/§3e item 3/§4f; S Appendix B; E cases 2/5).

## P1

- P1-1 — resolved (round 5): new total order with every merged control-first site
  (db_management.rs:87/:165/:553/:622; admin_db_repo.rs:118/:433;
  admin_replication.rs:65/:132/:170/:245/:295/:400/:454; replication_seed/catalogue.rs:161;
  quarantine_helpers/admin_replication.rs:121); dispatch split point BEFORE the DROP
  handlers' control acquisition (M §4e inventory, §4a preamble; S §7b rows 6/12/13/16).
- P1-2 — resolved (round 5): order control → `apply_gate.write()` adopted everywhere
  (merged lease.rs:15-16/:127-141); drain runs in short control sections; the
  drain-OUTSIDE-control rationale DELETED; followers never take control under the read
  guard (M §4e, §4a step 2, R1 tables; S §7a).
- P1-3 — resolved (round 5): destruction-authority token
  (`DestructionAuthority` + `remove_repo_locked_destructive`) — the destroy path is the
  only caller exempt from `ensure_destinations_unbound`, for exactly the frozen targets;
  executor takes control for the detach section (M §4a step 3; S Appendix B; E §10 8b-3b).
- P1-4 — resolved (round 5): `AuditSink` trait in shamir-db; real durability watermark
  (contiguous-prefix advance), chain poison, sync-error-never-proof, fresh
  write_all+sync_all reconciliation; sink injected by the launcher before replay
  (M §3e items 0/0b/1/6; S A.7a/A.7b; E case 3, risk 8).
- P1-5 — resolved (round 5): operation_id lookup BEFORE challenge; identical-params
  resubmission returns status/claims; `Rejected` insert-if-absent only; challenge/digest
  + 60 s expiry apply to first submission only (M §4a step 0, §4f; S §6a; E case 2/5).
- P1-6 — resolved (round 5): reservations published at Prepare (i) with the row write —
  every non-terminal row incl. a barrier-failed `Prepared` reserves; boot loads and
  publishes them; claims re-run full preflight against `target_digest`; expiry is
  new-submission-only (M §3c, §3e item 3, §4a step 1, §4b, §4d; S Appendix B; E case 1/5).
- P1-7 — resolved (round 5): retire fence designed — `RepoWriteDomain::retire`/
  `is_retired`, bounded fail-closed drain, lazy-WAL refusal on retired instances, order
  retire → drain → unlink; fjall stale-handle + Windows delete-pending recorded as
  explicit unknowns with CI probes in 8b-3a (M §5; E case 6, §10).
- P1-8 — resolved (round 5): catalogue mirror carries the FULL descriptor;
  error-propagating mirror writes; unreadable-ownership preflight (mirror-authoritative
  or fail-closed); boot skip-and-fences an anchor-covered unopenable repo instead of
  aborting init (M §3a; E cases 7, §10 8b-3a).

## P2

- P2-1 — resolved (round 5): AuditSink injection + init signature + scheduler
  `checkpoint_durable`-to-executor hook specified (M §3e items 0/5; S A.7b; E §10 8b-3b).
- P2-2 — resolved (round 5): single executor CAS writer (`executor_epoch`); the ack hook
  posts to the executor; claims enumerated; losers get status (M §3b, §4f; S Appendix B).
- P2-3 — resolved (round 5): re-read only after a forced successful flush; R1 resume is
  the binding-keyed revalidate-then-delete path (M §4a step 5, §4c stage 1).
- P2-4 — resolved (round 5): 0x0A dropped; `destruction_challenges` table + durable
  delete-if-matches; actor in canonical bytes, server-recomputed; preview authorization;
  v2 scope limited to drop confirmations (S §6a/§6b, A.2a/A.6; M §4a step 0).
- P2-5 — resolved (round 5): `Rejected` rows never reserve (excluded from §3c (ii) and
  boot loading), insert-if-absent only (M §3b/§3c/§4f).
- P2-6 — resolved (round 5): I13 restated — the reservation set is serialized by the
  control lock (the merged control-first sites), closing check-vs-publish without a
  generation counter (M I13, §3c; S §7a).
- P2-7 — resolved (round 5): staging re-done (R5-1..R5-3): audit barrier lands in
  8b-3b; R1-stage-1 + §3f together in 8b-3c; mirror fail-closed + propagating writes
  together in 8b-3a; every case's end-to-end form gated with its dispatch; per-PR
  acceptance criteria + non-goals in §10 (E §9/§10).
- P2-8 — resolved (round 5): step-0 predicate requires ≥1 referencing row + explicit
  unbound-repo destruction path; stage-2 absent-destination rule covers never-destroyed
  targets; §7c item 2 scoped to live/catalogued (item 4 live); S:228-vs-M:237 contradiction
  resolved (M §4a step 0, §4c (a); S §7c, A.4).
- P2-9 — resolved (round 5): terminal event kind unified as `destroyed`; CREATE REPO is
  repo-scope SHARED (mode by KIND); db-gate fairness caveat + multi-db sorted order added
  (M §3e item 2, §4e; S A.5).

## P3

- P3 torn audit line — resolved (round 5): `read_log_for_verify` skips a torn FINAL line
  only; a middle gap fails closed (M §3e item 0b).
- P3 retention sweep — resolved (round 5): sweeps exclude the active log while
  `audit_pending` is non-empty; a re-append after a bad sweep is benign (M §3e item 7).
- P3 admissions DashMap — resolved (round 5): entry removed on uncontended release /
  bounded-size policy (S §7a).
- P3 recovery-clear reachability — resolved (round 5): predicate is staged, unreachable
  before 8b-3d, asserted only negatively in stage 1 (M §3d; E §10).
- P3 Blocked(Prepare) — resolved (round 5): unsupported backends refused at preflight;
  `Blocked(Phase::Prepare)` unreachable; `UnsupportedBackend` kept for schema stability
  (M §3b; E §10 8b-3a).

## Round 6 (adversarial review of round 5, K1-K3)

- K1 — resolved (round 6): seed-install boundary (i) — 8b-3e wrapper takes gates+control
  before the installer body, `install_seed_subscription` gains guard parameters (M §4e
  item 8/Class B; S §7b row 8; E case 14, §10); mechanical merged lock-site inventory
  added as E Appendix 13 (29 rows) and the M §4e proof redone over the complete table.
- K2 — resolved (round 6): closed durability-proof rule replaces the adopt-after-fsync
  branch everywhere (M §3e item 6; S A.7a/A.7b; E risk 8, case 3).
- K3 — resolved (round 6): D5's binding-keyed resume specified — `SubscriptionRemoval`
  record (S A.2d), `(name, generation)` lookup-first retry endpoint, R1-stage-1 crash
  table, merged-handler fix (M §4c/§4f; S Appendix B; C D5).

## Round 6 pass 1 (adversarial review C1-C9) — resolved (round 6)

- C1 — resolved (round 6): generation-fenced R1 delete predicate
  `(name, binding_generation == record.binding_generation)`; resumed generation
  mismatch fails closed (record `Completed` WITHOUT deleting) (M §4c/§4e row 4;
  S A.2d/Appendix B; E case 9).
- C2 — resolved (round 6): `Rejected` rows never enter `audit_pending`; boot
  loading/publication excludes them (M §4d/§3b/§3e item 2; S A.2/A.5; E case 3).
- C3 — resolved (round 6): single-app-class unpoison path (reconciliation RE-WRITE
  of the failed event id only; unpoison on its own successful write+sync) (M §3e
  item 0b; S A.7a/A.7b; E risk 8/case 3).
- C4 — resolved (round 6): Prepare (i) put-if-absent; initial spawn through the
  epoch CAS (M §4a/§4f/§3b; S Appendix B).
- C5 — resolved (round 6): Appendix 13 row 30 admits inventory + honest scope
  statement (E §13; M §4e Class F).
- C6 — resolved (round 6): challenge failure at (iii) keeps `Prepared`; same-id
  resubmission remedy; challenge gates first submission only (M §4a step 1;
  S §6a/§6c).
- C7 — resolved (round 6): minimal seed-install reservation check in 8b-3c's
  existing control section; 8b-3a residual gap stated (M §1/I4; E §10).
- C8 — resolved (round 6): archival of final-`Blocked` + fully-acked with explicit
  reservation release (S Appendix B; M §3e item 7; E risk 7).
- C9 — resolved (round 6): 8b-3a structural boot hook + synthetic-view unit-test
  acceptance criterion (M §3a; E §10 8b-3a).
