# shamir-server -- Security & crypto boundary

## Summary

The connection/handshake/auth boundary in this crate is unusually well
hardened: constant-time HMAC verification (via `hmac::Mac::verify_slice`,
which is `subtle`-backed), explicit latency padding to close the SCRAM
timing oracle, per-pair exponential lockout backoff, fail-closed handling of
bootstrap-token metadata errors, a documented pre-auth frame-size ceiling
(`MAX_PRE_AUTH_FRAME`) before any Argon2id work, and a manifest-path
traversal guard on backup/restore. No `unsafe` blocks exist anywhere in the
crate. The main gaps found are secret-hygiene issues around the
replication-follower password (never wrapped in a zeroizing/secret type
until the last possible moment, and reachable through a `Debug`-deriving
`Config`) and one internal-only trust-on-first-use TLS pinning gap that is
already flagged as known future work by the authors.

## Findings

### 1. `ReplicationConfig::replicator_password` is a plain `String` held for the server's lifetime, reachable via `Debug`

- File: `crates/shamir-server/src/config.rs:133` (field), `crates/shamir-server/src/config.rs:71` / `:120` (`#[derive(Debug, ...)]` on `Config` and `ReplicationConfig`)
- Severity: medium
- Issue: The follower-replication password is deserialized straight into `Option<String>` and stored on `Config` for the whole process lifetime (`server_launcher.rs:500` clones it into `repl_cfg`, then `prod_factory.rs:58/67` stores it as `Arc<str>` inside `ReplicatorCreds`, cloned into every `LazyWireSource`). Unlike every other credential path in this crate (`bootstrap.rs`'s `Zeroizing<Vec<u8>>`, `db_handler/admin.rs`'s `Zeroizing` password buffer, `access_tree.rs`'s `Zeroizing`), this plaintext password is never wrapped in `Zeroizing`/`SecretString` until `prod_factory.rs:112` constructs the outbound `ConnectOptions` at connect time — by which point it has already been copied through `Config` (Debug-derived), `ReplicationConfig` (Debug-derived, cloned at `server_launcher.rs:500`), and `Arc<str>` (immutable, cannot be zeroized even if desired) across however many subscriptions exist.
- Failure scenario: (a) A future `tracing::debug!(?config)` / `anyhow::Context` chain that includes the `Config` or `ReplicationConfig` value (both derive `Debug`) in a log line or error message would print the plaintext replicator password into logs/telemetry — the same class of leak the codebase explicitly defends against elsewhere (e.g. `observability.rs`'s M5 gate on `/metrics` exposing `auth_attempts_total`, or the audit-log HMAC-only user-hash logging in `handshake.rs`). (b) Because the password lives in ordinary (non-`mlock`, non-zeroizing) heap memory for the server's entire uptime, a heap dump / core dump / swap write captures it in plaintext long after the credential is no longer in active use, unlike the SCRAM bootstrap/admin paths which minimize the plaintext window via `Zeroizing`.
- Suggested fix: Change `ReplicationConfig::replicator_password` to a wrapper that is `Debug`-redacted (e.g. a `SecretString`-like newtype with a custom `Debug` impl that prints `"<redacted>"`, mirroring `shamir_query_types::auth::SecretString` already used in `db_handler/admin.rs:103`), and thread `Zeroizing`/secret-typed values through `ReplicatorCreds` instead of `Arc<str>`.

### 2. Replication client uses trust-on-first-use with no leader-key pinning

- File: `crates/shamir-server/src/replication/prod_factory.rs:113-116`
- Severity: low
- Issue: `LazyWireSource::connected()` sets `accept_new_host: true, trusted_pin: None` unconditionally when a follower dials its leader — i.e. there is no persisted pin of the leader's TLS/identity key across reconnects. This is called out honestly in the code's own comment ("Trust-on-first-use: the follower has no pre-pinned leader key in 386-c. Persisting a leader pin is future work (#388)"), so it is a known, tracked gap rather than an oversight.
- Failure scenario: A network-positioned attacker who can intercept the very first follower→leader connection (or any reconnection after a state reset) can present a different TLS identity and the follower will accept it silently, enabling a MITM of the replication stream (read access to replicated data, or injection of a spoofed leader's stream).
- Suggested fix: No action needed beyond what's already tracked (#388) — flagging for visibility since this is a legitimate MITM surface on the replication data path, just already acknowledged as scoped-out for this milestone (386-c).

### 3. `bootstrap_password` accepted as a CLI argument

- File: `crates/shamir-server/src/main.rs:56` (`--bootstrap-password`)
- Severity: nit
- Issue: The bootstrap superuser password can be supplied via `--bootstrap-password <PASSWORD>` on the command line. Process command lines are visible to other local users via `/proc/<pid>/cmdline` (Linux) or process listing tools, and typically persist in shell history.
- Failure scenario: A co-resident local user or monitoring agent captures the plaintext bootstrap password from `ps`/process-listing output or shell history at server-start time.
- Suggested fix: This is a common, low-severity CLI ergonomics tradeoff (the tool already defaults to a safer random-token mode when the flag is omitted, per `bootstrap.rs`'s module doc), so no change is required unless the project wants to push operators toward an environment-variable or stdin-prompt alternative for this flag specifically.

No other findings for this theme — the SCRAM/Argon2id handshake, HMAC
"did-you-mean-it" destructive-op gating, TLS material lifecycle, session
resumption ticket handling, and pre-auth frame-size/rate-limit/lockout
defenses were all reviewed and are consistent with the documented spec
sections they cite.
