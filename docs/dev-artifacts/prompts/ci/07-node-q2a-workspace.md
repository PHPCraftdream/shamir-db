# DEC-3 Q2-a — `shamir-client-node` becomes a workspace member

## Context

`crates/shamir-client-node` (napi-rs Node binding, `cdylib`) is excluded from
the root workspace and carries its own `rust-toolchain.toml`, an untracked
`Cargo.lock`, an untracked `package-lock.json` and its own
`[profile.release]`. The owner-approved ruling (DEC-3, Q2) makes it a regular
workspace member so that the root `Cargo.lock`, `cargo-deny`, the admission
checker and the pinned root toolchain cover it. This is PR **Q2-a** only
(workspace membership). The file moves into `js/` and `native/`, the npm
lockfile and the `node-binding` CI job are PR Q2-b and are NOT part of this
task. Nothing here can be run to completion locally; CI is the acceptance.

## Scope — exact steps

1. Root `Cargo.toml`: remove `crates/shamir-client-node` from `exclude` (keep
   `crates/shamir-client-ts` and `tools/rust-guard`), and fix the explanatory
   comment above `exclude` accordingly.
2. `crates/shamir-client-node/Cargo.toml`: `[lib] crate-type = ["cdylib"]`,
   `test = false`, `doctest = false`. Delete the crate's own
   `[profile.release]` table (Cargo ignores profiles in non-root members).
3. Root `Cargo.toml`: add a per-package override for the binding
   (`[profile.release.package.shamir-client-node]` with `opt-level = "z"`,
   `codegen-units = 1`, `strip = true`; fat LTO is already set at the root).
   Update the nearby comment that says the binding has its own profile.
4. Delete `crates/shamir-client-node/rust-toolchain.toml`.
5. `.gitignore`: delete the lines `crates/shamir-client-node/Cargo.lock`,
   `crates/shamir-client-node/target/` and
   `crates/shamir-client-node/package-lock.json`; change
   `crates/shamir-client-node/*.node` to `crates/shamir-client-node/native/*.node`.
6. The binding's dependencies enter the root `Cargo.lock`. Let
   `cargo check -p shamir-client-node` write the lockfile update, keep the
   diff minimal (no unrelated version bumps), then run the CI helpers that gate
   dependency changes, locally: `python -I scripts/ci/gates/<admission checker>`
   (find it with `ls scripts/ci/gates/`; run it exactly as `ci.yml` does) and
   `cargo metadata --locked --format-version 1 > /dev/null`. If the checker or
   `cargo deny` policy (`deny.toml`) rejects a new crate (license, 30-day
   cooldown, source), do NOT weaken the policy: report the exact crate/version
   and rule in the final message and add nothing to the allowlists.
7. `CLAUDE.md` "Workspace" section: the binding is now a member; keep the
   crate count and list consistent with `Cargo.toml` `members`.
8. `python -I scripts/ci/check_layout.py --update-baseline` is run by the
   orchestrator after the files are tracked; you do not edit
   `scripts/ci/layout_baseline.txt` (the `crates/shamir-client-node` entry
   stays at 10 until PR Q2-b moves files; the ruling's "baseline 9" refers to
   the deleted `rust-toolchain.toml`: verify by `ls -A` that the crate root now
   has 9 entries and say so in the final message).
9. Do NOT touch `scripts/test.sh` unless you can show from the code that the
   cdylib with `test = false` would be built as a test harness. The documented
   fallback (adding `--exclude shamir-client-node` to the workspace selection)
   is applied by the orchestrator only if CI shows the problem.

## Do not

- Do not move or edit `index.js`, `index.d.ts`, `wrapper.*`, `package.json`,
  `proof-typed-errors.js`, `build.rs`, or anything under `src/` (Q2-b).
- Do not edit `.github/**`, `.rust-guard/**`, or any other crate.
- Do not bump versions of the project or of any dependency beyond what the
  lockfile needs to add the binding's dependency tree.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator
  commits. Delete a file with your file tool.
- Compile / build / test policy: see the policy text at the top of this run
  (verbatim). Allowed locally: `cargo check -p shamir-client-node --all-targets`
  (this also proves the cdylib still type-checks as a member),
  `cargo clippy -p shamir-client-node --all-targets -- -D warnings`,
  `cargo fmt -p shamir-client-node -- --check`, `cargo metadata --locked`, the
  admission checker script and `python -I scripts/ci/check_layout.py`. One cargo
  process at a time, `CARGO_BUILD_JOBS=4`. No `cargo build`, no `cargo test`,
  no `npm`.
- NEVER stop processes by image name or pattern (`Stop-Process -Name`,
  `taskkill /IM`, `pkill`); the machine runs other sessions' cargo.
- Never use the shell builtin `xargs`. Capture exit codes without pipes. Logs
  under `.scratch/`. No machine-specific absolute paths in any file.
- Before finishing, one verifier child agent (the whole policy pasted verbatim)
  reads every changed file in full and checks each numbered step above against
  the diff; start at most one child at a time.

## Final message

1. Per step 1-9: done / not done and the file:line.
2. The real exit codes of every allowed gate you ran.
3. `Cargo.lock` diff summary: number of crates added, the list of crates that
   are new to the whole graph (not already used by another member).
4. Any admission-checker / deny finding with crate, version, rule.
5. Anything you could not verify (CI-only: cdylib linking on Windows/macOS,
   nextest handling of `test = false`).
