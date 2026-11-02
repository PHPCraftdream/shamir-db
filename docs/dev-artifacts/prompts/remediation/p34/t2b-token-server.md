# TOKEN-T2b - shamir-server: token_key, durable token store, TokenService, owner floors

Source: `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md` (normative; sections 2, 3, 5, 6, 12 rules 1-5, 13 are the contract of this stage; read all of it) and the design `docs/dev-artifacts/design/token-api-rest-ws.md`. The pure core already exists in `shamir-connect` (stage T2a, verified by CI): `shamir_connect::server::token::{TokenId, TokenSecret, parse_token, encode_token, generate_token_parts, compute_mac, mac_eq, dummy_mac_work, TokenMirror, TokenState, TokenRecord, TokenMode, TokenChannels, Endpoint, verify_token, VerifyCtx, Verified, VerifyFailure, OwnerFloors}`, `shamir_connect::server::{TokenSessionStore, origin::SessionOrigin}`. Read those files first (`crates/shamir-connect/src/server/auth/token/`, `state/origin.rs`, `state/token_session.rs`, `state/session.rs` for `Session::for_token`), then `CLAUDE.md` (Code ideology, layout limits, rust-guard gates).

This stage is storage and service only: no listener, no HTTP, no WebSocket, no CLI, no wire `DbRequest` variants. Later stages: T3b listener and `ScopedHandler`, T4 offline CLI, T5 online administration, T6 WebSocket.

## Goal

### A. `token_key` in server_meta (`crates/shamir-server/src/engine/server_meta.rs`, 805 lines; stay under 1000 or move new code into a sibling file)

