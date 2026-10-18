# 131b — destination lifecycle: DDL/wire surface, creation exclusion, seed install, schemas, admin API

Companion to [`131-s5cx-destination-lifecycle.md`](131-s5cx-destination-lifecycle.md) (§1-5: goal, invariants, data model, protocols, backend contract) and [`131a-s5cx-destination-lifecycle-evidence.md`](131a-s5cx-destination-lifecycle-evidence.md) (§9-12: CI evidence, PR staging, risks, PR8b-4 appendix). This file holds §6-8 plus the durable-schema appendix (A) and the administrative inspect/retry API (B). Citation discipline: **OBSERVED** = verified in this worktree at origin/master @ b67a3c46; **REQUIRES #188** = PR8b-2 branch dependency with the contract named; **DECIDED** = binding design choice; **INFERRED** = derived consequence.

## 6. DDL / wire / builder / client surface

### 6a. Grammar and DTOs

**Grammar sketch (DECIDED):**

```text
DROP REPOSITORY <name> [CASCADE] [IF EXISTS] [DESTROY QUARANTINED DATA '<reason>' OPERATION ID '<op-id>' NONCE '<32 hex chars>'];
DROP DATABASE <name> [CASCADE] [IF EXISTS] [DESTROY QUARANTINED DATA '<reason>' OPERATION ID '<op-id>' NONCE '<32 hex chars>'];
```

- `CASCADE` controls child scope only; it never authorizes crossing replication bindings or physical destruction.
- **DTO (wire):** optional field `destroy_quarantined_data: Option<{ reason: String (nonempty, 1..=512, printable UTF-8), operation_id: String (≤64, [0-9A-Za-z._-]+), nonce: String (exactly 32 lowercase hex chars — the hex form of 16 bytes) }>` on `DropRepoOp` (**OBSERVED** crates/shamir-query-types/src/admin/types/repo_ops.rs:43-55: no `deny_unknown_fields` on the Drop ops) and `DropDbOp` (**OBSERVED** crates/shamir-query-types/src/admin/types/db/db_ops.rs:27-40). Unknown-field tolerance on Drop ops is intentional for forward-compatible builders; the new field is `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- **Builder (Rust):** `.destroy_quarantined_data(reason, operation_id, nonce)` on the drop builders — **OBSERVED** current locations `crates/shamir-query-builder/src/ddl/repo/drop_repo.rs` and `crates/shamir-query-builder/src/ddl/db/drop_db.rs` (panel paths `ddl/drop_repo.rs` are stale). HMAC computation moves behind the modifier check.
- **TS client:** `.destroyQuarantinedData(reason, operationId, nonce)` on the drop builders in `crates/shamir-client-ts/src/core/builders/ddl/` (**OBSERVED** `ddl-schema.ts` holds `dropDb` at :83-97 and `dropRepo` at :99-113, both computing the canonical bytes via `canonicalDropDb`/`canonicalDropRepo` imported from `core/protocol/hmac.ts`); request flows through `crates/shamir-client-ts/src/core/client/client.ts` unchanged.
- `IF EXISTS` semantics per 131 §4a (inspect incomplete operations first; **OBSERVED** early exits admin_db_repo.rs:106-112 and :367-368).
- `nonce`: supplied by the caller; the wire DTO carries `nonce: String` (32 hex chars) inside `destroy_quarantined_data`; **echoed verbatim in the response** (`{ operation_id, state, nonce }`), and bound into the v2 canonical encoding (the canonicalizer encodes the 16 decoded bytes) — a replayed confirmation over different nonces fails, and a lost response cannot be confused with a different request.

### 6b. Injective canonical confirmation encoding (v2) — DECIDED (F8)

**DECIDED:** replace the NUL-concatenation canonical forms for ALL destructive confirmations with a length-prefixed, injective encoding. The product is unreleased: **replace outright, no dual-form, no migration**.

```text
version(1B) = 2
field_count(1B)
for each field, in FIXED order:
    tag(1B) || len(u32 LE) || bytes
