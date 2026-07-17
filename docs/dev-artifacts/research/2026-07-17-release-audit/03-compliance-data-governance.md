# Compliance & Data Governance — Technical Gap Analysis (2026-07-17 release audit, part 03)

Read-only research pass over the working tree (branch `master`). This is a
**technical** gap analysis of the features/guarantees an operator would need to
run S.H.A.M.I.R. DB under real-world regulatory obligations (GDPR-style
erasure, audit-trail integrity, retention correctness, encryption-at-rest,
access logging, tenant isolation, supply-chain evidence). It is not a legal
opinion. Every claim cites `file:line` evidence from the tree as of this date.

---

## Executive summary

The project is in **substantially better compliance shape than a typical
alpha database**, because two prior audit-remediation waves (2026-07-06
security/compliance/supply-chain, 2026-07-10 permission audits) already
produced: a tamper-evident HMAC-chained durable audit log, an honest and
detailed at-rest / erasure posture document
(`docs/guide-docs/security/data-protection.md`), a per-table retention +
purge wire API, Argon2id credential storage with a redacting `SecretString`
wrapper, and a CI supply-chain gate (`cargo deny` + weekly `cargo audit`).

The significant remaining gaps cluster into four themes:

1. **Audit coverage is auth-only.** The durable HMAC-chained audit log is
   production-wired but the only events ever appended are handshake events
   (`auth_success` / `auth_failed`). DDL, ACL changes, admin operations,
   data reads/writes, retention/purge invocations, and backup/restore are
   **not** written to the durable audit chain — despite the operations guide
   claiming they are. Engine-level access tracing (`trace_access`) is a
   `log::trace!` line: ephemeral, off by default, not an audit record.
2. **Erasure has documented residuals plus undocumented ones.** The
   data-protection doc honestly covers WAL-segment and LSM-compaction
   lag and the append-only field-name interner, but does **not** cover
   secondary-index residuals (HNSW soft-delete tombstones retain vectors
   until compaction; vector snapshots persist tombstone sets), nor
   replica residuals (purge does not propagate through the replication
   pull API).
3. **Documentation-ahead-of-implementation.** Two operator-facing claims in
   `docs/guide-docs/guide/07-operations.md` are not implemented:
   audit-file age-based cleanup (`retention_days` is parsed but never used)
   and the claim that backup runs "без остановки сервера" (the backup code
   itself documents it as stop-and-copy requiring a stopped server).
4. **No machine-readable SBOM artifact** — license policy is *gated* in CI,
   but no CycloneDX/SPDX SBOM is generated for release evidence.

---

## Capability matrix

