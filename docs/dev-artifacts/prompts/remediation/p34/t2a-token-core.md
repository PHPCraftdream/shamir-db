# TOKEN-T2a - shamir-connect: API token core (format, MAC, record, verifier, mirror, token sessions)

Source: `docs/guide-docs/client-server-protocol-spec/TOKEN_API.md` (normative, v1 draft; read all of it, sections 2, 3, 5, 6, 7 and 10 are the contract of this stage) and the design `docs/dev-artifacts/design/token-api-rest-ws.md`. This stage is the pure, transport-free, storage-free core in `shamir-connect`. Persistence, the listener, the CLI and the wire operations are later stages (T2b server store and service, T3 REST, T4 CLI, T5 online administration, T6 WebSocket). Read first: `CLAUDE.md` (Code ideology, layout limits, rust-guard gates), `crates/shamir-connect/src/server/state/session.rs` (`Session`, `SessionPermissions`, `SessionStore`, `SessionIdHasher`), `crates/shamir-connect/src/server/mod.rs`, `crates/shamir-connect/src/common/primitives/crypto.rs` (`try_random_bytes`), and the test layout `crates/shamir-connect/src/server/tests/`.

## Goal

Add the following to `shamir-connect` (feature `server`), with no I/O and no async runtime.

### A. Token format (`server/auth/token/format.rs`)

- Constants and types: `TOKEN_PREFIX = "shk1"`, `TOKEN_LEN = 71`, `ID_B64_LEN = 22`, `SECRET_B64_LEN = 43`, `TokenId([u8; 16])` (Copy, Eq, Hash, Debug prints base64url id only), `TokenSecret` (wraps `Zeroizing<[u8; 32]>`; `Debug` prints `<REDACTED>`; no `Display`; no `Clone` unless unavoidable).
- `parse_token(input: &[u8]) -> Result<ParsedToken, TokenParseError>`: input is raw header bytes (may be non-UTF8, empty, huge: reject on length != 71 before any other work). Positional parse: `shk1` + `.` + 22 chars + `.` + 43 chars. Alphabet `A-Za-z0-9_-`. **Canonical decoding**: non-zero trailing bits in the last character of `id` (4 bits) and of `secret` (2 bits) are rejected, so one token has exactly one string form. `TokenParseError` is a single fieldless-in-effect error (callers must not be able to distinguish causes on the wire; a debug-only reason enum is acceptable if it is never exposed through `Display` of the public API).
- `encode_token(id: &TokenId, secret: &TokenSecret) -> Zeroizing<String>` and `generate_token_parts() -> Result<(TokenId, TokenSecret), TokenError>` from the OS CSPRNG (`try_random_bytes`).
- `TokenId::to_b64()` / `TokenId::from_b64(&str)` (22-char form, canonical, used by CLI and listings).
- Use the `base64` crate version 0.22 already present in the lockfile (add `base64 = "0.22"` to `shamir-connect/Cargo.toml`; this one dependency is pre-approved; no other new dependency). Engine `URL_SAFE_NO_PAD` with trailing-bits checking enabled and padding forbidden. If you prefer a hand-written codec, it must pass the same tests.

### B. MAC (`server/auth/token/mac.rs`)

- `compute_mac(key: &[u8; 32], key_id: u8, format_version: u8, id: &TokenId, secret: &TokenSecret) -> [u8; 32]` = `HMAC-SHA256(key, "shamir-api-token" || [format_version] || [key_id] || id(16) || secret(32))`. Exactly this byte layout (TOKEN_API.md section 3).
- `mac_eq(a: &[u8; 32], b: &[u8; 32]) -> bool` constant time (`subtle`).
- `dummy_mac_work(key, ...)`: computes one HMAC over fixed dummy data and discards it (used by the verifier on an unknown id so unknown-id and wrong-secret cost the same).

### C. Record (`server/auth/token/record.rs`)

