# Contributing to ShamirDB

## TL;DR — run this before every push (it IS the CI gate)

```bash
cargo fmt --all -- --check                              # 1. formatting
cargo clippy --workspace --all-targets -- -D warnings   # 2. lints (deny warnings)
cargo test  --workspace --lib                           # 3. unit tests
cargo test  --workspace --test '*'                      # 4. integration tests
```

All four must be **green**. These are the exact four jobs in
`.github/workflows/ci.yml` — if they pass locally, CI passes. No surprises.

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
   **all four jobs** of `.github/workflows/ci.yml` to the same version.
4. Push and confirm CI is green (`gh run watch <id> --exit-status`).

Keep `rust-toolchain.toml` and the `ci.yml` refs in lock-step — that's the
whole point.

---

## What CI does NOT run

- **Benchmarks never execute in CI.** The test jobs use `--lib` and
  `--test '*'`, which select unit and `[[test]]` integration targets only —
  not `[[bench]]`. `clippy --all-targets` *compiles* benches (catches bitrot)
  but never runs them. Run benches manually: `cargo bench`.
- The Node.js e2e suite under `tests/e2e/` is not wired into per-PR CI
  (needs `npm install` + a release build + the MSVC-only `shamir-client-node`
  napi binding). Run it manually per `tests/e2e/README.md`.
