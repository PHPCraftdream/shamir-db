# Brief: FILE-HEADROOM-b — move-only split of the six source files that sit at the 1000-line limit, so the next release steps have room

## Why

Six production files are at or next to the hard layout limit (measured on `origin/master` `47002fa44`): `crates/client/shamir-client/src/client/api.rs` 993, `crates/server/shamir-server/src/engine/server/server_launcher/launch.rs` 996, `crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs` 992, `crates/net/shamir-connect/src/server/state/session.rs` 1000, `crates/storage/shamir-tx/src/tx/tx_context.rs` 1000, `crates/engine/shamir-engine/src/query/batch/exec/batch_execute.rs` 1000. The next release steps (R5-3 status and point reads, R4b-3 cancel of interactive transactions and cursors, R4b-4 shape statistics, R3-2b online export, the Argon2 gate, the backup-schedule read operation) each have to add lines to some of them, and the layout ratchet (`python -I scripts/ci/check_layout.py`) refuses a file that grows past 1000. This task makes room first, with NO behaviour change.

## What (this step only)

Split each of the six files along its natural seams so that every resulting file is at most 850 lines (target; the hard limit stays 1000). It is a pure move:

1. Bodies are moved verbatim (same code, same doc comments); no renames, no signature change, no logic change, no new dependency, no changed default.
2. Every public path stays valid: re-export from the original module so no other file in the workspace needs an import change (grep the workspace for the paths before deciding; a downstream `use` that has to change is a sign the seam is wrong).
3. One primary item per new file, a `mod.rs` holds declarations and re-exports only (rust-guard `structure` gate: no logic, no local `use`, no inline `#[cfg(test)]` module in it), tests stay in their `tests/` folders and keep compiling (`cargo check --all-targets` covers them), at most 7 direct entries per folder and at most 1000 lines per file.
4. Seams: read each file and choose coherent groups (for example a block of impl methods that belongs to one concern, a group of free helper functions with their private types, a block of constants and small types). For `batch_op.rs` use the SAME seam that the unmerged branch `rel-r3-2b` already used, read-only: `git show rel-r3-2b:crates/query/shamir-query-types/src/ops/batch/core/batch_op_classify.rs` and `git diff 8a889d503 rel-r3-2b -- crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs` (that branch reaches 638 lines; take ONLY the move, none of its new feature code, and keep every existing item). For `api.rs`: the folder `crates/client/shamir-client/src/client/` already holds 7 entries (`api.rs`, `cursor_stream.rs`, `handshake/`, `mod.rs`, `session/`, `subscription/`, `transport/`), so a new sibling file is an eighth entry: turn `api.rs` into a folder `api/` (declarations-only `mod.rs` plus topical files) or move the groups into an existing topical subfolder, and say which you chose and why.

## The rust-guard trap (read before choosing seams)

