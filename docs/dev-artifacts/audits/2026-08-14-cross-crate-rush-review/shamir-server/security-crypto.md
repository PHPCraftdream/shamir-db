<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-server — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

All three numbered observations remain open. Replication pinning is missing, but network position alone does not bypass SCRAM mutual authentication and TLS-exporter binding.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 3 | 0 | 0 | 0 | 2 | 1 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — `ReplicationConfig::replicator_password` is a plain `String` held for the server's lifetime, reachable via `Debug`

Status: `confirmed-open`. Current risk: `medium`.

Debug-derived Config/ReplicationConfig expose Option<String>; factory credentials retain an Arc<str>, with zeroization applied only to the outbound temporary buffer. Config itself is boot-local, not retained in ServerHandle. Debug logging leakage is possible, not an identified current logging call; dump exposure requires local privileged access.

Evidence: [crates/shamir-server/src/config.rs:71](../../../../../crates/shamir-server/src/config.rs#L71); [crates/shamir-server/src/config.rs:120](../../../../../crates/shamir-server/src/config.rs#L120); [crates/shamir-server/src/config.rs:133](../../../../../crates/shamir-server/src/config.rs#L133); [crates/shamir-server/src/replication/prod_factory.rs:47](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L47); [crates/shamir-server/src/replication/prod_factory.rs:67](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L67); [crates/shamir-server/src/replication/prod_factory.rs:112](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L112); [crates/shamir-server/src/server/server_launcher.rs:501](../../../../../crates/shamir-server/src/server/server_launcher.rs#L501); [crates/shamir-server/src/server/server_handle.rs:30](../../../../../crates/shamir-server/src/server/server_handle.rs#L30).

<a id="review-2"></a>

### Claim 2 — Replication client uses trust-on-first-use with no leader-key pinning

Status: `confirmed-open`. Current risk: `low`.

The factory always requests acceptance of a new Ed25519 identity and supplies no saved pin. However, the client first verifies the password-derived server signature over TLS-exporter-bound authentication. Arbitrary identity substitution by a network-only attacker does not establish an authenticated stream; credential/server-key compromise or another authentication defeat is additionally required.

Evidence: [crates/shamir-server/src/replication/prod_factory.rs:115](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L115); [crates/shamir-server/src/replication/prod_factory.rs:116](../../../../../crates/shamir-server/src/replication/prod_factory.rs#L116); [crates/shamir-client/src/client.rs:480](../../../../../crates/shamir-client/src/client.rs#L480); [crates/shamir-client/src/client.rs:615](../../../../../crates/shamir-client/src/client.rs#L615); [crates/shamir-connect/src/client/handshake.rs:226](../../../../../crates/shamir-connect/src/client/handshake.rs#L226); [crates/shamir-connect/src/client/handshake.rs:259](../../../../../crates/shamir-connect/src/client/handshake.rs#L259); [crates/shamir-connect/src/client/handshake.rs:264](../../../../../crates/shamir-connect/src/client/handshake.rs#L264); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:315](../../../../../docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L315).

<a id="review-3"></a>

### Claim 3 — `bootstrap_password` accepted as a CLI argument

Status: `confirmed-open`. Current risk: `nit`.

The plaintext option remains. Exposure depends on local process-inspection permissions and how the operator launches the process; shell-history retention is not universal. This is an optional hardening concern, not remote credential disclosure.

Evidence: [crates/shamir-server/src/main.rs:48](../../../../../crates/shamir-server/src/main.rs#L48); [crates/shamir-server/src/main.rs:55](../../../../../crates/shamir-server/src/main.rs#L55); [crates/shamir-server/src/main.rs:56](../../../../../crates/shamir-server/src/main.rs#L56).

<a id="review-summary-hmac-constant-time"></a>

### Claim Summary/HMAC constant-time — HMAC verification is constant-time and subtle-backed

Status: `unverified`. Current risk: —.

Application code delegates the tag comparison to Mac::verify_slice. Matching pinned hmac/digest implementation source was unavailable, so the external constant-time assertion was not independently proven. Hex-format rejection also exits early.

Evidence: [crates/shamir-query-types/src/hmac.rs:425](../../../../../crates/shamir-query-types/src/hmac.rs#L425); [crates/shamir-query-types/src/hmac.rs:431](../../../../../crates/shamir-query-types/src/hmac.rs#L431); [crates/shamir-query-types/src/hmac.rs:434](../../../../../crates/shamir-query-types/src/hmac.rs#L434); [Cargo.lock:1638](../../../../../Cargo.lock#L1638); [Cargo.lock:4000](../../../../../Cargo.lock#L4000).

<a id="review-summary-auth-defenses"></a>

### Claim Summary/auth defenses — Latency padding, lockout, fail-closed bootstrap metadata and pre-auth frame ceiling

Status: `unverified`. Current risk: —.

The named defenses exist at source level, including metadata-error rejection and bounded pre-auth reads. Their universal oracle-elimination/spec-conformance guarantee is not established by this read-only review. Successful AuthOk is written before the outer success pad, so padding presence alone is not proof of equal response timing.

Evidence: [crates/shamir-server/src/connection/handshake.rs:375](../../../../../crates/shamir-server/src/connection/handshake.rs#L375); [crates/shamir-server/src/connection/handshake.rs:431](../../../../../crates/shamir-server/src/connection/handshake.rs#L431); [crates/shamir-server/src/connection/handshake.rs:454](../../../../../crates/shamir-server/src/connection/handshake.rs#L454); [crates/shamir-server/src/connection/handshake.rs:646](../../../../../crates/shamir-server/src/connection/handshake.rs#L646); [crates/shamir-server/src/connection/handshake.rs:717](../../../../../crates/shamir-server/src/connection/handshake.rs#L717); [crates/shamir-server/src/connection/handshake.rs:784](../../../../../crates/shamir-server/src/connection/handshake.rs#L784).

<a id="review-summary-unsafe-and-manifest"></a>

### Claim Summary/unsafe-and-manifest — No unsafe blocks and a manifest-path traversal guard

Status: `not-applicable`. Current risk: —.

No unsafe blocks were found by source search. Manifest verification rejects absolute paths, ParentDir components and duplicate entries before hashing. This establishes lexical path validation, not a universal filesystem-containment guarantee against locally manipulated symlinks.

Evidence: [crates/shamir-server/src/backup.rs:420](../../../../../crates/shamir-server/src/backup.rs#L420); [crates/shamir-server/src/backup.rs:421](../../../../../crates/shamir-server/src/backup.rs#L421); [crates/shamir-server/src/backup.rs:472](../../../../../crates/shamir-server/src/backup.rs#L472); [crates/shamir-server/src/lib.rs:17](../../../../../crates/shamir-server/src/lib.rs#L17).

## Corrections and qualified non-findings

- The missing pin is an Ed25519 protocol identity pin, not a persisted TLS-certificate pin.
- Remove unconditional network-only MITM/read/injection claims; SCRAM mutual authentication and exporter binding remain enforced.
- Config plaintext copies are boot-local; the Arc<str> credential is the long-lived copy. No current password-printing log call was established.
- Zeroizing is not mlock and does not itself prevent plaintext from appearing in a live-memory dump.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