`TokenRecord` per TOKEN_API.md section 3 with serde (msgpack, `to_vec_named`, `deny_unknown_fields` on decode): `id`, `owner_user_id`, `label`, `mac`, `key_id`, `format_version`, `created_at_ns`, `expires_at_ns`, `mode: TokenMode (ReadOnly|ReadWrite)`, `channels: TokenChannels (Rest|Ws|Both)`, `databases: Option<Vec<String>>`, `roles: Vec<String>` (snapshot of the owner's roles at creation), `revoked_at_ns: Option<u64>`. Byte fields use `serde_bytes`. `Endpoint { Rest, Ws }` with `TokenChannels::allows(Endpoint)`. A `NewTokenRequest`-style validator is NOT in this stage (TTL, label and per-user limits are T2b/T5); only structural validation (`TokenRecord::validate_shape`: expires after created, label non-empty and at most 64 bytes, at most 64 database names each non-empty and at most 128 bytes, at most 64 roles).

### D. Mirror and state (`server/auth/token/mirror.rs`)

- `TokenState`: immutable `TokenRecord` fields plus an atomic revocation cell (`AtomicU64` holding `revoked_at_ns`, 0 = not revoked; `revoke(now_ns)` is idempotent and keeps the first value via `compare_exchange`/`fetch_max` as you justify). `is_revoked()` uses `Acquire`, `revoke` publishes with `Release` (write the argument in the type doc, including why a session holding `Arc<TokenState>` observes a revoke published before `revoke` returns).
- `TokenMirror`: concurrent in-memory index `id -> Arc<TokenState>` and `owner_user_id -> set of ids`. Use `dashmap` with the crate's existing Fx hasher type (follow `SessionStore`), not a global lock. API: `insert(record)`, `get(&TokenId)`, `revoke(&TokenId, now_ns) -> bool`, `revoke_owner(&[u8;16], now_ns) -> usize`, `ids_of_owner`, `remove(&TokenId)` (garbage collection), `load_all(iter)`, `len_hint()` backed by an `AtomicUsize` mirror (never O(N) on a hot path). Guards are never held across an await (there are none here) and never two at once; say so in comments.

### E. Verifier (`server/auth/token/verify.rs`)

`verify_token(ctx: &VerifyCtx, input: &[u8]) -> Result<Verified, VerifyFailure>` implementing TOKEN_API.md section 5 steps 2 to 7 in that order:

- `VerifyCtx { mirror: &TokenMirror, key: &[u8; 32], key_id: u8, owners: &dyn OwnerFloors, now_ns: u64, endpoint: Endpoint }`.
- trait `OwnerFloors { fn floor_if_present(&self, user_id: &[u8; 16]) -> Option<u64>; }` (the server implements it over the user directory's `tickets_invalid_before_ns` cache; `None` = unknown user). Keep it object-safe and sync.
- Order: parse (fail -> `Invalid`); lookup (miss -> `dummy_mac_work`, then `Invalid`); MAC compare (mismatch -> `Invalid`); then `Revoked`, `Expired` (`now_ns >= expires_at_ns`), `OwnerInvalid` (owner unknown, or `created_at_ns <= floor`, strict `>` required to pass); only after all those pass: `ChannelDenied` if `channels` does not allow `endpoint`. `VerifyFailure` variants: `Invalid` (malformed, unknown id and bad MAC are ONE variant), `Revoked`, `Expired`, `OwnerInvalid`, `ChannelDenied`. `Verified { state: Arc<TokenState> }`.
- A `key_id` or `format_version` in the stored record that differs from the verifier's is `Invalid`.
- Wrong-secret and unknown-id must perform the same number of HMAC computations (one). Document this and test it structurally (a test hook counting MAC computations behind `#[cfg(test)]`, not a timing test).

### F. Token session (`server/state/origin.rs`, `server/state/token_session.rs`, edits in `session.rs`)

- `SessionOrigin { Scram, Token(TokenOrigin) }` in `origin.rs`; `TokenOrigin { state: Arc<TokenState> }` with accessors (`token_id()`, `scope()` returning the `mode`, `channels`, `databases` view, `is_live(now_ns)` = not revoked and not expired, `Acquire` load).
- `Session` gets a `pub origin: SessionOrigin` field set to `Scram` by `Session::new` (no signature change of `new`) and a constructor `Session::for_token(verified: &Verified, username: String, now_ns: u64, session_id: [u8; 32]) -> Session`: permissions `SessionPermissions::new(false, false, record.roles.clone())`, `transport_kind = TransportKind::WebSocket`, `binding_mode = BindingMode::None`, zero `channel_binding_at_auth` (fixed filler values, documented as never exposed), `user_id = owner_user_id`, `session_id` as given. Existing code and tests construct sessions through `Session::new` only; do not touch their call sites.
- `TokenSessionStore` (`token_session.rs`): process-wide store of WebSocket token sessions (REST sessions are per request and are not stored). `dashmap` + Fx like `SessionStore`. API: `admit(sid, session: Session, caps: TokenSocketCaps { per_token: usize, per_owner: usize }) -> Result<Arc<Session>, TokenAdmitReject>` (rejects `PerTokenCap`, `PerOwnerCap`; accounting with per-token and per-owner counters, race-free: reserve with a CAS loop on the counter, undo on failure of the later step; never consume the SCRAM `SessionStore` caps), `remove(sid)`, `get(sid)`, `sessions_of_token(&TokenId) -> Vec<Arc<Session>>` (for the revocation fan-out in T6), `count_for_token`, `count_for_owner`. Removing a session releases both counters exactly once (guard against double remove).
- `ResumeConfig`/ticket code: nothing to change, token sessions never pass through the handshake; add one test that a session built by `for_token` reports `origin` as `Token` and that `SessionStore` insertion is not required for it.

### G. Exports and layout

- `server/auth/mod.rs`: `pub mod token;` and `server/auth/token/mod.rs` is re-exports only. `server/state/mod.rs`: `pub mod origin; pub mod token_session;`. `server/mod.rs`: re-export `token` under `shamir_connect::server::token` (like `bootstrap`, `handshake`), and `SessionOrigin`, `TokenSessionStore`.
- Folder limits: `server/auth/token/` holds `mod.rs`, `format.rs`, `mac.rs`, `record.rs`, `mirror.rs`, `verify.rs` (6 entries; do not add more than 7). `server/state/` goes from 5 to 7 entries with `origin.rs` and `token_session.rs`; `session.rs` is 740 lines, keep it under 1000. `server/tests/` gets a `token_tests/` folder (the folder then has 5 entries) with topic files: `format_tests.rs`, `mac_tests.rs`, `record_tests.rs`, `mirror_tests.rs`, `verify_tests.rs`, `token_session_tests.rs`, `mod.rs` (manifest only). Wire it in `server/tests/mod.rs`.

## Tests (red first in CI; no sleeps; no timing assertions)

- format: round trip; every malformed shape (empty, 70/72 chars, wrong prefix, wrong separators, extra separator, `+` `/` `=` characters, non-ASCII, non-UTF8 bytes, 1 MiB input) returns the same error variant; non-canonical trailing bits rejected for `id` and for `secret` (build the string by flipping the low bit of the last character); canonical string accepted; `Debug` of secret and of a parsed token never prints the secret.
- mac: layout pinned (recompute independently in the test from the documented concatenation and compare), changing any of key, key_id, format_version, id, secret changes the MAC, `mac_eq` rejects a one-bit difference in every byte position.
- record: msgpack round trip, unknown field rejected, shape validation limits (each boundary and one past it), `TokenChannels::allows` truth table.
- mirror: insert/get/revoke/revoke_owner/remove/owner index consistency; revoke is idempotent and keeps the first timestamp; a stress test with threads (use `std::thread` and a barrier) racing `revoke` against readers asserts that a reader that starts after `revoke` returned never sees a live token; owner index has no residue after remove.
- verify: the full decision table (valid, malformed, unknown id, wrong secret, wrong key_id, revoked, expired exactly at the boundary, owner unknown, owner floor equal (`created_at_ns == floor` fails) and floor one below (passes), channel denied only after the secret verified (a wrong secret on the wrong channel is `Invalid`, not `ChannelDenied`)); the MAC computation counter is 1 for unknown id and for wrong secret.
- token session store: caps per token and per owner, release on remove, double remove does not underflow, concurrent admit at the cap admits exactly `cap` sessions (barrier-started threads), `sessions_of_token` sees only live entries.
- origin/session: `for_token` produces non-superuser, non-replicator permissions even if the roles list contains the strings `"superuser"` and `"replicator"` (the flags are false), `origin` is `Token`, `Session::new` yields `Scram`.

## Deliverables

A short design note `docs/dev-artifacts/audits/2026-11-01-token-t2a-core/DESIGN.md` written FIRST: the revocation publish/observe argument (who stores, who loads, with which orderings), the verifier decision table, the owner-floor contract (`OwnerFloors`), the cap accounting protocol, and the test plan mapped to the invariants.

## Scope

Crate `shamir-connect` only (plus `Cargo.toml` of that crate for `base64`, and the lockfile the tooling updates; if `Cargo.lock` changes, say so in the final report; do not touch other crates). No behaviour change to existing SCRAM paths. Public signatures of existing items do not change (`Session` gains a public field, which is the one allowed addition; `Session::new` keeps its signature). No `#[allow]`, no new `unsafe`. Hashers and maps follow `SessionStore`. Every secret-bearing type has a redacting `Debug`; tokens, secrets and MACs never appear in `Display`, `tracing` or error text.

## Coordination with other tasks

P34-03b edits engine/db/storage and a seed-fence file in `shamir-server`; T0b (already in the base) moved files in `shamir-server`. Nothing else edits `shamir-connect`.

## Decisions

TOKEN_API.md decisions are binding: `created_at_ns > floor` strict; unknown id performs a dummy HMAC; `Invalid` is one variant for malformed, unknown and bad MAC; channel check only after the secret verified; no async runtime and no `tokio` in this crate; roles are a snapshot in the record because any role change bumps the owner's floor and kills the token.

## Rules (verbatim policy for every delegated run)

- Compilation, builds and tests run on GitHub CI, not on this machine. BANNED: `cargo test`, `cargo nextest`, `./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, any npm/tsc/vitest build or test run, WASM builds, e2e/integration harnesses, loom, any release or full-workspace build.
- ALLOWED locally, scoped to the touched crates (`-p <crate>`, plus their dependents when a public signature changed): `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo fmt -p <crate>` and `cargo fmt -p <crate> -- --check`, `python -I scripts/ci/check_layout.py`. One cargo process at a time, `CARGO_BUILD_JOBS=2`. Report real exit codes.
- A child that believes it needs a build or test run says so in its final message; the orchestrator decides.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Touch only the listed crates and the files this brief names. No new dependency (if one is unavoidable, stop and report). No `#[allow]`, no visibility widening beyond what the card needs. Layout limits: at most 7 direct entries per folder, at most 1000 lines per source file; new logic goes into new files. `mod.rs` files are re-exports only; imports at the top of the file; tests live in `tests/` dirs (one topic file per group); JSON literals in tests are multi-line.
- Every behavioral fix ships with a test that FAILS on the old code (red first, observed in CI). Documentation-only cards need no test. A card that turns out to be already fixed or wrong is reported, not forced.
- A card whose Decision is SKIP is not changed; a card whose Decision is ESCALATE or that carries OWNER DECISION without an answer is skipped and said so in the final report. A card whose Decision is IMPLEMENT is implemented exactly as the decision paragraph says; deviations need a written reason.
- Cards whose recipe or evidence you cannot reproduce from the code at the stated location are reported with your own evidence (file:line), not silently changed.

## Local gates (only these)

`cargo fmt -p <crate> -- --check`; `cargo clippy -p <crate> --all-targets -- -D warnings`; `cargo check` of dependents when a public signature changed; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries of touched folders yourself with ls). Crates: shamir-connect; also run `cargo check -p shamir-server --all-targets` once at the end, because `Session` gains a public field. Report real exit codes.

## Final report

Per item of the design stage: DONE / ALREADY-FIXED / SKIPPED(decision) / DISPUTED with file:line evidence; the tests added (names and the old-code failure they prove); any file outside the listed ones you needed; anything that needs a build or test run; every deviation from the design with its written reason.
