# TOKEN-T6 revision 2 - integrate upstream master (AUTH1 login landed) and fix the native e2e commit

Base: `t6-ws-token-channel.md`, `t6-revision-1.md`, audits in `docs/dev-artifacts/audits/2026-11-02-token-t6/` (DESIGN.md, REVIEW-1, REVIEW-2). Every rule of those briefs applies verbatim: NO LOCAL COMPILATION, git-mutation prohibition, layout limits, clippy classes, no machine-local paths, no `#[allow]`, no local `use`.

## State

Upstream master (`origin/master`, 5dedfe648) landed AUTH1 (PR #430, the HTTP login endpoints: `serve_with_login`, `LoginRoutes`, login config in `http_listener.rs`, login wiring in the launcher). This branch (`fix/p34-t6-ws-token`) landed WebSocket token routes in the same files (`serve_with_ws`, `WsUpgrade`, `Context<G,E,W>`). The orchestrator started `git merge origin/master` in this worktree and left it IN PROGRESS: 12 files contain conflict markers. You resolve them by editing the files. Do NOT run git; the orchestrator runs `git add` and commits the merge.

## Part A - resolve the 12 conflicted files (semantic merge, keep BOTH features)

Files: `crates/shamir-server/src/boot/config/http_listener.rs`, `.../boot/config/tests/http_listener_tests.rs`, `.../engine/server/server_launcher/http/launcher.rs`, `crates/shamir-server/src/lib.rs`, `.../security/mod.rs`, `crates/shamir-server/tests/e2e/http_rest/common.rs`, `crates/shamir-transport-http/src/lib.rs`, `.../server/accept.rs` (8 hunks), `.../server/mod.rs`, `.../server/routes/mod.rs` (add/add: keep `pub mod ws;` and the login module declarations together), `.../server/service.rs`, `deploy/server.example.ktav`.

Rules:
- Use `git show origin/master:<path>` and `git show HEAD:<path>` (read-only git is allowed) plus the AUTH1 landed source to understand both sides. Master is the base for everything AUTH1 owns; T6 adds WS on top.
- Transport: one connection `Context` must carry BOTH the optional login routes and the WS upgrade service (both generic with no-op defaults, e.g. `NoLogin` and a `NoWs`-style default so `serve(..)` keeps compiling for REST-only callers). Provide `serve`, `serve_with_login`, `serve_with_ws` and ONE entry that takes both (`serve_with_routes` or equivalent) that the launcher uses when a listener has login and/or WS. Dispatch order: login paths first (before the bearer gate, as AUTH1 requires), then WS upgrade, then REST. `routes` stays `pub mod` only if `ws` types must be public; keep master's `pub(crate)` for the login module items and re-export what master re-exports.
- Both sides added a helper named `canonical_authority` (T6: returns `Option<String>`, master: returns `Result<String, String>`) and both add host-authority / origin validation in `http_listener.rs`. Keep master's function untouched and rename/adapt T6's usage so the two do not collide; keep both validations (T6's `host_authority_allowlist` canonical check and `browser_origin_allowlist` policy check, master's login block). Update `http_listener_tests.rs` accordingly (keep both test sets).
- Launcher: keep master's login audit-drain logic (detached workers, `routes.shutdown(5 s)`, audit cancellation) and T6's `ws_service` wiring; one `serve_*` call.
- `lib.rs` / `security/mod.rs`: export `http_login`, `http_rest` AND `http_ws`.
- `deploy/server.example.ktav`: keep both sets of keys and comments.
- `tests/e2e/http_rest/common.rs`: keep master's version of everything AUTH1/T3b owns and any T6 additions.
- After resolving, grep every file in the whole worktree for `<<<<<<<`, `=======`, `>>>>>>>` (none may remain), check `Cargo.toml`/features did not need changes, and verify module trees and entry counts (max 7 per folder, max 1000 lines per file, mounted tests) with `ls` and `python -I scripts/ci/check_layout.py`.

## Part B - fix the failing native e2e (root cause established)

CI failure `http_ws::native::http_ws_native_group` at `crates/shamir-server/tests/e2e/http_ws/native.rs:435`: the interactive `TxCommit` returns `access_denied` because commit authorizes Write on `db://default` (`crates/shamir-db/src/shamir_db/execute/core/db_tx.rs:376-384`) while the harness opened the database as `0o755` (`grant_default_main_access` in `tests/e2e/http_rest/common.rs`). Fix (test side): change the `chmod_db` mode of `db://default` to `0o757` and update its doc comment (interactive commit authorizes Write on the database). Do not change server policy.

## Part C - finish

Lint-aware read-through of every file you touched (clippy -D warnings classes: type_complexity, too_many_arguments max 7, needless_borrow, result_large_err, absolute_paths max 5 segments, unused imports in test targets, dead code, missing generic bounds after adding the second type parameter). `cargo fmt -p shamir-server -p shamir-transport-http` then `-- --check`. Append a short section "Revision 2: merge with AUTH1" to `docs/dev-artifacts/audits/2026-11-02-token-t6/DESIGN.md` describing the transport design (Context type parameters, entry points, dispatch order) and the final status.

## Rules (restated)

- NO LOCAL COMPILATION: only `cargo fmt -p <crate>` (and `-- --check`) and `python -I scripts/ci/check_layout.py`. No cargo check/clippy/build/test, no rustc, no npm. A child that runs them violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt. Do not edit `Cargo.toml`/`Cargo.lock`; a new dependency means stop and report.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, `git add`, `git commit`, `git merge --abort`, or any git command that mutates the tree or index. Only edit files.
- Children only edit files, are awaited, and each touched file is re-read by you. Never finish with a child outstanding.
- No machine-local absolute paths in any file you write.
- Time: run `date` first; stop editing at the time in the run file and report exactly what is done.
