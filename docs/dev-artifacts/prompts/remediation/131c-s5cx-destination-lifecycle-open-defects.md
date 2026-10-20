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
- D5 — resolved (round 4, amended round 5): commit-before-flush deletion is UNKNOWN on
  flush failure; the re-read happens AFTER a forced successful flush (M §4c stage 1,
  §4a step 5).
- D6 — resolved (round 5): single-writer executor epoch model; claims via CAS
  `(record_version, executor_epoch)`; executor spawned before the handler returns;
  stranded `Prepared` claimed by same-id / retry / periodic pass, every claim re-running
  full preflight (M §3b, §3e item 3, §4b/§4d/§4f; S Appendix B; E cases 2/5).

Round-5 resolutions (131d P1/P2/P3) — see 131d's resolved log for the per-item map.
