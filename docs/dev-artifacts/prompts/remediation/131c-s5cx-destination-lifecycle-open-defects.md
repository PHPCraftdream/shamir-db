# 131c — PR8b-3 destination lifecycle: defect resolution log (round 4 — ALL DEFECTS CLOSED)

STATUS: all six defects D1-D6 found in review round 3 are resolved (round 4) in
`131-s5cx-destination-lifecycle.md` (M), `131b-s5cx-destination-lifecycle-surface.md` (S)
and `131a-s5cx-destination-lifecycle-evidence.md` (E), kept mutually consistent.
This file remains as the resolution log only; it no longer lists open work.

- D1 (P1) — resolved (round 4): db gate taken ONCE up front in its final mode (exclusive for
  kind-`Database` destructions / create-or-replace-under-db; shared for repo-scope, incl. a
  single-repo destruction, for the operation's whole lifetime), held to the end, never upgraded
  or released/reacquired; the round-3 release-before-step-5/reacquire-exclusive language removed;
  pair-by-pair deadlock-freedom proof added (M §4e order + both lock tables + acquisition-site
  inventory; S §7a; M I13/§3c).
- D2 (P1) — resolved (round 4): ambiguous-append reconciliation reads the DURABLE log tail
  (forced-fsync `flush_buffer`/`sync_all`/`read_durable_tail` read, or `checkpoint_durable`'s
  durable ack record) — never `AuditChain::snapshot`; a found entry counts only after a
  confirmed durability operation (fsync confirmed or seq ≤ durable ack watermark); restart case
  (empty in-memory vector) defined (M §3e item 6; S A.7a; E risk 8).
- D3 (P1) — resolved (round 4): destruction completion writes `Resolved(by destruction)` —
  incidents flip to `Cleared` with `cleared_by = ByDestruction { operation_id }` in the SAME
  durable write as `Destroyed`; stage-2 R1 upserts incidents ONLY for destinations still live
  or catalogued; an absent destination with a completed destruction operation records NOTHING
  and its reservation is released (M §4a step 6, §4c (a)/(d), §3d schema + predicate; S §7c,
  §8c flow 1, A.3; E case 15).
- D4 (P1) — resolved (round 4): two-step confirmation exchange — new admin wire op
  `destruction_preview(targets, cascade)` returning the sorted frozen target list, its SHA-256
  digest, a single-use ≤60 s challenge and `expires_at_ns`; the DDL modifier and the v2 canonical
  encoding bind digest (tag 0x0B) + challenge (tag 0x0C), HMAC over the digest, verified at the
  admin.rs:636-659 seam without a Prepared operation; executor re-resolves the live/catalogued
  set at Prepare and fails closed with the NEW eleventh typed wire code
  `destruction_target_changed` (all "ten" counts updated to eleven; TS `.destructionPreview` +
  builder changes; Rust analogue) (M §1, I2, §4a preflight/step 1; S §6a, §6b, §6c, A.2, A.6,
  Appendix B; E case 12, §10).
- D5 (P2) — resolved (round 4): R1-stage-1 deletion is commit-before-flush (as §4a step 5
  documents), so a flush failure leaves the row UNKNOWN; retry tolerates the row already gone,
  re-verifies destination absence, and reconciles by re-reading the subscriptions table after a
  flush error (absent ⇒ step 4 complete, continue at step 5) (M §4c stage 1).
- D6 (P2) — resolved (round 4): same-id submission of a `Prepared` operation re-runs the audit
  barrier (idempotent by event id) and spawns the executor; `destruction_retry` accepts
  `Prepared` as well as `Blocked`; the executor is spawned BEFORE the handler returns the
  operation id; a failed barrier spawns nothing — the stranded `Prepared` row is claimed by
  same-id submission, `destruction_retry`, or the periodic recovery pass (claims `Prepared`
  rows older than 5 minutes); callers see operation id + state and `destruction_audit_unavailable`
  while the barrier keeps failing (M §3e item 3, §4b, §4d step 5, §4f; S §6c row, Appendix B).
