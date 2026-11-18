# R5 DESIGN — the administrative shell (`shamir-server shell`)

Status: proposal (design only — no code, no tests, no dependencies in this step).
Plan source: `docs/dev-artifacts/release-1/05-admin-cli.md`. Owner brief: `docs/dev-artifacts/prompts/release/53-r5-design-admin-shell.md`. Owner plan: `docs/dev-artifacts/release-1/README.md` — the §5 release gates (`release-1/README.md:91-107`) are the acceptance frame this design must not contradict. Named upstream design refs consumed here: `design/operational-views.md` (R4b), `design/online-backup.md` (R2a backup semantics), `design/logical-dump.md` (R3 export/import), `design/engine-metrics.md` (R4a telemetry the P1 DTO presents), `design/app-schema-migrations.md` (R1c `schema-migration status`), `design/format-stamp.md` (format-stamp semantics consumed by the P1 status DTO).
Evidence key: OBSERVED = `path:line` re-opened on this branch; INFERRED = conclusion drawn from observed facts, marked as such. Long paths use the full-path aliases of §1.0; every alias is a full real path defined exactly once.

---

## 0. Decision summary

| # | Decision | Chosen |
|---|---|---|
| 1 | Packaging | `shamir-server shell` subcommand of the production binary, dispatched before config/logging (`MAIN:287-320` slot); package-root `shell/` module via the `#[path]` precedent (`MAIN:24-30`, `DHMOD:1-15`); no new crate |
| 2 | Transports | R5a PR1: TLS-TCP + IPC SCRAM only; R5a PR2 adds native WSS SCRAM and HTTPS-REST/WSS with a provisioned token. Never token or SCRAM session resume. No plaintext network transport, even loopback. Verified SCRAM pin required (no `--accept-new-host` in any stage); token auth uses WebPKI or a profile-supplied CA PEM |
| 3 | Command surface | Only commands mapped to existing or explicitly owned operations; every batch command carries the routing-DB floor; non-batch and local/explained commands carry no implicit DB/auth requirement; offline commands are explained, never executed |
| 4 | Modes/output | Batch = exactly one argv command after `--`; batch output is ALWAYS JSON-lines v1 (TTY included) — `--output-version 1` is a local assertion only and unsupported versions are rejected; human tables interactive-TTY only; no history at all in R5 |
| 5 | Safety | No client-side privilege; typed denial for token-subset and unsupported cases; destructive commands blocked before dispatch without `--yes`; no raw server error strings, argv, or handshake metadata in output |
| 6 | Dependencies | Zero new registry crates; existing workspace edges only, declared per PR |
| 7 | Testing | CI-only, in-package integration target `tests/admin/`, real binary per stage per supported transport, three-OS `workflow_dispatch` on the same head |
| 8 | Plan | Exactly six PRs (§9); PR1 starts now; PR4 after R4b steps 1–2 only (steps 3 tx/cursor and 4 shapes are out of R5); PR6 after R2d/R3 |

Rejected alternatives per decision (the numbered decisions above):

1. *Packaging* — separate executable / new crate rejected (§2).
2. *Transports* — plaintext loopback rejected (normalizes plaintext admin use); token/SCRAM session resume rejected (ticket persistence and reconnect/replay semantics are out of scope; unbound resume paths omit the identity-proof preamble, `ENTRY_WS:224-247,269-325`); `--accept-new-host` rejected (silent first-contact trust). Chosen alternative to "CA everywhere": application pin for fresh SCRAM, real CA validation for token — each transport keeps its OBSERVED model instead of inventing a new one.
3. *Command surface* — arbitrary query shell rejected (second query-construction path, plan §3); wrapping offline ops rejected except the one bounded `backup-verify` exception; tx/cursor activity views rejected for R5 (would ride PR4 without their own PR).
4. *Modes/output* — multi-command batch/file mode rejected (one command = one record, no terminator machinery); base64 rejected for the binary encoding (hex chosen, one convention); a `--output-version` server round-trip rejected (rendering is client-side only); optional placeholder successes for pending R2d/R3 ops rejected (exit 6 instead).
5. *Safety* — client-side privilege caching rejected; raw server error passthrough rejected; argv secrets rejected (process lists/history).
6. *Dependencies* — `rustyline`/`dialoguer`/`tabled`/PTY crates rejected (new registry packages, license/cooldown gates unjustified); a new shell crate rejected (§2); new `client/src/client` sibling file rejected (folder already at the 7-entry limit).
7. *Testing* — local test runs rejected (binding policy); per-PR matrices rejected (stage-deduplicated acceptance); assuming macOS coverage from ordinary PR CI rejected (manual same-head dispatch required).
8. *Plan* — letting R4b step 3 ride PR4 rejected (scope change without review); a seventh PR rejected (stage granularity suffices).

---

## 1.0 Evidence aliases (full paths, used throughout)

| Alias | Full path |
|---|---|
| `MAIN` | `crates/server/shamir-server/src/main.rs` |
| `PKG` | `crates/server/shamir-server/Cargo.toml` |
| `GATE` | `crates/server/shamir-server/src/engine/db_handler/handler.rs` |
| `DHMOD` | `crates/server/shamir-server/src/engine/db_handler/mod.rs` |
| `REPO_ADMIN` | `crates/server/shamir-server/src/engine/db_handler/repo_admin/` |
| `HANDLERS_ADMIN` | `crates/server/shamir-server/src/engine/db_handler/handlers/admin.rs` |
| `SCOPE` | `crates/server/shamir-server-security/src/security/perimeter/http_rest/scope.rs` |
| `AUTH` | `crates/engine/shamir-db/src/shamir_db/shamir_db/access/authorize.rs` (note: the doubled `shamir_db` directory segment is real, OBSERVED) |
| `META` | `crates/engine/shamir-db/src/shamir_db/shamir_db/access/meta.rs` |
| `POLICY` | `crates/foundation/shamir-types/src/entries/access/policy.rs` |
| `DESCRIBE` | `crates/engine/shamir-db/src/shamir_db/execute/admin_meta/admin_describe.rs` |
| `LIST` | `crates/engine/shamir-db/src/shamir_db/execute/admin_meta/admin_list.rs` |
| `ACCESS` | `crates/engine/shamir-db/src/shamir_db/execute/admin_meta/admin_access.rs` |
| `DURAB` | `crates/engine/shamir-db/src/shamir_db/execute/repo_admin/admin_repo_durability.rs` |
| `DBEXEC` | `crates/engine/shamir-db/src/shamir_db/execute/repo_admin/db_execute.rs` |
| `BATCHOP` | `crates/query/shamir-query-types/src/ops/batch/ops/batch_op.rs` (992 physical lines, OBSERVED `wc -l`) |
| `DBMSG` | `crates/query/shamir-query-types/src/wire/db_message.rs` (537 physical lines, OBSERVED `wc -l`) |
| `HMAC` | `crates/query/shamir-query-types/src/security/hmac.rs` |
| `CHMOD_B` | `crates/query/shamir-query-builder/src/ddl/schema/config/access_builders/resources/chmod.rs` |
| `Q` | `crates/query/shamir-query-builder/src/read/query/query.rs` (419 lines) |
| `PAGED` | `crates/engine/shamir-engine-table/src/read/dispatch/paged.rs` |
| `RID` | `crates/foundation/shamir-types/src/types/ids/record_id.rs` |
| `ENTRY` | `crates/client/shamir-client/src/client/session/entry.rs` |
| `ENTRY_WS` | `crates/client/shamir-client/src/client/session/entry_ws.rs` |
| `ENDPOINT` | `crates/client/shamir-client/src/client/transport/token/endpoint.rs` |
| `TLOGIN` | `crates/client/shamir-client/src/client/handshake/token/login.rs` |
| `DIAL` | `crates/client/shamir-client/src/client/transport/dial.rs` |
| `TOKENHTTP` | `crates/client/shamir-client/src/client/session/token/http_client.rs` |
| `TOKENTLS` | `crates/client/shamir-client/src/client/transport/token/tls.rs` |
| `HANDSHAKE` | `crates/net/shamir-connect/src/client/handshake.rs` |
| `IPCNET` | `crates/net/shamir-transport-ipc/src/lib.rs` |
| `TCPNET` | `crates/net/shamir-transport-tcp/src/listener.rs` |
| `WSNET` | `crates/net/shamir-transport-ws/src/listener.rs` |
| `HTTPNET` | `crates/net/shamir-transport-http/src/server/service.rs` |
| `TSDOC_T` | `crates/client/shamir-client-ts/docs/TRANSPORT_SPEC.md` |
| `TSDOC_K` | `crates/client/shamir-client-ts/docs/TOKENS.md` |
| `API` | `crates/client/shamir-client/src/client/api.rs` (918 physical lines, OBSERVED `wc -l`) |
| `BSTDIN` | `crates/server/shamir-server-security/src/security/service/bootstrap_stdin.rs` |
| `FOLLOW` | `crates/server/shamir-sync/src/sync/replication/follower_loop/metrics.rs` |
| `SEL` | `scripts/ci/matrix/selection.py` |
| `CI` | `.github/workflows/ci.yml` |
| `WS` | root `Cargo.toml` |
| `LOCK` | root `Cargo.lock` |

CI / workspace facts:

* Workspace members are the glob `crates/*/*` plus `vendor/hnsw_rs`; excluded from the workspace: `crates/client/shamir-client-ts` (pure TS, no Cargo.toml) and `tools/rust-guard` — OBSERVED `WS:1-14`.
* CI computes affected crates/targets from the nearest owning package per changed path (`SEL:42-63`) and classifies `tests/**` trees against declared `[[test]]` targets (`SEL:126-154` `role`, including the `main.rs` companion-tree rule). The `admin` integration target is declared at `PKG:214-218` (`[[test]] name = "admin" path = "tests/admin/main.rs"`).
* PR CI matrix for the integration job is **Ubuntu + Windows only**; **macOS runs only on `workflow_dispatch`/`schedule`** — OBSERVED `CI:706-715` (ternary on `github.event_name`), test selection/invocation `CI:769-781`. Three-OS acceptance therefore requires a computed-scope `workflow_dispatch` on each stage head (or at least the final changed head), verifying via job logs that the `admin` target was selected on all three OSes on the same head.
* Dependency versions in the lock: `clap` (`LOCK:666`), `ktav 0.8` (`PKG:76`), `serde`/`serde_json` (`LOCK:3319`), `zeroize` (`LOCK:5864`), `base64` (`LOCK:381`), `hex` (`LOCK:1660`), and two `rustix` / two `windows-sys` graph nodes (`LOCK:3103,3116`; `LOCK:5628,5637`) — OBSERVED.
* Physical line counts that cap file growth: `GATE` and `BATCHOP` are each **992** physical lines; `DBMSG` **537**; `API` **918** (OBSERVED `wc -l`). The project's practical cap is ~1000 lines/file (`scripts/ci/check_layout.py:83-90`, `FILE_LIMIT = 1000`, `FOLDER_LIMIT = 7`).
* `tests/admin/` currently has **5 entries** (`identity/`, `logical_dump/`, `logical_import/`, `main.rs`, `ops/`) — OBSERVED `ls`; room for exactly one more directory.
* `crates/client/shamir-client/src/client/` currently has exactly **7 entries** (`api.rs`, `cursor_stream.rs`, `handshake/`, `mod.rs`, `session/`, `subscription/`, `transport/`) — OBSERVED `ls`; the layout gate (`check_layout.py:83-90`, `FOLDER_LIMIT = 7`) leaves **no room for a new sibling file there** — PR3 edits must stay inside `api.rs` (918 lines) or the planned `shell`/`REPO_ADMIN` homes.
* `REPO_ADMIN/` currently has **4 entries**; `DHMOD:1-15` wires them as `#[path]` modules — the exact precedent the plan reuses (`DHMOD:4-6,8-14`). `GATE`/`BATCHOP` at 992 lines must not grow by a new op variant; `DBMSG` has room.