- A new persisted 32-byte `token_key`, created from the OS CSPRNG, stored under a new key of the `server_meta_v1` keyspace next to `secrets`/`identity` (follow how `lockout_secret` is generated, stored, redacted in `Debug`, zeroized and exposed through `ServerSecrets`; the token key is NEVER rotated in v1). `key_id = 1`.
- Existing deployments have a meta without the key: it must be created lazily and durably on first open after upgrade (idempotent, crash-safe, no change to existing keys' bytes), and a meta that has it must reload the same bytes. Never log it; `Debug` prints `<REDACTED>`.
- Accessor returning a zeroizing 32-byte copy or a small `TokenKey` newtype (your choice; redacted `Debug`, zeroize on drop).

### B. Durable token store (`crates/shamir-server/src/security/tokens/`, new folder; the `security/` folder has 6 entries, so this is the 7th: do not add anything else to `security/`)

- One fjall database under `<data_dir>/api_tokens` (look how other stores open fjall under the data dir and how backup/restore enumerate them: `sync/durability/restore.rs`, `server_meta`, the user directory), keyspace `api_tokens_v1`, key = the 16-byte token id, value = msgpack `TokenRecord` (named encoding, as T2a's `to_vec_named`/`validate_shape` expect). Open performs a full scan, validates each record's shape, and loads the live ones into a `TokenMirror`; a corrupt record is skipped with a WARN (id only, never the content) and counted, not fatal.
- Writes are durable (fsync / persist mode as the user directory does) BEFORE any in-memory publication. API on a `TokenStore` type: `open`, `put(record)`, `get`, `scan`, `delete(id)`.
- Garbage collection: records expired or revoked more than 30 days ago are deleted, once at open and then hourly by a task owned by the service (cancellable through the service's `CancellationToken`, no detached tasks that outlive shutdown). Use the same clock abstraction the rest of the server uses for time (look for an injectable clock in the user directory or lockout code); GC must be testable without sleeping.

### C. `TokenService` (same folder)

An `Arc`-shareable service owning the store, the mirror, the key and the process-wide `TokenSessionStore` (single instance, see TOKEN_API.md section 6). Methods (async where they touch fjall; use `spawn_blocking` for blocking fjall calls, as the rest of the crate does):

- `create(NewToken) -> Result<CreatedToken, TokenCreateError>` where `NewToken { owner_user_id, owner_roles (snapshot), label, ttl_secs: Option<u64>, mode, channels, databases }`. Enforces: TTL default `tokens.default_ttl_days`, maximum `tokens.max_ttl_days` (reject larger or zero with `TokenTtlInvalid`), label non-empty, at most 64 bytes, unique among the owner's ACTIVE tokens (not expired, not revoked) -> `LabelExists`; at most `tokens.max_per_user` ACTIVE tokens per owner -> `LimitReached`; `databases` allowlist shape. Generates id and secret (T2a), computes the MAC with the stored key (`key_id = 1`, `format_version = 1`), writes the record durably, then publishes it into the mirror, then returns `CreatedToken { record_view, token: Zeroizing<String> }`. The secret exists only in that return value. The per-owner checks and the insert must be atomic with respect to concurrent creates for the same owner (a per-owner serialization point; no global lock on the verification path).
- `revoke(id, now) -> Result<RevokeOutcome, ..>`: ORDER IS NORMATIVE: (1) durable write of `revoked_at_ns`, (2) publish in the mirror (`TokenState::revoke`), (3) cancel the sessions of that token (the `TokenSessionStore` gives the sessions; cancellation of sockets is asynchronous work that belongs to T6, so here call a `SessionCanceller`-style hook trait object that the service holds, default no-op, with a test double), (4) return. Idempotent (revoking a revoked token succeeds and does not change the timestamp). `revoke_user(owner, now) -> usize` does the same for all of an owner's active tokens. `RevokeUserTokens` does NOT touch the owner floor.
- `list(owner) -> Vec<TokenView>` with no secret, no MAC, no hash.
- `verify(header_bytes, endpoint, now_ns) -> Result<Verified, VerifyFailure>`: thin wrapper over T2a `verify_token` with the key and an `OwnerFloors` implementation; the dummy HMAC path for unknown ids is inside `verify_token`.
- Counters: active token count hint; metrics hooks are T3b's concern, but expose cheap `u64` counters (created, revoked, verified-ok, verified-failed) via atomics.
- Audit: emit `api_token_created`, `api_token_revoked`, `api_token_user_revoked` through the existing audit writer (look at how `audit_writer` events are defined and queued; the entry carries the token id and the owner id for created, never the secret or the MAC). If adding event variants forces changes in `shamir-connect` audit types, keep them minimal and put them in this stage.

### D. Owner floors over the user directory

- `FjallUserDirectory` (`crates/shamir-server/src/security/user_directory/directory.rs`, 947 lines: add at most a few lines there; if more is needed put it in a new sibling file) gets `floor_if_present(&self, user_id: &[u8; 16]) -> Option<u64>`: reads the existing lock-free `tickets_cache` (`tickets_invalid_before_ns_by_user_id` at line 216 is the model) and returns `None` for an unknown user (the existing method returns 0 for unknown; the token path must be able to tell unknown from zero). Do not change existing behaviour.
- An adapter type implementing `shamir_connect::server::token::OwnerFloors` for `Arc<FjallUserDirectory>` (in the tokens folder).

### E. Configuration

A `TokensConfig` section (`boot/config.rs` is 715 lines; put the struct into `boot/config/tokens.rs`, the folder has only `limits.rs`): `default_ttl_days = 30`, `max_ttl_days = 365`, `max_per_user = 20`, `allow_self_service = true`, `session_max_age_hours` (default 24), `max_sockets_per_token = 8`, `max_sockets_per_owner = 32`. Wire it into the server `Config` with `#[serde(default)]` and the same parse/validate path as the other limit structs (look at `boot/config/limits.rs` for the pattern, including how `Config::validate` reports errors and how the ktav/config examples under `deploy/` are documented; add the section to the example config only if that pattern exists). Validation: all values positive, `default_ttl_days <= max_ttl_days`, caps sane.

### F. Boot wiring

Construct the `TokenService` during launch (find the place where the user directory, lockout and audit writer are built, `engine/server/server_launcher/launch.rs`, 689 lines, and `listeners.rs`), keep it in the launcher/server handle so later stages (listener, CLI) reach it, and shut its GC task down with the server's cancellation token. Add it to `ListenerEnv` only if that is a one-line field; do NOT start any listener. The read-only replica role: the service opens and verifies, but `create`/`revoke` return `TokenCreateError::ReadOnlyNode` (TOKEN_API.md section 12 rule 5) - find how the server knows its role.

## Non-goals

HTTP listener and routing (T3b); wire ops and `DbRequest` variants (T5); the offline CLI (T4); WebSocket (T6); the `ScopedHandler`; metrics export; per-subnet failure limiter (T3b); rate limits; `shamir-client` changes.

## Tests (all deterministic: no sleeps, injectable clock; tests live in `tests/` folders wired with `#[cfg(test)] mod tests;`, one topic per file, JSON-free; fjall in `tempfile` directories)

token_key: first open creates, reopen returns the same bytes, an old meta without the key gets one without changing other keys, Debug is redacted. Store: put/get/scan round trip; reopen loads live records; a corrupt record is skipped and counted; GC removes only records older than 30 days past expiry/revocation, keeps younger ones, runs at open. Service: create returns a token that `verify` accepts on the right endpoint with the right owner floor and rejects after revoke; MAC/secret never in `Debug`/errors; TTL bounds (zero, over max, default), label rules, per-owner active limit counted over active tokens only (an expired or revoked token frees a slot and its label), concurrent creates for one owner never exceed the limit (use `std::thread::scope` + `Barrier`); revoke ordering test (hook observes the mirror already revoked and the store already durable when it fires; failure of the durable write leaves the mirror untouched); revoke idempotence; `revoke_user`; owner floor: a token whose `created_at_ns <= floor` fails, unknown owner fails (using a fake `OwnerFloors`), plus one test through the real `FjallUserDirectory::floor_if_present` (None for unknown, Some for known, bumped by the existing ticket-invalidation path, `invalidate_all_tickets` bumps it); read-only role rejects mutations; config defaults/validation.

## Rules

- NO LOCAL COMPILATION (operator rule): the only cargo command allowed is `cargo fmt -p shamir-server` (+ `-- --check`) and `cargo fmt -p shamir-connect` if you touch it; no `cargo check`/`clippy`/`build`/`test`, no rustc, no npm. CI does the compiling and linting; the workspace lints run with `-D warnings` (`#![warn(missing_docs)]` is promoted to errors in crates that enable it - check `crates/shamir-server/src/lib.rs`; `clippy.toml` forbids `std::collections::HashMap/HashSet` and `RandomState`: use `shamir_collections` types or DashMap/scc with `THasher`; `absolute_paths` max 5 segments; no `#[allow]`; no inline `#[cfg(test)]` modules; no local `use`; `mod.rs` files re-export only; at most 7 entries per folder and 1000 lines per file - count with `ls`/`wc -l` because `check_layout.py` ignores untracked files).
- Before the final report, read every file you touched end to end for compile and clippy errors (module paths, imports, unused items, privacy, `cfg(test)` gating, `Send`/`'static` on spawned futures, trait bounds, exhaustive matches, needless clones and borrows, `manual_flatten`/`needless_match`/`type_complexity` style lints) and list the files you read. A lint-aware pass is worth it: each failed CI round costs two hours.
- Touch only `crates/shamir-server/` (and `crates/shamir-connect/` for minimal audit-type additions if unavoidable), your design note, and `Cargo.toml`/`Cargo.lock` only if a dependency is truly missing (stop and report instead of adding). Keep every existing comment. No visibility widening beyond what the new folder needs.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits. You cannot move or delete files: list any such need. Use forward slashes in all paths.
- Crash resilience: keep a design note `docs/dev-artifacts/audits/2026-11-01-token-t2b-server/DESIGN.md` (decisions, new public items, deviations); append one line per finished item (A-F) as you go, and write the final report into it under `## Final report`. Stop editing at the time given in your run file.

## Final report (mandatory, also in the note)

Per item A-F DONE / PARTIAL / DISPUTED with file:line evidence, tests added by name, files read end to end for compile errors, every new public item, files for the orchestrator to move or remove, and what needs CI. Gates that need cargo are reported NOT RUN (CI).
