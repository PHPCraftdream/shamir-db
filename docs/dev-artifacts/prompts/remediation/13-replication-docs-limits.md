# Brief — S0 replication docs

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.

## FILE SCOPE (only these paths may be edited)

- `docs/guide-docs/KNOWN_LIMITATIONS.md` (the replication section, ~§5, including the "Resume" procedure at ~:464-474)
- `README.md` (the experimental replication bullet, ~line 70 only)
- `docs/guide-docs/guide/08-interconnect.md` (replication parts only)
- NO code, NO other files

## Task

[S0] Replication docs — state plainly what does NOT work today.

Source of truth: a read-only code review of the replication path on master (every statement below was verified by reading the code; keep the file:line hints in your head, do not paste them into user docs unless a doc already cites code locations). The project is an ALPHA; replication is already labelled experimental. The docs must stop promising more than the code delivers, and say what an operator can actually do.

Write (concise, honest, in the existing style/tone of each document; the docs are English):
1. A replicated follower cannot reliably READ replicated map records yet: record field keys are ids of the leader's interner and the journal event carries no id->name bindings, so a follower either fails to decode them or reads them under the wrong field names. State this as a current limitation (a fix is planned: translating field names per event).
2. Secondary indexes and `COUNT(*)` are not maintained on a follower: index lookups miss replicated rows and `SELECT COUNT(*)` reads the table counter, which replication does not update (it reads 0).
3. DDL / schema / metadata is NOT replicated (only the data writes of a transaction). Today an event for a table that does not exist on the follower is skipped with a warning and the bookmark still advances (silent loss); create the same tables on the follower first and do not create secondary indexes there.
4. `ReplMode::ReadOnly` is not enforced: a follower accepts local writes.
5. The leader's journal can have SILENT holes that the gap detector does not catch: (a) the journal event is queued after the version is published and outside the commit lock, so with concurrent leader writes version 11 can be journaled before version 10 and a follower that applied 11 never asks for 10; (b) a failure to persist one journal record is only logged; (c) a corrupt journal record is skipped silently; (d) the in-memory gap marker is forgotten on leader restart and the unflushed tail of the journal can be lost on a crash. Say that, with concurrent writes on the leader, a follower can silently miss an event today.
6. Recovery: there is no snapshot/reseed and no journal retention. If a follower is behind a gap, recreate it from scratch (bookmark 0, full replay) — this only works if the leader's journal has no hole; otherwise restore the follower from a backup. `Resume` only changes the subscription state and fixes nothing: the current text that tells the operator to "repair the data and Resume" is misleading — rewrite that procedure.
7. Two subscriptions for the same (db, repo) start two loops, and advancing the replication bookmark is not atomic. The bookmark lives in the `__tx__` store without a flush ordering against the follower's data, so after a crash the bookmark may be ahead of the data (unverified on the on-disk engine) — mark this as "not verified".
8. README: keep the experimental bullet short but make it say "not for production data; see KNOWN_LIMITATIONS".
Do not touch any other section, do not change claims about other features, do not invent behaviour that is not listed above.
