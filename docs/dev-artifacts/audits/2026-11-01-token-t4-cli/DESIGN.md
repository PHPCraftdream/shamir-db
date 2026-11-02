# TOKEN-T4 offline token CLI final review

## Decisions and known defects

- The prior audit blocker was false. `AuditChainWriter::checkpoint_now` rejects the persistence-failed latch (`crates/shamir-connect/src/server/infra/audit_chain.rs:547-567`). T2b remains unchanged.
- Offline operations hold the maintenance lock through the operation, verify the full chain from genesis and verify available appender/meta checkpoints as anchors. Older valid checkpoints are permitted. Strict writer is used. Rotation/retention are explicitly disabled offline: verify requires the complete log from genesis and no rotation/pruning behavior is introduced.
- Existing online boot checkpoint mismatch is observed at `crates/shamir-server/src/engine/server/server_launcher/launch.rs:317`: online launch consumes meta checkpoint while scheduler persists appender checkpoint; `store_audit_checkpoint` has no other callers. Boot untouched.
- Do not delete/move three accidental files outside permitted scope: `src/security/tokens/cli/list.rs`, `src/security/tokens/cli/run.rs`, `crates/shamir-db/worktrees/p34-t4-token-cli/crates/shamir-server/src/security/tokens/cli/create.rs`. Orchestrator cleanup required.

## Progress

- DONE — Re-read brief, listed sources, CLAUDE mandatory lines 51-106 verbatim and relevant rust-intel modules.
- DONE — Single CLI dispatch in run module, args in args.rs; public API and main dispatch.
- DONE — Shared Fjall user directory Arc, no second open; T2b list_all change removed.
- DONE — Preflight create_new secret output and no-TTY refusal before data open; deliver only after checkpoint/meta persist; injected stdout/stderr/terminal seam; generic sanitized delivery errors with non-secret token ID.
- DONE — Clap TTL/channel/id parsers, fixed/stable id-sorted output, control escaping, allowlist sentinel.
- DONE — Lifecycle tests added (create/list/revoke/reopen/audit verification), parser, output file permissions/exclusivity, no-tty and existing file negatives.
- DONE — `cargo fmt -p shamir-server`, fmt check, and layout check passed after the final edits.
- PARTIAL — Tests cannot be run and compilation is not proven. No lock-refusal or orphan revoke-all dedicated test. Full touched-file compile-oriented read-through not complete; see CI requirements.

## Final report

- Public API: `tokens::cli::{TokenAction, CreateArgs, ListArgs, CliIo, run_with_io, run}`. `TokenService` API unchanged.
- Tests added: `parser_rejects_missing_label_bad_ttl_bad_channel_and_revoke_without_yes`, `stable_list_render_escapes_label_owner_and_database_controls`, `no_tty_refusal_precedes_data_directory_open`, `existing_secret_file_is_refused_before_data_open`, `secret_file_is_exclusive_and_unix_mode_0600`, `create_list_revoke_reopen_and_verify_audit_continuity`.
- No local test/build/check/clippy/rustc/npm performed. Only allowed format/layout commands; passed.
- CI must establish compile/lint and run full suite, particularly lifecycle/audit checkpoint failure cases and Windows output path.
- Final changes remain NOT compile-proven. Inspect CLI source/tests before commit; there may be issues hidden absent compilation.
- Cleanup required outside scope for the three misplaced paths named above. No git mutation, moving, or deleting performed.