| # | Capability | Status | Evidence |
|---|---|---|---|
| 1a | Durable, tamper-evident audit log (HMAC chain, seq, truncation defence) | **Present** | `crates/shamir-connect/src/server/audit_chain.rs:49-74` (entry: seq, ts, event, user, ip_subnet, session prefix, prev_hmac, hmac), `:121-123` (HMAC-SHA256 per entry), `:221-224` (checkpoint for truncation defence). Durable appender: `crates/shamir-server/src/audit_appender.rs:1-22` (fixed-format `audit.log` + fjall checkpoint; strict fsync-per-entry and batched modes). Wired at boot: `crates/shamir-server/src/server/server_launcher.rs:151-156` (batched, 5 s flush), `:242-249` (chain restored from checkpoint; split-brain regression test exists). Startup verification: `audit_appender.rs:372-376` re-reads the log for `verify_chain`. |
| 1b | Audit **coverage** (DDL / ACL / admin / data ops) | **Absent** | The only production append call site is `crates/shamir-server/src/connection/handshake.rs:50` (`ctx.audit.append`, via `audit_emit` at `:40-63`) — auth events only. Grep for `ctx.audit` / `audit_writer` across `crates/` finds no other emitter. The `AuditSink` used by the admin API (`user_created` etc., `crates/shamir-connect/src/server/admin.rs:62,390-432`) has only an **in-memory Vec** implementation, used in tests. Contradicts `docs/guide-docs/guide/07-operations.md:343` ("События: аутентификация, DDL, ACL-изменения, admin-операции"). |
| 1c | Per-operation access tracing | **Partial (ephemeral only)** | `crates/shamir-types/src/access.rs:632-660` — `trace_access` is explicitly "OBSERVABILITY trace … NOT an enforcement gate"; it emits one `log::trace!` line and always returns `Ok`. Nothing is persisted; at default log levels nothing is even emitted. Read/query operations therefore leave **no durable who-did-what-when record**. |
| 1d | Audit log rotation | **Present** (size-based) | `crates/shamir-server/src/audit_appender.rs:471-527` (rotate at `max_file_size_mb`, timestamped rename); config `crates/shamir-server/src/config.rs:179-188`; e2e tests `crates/shamir-server/tests/audit_rotation.rs`. |
| 1e | Audit log age-based retention | **Absent (config knob is dead)** | `config.rs:184-187` parses `audit.retention_days` ("Delete rotated audit files older than this … Default 30 days") but grep shows **zero consumers** outside `config.rs` — no cleanup task exists. `07-operations.md:342` claims "устаревшие файлы удаляются автоматически" — false today. |
| 2a | Retention policy (per-table) | **Present** | `crates/shamir-query-types/src/admin/types/retention.rs` (SetRetention/PurgeHistory DTOs), `crates/shamir-tx/src/mvcc_store/retention.rs`, handler `crates/shamir-db/src/shamir_db/execute/admin_retention.rs`. Default is privacy-favorable `CurrentOnly` (`crates/shamir-tx/src/mvcc_store/mod.rs:200`; e2e `crates/shamir-db/tests/retention_ddl.rs:91-106`). |
| 2b | Time-based history purge | **Present** | `PurgeHistory` → `MvccStore::purge_below_ts` (`crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:375-450`): drains overlay first, removes history versions + ts keys synchronously. Tests: `crates/shamir-db/tests/purge_history.rs`. |
| 2c | Per-subject / per-key erasure primitive | **Absent** | Purge predicate is time-based only (`retention.rs:60-65` — `OlderThan` / `OlderThanAge`). Documented: `data-protection.md:153-156` ("There is no by-key, by-record-id, or by-subject purge primitive"). Erasure is an operator-composed procedure (`data-protection.md` §2.2). |
| 2d | Physical-deletion honesty (WAL / LSM lag) | **Present (documented)** | `docs/guide-docs/security/data-protection.md:229-257` — purged bytes survive in sealed WAL segments until drainer `truncate_below` and in SST files until fjall compaction; no force-compaction API; doc explicitly forbids claiming "deleted immediately and unrecoverably". |
| 2e | Interner residual | **Present (documented, unclosable)** | `data-protection.md:259-333` — the field-name interner is append-only with no prune API; PII-bearing dynamic field names cannot be erased. Tracked as a future "interner GC" gap. |
| 2f | Secondary-index residuals (HNSW / FTS) after delete/purge | **Gap — undocumented** | HNSW deletion is a **soft-delete tombstone set**; vectors of tombstoned ids remain in the adapter until compaction (`crates/shamir-index/src/vector/hnsw_adapter.rs:7-8,232,503` — tombstone internals are even iterated "for snapshot serialisation", i.e. persisted into index snapshots), compaction at `crates/shamir-index/src/backend.rs:242` + `vector/tests/compaction_tests.rs`. `data-protection.md` §2 does not mention index residuals at all — an operator following its erasure procedure would not know deleted embeddings can persist in `PersistedIndexes` snapshots until a compaction rewrite. FTS posting reclamation is similarly undocumented in the compliance doc. |
| 2g | Purge propagation to replicas | **Gap — undocumented** | Replication is a leader-side pull API (`crates/shamir-server/src/db_handler/repl_handler.rs:1-10`, gated on `is_replicator`/superuser at `:50`). Nothing propagates `PurgeHistory`/`Delete`-vacuum to already-pulled replica copies; `data-protection.md:85` only says backup/replica volumes must be encrypted. For GDPR Art. 17 the operator must purge each replica independently — nowhere stated. |
| 3 | Encryption at rest | **Absent by design — documented delegated model** | Still accurate as of today: no cipher in the storage path (`crates/shamir-storage/src/storage_fjall.rs` — verbatim `keyspace.insert`; no AES/ChaCha dep in `crates/shamir-storage/Cargo.toml`). The delegated-model doc is thorough and current: `docs/guide-docs/security/data-protection.md` §1 (verification table at `:34-39`, operator LUKS/BitLocker/CMEK requirement table at `:79-85`, "encryption obligation follows the data" incl. backups). TLS in transit via rustls is separate. The encrypted-volume requirement is **load-bearing** for the purge-latency story (`:248-257`) — an operator who skips it silently loses both protections. |
| 4a | Password storage | **Present** | Argon2id server-side before persist: `crates/shamir-db/src/shamir_db/execute/helpers.rs` (`hash_password`, cited at `data-protection.md:44-47`); SCRAM record derivation moves plaintext into `Zeroizing` immediately (`crates/shamir-server/src/db_handler/admin.rs:48-54,177-180`). Password change requires SCRAM proof-of-old-password (`admin.rs:406-572`). HMAC canonical forms for user ops **exclude** the password by construction (`crates/shamir-query-types/src/hmac.rs:40,68,182,405` + test `tests/hmac_tests.rs:124-126`). |
| 4b | Secret redaction in Debug/logs | **Present with two latent exceptions** | `SecretString` (`crates/shamir-query-types/src/auth/secret.rs:46-50` — Debug prints `SecretString(***)`; zeroize-on-drop `:67-75`) is used for `CreateUserOp.password` / `password_hash` (`auth/types.rs:150,182`). `AuditChain`'s Debug redacts its key (`audit_chain.rs:143-153`). Client passwords are `Zeroizing<Vec<u8>>` (`crates/shamir-client/src/client.rs:63`); bootstrap token is `Zeroizing<[u8;32]>` (`crates/shamir-connect/src/server/bootstrap.rs:219`). **Exception 1:** `DbRequest::CreateScramUser.password` is a plain `String` inside `#[derive(Debug)]` on `DbRequest` (`crates/shamir-query-types/src/wire/db_message.rs:27,54-62`) — no current `{:?}`-log site was found in `shamir-server/src`, but any future debug-log of a request would print the plaintext. **Exception 2:** `VectorBackendRef::External { api_key_secret: String }` under `#[derive(Debug, Clone, Serialize, Deserialize)]` (`crates/shamir-index/src/kind.rs:188-199`) — currently a dead/roadmap variant (no other reference in the tree), but if wired it would Debug-print and bincode-persist the API key in plaintext. `shamir-client-node/src/lib.rs:63` also carries `pub password: String` (napi boundary, outside the default workspace). |
| 4c | WASM function secret access | **Present (gated)** | `env.*` secrets are readable by guest functions only when named in the function's `secret_grants` allow-list (`crates/shamir-wasm-host/src/context.rs:289-304`; definer/secret-grant creation gated in `crates/shamir-server/src/db_handler/admin.rs:743-756`). |
| 5 | Backup / restore | **Partial — v1 stop-and-copy, doc mismatch** | `crates/shamir-server/src/backup.rs:1-18` — "v1: stop-and-copy. **Operator stops the server**", recursive `fs::copy` into `<to>/<timestamp>/`, refuses existing dest; CLI `shamir-server backup --to` (`main.rs:172-178`); tests `src/tests/backup_tests.rs`. **No consistency guarantee against a live server** — nothing acquires a lock or quiesces writers; a copy taken while running can capture torn fjall/WAL state. `docs/guide-docs/guide/07-operations.md:345-353` claims the opposite ("без остановки сервера … Блокирует данные на время snapshot") — dangerous doc drift. Restore-side compliance step IS specified: mandatory `revokeAllTickets` after any SystemStore restore to prevent resumption-counter rollback replay (`docs/guide-docs/client-server-protocol-spec/IMPLEMENTATION_GUIDE.md:446-457`, `SECURITY_MODEL.md:64`) — but there is no restore command that enforces it; it is a runbook obligation. Backup file comment references "redb files" while storage docs are fjall-centric (`backup.rs:5,9-14`) — stale internals note, copy-everything behaviour is still correct. No incremental/verified/encrypted backup; roadmap item (`docs/dev-artifacts/roadmap/PRODUCTION_HARDENING_ROADMAP.md:58`). |
| 6 | Multi-tenancy isolation | **Partial — logical ACL only, no isolation doc** | One process serves multiple named databases (`DbRequest::Execute { db }`, `db_message.rs:36-44`). Isolation is the POSIX-style ACL tree (owner/group/mode per Database/Store/Table — `crates/shamir-types/src/access.rs:189-201,370-426,702-715`; hierarchy doc `docs/dev-artifacts/roadmap/ACCESS_HIERARCHY.md`; extensive permission audits under `docs/dev-artifacts/audits/2026-07-10-security-permission-*`). Caveats for a tenant-isolation claim: (a) legacy/absent-meta records load as **open `0o777`** (`access.rs:203-215,266-280` — only NEW objects get `0o700` `owned_enforced`); (b) `Actor::System`/`Admin` bypass all checks (`access.rs:702-704`); (c) all tenants share one `data_dir`, one WAL/storage substrate, one process — no per-database encryption, physical separation, or resource quotas; (d) no data-residency / tenant-isolation statement exists anywhere in `docs/guide-docs/`. Fine for single-org multi-app use; not defensible for hostile-tenant or data-residency-partitioned deployments without an explicit doc. |
| 7a | Dependency license gate | **Present** | `deny.toml` (permissive-only allow-list derived from the actual tree, per-crate exceptions for `aws-lc-sys`/`webpki-roots` at `:89-104`; `unknown-registry`/`unknown-git = "deny"` at `:132-133`), enforced every push/PR by `.github/workflows/supply-chain.yml:46-60`. |
| 7b | Vulnerability advisory gate | **Present** | `cargo deny check` advisories per-push + weekly `cargo audit` (`supply-chain.yml:65-84`); `yanked = "deny"`, `unmaintained/unsound = "all"` (`deny.toml:34-40`); single triaged ignore (bincode RUSTSEC-2025-0141) with full written justification (`deny.toml:47-59`). 30-day dependency cooldown with a wasmtime security-bypass carve-out (`SECURITY.md:82-123`). |
| 7c | SBOM artifact | **Absent** | No CycloneDX/SPDX/`cargo-about` output anywhere in `docs/` or CI (grep for `SBOM`/`cargo-about` finds only the deny/audit gate). License **policy** exists; license/dependency **evidence artifact** for a release does not. Dual MIT/Apache project licensing present (`LICENSE-MIT`, `LICENSE-APACHE`). |

