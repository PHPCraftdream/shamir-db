# 30 — S5c-X PR6b-2c receipt activation, W6: docs

Parent: `docs/dev-artifacts/prompts/remediation/119-s5cx-receipt-activation.md`. W1–W4 landed in the
working tree. Read CLAUDE.md. Hard rules: ⛔ no git-mutating commands (read-only git only); NO
cargo/build/test; FILE SCOPE = the three doc files below ONLY; no machine-specific absolute paths.

## FILE SCOPE

- docs/guide-docs/follower-journal-durability.md
- docs/guide-docs/KNOWN_LIMITATIONS.md
- docs/dev-artifacts/roadmap/REPLICATION.md

## Work items

Port the old branch's doc changes
(`git diff origin/master...feat/s5cx-follower-receipt-2 -- docs/guide-docs/follower-journal-durability.md docs/guide-docs/KNOWN_LIMITATIONS.md docs/dev-artifacts/roadmap/REPLICATION.md`)
and ADAPT the wording to what actually shipped in W1–W4 (read the code sections you document —
`crates/shamir-engine/src/tx/follower_durability/receipt/**`,
`crates/shamir-engine/src/tx/cold_reconcile/receipt_recovery.rs`,
`crates/shamir-engine/src/tx/cold_reconcile/lifecycle/owner.rs` doc comments, and the existing
lifecycle contract section of follower-journal-durability.md — do not document removed mechanisms):

1. **follower-journal-durability.md** — the receipt-apply section must state, in the existing doc's
   voice: the receipt lifecycle (Prepared → Materialized → Checkpointed, one atomic repository batch
   per phase, publication only after durability, Uncertain → read-back reconcile); the ACTIVATION RULE
   (WAL-less follower + backend `supports_atomic_batch` + stored bookmark v3-or-absent → receipt path;
   v1/v2 bookmarks keep the legacy path until PR6b-3 migrates them; unsupported persistent backend →
   `DbError::AtomicBatchUnsupported` before allocation); the NEW follower guarantee (retry convergence:
   no extra versions, no lost counters, no new gaps on ordinary I/O failure — an ordinary I/O failure
   PAUSES with a paused write reservation and the next pull resumes the SAME receipt; corruption /
   partial publication / identity mismatch / shutdown timeout remain fail-closed quarantine + reopen);
   admission = the repository write-domain permit (permit → marker-mutex lock order; DDL and local
   writes are excluded for the pipeline's whole run; paused reservations reject ordinary admission via
   `RepoWriteError::Reserved` until the receipt resolves); the worker = the repository-owned
   lifecycle (owner holds the sender, jobs carry payload + reply, Open → Closing → Joined,
   unconditional quarantine on timeout or panic; shutdown reaches publication or a safe durable pause
   and reports failures to the caller); cold recovery completing Prepared/Materialized receipts BEFORE
   PR6c gap classification; journal installs ride the sole journal writer (`install_durable_events`,
   never `try_send`). Do NOT document the removed `ddl_admission` mutex, `DdlGuard`, or the old
   `ReceiptWorker`.
2. **KNOWN_LIMITATIONS.md** — update the entries the old branch touched: the follower apply guarantee
   changes from "barrier failure is terminal" to the pause/resume contract; keep (still true) the
   index/vector maintenance rejection before allocation, the PR6b-3 migration step as the remaining
   activation item for existing v1/v2 deployments, and any entry the old branch removed that is now
   actually resolved (state it as resolved by this PR).
3. **REPLICATION.md** — port the roadmap/status edits, adapted: mark the receipt apply/resume unit
   (PR6b-2c) as implemented on the merged foundations (#182 lifecycle, #183 write domain), listing the
   same activation rule and pause/resume guarantee.

House style: match each file's existing heading levels, tone and terminology; no marketing language;
every behavioral claim must be true of the code in this tree (check before writing).

## Acceptance criteria

- Only the three files touched; the diff is wording-level; every documented behavior matches the code.
- Final message: per file, what changed and which old-branch wording you deliberately altered, plus
  anything you chose NOT to port and why.
