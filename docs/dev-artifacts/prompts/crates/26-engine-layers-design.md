You work ONLY inside this git worktree: the current directory (a linked worktree of shamir-db on branch
engine-layers-1872). Read the root CLAUDE.md (binding). The brief below is the task; it is committed at
docs/dev-artifacts/prompts/crates/26-engine-layers-design.md. This is a DESIGN task: the deliverable is one
document; you change no source code.

# Brief: layering and crate-split design for `shamir-engine` (ENGINE-LAYERS-DESIGN)

## Why

`crates/engine/shamir-engine` has 62,573 production lines and 142,808 test lines (top-level modules: `table`
23k/50k, `tx` 16k/37k, `query` 11k/28k, `repo` 9k/10k, `db_instance`, `validator`). It is the most changed crate
(about 3,700 changed files since September) and every dependant (`shamir-db`, `shamir-server-security`,
`shamir-sync`, `shamir-server`) is rebuilt and retested when it changes, so CI pays for the whole chain. We want to
split it into several crates and to speed up CI, but its top-level modules form dependency cycles inside the crate.
Measurement (production code, files that refer upward against the candidate order `repo` < `table`,`validator` <
`query` < `tx` < `repo/instance` < `db_instance`): table->query 40 (table/read->query/read 11, table/read->
query/filter 10, table/write->query/write 4, table/write->query/TableResolver 4, table/table_manager->query/read 2,
->query/filter 2), query->tx 5 (query/batch -> CommitError, ack, release_pessimistic_locks, TxOutcome), table->
db_instance 3, table->tx 2, validator->query 2, repo->table 1. The composition root `RepoInstance` (`repo/instance`)
is referenced from outside by 40 files in `tx`, 7 in `query`, 6 in `table`, 11 in `repo`, 1 in `db_instance` (its
methods are spread over modules through `impl` blocks, see `.rust-guard/owned_impl.toml`). A separate task is
adding a `rust-guard layers` ratchet gate that reads a layer order from `.rust-guard/layers.toml`.

## What (deliverable: `docs/dev-artifacts/design/engine-layering.md`)

Read the code (re-measure my numbers with throwaway analysis; do not commit analysis scripts) and write:

1. **Final layer order and target crates.** Which modules become crates, what each owns, what each depends on; the
   group `engine` has 4 crates and may hold at most 7 (CLAUDE.md); `shamir-engine` may stay as a facade that
   re-exports so downstream package names and `-p` flags do not change. Consider the existing
   `shamir-engine-query` / `shamir-engine-validator` crates (what they already own, why they exist). Weigh the
   cut by the real purpose: the fewer lines a typical change forces to recompile and the fewer tests CI reruns,
   the better; say which cuts pay off and which do not (a cut that only moves lines does not pay).
2. **For every backward edge, the way to break it:** move a type down, put a trait in the lower layer that the
   upper layer implements, move a call up, split a type. Name concrete types and files. Hot paths must not gain
   dynamic dispatch where a generic or a plain move works (CLAUDE.md pillars).
3. **The `RepoInstance` knot:** how `tx`, `query` and `table` stop referring to the composition root: which narrow
   traits or handles (commit context, table registry, journal access ...) are owned by which lower layer and
   implemented by `RepoInstance` on top; what happens to the `impl RepoInstance` blocks scattered across modules
   and to `owned_impl.toml`.
4. **Refactoring waves R1/R2/R3** with EXPLICIT, non-overlapping file sets per wave so that three agents can work in
   parallel in separate worktrees, plus the order constraints between them (what must land first). Name the
   candidate waves: R1 = table->query edges, R2 = RepoInstance knot (split into sub-waves if it is too big for
   one), R3 = the small edges. State for each wave the acceptance (which backward pairs in the layers baseline reach
   zero) and the risks. Say whether the `rust-guard` conservation campaign (`.rust-guard/stages.toml`, delta file;
   see `tools/rust-guard/README.md`) is the right tool for the moves and how to use it.
5. **The final `layers.toml` content** (crate, ordered layers, expected baseline per pair) as a fenced block in
   the document, ready to paste.
6. **Risks and invariants:** public API and `lib.rs` re-exports, cargo features (engine features matrix, loom
   model, `fjall`), the rustdoc gate, benches and examples that live in the crate, the CI matrix (`scripts/ci/matrix/
   edges.toml` hidden edges), tests that move with their module, the order of landing.

## Observable acceptance

The document exists, is concrete (types and files, not generalities), contains a conflict-free partition of files
among waves, and is internally consistent with the numbers (if your re-measurement differs, say so and use yours).
Layout passes for the added file. Non-goals: any source change, creating crates, changing `Cargo.toml`.

## Constraints (binding)

Compilation, builds and tests run in CI, run by the orchestrator, never on this machine. Banned: `cargo test`,
`cargo nextest`, `./scripts/test.sh`, `cargo build/run/bench/doc`, `npm`/`tsc`/`vitest`, wasm builds, e2e and
integration harnesses, loom, any release or full-workspace build, any command whose purpose is to build the project
or run its tests. Allowed locally, scoped to the crates you touched and nothing else:
`cargo fmt -p <crate>` (and `-- --check`), `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git (`status`, `diff`, `log`). Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --` with `-j 2`. A change you cannot verify here is verified by the orchestrator's CI
run: say in your final report exactly what must be observed there. If you believe you need a build or test run,
say so in the final report instead of running it.

Repo rules (read the root CLAUDE.md, it is binding): at most 7 entries per folder, at most 1000 lines per source
file, `mod.rs` re-exports only, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no visibility
widening, `shamir_collections::THasher` for hash maps, comments terse. Never write private machine paths, user
names, or the names of any CI infrastructure other than the workflow files into repository files. Never raise
versions. Do not touch code unrelated to the task.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check.)

Your last message must be a final report: what changed (files), what you verified locally (fmt/clippy/layout only),
what is unverified and must be observed in CI, anything you decided and why. Do not end on a tool call.