The `source-integrity` CI job runs `tools/rust-guard`; its `structure` gate rejects, on the changed files of the PR: logic in a `mod.rs`, inline `#[cfg(test)]` modules, local `use`, an `impl` block outside the owner modules listed in `.rust-guard/owned_impl.toml`, a visibility WIDENING against the merge base (for example a private item becoming `pub(super)` or `pub(crate)` because the move put its user in a sibling file) and any new `#[allow]`. Read `tools/rust-guard/README.md` (the `structure` section and "Delta file") and `.rust-guard/owned_impl.toml` first, then read the three landed precedents of exactly this kind of task: `git show --stat d1fac363f`, `git show --stat 82ac99f7c`, `git show --stat 2962e5ca9` (PRs #562, #559, #564) and how they handled visibility and `impl` ownership (the last one needed a campaign entry in `.rust-guard/stages.toml` plus a delta file; read how).

Plan so that nothing widens: group an item together with ALL of its private users, so each moved group depends on the rest only through items that are already `pub` or `pub(crate)`; a child module may use its parent's private items through `super::`, so a group can also be moved into a child module of the file it came from without widening anything. Every `impl` block you add in a new file needs an owner entry in `.rust-guard/owned_impl.toml` (exact path and name, an `owner`, a `reason`): prefer keeping an `impl` where its type lives or add the entry. If some widening is truly unavoidable, do NOT use `#[allow]`: list each one (file, item, old and new visibility, the reason) in the final report; the orchestrator then opens a campaign for it (stages entry plus delta, like #564) from the CI findings. Do not create a campaign yourself.

## Do not

Change behaviour, rename, reorder public API, touch tests except where a path moves, bump a version, add a dependency or an `#[allow]`, touch anything outside these six files, their new siblings, the manifests and re-exports they need, `.rust-guard/owned_impl.toml` and the layout baseline. If a file's line count is in `scripts/ci/layout_baseline.txt`, lower or remove the entry accordingly with `python -I scripts/ci/check_layout.py --update-baseline` (it only lowers). Do not touch `crates/server/shamir-server/shell/` or `crates/server/shamir-server/tests/admin/shell/` (another executor works there).

## Cargo (one batch, minimal)

Run cargo at most three times in total, all crates in the same invocation: `cargo fmt -p shamir-client -p shamir-server -p shamir-query-types -p shamir-connect -p shamir-tx -p shamir-engine` (with `CARGO_BUILD_JOBS=1`), then ONE `cargo check` and ONE `cargo clippy -- -D warnings` with `-p` for those six crates and `--all-targets` (workspace dependents are unaffected because no public path changes; if a path does change you made a mistake: fix the seam). The machine is shared and `shamir-engine` and `shamir-server` are slow to check.

## Observable acceptance

`python -I scripts/ci/check_layout.py` is clean with every touched file at most 850 lines (report each file's old and new line count and the new files with their counts and the folder entry counts); `git diff -M --stat` shows the moves as renames or copies with no body edit; `cargo fmt --check`, `cargo check` and `cargo clippy -D warnings` on the six crates exit 0; no public path changed (state how you verified: the grep for every moved public item). The final report lists the seams chosen per file, every visibility widening that could not be avoided and every `owned_impl.toml` entry added, and anything not done.

## Layout and fixture lessons

Count every folder you add to with `ls` (many are at 7 entries: `python -I scripts/ci/check_layout.py` counts only tracked files, recheck after staging); at most 1000 lines per file, one primary item per file, `mod.rs` re-exports only (the rust-guard structure gate allows nothing else in a `mod.rs`: shared helpers go in a named file), tests in `tests/` folders, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`. Persistent stores, every `TempDir` alive for the whole test, close handles and wait for the server task to finish before reopening a directory (Windows holds Fjall locks until the dropped engine's background handles finish: reopen with a bounded retry on `Locked` only, like `init_waiting_for_locks` in the function-family crash tests), handshakes not sleeps, every await bounded by a generous timeout (at least 120 s: a hang detector, the Windows runner is up to 10x slower), a batch is at most 50 queries, per-session request rate limit 500 req/s (never poll in a tight loop: pace status polls at 20 ms or more), set `argon2_concurrent_max` and `auth_init_rate_per_second` to 64 in fixtures that open several sessions, debug-test stacks are small (call concrete handlers through `Box::pin`, run deep tests on a roomy thread, `on_big_stack`), assert on invariants and sets, `shamir-test-diag` marks for flake-prone tests.

## Rules that bind the change

* Machine memory: at most ONE cargo command at a time machine-wide (other executors run: if a cargo run is already in progress wait for its notification, never poll), ALWAYS as `bash /d/system_artefact/orch/cg.sh env RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1 > <logfile OUTSIDE the repository> 2>&1` (the wrapper serializes cargo across executors; never `-j 2`; never piped through `tail`/`head`; `cargo fmt` rejects `-j`: run it with `CARGO_BUILD_JOBS=1` under the same wrapper); every worker you spawn must be told the same, and no worker may run cargo at all: only you, the top executor, run `check`/`clippy`/`fmt` under these rules (workers edit files and read code only).
* Leave no scratch files in the tree and none outside it (edit only paths inside this worktree).
* Wrap-up: the hard limit is 150 minutes. At about 105 minutes (70%) stop adding scope, bring the change to a compiling, formatted, clippy-clean state and write the final report (what is done, what is missing, what each check printed); a run that hits the limit without a report is lost work.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits. (For file moves use a plain filesystem move and say so in the report;
the orchestrator stages them so git detects the renames.)