---

## 1. OBSERVED facts (evidence)

Server binary and dispatch:

* `MAIN` is a clap app with an optional subcommand enum; without a subcommand (or with `run`) it boots the server. Subcommands today: `run`, `logical-export`, `logical-import`, `replication-seed-{export,verify,install}`, `backup`, `restore`, `access-tree`, `service`, `token`, `format-migrate`, `doctor` — OBSERVED enum `MAIN:89-246` (`Restore { from, force }` variant at `MAIN:150-173`; required `--config` on `Cli` at `MAIN:44-48`).
* `format-migrate` is dispatched **before** logging init on purpose (`MAIN:287-320` early dispatch in `run_main`); every other subcommand is dispatched after config load + logging init (`MAIN:323-355`), including offline dump/import (`MAIN:356-400`) and backup/restore (`MAIN:443-470`); `doctor` args/dispatch at `MAIN:217-245,510-527`. A new `shell` subcommand that must not touch server config/logging needs its own early-dispatch slot beside `format-migrate`; the same early slot is where the `backup-verify` wrapper goes (§4.7).
* The `#[path]` cross-package module mechanism is already used by the binary: `MAIN:24-30` (`bootstrap_cli_tests.rs`, `bootstrap_stdin.rs` from `shamir-server-security`). `DHMOD:1-15` uses the identical mechanism inside `db_handler` for the four `REPO_ADMIN` files.
* Offline `restore` and `backup` exist as server subcommands with manifest verification and pre-restore preservation; `Restore { from, force }` is at `MAIN:150-173` (OBSERVED). The argv-password warning precedent ("Avoid argv: it may be exposed in process lists and shell history") is at `MAIN:54-68`.
* Offline `doctor` (verify + optional `--apply` repair, offline only) exists (`MAIN:217-245,510-527`).
* Offline logical export/import exist with their exact flags (`logical-export --db .. --output .. [--repo .. --table ..]`; `logical-import --input .. --target ..`) — dispatch arms re-checked at `MAIN:356-400` (OBSERVED).
* The server keeps a plain WebSocket listener surface (`WSNET` `PlainWsLoopback`); the shell never uses it (§3.2).

Online backup is wired end to end on this branch:

* Production launch constructs the per-server `OnlineBackupRegistry` and installs it as the engine's `OnlineBackupAdminPort` (`crates/server/shamir-server/src/engine/server/server_launcher/launch.rs:429-441`).
* The engine dispatches `StartBackup` / `BackupStatus` / `CancelBackup` through the node-local port (`crates/engine/shamir-db/src/shamir_db/execute/core/admin_dispatch.rs:226-269`); these are **node-local, superuser-only, not database-scoped** (`BATCHOP:209-211` and the server bridge).
* `CancelBackup` has **no id parameter** — it targets the current run on the node (`crates/server/shamir-server/src/boot/config/diagnostics/backup_online/registry.rs:211-241`); `StartBackup` supplies a server-local destination root (`registry.rs:153-165`).

Observability and status sources:

* `/info` exposes only uptime, bound addrs, ready flag, visibility stalls, operator bind opt-in (`crates/server/shamir-server-security/src/security/observability/mod.rs:640-672`).
* The admin op `ReplicationStatus` returns **subscription records**: identities, state, the upstream bookmark, the next pull version, the local consistent version and certified gap endpoints (`crates/engine/shamir-db/src/shamir_db/execute/replication_seed/status.rs:28-83`); runtime upstream lag and applied-LSN against a live head are explicitly a later task (`crates/engine/shamir-db/src/shamir_db/execute/admin_db/admin_replication.rs:760-782`). None of these fields is a live upstream lag.
* WAL/durability telemetry exists as Prometheus gauges/counters, not as an admin record (`crates/server/shamir-server-security/src/security/observability/durability_export.rs:7-51`).
* The format stamp is reachable in-process via a `ServerHandle` getter (`crates/server/shamir-server/src/engine/server/server_handle.rs:171-175`, captured at `launch.rs:140-145`) — but the getter yields a **`StampOutcome` enum**, not a decoded epoch: `Created`/`Adopted`/`Unchanged`/`VersionUpdated`/`InspectedStampless`/`InspectedOlder` (`crates/server/shamir-sync/src/format_stamp/check/stamp_outcome.rs:6-20`, OBSERVED). The decoded epoch lives in the stamp model (`crates/server/shamir-sync/src/format_stamp/model/format_stamp.rs:90`, `format_epoch()`); the on-disk stamp is classified by `inspect` (`crates/server/shamir-sync/src/format_stamp/io/inspect.rs:26+` — `Valid(FormatStamp)` / `Absent` / `Corrupt`, fail-closed). Consequence for P1 (§4.2): the **server launcher must capture the verified on-disk stamp during boot — read via `inspect` while holding the boot lock** — and expose epoch + outcome + monotonic uptime from that capture; the shell never reads the disk and never relies on the bare getter or a process-local constant.
* Replication follower telemetry: per-stream cells with `lag_versions`, `observed_at`, `last_receipt_ns`, `acknowledged_bookmark`, `state_code` (`FOLLOW:31-55`); aggregated snapshot registry (`FOLLOW:396-408`); updater stores `upstream_head − bookmark` and receipt time (`FOLLOW:430-444`). **Nuance (OBSERVED): `observed_at` is a `NEXT_OBSERVATION` monotonic sequence number (`FOLLOW:397,431-437` `fetch_max` over an incrementing `AtomicU64`), not wall-clock time** — it carries no sample age. `last_receipt_ns` is a receipt instant, not a lag measure. Consequence (§4.2): the status DTO must either expose **nullable sample age backed by new coherent owner-timestamp plumbing added in `metrics.rs`** (a wall-clock field set alongside `set_lag`/`note_durable_progress`), or expose the **observation sequence plus `age: unknown`**; it must not present `observed_at` as a timestamp. Version gap is nullable; receipt age is only "age since last receipt" and may be unknown; inactive/untracked/unknown are distinguishable from zero.
* The v2 backup/restore *library* verify exists but has **no CLI**: `verify_manifest` (report-only) and `verify_manifest_for_restore` live in `crates/server/shamir-sync/src/sync/durability/backup/offline.rs:552-580`. No `verify` subcommand exists in `MAIN:89-246` (checked the enum). Verify is **not cheap**: it hashes snapshot content.
* Offline `restore` verifies, stages, and replays WAL before the atomic swap (`crates/server/shamir-sync/src/sync/durability/backup/restore.rs:228-272,324-337`). Offline `doctor` may perform recovery and touches the registry (`crates/server/shamir-server/src/boot/config/diagnostics/doctor.rs:214-280`); the underlying verify is a full streaming scan that can see transient miscounts on a live store (`crates/engine/shamir-engine-table/src/table_manager/diagnostics/doctor.rs:200-217,268-283`) — integrity diagnostics are offline-only.
* `DescribeRepo` (effective vs stored durability) is at `DURAB:182-197` and `DURAB:222-252`. It is a repo durability command — **not** evidence for `describe TABLE`.

Authorization:

* Server admin gate + allowlist: `GATE:660-681` (superuser coarse block over all admin ops with an exemption list). The authoritative allowlist is `HANDLERS_ADMIN:616-629`: exactly **five** gate-exempt ops — `List`, `AccessTree`, `DescribeTable`, `ApplicationSchema`, `GetTableSchema` (OBSERVED; the comment just above the function at `GATE:~610-615` still says "4-op exemption" — **stale**, the actual `is_coarse_admin_gate_exempt` includes `ApplicationSchema`). Every admin call passes through this gate; there is no client-side privilege path.
* **The batch permission floor applies to every command that rides an existing batch**, read-only included: the routing DB must exist and the caller must hold `Read` on it (`crates/engine/shamir-engine/src/query/batch/control/authorized.rs:84-114`); the selected DB must exist even for server-global ops (`DBEXEC:123-144`). **Non-batch commands (top-level `DbRequest` variants such as ping and the P1/P2 variants) and local/explained commands do NOT implicitly need database access or auth** beyond their own contract.
* Catalogue ACLs: Root List, DB List, repo/store List, Table List, FunctionNamespace List — `LIST:28-112,113-209`.
* `user list` is `ListOp::Users` (the enum **variant** — not a `List(Users)` permission) and its handler authorizes **`Root` + `Action::Manage`** (`LIST:76-112`, OBSERVED); it degrades to typed `not_supported` when no principal resolver is configured.
* Describe/schema read require `Read(table)` (`DESCRIBE:37-53`; `crates/engine/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:932ff`). `DescribeTable`'s response carries `owner`, `group`, `mode` plus schema/indexes/validators/retention/buffer — `DESCRIBE:215-232`.
* DB read authorization, including ancestor `Execute` traversal and fail-closed catalogue errors: `AUTH:44-115`. ACL policy model: `POLICY:3-53`. Root default mode is `0o751` (`META:214-220`) — an ordinary principal cannot list Root children; describe tests must target a scoped DB.
* `AccessTree` (depth, db filter): `ACCESS:558-594`; traversal and DB-filter semantics — hierarchy-only; function namespaces and principals remain global — `AUTH:453-602`. Tree filtering is local presentation and does not narrow server enforcement.
* `Chmod`/`Chown`/`Chgrp`: coarse superuser + Manage on the target path + per-session HMAC tag (`ACCESS:63-70,125-172,228ff`); HMAC gate is a separate batch-gate pass (`GATE:713-722`, `check_destructive_hmacs`). Canonical-bytes v2 helpers bind the **exact typed resource** per op: `HMAC:453-485` (`canonical_batch_op`), session-key derivation `HMAC:800-823` (`derive_session_hmac_key`, `compute_tag_hex`, `verify_tag_hex`), policy classification `HMAC:329`. **These canonical/derive/verify helpers are public, but the builder does not sign**: `CHMOD_B:6-39` constructs `Chmod { hmac: None }` and only `build()` fails with `BuilderError::MissingHmac` unless the caller attaches an **externally supplied** tag via `.hmac(tag)` (OBSERVED). The shell's PR4 therefore adds a **small typed builder + client signing seam** that reuses the existing canonical helpers (`HMAC:453-485,800-823`); no signing helper exists in the builder today, and this design does not claim one does.
* **Token perimeter**: token scopes are a *write-capable perimeter under their scope*, classified op-by-op (`SCOPE:97-134`, `SCOPE:141-184,190-261`). The shell **voluntarily exposes only a narrower read-only subset** over token sessions and refuses everything else client-side with a typed denial. New top-level `DbRequest` variants are exhaustively matched by `classify_request` (`SCOPE:34-61,97-134`) — every added variant must be given an explicit token decision.
* HTTP username/password login **mints a reusable API token**, not a session (`TLOGIN:22-23,76-125`; TS mirror `TSDOC_K:194-206`).

Data views:

