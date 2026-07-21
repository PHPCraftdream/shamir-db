# Contributing to ShamirDB

## TL;DR — run this before every push (it mirrors the CI gate)

```bash
cargo fmt --all -- --check                              # 1. formatting
cargo clippy --workspace --all-targets -- -D warnings   # 2. lints (deny warnings)
./scripts/test.sh                                       # 3. unit tests (cargo tl)
./scripts/test.sh --full -E 'kind(test)'                # 4. integration tests (cargo t -E 'kind(test)')
```

All four must be **green**. Raw `cargo test` is blocked outright by a
perimeter guard in `.cargo/config.toml` — the wrapper above (backed by
`cargo-nextest`) is the only way past it; see `CLAUDE.md`'s "Centralised
test entry point" section.

These four local commands correspond to 4 of the 5 jobs in
`.github/workflows/ci.yml` (`fmt`, `clippy`, `test`, `integration` — each of
those four also runs as an OS matrix across Ubuntu/Windows/macOS in CI, and
CI's actual `test`/`integration` steps invoke `./scripts/test.sh --locked`
and `./scripts/test.sh --full --locked -E 'kind(test)'`). There's a fifth
job, `cooldown` (a supply-chain dependency-freshness check), which isn't
part of this local gate. If the four above pass locally, the matching CI
jobs will pass too. No surprises.

---

## Why green-locally now means green-in-CI

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
- Prefer `--workspace` flags over per-crate invocations for the final gate.
- Tests follow the per-module `tests/` layout (see `CLAUDE.md` §"Test
  organisation"); JSON literals in tests are multi-line and indented.

---

## Bumping the pinned toolchain

Pinning trades currency for stability — bump deliberately, not by accident:

1. Edit [`rust-toolchain.toml`](rust-toolchain.toml): raise `channel` to the
   new version. `rustup` installs it automatically on the next `cargo` call.
2. Run the full gate (the four commands above). A newer clippy will likely
   surface new lints — fix them in a dedicated `chore(clippy): …` commit.
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
  (`crates/shamir-client-ts/src/__tests__/e2e-*.test.ts`) are not wired into
  per-PR CI (both need a release `shamir-server` build; the napi one also
  needs the MSVC-only `shamir-client-node` binding). They run on the
  scheduled nightly workflow `.github/workflows/ts-e2e-nightly.yml` (cron +
  `workflow_dispatch`); run them manually per `tests/e2e/README.md`. The
  pure-TS unit tests for `shamir-client-ts` ARE gated per-PR (`ts-unit` job
  in `ci.yml`).
