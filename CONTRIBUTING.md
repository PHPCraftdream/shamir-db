# Contributing to ShamirDB

## TL;DR — run this before every push

```bash
cargo fmt -p <crate> -- --check                         # 1. formatting
cargo clippy -p <crate> --all-targets -- -D warnings    # 2. lints (deny warnings; type-checks everything)
python -I scripts/ci/check_layout.py                    # 3. layout limits
```

All three must be **green**. **Tests, builds and benches run in the PR's
GitHub CI, not locally**: push to a branch, open a draft PR (fast tier runs
`./scripts/test.sh` for the affected crates) and run `gh pr ready` for the
full matrix (see `CLAUDE.md`'s "Compile / build / test policy"). Raw
`cargo test` is blocked by a perimeter guard in `.cargo/config.toml`;
`./scripts/test.sh` (the `cargo-nextest` wrapper) is what CI runs.

CI's jobs are `fmt`, `clippy`, `test`, `integration` (each an OS matrix
across Ubuntu/Windows/macOS), `cooldown` (a supply-chain dependency-freshness
check) and the layout check; CI's `test`/`integration` steps invoke
`./scripts/test.sh --locked` and `./scripts/test.sh --full --locked -E
'kind(test)'`.

CI-03b adds the compiled gates — `rustdoc`, `engine-features`,
`engine-features-fast`, `loom-model` discovery, `test-discovery` — plus
`node-binding` (the Node binding build/pack/typecheck consumer check), and the
aggregate `merge-gate` job, which is the ONE required check. A draft PR
always fails it with "draft PR: full matrix not run, not merge-ready";
`gh pr ready` runs the full matrix and turns it green. Details:
`CLAUDE.md`, "Compiled gates + merge gate (CI-03b)".

---

## Why the pinned toolchain keeps local checks equal to CI

The toolchain is **pinned** in [`rust-toolchain.toml`](rust-toolchain.toml)
(currently `1.93.0`), and CI installs the *same* version
(`dtolnay/rust-toolchain@1.93.0` in `ci.yml`).

This matters because `cargo clippy` is **version-specific**: every new `stable`
release ships new lints. When CI tracked a moving `@stable`, fresh lints would
fire on untouched code and redden the pipeline for no code reason, even though
local clippy (an older toolchain) was green. Pinning removes the drift — local
and CI lint identically.

`rustup` auto-selects the pinned version inside this repo, so just run the
commands above; you don't pass `+1.93.0`. If a component is missing:

```bash
rustup component add clippy rustfmt
```

---

## Discipline that keeps the gate cheap (from `CLAUDE.md`)

- **Don't blanket-format.** If `fmt --all -- --check` fails, do **not** run
  `cargo fmt --all` (it reformats the whole repo and pollutes your diff). Run
  `cargo fmt -p <crate>` on the crates you touched, or fix the few lines by
  hand. Keep the diff scoped to the task.
- **Pre-existing clippy lints in untouched code** don't ride inside a feature
  commit. Land them in a dedicated `chore(clippy): …` commit.
- **Style-only sweeps** (`cargo fmt --all`, bulk clippy auto-fix) live in their
  own `style:`/`chore:` commit, and the SHA goes into
  `.git-blame-ignore-revs` so `git blame` skips it.
- Local checks are scoped to the touched crates (`-p <crate>`); CI runs the
  `--workspace` gates.
- Tests follow the per-module `tests/` layout (see `CLAUDE.md` §"Test
  organisation"); JSON literals in tests are multi-line and indented.

---

## Bumping the pinned toolchain

Pinning trades currency for stability — bump deliberately, not by accident:

1. Edit [`rust-toolchain.toml`](rust-toolchain.toml): raise `channel` to the
   new version. `rustup` installs it automatically on the next `cargo` call.
2. Run the lint gate (the commands above, with `--workspace`). A newer
   clippy will likely surface new lints — fix them in a dedicated
   `chore(clippy): …` commit.
3. Bump the matching `dtolnay/rust-toolchain@<version>` refs in
   **all five jobs** (`fmt`, `clippy`, `test`, `integration`, `cooldown`) of
   `.github/workflows/ci.yml` to the same version.
4. Push and confirm CI is green (`gh run watch <id> --exit-status`).

Keep `rust-toolchain.toml` and the `ci.yml` refs in lock-step — that's the
whole point.

---

## What CI does NOT run

- **Benchmarks never execute in CI.** The `test` job runs lib tests only
  (`./scripts/test.sh --locked`) and the `integration` job selects only
  `[[test]]` integration targets (`./scripts/test.sh --full --locked -E
  'kind(test)'`) — neither ever touches `[[bench]]` targets.
  `clippy --all-targets` *compiles* benches (catches bitrot) but never runs
  them. Run benches manually: `cargo bench`.
- The Node.js napi e2e suite under `tests/e2e/` and the TS client e2e suite
  (`crates/shamir-client-ts/src/__tests__/**/e2e-*.test.ts`) are not wired into
  per-PR CI (both need a release `shamir-server` build; the napi one also
  needs the MSVC-only `shamir-client-node` binding). They run on the
  scheduled nightly workflow `.github/workflows/ts-e2e-nightly.yml` (cron +
  `workflow_dispatch`); run them manually per `tests/e2e/README.md`. The
  pure-TS unit tests for `shamir-client-ts` ARE gated per-PR (`ts-unit` job
  in `ci.yml`).
