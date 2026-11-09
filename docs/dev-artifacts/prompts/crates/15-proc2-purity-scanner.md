# Brief: close the bypasses of the `shamir-lang` purity scanner

## Why

`shamir-lang` is the runtime-free compiler crate: no `std::process`, `fs`, `net`,
`env`, `thread` in its production code (milestone 8). The decision is enforced by a
source-scan test, `crates/wasm/shamir-lang/src/api/tests/purity_tests.rs`. Review
found it can be bypassed: `use std as s; s::process::exit(1);`,
`extern crate std as x;`, renamed imports and re-exports of the forbidden modules
are not seen (only fully spelled paths and `Command::new` are caught).

## What

Strengthen the guard so those bypasses fail the test. Prefer a token/AST based scan of
the production sources of the crate (`crates/wasm/shamir-lang/src`, excluding the
`tests` folders and `testsupport` that are test-only) over more substring matches.
Constraints on the implementation:

* the crate's production dependencies stay exactly `shamir-wasm-admission`,
  `wasm-encoder` and `thiserror` (the guard itself pins that); a scanner may use
  `syn` ONLY if it is already a dependency available to the test target without a new
  entry; otherwise write a small tokenizer in the test support (strings, comments and
  raw strings must not produce false positives or negatives);
* a separate `clippy.toml` inside the crate is NOT an option (it replaces the
  workspace configuration);
* a rust-guard rule is also out of scope for this brief.

Forbidden after the change (each is covered by a negative test that feeds a synthetic
source to the scanner function, so the guard is proven to bite):

1. any path rooted at `std`/`core`/`alloc` through an alias: `use std as s;`,
   `use std::{self as s}`, `extern crate std as x;`, `extern crate` of anything, `use
   ::std as s`;
2. the forbidden modules (`process`, `fs`, `net`, `env`, `thread`) reached by any
   spelling: full path, `use std::process;` then `process::exit`, grouped imports
   (`use std::{process, fs}`), renamed (`use std::process as p`), glob (`use
   std::process::*`), qualified through `::std::`;
3. the macros that wrap them (`std::env!`, `env!`/`option_env!` in production code).

Allowed: the existing legitimate uses (keep the existing positive tests green; list
in the final message any production line the stronger scan newly flags and why it is a
true positive; if a real production use is flagged, report it instead of weakening
the scanner).

Also keep the dependency pin test and the existing negative tests; update the
doc comment of the guard. Do NOT edit docs outside the crate (the orchestrator updates
the design/ADR text).

## Observable acceptance

1. The scanner catches every bypass above (one negative test per bypass family, with
   the exact synthetic source in a multi-line literal).
2. `cargo fmt -p shamir-lang -- --check`, `cargo check -p shamir-lang --all-targets`,
   `cargo clippy -p shamir-lang --all-targets -- -D warnings` clean.
3. `python -I scripts/ci/check_layout.py` passes (folder at most 7 entries, file at
   most 1000 lines: put the scanner in its own file under `src/api/tests/` and wire it
   in that folder's `mod.rs`).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local
   `use`, no new `#[allow]`, no visibility widening.

## Non-goals

No production code change in `shamir-lang`, no new dependency, no version change.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and
`python -I scripts/ci/check_layout.py`, plus read-only git. Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --`. No `cargo build/test/run/bench/doc`, no
nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
