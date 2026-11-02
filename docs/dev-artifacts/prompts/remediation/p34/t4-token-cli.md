# TOKEN-T4 - shamir-server: offline token CLI (`shamir-server token ...`)

Sources, read in this order: `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md` (normative: section 2-4 token format and representation, 5 verification and owner floors, 8 error codes, 12 rule 5 and 13 audit), `docs/dev-artifacts/design/token-api-rest-ws.md` (section 7.1 offline CLI, section 11 logging, section 12 configuration), then the code this stage uses (stage T2b, already in this branch): `crates/shamir-server/src/security/tokens/{mod.rs,store.rs,owners.rs,model/*,service/*}` (`TokenService`, `NewToken`, `TokenServiceOptions`, `TokenView`, `CreatedToken`, `TokenCreateError`, `DirectoryOwnerFloors`), `crates/shamir-server/src/security/user_directory/*` (resolving a user name to the 16-byte user id), the server boot that wires the token service and the audit chain (`src/engine/server/server_launcher/launch.rs`, search `TokenService::open`, `AuditChainWriter`, `audit_chain_key`), `src/security/access_tree.rs` and `src/main.rs` (the existing offline subcommands `access-tree`, `backup`, `restore`: copy their pattern for opening `data_dir` offline, their exclusive-access behaviour and their error style), `src/boot/config.rs` (`tokens` section), `CLAUDE.md`.

## Goal

An operator with shell access can manage API tokens on a STOPPED data directory:

```
shamir-server --config C token create --user U --label L [--ttl 30d] [--read-write] [--channels http|ws|both] [--db NAME]... [--out-file PATH]
shamir-server --config C token list [--user U]
shamir-server --config C token revoke <ID>
shamir-server --config C token revoke-user U
shamir-server --config C token revoke-all
```

This is the only way to manage tokens on a read-only replica and the bootstrap path before any online operation exists (online operations are stage T5; not here).

