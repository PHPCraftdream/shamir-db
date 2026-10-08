<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Credential hygiene and missing persisted protocol pinning remain, with the existing threat-model qualifications. Exact archives resolve the HMAC comparison question. Authentication timeout, padding placement and expiry-clock defects prevent a clean spec-conformance conclusion.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 3 | 0 | 0 | 1 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `ReplicationConfig::replicator_password` is a plain `String` held for the server's lifetime, reachable via `Debug`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Config and ReplicationConfig expose plaintext through derived Debug. Boot clones are temporary, but the production factory retains an ordinary Arc&lt;str&gt; password for reconnects; only the outgoing copy is Zeroizing. No existing logging call printing this password was found. Live-memory exposure requires the corresponding local access.

Evidence: [crates/shamir-server/src/config.rs:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L71); [crates/shamir-server/src/config.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L120); [crates/shamir-server/src/config.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L133); [crates/shamir-server/src/replication/prod_factory.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L47); [crates/shamir-server/src/replication/prod_factory.rs:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L112).

<a id="review-2"></a>

### Claim 2 — Replication client uses trust-on-first-use with no leader-key pinning

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Every newly built source accepts a fresh Ed25519 identity without a configured persisted pin. Nonetheless, the client first verifies the password-derived server signature over exporter-bound authentication, then the identity signature. Network position alone does not prove stream impersonation.

Evidence: [crates/shamir-server/src/replication/prod_factory.rs:115](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L115); [crates/shamir-server/src/replication/prod_factory.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L116); [crates/shamir-client/src/client.rs:480](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L480); [crates/shamir-connect/src/client/handshake.rs:259](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L259); [crates/shamir-connect/src/client/handshake.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L278).

<a id="review-3"></a>

### Claim 3 — `bootstrap_password` accepted as a CLI argument

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The plaintext option remains. Process-command-line/history disclosure depends on local permissions and launch method; this is optional credential-input hardening, not remote disclosure.

Evidence: [crates/shamir-server/src/main.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/main.rs#L55); [crates/shamir-server/src/main.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/main.rs#L56).

<a id="review-summary-hmac-constant-time"></a>

### Claim Summary/HMAC constant-time — HMAC verification is constant-time and subtle-backed

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Exact hmac 0.12.1, digest 0.10.7 and subtle 2.6.1 archives support fixed-length content-independent tag comparison: digest verify_slice calls finalize_fixed().ct_eq(tag), and subtle folds every byte. Published sources: https://docs.rs/crate/digest/0.10.7/source/src/mac.rs and https://docs.rs/crate/subtle/2.6.1/source/src/lib.rs. Public length/hex validation may reject early; no machine-code timing proof is claimed.

Evidence: [crates/shamir-query-types/src/hmac.rs:425](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/hmac.rs#L425); [crates/shamir-query-types/src/hmac.rs:434](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/hmac.rs#L434); [Cargo.lock:1151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1151); [Cargo.lock:1638](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1638); [Cargo.lock:4000](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4000).

<a id="review-summary-auth-defenses"></a>

### Claim Summary/auth defenses — Latency padding, lockout, fail-closed bootstrap metadata and pre-auth frame ceiling

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

The named mechanisms exist, but the associated clean/spec-conformance assurance has positive counter-evidence: challenge output precedes padding, proof waiting lacks a deadline, and bootstrap expiry uses a pre-wait timestamp. Metadata read errors do reject. TCP checks length before allocation; WS checks the application ceiling after obtaining a complete Binary message, so 4 KiB is not a universal transport allocation bound.

Evidence: [crates/shamir-server/src/connection/handshake.rs:210](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L210); [crates/shamir-server/src/connection/handshake.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L277); [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291); [crates/shamir-server/src/connection/handshake.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L431); [crates/shamir-server/src/connection/handshake.rs:784](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L784); [crates/shamir-transport-ws/src/framing.rs:147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L147); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:515](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L515).

<a id="review-summary-unsafe-and-manifest"></a>

### Claim Summary/unsafe-and-manifest — No unsafe blocks and a manifest-path traversal guard

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

No handwritten unsafe blocks were found in this crate. Manifest paths receive absolute/ParentDir and duplicate checks. This does not cover unsafe code in dependencies or filesystem escape through locally controlled symlinks.

Evidence: [crates/shamir-server/src/backup.rs:420](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L420); [crates/shamir-server/src/backup.rs:472](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L472); [crates/shamir-server/src/lib.rs:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/lib.rs#L17).

## Evidence and recipe corrections

- Replace 'exact hmac/digest source unavailable' with the verified published-archive comparison chain; this is supported pre-existing behavior, not a source fix.
- The normative challenge-padding obligation is contradicted by write ordering, not merely unmeasured. Statistical timing-oracle exploitability remains unverified.
- Fail-closed metadata-error handling does not prove TTL correctness when its now_ns argument predates an attacker-controlled wait.
- The copied evaluate_bootstrap_gate helper exercises the metadata API, not the actual handshake caller or its timestamp placement.
- A secret wrapper should redact Debug and zeroize retained copies, but neither property prevents reading a live credential or provides mlock.
- Persist the upstream Ed25519 protocol identity, with explicit rotation/replacement semantics; do not substitute an unrelated TLS-certificate pin.

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