* `BatchOp::Read(ReadQuery)` exists (`BATCHOP:55-56`); a count is a normal `Read` with a `select::count_all` item (`crates/query/shamir-query-builder/src/read/select/select_item.rs:80-88` via `Query::select`). `Query::count_total` exists (`Q:241-245`) but is **not used** by the shell count (separate expensive flag).
* `Query::with_repo(repo, table)` (`Q:55-57`) and `Query::limit(n)` (`Q:159-172`) exist. The streaming SELECT+LIMIT path fetches up to `limit + 1` and stops — `PAGED:185-203`; rows are streamed and byte-metered with no synthetic `_id` on ordinary rows — `PAGED:395-426`. This is the audited bounded route for `data head` (§4.4).
* Typed record keys are `RecordId`, Base58-encoded, `FromStr`-decodable — `RID:157-173`. A typed exact-key engine point-read primitive exists in the engine, not on the wire (`crates/engine/shamir-engine-table/src/table_manager/operations/table_manager_crud.rs:694-714`); `where_eq('_id')` filters the payload and is NOT a key lookup. Exposing it is prerequisite P2 (§4.11), designed as a **top-level `DbRequest` variant**, reusing exactly the DB/table/ancestor authorization of `AUTH:44-115`.
* `ORDER BY _id` snapshot reads materialize and sort all matching rows **before** the limit is applied (`crates/engine/shamir-engine-table/src/read/dispatch/scan.rs:239-278`) — not the `data head` route.

R1c status: the client `status` requires a compiled migration set; the raw journal path is private (`crates/client/shamir-client/src/receipts/schema_migration/status.rs:42-80,94-107`). The server-side `ApplicationSchema` Status sub-op reads the DB directly and reports a real id on mismatch (`crates/engine/shamir-db/src/shamir_db/execute/admin_db/schema_migration/schema_migration.rs:65-119,133-155`).

Not found on this branch (checked with `rg` over `crates/`): `ListActivity` (R4b step 1, in implementation), per-request `CancelRequest` (R4b step 2), transaction/cursor views (R4b step 3), shape statistics (R4b step 4), backup schedule state (R2d), online export operation (R3).

Audit: existing coverage is narrow, not universal (`REPO_ADMIN/repo_audit.rs:74-111`); the server audit prefix is sensitive and must not be exposed by the shell.

Secret input precedent: `BSTDIN` reads exactly one line from a `BufRead`, caps at 4096 bytes, rejects empty/non-UTF-8, and zeroizes on every path (`BSTDIN:1-60`) — the shell reuses this bounded-reader pattern. The server warns against argv passwords (`MAIN:54-68`).

Transport/session evidence (Rust client, OBSERVED):

* Full secure session: `ENTRY:68-109` (`connect` / `connect_until`, absolute-deadline handshake with phase tracking) and `ENTRY:112-163` region (`connect_host`, per-address attempts); IPC: `ENTRY:204-237` (`connect_local`, IPC→SCRAM, no TLS, same SCRAM flow, trusted pin / `accept_new_host` options, zeroed channel binding); TLS ticket resume: `ENTRY:239-291` (`resume`, `resume_secure` — identity-proof preamble, then ticket disclosure; a resumed ticket is a **session resumption mechanism, not a bearer token**).
* Native WSS: `ENTRY_WS:56-84` (`connect_ws`, TCP→TLS→WS-upgrade→SCRAM, same options/trust model as TCP); WSS ticket resume `ENTRY_WS:142-181` (`resume_ws`/`resume_ws_secure`); plain TCP loopback `ENTRY_WS:86-109,269-310` (`connect_plain`/`resume_plain` — loopback-only refusal before any socket, SCRAM still runs, application pin still applies — **plain is not pin-less**); browser profile `ENTRY_WS:183-265` (`connect_ws_browser` — native Rust API, full SCRAM + `root`/`Origin` trust options + application pin; a **native** browser-profile path, not WASM and not token); IPC resume `ENTRY_WS:312-345` (`resume_local`).
* Token: native token WS `ENTRY_WS:347-397` (`connect_token_ws` — `GET /shamir/v1/ws` on a `kind: http` listener, waits for the bind frame, TLS WebPKI, admin ops refused locally with `token_request_denied`); native HTTPS REST authenticates on the **first request** (ping verifies on connect); `ENDPOINT:22-41` — `TokenEndpoint::https` (SNI + certificate checks, `TokenTrust`) and `TokenEndpoint::plain_loopback` (refused unless loopback). Native WSS token uses the same endpoint and TLS configuration.
* Wire/handshake verification core: `HANDSHAKE:252-301` (SCRAM mutual auth → pin check → Ed25519 identity verify → TOFU pin capture on first contact).
* Net-crate listener surfaces: IPC `IPCNET:1-22` (Unix socket / named pipe, one API, `binding_mode = 0x00`, OS boundary enforced by `IpcListener::bind`); TCP `TCPNET:20-44` (`ListenerProfile::{TlsExporter, TlsNoExport, Plain}`; Plain loopback-only); WS `WSNET:19-43` (`WsListenerProfile::{Wss, WssBrowser, PlainWsLoopback}`); HTTP `HTTPNET:43-95` (login routes `/shamir/v1/auth/{begin,finish}`, WS routes `/shamir/v1/ws[/browser]`, REST `/shamir/v1/{request,ping}`, auth gate on every request).
* HTTP SCRAM-style proof login issues a token only (`TLOGIN:22-23`; `TSDOC_K:194-206`) — explicitly **deferred** for the shell (§3.2).
* TS client mirrors (browser first-frame token etc.): `TSDOC_T:15-19` (browser WSS connect, `wss://…/shamir/v1/browser`), `TSDOC_T:88-95` (TOFU pinning in the shared session layer, `acceptNewHost: false`, `trustedPin()`), `TSDOC_K:115-143` (token scope/capability table, deny-by-default request allowlisting, four introspection leaves), `TSDOC_K:194-206` (password login mints a scoped bearer token, not a SCRAM session).

---

## 2. Decision 1 — Packaging

**Chosen:** `shamir-server shell` — a subcommand of the existing production binary, dispatched **before** config load, logging init and any runtime boot, in the `format-migrate` early-dispatch slot (`MAIN:287-320`).

**Module home:** package-root `shell/` folder (`crates/server/shamir-server/shell/`), reached from `src/main.rs` with literal `#[path = "../shell/…"]` declarations — the verified mechanism of `MAIN:24-30` and `DHMOD:1-15`. Package root goes 4 → 5 entries. Inside `src/engine/db_handler/`, `REPO_ADMIN` goes 4 → 5 via the same `#[path]` precedent in `DHMOD`. No folder is reorganized.

**Group arithmetic:** the `client` group holds 2 Rust members plus the TS directories; `devtools` holds 3 — room remains in both (`CLAUDE.md:30-53` group table). A new crate is nonetheless rejected: it adds a workspace member, a CI target and release surface for glue that ships with the server. INFERRED build-time effect, stated carefully: sharing the server's existing `shamir-client` edge keeps one compiled instance in the normal graph, but this design **does not claim a guaranteed single compile, a measured size delta, or a doubled-CI effect** — shared-dependency compilation counts and binary size are Cargo/CI graph properties, not something a design asserts. Zero new registry crates either way (§7).

Rejected alternatives: **separate executable** — breaks the one-binary, < 50 MB, no-external-runtime goal (`CLAUDE.md:18-23`); **new crate in `client`/`devtools`** — new member/CI target for no isolation benefit. Rejected.

Explicit non-goals: the shell never reads the data directory, never opens the server config file, and never gets an in-process LAN bypass — it is a plain remote client with a profile. The only exceptions are the commands that are *explained* (§4.7–4.10) and the manual `backup-verify` wrapper, which is a separate top-level subcommand, not shell functionality (§4.7).

**`--config` handling (no pre-existing boot behavior change):** `--config` is **required today** on `Cli` (`MAIN:44-48`) and every existing dispatch path keeps that requirement and validation exactly as-is. The two new early-dispatch paths — `shell` and `backup-verify` — are the only paths that do **not** require it (they never boot a server); CI tests prove this by running both against a machine with no readable server config and asserting they never open it (no server-config read, byte-scan of the config path in strace-equivalent CI logs is not claimed — the proof is behavioral: both run with the flag absent and behave identically).

---

## 3. Decision 2 — Connection and authentication

### 3.1 Full transport inventory of the Rust client today (OBSERVED)

| # | Transport | Session class | Trust model | Evidence |
|---|---|---|---|---|
| T1 | TLS TCP (native), `connect`/`connect_host` | full secure session (SCRAM) | application pin (Ed25519 identity TOFU verify), no CA bypass | `ENTRY:68-109,112-163`; `HANDSHAKE:252-301`; `TCPNET:20-44` |
| T2 | TLS TCP ticket resume (`resume`/`resume_secure`) | session resumption of a prior session (SCRAM TICKET — not a bearer token) | as T1 | `ENTRY:239-291` |
| T3 | Native WSS `connect_ws` / `resume_ws[_secure]` | SCRAM / ticket resume | application pin | `ENTRY_WS:56-84,142-181`; `WSNET:19-43` |
| T4 | Local IPC (Unix socket / named pipe) + SCRAM, incl. ticket resume | full secure session | local OS boundary, `binding_mode = 0x00`, application pin still applied | `ENTRY:204-237`; `ENTRY_WS:312-345`; `IPCNET:1-22` |
| T5 | `connect_plain` / `resume_plain` (loopback-only plain TCP) | unencrypted session; **SCRAM still runs and the application pin still applies — it is not trust-less** | loopback-only refusal before any socket | `ENTRY_WS:86-109,269-310` |
| T6 | Browser-profile WSS (`connect_ws_browser`/`resume_ws_browser`) | full SCRAM | conventional roots (`trust`) + `root`/`Origin` + application pin; **native Rust API** — not WASM, not token | `ENTRY_WS:183-265`; `TSDOC_T:15-19,88-95` |
| T7 | Native token: HTTPS REST (auth on first request; ping verifies on connect) + native token WSS (`GET /shamir/v1/ws`) | API token | WebPKI (`TokenTrust`) or profile-supplied CA PEM; plain-loopback HTTP refused by the SDK unless loopback | `ENTRY_WS:347-397`; `ENDPOINT:22-41`; `HTTPNET:43-95` |
| T8 | HTTP SCRAM-style proof password login | **mints a reusable API token**, not a session | WebPKI/CA | `TLOGIN:22-23,76-125`; `TSDOC_K:194-206` |
| — | TS browser first-frame token (`/shamir/v1/ws/browser`) | TS-only capability | Host/Origin allowlists | `TSDOC_K:115-143`; `HTTPNET:43-95` |

Plain WS (`WSNET` `PlainWsLoopback`), token-over-IPC and token-over-plain-TCP are **refused by the shell** (§3.2); the Rust plain-loopback constructors exist (T5) but an admin tool must not normalize plaintext.

### 3.2 What the shell supports, per stage

| Stage | Supported | Refused, exact refusal message (typed denial, §5) |
|---|---|---|
| R5a PR1 | T1 TLS-TCP + SCRAM; T4 IPC + SCRAM. Verified pin required | plaintext (T5): `refusing plaintext admin connection; use TLS TCP, WSS, or local IPC`; token: `API tokens require HTTPS REST or the native token WSS listener` |
| R5a PR2 | T3 native WSS + SCRAM; T7 token (HTTPS REST + native WSS, provisioned token, read-only shell subset — §3.3) | token outside subset: `token sessions expose only the read-only shell subset; log in with SCRAM for this command` |
| R5b/R5c | rights/activity/backup/versions — SCRAM sessions only | same token and plaintext refusals |
| Deferred (all stages) | T6 browser profile (browser SCRAM), browser-token, T8 HTTP password login, TS first-frame token, any session resume | `browser profile and HTTP password login are deferred; HTTP login mints a reusable API token, not a session`; `session resume is never used by the admin shell; log in fresh` |

