# P34-03d - table rename "in doubt": reconcile after a persist error, name fence, three-state outcome, M5 fix

Base: sub-stage 03c-1 is landed (the writer/DDL freeze of a table rename). Read, in this order: `docs/dev-artifacts/prompts/remediation/p34/03c-rename-in-doubt-barrier.md` (original brief, decisions 1-9), `03c-revision-1.md`, `03c-revision-2.md` (its S6 lists exactly what was deferred to this stage; its "Decisions taken", failure-mode table M1-M7 and acceptance sections are the background) and the design note `docs/dev-artifacts/audits/2026-11-02-p34-03c-rename-in-doubt/DESIGN.md` plus `REVIEW-1.md`. CLAUDE.md applies in full (layout limits, one primary export per file, no logic in mod.rs, tests in `tests/` folders, lock-free primitives and `THasher` collections, no raw `std::collections::HashMap/HashSet` - use `shamir_collections`).

## Scope (the S6 list of revision 2; implement in this order, each item DONE / PARTIAL / BLOCKED in the note)

1. **M5**: after a SUCCESSFUL rename (and DROP TABLE) a transaction staged through a pre-rename clone and committed afterwards must be rejected, not land rows in orphaned stores (revision-2 table row M5). Test first (compile-checked by reading, run by CI).
2. **`RenamePersistOutcome` and the three-state result**: when persisting the rename fails, the engine must not guess: reconcile against the durable catalogue and end in exactly one of {rolled forward (rename is durable), rolled back (old name is durable), still in doubt (could not determine)}; the table stays frozen/fenced while in doubt and the caller gets the documented error code (original decisions 2/3/5, R5 phases and into-doubt-on-drop, R6, R7).
3. **R2 name fence and mirror**: while a rename is in doubt the old and the new name cannot be taken by another DDL; the in-memory mirror and the durable catalogue are kept consistent.
4. **R9 repo-rename guard** and **R10 wire code** for the in-doubt error, **R11**, original decisions 6/7 (catalogue split, `resolve_table_rename`, retry convergence: a retried rename converges to the same end state; the durability-tail seam), and the stage-before-staging vector shape from the stop-E verdict.
5. **On-disk restart test** (tests with a real on-disk store, restart in the middle of an in-doubt rename, reconcile on open).

Where a revision-1/2 decision contradicts the code you find, follow the code and record the correction with file:line in the note. Append to the note (never replace): `## Phase 0 (03d)` (every claim you rely on, with file:line), `## Design (03d)`, `## Result (03d)` with the per-item status, the failure-mode table updated, every visibility widening, and the test list.

## Rules

- NO LOCAL COMPILATION OR TESTS: the only commands allowed are `cargo fmt -p <crate>` (and `-- --check`) for the crates you touch (shamir-storage, shamir-engine, shamir-db) and `python -I scripts/ci/check_layout.py`. No `cargo check/clippy/build/test`, no rustc. A child that runs them violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt. No `Cargo.toml`, lockfile, workflow edits; a new dependency means stop and report.
- Nothing is compiled locally: finish with a compiler-style read of every file you touched (privacy of paths, exhaustive `match`, struct literals with every field including in `#[cfg(test)]` constructors, imports, `rustfmt` alphabetical `mod` order, clippy classes `type_complexity`, `useless_vec`, `needless_borrows_for_generic_args`, `result_large_err`, `disallowed_types` for std HashMap/HashSet, unused imports in test targets; no new `#[allow]`, no local `use`, no inline `#[cfg(test)] mod`).
- Layout: at most 7 entries per folder and 1000 lines per file; count with `ls` before adding; new logic goes into new files or subfolders.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. No machine-local absolute paths in any file you write.
- Children only edit files, are awaited, and every touched file is re-read as the compiler would. Never finish with a child outstanding.
- Time: run `date` first; stop at the time in the run file and report per item in the note.
