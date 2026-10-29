# 242 — connect-one-test-binary (CI speed #1549, stage A1: shamir-connect 12 integration binaries → 1)

Context (@oh ruling 2026-10-29). Every `[[test]]` target is compiled and LINKED separately; the CI `integration` job (`./scripts/test.sh --full --locked -E 'kind(test)'`, ubuntu + windows) links ~97 test binaries and link time dominates. nextest runs every test in its own process, so sharing one binary does not change isolation (the repo's only sanctioned runner is nextest; raw `cargo test` is blocked by the `.cargo/config.toml` runner guard). `crates/shamir-connect` declares 12 explicit `[[test]]` targets (146 tests at origin/master f9dc5788). This stage collapses them into ONE binary named `integration` by `mod`-aggregation, without editing any test body. Sibling stage 243 does the same for `shamir-server` (disjoint files; may run in parallel; both land in one PR).

Re-verified facts (f9dc5788):
- `tests/` already has 7 entries (admin, audit, auth, identity, lifecycle, resume, session) — no new top-level file or folder is allowed. The binary root therefore goes to `tests/auth/main.rs` (auth: 3 → 4 entries).
- No test file uses `crate::` paths, `#[path]`, inner `#![...]` attributes, statics, env mutation or self-respawn. `tests/resume/main.rs` is a pure manifest (`mod authz_rotation; mod helpers; mod resume_flow;`); its children use `super::helpers` (survives nesting unchanged).
- `.config/nextest.toml` override `package(shamir-connect) and test(/.*scram.*/)` matches by test NAME; none of the new module prefixes below contains `scram`, so the matched set is unchanged. No workflow/script references a shamir-connect binary id.
- rust-guard `modules` resolves a `#[path]`-mounted NON-`mod.rs` file as non-mod-rs (children looked up in `<dir>/<stem>/`), unlike rustc. So a `#[path]` must never point at a `main.rs` that declares child modules — hence the `main.rs → mod.rs` rename below.

Read first: `crates/shamir-connect/Cargo.toml` (the `[[test]]` block at the end), `tests/resume/main.rs`, `tools/rust-guard/README.md` ("modules" section), `CLAUDE.md` (layout limits, rust-guard gates).

## Changes
1. Rename (plain filesystem move, content unchanged) `tests/resume/main.rs` → `tests/resume/mod.rs`.
2. New `crates/shamir-connect/tests/auth/main.rs` — the binary root. Content: a short `//!` doc ("single integration binary for all shamir-connect topic folders; one module per former test target, named after it") followed ONLY by these module declarations (module name = former `[[test]]` name, so every test keeps its old path with one extra leading segment):
   ```rust
   #[path = "../admin/integration_admin.rs"] mod integration_admin;
   #[path = "../lifecycle/integration_bootstrap.rs"] mod integration_bootstrap;
   #[path = "../lifecycle/integration_changepw.rs"] mod integration_changepw;
   mod integration_full_auth;
   #[path = "../audit/integration_log_redaction.rs"] mod integration_log_redaction;
   #[path = "../resume/mod.rs"] mod integration_resume;
   #[path = "../lifecycle/integration_rotation.rs"] mod integration_rotation;
   #[path = "../session/integration_session.rs"] mod integration_session;
   #[path = "../identity/integration_username_precis.rs"] mod integration_username_precis;
   mod rv_client_f20c09bf_rotation_recovery;
   #[path = "../lifecycle/rv_feature_fed6910e_client_only_surface.rs"] mod rv_feature_fed6910e_client_only;
   #[path = "rv_handshake_0fc58d93_tofu_pin_after_identity_verify.rs"] mod rv_handshake_0fc58d93_tofu_pin;
   ```
   (one attribute + `mod` per line pair as rustfmt formats it; keep alphabetical order).
3. `Cargo.toml`: replace all 12 `[[test]]` entries with exactly one:
   ```toml
   [[test]]
   name = "integration"
   path = "tests/auth/main.rs"
   ```
   and update the comment above it: topical folders are mounted as modules of one binary (link-time saving), cargo autodiscovery would otherwise infer `tests/<dir>/main.rs` targets — the explicit entry with the same path suppresses the inferred `auth` target.
4. Nothing else: no test body, import, helper or attribute changes. If a file fails to compile as a module (e.g. a name clash surfaced by clippy), STOP and report the exact error instead of rewriting test logic.

## Tests (CI runs them — never locally)
No new tests. Identity mapping (report it): old `(shamir-connect::<target>, <name>)` → new `(shamir-connect::integration, <target>::<name>)`; for `integration_resume` the module chain is `integration_resume::<resume_flow|authz_rotation>::<name>` as before plus the prefix.

Acceptance a reviewer checks by reading:
- `grep -c '^\[\[test\]\]' crates/shamir-connect/Cargo.toml` → `1`.
- `grep -rE '^\s*#\[(tokio::)?test' crates/shamir-connect/tests | wc -l` → `146` (unchanged).
- every `.rs` under `crates/shamir-connect/tests/` except the new root is reachable from `tests/auth/main.rs` (12 `mod` lines + resume's 3 children); `find crates/shamir-connect/tests -name main.rs` → only `tests/auth/main.rs`.
- CI (full tier, `integration` job log): nextest's `Starting N tests across M binaries` — N unchanged vs the last master run on the same OS, M lower by 11.

## Rules
- Touch only `crates/shamir-connect/Cargo.toml` and `crates/shamir-connect/tests/**`.
- No rust-guard campaign and no delta file (decided in E_PLAN: test-discovery lists only `-p shamir-engine`; the whole-repo `modules` gate is the lost-file guard here). Do not edit `.rust-guard/**`.
- Layout: ≤ 7 entries per folder, ≤ 1000 lines per file (verify `tests/auth` = 4, `tests/resume` = 4). `mod.rs` re-exports/`mod` lines only. Imports at file top. No new `#[allow]`. Comments laconic.
- File moves are plain filesystem moves; ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm` / `mv`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

## Local gates (only these; never --all-features)
- `cargo fmt -p shamir-connect -- --check`
- `cargo clippy -p shamir-connect --all-targets -- -D warnings`
- `python -I scripts/ci/check_layout.py`
Report every NEW/RENAMED file path, the before/after test count and the real exit codes.
