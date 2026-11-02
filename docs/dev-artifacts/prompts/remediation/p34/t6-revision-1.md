# TOKEN-T6 revision 1 - make the first run compile, fix the review defects, finish the acceptance items

Base: `t6-ws-token-channel.md` (the original brief: read it fully with its Rules, Decisions taken and acceptance list; every rule there applies verbatim: NO LOCAL COMPILATION, the git-mutation prohibition, layout limits, clippy classes, no machine-local paths). The first executor run timed out; the tree has 26 compile errors, about 29 lint/gate failures and 33 places where behaviour contradicts the brief. An independent compiler-style review is in `docs/dev-artifacts/audits/2026-11-02-token-t6/REVIEW-1.md` (read ALL of it): section 1 is the defect list with exact fixes, section 2 the acceptance status, section 3 the finishing plan with three disjoint work packages WP-A / WP-B / WP-C and the cross-package contracts.

Step 0 of REVIEW-1 section 3 (moving `http_ws/` files into `browser/`, turning `token_ws/tests.rs` into `tests/`, the new `http_ws/mod.rs`) has ALREADY been done by the orchestrator: do not redo it; the paths in the packages that mention it now refer to the moved files (`http_ws/browser/{policy,pre_auth,first_frame}.rs`, `token_ws/tests/binding_tests.rs`). Re-check that every `mod`/`use` path in the tree matches.

## Scope

Implement WP-A, WP-B and WP-C of REVIEW-1 section 3 completely, in the order and with the exact fixes it gives, plus the acceptance items 1-12 of the original brief that REVIEW-1 section 2 marks PARTIAL/MISSING within those packages. The contracts between packages in REVIEW-1 section 3 ("Cross-package contracts, do not change") are fixed: each package must compile against the other's declared interface.

## How to work

The three packages have disjoint file sets. Work them in parallel: delegate each package to its own child agent that ONLY edits files (never runs commands; give each child its file set, the package text, the cross-package contracts, the no-build / no-git rules verbatim and the clippy classes), `await_tasks` every child, then re-read every file a child touched as rustc/clippy/rust-guard: open the target of every `use`, check visibility and signatures at all call sites, trait imports, moved values (watch fields moved out of `Drop` types, `Arc<Verified>` vs `Verified`, `split()` order), Send/'static across `.await`, `result_large_err`, `manual_async_fn`, `private_interfaces` after narrowing re-exports, `absolute_paths` (max 5 segments), `too_many_arguments` (max 7), no local `use`, no `#[allow]`, every test file mounted and `#[cfg(test)]`-gated, folder entry counts by `ls` (max 7) and file lengths (max 1000). Run `cargo fmt -p shamir-transport-http -p shamir-transport-ws -p shamir-connect` after WP-A and `cargo fmt -p shamir-server` ONCE after both server packages finish (REVIEW-1 says so). Files that must be deleted (executors cannot delete): list them in the final report.

Existing tests of the REST path must keep their meaning (REVIEW-1 S3 lists the ones that were edited to fit the new code: restore them from master `git show origin/master:<path>` read-only and adapt only what the endpoint parameter forces).

## Rules (restated)

- NO LOCAL COMPILATION: the only cargo commands allowed are `cargo fmt -p <crate>` (and `-- --check`) and `python -I scripts/ci/check_layout.py` (it ignores untracked files: count by hand). No cargo check/clippy/build/test, no rustc, no npm - a child that runs one violates the project's compile policy (a previous child did and left a 2 GB target directory). Do not edit `Cargo.toml` / `Cargo.lock`; a new dependency means stop and report.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index; only edit files; the orchestrator commits.
- DEADLINE: stop editing at the time in the run file and report; never revert or empty files; never finish with a child outstanding.
- Append progress and the final per-package, per-acceptance-item DONE / PARTIAL report (file:line, what needs CI) to `docs/dev-artifacts/audits/2026-11-02-token-t6/DESIGN.md` (append; correct its stale blocker list).