Rules:

* **No plaintext network transports**; IPC is intentionally local with no TLS (IPC over a local socket is not a network transport); **never resume in any stage**: no automatic reconnect — a connection, when needed, uses fresh SCRAM or validation of a provisioned token. T2/T3-resume are client-library features the shell does not engage. Note (OBSERVED): the bound resume paths over TCP/TLS and native WSS **do verify the resume identity proof** (`ENTRY:239-248`; `ENTRY_WS:142-162`); only the unbound browser/IPC/plain resume paths skip the proof preamble (`ENTRY_WS:224-247,269-325`) — one more reason the shell refuses all resume.
* **Verified SCRAM pin only**: the profile stores the captured `SHA256(server_pub_key)` pin; a missing/mismatched pin is a hard refusal. **`--accept-new-host` does not exist in any R5 stage** (simplicity and safety: enrollment is a profile-management step, out of band). No CA bypass for SCRAM transports.
* **How each supported connection validates (OBSERVED, exact):** a **fresh SCRAM session over TLS TCP or native WSS** is validated by the **application pin** (Ed25519 identity verification against the configured pin — `HANDSHAKE:252-301`, dial path `DIAL:35-61`); it is **not PKI/CA validation** and this design must not imply otherwise — **the shell offers no TLS verification bypass**, and no CA decision is made for native SCRAM. A **token session** is validated by **CA certificate verification** — WebPKI roots or a profile-supplied CA PEM (`TOKENTLS:11-39` `TokenTrust`, `TOKENTLS:43-69` `build_connector`, always full rustls certificate validation, TLS 1.3 only) **plus** the server-side token allowlist enforced per request (`TOKENHTTP:18-51`: the token moves into a sensitive header, `check_request_allowed` refuses non-allowlisted requests locally before anything is sent). **IPC** has no TLS: the OS boundary (Unix socket / named pipe, `binding_mode = 0x00`, `IPCNET:1-22`) is the boundary, with the same SCRAM + application pin on top.
* **Token auth TLS**: WebPKI trust or an explicit profile path to a CA PEM (`ENDPOINT:22-41` `TokenTrust`); no token over IPC or plain anything.
* **Timeouts (bounded defaults, no infinite waits):** connect **10 s**, total handshake **30 s**, per-command **60 s** — each an outer deadline that wraps the whole step including IPC dial and DNS resolution; on expiry the outcome-after-send rules of §6 apply. The prompt-facing decline path is a refusal, not a hang: an interactively **declined confirmation exits with the typed-refusal code (6)**, and the guide documents that a declined prompt is exactly this outcome.
* **Token sessions expose exactly this subset — nothing else** (§3.3).
* **Profiles** (`--profile NAME`): strict **Ktav `.ktav`** files, **only allowlisted fields** (address, transport, username, pin, SNI/CA path, routing database — never secrets). The routing database field accepts only plain names with no separators — no URLs and no auth-header secrets. Default location: `$XDG_CONFIG_HOME/shamir-shell/profiles` or `$HOME/.config/shamir-shell/profiles` on Unix; `%LOCALAPPDATA%/ShamirDB/shell/profiles` on Windows. Security: Unix — owner-only `0700` directory and `0600` files; Windows — a protected current-user-only DACL on the directory and on each leaf file. Verification **fails closed** on symlink/reparse-point/link or wrong owner/ACL; no permission "fix-up" fallback. First release ships **`profile list` and `profile show` only** (both local, no secret ever displayed); `profile add`/`remove` are **dropped from the first release** — provisioning is manual (create the file with ordinary tooling).
* **Secret entry, no echo, never argv/history/output:**
  * The **hidden TTY prompt exists only in interactive mode, or in batch via an explicit `--secret-prompt` opt-in**; plain batch (no `--secret-prompt`, no TTY) must supply the secret via `--secret-env` or `--secret-fd`/`--secret-stdin` — it never silently prompts.
  * The secret **sources are mutually exclusive**: at most one of prompt / `--secret-stdin` / `--secret-fd` / `--secret-env` may be given; a TTY is **refused as** `--secret-stdin`/`--secret-fd` input (use the prompt interactively), with a fixed message.
  * Every source reads through the **same bounded reader** (4096-byte cap, `BSTDIN:1-60` pattern); overflow is a typed error, never a truncated secret.
  * Hidden TTY prompt with echo disabled (Unix termios/fd via the locked `rustix`; Windows console API via `windows-sys`), terminal mode restored on every exit path.
  * `--secret-stdin`: reads the secret from inherited fd 0 on **both** OSes (non-TTY included), no echo; used exactly once.
  * `--secret-fd N`: **Unix only** — reads from an already-open file descriptor, duplicating it safely and closing only the duplicate; on Windows this flag is **explicitly refused** with a pointer to `--secret-stdin`.
  * `--secret-env NAME`: allow-listed to exactly the auth-matched fixed variable (`SHAMIR_SHELL_PASSWORD` for SCRAM, `SHAMIR_SHELL_TOKEN` for token); the documented risk (environment variables are visible to same-user processes) is stated in the guide. New variable names, proposed by this design.
  * `--password`/`--token` on argv are **refused** with `secrets on the command line are refused; use --secret-stdin, --secret-fd, or --secret-env` (mirrors `MAIN:54-68`). No echo of a failed secret read; no fallback paths.
* **History: none.** Persistent history does not exist in R5 (§5).
* Shell/server config split: the shell never loads or forwards the server's `--config`; its only local files are its own profile directory. (`--config` stays a **required, validated** argument on every pre-existing dispatch path — see §2.)

### 3.3 The token read-only subset (exact)

Over a token session (`--auth token`, PR2+), the shell sends **only** these operations; everything else gets the typed `token_request_denied` refusal (exit 4):

* `ping`;
* `db list` — **only when the server both accepts the unscoped token at the gate and the token principal holds Root ACL List** (`LIST:28-112`; the server, not the shell, enforces this — the shell merely does not send more);
* `repo list`, `table list`, `index list`, `function list`;
* `table describe`, `table schema`;
* `data head`, `data count`.

Explicitly **not** in the subset (each refused with the fixed message above): `repo describe`, `user list`/`group tree`/`rights show`/`chmod` family, `data get` (P2 variant), `status`/`version show` (P1 variant), `replication subscriptions`, backup commands, schedule/online-export commands, and everything explained-only.

Refusal message inventory (typed denial, §5 — the shell never invents per-case wording):

| Situation | Exact fixed message |
|---|---|
| token on TLS TCP (SCRAM transport) or IPC or plain | `API tokens require HTTPS REST or the native token WSS listener` |
| token command outside §3.3 | `token sessions expose only the read-only shell subset; log in with SCRAM for this command` |
| plaintext connection attempt (incl. loopback T5) | `refusing plaintext admin connection; use TLS TCP, WSS, or local IPC` |
| browser profile / HTTP password login | `browser profile and HTTP password login are deferred; HTTP login mints a reusable API token, not a session` |
| session resume requested | `session resume is never used by the admin shell; log in fresh` |
| missing pin in profile | `SCRAM server identity pin is required; configure a verified pin in the profile` |
| `--secret-fd` on Windows | `--secret-fd is Unix-only; use --secret-stdin` |
| `--password`/`--token` on argv | `secrets on the command line are refused; use --secret-stdin, --secret-fd, or --secret-env` |

**Validation distinction (restated):** fresh SCRAM over TLS TCP/WSS = mutual SCRAM + **application pin** (not PKI); token over HTTPS/WSS = **real CA validation** (`TOKENTLS:43-69`) + allowlist; IPC = OS boundary + SCRAM + pin, no TLS, no CA.

---

## 4. Decision 3 — Command surface

Legend: **Op** = the server operation invoked; **Perm** = observed permission; **D** = destructive; stage ∈ R5a/R5b/R5c. Rows are split whenever stage or rights differ. Every command that rides an **existing batch** carries the routing-DB floor `Read(routing DB)` (`authorized.rs:84-114`) — noted once here, not repeated per row. Top-level `DbRequest` variants and local/explained commands carry **no** implicit floor; their auth requirement is stated in the row.

### 4.1 connection (R5a)
| Command | Op | Perm | D |
|---|---|---|---|
| `connect` / `--profile` use | client connect + SCRAM login (PR1: T1/T4; PR2 adds T3) | — | — |
| `connect --auth token` | token HTTPS REST / native WSS (PR2, provisioned token) | token read-only subset only | — |
| `ping` | `DbRequest::Ping` (non-batch top-level) | none beyond authentication | — |
| `profile list` / `profile show` | local file read in the profile dir; **no secret is ever displayed** | — | — |

`profile add`/`profile remove` are **dropped from the first release** (manual provisioning; §3.2). They may return in a later owner decision; they are not planned surface.

### 4.2 state (P1 — R5a; delivered by R5 PR3 alone)
| Command | Op | Perm | D |
|---|---|---|---|
| `status` | **P1: new typed read-only top-level status DTO as a new `DbRequest` variant — delivered in PR3 only** (§9): server binary version, **observed format epoch captured by the server launcher at boot — the on-disk stamp read via `inspect` (`inspect.rs:26+`) while the boot lock is held, yielding epoch (`format_stamp.rs:90`) plus the `StampOutcome` outcome (`stamp_outcome.rs:6-20`); never the bare getter's outcome-only enum alone, never a process constant, and the shell never reads disk**; monotonic uptime, replication **role** (from the node's configured mode at launch, never inferred from the telemetry snapshot), and the replication gap exactly as the exported `ReplicationSnapshot` carries it: **one merged version-gap number per repository, labelled `gap_versions_merged`, documented as non-authoritative** (the registry merges leader- and follower-side observations and initializes the gap to zero, `FOLLOW:176-207,323-348`; the public snapshot exposes no role, no observation sequence and no sampled/unsampled distinction, `export/replication.rs:9-19`), with **`age: unknown`** always (`observed_at` is a monotonic sequence, not a clock, `FOLLOW:397,431-437`; no timestamp plumbing is added); `last_receipt_ns`-based receipt age only if present in the snapshot, else unknown; WAL watermark/pending/commit telemetry (`durability_export.rs:7-51` semantics), effective vs stored durability per repo (`DURAB:222-252`). There is no per-stream role-availability API today: the replication gap in `status` is the merged per-repository number above, **labelled as a merged aggregate and never presented as per-stream data**, and a zero in it can mean "not sampled" (the shell prints the field name and the documented caveat; it never calls it lag). Distinct `inactive` / `untracked` / `unsampled` states, role-labelled gaps and an observation sequence would need an additive provenance change in the replication owner's code (`follower_loop/metrics.rs`, `export/replication.rs`): that is **not** part of R5 by default (open question Q8); if the owner wants it, it becomes a separate prerequisite task before PR3's replication fields are frozen | superuser-class read; token sessions refused (outside subset, §3.3) | — |
| `replication subscriptions` | existing `ReplicationStatus` admin op (existing batch → carries the floor) | superuser + `List(Root)` via `authorize_repl_read` (`admin_replication.rs:45-55`) + routing DB read floor (**not** gate-exempt); returns the subscription records of `replication_seed/status.rs:28-83` (identities, state, bookmark, next pull version, local consistent version, certified gap endpoints; no live upstream lag, `admin_replication.rs:760-782`); the command projects those fields as documented in the golden output | — |

Why not reuse what exists: `/info` carries only uptime/addrs/ready; `ReplicationStatus` carries subscription records without live upstream lag; WAL numbers exist only as Prometheus text. `status` is a dedicated small DTO; subscription detail is a separate command.