```

Fields, fixed order (omitted fields are simply absent; `field_count` counts present fields):

| # | tag | Field | Present | Content |
|---|-----|-------|---------|---------|
| 1 | 0x01 | operation_kind | always | `"drop_repo"` \| `"drop_db"` (UTF-8) |
| 2 | 0x02 | db | always | db name (UTF-8) |
| 3 | 0x03 | repo | `drop_repo` only | repo name (UTF-8) |
| 4 | 0x04 | cascade | always | 1 byte, 0 or 1 |
| 5 | 0x05 | destroy_flag | always | 1 byte, 0 or 1 |
| 6 | 0x06 | destroy_reason | when destroy_flag = 1 | reason (UTF-8) |
| 7 | 0x07 | operation_id | when destroy_flag = 1 | operation id (UTF-8) |
| 8 | 0x08 | actor | always | actor (UTF-8) |
| 9 | 0x09 | nonce | when destroy_flag = 1 | exactly 16 bytes |
| 10 | 0x0A | targets | when destroy_flag = 1 | the frozen `{db, repo, storage_object_id}` list (length-prefixed sub-encoding, injective) |

Injectivity: length prefixes make distinct field sequences encode distinctly (no NUL-delimiter ambiguities); fixed field order + explicit `destroy_flag` makes a plain DROP (`destroy_flag = 0`, fields 6-7 absent) byte-distinct from any destruction confirmation. **All drop confirmations upgrade to v2** — plain drops get `destroy_flag = 0` and omit the optional fields; a plain-DROP HMAC therefore can never authorize destruction. Plain drops carry no nonce — the nonce exists only inside `destroy_quarantined_data`, so it is encoded only when `destroy_flag = 1` (fields 6-7, 9 and 10 all absent for a plain drop). **Round-3 addition:** the `targets` field (tag `0x0A`) carries the frozen `{db, repo, storage_object_id}` list from the `Prepared` operation; the executor verifies the live set equals the signed frozen set before Quiescing (I6).

**Validation (server and client):** reject NUL and C0 control characters in `reason`, `operation_id`, `actor`; enforce the Appendix-A bounds; `nonce` must decode from exactly 32 hex chars to 16 bytes; `field_count` and `version` must match this table exactly (unknown tags/versions → reject).

**Code touchpoints (all must change together):**

- **Rust canon module:** `crates/shamir-query-types/src/security/hmac.rs` — **OBSERVED** `canonical_drop_db` at :101 and `canonical_drop_repo` (same module; doc table of the NUL forms at :30-31; round-1 tests hmac_tests.rs:29-32 pin `b"drop_db\0mydb"` / `b"drop_repo\0mydb\0cold"` and must be rewritten to v2 vectors). Replacement functions: `canonical_drop_db_v2(DropDbConfirm) -> Vec<u8>` / `canonical_drop_repo_v2(DropRepoConfirm) -> Vec<u8>` over a shared `struct ConfirmV2 { kind, db, repo: Option<String>, cascade, destroy: Option<DestroyConfirm{reason, operation_id, nonce, targets}>, actor }` (the nonce + frozen targets live inside `DestroyConfirm` — plain drops carry neither); the old functions are deleted, not deprecated. **Update the field table and `ConfirmV2` everywhere they appear (Rust canon module, TS hmac.ts builders, shared vectors) when adding tag `0x0A`.**
- **Server verification seam:** `crates/shamir-server/src/db_handler/admin.rs:655-659` — **OBSERVED** the match arms `BatchOp::DropDb(op) => (canon::canonical_drop_db(&op.drop_db), op.hmac.as_ref())` and the `DropRepo` analogue inside the destructive-op HMAC gate. These arms construct `ConfirmV2` from the op (+ actor + nonce) and call the v2 functions. **BatchOp handling:** the gate iterates `BatchTreeVisitor` over the batch (**OBSERVED** admin.rs:652) — when `destroy_quarantined_data` is present on a drop op, the arm builds the v2 form with `destroy_flag = 1` and the dispatch additionally routes to the R2' protocol instead of the ordinary drop path; plain drops use `destroy_flag = 0`. A batch mixing a destruction drop with other ops is verified op-by-op as today.
- **TS client canonicalizers:** `crates/shamir-client-ts/src/core/protocol/hmac.ts` — **OBSERVED** `canonicalDropDb` at :97 and `canonicalDropRepo` at :101 — replaced by v2 builders mirroring the Rust encoder (shared test vectors, see §6c); the builders in `core/builders/ddl/ddl-schema.ts` (:83-97, :99-113) pass the new canonical bytes through `signer.hmacTagHex` unchanged in shape.
- **Test vectors:** add shared JSON test vectors (Rust + TS) covering: plain drop, destruction drop, each omission, overlong/boundary lengths, control-char rejection, nonce mismatch. The old pinned NUL expectations (hmac_tests.rs:29-32; `crates/shamir-client-ts/src/core/protocol/__tests__/hmac.test.ts:42-45`) are replaced.

### 6c. Error codes — the ten typed wire codes (DECIDED, normative)

Survive dispatch incl. rename (**REQUIRES #188** the `db_error_to_batch` fix). Each `DbError` variant maps 1:1 to its wire code; the remedy contract is what the client/operator does on receipt:

| DbError variant | Wire code | Remedy contract |
|---|---|---|
| `ReplicationDestinationBound` | `replication_destination_bound` | The name is bound by live replication bindings; release via R2'/R1 first (rename: format limitation per 131 §1). |
| `QuarantinedSubscriptionDestinationsRemain` | `quarantined_subscription_destinations_remain` | Destroy each remaining bound destination, then retry R1 (stage-1 semantics; I9). |
| `QuarantinePersistencePending` | `quarantine_persistence_pending` | A pending terminal cause for a referencing generation is unresolved (§3f of 131); retry after the flush is confirmed. |
| `DestinationNameReserved` | `destination_name_reserved` | The name is reserved (anchor or non-`Destroyed`/non-`Rejected` operation); only the §7c recovery exception admits publication. |
| `DestructionInProgress` | `destruction_in_progress` | An incomplete operation covers the target; poll `destruction_status`, then `destruction_retry`/wait. |
| `DestructionBlocked` | `destruction_blocked` | External block; clear the cause, then `destruction_retry(operation_id)`. |
| `StorageDestructionUnsupported` | `storage_destruction_unsupported` | The backend cannot destroy this component; no retry path until the backend gains support. |
| `UnsafeStorageOwnership` | `unsafe_storage_ownership` | A live replacement/foreign object occupies the recorded identity; never deleted (I6); operator investigation required. |
| `DestructionOperationConflict` | `destruction_operation_conflict` | Same operation id with different frozen parameters; issue a new operation id. |
| `DestructionAuditUnavailable` | `destruction_audit_unavailable` | The audit chain could not durably record the intent; retry the same operation id. (The operation STAYS `Prepared`, reservations unpublished, nothing deleted — 131 §3e item 3.) |

`ErrorClass` mapping (journal `error_class` field → wire code, 131 §3b): `UnsupportedBackend → storage_destruction_unsupported`; `OwnershipMismatch → unsafe_storage_ownership`; `ExternalLock` / `PermissionDenied` / `UnknownIo → destruction_blocked`.

## 7. Creation exclusion matrix — complete (F3)

Reservation check = `(db, repo)` covered by anchors or non-`Destroyed`/non-`Rejected` operations (131 §3c; `repo = "*"` = database-scope reservation covering every `(db, *)`).

### 7a. NamespaceAdmission guard — DECIDED (round 3: hierarchical admission, merged with the lifecycle exclusion)

- **Two-level hierarchy:** a per-database RW-style gate (repo-scope operations take it SHARED; database-scope operations EXCLUSIVE) plus per-(db, repo) admission mutexes beneath it, held in an in-process registry on `ShamirDb`: `admissions: DashMap<(String, String), Arc<tokio::sync::Mutex<()>>>`. A publication acquires the db gate, then the mutexes for its (db, repo) namespaces (or `(db, "*")` for database scope) in SORTED (db, repo) order (deadlock-freedom), **BEFORE any validation**, and holds them until publication or abort; RAII drop = abort; cancellation-safe; **no durable trace**.
- **ONE mechanism with the per-target lifecycle exclusion (round 3):** the same admission-guard object serves as the per-target lifecycle exclusion, held from preflight start until `Destroyed`/`Rejected`/`Blocked`-release (131 §4e). Keys normalized exactly like reservations (§3c of 131).
- **Mutual exclusion with reservation creation:** reservation writes (Prepare publication, R1 publication) acquire the same db gate + admission mutexes for the names they reserve; a newly admitted publisher therefore cannot interleave between a reservation's check and its publication.
- **The round-2 generation-counter fallback is REMOVED** — no generation token, no belt-and-braces re-check with token invalidation, no second invalidation path; holding the guard across check-and-publish is the ONLY serialization (I13 of 131).
- **Lifetime:** the guard lives only within one operation; cancellation drops it (RAII); it is not a lease, not persisted, and never blocks boot.
- Boot and the executor take the same guard when publishing/skipping so boot-time publication cannot race an in-flight Prepare.

### 7b. Publication-API inventory (every managed/engine path — F3-i)

Each row: API, file:line (verified this worktree), where the reservation/admission check lives, what happens on violation.

| # | Publication API | file:line | Check placement | On violation |
|---|---|---|---|---|
| 1 | `ShamirDb::create_db` / `create_db_as` | db_management.rs:43-80 (`create_db_as` :49-80; direct `dbs.insert` at :51 + best-effort `save_database`; the `#[doc(hidden)]` system-actor hazard is documented on `create_db` — **OBSERVED** `#[doc(hidden)]` at :42, fn at :43, and the SAFETY note at :348-363 on the bare `add_repo` wrapper (`#[doc(hidden)]` :363, `pub async fn add_repo` :364)) | admission `(db, "*")` acquired first; reservation check at the top, before `dbs.insert` | `destination_name_reserved`; no DbInstance inserted |
| 2 | `handle_create_db` wire path | admin_db_repo.rs:18+ | same as (1) — it routes into `create_db_as`; the wire layer adds no second insertion | as (1) |
| 3 | `DbInstance::add_repo` / `add_repo_with_durability` | db_instance.rs:52-75 (recovers/certifies in the constructor, then `repos.insert` :73) | **DECIDED:** direct call on a managed home fails with `destination_name_reserved`; the only allowed engine seam is (4). Config-driven/boot callers go through the boot gate instead | typed refusal before `repos.insert` |
| 4 | `publish_repo_admitted` | db_instance.rs:79-96 (vacant-only insert, debug-checked `RepoWritePermit`) | reservation + admission check inside the seam before the `entry()` call | `destination_name_reserved`; nothing published |
| 5 | `DbInstance::rename_repo` registry re-key | db_instance.rs:176-184 (`remove(from)` + `insert(to)`) | admission on `to` held across both operations (atomicity of the guard makes the re-key check-then-act safe) | `destination_name_reserved`; no re-key |
| 6 | Wire CREATE REPO handler | admin_db_repo.rs:153+ (`repo_create_locks` :219, `create_dir_all` :266/:291) | create-path order (131 §4e site 7): admission guard (sorted) → per-db create mutex (`repo_create_locks`) → control lock, all before `create_dir_all`, with the reservation check under the guard | `destination_name_reserved`; no directory created |
| 7 | Generic managed path `add_repo_as[_with_durability]` | db_management.rs:371-419 (`db.add_repo_with_durability` at :419; catalogue persistence best-effort warn-only :455-460) | admission + reservation check at entry; the `repositories` mirror row (131 §3a) must be written with **error propagation** on this path (the warn-only pattern would break the mirror) | `destination_name_reserved` before any open |
| 8 | Seed install: `check_seed_target` + installer | installer.rs:113-122, seed/catalogue.rs:16-109 | lifecycle preflight (§8c) after the maintenance lock :115, before `find_complete` :117; `check_seed_target` branches keep their reservation check | `destination_name_reserved` / lifecycle refusal |
| 9 | Boot/config publication | core.rs:296-317 (loop, `seed_boot_gate` :303, `add_repo_with_durability` :316) + `publish_repo_admitted` | 131 §4d step 4: reserved targets skipped fail-closed, never opened | skip + log; never publish |
| 10 | Migration publication (experimental API) | core.rs:132-141 gate (`enable_experimental_migration_api`), MigrationCoordinator :19/:80 | **DECIDED:** migration admits two repos per instance; each admitted repo passes the admission+reservation check via seam (4); migration refuses when either target namespace is reserved | migration refused with `destination_name_reserved` |
| 11 | `ShamirDb::remove_db` / `remove_repo` (unguarded on this tree) | db_management.rs:82-100, :519+ | not a creation path; 8b-3b marks them `#[doc(hidden)]`-restricted and R2' calls guarded variants (**REQUIRES #188** `_locked` shape) | n/a (I2 prevents their use for destruction) |
| 12 | CREATE SUBSCRIPTION / RESUME binding | binding validation (**REQUIRES #188** the binding-validation seam) | reservation check on each bound destination name | `destination_name_reserved` |
| 13 | RENAME into a reserved target | `rename_db_as` db_management.rs:139+, `rename_repo_as` :561+, `DbInstance::rename_repo` (row 5), R3 guard (**REQUIRES #188**) | reservation check on target name, admission held | `destination_name_reserved` (R3 wording per 131 §1) |
| 14 | Destruction executor re-publication avoidance | boot loop + executor | covered by 131 §4d (skip + never publish) | n/a |
| 15 | `DbInstance::with_repos` | db_instance.rs:34-48 (**OBSERVED** construct-time insertion) | **DECIDED:** construct-time insertion bypasses the runtime seam; allowed ONLY for boot-time construction before the server accepts requests (the admission registry is empty, reservations already published by 131 §4d step 2); any post-boot caller must go through seam (4) | boot-only constraint; post-boot use rejected in review/CI |
| 16 | `register_seed_image` | crates/shamir-db/src/shamir_db/shamir_db/seed/catalogue.rs:113-198 (**OBSERVED** writes `databases`/`tables`/`repositories` catalogue rows) | **DECIDED:** must take the admission guard (db gate shared + repo-level mutex) + reservation check BEFORE its catalogue writes | `destination_name_reserved`; no catalogue rows written |

**Reads/writes against reserved names (round 3 correction):** the quarantine cache gates READS only (**OBSERVED** db_execute.rs:76-90 reads-only gate; :131-150 walks `BatchOp::Read`) — reserved/quarantined targets get WRITE fencing via the engine write-admission seam (`RepoWriteDomain`/`RepoWritePermit`, **OBSERVED** db_instance.rs:79-96), not via the cache; the cache remains the read gate (reserved targets are never in the live map by I3/I4 of 131).

### 7c. Recovery exception — RESUME / replacement-rebind into a reserved destination (F3-iii)

**DECIDED.** Publication into a reserved destination is permitted **only** for an authorized recovery RESUME / install-retry, and only when **all four** hold:

**DECIDED (round 3 — the ONE recovery path this series):** R2' (destroy) → R1 (release the name) → fresh install into the ABSENT target as a NEW incarnation. In-place replacement of a destroyed destination's storage into the CURRENT incarnation is OUT of scope for this series (listed as a follow-up; 131 §1 non-goals). §8's flows implement exactly this path.

1. The subscription row already binds THIS destination (name match at bind time).
2. The live-or-catalogued `storage_object_id` **equals** the id recorded in the binding/incident (no replacement incarnation).
3. The installed receipt/incident evidence matches (same receipt digest / incident evidence; the exact retry-identity tuple is (db, repo, receipt `artifact_digest`, lineage_id) per §8c).
4. No non-terminal destruction operation covers the name; a `Destroyed` operation covering it is permitted exactly when its recorded target (db, repo) equals the install target (the destroyed-then-reinstall path of §8).

Anything else → `destination_name_reserved`. **Forbidden replacement publication (a NEW object under a reserved name) is never exempt.**

## 8. Seed install changes (8b-3e)

### 8a. Verified current installer ordering (F6 baseline)

**OBSERVED** (installer.rs, all re-verified): Unix gate :92-94; source/destination name-equality check :95-97; `create_dir_all` :113; `canonicalize` :114; `maintenance_lock::acquire` :115; `staging::destination` :116; `find_complete` :117 — which **opens candidate storage** via `staging.rs` `completed/find_complete` → `BoxRepoFactory::fjall_raw(path).create()` (**OBSERVED** staging.rs:60, :78-80, and `open_image` :116-119); `init_seed` :118-121; `check_seed_target` :122; import :142; `staging::publish` :170; `register_seed_image` :176; `subscription.persist` :179. Staging layout (**OBSERVED** staging.rs): `seed-<key>` target dir :14-17; `<target>-attempt-N` dirs :19-42; `.seed-install` marker :32; completed `SeedInstallState`/receipt inside the backing store `__tx__` :60-71; published rename to the final target :121-130 (publish step).

### 8b. Exact ordering change — DECIDED

After the maintenance lock (:115) and **BEFORE** `find_complete` (:117), load lifecycle authority (open the system store journal read-only view + reservations) and run:

```text
lifecycle_preflight(db: &str, repo: &str, receipt: Option<&SeedReceipt>) -> Result<(), LifecycleRefusal>

**DECIDED:** `LifecycleRefusal` is `enum LifecycleRefusal { NameReserved, OwnershipMismatch }`, mapping 1:1 to the wire codes `destination_name_reserved` (§7c recovery exception does not apply) and `unsafe_storage_ownership` (a destroyed/foreign target's ownership verification failed, §8b refusal case 2).
```

**DECIDED:** the signature takes `(db, repo, receipt)` only — `storage_object_id` is resolved INTERNALLY from the journal/ownership records, never passed in by the caller. It (1) **refuses** when an anchor or a non-terminal operation covers the name, UNLESS the recovery exception (§7c) applies to an authorized install-retry; and (2) **verifies NO owned component of a destroyed target still exists** — destroyed-and-installed-over is forbidden (`verify_absent` over the recorded descriptor of the completed `Destroyed` operation; any remnant → refusal, never silent reuse). Only then does the existing `find_complete`/`check_seed_target`/import/publish/register/persist sequence run, order otherwise unchanged.

### 8c. Authorized install retries — DECIDED

Same operation semantics: same `(db, repo, receipt artifact_digest, lineage_id)` → may **resume a completed stage** (the existing `find_complete` path). A DIFFERENT receipt/incident over a name covered by any anchor or non-`Destroyed` operation → `destination_name_reserved`. Retry remnants may be reused only when the receipt matches exactly; **else they are deleted first, within the same admission guard** (§7a).

**DECIDED (round 3) — the three install flows this series supports (the ONE recovery path of §7c):**

1. **Absent-target install:** preflight requires NO component of any PRIOR incarnation (by storage-object id from the destruction journal) and no reservation; the install creates a NEW incarnation (fresh storage_object_id).
2. **Interrupted-new-install retry:** the install journal records the NEW attempt's storage-object id (the new Appendix-A install-journal record); preflight accepts components owned by THAT attempt and rejects only foreign / old-incarnation ones.
3. **Remnant classification:** owned by this attempt → resume or delete; owned by a destroyed prior incarnation → delete as destruction completion; FOREIGN → REFUSE, never delete (this resolves the §8d table contradiction).

### 8d. Seed staging/publication remnant inventory (ownership + deletion rules)

**DECIDED:** all seed remnants are **OWNED COMPONENTS** of the target's ownership descriptor when the target is seeded (component kinds SeedStagingDir, SeedInstallMarker, SeedReceiptStore; 131 §3a). Destruction destroys them (I5); install retries may only reuse remnants whose receipt matches exactly, else delete them first (within the same admission).

| Remnant | Where (OBSERVED) | Ownership rule | Deletion rule |
|---|---|---|---|
| `seed-<key>` target dir | staging.rs:14-17 | SeedStagingDir component of the target | destroyed by R2' step 4; retried install deletes stale dirs with mismatched receipt |
| `<target>-attempt-N` staging dirs | staging.rs:19-42 | SeedStagingDir (one descriptor entry per attempt dir recorded at preflight) | destroyed with the target; incomplete attempts deleted by a matching-receipt retry |
| `.seed-install` marker | staging.rs:32 | SeedInstallMarker | as above |
| completed `SeedInstallState`/receipt in backing store `__tx__` | staging.rs:60-71 | SeedReceiptStore (lives inside the opened candidate store — reachable only because `find_complete` opens storage; hence preflight MUST run before :117) | classified per the §8 remnant-classification rule (round 3): owned by THIS attempt → resume or delete; owned by a DESTROYED prior incarnation → delete as destruction completion; FOREIGN → REFUSE, never delete |
| published rename (attempt dir → final target) | staging.rs publish step (:121-130 region) | the final path IS the PrimaryStore + sibling WAL descriptor | standard R2' component destruction |

**§8d contradiction resolution (round 3):** the earlier "retry deletes stale/mismatched dirs" rows and the "non-matching receipts make the store a foreign object → refusal" row are resolved by the three-way remnant classification above — deletion happens only for remnants owned by this attempt or by a destroyed prior incarnation; anything FOREIGN (a live different incarnation, an unowned object) is REFUSED, never deleted (I6 of 131).

### 8e. Other seed-install items (unchanged from round 1, re-affirmed)

- **Repo-scoped installation into an existing database:** new `ShamirDb` API + installer mode touching only the target repository; preflight requires the target ABSENT or `Destroyed`-with-journal-evidence; honours reservations (I4); reuses the maintenance lock (installer.rs:115) for the instance-exclusive window.
- **Database-wide rule replacement at seed/catalogue.rs:54-105 — DECIDED (round 3, **OBSERVED**):** the current code validates database-wide (every repository row/path/marker). Replacement: repo-scoped install validates ONLY the target repository — its catalogue row, its path, its `.seed-install` marker, and its completed state; unrelated repositories and other tables are untouched and unvalidated. STAYS enforced: the per-repo ordinary-repository refusal for a live non-seed target, the reservation/lifecycle preflight (§8b), receipt equality for the target, and the unix gate (installer.rs:92-94).
- Source==destination name equality kept and documented as a limitation until PR8b-4 (**OBSERVED** installer.rs:95-97; seed/catalogue.rs:38-41 in `check_seed_target_inner`).
- Single-stream seed scope stays: `matches_scope` accepts exactly one Pull, ReadOnly, single-target stream (**OBSERVED** crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:50-57). Documented; multi-target supported path is "destroy and recreate as single-target subscriptions".
- Runbook rewrite plan for `docs/guide-docs/guide/09-seed-recovery.md` (**OBSERVED** :28-42 currently instructs DROP of the quarantined subscription, which #188's R1 refuses): (s0) truthful statement of what this version supports; then R2' per destination → R1 (after stage-appropriate preconditions) → repo-scoped install → Resume; abandonment stop-point is "stop after R1" — names are released, no reseed needed; Windows: destruction + name release supported, reseed NOT (installer Unix-only, **OBSERVED** installer.rs:92-94). The new-data-directory bootstrap procedure (:103-145) stays as an alternative.

## Appendix A — durable-schema appendix: all records, types, bounds (F7-ii, normative)

Common conventions: names (db/repo/subscription/profile) ≤ 255 bytes UTF-8, no NUL/C0; paths ≤ 4096 bytes; reason 1..=512 chars printable UTF-8; operation_id ≤ 64 chars `[0-9A-Za-z._-]+`; actor ≤ 255 bytes printable UTF-8; ids/digests fixed-size byte arrays; generations/timestamps u64 (ns since UNIX_EPOCH for timestamps); every record carries `format_version: u16`.

**A.1 `MetaKey::StorageOwnership` (repo info store, MetaEnvelope v1)**

```text
{ storage_object_id: [u8;16], durable: bool,
  components: Vec<{ component_id: u32, kind: ComponentKind,
                    path: Option<PathBuf(≤4096)>, bytes_repr: Option<String> }>,
  created_at: u64 (ns), format_version: u16 = 1 }
```

**A.2 `destruction_operations` row (system store table, v1)** — the full row is specified in 131 §3b; bounds per the common conventions; `record_version` starts at 1 and increments on every CAS write. **Round-3 note:** the operation row now carries `nonce: [u8;16]`, `incident_refs: Vec<IncidentRef>` (empty in stage 1) and `audit_pending: Vec<PendingAuditEvent>` (the in-record outbox; 131 §3b/§3e).

**A.2a Phase enum + safe-restart mapping (DECIDED round 3):** `Phase ∈ { Preflight, Prepare, Quiesce, Detach, Destroy, Catalogue, Complete }`. The phase→safe_restart_phase mapping: a `Blocked` during phase X restarts at X (via `safe_restart_phase`). All phases are idempotent. `Destroy` restarts at `Destroy` with the absence re-check (per-component `verify_absent`); `Catalogue` restarts at `Catalogue` with the I14 catalogue re-read (131 §4a step 5).

**A.2b Install-journal record (new, v1 — DECIDED round 3):**

```text
{ attempt_id: String(≤64), storage_object_id: [u8;16], db: String(≤255),
  repo: String(≤255), receipt_digest: [u8;32], created_at: u64 (ns),
  format_version: u16 = 1 }
```

Records the NEW attempt's storage-object id so an interrupted install's preflight accepts components owned by THAT attempt only (§8c flow 2).

**A.3 `destination_incidents` row (system store table, v1)** — 131 §3d; `incident_id`/`storage_object_id` `[u8;16]`; `cause.generation` u64; `cleared_by.evidence_digest` `[u8;32]`; plus the recovery baseline fields (**DECIDED round 3**): `receipt_artifact_digest: [u8;32]`, `source_lineage_id: [u8;16]`, `source_history_epoch: u64`, `bookmark_floor: u64` (plus cause/reason/timestamps/storage_object_id/db/repo as above).

**Recovery-clear predicate (EXACT, normative):** the incident is resolved ONLY when a replacement incarnation has `storage_object_id ≠` the incident's, its installed receipt `artifact_digest ==` the recorded `receipt_artifact_digest`, its `lineage_id ==` the recorded `source_lineage_id`, its `history_epoch ==` the recorded `source_history_epoch`, and its new bookmark `== bookmark_floor + 1`. The clear rewrites EXACTLY that incident's copies (row + repo record) by `incident_id`. Deletion of a subscription NEVER resolves an incident.

**A.4 `MetaKey::ConsistencyIncidents` (repo info store, MetaEnvelope v1)** — **DECIDED round 3:** the repo-carried record holds the FULL incident copy (ALL A.3 fields, incl. the recovery baseline), not only id + digest, so missing catalogue rows are rebuilt from it at boot when the repo is openable:

```text
{ unresolved: Vec<IncidentCopy>,   // IncidentCopy = the full A.3 row (all fields)
  format_version: u16 = 1 }
```

**Degraded-copy rule:** repo unopenable ⇒ the catalogue copy alone is authoritative. The R1-stage-2 handoff completes only when the catalogue copy is verified AND (the repo copy is verified OR the destination is destroyed). Retry is idempotent by `(cause, storage_object_id)` with incident-id reuse (131 §3d).

**A.5 Audit event envelope (extension of `AuditEntry`)** — `details_canonical_msgpack` payload:

```text
{ event_id: { operation_id: String(≤64), transition_ordinal: u32 },
  operation_kind: "drop_repo" | "drop_db",
  storage_object_ids: Vec<[u8;16]>,
  anchors: Vec<{ name: String(≤255), generation: u64 }>,
  actor: String(≤255), reason: String(1..=512), target_state: enum, outcome: enum }
```

**Round-3 id note:** `event_id = { operation_id, transition_ordinal }` — `record_version` and `target_state` are NOT part of the id (`target_state` remains a separate payload field). A retried `Blocked` emits a second `blocked` event for the same state, so distinct ordinals are required for idempotent replay/reconciliation (131 §3e reconciliation rule).

Enums (normative): `event_kind ∈ { prepared, rejected, quiesced, catalogued, destroyed, blocked }` — there is NO `destroying` event kind: the `Destroying` transition emits no dedicated audit event (131 §3e); `destroyed` is the terminal event kind for `Destroyed`; a retired `Blocked`'s final `blocked` event likewise gates retirement (131 §3e). The event kind names map to the operation state whose completion they record (`prepared` records `Prepared` reached, `catalogued` records `Cataloguing` finished, etc.). `target_state ∈` operation states (the 3b state enum); `outcome ∈ { emitted, replayed, acked }`.

**A.6 Wire DTO `destroy_quarantined_data`** — `{ reason: String(1..=512, printable), operation_id: String(≤64), nonce: String (32 hex chars) }`; echoed back as `{ operation_id, state, nonce }`.

**A.7 `AuditAck`** — `{ seq: u64, checkpoint_position: [u8;32] }` (131 §3e).

**A.7a `checkpoint_durable` (DECIDED round 3):** `checkpoint_durable() -> Result<AckWatermark, AuditError>` — flushes + fsyncs the log FIRST and FAILS if the flush failed (unlike the OBSERVED void `appender.checkpoint()`, audit_appender.rs:706-713, which logs the flush failure and still writes the checkpoint), then writes the checkpoint keyspace durably (SyncAll) and returns `AckWatermark = next_seq − 1` — the highest assigned seq covered by the durable log + checkpoint. An outbox event is acknowledged only when `ack_watermark >= event.audit_seq`; the periodic scheduler (scheduler.rs:204-216) switches from void `appender.checkpoint()` to `checkpoint_durable` + the pending-removal hook (one durable CAS batch per checkpoint). `checkpoint_now` (audit_chain.rs:436-441) is a manual force, NOT a durability barrier.

## Appendix B — administrative inspect/retry API (F7-iv, DECIDED)

On the existing admin surface (wire DTOs alongside the other `destruction*` operations; the HTTP/wire handler returns immediately — status is a pure read, retry spawns a detached executor per §4f of 131):

- `destruction_status(operation_id) -> { state, failed_phase?, error_class?, safe_restart_phase?, targets: Vec<{db, repo, storage_object_id}>, progress: Vec<{component_id, kind, absent: bool | "unknown"}> — **DERIVED** from `verify_absent` probes at status time, not durably stored, audit_acked: bool — true when `audit_pending` is empty (all outbox entries drained), anchors_remaining }` — a **pure read of `destruction_operations`** (+ a `verify_absent` probe only when the caller asks for `?verify=true`); a lost response is answered by re-issuing the call (idempotent, no side effects).
- `destruction_retry(operation_id)` — resumes a `Blocked` operation from `safe_restart_phase` under a fresh executor token; refuses on terminal states (`Destroyed` → status; `Rejected` → new operation required); authorization required.
- `destruction_list(limit: u32, pagination_token: Option<String>) -> { items: Vec<{ operation_id, state, targets }>, next_token: Option<String> }` — bounded (≤ `limit` items), ordered by `created_at`, paginated via the opaque `pagination_token`/`next_token` pair.
- Optional (8b-3d+): `destruction_archive(operation_id)` — owner-approved archival removing a terminal, fully-acked row (the only sanctioned path to shrink the journal; 131 §3e retention rule).

**Authorization:** same **Delete** authority as DROP **plus** replication-management authority. **Idempotence/safety:** status/list are pure reads; retry is idempotent for an already-running operation (returns current state); no admin call can bypass the audit barrier or delete a journal row with an unacked terminal event.
