# TOKEN-AUTH1 revision 1 - make the first slice compile, add config and the transport route hook

Base: `auth1-login-http-server.md` (the original brief: read it fully, with its Rules, Decisions taken and acceptance list; every rule there applies verbatim, including NO LOCAL COMPILATION, the git-mutation prohibition, the layout limits and the clippy classes). The first executor run timed out after doing only part of acceptance item 1 and the result does not compile. An independent compiler-style review is in `docs/dev-artifacts/audits/2026-11-02-token-auth1/REVIEW-1.md` (read ALL of it): section 1 is the DONE/PARTIAL/MISSING table, section 2 the defect list with exact fixes, section 3 the three work packages WP-A / WP-B / WP-C.

## Scope of this revision

Implement WP-A, WP-B and WP-C of REVIEW-1 section 3 completely, and nothing beyond them (items 4-10 of the original brief - wire, admission, pending store, services, launcher wiring, e2e - are the NEXT revision; do not start them, but do not make them harder: keep names and module boundaries as the original brief's "Decisions taken" prescribes).

- WP-A: fix every defect REVIEW-1 section 2 lists for the existing code (compile errors incl. `ns::MILLISECOND`, the missing `serde_json` dependency problem - parse the vector JSON without serde_json exactly as REVIEW-1 proposes or with an existing dependency, never edit Cargo.toml/Cargo.lock; missing trait imports; wrong vectors with the corrected hex given in the review; test logic bugs; B1: database names must follow `^[A-Za-z0-9_-]+$` and must NOT go through the username normaliser; FINISH padding required by the spec; the orphan `http_login/tests/mod.rs`; rustfmt; local `use` in cap_tests; unused re-export), and finish acceptance item 1 and the vectors (item 12 for the transcript part).
- WP-B: configuration keys, startup validations and the persisted listener id exactly as REVIEW-1 WP-B specifies.
- WP-C: the transport route hook in `crates/shamir-transport-http` exactly as REVIEW-1 WP-C specifies (no new field on `HttpServerConfig`; do not edit `shamir-server` in this package).

## How to work

The three packages have disjoint file sets (REVIEW-1 section 3). Work them in parallel: delegate each package to its own child agent that ONLY edits files (never runs commands; give each child its file set, the package text, the no-build / no-git rules verbatim and the clippy classes), `await_tasks` every child, then re-read every file a child touched as rustc/clippy: open the target of every `use`, check visibility and signatures at all call sites, trait imports, moved values, Send/'static, `result_large_err`, `absolute_paths` (max 5 segments), `too_many_arguments` (max 7), no local `use`, no `#[allow]`, mounted tests, folder entry counts by `ls` (max 7) and file lengths (max 1000). Files that must be deleted per REVIEW-1 (executors cannot delete): list them in the final report; the orchestrator removes them.

## Rules (verbatim from the original brief, restated)

- NO LOCAL COMPILATION: the only cargo commands allowed are `cargo fmt -p <crate>` (and `-- --check`) for the crates you touch and `python -I scripts/ci/check_layout.py` (it ignores untracked files: count by hand). No cargo check/clippy/build/test, no rustc, no npm. A child that runs one is a policy violation - say so in every child prompt. Do not edit `Cargo.toml` / `Cargo.lock`; a new dependency means stop and report.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index; only edit files; the orchestrator commits.
- DEADLINE: stop editing at the time in the run file and report; never revert or empty files; never finish with a child outstanding.
- No machine-local absolute paths in any file you write.
- Append progress lines and the final per-package DONE / PARTIAL report (file:line, what needs CI) to `docs/dev-artifacts/audits/2026-11-02-token-auth1/DESIGN.md` (append; it must end up with every section the original brief requires).
