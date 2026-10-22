# DEC-3 Q2-b — `shamir-client-node` layout (`js/` + `native/`) and the `node-binding` CI job

## Context

PR Q2-a (merged) made `crates/shamir-client-node` (napi-rs `cdylib`) a regular
workspace member. This is PR **Q2-b** of the owner-approved ruling (DEC-3, Q2):
split the crate root into `js/` (handwritten) and `native/` (napi-generated),
pin the npm toolchain, add a full-tier CI job `node-binding` and rewrite the
errors e2e test so it asserts typed errors through the public package. Nothing
here can be run to completion locally (no npm, no node, no napi build); CI is
the acceptance. The orchestrator generates `package-lock.json` after your
edits (one npm resolution-only command it is allowed to run); you must NOT run
npm, node, tsc or napi.

Current state to read first: `crates/shamir-client-node/{Cargo.toml,build.rs,package.json,index.js,index.d.ts,wrapper.js,wrapper.d.ts,proof-typed-errors.js,src/lib.rs}`,
`tests/e2e/{package.json,.npmrc,README.md,e2e.test.js,helpers/*,tests/09-errors.test.js}`,
`.github/workflows/{ci.yml,release.yml,ts-e2e-nightly.yml}`,
`scripts/ci/gates/merge_gate_check.py` (the `REQUIRED_JOBS` list),
`scripts/ci/layout_baseline.txt`, `.gitignore`.

## Scope — exact steps

1. **Moves** (create the new file with your file tool, delete the old one with
   your file tool; never `git mv`):
   `wrapper.js` -> `js/wrapper.js`; `wrapper.d.ts` -> `js/wrapper.d.ts`;
   `index.js` -> `native/index.js`; `index.d.ts` -> `native/index.d.ts`; delete
   `proof-typed-errors.js`. End-state crate root (7 tracked entries):
   `Cargo.toml build.rs src/ package.json package-lock.json js/ native/`
   (the lockfile is added by the orchestrator). `native/shamir-client.<triple>.node`
   is gitignored (the `.gitignore` line already points at `native/*.node`).
2. In `js/wrapper.js` change `require('./index.js')` to `require('../native/index.js')`
   (and any other relative path to a moved file; grep every `require(` /
   `import` / `from` in both wrapper files and `src/`/`build.rs` for the names).
3. `crates/shamir-client-node/package.json`:
   - `"main": "js/wrapper.js"`, `"types": "js/wrapper.d.ts"`
   - `"files": ["js/wrapper.js","js/wrapper.d.ts","native/index.js","native/index.d.ts","native/shamir-client.*.node"]`
   - `"build": "napi build --platform --release --output-dir native --js index.js --dts index.d.ts"`
   - `"build:debug": "napi build --platform --output-dir native --js index.js --dts index.d.ts"`
   - `@napi-rs/cli` and `typescript` pinned EXACTLY (no caret) to the newest
     releases that are at least 30 days old today. Find the versions and publish
     dates with plain HTTP reads of the registry (for example
     `curl -s https://registry.npmjs.org/<pkg>` and read the `time` object); do
     not run npm. State the chosen versions and dates in the final message.
     `@msgpack/msgpack` stays as it is unless it is a floating range that the
     30-day rule forbids (then pin the newest release at least 30 days old).
   - remove `x86_64-pc-windows-gnu` from `napi.targets`.
   If napi v3 cannot emit into `--output-dir` with exactly these flags (check the
   installed CLI docs only if they are in the repo; do not guess), keep the
   flags as specified and say so in the final message.
4. `tests/e2e/package.json`: `build:binding` becomes
   `cd ../../crates/shamir-client-node && npm ci && npm run build`.
5. Rewrite `tests/e2e/tests/09-errors.test.js`: delete the obsolete NOTE and
   the message parsing. Through the public `shamir-client` package assert
   `err instanceof ShamirDbError`, `err.code === 'unknown_db'`,
   `err.retryable === false`; one retryable code where the suite already has a
   deterministic trigger (`tx_conflict` or `read_only_replica`), otherwise the
   exported `isRetryableCode` truth table against the real wrapper; and a
   success-path call. Queries go through the typed builder as in neighbouring
   tests (no raw JSON assembly).
