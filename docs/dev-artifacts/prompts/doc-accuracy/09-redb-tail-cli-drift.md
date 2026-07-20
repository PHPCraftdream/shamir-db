# RI-1: redb-остатки и CLI-расхождения — main.rs, backup.rs, deploy/, AGENTS.md

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Direct follow-up to task #715 (commit `33109972`, which rewrote
`crates/shamir-storage/src/README.md` and `crates/shamir-server/src/backup.rs`'s
module doc against the real fjall-only backend). An external release review
(2026-07-20, HEAD `33109972`) found the redb-stale tail that #715's brief did
NOT cover, plus one drift #715's own rewrite introduced/preserved. Five
precise fixes, all documentation/comments — no behavior change.

The authoritative fjall reasoning ALREADY EXISTS in
`crates/shamir-server/src/backup.rs`'s rewritten module doc (read it first —
lines ~12-27): fjall exposes no online backup API; commits are journal
batches (`Start → items → End(xxh3 checksum)`); default
`RecoveryMode::TolerateCorruptTail` truncates a torn tail batch back to the
last fully-checksummed batch boundary on recovery. Reuse that reasoning —
do not invent new durability claims.

## The five fixes

### 1. `crates/shamir-server/src/main.rs` (~lines 76-80)

The `Backup` subcommand's doc comment still says:

> redb's per-page CRC + atomic-commit design means a copy taken during a
> quiescent window is recoverable as the pre-commit state, but for
> confidence stop the server.

Rewrite against the fjall model (consistent with `backup.rs`'s module doc):
stop-and-copy is the supported path; a copy racing an in-flight journal
append loses only the torn tail batch on next open
(`TolerateCorruptTail`), it does not corrupt earlier committed batches —
but stopping the server first is the strongest guarantee. Keep it short
(it's a CLI help doc comment — 2-4 lines).

### 2. `crates/shamir-server/src/backup.rs` (line ~4)

The module doc's CLI example says:

> `shamir-server backup --from <data_dir> --to <dest>`

but the REAL CLI (check `main.rs`'s clap derive for the `Backup` variant)
takes `--config <ktav>` + `--to <dest>` and derives `data_dir` from the
config — there is no `--from` flag. Fix the example to the real invocation.
Verify against the actual clap definition, don't trust this brief's wording.

### 3. `deploy/README.md` (~lines 61-72, the "## Backup" section)

Two problems:

a. Line ~65: "(redb's per-page CRC + atomic-commit makes live copies safe
   to recover, but stop-and-copy is the strongest guarantee)" — rewrite to
   the fjall equivalent (same reasoning as fix 1: torn-tail-batch loss,
   `TolerateCorruptTail`, stop-and-copy strongest).

b. The cron example right below runs
   `shamir-server backup --config ... --to /backups/` with NO server stop,
   directly contradicting the "Stop the service for a fully consistent
   snapshot" instruction above it. Make the cron example honest: either
   (i) wrap it with `systemctl stop shamir-db && … backup … && systemctl
   start shamir-db` (noting the downtime window), or (ii) keep the live
   copy but explicitly label it "best-effort live snapshot — recovers to
   the last complete journal batch; for a guaranteed-consistent snapshot
   stop the service first". Pick ONE and make the text and the example
   agree — do not leave the contradiction.

### 4. `deploy/shamir-db.service` (line ~17)

Comment says "(… socket + several per redb mmap region) plus headroom" —
this justifies a file-descriptor/memory limit in redb mmap terms. Check
what the limit actually is (read the surrounding lines) and rewrite the
justification for fjall's actual file usage: a fjall database is a
directory of journal files + LSM segment files per keyspace (segments are
opened as needed; compaction creates new files). Don't over-claim — if you
can't substantiate a precise per-file accounting, say "journal + LSM
segment files per keyspace" and leave the headroom framing.

### 5. `AGENTS.md` (~lines 45-52)

The pre-commit gate block recommends raw `cargo test --workspace --lib`.
That invocation is BLOCKED by the workspace perimeter guard (cargo runner
in `.cargo/config.toml` gates on `$NEXTEST`) — an agent following
AGENTS.md verbatim hits a refusal. Replace the test line with the
centralised entry point, consistent with `CLAUDE.md`'s "Centralised test
entry point" section and `CONTRIBUTING.md`:

```
./scripts/test.sh          # or: cargo tl
```

Read the surrounding AGENTS.md section first and keep its formatting/tone;
change only the test invocation line(s) and any sentence that directly
describes it. If AGENTS.md mentions raw `cargo test` anywhere else, fix
those too (grep the file).

## Out of scope

- Do NOT touch `deploy/Dockerfile` or Rust-version mentions in
  `deploy/README.md` (that's task RI-2, a separate brief).
- Do NOT touch `crates/shamir-storage/src/README.md` (already rewritten,
  #715).
- No code changes — comments/docs only. `main.rs`'s doc comment IS
  compiled (clap help text), so the build gate below matters.

## Verification (MANDATORY before you report done)

- `cargo build -p shamir-server` clean.
- `./scripts/test.sh -p shamir-server` green.
- `cargo fmt -p shamir-server -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above, plus file:line
  citations for what you verified (the real clap flags for fix 2, the
  actual service-file limit for fix 4).
