# 259 — P1 post-merge audit: verdict table and report (docs only)

## Goal

Write `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1-POST-MERGE-AUDIT.md`:
the sign-off record of the read-only post-merge audit of every merged P1 PR
(55 PRs, #311–#378), run 2026-10-29 by 13 rush-reviewer sessions (batches A1–A13).

## Inputs (read-only, untracked, never cite their location in the report)

- `.audit-input/A1.md` … `A13.md` — one reviewer report per batch; each PR section
  carries a verdict (`OK`, `OK_WITH_NITS`, `FAIL`), coverage, tests, defects, rules.
- `.audit-input/audit_batches.json` — batch → `[pr, squash sha, title, files, +, -]`.
- `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1-FIX-PLAN.md` and
  `P1-FIX-TASKS.md` — the cards (F*, G*, H*, RG*, RU*) the PRs implement.

## Orchestrator triage (authoritative; the report records it, does not re-judge it)

Each FAIL was re-checked against master. Outcome:

| Card | Finding | Disposition |
|---|---|---|
| F2 | RV-STORAGE-EC3A171B (fjall 3.1.6 drops journal write error) | fix: lock-only bump fjall/lsm-tree 3.1.6 → 3.1.10 + lock guard test (owner-approved) — PR #393 |
| F6 | typed `io::ErrorKind` lost on append/fsync/replay/sync-snapshot | fix: stage 252 (in progress) |
| F9 | RV-INDEX-E69B5ACD SQ8 carrier | fix: stage 256, carrier in `IndexDescriptor::options` (design decision by @oh) |
| F14 | regex/LIKE validated after unknown-field folding | fix: PR #395 |
| G4 | wasm-host HTTP gateway IP policy | fix: stage 255, three-class `classify_ip` policy (@oh) |
| G5a | RV-WASM-HOST-91385BE0 test matrix incomplete, probabilistic setter tests | fix: stage 258 (tests only) |
| G5d | proc-tree startup cleanup unbounded, kill errors ignored | fix: PR #397 |
| G10 | root `ResultEncoding` re-export removed by #334 | fix: PR #396 |
| H5 | unknown-field rejection, deterministic `FilterValue`, re-encode trampoline; RU6 test | RU6 + G10 fixed in PR #396; the rest pending a design decision (tracked) |
| H8 | (reviewer FAIL) | already closed by H8b — no second fix |
| H11 | RV-FUNCLIB-8B0C96FB argon2 offloaded at 1 of 4 sites | fix: stage 257; acceptance amended by @oh: off the workers on multi-thread runtimes, inline + warn-once + counter on current-thread |
| RG1 | rust-guard orderings gate missed UFCS atomic calls | fix: PR #394 (merged) |

`OK_WITH_NITS` items are recorded but **not scheduled**; list them so the owner can decide.

## Report structure

1. Scope and method (one paragraph: read-only, static review, no builds; verdict scale).
2. Summary counts: PRs per verdict; FAILs real vs already-closed.
3. Verdict table — one row per PR in PR-number order: PR, card, verdict, one-line
   reason, disposition (from the triage table, else `signed off` / `nits recorded`).
4. FAIL arbitration — one short subsection per FAIL card: finding, evidence summary,
   disposition.
5. Unscheduled nits — grouped by crate, one line each with the reviewer's file:line
   reference (repo-relative paths only).
6. Process notes — what the audit could not verify (runtime behaviour, cross-platform)
   and that CI is the authority.

## Rules

- Docs only: create exactly that one file. Do not edit any other file.
- Repo-relative paths only. Never write an absolute local path (drive letters, user
  home, cargo registry) — strip them from quoted reviewer text.
- English, concise; tables over prose. Keep it under ~600 lines.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
  git command that mutates the working tree or index. Only edit files; the
  orchestrator commits.
- ⛔ No builds, tests or benches of any kind (`cargo build/test/run/bench/doc`,
  nextest, `scripts/test.sh`, npm). Allowed locally only: `python -I scripts/ci/check_layout.py`.
