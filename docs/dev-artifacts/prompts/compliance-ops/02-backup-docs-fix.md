# Compliance & Ops 5b — fix 07-operations.md backup docs (stop-and-copy, not live)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Second item of "Этап 5 — Compliance & Ops"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 03
(`docs/dev-artifacts/research/2026-07-17-release-audit/03-compliance-data-governance.md`,
capability-matrix row 5, and the "P2 — Backup doc contradicts
implementation" section):

> `backup.rs` is explicitly stop-and-copy ("Operator stops the server",
> `backup.rs:3`); it takes no lock and does not quiesce the engine.
> `07-operations.md:347-353` instructs operators to run it against a live
> server and asserts it "блокирует данные". A backup taken per the guide
> can be torn (mid-WAL-write / mid-SST-flush) and silently unrestorable.

This is a **dangerous doc drift**, not a cosmetic one: following the
current doc's instructions against a live server risks a torn, silently
unrestorable backup — a real data-safety hazard for any operator who
trusts the guide as written.

## The gap — read `crates/shamir-server/src/backup.rs`'s module doc comment
(~lines 1-16) first, it is the ground truth:

> v1: stop-and-copy. **Operator stops the server**, runs `shamir-server
> backup --from <data_dir> --to <dest>` which recursively copies every
> file under `data_dir`... Why stop-and-copy instead of an online backup:
> ...copying the file while the writer is paused (i.e. server stopped) is
> the simplest path... a copy taken during a quiescent window between
> commits is recoverable as the pre-commit state on the next open.
> Future enhancement (P2): in-process snapshot... without downtime. Not
> done here because it requires a wire-protocol change.

The current (false) text in `docs/guide-docs/guide/07-operations.md`,
`## 8. Backup` section (~lines 344-353):

```
## 8. Backup

Однократный snapshot — без остановки сервера:

​```bash
shamir-server --config db.ktav backup --to /backup/shamir-$(date +%Y%m%d)
​```

Блокирует данные на время snapshot, записывает в целевой каталог.
Для scheduled backup — cron / systemd timer.
```

Two false claims:
1. "без остановки сервера" ("without stopping the server") — the operator
   MUST stop the server first; there is no online/live backup mode.
2. "Блокирует данные на время snapshot" ("locks/blocks data during the
   snapshot") — there is no lock or quiesce mechanism at all; this phrase
   implies an active consistency guarantee that doesn't exist. The actual
   safety property is different and weaker: a copy taken with the server
   STOPPED is safe because there's no concurrent writer, not because
   anything "blocks."

## The fix

Rewrite the `## 8. Backup` section to accurately describe the v1
stop-and-copy model:
- The operator must stop the server (`shamir-server` process) before
  running `backup`.
- The command copies the ENTIRE `data_dir` (not just a subset) into
  `<dest>/<timestamp>/`.
- There is no lock/quiesce/online-backup capability today — running
  `backup` against a LIVE server is unsafe and can produce a torn,
  unrestorable copy (mid-WAL-write / mid-SST-flush).
- An in-process online-snapshot capability (no downtime) is a planned P2
  enhancement, not yet implemented (cite `backup.rs`'s own "Future
  enhancement (P2)" note if useful, don't invent new roadmap language).
- Keep the "for scheduled backup — cron / systemd timer" guidance, but
  make clear the scheduled job must ALSO stop the server first (e.g.
  systemd `ExecStartPre`/`ExecStopPost` ordering, or a wrapper script that
  stops, backs up, restarts) — a naive cron job running `backup` against a
  still-live server is exactly the hazard this fix exists to prevent.

Verify the exact CLI invocation shown in the doc against the real CLI
definition (`crates/shamir-server/src/main.rs`'s `Subcmd::Backup`) before
leaving the example command as-is or adjusting it — confirm whether
`--from` is a real flag or whether `data_dir` comes from the loaded
`--config` file (read the dispatch code, ~`main.rs`'s `Some(Subcmd::Backup
{ to })` arm, to see exactly what's passed to `backup::backup`).

## Out of scope

- Do NOT touch the audit-coverage line (already fixed in task 5a) or the
  retention_days line (task 5d's job) in the same file — this brief is
  scoped to the `## 8. Backup` section only.
- Do NOT implement the in-process online-snapshot P2 enhancement, or add a
  Restore subsection with the `revokeAllTickets` runbook obligation
  (report 03 notes this as a separate, currently-unenforced operator
  obligation) — both are bigger scope than "fix the doc drift" calls for;
  if you have clear capacity and it's a trivial one-paragraph addition,
  you MAY add a short restore-obligation note, but do not treat it as
  required — state explicitly in your summary whether you included it or
  left it out.
- Do NOT fix `backup.rs`'s own internal doc comment mentioning "redb
  files" (storage has since moved to fjall) — that staleness is tracked
  under a separate, later campaign task (Этап 6, item 3: "03-storage.md +
  doc-комменты: redb→fjall").
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, Этап 4's funclib top-up, or task 5a (already committed) —
  this brief is scoped to the Backup section fix only.

## Verification (MANDATORY before you report done)

- Docs-only change — confirm you touched ONLY
  `docs/guide-docs/guide/07-operations.md` (or state explicitly if you
  found and fixed a genuinely necessary secondary location, per the "if
  found" guidance elsewhere in this campaign's docs tasks — but be
  conservative here, this brief's scope is narrower than 5a's).
- Re-read the corrected section as a fresh operator would — confirm it
  cannot be misread as "safe to run against a live server."
- No `cargo test`/`clippy`/`fmt` gate applies (docs-only) — state this
  explicitly rather than running an irrelevant gate.
