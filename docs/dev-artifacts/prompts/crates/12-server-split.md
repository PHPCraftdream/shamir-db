# Brief: split `shamir-server` into `shamir-server-security` and `shamir-sync`

Task #1829. Delegated stage; the orchestrator commits, you only edit files.

## Why

`shamir-server` is one crate of about 64,000 source lines in three large module
groups (`src/engine`, `src/security`, `src/sync`, plus `src/boot` and
`src/tests`). Any edit to it rebuilds and retests all of it. Splitting the two
lower groups into their own crates lets the CI selector (nearest `Cargo.toml`
plus workspace dependents) test only what an edit can reach. The estimated gain
from change history is about 4% fewer test-seconds per edit; the point of this
stage is that the gain is real and nothing else changes.

## What

Create two library crates in the `server` layer group, next to `shamir-server`:

* `crates/server/shamir-server-security` — the `security` module group
  (access tree, byte budget, HTTP login/REST/WS, observability, service, TLS,
  tokens, user directory, and whatever else the group contains today).
* `crates/server/shamir-sync` — the `sync` module group (connection,
  durability/backup/restore, framer, ports, replication, replication seed).

`shamir-server` keeps `engine`, `boot`, the binary, and depends on both new
crates. The dependency graph must be acyclic. The recorded coupling analysis
(references between the groups, counted by module) was:

| from → to | references |
|---|---|
| engine → security | 9 |
| engine → boot | 2 |
| security → engine | 1 |
| security → sync | 1 |
| sync → engine | 4 |
| sync → security | 1 |

Measure it yourself first (the flat `crate::<name>` paths re-exported from
`lib.rs` hide cross-group edges; resolve them). The edges that point against the
chosen layering must be cut, preferably by moving the small shared types or
traits they use down into the lower crate. Do not duplicate types, do not add a
fourth crate unless the analysis shows it is unavoidable, and report any edge you
could not cut honestly instead of papering over it. Pick the layering that makes
the fewest edges reverse and say what you chose and why.

## Observable acceptance

1. `shamir-server-security` and `shamir-sync` exist in `crates/server/`, are
   workspace members (the `crates/*/*` glob picks them up), use the same package
   version as the other workspace crates (do NOT raise any version),
   `publish = false`, `doctest = false`, `[lints] workspace = true`.
2. The dependency graph is acyclic and each new crate has only the dependencies
   its own code needs (taken from the current `shamir-server` manifest, not copied
   wholesale).
3. The public API of `shamir_server` is unchanged: every `shamir_server::<path>`
   used by other crates, benches, `src/tests` and `tests/` keeps resolving
   (re-export the moved modules from `lib.rs`). Other workspace crates
   (`shamir-e2e`, the napi binding, benches) need no source change.
4. No test is lost, duplicated, renamed without need, or newly `#[ignore]`d.
   Tests move with the code they test. The `nextest` overrides in
   `.config/nextest.toml` select tests by `package(shamir-server)` and test
   name; every override that matched a moved test must still match it (add the
   new package to the filter), otherwise a moved test silently loses its
   slow-timeout class. The `scripts/test.sh` scopes (`@server`, `@e2e`) must
   include the new packages. Check `scripts/ci/` selectors and tests, the layout
   baseline, `.rust-guard/*.toml`, `deny.toml` and the workspace table in
   `CLAUDE.md` / `AGENTS.md` for names or paths that need the new crates.
5. Layout limits hold in every touched folder: at most 7 direct entries per
   folder, at most 1000 lines per source file. Moving a folder that is already
   over a limit keeps its baseline entry under the new path (the baseline only
   ratchets down).
6. rust-guard rules hold: no new `#[allow]`, no local `use`, no inline test
   modules, `mod.rs` files contain only `mod`/`use`. Items that stay in
   `shamir-server` must not get wider visibility than they have now; items in
   moved files are new at their new path and may be `pub` where cross-crate use
   needs it, but prefer the narrowest visibility that compiles.
7. `cargo fmt`, `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings`
   are clean for `shamir-server-security`, `shamir-sync`, `shamir-server` and every
   workspace crate that depends on `shamir-server` (find them from the manifests),
   and `python -I scripts/ci/check_layout.py` passes (stage new files first so the
   checker, which sees tracked files only, sees them).

## Non-goals

No behaviour change, no refactor beyond what the cut edges require, no version
bump, no dependency upgrade, no change to the wire formats or the on-disk
formats, no new feature flags. Do not touch unrelated comments.

## Order of work

1. Measure the coupling and decide the layering; write the decision and the list
   of edges to cut at the top of the scratch report file.
2. Move files (plain filesystem moves; each moved test module keeps its
   `tests/` folder next to its code). Create the two manifests.
3. Cut the reverse edges; fix imports, privacy and `cfg` gating; restore the
   public paths with re-exports.
4. Update the selectors, overrides, scopes, baseline and docs named in item 4.
5. Read every touched file for the whole class of error (module paths after a
   move, imports, privacy, `cfg(test)` gating, unused items) before declaring done.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and
`python -I scripts/ci/check_layout.py`, plus read-only git and plain file moves.
No `cargo build/test/run/bench/doc`, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
