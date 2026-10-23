# PR8b-3 design (#196) — refresh every source citation to current master (amendment A13)

## Context

The PR8b-3 destination-lifecycle design documents were written and reviewed against an old
master. Amendment A13 of the DEC-3 ruling requires every cited path/line to be valid against
current master before this design becomes the merged baseline. Since then the engine layout
sweep (stages D2, E1, F and the earlier ones) moved many files, so some citations are dead and
line numbers may have drifted. The design content itself is final (rounds 1-6 applied
A1..A15, the defects D1-D6 and the three final-review items). **This task changes citations
and the status header only. No rule, state, API, case, table row meaning or any normative
sentence may change.**

The worktree `worktrees/pr8b3-design` already contains `origin/master` merged in (commit
`c1fbdeec`), so every cited file must be checked in THIS worktree.

## Scope

Files: `docs/dev-artifacts/prompts/remediation/131-s5cx-destination-lifecycle.md`,
`131a-s5cx-destination-lifecycle-evidence.md`, `131b-s5cx-destination-lifecycle-surface.md`
(131c and 131d are historical and non-normative: do not touch them; 131e-131i are the
amendment/fix logs: do not touch them).

1. Extract every source citation (`crates/...` paths, bare `file.rs:NNN`, shorthand rows such
   as those in the 131a appendix mapping short names to full paths) from the three files.
   There are about 41 distinct full source paths; at least these no longer exist at the cited
   location and must be re-resolved: `repo/changelog_store/write_domain.rs` (now under
   `repo/journal/changelog_store/`), `meta/repo_identity.rs` (now `repo/meta/`),
   `execute/admin_replication.rs` (now under `execute/admin_db/`), `shamir-storage/src/error.rs`
   (now `core/error.rs`), `clean_horizon/shutdown_impl.rs`, `repo_types.rs`,
   `follower_durability/receipt/resume/worker.rs`, `db_handler/admin.rs`,
   `tests/access_tree_e2e.rs`. Find the real current location of each with file listing /
   search in the worktree (`git ls-files`, grep for the item name); never guess.
2. For each citation with a line number, open the file at the resolved location and confirm
   the cited line (or range) still shows the claimed item; if lines drifted, update the number
   to the new correct line. If an item no longer exists at all, do NOT invent a replacement:
   report it (path, claim, what you found) in the final message and mark the citation in the
   document with the existing "proposed" convention only if the document already uses it for
   such cases; otherwise leave the text and list it for the orchestrator.
3. The 131 header says it was "verified against HEAD 585259f0": replace that SHA with the
   output of `git rev-parse --short=8 origin/master` and add a one-line note "citations
   refreshed after the engine layout sweep (stages D2/E1/F)".
4. Preserve every table structure, anchor and cross-reference. Do not reflow paragraphs.
   Docs only: no code files, no Cargo files, no workflow files.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index (including `git add` / `git commit`). Only
  edit files; the orchestrator commits.
- Compile / build / test policy: see the policy text at the top of this run (verbatim). This
  task needs no cargo at all. Allowed: `git diff --check` (must exit 0), `python -I
  scripts/ci/check_layout.py` (the layout check may exit 2 only because of untracked changes;
  say so).
- NEVER stop processes by image name or pattern. Never use the shell builtin `xargs`. Capture
  exit codes without pipes. No machine-specific absolute paths in any file.
- Before finishing, start ONE read-only verifier child (policy pasted verbatim) that re-checks
  every changed line of the diff: the diff must contain ONLY citation text and the header
  line, and every cited path must exist in the worktree. At most one child at a time.

## Final message

1. The list of every citation re-read (count) and every changed one (old -> new, file:line).
2. Citations whose item disappeared or could not be resolved, if any.
3. Exit codes of `git diff --check` and the layout check; confirm no normative text changed.
