# CI-WIN follow-up 1 - narrow the archive to the lib job, fix filters and cache key

Supersedes the parts of `01-test-build-split-and-cache-warm.md` that make `integration` consume the archive. Read the review findings below, then change the uncommitted work in this worktree (do not start over). Same rules as the first brief (no builds or tests, no git mutation, orchestrator commits).

## Review findings (independent read-only review of the first round; all accepted)

1. **Integration binaries are not relocatable.** `crates/shamir-sdk/tests/h9/consumer_compile.rs` searches beside its own executable for exactly one SDK `.rlib` and runs rustc against that dependency directory; `crates/shamir-server/tests/seed_recovery_e2e/harness/child.rs` embeds a build-time absolute server path with `env!`. A nextest archive extracted into a temp dir with `--workspace-remap` keeps neither. Library unit tests do not have these dependencies.
2. **`kind(lib)` is narrower than `--lib`.** The proc-macro libraries (`shamir-sdk-macros`, `shamir-query-builder-macros`) have unit tests with target kind `proc-macro`; the wrapper's `kind(lib)` filters drop them.
3. **Filter expressions are concatenated without grouping** (`kind(lib) and a or b`).
4. **An existing action pin was replaced:** `release.yml` already pins `actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1`; the brief said to reuse existing pins.
5. **"Same cache key by construction" is not established.** `Swatinem/rust-cache` hashes the environment variables whose names start with `CARGO`, `CC`, `CFLAGS`, `CXX`, `CMAKE` or `RUST` into the key (default `env-vars`). `ci.yml` sets `CARGO_TERM_COLOR` and `RUST_BACKTRACE` at workflow level; `cache-warm.yml` does not.
6. `cargo nextest archive` is a build-only command; a `-E` filterset is a run/list option. The first round passes `-E` to `archive`; do not rely on it.

## Required changes

1. **Scope.** Only the `test` job (library unit tests) consumes the archive. Restore `integration` exactly to its content on master: `needs: plan`, `Swatinem/rust-cache` + `mozilla-actions/sccache-action` + the `enable sccache` step, `./scripts/test.sh --full --locked -E 'kind(test)'`, no download step. `test-build` stays `needs: plan` with the 60-minute limit and stays in `merge-gate.needs` and `REQUIRED_JOBS` (17 jobs); `test` stays `needs: [plan, test-build]`.
2. **Wrapper.** `--archive-to <file>`: build only, no filterset: `cargo nextest archive --workspace --lib --locked --archive-file <file>` in lib mode (the default); in `--full` mode omit `--lib` (nextest's default target selection, never benches). `--from-archive <file>`: run `cargo nextest run --archive-file <file> --workspace-remap .` with the usual `--no-fail-fast` and the `--profile ci` auto-selection, drop `--lib` (build options are rejected with an archive) and drop `--locked`; every other argument, including a user `-E`, is passed through untouched. Remove all `kind(...)` filter construction from the archive path. Keep `bash -n` clean and keep the header usage text accurate.
3. **Pins.** The `test` job's download step uses the `release.yml` v8.0.1 SHA above; keep the existing `upload-artifact` SHA.
4. **Cache key.** Add the same workflow-level `env:` block as `ci.yml` (`CARGO_TERM_COLOR: always`, `RUST_BACKTRACE: 1`) to `cache-warm.yml`, with a comment in both files that the two blocks must stay identical because rust-cache hashes them. Open the pinned rust-cache action's `README`/`action.yml` (fetch the file at the pinned commit) and quote what it says about `env-vars`, `shared-key` and when the cache is saved (success only or also on failure); put the finding in the final report and, if `cache-on-failure` or `save-always` exists at the pinned version and is useful for `test-build`, say so (do not enable it silently).
5. **Docs.** `CLAUDE.md` bullet and the comments in `ci.yml` / `cache-warm.yml` / the composite action must say that only the lib `test` job consumes the archive and that `integration` builds in place. Leave `docs/dev-artifacts/prompts/remediation/ci/01-*.md` untouched.
6. **Nothing else changes.** No test selection changes: `test` must still run exactly the former `--lib` set (including proc-macro unit tests); the ubuntu doctest step stays. Keep `scripts/ci/tests/gates/merge/` and `scripts/ci/gates/` within 7 entries.

## Acceptance (observed by the orchestrator in CI)

The lib-test counts per OS in the `test` job `Summary` line equal the pre-change baseline; `integration` is byte-identical to master apart from nothing; `test-build` cold finishes inside 60 minutes; the second run restores the cache and is clearly faster.

## Local gates (only these)

`bash -n scripts/test.sh`; `python -I scripts/ci/check_layout.py`; `python -I -m py_compile` on touched Python files; read every YAML/shell change twice for the whole error class. Report real exit codes.

## Final report

Per required change: what you changed (file and lines), the exact wrapper command lines for both modes, what the pinned rust-cache says about env hashing and saving, and every remaining risk that only CI can answer.