### 4.3 catalogue (R5a)
| Command | Op | Perm | D |
|---|---|---|---|
| `db list` | `BatchOp::List(ListOp::Databases)` (existing batch → floor) | `List(Root)` — `LIST:29-38` | — |
| `repo list` | `BatchOp::List(ListOp::Repos)` (existing batch → floor) | `List(Database)` — `LIST:39-55` | — |
| `table list` | `BatchOp::List(ListOp::Tables)` (existing batch → floor) | `List(Store)` — `LIST:57-75` | — |
| `index list` | `BatchOp::List(ListOp::Indexes)` (existing batch → floor) | **`List(Table)`** — `LIST:113-121` | — |
| `function list` | `BatchOp::List(ListOp::Functions)` (existing batch → floor) | `List(FunctionNamespace)` — `LIST:179-183` | — |
| `table describe`, `table schema` | `DescribeTable` / `GetTableSchema` (gate-exempt; existing batch → floor) | `Read(table)` (`DESCRIBE:37-53`) | — |
| `repo describe` | `DescribeRepo` (`DURAB:182-197,222-252`) | superuser + `Read(repo)`; token refused | — |

Note: `DescribeTable` already returns `owner`/`group`/`mode` (`DESCRIBE:217-230`). A separate "table rights" command would be a narrower alternative to `AccessTree`, but its distinct ACL must be documented if chosen — **default: not chosen**. The `0o751` Root default (`META:214-220`) means token-principal catalogue fixtures must use a scoped DB.

### 4.4 narrow data views (R5a)
| Command | Op | Perm | D |
|---|---|---|---|
| `data count DB REPO TABLE` | `BatchOp::Read` via `Query::select([count_all])` (`BATCHOP:55-56`; `select_item.rs:80-88`; floor). The count reaches the gated O(1) counter path only in specific dispatch cases (`crates/engine/shamir-engine-table/src/read/execution/dispatch.rs:477-492`); otherwise a real scan — the result reports which semantics ran | `Read(table)` | — |
| `data head DB REPO TABLE N` | plain first-N read in **server scan order** — `Query::with_repo(...).limit(n)` (`Q:55-57,159-172`) on the streaming path (`PAGED:185-203,395-426`; floor). Shell-side caps: rows, bytes, wall time. **Budget, not a physical-examination promise** — filters still scan. **No ordering promise**; explicitly not `ORDER BY _id` (`scan.rs:239-278`) | `Read(table)` | — |
| `data get DB REPO TABLE KEY` | **P2 (R5 PR3): typed exact-key point read**, new top-level `DbRequest` variant (not a `BatchOp`; `BATCHOP` at 992 lines). **Authorization reuses the exact real two-call ACL of `cursor_support.rs:46-92` (`authorize_cursor_read`): `Database` `Read` first, then `Table` `Read` with the ancestor check done before any lookup** — Perm column: **`Read(DB)` then `Read(table)`**, both, in that order — no cursor is created and no cursor path is engaged. **A new top-level variant does not inherit the batch gates by magic**: the handler must explicitly apply the same repo quarantine as the batch path, a typed-key size cap, and the existing server result-size/inflight controls. KEY is a Base58 `RecordId` (`RID:157-173`) | `Read(DB)` + `Read(table)` | — |

### 4.5 rights (R5b, SCRAM only)
| Command | Op | Perm | D |
|---|---|---|---|
| `user list` | `ListOp::Users` (variant; existing batch → floor) | **`Manage(Root)`** (`LIST:76-112`) — *not* a "`List(Users)` permission"; the permission is `Manage` on the `Root` target | — |
| `group tree [--db DB]`, `group members` | `AccessTree` (`ACCESS:558-594`; `AUTH:453-602`; floor) | **`Manage(Root)` always** (gate-exempt op, but the server's own handler requires Manage — never narrowed by the client's db filter) | — |
| `rights show PATH` | `AccessTree` subtree for a typed supported path | **`Manage(Root)` always** | — |
| `chmod` / `chown` / `chgrp` PATH … | `Chmod`/`Chown`/`Chgrp` with per-session HMAC over the **exact typed resource** (`HMAC:453-485`; `ACCESS:63-70,125-172`; gate `GATE:713-722`; floor) | superuser + Manage on target + HMAC | D |

Omitted deliberately: `user create/drop`, `grant/revoke role`, `create/drop db` — credential-lifecycle and destructive catalogue scope an operator shell does not need in R5. They remain server operations.

Presentation rule: group/tree output is supported paths and metadata only — no client-side "effective ACL" synthesis (`AUTH:453-602`). Chmod-family HMAC is computed **only after** interactive confirmation or `--yes`; the key is never argv-derived. **PR4 adds the small typed builder/client signing seam** that reuses the existing canonical helpers (`HMAC:453-485,800-823`); the builder today only accepts an externally supplied tag (`CHMOD_B:6-39`) — no signing helper is claimed to exist.

**ACL target for shell rights commands (defined):** The wire enum `ResourceRef` (`crates/query/shamir-query-types/src/admin/access.rs:27-58`, OBSERVED) has exactly the variants `Database`, `Store`, `Table`, `Function`, `FunctionFolder`, `FunctionNamespace` — **no `Root`, no `Repo` variant**. The default mutation subset is database/store/table/function — **no Root mutations in R5**; `Root` is not addressable as a `ResourceRef`. `rights show` uses a **separate local selector** for Root/database/store/table/function over the authorized `AccessTree` result: the selector is **not sent as a `ResourceRef`** — `Root` is permitted locally in that selector, and a folder/namespace path is **not promised** if it is absent from the authorized `AccessTree`. The shell type-checks the path client-side (so garbage never reaches the wire), but the **server remains the sole authority** over whether the resource exists and whether the actor holds `Manage` on it. No free-form string path is ever sent.

### 4.6 activity (R5b — blocked on R4b)
| Command | Op | Perm | D |
|---|---|---|---|
| `activity sessions` | **R4b step 1 `ListActivity`**, kind Sessions, scope **Own by default** (`operational-views.md:219,230,284`) | own rows: authenticated; scope All / other users' rows: `ActivityView:Manage` | — |
| `activity requests` | **R4b step 1 `ListActivity`**, kind Requests, scope Own by default | same split as `activity sessions` | — |
| `activity cancel REQUEST` | **R4b step 2 `CancelRequest`** (`operational-views.md:220,230,236`) | own request: authenticated (needs a second control session — dispatch topology D20); otherwise `ActivityView:Manage`. **Ownership is by server actor**; request/session ids are **node-local minted handles** — the shell displays only minted handles, never raw tokens or bearer material (`operational-views.md:19,127`) | D |

**Transaction and cursor views are NOT shell commands.** R4b step 3 (tx/cursor views) and step 4 (shape statistics) are both **out of R5**: they cannot ride along in PR4 without a PR of their own — adding them would change PR4's reviewed scope, so the shell waits for an explicit owner decision and its own PR if ever. This matches the release README R5 row (activity waits R4b steps 1–2, nothing more). PR4 therefore blocks **only** on steps 1–2; steps 3 and 4 are non-dependencies, not "non-blocking riders".

### 4.7 backups (R5c, SCRAM only)
| Command | Op | Perm | D |
|---|---|---|---|
| `backup start --to DIR` | `StartBackup` (`BATCHOP:209-211`; wired `launch.rs:429-441`, `admin_dispatch.rs:226-269`; floor). `--to DIR` is a **server-side absolute path only** — the shell never resolves or creates it locally; it passes the path **unmodified to the server**, and **server-side validation is the authority** (non-absolute values are rejected there). I/O-heavy; requires `--yes` in batch or interactive confirm | superuser, node-local (no DB scoping); refused on a follower with `online_backup_follower_read_only` (`backup_handlers.rs:12-35`) | D |
| `backup status` | `BackupStatus` — read-only, never reaches the follower write gate (`backup_handlers.rs:12-35`) | superuser, node-local | — |
| `backup cancel` | `CancelBackup` — **no id**; targets the current node run (`registry.rs:211-241`); wording is always "current run"; confirmation required; refused on a follower with `online_backup_follower_read_only` | superuser, node-local | D |
| `backup verify SNAPSHOT` | **explained only**: prints the manual `shamir-server backup-verify SNAPSHOT` usage and exits; never opens the data dir or boots anything | — | explained |
| `backup restore`, `backup snapshot` | explained: server `restore --from SNAPSHOT` / `backup` subcommands with their exact current flags (`MAIN:443-470`; the shell never documents or suggests `--force`) | — | explained |
| `backup schedule show` | **P5: R2d schedule state — required in R5c after R2d lands**; **proposed superuser-only**, read-only; exact DTO owned by R2d | superuser (proposed) | — |

Scheduling and history (retention, run history, storage policy) belong to **R2d and are postponed** — until R2d lands, `backup schedule show` is unsupported (exit 6, §5), not a placeholder success.

**`backup-verify SNAPSHOT` — manual wrapper, not shell:** a new top-level `shamir-server backup-verify SNAPSHOT` subcommand (owner: R5 PR5) calling the real library verify (`offline.rs:552-580`), **dispatched in the early pre-config slot** beside `format-migrate` (`MAIN:287-320`) — no boot path, no data-dir open, no config load — and no reuse of bootstrap stdin paths. It runs with the **OS rights of the invoking user** and its destructive effect is none (read-only hashing); the *manual* `restore`/`doctor` commands it explains DO touch the data directory (atomic swap, WAL replay, possible `--apply` repair), and the guide says so explicitly, including the required stopped-server and OS-filesystem-rights preconditions. Verify hashes content — never called cheap. Inline in `MAIN` parsing; no new files beyond the plan list (§9).

### 4.8 versions (R5c)
| Command | Op | Perm | D |
|---|---|---|---|
| `version show` | P1 status DTO: binary version + **observed** epoch — **delivered in R5a PR3** (rides the P1 DTO; not deferred to R5c) | as `status` | — |
| `version stamp` | **R5c (PR5)**: P1 status DTO view with the observed format-stamp state (epoch, migration id as reported by the server) | as `status` | — |
| `version migrate` | explained only: server `format-migrate` (`--plan` for a plan, `--yes --from-backup …` to apply — manual plan/apply only; the shell never applies) | — | explained |
| `schema-migration status --database-id ID` | server `ApplicationSchema` Status sub-op (`BATCHOP:198-203`; server read `schema_migration.rs:65-119,133-155`; floor) — **one line, `--database-id ID` required**; status is `Read(DB)` and is **refused on a follower** with the stable follower code (`GATE:694-703`). Gate-exempt op (`ApplicationSchema` is in the five-op allowlist, `HANDLERS_ADMIN:616-629`) | `Read(DB)` | — |

### 4.9 export/import (R5c)
| Command | Op | Perm | D |
|---|---|---|---|
| `export logical`, `import logical` | explained: server `logical-export` / `logical-import` with their exact current flags, including documented implementation limits (`MAIN:356-400`; `logical_dump/driver.rs:24-42,54-83`; `logical_import/args.rs:3-22`) | — | explained |
| `export online start` | **P6: R3 online export op — required in R5c after R3 lands.** Job start: **superuser + the Read/Manage set the export includes**; server-side artifact destination; **the output is an opaque artifact label, never a raw server filesystem path** | superuser + included Read/Manage | D |
| `export online status` | **P6** same operation family, read-only status of the job | **superuser + job owner** (read-only) | — |
| `export online cancel` | **P6** cancel with **ownership** on the job | **superuser + job owner** | D |

