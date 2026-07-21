# RI-9: Bootstrap-token lifecycle — TTL, auto-delete, configurable path, doc accuracy

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context — already investigated, do not re-derive

Review 2026-07-20, P0#7. The LIVE bootstrap path today (`--bootstrap-password`
omitted → `BootstrapMode::RandomToken`) is:
`main.rs` CLI → `server_launcher.rs` → `bootstrap.rs::ensure_superuser` →
writes a plaintext token to `data_dir/bootstrap_token.txt` (0o600 on Unix)
and logs the path at WARN. **The operator must manually delete this file.**
Two concrete problems: (a) it sits in `data_dir` until manually removed, so
every `backup --to` sweep (which copies the whole `data_dir`) captures it
verbatim; (b) nothing ever auto-expires or auto-deletes it.

**Key finding: the fix infrastructure for (b) ALREADY EXISTS but is
UNWIRED.** `crates/shamir-server/src/server_meta.rs`'s `ServerMetaStore`
already has a complete `PersistedBootstrap` row with
`bootstrap_token_hash: Option<Zeroizing<Vec<u8>>>`,
`bootstrap_token_expires_at_ns: Option<u64>`, and three methods —
`bootstrap_token_active()`, `set_bootstrap_token(hash, expires_at_ns)`,
`consume_bootstrap_token()` — **none of which is ever called from any live
code path** (verified: zero call sites outside `server_meta.rs` itself and
its own doc comments). This task's job for TTL/consume is to WIRE this
existing machinery up, not build new state-tracking from scratch.

