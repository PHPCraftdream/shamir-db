# Compliance & Ops 5a — audit-coverage doc fix (AuditSink→AuditChainWriter bridge is deferred)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

First item of "Этап 5 — Compliance & Ops"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 03
(`docs/dev-artifacts/research/2026-07-17-release-audit/03-compliance-data-governance.md`).
The work plan explicitly scopes the REAL fix (bridging `AuditSink` to the
durable `AuditChainWriter`, adding append call sites on DDL/ACL/admin
operations) as **P1 / deferred** — this task is DOCUMENTATION-ONLY: "until
then, fix the docs" (verbatim from the work plan).

## The gap (already fully investigated in report 03 — read it yourself too)

Report 03's capability matrix, row **1b** (`03-compliance-data-governance.md`,
~line 54), and the "P1 — Audit log covers only authentication; docs claim
more" section (~lines 80-91):

> The only production append call site is
> `crates/shamir-server/src/connection/handshake.rs:50` (`ctx.audit.append`)
> — auth events only. The `AuditSink` used by the admin API (`user_created`
> etc., `crates/shamir-connect/src/server/admin.rs`) has only an **in-memory
> Vec** implementation, used in tests. No DDL, ACL/chmod/chown, admin ops
> (`CreateScramUser`, `SetSuperuser`, retention/purge), interactive-tx, or
> backup events reach the durable HMAC-chained audit log.
>
> Contradicts `docs/guide-docs/guide/07-operations.md:343` ("События:
> аутентификация, DDL, ACL-изменения, admin-операции").

The exact current (false) doc text, `docs/guide-docs/guide/07-operations.md`
~lines 341-343:

```
Audit-лог в audit-line-формате с HMAC-chain (каждая запись включает HMAC от предыдущей).
Ротация по размеру; устаревшие файлы удаляются автоматически.
События: аутентификация, DDL, ACL-изменения, admin-операции.
```

## The task

Correct ONLY the third line ("События: ...") to accurately state that the
durable, HMAC-chained audit log currently covers **authentication events
only** — DDL/ACL/admin operations are NOT yet durably audited (only
ephemeral `log`/`tracing` output exists for those, which is explicitly
documented elsewhere as "NOT an enforcement gate, not persisted" per
`crates/shamir-types/src/access.rs`'s `trace_access` — you can reference
this if useful context, but do not need to re-derive it, report 03 already
covers it under capability-matrix row 1c). State plainly that broader
coverage (DDL/ACL/admin durable audit) is a planned, not-yet-implemented
enhancement.

**Do NOT touch the second line** ("устаревшие файлы удаляются
автоматически", the `retention_days`-dead-knob claim) — that line's
correction depends on a SEPARATE, later task in this same campaign (5d,
"Fix dead audit.retention_days config knob") deciding whether to implement
a real retention sweep or remove the knob; whichever code path that task
takes determines whether this second line becomes true or needs removal.
Fixing it now, before that decision is made, risks contradicting whatever
5d lands on. Leave it exactly as-is.

**Do NOT touch the Backup section** (`## 8. Backup`, further down in the
same file) — that is a separate later task (5b) in this same campaign.

Check whether ANY other doc file in `docs/guide-docs/` or elsewhere makes
the same "DDL/ACL/admin operations are audited" claim (grep for similar
phrasing, e.g. "DDL" near "audit" / "аудит") and correct those too if
found, so the fix isn't only cosmetic in one file while another doc still
overclaims.

## Out of scope

- Do NOT implement the `AuditSink` → `AuditChainWriter` bridge itself, or
  add any new append call sites — that is explicitly P1/deferred per the
  work plan; this task is documentation-only.
- Do NOT touch `07-operations.md`'s retention-days line (see above) or its
  Backup section — separate later tasks in this campaign.
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, or Этап 4's funclib top-up — this brief is scoped to the
  single audit-coverage doc claim only.

## Verification (MANDATORY before you report done)

- Read the corrected doc section aloud (mentally) as an operator would —
  confirm it is unambiguous about what IS vs is NOT durably audited today.
- If this project has any doc-linting/build step that validates
  `docs/guide-docs/` (check for a doc-build CI job or script), run it and
  confirm it passes — if no such tooling exists, say so explicitly rather
  than inventing a check.
- Since this is a docs-only change, there is no `cargo test`/`clippy`/`fmt`
  gate to run — confirm explicitly that you did NOT touch any `.rs` file
  (a `git diff --stat`-style self-check, described in your summary, is
  sufficient — you don't have git access yourself per the rules above, so
  just state which files you edited).