**No online import exists or is planned** — R3 defines export/import, but import is the offline `logical-import` subcommand only; the shell never exposes an online-import command (out of R5, matching the release README R3 row). Until R3/R2d land, the corresponding commands are **unsupported (exit 6)** — never a placeholder success (§5).

**Key rotation is excluded from R5.** The plan's backup group mentions key rotation (`05-admin-cli.md:23`), but backup/export encryption was removed by owner decision (`release-1/README.md:63`, defaults `:116-118`): copies and exports are encrypted by the user with their own means, so the key rotation that served that mechanism has no R5 surface. Any future key-rotation command is an explicit owner decision, not shell scope.

### 4.10 diagnostics (R5c)
| Command | Op | Perm | D |
|---|---|---|---|
| `diag integrity` | **explained only (offline docs)**: server `doctor` — offline only; a live scan can be racy (`doctor.rs:214-280`; engine doctor `:200-217,268-283`); `--apply` repair stays out of the shell; the shell never executes it | — | explained |
| `diag wal`, `diag durability` | P1 status DTO (WAL watermark/pending/commit) and existing `DescribeRepo` (`DURAB`) | as `status` / superuser + `Read(repo)` | — |

**All offline commands are explain-only**: the shell prints the exact manual command line, the OS rights the invoking user needs (filesystem ownership of `data_dir`, stopped-server requirement), and the destructive effects of the manual command (restore swaps the live data dir; doctor `--apply` repairs in place). The shell itself executes none of them.

### 4.11 Prerequisites that do not exist (each owned)

| ID | Missing operation | Owner | Notes |
|---|---|---|---|
| P1 | Typed read-only status DTO as a new top-level `DbRequest` variant + SDK method | **R5 PR3** (sole owner) | avoids growing `BATCHOP` (992 lines); epoch/outcome from a launcher boot-time `inspect` capture under the boot lock (§4.2); **new variant must** be added to the exhaustive token-perimeter match (`SCOPE:34-61,97-134`) as `Denied("token_request_denied")`, and to the SDK exhaustive response mapping (`API`, 918 lines — watch the cap; factoring stays in existing files / `operator_state.rs`, no new client-side file); version-gap source: the merged per-repository number of the exported snapshot, labelled `gap_versions_merged` (non-authoritative; no role, no sequence, no age); any provenance or sample-age plumbing in `metrics.rs` / `export/replication.rs` is a separate replication-owner prerequisite, added only if the owner decides so (Q8) |
| P2 | Typed exact-key point read as a new top-level `DbRequest` variant | **R5 PR3** | same perimeter/SDK-match obligations; authorization reuses the exact `cursor_support.rs:46-92` two-call ACL (Database Read + Table Read with ancestor check, before lookup; no cursor creation); handler applies repo quarantine, typed-key cap and existing result-size/inflight controls explicitly |
| P3 | `backup-verify` top-level CLI wrapper | **R5 PR5** | `offline.rs:552-580`; no boot, no data dir, early dispatch before config |
| P4 | `ListActivity` (step 1) + `CancelRequest` (step 2) | **R4b** (`operational-views.md`) | steps 3 (tx/cursor views) and 4 (shapes) are **out of R5 entirely** — not consumed by any shell command and not riders on PR4 |
| P5 | Backup schedule state + read-only schedule op | **R2d** (owner lands it; R5 PR6 consumes) | required in R5c |
| P6 | Online export op + job/status/cancel ownership + server-side artifact destination | **R3** (owner lands it; R5 PR6 consumes) | required in R5c |

No command in the tree relies on an operation outside this table. **External owner PRs (R2d, R3, R4b) touch no R5 files**; their contracts as consumed by R5 are exactly the rows above.

---

## 5. Decision 4 — Modes and output

* **Modes:** `shamir-server shell` (interactive line mode) and `shamir-server shell -- CMD...` (batch). **Batch is exactly ONE argv command after `--`** — no `-c` flag, no file mode: one command, argv-parsed, one response. Both modes share one command parser. R5a ships both from day one. (There is no keep-going/end machinery because there is only one command; nothing contradicts this.) **Interactive mode requires both stdin and stdout to be a TTY**; when either is piped, the shell expects the single `--` batch form and refuses interactive mode with a fixed safe message.
* **Machine output:** **batch output is ALWAYS JSON lines, schema v1 — on a TTY too** (batch is for scripts; the TTY never changes batch rendering). The **`--output-version` flag is a local assertion only**: it is never sent to the server, `--output-version 1` merely asserts the already-default v1, and any **unsupported version value is rejected** with exit 1 before connecting. Default is v1 everywhere in batch; the human format is interactive-only.

  Field table (typed serialized records, not a raw example — no JSON literals are designed anywhere in this document):

  | Field | Content |
  |---|---|
  | `version` | mandatory, `1` |
  | `command` | the shell's **stable command ID** — not the raw argv |
  | `kind` | `result` or `error` |
  | `op` | the wire operation name; **nullable** for local/explained commands |
  | `node` | observed role + node id; **nullable** — `null` for local/explained commands and pre-connect refusals, never a fabricated observation |
  | `data` | command-specific typed fields, documented per command in the guide |
  | `code` | errors only: a **bounded, stable enum mapping** — the shell maps server codes onto its fixed set; never the verbatim arbitrary server string |
  | `message` | errors only: a **fixed local message** authored by the shell, chosen from the §3.2/§3.3 inventory — never the opaque server message |

  Payload conventions: **decimal strings for `u64`/`i64` counters and ids** (no large-number fidelity loss through JSON float parsing), null for unknown, ISO-8601 for wall-clock timestamps, **observation sequences rendered as decimal strings** (they are not ages). **Binary encoding: hex, selected** (not base64) — one convention, documented; **non-finite floats are tagged explicitly** (`"nan"`/`"inf"`/`"-inf"` as tagged typed values, never bare JSON numbers) and **out-of-range/range-violating values are typed errors**, never coerced. Typed record values are rendered through the typed encoding with strings **safely escaped** by the JSON serializer; this is distinct from untrusted error text, which is never reproduced (below). **No raw hand-written JSON is designed anywhere in this document**; the schema is defined by the typed DTOs and documented as a field table. Adding fields is additive; changing meaning bumps the version and the guide documents both. Exactly **one `result` record or one structured `error` record is written to stdout** per batch run, after the command parses, **LF-terminated** — one line needs no further terminator and **no final record** follows it (the stream ends after it).

* **Streams and redaction:** the result/error record goes to **stdout once**. **stderr is human-only** — prompts and diagnostics; it never carries untrusted raw details: the shell **never echoes parsed argv**, never reproduces the server's opaque error message verbatim (it prints the stable code plus its own wording), and never reflects peer-supplied strings (including handshake metadata — there is no "fake server handshake metadata" channel at all). Secret values are never printed; parser errors name the flag, never the value. §5 shows no untrusted raw server errors; **authorized data returned by the server may be rendered with escaping** — color/control bytes originating from the server are **never** rendered.
* **Exit codes:**

| Code | Meaning |
|---|---|
| 0 | success — a delivered result, or a help/explained printout the user asked for. (A refusal — declined confirmation, shell policy, missing prerequisite op — is **never** exit 0.) |
| 1 | usage/parse error (shell-side) |
| 2 | connection failure (connect timeout, transport error) |
| 3 | authentication failure (SCRAM/token rejected) |
| 4 | typed denial: permission denied by server (`permission_denied`/ACL) or token-subset denial |
| 5 | server error (op failed server-side) |
| 6 | **unsupported**: command/transport/auth combination unsupported in this stage or refused by shell policy (plaintext, token transport, declined confirmation, missing `--yes` — blocked **before dispatch**, nothing sent); **also the required R2d/R3 prerequisite ops when absent** (`backup schedule show` before R2d, `export online *` before R3) return exit 6 — never a placeholder success |
| 7 | **timeout after send / unknown outcome** ("the operation may still be running; outcome unknown") |
| 130 | interrupted |

No failure is ever reported as success; a destructive command without `--yes` is blocked before dispatch (exit 6), so a declined confirmation in batch is a refusal, not silence.

* **Human vs JSON:** interactive output is human tables (fixed-width) on a **TTY only**; batch is always JSON v1 (§5 above) regardless of TTY. No color when piped; `--no-color`/`NO_COLOR` disable. **No history at all in R5** — no history file, no transcript, no paste capture; therefore no secret surface there.
* **Destructive confirmation:** interactive prompt; per-command `--yes` in batch (no blanket flag). **All** cancel/confirmations (`backup cancel`, `activity cancel`, `export online` cancel/start, `chmod` family) are confirmed.
* **Quiet/verbose:** `-q` and `-v` affect **stderr informational output only** (progress, op/duration/node detail) — they never change stdout records, suppress the single JSON result line, or alter exit codes. **No banners on any stream in any mode.**

Examples (safe shell command lines only; DB REPO TABLE are positional arguments; secrets never in argv or stdout — the secret arrives on an inherited fd or flows from a secret producer's stdout directly into the shell's stdin, so its value is never embedded in this command line):

```
shamir-server shell --profile prod --secret-prompt -- status
shamir-server shell --profile prod --secret-prompt -- data count billing main invoices
secret-producer | shamir-server shell --profile prod --secret-stdin --output-version 1 -- rights show db/billing
shamir-server shell --profile prod --secret-env SHAMIR_SHELL_TOKEN --output-version 1 -- table list main
shamir-server shell --profile prod --secret-prompt -- backup cancel --yes
```

---

## 6. Decision 5 — Safety