**Second finding — separate, larger, explicitly OUT OF SCOPE for this
task:** `crates/shamir-connect/src/server/bootstrap.rs` (`BootstrapState`,
`make_bootstrap_challenge`, `BootstrapRequest`, wire types
`BootstrapHello`/`BootstrapChallenge`) and its client-side twin
`crates/shamir-connect/src/client/bootstrap.rs` (`build_hello`,
`verify_challenge`, `build_request`, `run_local_bootstrap_with`) implement a
COMPLETE challenge-response bootstrap wire protocol per spec §11.3 — but
**neither side is ever invoked from the live server dispatch
(`shamir-server`) or any live client connection path** (verified: zero
references to `BootstrapState`/`BootstrapHello`/`BootstrapChallenge`/
`make_bootstrap_challenge` anywhere in `crates/shamir-server` or
`crates/shamir-query-types`'s wire-message enum). It is a fully-built,
disconnected library feature — not reachable by any operator today. Do
**NOT** wire this up in this task (that is a materially larger feature:
new pre-auth wire message types, a new dispatch branch, client-side
integration in both Rust and TS clients). Instead, this task's doc-honesty
obligation (item 5 below) is to record this accurately as ROADMAP, not
DONE — the same "spec says X, code has X in a library, nothing invokes it"
gap this campaign's discipline (RI-6) already established a pattern for.

## The task — DECIDED scope, implement exactly this

### 1. Extend `ServerMetaStore`'s bootstrap row (`crates/shamir-server/src/server_meta.rs`)

- Add two fields to `PersistedBootstrap`: `bootstrap_username: Option<String>`
  and `bootstrap_token_path: Option<std::path::PathBuf>` (add `PathBuf` to
  the existing `use std::path::Path;` import). Update every literal
  construction of `PersistedBootstrap` in this file (there are 3: initial
  state, and the two `unwrap_or(PersistedBootstrap { ... })` fallbacks in
  `set_bootstrap_token`/`consume_bootstrap_token`) to include the new
  fields (`None` in all the "empty" constructions).
- `set_bootstrap_token`: change signature to
  `pub fn set_bootstrap_token(&self, username: &str, hash: [u8; 32], expires_at_ns: u64, token_path: std::path::PathBuf) -> Result<(), MetaError>`.
  Persist `username`/`token_path` alongside the existing hash/expiry.
- `consume_bootstrap_token`: also clear the two new fields back to `None`
  in its `next` construction (existing early-return-if-already-consumed
  guard logic stays the same, just check all fields consistently).
- Add three new getters, matching the style of `bootstrap_token_active()`:
  `bootstrap_username(&self) -> Option<String>`,
  `bootstrap_token_path(&self) -> Option<std::path::PathBuf>`, and
  `bootstrap_token_expired(&self, now_ns: u64) -> bool` — returns `true`
  iff a hash is present AND `bootstrap_token_expires_at_ns <= now_ns` (use
  this for the boot-time sweep in item 3; do NOT reuse `bootstrap_token_active`
  for expiry — it only checks hash presence, not expiry, and must keep that
  narrower meaning since other future callers may want "is a token
  currently outstanding at all" separately from "has it expired").

### 2. Configurable token output path (`crates/shamir-server/src/bootstrap.rs`, `server/bootstrap_mode.rs`, `main.rs`)

- `bootstrap_mode.rs`: add a field to `BootstrapMode::RandomToken`:
  `RandomToken { username: Option<String>, token_path: Option<std::path::PathBuf> }`.
- `main.rs`: add a new CLI flag `--bootstrap-token-path <PATH>`
  (`#[arg(long, value_name = "PATH")] bootstrap_token_path: Option<PathBuf>`,
  next to `--bootstrap-user`/`--bootstrap-password`), threaded into
  `BootstrapMode::RandomToken { username: cli.bootstrap_user, token_path: cli.bootstrap_token_path }`.
  Update the `--bootstrap-password` doc comment (currently: "written to
  `data_dir/bootstrap_token.txt`") to describe: default path unchanged,
  override via `--bootstrap-token-path`, auto-deletes on first successful
  login or after a 24h TTL (whichever first) — do not describe manual
  deletion as the primary mechanism any more.
- `bootstrap.rs`: change `BootstrapPolicy::RandomToken` to carry the same
  optional override: `RandomToken(Option<std::path::PathBuf>)`. In
  `ensure_superuser`, when writing the token file, use
  `token_path_override.unwrap_or_else(|| data_dir.join(BOOTSTRAP_TOKEN_FILE))`
  instead of the current hardcoded `data_dir.join(BOOTSTRAP_TOKEN_FILE)`.
  If the override path's parent directory doesn't exist, create it
  (`fs::create_dir_all`) before writing — same as the existing `data_dir`
  fallback already does. Same 0o600 perms logic applies regardless of path.
  Update the module doc comment (the "Operators are expected to read the
  token, log in once via SCRAM and `changePassword`, then delete the token
  file" paragraph) to describe the new automatic behavior — auto-delete on
  first successful login (primary) or 24h TTL sweep at next boot
  (backstop for tokens nobody ever used) — while still recommending manual
  deletion as an immediate belt-and-braces step for anyone who can't wait.
- `server_launcher.rs`'s two `ensure_superuser(...)` call sites: pass the
  new `BootstrapPolicy::RandomToken(token_path)` (the `Password` call site
  is unaffected — its `BootstrapPolicy::Password` variant doesn't change).

### 3. Wire the TTL/consume machinery into the live boot + login paths (`crates/shamir-server/src/server/server_launcher.rs`, `crates/shamir-server/src/connection/{connection_context.rs,handshake.rs}`)

- Define `pub const BOOTSTRAP_TOKEN_TTL_NS: u64 = 24 * shamir_connect::common::time::ns::HOUR;`
  in `bootstrap.rs` (co-located with `BOOTSTRAP_TOKEN_FILE`) — 24h, matching
  the existing resumption-ticket TTL convention already used in
  `handshake.rs` (`RESUMPTION_TICKET_TTL_NS`).
- **Boot-time TTL sweep** (`server_launcher.rs`, run unconditionally, in
  ALL bootstrap modes, before the existing "2. Bootstrap (idempotent)"
  step, using the already-constructed `meta`): if
  `meta.bootstrap_token_expired(now_ns)` is true, best-effort-delete the
  file at `meta.bootstrap_token_path()` (log a `tracing::warn!` on I/O
  failure, do not fail boot) and call `meta.consume_bootstrap_token()`.
  This must run even in `BootstrapMode::Skip`/`Password` boots, since a
  previously-issued token from an earlier `RandomToken` boot could still be
  outstanding and expired.
- **On successful `RandomToken` bootstrap** (`server_launcher.rs`'s
  `BootstrapMode::RandomToken` match arm, after `ensure_superuser` returns
  `BootstrapOutcome::Created { token: Some(tok), token_path: Some(path) }`):
  call `meta.set_bootstrap_token(name, shamir_connect::common::crypto::sha256(tok.as_bytes()), now_ns.saturating_add(BOOTSTRAP_TOKEN_TTL_NS), path)`.
  (`sha256` is already imported/available via `shamir_connect::common::crypto` —
  confirm the exact import path used elsewhere in this crate.)
- **Thread `meta` into `ConnectionContext`** (`connection_context.rs`): add
  `pub meta: Arc<ServerMetaStore>` as a new field + constructor param
  (there is exactly ONE call site for `ConnectionContext::new(...)`, in
  `server_launcher.rs:801` — pass the same `meta` already in scope there).
- **Consume-on-first-successful-login** (`handshake.rs`, right after the
  existing `// Reset lockout on success per spec §5.2.5 NORMATIVE.` /
  `ctx.lockout.reset_on_success(pair);` lines, before the session is
  built): if `ctx.meta.bootstrap_token_active()` AND
  `ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())`,
  best-effort-delete the file at `ctx.meta.bootstrap_token_path()` (log a
  `tracing::warn!` on failure) and call `ctx.meta.consume_bootstrap_token()`.
  This must be **best-effort and non-fatal** — a failure here must NEVER
  abort an otherwise-successful login.

### 4. Deploy artefacts — recommend an out-of-data_dir path

- `deploy/README.md`: rewrite the bootstrap paragraph (currently lines
  ~64-67, ~84) to state: default path is still `data_dir/bootstrap_token.txt`
  for backward compatibility, but operators SHOULD pass
  `--bootstrap-token-path /run/shamir/bootstrap_token.txt` (tmpfs — not
  swept into `backup --to`, matching the IMPLEMENTATION_GUIDE's own tmpfs
  recommendation) — and the file auto-deletes on first successful login or
  a 24h TTL, whichever comes first, so the "then delete the token file"
  manual-cleanup instruction is no longer the primary safeguard.
  Explicitly note: if left at the default `data_dir` path, the token WILL
  be captured by any `backup --to` run before it is consumed/expired.
- `deploy/shamir-db.service`: add `--bootstrap-token-path
  /run/shamir/bootstrap_token.txt` to the example `ExecStart` invocation
  (create the `/run/shamir` dir via `RuntimeDirectory=shamir` in the unit
  file if that directive isn't already present — check the current file
  first).

### 5. IMPLEMENTATION_GUIDE.md — honest DONE/PARTIAL/ROADMAP status (RI-6 discipline)

Find the "Bootstrap token output options" subsection (§2.1, `tty` /
`file:<path>` / `command:<cmd>`) and the full wire-protocol description
(§11.3-area references to `bootstrap_hello`/`bootstrap_challenge`/
`bootstrap`). Using the same DONE/PARTIAL/ROADMAP legend RI-6 already
established in this file:
- `file:<path>` output mode: **DONE** — describe accurately what's now
  implemented (configurable path via `--bootstrap-token-path`, 0o600 perms,
  24h TTL, auto-delete-on-first-login, boot-time expiry sweep).
- `tty` output mode (stdout-only-if-isatty-and-not-systemd) and
  `command:<cmd>` output mode (pipe to external command): **ROADMAP** — not
  implemented; only `file:<path>` exists today.
- The full challenge-response wire protocol (`bootstrap_hello` →
  `bootstrap_challenge` → `bootstrap`, `BootstrapState`,
  `shamir_connect::client::bootstrap`): **ROADMAP** — add a note that the
  server-side (`shamir_connect::server::bootstrap`) and client-side
  (`shamir_connect::client::bootstrap`) implementations exist as library
  code but are NOT wired into any live server dispatch handler or client
  connection path; the operationally-reachable bootstrap flow today is
  CLI-flag + local-file-based (`--bootstrap-password` /
  `--bootstrap-token-path`), not the wire protocol described elsewhere in
  this document. Do not delete the wire-protocol documentation — mark it
  ROADMAP so the gap is visible, not silently true.
- Add a short module-doc note to the top of
  `crates/shamir-connect/src/server/bootstrap.rs` (one or two lines) making
  the same "library code, not wired into any live dispatch path" fact
  visible to a reader of the source directly, not just the spec doc.

## Out of scope

- Do NOT wire up the `BootstrapState`/wire-protocol challenge-response
  bootstrap flow. That's a separate, materially larger feature.
- Do NOT implement `tty` or `command:<cmd>` output modes.
- Do NOT change `BootstrapPolicy::Password` / `BootstrapMode::Password`
  behavior — only the `RandomToken` path is in scope.
- Do NOT touch `changePassword` or any other post-bootstrap admin flow.

## Tests (MANDATORY)

1. **`crates/shamir-server/src/tests/server_meta_tests.rs`** (new file —
   none exists yet; wire into `crates/shamir-server/src/tests/mod.rs`'s
   `pub mod ...;` list). Unit-level, using `ServerMetaStore::open_or_init`
   against a `tempfile::TempDir`-backed path (mirror the pattern in
   `crates/shamir-server/tests/server_meta.rs`, the existing INTEGRATION
   test file for this store — but this new file is a `src/tests/` UNIT
   test file, faster, no server boot). Cover: `set_bootstrap_token` +
   `bootstrap_token_active`/`bootstrap_username`/`bootstrap_token_path`
   round-trip; `consume_bootstrap_token` clears all three fields and sets
   `superuser_ever_existed`; `bootstrap_token_expired` returns `false`
   before expiry and `true` at/after `expires_at_ns`; a fresh store with no
   bootstrap row ever written returns `false`/`None` from every getter
   (not a panic).
2. **Extend `crates/shamir-server/src/tests/bootstrap_tests.rs`**: a test
   proving `ensure_superuser` with `BootstrapPolicy::RandomToken(Some(override_path))`
   writes the token file at `override_path`, NOT `data_dir/bootstrap_token.txt`.
3. **Genuine end-to-end regression test** proving the full live wiring —
   add it alongside the existing integration tests in
   `crates/shamir-server/tests/` (mirror `quickstart_e2e.rs`'s
   `spawn_ephemeral`/`Client::connect` pattern). `tests/common/mod.rs`
   currently only has `spawn_with_password`/`spawn_ephemeral` (password
   mode) — add a `spawn_with_random_token(temp, addr) -> (ServerHandle, String /* token */)`
   helper mirroring `spawn_with_password` but using
   `BootstrapMode::RandomToken { username: None, token_path: None }`,
   reading the generated token back off disk (`temp.path().join("bootstrap_token.txt")`)
   so the test can use it as the login password. New test file (e.g.
   `crates/shamir-server/tests/bootstrap_token_lifecycle_e2e.rs`) proving:
   (a) the token file exists right after boot; (b) connecting via
   `shamir_client::Client::connect` with username `admin` + the token as
   password succeeds; (c) AFTER that successful connect, the token file is
   gone from disk; (d) a second connection attempt using the SAME
   (now-stale) token as password fails (since `changePassword` was never
   called, the SCRAM record itself is unchanged and the token is still a
   valid credential — this test is specifically about the FILE being gone
   and `ServerMetaStore::bootstrap_token_active()` being `false`, not about
   revoking the credential itself, which is intentionally out of scope —
   assert file-gone and `bootstrap_token_active() == false` via a
   `ServerMetaStore::open_or_init` re-open against the same data_dir after
   `handle.shutdown()`, not a second failed connection).

## Docs

- `CHANGELOG.md`: add one bullet under `## [Unreleased]` — new
  `--bootstrap-token-path` CLI flag, bootstrap token now auto-deletes on
  first successful login or a 24h TTL (whichever first), matching the
  existing bullet style in that section.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-server --full` green, including all new/
  extended tests.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above.