6. New full-tier job `node-binding` in `.github/workflows/ci.yml`
   (`windows-latest`, unconditional on ready PRs/master/dispatch, i.e. same `if`
   shape as the other full-tier jobs, added to the `merge-gate` job `needs:` and
   to `REQUIRED_JOBS` in `scripts/ci/gates/merge_gate_check.py`, plus whatever
   unit test inventories that list (grep `REQUIRED_JOBS` and the gate inventory
   test and update every place, keeping them consistent):
   setup-node 22 with npm cache keyed on the binding lockfile, Rust 1.95.0
   (follow how the other jobs install the toolchain; actions pinned by full SHA
   exactly like the existing steps); `npm ci`, then `npm run build:debug`;
   `git diff --exit-code -- native/index.js native/index.d.ts` (drift in the
   generated files); `npm pack` into `$RUNNER_TEMP`, install the tarball into a
   clean consumer directory; `node -e` requiring `shamir-client` and checking
   that `ShamirClient` and `ShamirDbError` are functions; `tsc --noEmit` on
   `tests/e2e/packaging/consumer.ts` with its `tsconfig.json` (create both files;
   `tests/e2e` reaches 7 entries). Draft PRs skip it like the other full-tier jobs.
7. Update the explanatory comments in `ts-e2e-nightly.yml` (the toolchain-file
   rationale: `rust-toolchain.toml` no longer exists; commands stay) and the
   path comments in `release.yml` and `ci.yml` that still say the binding is
   excluded from the workspace or name `proof-typed-errors.js` / old paths. No
   npm publish pipeline.
8. Delete the `dir crates/shamir-client-node` line from
   `scripts/ci/layout_baseline.txt` ONLY IF you can show by `ls -A` that the
   crate root has at most 7 entries after the lockfile is added (the
   orchestrator runs `--update-baseline` after tracking the files; do not edit
   the baseline yourself, just verify the count by hand and report it).
9. Docs: any README/CLAUDE.md/AGENTS.md line that names the old paths or the old
   build command (`napi build --platform`) is updated; do not rewrite unrelated text.

## Do not

- Do not touch Rust sources under `src/` or `build.rs` except for a path that the
  moves invalidate (report it if so). Do not change the crate's public API.
- Do not edit `.rust-guard/**`, other crates, or the compile/test policy text.
- Do not bump the project version or any dependency other than the exact pins above.
- Layout limits: at most 7 entries per folder, at most 1000 lines per file.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index
  (including `git add` / `git commit`). Only edit files; the orchestrator
  commits. Delete a file with your file tool.
- Compile / build / test policy: see the policy text at the top of this run
  (verbatim). Allowed locally: `cargo check -p shamir-client-node --all-targets`
  (type-check only; the crate is a cdylib member), `cargo clippy -p shamir-client-node --all-targets -- -D warnings`,
  `cargo fmt -p shamir-client-node -- --check`, `python -I scripts/ci/check_layout.py`,
  `python -I scripts/ci/gates/merge_gate_check.py` self-tests only if the repo
  documents a way to run them without a test runner (otherwise read the code and
  say they are CI-only), plain `curl` GET of registry metadata. NOT allowed:
  npm, npx, node, tsc, napi, cargo build/test/bench/doc, yaml linters that
  download tools. One cargo process at a time, `CARGO_BUILD_JOBS=4`.
- NEVER stop processes by image name or pattern (`Stop-Process -Name`,
  `taskkill /IM`, `pkill`); the machine runs other sessions' cargo.
- Never use the shell builtin `xargs`. Capture exit codes without pipes. Logs
  under `.scratch/`. No machine-specific absolute paths in any file.
- Before finishing, one verifier child agent (the whole policy pasted verbatim)
  reads every changed file in full and checks every numbered step against the
  diff, with special attention to: every path that mentions a moved file, the
  workflow YAML (indentation, `needs`, `if`, action SHAs), and the
  `REQUIRED_JOBS` / inventory consistency. Start at most one child at a time.

## Final message

1. Per step 1-9: done / not done with file:line.
2. The exact pinned versions of `@napi-rs/cli`, `typescript` (and msgpack if
   touched) with their publish dates.
3. Every place `node-binding` was added for the merge gate.
4. Real exit codes of every allowed gate you ran; `ls -A` of the crate root and
   of `tests/e2e/`.
5. Anything you could not verify (all CI-only: napi flags, the job itself).