* **No client-side privilege.** Every call is a normal protocol request; `GATE:660-681` + `HANDLERS_ADMIN:616-629` + per-path ACLs decide everything. The shell's token-subset narrowing is shell policy (typed denial, exit 4) layered on the wider server perimeter (`SCOPE:141-184,190-261`); denied commands outside the subset are refused **even for reads**.
* **Timeouts:** a connect timeout and a per-command timeout. A client timeout cancels **only the client's wait** — after-send, the outcome is **unknown**, reported with the dedicated exit code 7 and an explicit line; no read-timeout retry. **Synchronous confirmations are local only** — prompt/`--yes` proves intent, never server receipt. R4b cancel and backup cancel are best-effort server semantics.
* **Follower behavior:** every output line carries the connected node's role. `status` reports role **as observed**; `unknown` when absent — never inferred from a `ReadWrite` mode value. The follower read-only gate is OBSERVED at `GATE:679-705`: a `ReadOnly` node refuses every client write, and the backup specials get the stable `online_backup_follower_read_only` code via `follower_write_refusal` (`backup_handlers.rs:12-35`), while `BackupStatus` (read-only) never reaches that gate. The shell **surfaces the stable code verbatim from its bounded enum mapping** and never retries elsewhere; a positive follower probe verifies **actual node-local stream availability**; when no stream is locally present the status prints `unknown` and **stale target aliases are never trusted** as evidence of a stream. `schema-migration status` is refused on followers server-side (`GATE:694-703`). **Reads are node-local — no redirect**: nothing in the shell redirects or re-targets a read to another node (leader or peer); whatever node the profile points at answers, and quarantine/timeouts never retry or redirect.
* **Freshness semantics (P1 defaults):** the status DTO reports the **merged per-repository version gap** of the exported snapshot, labelled `gap_versions_merged` and documented as non-authoritative — it never pretends per-stream data and never calls the number lag; **no new timestamp or provenance plumbing is added by default** (Q8 leaves that open for a later owner decision): `observed_at` is a **monotonic observation sequence, not a sample age** (`FOLLOW:397,431-437`), so sampling age is **always unknown, not fabricated**; `last_receipt_ns` gives receipt age only, may be unknown; a zero can mean "not sampled" and the field's documentation says so. The shell never triggers a boot-time stamp read — the epoch comes from the launcher's boot capture (§4.2). This does not replace real lag; the existing exporter's receipt-age field is presented as exactly what it is.
* **Audit visibility:** R5 adds **no** new server audit API, and this design does **not** claim destructive/admin mutations are "already recorded" — existing coverage is narrow (`REPO_ADMIN/repo_audit.rs:74-111`) and the server audit prefix is sensitive and not exposed. For the mutations R5 exercises, CI tests assert **actor attribution** (the audit entry names the shell's authenticated actor) where audit exists today; for every mutation gap uncovered by audit, this design records the gap and its owner here: `chmod`/`chown`/`chgrp` and backup start/cancel have no observed audit coverage — widening audit is owned by the PR that proposes it, not by R5.
* **TLS pinning:** verified-pin only. The profile carries the captured pin; mismatch or absence is a hard refusal. **No `--accept-new-host` in any stage; no CA bypass.** Enrollment is a manual profile step.
* **Runtime hardening:** the interactive reader uses the existing OS bindings (rustix termios / windows-sys console) with mode save/restore on every exit path, exercised by CI tests (§8).

---

## 7. Decision 6 — Dependencies (zero new registry crates)

| Need | Choice | New registry crate? | Rationale / gates |
|---|---|---|---|
| Arg parsing | `clap` (already a server dep, `LOCK:666`) | no | binary already pays for it |
| Profile parsing | `ktav 0.8` strict **typed** parse (`PKG:76`, `LOCK:1940`); `serde`/`serde_json` typed derives (`LOCK:3319`) for the output DTOs and profile structs | no | existing workspace versions |
| No-echo input, terminal restore | `rustix` **1.1.6** (exact selected node, `LOCK:3116`) Unix-only target dep; `windows-sys` **0.61.2** (exact selected node, `LOCK:5637`) windows-only target dep | no | **target-specific** direct/feature edges declared explicitly in PR1; **these edges add zero new registry packages** — both exact versions are already resolved graph nodes; no PTY crate |
| Secret zeroization | `zeroize` (`LOCK:5864`) | no | already in tree (`BSTDIN` precedent) |
| Async runtime (inherited) | `tokio` — already a server dependency (`PKG` async region); the shell compiles against the binary's existing runtime feature set, no new edge | no | inherited via the shared binary |
| Base58 / hex rendering | `hex` (existing, `LOCK:1660`) for the selected hex binary encoding; `base64` (`LOCK:381`) unused — **hex is the one binary convention** (§5); RecordId printing uses the existing Display (`RID:157-173`) | no | existing versions only |
| Profile directory | `shamir-private-dir` — new **in-workspace path** edge for `shamir-server` | no registry crate | already linked transitively (`crates/foundation/shamir-private-dir/src/lib.rs:1-20,28-30`) |
| JSON lines output | hand-serialization over existing serde tooling | no | schema stays tiny and versioned |
| Line editing / tables | hand-rolled reader; fixed-width tables | no | `rustyline`/`dialoguer`/`tabled` would be new registry versions under `deny.toml:35-54` and the license allowlist `deny.toml:81-100` plus the 3-day cooldown — none justified for R5 |

Profile-dependent existing server dependencies (the shell shares the binary, so these edges already exist and none are added by the shell): `shamir-client` path dep at **`PKG:50-56`** (comment block + `shamir-client = { path = "../../client/shamir-client" }` — OBSERVED; the previously cited `:55-57` was wrong), plus the async/TLS stack (`tokio`/`tokio-rustls`/`rustls` region directly below) and `clap` at `PKG:112-113` — OBSERVED.

License/advisory gates: new crates would have to clear `deny.toml:35-54` (advisories), `deny.toml:81-100` (license allowlist), and `cooldown.toml:10-28` (version cooldown, **security-only exception**) — **no live registry network checks are required by this design**; the gates are the existing CI `cargo deny` run. Zero crates are added, so nothing new is gated.

Binary size/link/build-time effect: **unmeasured, INFERRED only** — this design does not claim the pipeline currently measures release binary size (no such step was located) and does not claim a measured build-time delta. The acceptance criterion is stated as a comparison: the PR reports the CI baseline binary size vs the post-shell size, against the existing **< 50 MB goal** (`CLAUDE.md:20`), and CI confirms or falsifies the estimate.

---

## 8. Decision 7 — Testing and acceptance (CI-only)

All tests run on GitHub CI (binding policy; locally only `cargo check`/`clippy`/`fmt`/`check_layout.py` are allowed). Tests live in the **in-package integration target** `crates/server/shamir-server/tests/admin/` — wired through the declared target `PKG:214-218`; the acceptance runs the **actual binary** per stage, and each stage's matrix covers **each transport the stage supports** (PR1: TLS-TCP + IPC; PR2 onward adds WSS SCRAM and token HTTPS/WSS, including the later SCRAM-only refusal when a token session attempts a rights/backup command). Under a token session the acceptance runs **both** the allowed read probes of §3.3 **and** the refusal probes for everything outside the subset. **Profile-directory safety is never asserted as automatic under manual provisioning**: every acceptance that touches profile permissions runs **explicit OS checks** (owner/ACL assertions on the created profile dir/files, per OS) — the design claims only what those checks observe.

Acceptance is deduplicated **per stage** (one matrix row per stage, not per PR):

| Stage | CI acceptance (observable) |
|---|---|
| R5a (PR1+PR2+PR3 head) | scripted one-shot scenarios "connect → ping → catalogue → status → data" per supported transport (TLS-TCP+SCRAM, IPC+SCRAM, WSS+SCRAM, token on the read-only subset incl. SCRAM-only-command refusal under token); typed-refusal probes for every §3.2 row (plaintext incl. loopback, token-over-IPC, no-pin, deferrals); permission-denied (non-admin on `repo describe`; token principal on a scoped DB given Root `0o751` defaults, `META:214-220`); output-schema golden tests (field table, `--output-version 1`, one stdout record, human-only stderr); secret-leak byte-scan over **stdout, stderr, argv, profiles, and history-absence** (unique canary secret per run); parser and malicious-peer-reflection scans (peer-supplied strings never reach stdout/stderr); no-echo/terminal-restore on all three OSes |
| R5b (PR4 head) | rights commands under non-manage user → exit 4 with stable code; `user list` permission-denied (non-Root-Manage per `LIST:76-112`) → exit 4 with stable code; `activity sessions/requests` list + own-request cancel after R4b steps 1–2 land, proven on a genuinely PARKED request (a request blocked in a lock wait on another connection): after the shell's cancel the parked request wakes, its own client receives the `cancelled` code and its snapshot, locks and in-flight entry are released (the observable of R4b's `cancel_wakes_parked_lock_wait`; the shell's control reply alone is never accepted as proof) (**no tx/cursor rows — steps 3 and 4 are out of R5**); cross-user cancel without `ActivityView:Manage` → denial; `chmod` HMAC requires confirmation (declined → exit 6, no request sent; tag verified server-side) — tests in the same approved integration file as the seam (§9 PR4) |
| R5c (PR5+PR6 head) | backup start/status/cancel on a real server (current-run wording, confirmations enforced); `backup-verify` wrapper accepts a good snapshot and rejects each corruption class the library detects (`offline.rs:552-580`); `version stamp` reports the observed epoch (`version show` already accepted in R5a); `schema-migration status --database-id` happy + follower-refused; schedule/online-export rows once R2d/R3 land (before that: exit 6 unsupported, asserted); no-effect assertions for explained commands (`backup verify`/`backup restore`/`backup snapshot`, `version migrate`, `export`/`import logical`, `diag integrity`) |

**OS nuance (mandatory, not best-effort):** ordinary PR CI runs Ubuntu + Windows only; **macOS appears only on `workflow_dispatch`/`schedule`** (`CI:706-715`) — so **each stage requires a manual `workflow_dispatch` on the same head** as the PR CI run (at minimum the final changed head), verifying from the logs that the `admin` target was selected on **all three OSes**. Same-head dispatch is a hard acceptance gate, not an optional extra. Test selection honors the affected-targets computation (`CI:769-781`, `SEL:126-154`). No tests are run locally (binding policy).

---

## 9. Decision 8 — Implementation plan (exactly six PRs, named files, counted parents)

Ordering: **PR1** foundation — no P1/P2, no migration journal surface; TCP + IPC SCRAM ping/catalogue. **PR2** WSS/tokens/REST + finish catalogue. **PR3** P1 and P2 (both — P1 and P2 are PR3-only deliverables, referenced nowhere else as landed earlier) and data views + `version show`. **PR4** rights/activity after **R4b steps 1–2 only** (steps 3 and 4 are out of R5 — no riders). **PR5** backup/version journal/offline manuals + verify. **PR6** only after **R2d/R3**: schedule, online export, acceptance guide. External owners (R4b, R2d, R3) touch **no** R5 files.

Every folder count below was verified with `ls` on this branch; each PR re-counts before landing (`python -I scripts/ci/check_layout.py`, limits at `scripts/ci/check_layout.py:83-90`, is the backstop). Final shapes: package root **4 → 5**; `shell/` final **6** entries (`mod.rs`, `connect/`, `terminal/`, `output.rs`, `parser.rs`, `commands/`); `shell/connect/` final **4** (`mod.rs`, `scram.rs`, `token.rs`, `profile.rs`); `shell/terminal/` final **4** (`mod.rs`, `unix.rs`, `windows.rs`, `secret.rs`); `shell/commands/` final **7** (`mod.rs`, `catalogue.rs`, `state.rs` [point/head/count], `access.rs`, `activity.rs`, `maintenance.rs` [backup/version/offline], `jobs.rs` [schedule/export]); `REPO_ADMIN/` **4 → 5** (`operator_state.rs`); `tests/admin/` **5 → 6** (adds `shell/`); `tests/admin/shell/` final **7** (`mod.rs` [module declarations and re-exports only: the rust-guard structure gate allows nothing else in any `mod.rs`], `harness.rs` [shared fixture and helpers], `transport.rs`, `output.rs`, `state_data.rs`, `access_activity.rs`, `maintenance.rs` [also carries the PR6 schedule/export tests; there is no `jobs.rs` test file]).

| PR | Content | New/edited files (trajectory: parent → count at end of PR) | Deps/prereqs | Tests |
|---|---|---|---|---|
| **PR1** | Shell foundation: early `shell` dispatch (`MAIN`, `#[path]`, precedent `MAIN:24-30`); `--config` stays required + validated on every pre-existing dispatch path (`MAIN:44-48`) — unchanged; `shell` (and later `backup-verify`) are the only paths that do not require it; TCP + IPC SCRAM connect with verified-pin-only profiles (strict Ktav, allowlisted fields, fail-closed dir checks); secrets (TTY no-echo, `--secret-stdin`, `--secret-fd` Unix-only, `--secret-env`, argv refusal); interactive + one-command `--` batch; JSON-lines v1 + exit codes; commands: `ping`, catalogue (`db/repo/table/index/function list`, `table describe/schema`) | package root **4→5** (`shell/` module); `shell/`: `mod.rs`, `output.rs`, `parser.rs`, `connect/`(`mod.rs`,`scram.rs`,`profile.rs`), `terminal/`(`mod.rs`,`unix.rs`,`windows.rs`,`secret.rs`) → **6**; `shell/commands/`: `mod.rs`, `catalogue.rs` → **2**; edit `MAIN`; `tests/admin/` **5→6** adds `tests/admin/shell/`: `mod.rs` (declarations only), `harness.rs`, `transport.rs`, `output.rs` → **4** (registration edited into `tests/admin/main.rs`) | direct edges: `shamir-private-dir` (path), target-specific `rustix 1.1.6`/`windows-sys 0.61.2`; no registry crates | §8 R5a rows (PR1 subset) + tests proving the shell performs **no server-config read** and that pre-existing boot behavior is unchanged (`--config` still required for `run`/offline paths) |
| **PR2** | All secure transports: native WSS + SCRAM; token HTTPS-REST/WSS (provisioned token) on the read-only subset with every typed refusal (plaintext incl. loopback, token-over-IPC, SCRAM-only-refusal scaffolding, browser/HTTP-login deferrals); no resume in any stage; no `--accept-new-host`; **completes the catalogue with `repo describe`** | `shell/connect/token.rs` → `connect/` **4**; extend `tests/admin/shell/transport.rs` (no new file) | PR1 | transport matrix and catalogue commands supported at the PR2 head (§8 R5a rows subset) |
| **PR3** | P1 status + P2 point-read + data views (P1 and P2 are delivered here and only here): both as new top-level `DbRequest` variants; **`replication subscriptions` (the existing `ReplicationStatus` op — no wire changes)**; `data count` (`count_all`), `data head` (streaming limit route), `data get`; `version show` (P1 view — R5a); **explicit edits to exhaustive matches**: `SCOPE:34-61,97-134` (both variants → `Denied("token_request_denied")`), SDK response mapping **inside the existing `api.rs` (918 lines — ≤ 1000 cap; any needed factoring goes into the planned `REPO_ADMIN/operator_state.rs` or existing client files — `client/src/client/` is already at its 7-entry folder limit, so no new `api_status.rs` and no unnamed helper file)** | `shell/commands/state.rs` → `commands/` **3**; `DBMSG` DTO additions (537 lines, room); new `REPO_ADMIN/operator_state.rs` via `#[path]` in `DHMOD` → **5**; edits: `DHMOD`, `GATE` (thin dispatch only — factoring uses `operator_state.rs`, never a new helper file), server launcher wiring, `DBMSG`, `API`, `SCOPE` — **no folder additions**; `tests/admin/shell/state_data.rs` **added here** (`tests/admin/shell/` 4 → 5) | PR2 | §8 R5a rows: status on SCRAM transports (token sessions get the explicit token refusal, never a status); token probes; count/head/get golden + bounded-budget assertions; `version show` reports the observed epoch; `replication subscriptions` golden
| **PR4** | Rights + activity: `user list` (`Manage(Root)` per `LIST:76-112`), `group tree/members`, `rights show` (`Manage(Root)`), `chmod/chown/chgrp` — **signing seam**: small typed builder + client method added **only to existing named files** — the existing builders `chmod.rs`/`chown.rs`/`chgrp.rs` (`access_builders/resources/`, OBSERVED `ls`) plus the client `api.rs` — reusing `HMAC:453-485,800-823` (the builders today only take an external tag, `CHMOD_B:6-39`; **no new file**); `activity sessions/requests` + cancel (R4b steps 1–2 **only** — no tx/cursor, no shapes) | `shell/commands/access.rs`, `shell/commands/activity.rs` → `commands/` **5**; seam edits named above; `tests/admin/shell/access_activity.rs` → `tests/admin/shell/` **6** (tests approved in the same integration file) | R4b steps 1–2 only | §8 R5b rows |
| **PR5** | Backups/versions/offline manuals + verify: `backup start --to DIR/status/cancel` (current-run wording, confirmations); `version stamp` (R5c), `version migrate` + restore + doctor + logical export/import as **explained commands** (no execution; OS rights + destructive effects documented); `schema-migration status --database-id ID`; **`backup-verify SNAPSHOT` wrapper inline in `MAIN` early parsing** (pre-config slot, `MAIN:287-320`) backed by `offline.rs:552-580`; **diagnostics: `diag integrity` (explained-only docs), `diag wal` and `diag durability` (P1 status DTO / existing `DescribeRepo`, §4.10)** | `shell/commands/maintenance.rs` → `commands/` **6**; `MAIN` edits only (no new files beyond the list); `tests/admin/shell/maintenance.rs` → `tests/admin/shell/` **7** | after PR3; schedule + export-online commands are unsupported (exit 6) until R2d/R3 land | §8 R5c rows incl. corruption classes |
| **PR6** | Completion after R2d/R3: `backup schedule show` (P5) and `export online start/status/cancel` (P6: superuser + included Read/Manage on start, status/cancel superuser + job owner, opaque artifact label output, server-side artifact destination); acceptance guide updated **by editing the existing `docs/guide-docs/guide/07-operations.md`** (the operations guide named by the release gates, `release-1/README.md:91-107` — **not** a file under `release-1/`; no new guide file); schema-golden freeze; macOS `workflow_dispatch` verification of the `admin` target across all stages on the same head | `shell/commands/jobs.rs` → `commands/` **7**; extend `tests/admin/shell/maintenance.rs` (`tests/admin/shell/` stays **7**); edit `07-operations.md` | R2d, R3 | full three-OS matrix green |

Dependency/PR notes: every new direct or feature dependency edge is named in its PR description (PR1: `shamir-private-dir`, target-specific `rustix`/`windows-sys`). No PR adds a registry crate. `GATE` and `BATCHOP` are not grown; `GATE` at 992 lines gets explicit helper factoring in PR3 if the ~1000 cap is threatened.

Runtime output guarantees across all PRs: no raw parsed argv, no server error chains, and no fabricated server handshake metadata ever appear in stdout or stderr (§5).

---

## 10. Decision 9 — open questions for the owner (each with a recommended default)

| # | Question | Default |
|---|---|---|
| Q1 | Browser profile / HTTP password login? | **Deferred** with the explicit printed refusal; HTTP login mints a reusable token (`TLOGIN:22-23`) |
| Q2 | `profile add`/`remove` in a later stage? | **Not in first release**; manual provisioning; revisit only on operator request |
| Q3 | `backup-verify` owner? | **R5 PR5** as a manual top-level subcommand, early dispatch |
| Q4 | `--secret-fd N`? | **Unix-only, in R5 PR1** (safe duplicate + close-duplicate-only); Windows refused in favor of `--secret-stdin` |
| Q5 | JSON output versioning flag? | **`--output-version 1`, client-side only, never sent to the server** |
| Q6 | Table-rights shortcut via `DescribeTable` fields? | **Not chosen**; rights stay on Root Manage `AccessTree` unless the distinct ACL is documented first |
| Q7 | Line-editing polish (rustyline-class)? | **Defer**; explicit dependency decision later |
| Q8 | `status` freshness source: owner-plumbed wall-clock sample age in `metrics.rs`, or observation sequence + `age: unknown` (`observed_at` is a monotonic sequence, `FOLLOW:397,431-437`)? | **Sequence + unknown** unless the owner adds the timestamp plumbing; per-repo aggregate gap labeled source/role when no per-stream API exists |

## 11. What the owner must confirm (largest blast radius)

1. **Packaging as a server subcommand** with the `shell/` package-root module — shapes distribution, CI and the file layout for the whole feature.
2. **Token sessions restricted to the shell's read-only subset** — any widening is a server allowlist change plus shell-policy change, never a client bypass.
3. **"Explain, don't wrap" for offline operations**, with exactly one bounded exception (`backup-verify`, manual, early dispatch) and the P2 point-read as a top-level variant.
4. **`data head` has no ordering guarantee** (server scan order) — if deterministic first-by-key is wanted later, a concrete bounded key-ordered API must be specified first.
5. **`profile add`/`remove` dropped from the first release** (manual provisioning).
6. **No `--accept-new-host`** in any stage; first-contact enrollment is a manual profile step.

## 12. Self-review — weak points found in this design

1. **P1/P2 are new top-level wire surface.** Mitigation: read-only/typed; the exhaustive perimeter match (`SCOPE:34-61,97-134`) and SDK response mapping are named PR3 edits, not afterthoughts.
2. **`data head` is order-less by construction.** Stated as a property (§4.4), not hidden; `ORDER BY`-based alternatives were rejected because that path materializes before limiting (`scan.rs:239-278`). Budget caps are honest: filters still scan.
3. **`commands/` reaches its final 7 entries only at PR6.** The trajectory is explicit per PR (§9); every PR re-counts before landing.
4. **Cancel asymmetry** (`CancelBackup` has no id, `registry.rs:211-241`) — wording and confirmations say "current run"; an id-conditional cancel is a backup-API change owned by R2a, not the shell.
5. **Freshness presentation can be misread as lag.** `observed_at` is a monotonic observation sequence (`FOLLOW:397,431-437`), not a clock; the DTO defaults to per-repo aggregate gap, sequence, and unknown/null age — no timestamp plumbing by default (Q8 later), and receipt age, gap, and inactive/untracked/unknown stay distinct fields (§4.2, §6).
6. **`DescribeRepo` is cited only for repo durability** (`DURAB:182-197,222-252`), never as describe-TABLE evidence.
7. **Audit coverage is narrow** (`repo_audit.rs:74-111`); no blanket "already recorded" claim — CI asserts actor attribution only where audit exists, and every uncovered mutation gap is named with its owner (chmod family, backup start/cancel — §6).
8. **macOS CI gap**: three-OS acceptance depends on a manual `workflow_dispatch` on the same head (§8) — a mandatory gate, not assumed. Local runs are banned by policy; nothing was executed here.
9. **`schema-migration status` collapsed to one row** with `--database-id` required; mismatch-zero is never reported as an empty journal (`schema_migration.rs:65-119,133-155`).
10. **No raw JSON, no raw wire, no reflected peer data**: output is defined by field tables and typed DTOs; error codes are a bounded enum mapping with fixed local messages; every command example is a shell command line; untrusted strings never reach stdout/stderr (§5).
11. **Signing seam is new surface**: the builders accept only external tags today (`CHMOD_B:6-39`), so PR4's seam is genuinely new code — kept small, confined to the named existing files (`chmod.rs`/`chown.rs`/`chgrp.rs` + client `api.rs`), tested in the same approved integration file.
12. **Size estimates are INFERRED**: no claim that the pipeline currently measures release binary size; acceptance is baseline-vs-new comparison in CI against the < 50 MB goal (§7).
13. **P1 epoch source is a launcher capture, not the getter.** `ServerHandle::format_stamp()` returns a `StampOutcome` enum only (`stamp_outcome.rs:6-20`); the launcher reads the on-disk stamp via `inspect` under the boot lock at boot and exposes epoch + outcome + monotonic uptime; the shell never reads disk and no boot-time stamp read is triggered by shell commands (§4.2).
14. **P2 does not inherit batch gates implicitly.** The handler must explicitly reuse the two-call ACL (`cursor_support.rs:46-92`), repo quarantine, typed-key cap and result-size/inflight controls (§4.4).
15. **Transport validation must not be over-claimed.** Fresh TLS TCP/WSS SCRAM is application-pin trust, not PKI — no CA validation is implied for native SCRAM; only token sessions do CA validation (`TOKENTLS:43-69`); the exact §3.3 subset is closed — anything not listed is refused (§3.2, §3.3).
16. **Batch output on a TTY is still JSON v1** — scripts must be able to trust the stream regardless of redirection; `--output-version` is an assertion, not a selector (§5).