---

## Prioritized gap list (operator-relevant, highest first)

### P1 — Audit log covers only authentication; docs claim more
- **Gap:** the durable HMAC-chained audit log has exactly one class of
  producer — the connection handshake (`handshake.rs:50`). No DDL, ACL/chmod/
  chown, admin ops (`CreateScramUser`, `SetSuperuser`, retention/purge),
  interactive-tx, or backup events reach it. The admin-API `AuditSink` has no
  durable implementation wired. Meanwhile `07-operations.md:343` tells the
  operator DDL/ACL/admin ARE audited.
- **Why an operator cares:** SOC 2 CC7.2 / ISO 27001 A.12.4 / GDPR Art. 30
  all hinge on "who changed what, when" for privileged operations. Today a
  malicious admin can create users, grant superuser, purge history, and drop
  tables with **zero durable trace** (only ephemeral `tracing`/`log` lines).
- **Fix direction:** bridge `AuditSink` → `AuditChainWriter`; add append
  sites at the `DbRequest` admin arms and destructive `BatchOp`s (the
  HMAC-tag gate at those sites already identifies them); correct the doc
  until then.

### P2 — Backup doc contradicts implementation (consistency hazard)
- **Gap:** `backup.rs` is explicitly stop-and-copy ("Operator stops the
  server", `backup.rs:3`); it takes no lock and does not quiesce the engine.
  `07-operations.md:347-353` instructs operators to run it against a live
  server and asserts it "блокирует данные". A backup taken per the guide can
  be torn (mid-WAL-write / mid-SST-flush) and silently unrestorable.
- **Why an operator cares:** backup integrity is the backbone of every
  retention/DR compliance control; a plausibly-corrupt backup discovered at
  restore time is the worst failure mode. Also: restore does not enforce the
  MANDATORY `revokeAllTickets` step (IMPLEMENTATION_GUIDE §5.7) — it is
  runbook-only.
- **Fix direction:** fix the guide immediately (cheap); longer-term implement
  the in-process quiesced snapshot (already sketched in `backup.rs:16-18`)
  and a restore path that auto-revokes tickets.

### P3 — Erasure residuals in secondary indexes and replicas are undocumented
- **Gap:** `data-protection.md` §2 is exemplary for WAL/LSM/interner but
  silent on (a) HNSW soft-delete: deleted vectors persist in memory and in
  persisted index snapshots until compaction (`hnsw_adapter.rs:503` — the
  tombstoned internals are serialised into snapshots); (b) FTS postings
  reclamation timing; (c) replicas: a puller that copied data before a purge
  keeps it forever — no purge propagation exists in the repl protocol.
- **Why an operator cares:** an Art. 17 erasure response drafted from the
  current doc would be incomplete — embeddings frequently ARE personal data
  (face/voice/text embeddings are re-identifiable).
- **Fix direction:** extend §2.3's residual table with index-snapshot and
  replica rows; add "purge each replica independently" to the §2.2 procedure;
  consider a force-compaction admin op for vector indexes.

### P4 — `audit.retention_days` is a dead config knob
- **Gap:** parsed (`config.rs:186-187`, default 30) but no consumer; rotated
  audit files accumulate forever. The operations guide claims auto-cleanup.
- **Why an operator cares:** two-sided compliance risk — unbounded retention
  of audit data (which itself contains usernames + IP subnets = personal
  data, `audit_chain.rs:59-62`) violates data-minimisation/retention
  schedules; conversely an operator relying on the documented 30-day cleanup
  for their retention policy is misled about what is on disk.
- **Fix direction:** implement the cleanup sweep in the scheduler task, or
  delete the knob and the doc claim.

### P5 — No durable data-access log at all (reads are untraceable)
- **Gap:** `trace_access` (`access.rs:657-660`) is a `log::trace!` no-op
  gate. There is no option to durably record read access to sensitive tables.
- **Why an operator cares:** HIPAA-style and many internal-policy regimes
  require access logging on sensitive records, not just mutation logging.
  This is a feature gap, not a bug — but it caps the certifiable use cases.
- **Fix direction:** optional per-table "access-audit" flag routing
  `required_access` outcomes into the audit chain (with sampling/batching —
  the batched appender mode already exists for exactly this cost profile).

### P6 — Tenant-isolation posture undocumented; permissive legacy defaults
- **Gap:** no document states what isolation one database/tenant has from
  another in the same process; legacy/absent catalogue meta loads as `0o777`
  open (`access.rs:266-280`), and `System`/`Admin` bypass everything.
- **Why an operator cares:** data-residency and tenant-isolation claims must
  be backed by a written boundary description. Today the honest statement is
  "logical ACL isolation, shared process/storage/keyspace, open-by-default
  for legacy objects" — that sentence exists nowhere.
- **Fix direction:** a one-page `docs/guide-docs/security/tenant-isolation.md`
  in the style of `data-protection.md` (state the model, the bypasses, the
  shared surfaces, and the recommendation: one process per hostile tenant).

### P7 — No SBOM release artifact
- **Gap:** license/advisory *gating* is strong (deny.toml + CI), but no
  machine-readable SBOM (CycloneDX/SPDX) or `cargo-about` license report is
  generated or committed; nothing in docs/ inventories the ~full dependency
  tree for release evidence.
- **Why an operator cares:** procurement/regulated-industry onboarding (and
  e.g. US EO 14028-style requirements) increasingly demand an SBOM per
  release, not just a CI gate the vendor runs privately.
- **Fix direction:** add a `cargo cyclonedx` (or `cargo-about`) step to the
  supply-chain workflow emitting an artifact per tagged release. (Not
  generated in this read-only pass.)

### P8 — Latent plaintext-secret carriers (low, preventive)
- **Gap:** `DbRequest::CreateScramUser.password: String` inside a
  `#[derive(Debug)]` enum (`db_message.rs:27,62`) — safe today only because
  no code Debug-logs requests; and the dead `api_key_secret: String` in
  `VectorBackendRef::External` (`kind.rs:197`) which would be Debug-printed
  and bincode-persisted in plaintext if ever wired.
- **Fix direction:** switch both fields to `SecretString` (its serde
  pass-through keeps the wire shape unchanged by design,
  `secret.rs:52-65`).

---

## What is genuinely in good shape (for the release notes)

- **Tamper-evidence design of the audit chain** is real: per-entry
  HMAC-SHA256 over canonical bytes, prev-hmac chaining, persisted
  `(next_seq, prev_hmac)` checkpoint as truncation defence, startup
  chain verification, split-brain regression test, strict (fsync-per-entry)
  and batched modes, size-based rotation with tests.
- **`data-protection.md` is a model compliance document** — current-state
  (verified against this tree: storage path still cipher-free), honest about
  residuals, with a concrete operator erasure procedure and the load-bearing
  encrypted-volume requirement.
- **Privacy-favorable retention default** (`CurrentOnly`) with the DTO
  `Retention::default()` ≠ engine-default asymmetry explicitly documented
  and tested.
- **Credential hygiene**: Argon2id, SCRAM, `Zeroizing` plaintext handling,
  `SecretString` redaction, HMAC canonical forms that structurally exclude
  passwords, redacted `AuditChain` Debug.
- **Supply-chain gate**: strict deny.toml (near-empty ignore list with a
  written justification for the single entry), per-push license+advisory
  check, weekly time-decoupled `cargo audit`, dependency cooldown with a
  security-fix bypass policy for wasmtime.

---

*Read-only research pass; no code was modified. All lines cited are
re-verifiable with `rg`/`Read` against the tree at the time of writing.*
