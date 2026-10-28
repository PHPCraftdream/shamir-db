# P1 stage F14c — p1-engine-unwrap-inventory

## Goal

Card **RV-ENGINE-B789E7AC** (split from F14): the claim "every remaining production `unwrap`/`expect` in `shamir-engine` is structurally guarded" is unverified. Deliver an exhaustive inventory with a guard proof per site, fix the sites that are NOT guarded, and a CI ratchet so a new unguarded site cannot land silently.

Card text (from `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md`, R0 re-decision 2026-10-26):

### RV-ENGINE-B789E7AC — Every remaining production unwrap/expect is structurally guarded

- R0 verdict: `unverified`, risk `—`; evidence: crates/shamir-engine/src/query/auth/session.rs:184; crates/shamir-engine/src/table/read/dispatch/collect.rs:106; changed by: none
- R0 note (includes the fix site): OBSERVED: singleton extraction is guarded by len==1 and top-k unwraps by is_some checks, supporting these particular sites only. Missing premise: an exhaustive current production unwrap/expect inventory with a guard proof for every entry. Fix scope starts at crates/shamir-engine/src/query/auth/session.rs and crates/shamir-engine/src/table/read/dispatch/collect.rs, extending the inventory across production engine sources.


## Scope

- Production sources only: `crates/shamir-engine/src/**/*.rs` excluding `tests/` directories, `*_tests.rs`, `#[cfg(test)]`-only files, `benches/`, `examples/`. Calls counted: `.unwrap()`, `.expect(`, `.unwrap_unchecked(` (and `unreachable!`/`panic!` are OUT of scope).
- This branch is based on `fix/p1-engine-staged-probe-index` (F14b, PR #362, not yet merged) — do not touch the F14b files beyond what a site fix strictly requires.

## Deliverables

1. **Classify every site** (read the code around each one):
   - *guarded*: a structural invariant proves it cannot fail (e.g. `len()==1` checked just above, `Some` checked by `is_some()`, a constant regex/literal, a lock poisoning policy already documented). Leave the call; add nothing in code unless the guard is non-local — then add one short `// guard: <why>` comment on the line above.
   - *unguarded*: input-, I/O-, decode- or caller-dependent. Replace with `?` / a typed error / `ok_or(...)` in the narrowest layer that owns the error. No behaviour change on the success path. Each replacement gets a revert-sensitive test where an input can trigger it.
2. **Inventory file** `scripts/ci/gates/engine_unwrap_inventory.txt`: one line per remaining site `path:<fn-or-item>:<count>  # <guard reason>` keyed by enclosing item (NOT line numbers, so unrelated edits don't churn it). Sorted, deterministic.
3. **Ratchet checker** `scripts/ci/gates/engine_unwrap_ratchet.py` (Python stdlib only, `-I`-safe, like `scripts/ci/check_layout.py`): scans the scope above, groups counts by (path, enclosing item) using a simple brace/`fn`-aware scanner, strips `#[cfg(test)] mod` blocks and comments/strings, and fails if any (path, item) count exceeds the inventory or a new key appears; `--update` only lowers/removes entries (never adds), mirroring `check_layout.py --update-baseline`. Add unit tests for the scanner in the same style as the existing `scripts/ci` python tests (find them, e.g. `affected_crates` tests) and register them where those are run.
4. **Wire into CI**: add the checker next to the layout gate in BOTH the fast tier and the full tier of `.github/workflows/ci.yml` (instant step, no toolchain). Do not change any other job.
5. Final message: the counts (total sites, guarded, fixed), the list of fixed sites with file:line, and the exact CI lines added.

## Rules

- Layout: ≤7 entries per folder (`scripts/ci/gates/` — check its current count; if adding two files overflows it, put both in a new subfolder `scripts/ci/gates/unwrap/`), ≤1000 lines per file, tests in tests/ dirs with manifest-only mod.rs.
- Do not touch unrelated code or comments. No dependency changes.
- Local gates: `cargo fmt -p shamir-engine -- --check`, `cargo clippy -p shamir-engine --all-targets -- -D warnings` (CARGO_BUILD_JOBS=1), `python -I scripts/ci/check_layout.py`, `python -I scripts/ci/gates/<checker>.py` (must pass on the tree), and the checker's own python unit tests via `python -I -m unittest <path>` (allowed: they are instant stdlib scripts, not a project build/test run).
