# Compliance & Ops 5c — expand data-protection.md §2 (index/replica/FTS erasure remnants)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Third item of "Этап 5 — Compliance & Ops"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 03
(`docs/dev-artifacts/research/2026-07-17-release-audit/03-compliance-data-governance.md`),
capability-matrix rows **2f** and **2g** (undocumented gaps — the CODE
behavior already exists and is correct/intentional; this task is a
DOCUMENTATION expansion, not a code change).

## The two gaps (already investigated in report 03 — verify the code
citations yourself before writing, this campaign's earlier stages have
touched some of these files)

**Row 2f — secondary-index residuals (HNSW/FTS) after delete/purge:**

> HNSW deletion is a **soft-delete tombstone set**; vectors of tombstoned
> ids remain in the adapter until compaction
> (`crates/shamir-index/src/vector/hnsw_adapter.rs:7-8,232,503` — tombstone
> internals are even iterated "for snapshot serialisation", i.e.
> **persisted into index snapshots**), compaction at
> `crates/shamir-index/src/backend.rs:242` +
> `vector/tests/compaction_tests.rs`. `data-protection.md` §2 does not
> mention index residuals at all — an operator following its erasure
> procedure would not know deleted embeddings can persist in
> `PersistedIndexes` snapshots until a compaction rewrite. FTS posting
> reclamation is similarly undocumented in the compliance doc.

**Row 2g — purge propagation to replicas:**

> Replication is a leader-side pull API
> (`crates/shamir-server/src/db_handler/repl_handler.rs:1-10`, gated on
> `is_replicator`/superuser at `:50`). Nothing propagates
> `PurgeHistory`/`Delete`-vacuum to already-pulled replica copies;
> `data-protection.md:85` only says backup/replica volumes must be
> encrypted. For GDPR Art. 17 the operator must purge each replica
> independently — nowhere stated.

Before writing, investigate:
1. Read `hnsw_adapter.rs` and `backend.rs`'s compaction code (the exact
   lines cited above) to confirm the tombstone-until-compaction behavior
   and word the doc precisely (don't paraphrase from the audit report
   alone — verify against the actual code).
2. Search for the FTS (full-text-search) posting-list equivalent of this
   same tombstone/compaction pattern — the report says "similarly
   undocumented" but doesn't cite exact FTS line numbers; find them
   yourself (grep for FTS index/posting-list deletion/compaction code,
   likely near wherever the FTS index implementation lives — check
   `crates/shamir-index/src/` for an FTS-specific module).
3. Read `repl_handler.rs`'s pull-API gating to confirm the "replication
   doesn't propagate purges" claim, and read `data-protection.md:85`'s
   current wording (the "encrypt backup/replica volumes" line) to know
   exactly what's already said vs. what's missing.

## The task

Add new subsection(s) to `docs/guide-docs/security/data-protection.md`'s
`## 2. PII retention / erasure procedure`, positioned after the existing
`### 2.4 Residual: the field-name interner (honest assessment)` subsection
(read 2.4 first — mirror its tone and structure EXACTLY: "This is the
residual the audit specifically flagged, documented honestly," concrete
code citations, a clear statement of what an operator must understand and
do about it). Two new subsections (numbering continues from 2.4, so `2.5`
and `2.6`, or fold into one `2.5` with two clearly-headed parts — your
call, whichever reads better given how 2.4 is structured):

- **Secondary-index residuals (HNSW / FTS)**: explain that vector-index
  deletion is a soft-delete tombstone, embeddings physically persist
  (including in on-disk snapshots) until a compaction rewrite runs; same
  story for FTS postings (once you've found the exact code). State
  plainly: an operator executing the §2.2 erasure procedure has NOT
  achieved physical erasure of index-resident copies until compaction has
  run — this is a real gap in the erasure procedure's completeness, not
  just a documentation nicety.
- **Purge propagation to replicas**: state plainly that `PurgeHistory`/
  delete-vacuum operations do NOT automatically propagate to already-
  pulled replica copies; an operator satisfying a GDPR Art. 17 (right to
  erasure) request across a replicated deployment MUST separately purge
  each replica — there is no single-command "purge everywhere" today.
  Cross-reference `data-protection.md:85`'s existing "encrypt backup/
  replica volumes" line as adjacent-but-insufficient context, don't
  duplicate it.

Also check `### 2.2 How to satisfy a "right to erasure" (GDPR Art. 17)
request` — if its procedure steps currently imply completeness without
mentioning these two residual classes, add a cross-reference to the new
subsection(s) from within 2.2's own step list so an operator following
that procedure is actually pointed at the caveats, not just hoping they
read the whole document.

## Out of scope

- Do NOT implement a "purge everywhere" replica-propagation feature or a
  forced HNSW/FTS compaction-on-erasure feature — these are documentation
  fixes describing EXISTING (correct, intentional) behavior, not new code.
- Do NOT touch `### 2.1`/`### 2.3`/`## 3`/`## 4` or any other section of
  this file beyond the 2.4-adjacent additions and 2.2's cross-reference.
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, Этап 4's funclib top-up, or tasks 5a/5b (already committed)
  — this brief is scoped to the index/replica erasure-remnants doc
  expansion only.

## Verification (MANDATORY before you report done)

- Confirm every code citation you add (file:line) actually matches the
  current code — don't just copy report 03's line numbers verbatim
  without checking they still point at the right place (this campaign has
  modified several files since the report was written; the FILES cited
  above for rows 2f/2g have NOT been touched by this campaign so far, but
  verify anyway).
- Re-read the new subsections as an operator executing §2.2's erasure
  procedure would — confirm they'd now know to (a) wait for/trigger index
  compaction, and (b) purge every replica separately, neither of which
  was previously stated anywhere in this document.
- No `cargo test`/`clippy`/`fmt` gate applies (docs-only) — state this
  explicitly.