Rules of the command set (TOKEN_API sections 3, 4, 12 and the design note 7.1):
- `create` defaults: `read_only` mode, channels `both`, ttl `tokens.default_ttl_days`; `--ttl` accepts `<N>s|m|h|d`, validated against `tokens.max_ttl_days`; `--db` repeated builds the allowlist (omitted = no allowlist); label and limits are validated by `TokenService::create` (surface its `TokenCreateError` as a clear message). The owner is resolved by user name through the user directory; an unknown user is an error that does not reveal anything else.
- The secret (the whole `shk1.<id>.<secret>` string) is printed exactly once: to stdout only when stdout is a terminal (`std::io::IsTerminal`), or written to `--out-file` created with `create_new` (never overwrite) and, on Unix, mode 0600 at creation time (`OpenOptionsExt::mode`), on Windows the file is created exclusively and the command prints a warning that file ACLs are not tightened. With neither a TTY nor `--out-file` the command refuses BEFORE creating the token. The secret is never logged, never put in an error, never in `Debug` output, and the buffer is zeroized after use (`Zeroizing`). After a successful create the non-secret metadata (id as base64url of 16 bytes, label, owner, mode, channels, databases, expiry) is printed to stdout regardless.
- `list [--user U]` prints one line or block per token: id (base64url, 22 chars), label, owner name, mode, channels, databases, created, expires, revoked; never a secret, MAC or hash. Stable, script-friendly output.
- `revoke <ID>` accepts the 22-character base64url id; unknown id is an error; an already revoked token is reported and exit code stays 0 only if that is what `TokenService::revoke` returns as "no change" (document the choice). `revoke-user U` and `revoke-all` (all users; iterate the directory's users, or scan the store) print the number revoked. `revoke-all` asks for the literal confirmation flag `--yes` and refuses without it.
- Offline only: opening a data directory that a running server holds must fail with a clear message, not corrupt anything. Reuse the exclusive-lock behaviour the other offline subcommands rely on and say in the report what the failure looks like. Read-only replica data directories are the target audience: the CLI opens the token service with `read_only_node = false` (it is an operator action on a stopped node); say so in a comment.
- Audit: every create and revoke the CLI performs must produce the same audit events as the online path (`TokenService` already appends them to the chain when it is given the shared `AuditChainWriter`). Build the writer the same way the server boot does (chain key from server meta, checkpoint, the file appender) so the offline events land in the same HMAC chain and the checkpoint stays consistent; if you find that this cannot be done safely offline, stop and report it as PARTIAL with the exact obstacle instead of writing a second chain.
- Exit codes: 0 success, non-zero on any error; errors go to stderr without secrets.

Layout (hard limits: at most 7 direct entries per folder, at most 1000 lines per file, one primary export per file, tests in `tests/` folders, `mod.rs` re-exports only). `crates/shamir-server/src/` and `src/security/` are at 7 entries and `src/security/tokens/` is at 6: put all CLI logic in ONE new subfolder `src/security/tokens/cli/` (at most 7 entries: for example `mod.rs`, `args.rs` for the clap types, `run.rs` dispatch, `create.rs`, `list.rs`, `revoke.rs`, `secret_output.rs`), tests in `src/security/tokens/tests/` (currently 3 entries; add topic files, a subfolder if you need more than 4). `src/main.rs` gets only a thin `Subcmd::Token { action }` variant and the dispatch arm (keep it under 600 lines); the clap `TokenAction` enum lives in the library module so it is testable with `try_parse_from`.

Optional last item, only after everything above is complete and read through: a "tokens" section in the `access-tree` output (`src/security/access_tree.rs`): per principal, the number of live tokens. If it needs more than a small additive change, skip it and say so.

Non-goals: online wire operations (T5), HTTP/WS listeners, changing `TokenService` behaviour (if you find a defect in T2b code, report it with file:line, do not rewrite it), new dependencies (stop and report if one is unavoidable; `clap`, `base64`, `zeroize` are already available to this crate: verify in `crates/shamir-server/Cargo.toml` before use).

## Tests (write them, you cannot run them)

Unit tests with `Cli::try_parse_from`-style parsing for every subcommand and every error (missing `--label`, bad `--ttl`, unknown `--channels`, `revoke-all` without `--yes`); ttl parser table; id parsing (22-char base64url, wrong length, wrong alphabet); output rendering for `list` (no secret material, fixed column order); `create` refuses without TTY and without `--out-file` before touching the store (inject the "is terminal" decision through a small parameter so it is testable); out-file written with `create_new`, existing file refused, Unix mode 0600 (`#[cfg(unix)]`); an end-to-end test on a temp data dir that creates a user, runs create -> list -> revoke -> list through the library entry points and asserts the secret never appears in any stdout/stderr buffer except the one intended output, the audit events were appended, and a second open of the same directory after the first handle is dropped works. Use existing test fixtures for users and directories (look in `src/tests/` and the T2b tests).

## Rules

- NO LOCAL BUILDS OR TESTS: the only cargo commands you may run are `cargo fmt -p shamir-server` and `cargo fmt -p shamir-server -- --check`, and `python -I scripts/ci/check_layout.py`. No cargo check/clippy/build/test, no rustc, no npm. CI compiles, lints and tests after the orchestrator pushes. Compensate with a lint-aware read-through of every file you touch: imports and paths, visibility, `Send`/`'static` captures, guards across `.await`, exhaustive matches, struct literals of any type you extend (grep the WHOLE workspace for literals and builders), `Option<RecordId>` is Copy (no clone), `absolute_paths` at most 5 segments, `too_many_arguments` at most 7, no `#[allow]`, no inline `#[cfg(test)]` modules, no local `use`, no `std::collections::HashMap`/`HashSet` (use the workspace Fx types), `#[cfg(unix)]`/`#[cfg(windows)]` gating of platform code and unused-import effects on the other platform.
- Touch only `crates/shamir-server/` (and its `Cargo.toml` only if a dependency is already in the workspace lock; tell the orchestrator because `Cargo.lock` is edited by hand) plus the note below.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. You cannot move or delete files: list such needs. Forward slashes in all paths. No machine-local absolute paths in any file you write (use repo-relative paths in notes).
- Crash resilience: keep a note `docs/dev-artifacts/audits/2026-11-01-token-t4-cli/DESIGN.md` (decisions, new public items, tests, what remains); append one line per finished item as you go; write the final report into it under `## Final report`.

## Final report (mandatory, also in the note)

Per item DONE / PARTIAL / DISPUTED with file:line evidence; tests added by name; files read end to end for compile errors; every new public item; deviations from the sources with a written reason; what needs CI.
