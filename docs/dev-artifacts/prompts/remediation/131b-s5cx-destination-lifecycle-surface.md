# 131b — destination lifecycle: DDL/wire surface, creation exclusion, seed install, schemas, admin API

Companion to [`131-s5cx-destination-lifecycle.md`](131-s5cx-destination-lifecycle.md) (§1-5: goal, invariants, data model, protocols, backend contract) and [`131a-s5cx-destination-lifecycle-evidence.md`](131a-s5cx-destination-lifecycle-evidence.md) (§9-12: CI evidence, PR staging, risks, PR8b-4 appendix). This file holds §6-8 plus the durable-schema appendix (A) and the administrative inspect/retry API (B). Citation discipline: **OBSERVED** = verified in this worktree at merge commit 95c825f7 (master incl. #188/#189); **REQUIRES #188** = a still-future stage dependency with the contract named; **DECIDED** = binding design choice; **INFERRED** = derived consequence.

## 6. DDL / wire / builder / client surface

### 6a. Grammar and DTOs

**Grammar sketch (DECIDED):**

```text
DROP REPOSITORY <name> [CASCADE] [IF EXISTS] [DESTROY QUARANTINED DATA '<reason>' OPERATION ID '<op-id>' NONCE '<32 hex chars>' DIGEST '<64 hex chars>' CHALLENGE '<preview challenge>'];
DROP DATABASE <name> [CASCADE] [IF EXISTS] [DESTROY QUARANTINED DATA '<reason>' OPERATION ID '<op-id>' NONCE '<32 hex chars>' DIGEST '<64 hex chars>' CHALLENGE '<preview challenge>'];
```

**Round 4 (D4):** the `DESTROY QUARANTINED DATA` modifier additionally carries `digest` + `challenge` from the mandatory preceding `destruction_preview` call (below); the HMAC covers the digest (§6b tag 0x0B).

- `CASCADE` controls child scope only; it never authorizes crossing replication bindings or physical destruction.
- **DTO (wire):** optional field `destroy_quarantined_data: Option<{ reason: String (nonempty, 1..=512, printable UTF-8), operation_id: String (≤64, [0-9A-Za-z._-]+), nonce: String (exactly 32 lowercase hex chars — the hex form of 16 bytes), digest: String (exactly 64 lowercase hex chars), challenge: String (from destruction_preview) }>` on `DropRepoOp` (**OBSERVED** crates/shamir-query-types/src/admin/types/repo_ops.rs:43-55: no `deny_unknown_fields` on the Drop ops) and `DropDbOp` (**OBSERVED** crates/shamir-query-types/src/admin/types/db/db_ops.rs:27-40). Unknown-field tolerance on Drop ops is intentional for forward-compatible builders; the new field is `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- **Builder (Rust):** `.destroy_quarantined_data(reason, operation_id, nonce, digest, challenge)` on the drop builders — **OBSERVED** current locations `crates/shamir-query-builder/src/ddl/repo/drop_repo.rs` and `crates/shamir-query-builder/src/ddl/db/drop_db.rs` (panel paths `ddl/drop_repo.rs` are stale). HMAC computation moves behind the modifier check.
- **TS client:** `.destroyQuarantinedData(reason, operationId, nonce, digest, challenge)` on the drop builders in `crates/shamir-client-ts/src/core/builders/ddl/` (**OBSERVED** `ddl-schema.ts` holds `dropDb` at :83-97 and `dropRepo` at :99-113, both computing the canonical bytes via `canonicalDropDb`/`canonicalDropRepo` imported from `core/protocol/hmac.ts`); request flows through `crates/shamir-client-ts/src/core/client/client.ts` unchanged.
- `IF EXISTS` semantics per 131 §4a (inspect incomplete operations first; **OBSERVED** early exits admin_db_repo.rs:106-112 and :367-368).
- `nonce`: supplied by the caller; the wire DTO carries `nonce: String` (32 hex chars) inside `destroy_quarantined_data`; **echoed verbatim in the response** (`{ operation_id, state, nonce }`), and bound into the v2 canonical encoding (the canonicalizer encodes the 16 decoded bytes) — a replayed confirmation over different nonces fails, and a lost response cannot be confused with a different request. The nonce is still consumed ONCE per terminal outcome (existing rule, unchanged in round 4); the CHALLENGE (below) is a separate, server-issued single-use credential.
- **Destruction preview — DECIDED (round 4, the two-step confirmation exchange):** new admin wire op `destruction_preview(targets, cascade)` → response `{ targets: Vec<{db, repo, storage_object_id}> (canonical order: sorted by (db, repo)), digest: [u8;32] — SHA-256 over the canonical length-prefixed encoding of the target list (the encoding described in §6b for tag `0x0B`; the same encoder, used standalone), challenge: String — short-lived, server-issued, SINGLE-USE, expires_at_ns (≤ 60 s ahead) }`. The client MUST call `destruction_preview` first and sign the returned digest + challenge.
- **TS client:** new method `.destructionPreview(targets, cascade)` (returns the preview response); `.destroyQuarantinedData(reason, operationId, nonce, digest, challenge)` — the builder carries the preview's digest + challenge into the DTO and the canonical encoding (§6b fields 0x0B/0x0C).
- **Builder (Rust):** `.destroy_quarantined_data(reason, operation_id, nonce, digest, challenge)` on the drop builders, plus a Rust admin-call analogue of `destruction_preview(targets, cascade)`; `digest` is the 32-byte value (hex on the wire).
- **Challenge rules:** expiry ≤ 60 s, single use, consumed at Prepare. **DECIDED (round 5, P2-4): the challenge store/format** — a new system-store table `destruction_challenges` (v1): `{ challenge_id: [u8;32] /* random */, digest: [u8;32], session_binding: [u8;32] /* HMAC-key fingerprint of the issuing session */, created_at_ns: u64, expires_at_ns: u64 }`. SINGLE USE is enforced by a durable DELETE-IF-MATCHES (challenge_id + digest) at the Prepare write — not by a per-operation `challenge_consumed` boolean (a boolean cannot stop two concurrent prepares; a durable delete is atomic). An expired, unknown or already-deleted challenge → `destruction_target_changed` (remedy: re-run `destruction_preview`). Expired rows are swept opportunistically; deletion is the single-use mechanism.
- **Preview authorization — DECIDED (round 5, P2-4):** `destruction_preview` requires the SAME authority as the destroy it precedes (Delete + replication-management), and the challenge it mints is SESSION-KEYED (`session_binding` above) — a challenge minted in one session is unusable in another; the digest is otherwise a pure read.
- **Actor — DECIDED (round 5, P2-4):** the actor is ALREADY in the canonical bytes (tag `0x08`); the SERVER recomputes the canonical encoding from the authenticated session's canonical actor string and REJECTS a client-supplied actor that differs (the client value is informational); the TS client obtains the actor from the session/hello response (the exact server actor string is exposed there — if the hello does not currently carry it, 8b-3c adds it to the hello payload; the shared test vectors pin the string).
- **Verification seam:** the digest is what `check_destructive_hmacs` (**OBSERVED** crates/shamir-server/src/db_handler/admin.rs:636-659) can verify WITHOUT a Prepared operation — the HMAC is over the digest (not only the logical names), so the seam verifies it session-keyed exactly as today; the executor's re-resolution of the live/catalogued set against the signed digest at Prepare (131 §4a preflight) is the second half.

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
| 10 | 0x0B | digest | when destroy_flag = 1 | exactly 32 bytes — SHA-256 over the canonical encoding of the target list (round 4; **round 5, P2-4: tag `0x0A` is DROPPED/UNASSIGNED — the wire seam cannot build a targets field because the frozen target list is not in the drop DTO; the digest (0x0B) alone commits to the list, and the executor compares its recomputed digest of the resolved live/catalogued set against the signed digest at Prepare**) |
| 11 | 0x0C | challenge | when destroy_flag = 1 | challenge string (UTF-8; round 4) |

Injectivity: length prefixes make distinct field sequences encode distinctly (no NUL-delimiter ambiguities); fixed field order + explicit `destroy_flag` makes a plain DROP (`destroy_flag = 0`, fields 6-7 absent) byte-distinct from any destruction confirmation. **v2 scope — DECIDED (round 5, P2-4): v2 replaces the NUL forms for the drop_db/drop_repo confirmations ONLY in this series**; other destructive confirmations (if any exist) are out of scope and keep their current encodings. Plain drops get `destroy_flag = 0` and omit the optional fields; a plain-DROP HMAC therefore can never authorize destruction. Plain drops carry no nonce — the nonce exists only inside `destroy_quarantined_data`, so it is encoded only when `destroy_flag = 1` (fields 6-7, 9-11 all absent for a plain drop). **Round-4/5 (D4/P2-4):** the v2 canonical encoding BINDS the digest + challenge — the HMAC covers the DIGEST (tag `0x0B`, 32 bytes of SHA-256 over the canonical length-prefixed target-list encoding — the same encoding `destruction_preview` uses, computable at the server seam from the preview response), not only the logical names, and the server-issued single-use challenge (tag `0x0C`); a stale preview (the live/catalogued set changed after `destruction_preview`) fails closed at Prepare with `destruction_target_changed`. The frozen target list itself lives in the `destruction_preview` response, the `destruction_challenges` row (below) and the `Prepared` operation — never in the drop DTO.

**Validation (server and client):** reject NUL and C0 control characters in `reason`, `operation_id`, `actor`; enforce the Appendix-A bounds; `nonce` must decode from exactly 32 hex chars to 16 bytes; `digest` must decode from exactly 64 hex chars to 32 bytes (round 4); `field_count` and `version` must match this table exactly (unknown tags/versions → reject).

**Code touchpoints (all must change together):**

- **Rust canon module:** `crates/shamir-query-types/src/security/hmac.rs` — **OBSERVED** `canonical_drop_db` at :101 and `canonical_drop_repo` (same module; doc table of the NUL forms at :30-31; round-1 tests hmac_tests.rs:29-32 pin `b"drop_db\0mydb"` / `b"drop_repo\0mydb\0cold"` and must be rewritten to v2 vectors). Replacement functions: `canonical_drop_db_v2(DropDbConfirm) -> Vec<u8>` / `canonical_drop_repo_v2(DropRepoConfirm) -> Vec<u8>` over a shared `struct ConfirmV2 { kind, db, repo: Option<String>, cascade, destroy: Option<DestroyConfirm{reason, operation_id, nonce, digest, challenge}>, actor }` — **NO `targets` member (round 5, P2-4: tag 0x0A dropped; the digest commits to the list)** — plain drops carry none of the destroy fields. The old functions are deleted, not deprecated. **Update the field table and `ConfirmV2` everywhere they appear (Rust canon module, TS hmac.ts builders, shared vectors) with tags `0x0B` and `0x0C` (0x0A unassigned).**
- **Server verification seam:** `crates/shamir-server/src/db_handler/admin.rs:655-659` — **OBSERVED** the match arms `BatchOp::DropDb(op) => (canon::canonical_drop_db(&op.drop_db), op.hmac.as_ref())` and the `DropRepo` analogue inside the destructive-op HMAC gate. These arms construct `ConfirmV2` from the op (+ actor + nonce) and call the v2 functions. **BatchOp handling:** the gate iterates `BatchTreeVisitor` over the batch (**OBSERVED** admin.rs:652) — when `destroy_quarantined_data` is present on a drop op, the arm builds the v2 form with `destroy_flag = 1` and the dispatch additionally routes to the R2' protocol instead of the ordinary drop path; plain drops use `destroy_flag = 0`. A batch mixing a destruction drop with other ops is verified op-by-op as today. **Round 4:** because the HMAC covers the preview DIGEST, this seam verifies the confirmation WITHOUT a Prepared operation, exactly as today (session-keyed); the executor's re-resolution at Prepare is the second half of the check (131 §4a preflight).
- **TS client canonicalizers:** `crates/shamir-client-ts/src/core/protocol/hmac.ts` — **OBSERVED** `canonicalDropDb` at :97 and `canonicalDropRepo` at :101 — replaced by v2 builders mirroring the Rust encoder (shared test vectors, see §6c); the builders in `core/builders/ddl/ddl-schema.ts` (:83-97, :99-113) pass the new canonical bytes through `signer.hmacTagHex` unchanged in shape.
- **Test vectors:** add shared JSON test vectors (Rust + TS) covering: plain drop, destruction drop, each omission, overlong/boundary lengths, control-char rejection, nonce mismatch, digest mismatch, stale/consumed challenge (round 4). The old pinned NUL expectations (hmac_tests.rs:29-32; `crates/shamir-client-ts/src/core/protocol/__tests__/hmac.test.ts:42-45`) are replaced.

### 6c. Error codes — the eleven typed wire codes (DECIDED, normative; round 4 added `destruction_target_changed`)

Survive dispatch incl. rename (**round 5 (B14):** `db_error_to_batch` exists on this tree — **OBSERVED** execute/helpers.rs:85). Each `DbError` variant maps 1:1 to its wire code; the remedy contract is what the client/operator does on receipt:

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
| `DestructionAuditUnavailable` | `destruction_audit_unavailable` | The audit chain could not durably record the intent; retry the same operation id — the resubmission RE-RUNS the audit barrier (idempotent by event id) and spawns the executor — or call `destruction_retry` (accepts `Prepared`, round 4). (The operation STAYS `Prepared`, reservations PUBLISHED — round 5 P1-6 — nothing deleted — 131 §3e item 3.) |
| `DestructionTargetChanged` | `destruction_target_changed` | The resolved live/catalogued target set no longer matches the signed preview digest, or the challenge is stale/expired/consumed (round 4); re-run `destruction_preview` and retry with the fresh digest/challenge (the retry is a NEW operation submission — a `Rejected` row requires a fresh operation id). |

`ErrorClass` mapping (journal `error_class` field → wire code, 131 §3b): `UnsupportedBackend → storage_destruction_unsupported`; `OwnershipMismatch → unsafe_storage_ownership`; `ExternalLock` / `PermissionDenied` / `UnknownIo → destruction_blocked`.

## 7. Creation exclusion matrix — complete (F3)

Reservation check = `(db, repo)` covered by anchors or non-`Destroyed`/non-`Rejected` operations (131 §3c; `repo = "*"` = database-scope reservation covering every `(db, *)`).

### 7a. NamespaceAdmission guard — DECIDED (round 3: hierarchical admission, merged with the lifecycle exclusion; serializer fixed round 5)

- **Two-level hierarchy:** a per-database RW-style gate (repo-scope operations take it SHARED; database-scope operations EXCLUSIVE) plus per-(db, repo) admission mutexes beneath it, held in an in-process registry on `ShamirDb`: `admissions: DashMap<(String, String), Arc<tokio::sync::Mutex<()>>>`. A publication acquires the db gate, then the mutexes for its (db, repo) namespaces (or `(db, "*")` for database scope) in SORTED (db, repo) order (deadlock-freedom), **BEFORE any validation**, and holds them until publication or abort; RAII drop = abort; cancellation-safe; **no durable trace**. **DECIDED (round 5, P3):** the DashMap ENTRY is REMOVED when the guard drops and the mutex is uncontended (`try_lock`-probe on release; entries for hot namespaces may be retained under a bounded-size policy — never an unbounded leak). **DECIDED (round 4):** the db gate is acquired ONCE, in its FINAL mode (SHARED for repo-scope operations and publishers; EXCLUSIVE for a destruction of kind `Database`) and held to the end of the operation — no shared → exclusive upgrade, no release/reacquire while repository mutexes are held (deadlock proof in 131 §4e). A destruction of a single repository takes the db gate SHARED plus its repository mutex for its whole lifetime and does NOT need the exclusive gate.
- **ONE mechanism with the per-target lifecycle exclusion (round 3):** the same admission-guard object serves as the per-target lifecycle exclusion, held from preflight start until `Destroyed`/`Rejected`/`Blocked`-release (131 §4e).
- **Order relative to the merged locks — round 5:** the admission gates/mutexes sit OUTSIDE (before) the replication control lock and the lease gate; the total order is 131 §4e: token → db gate → sorted repo mutexes → control → lease `apply_gate.write()` → `RepoWriteDomain::admit()`. Reservation mutation and every publication-path reservation CHECK run under a short control section inside the already-held gates (131 §3c/I13 round 5).
- **The round-2 generation-counter fallback is REMOVED** — no generation token, no belt-and-braces re-check with token invalidation, no second invalidation path; the control-lock-serialized reservation set (I13 of 131, round 5) is the ONLY serialization of check-vs-publish.
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
| 6 | Wire CREATE REPO handler | admin_db_repo.rs:153+ (`repo_create_locks` :267, `create_dir_all` :314/:339) (**Citation re-verified (round 5)**) | create-path order (131 §4e, CREATE REPO path paragraph): admission guard (sorted) → per-db create mutex (`repo_create_locks`) → control lock, all before `create_dir_all`, with the reservation check under the guard | `destination_name_reserved`; no directory created |
| 7 | Generic managed path `add_repo_as[_with_durability]` | db_management.rs:371-419 (`db.add_repo_with_durability` at :419; catalogue persistence best-effort warn-only :455-460) | admission + reservation check at entry; the `repositories` mirror row (131 §3a) must be written with **error propagation** on this path (the warn-only pattern would break the mirror) | `destination_name_reserved` before any open |
| 8 | Seed install: `check_seed_target` + installer | installer.rs:113-122, seed/catalogue.rs:16-109 | **install runs UNDER the replication control lock** (merged `install_seed_subscription` holds control for the whole install, replication_seed/catalogue.rs:161); lifecycle preflight (§8c) after the maintenance lock :115 and inside the control section, before `find_complete` :117; `check_seed_target` branches keep their reservation check (control-serialized, I13) | `destination_name_reserved` / lifecycle refusal |
| 9 | Boot/config publication | core.rs:329-397 (loop, `seed_boot_gate` :381, `add_repo_with_durability` :394) (**Citation re-verified (round 5)**) + `publish_repo_admitted` | 131 §4d step 4: reserved targets skipped fail-closed, never opened | skip + log; never publish |
| 10 | Migration publication (experimental API) | core.rs field :141-143, `enable_experimental_migration_api` :672+ (**Citation re-verified (round 5)**), MigrationCoordinator :19/:80 | **DECIDED:** migration admits two repos per instance; each admitted repo passes the admission+reservation check via seam (4); migration refuses when either target namespace is reserved | migration refused with `destination_name_reserved` |
| 11 | `ShamirDb::remove_db` / `remove_repo` (**round 5 (B14): merged guarded variants exist** — `remove_db_locked` db_management.rs:95, `remove_repo_locked` :561, both taking `&ReplicationControlGuard`) | db_management.rs:82-100, :549-561 | not a creation path; 8b-3b adds `remove_repo_locked_destructive` (the `DestructionAuthority` variant, Appendix B) beside the merged guarded variants | n/a (I2 prevents their use for destruction) |
| 12 | CREATE SUBSCRIPTION / RESUME binding | binding validation (merged: the handler takes control first, admin_replication.rs:295) | reservation check on each bound destination name, under the control section (I13) | `destination_name_reserved` |
| 13 | RENAME into a reserved target | `rename_db_as` db_management.rs:165, `rename_repo_as` :622, `DbInstance::rename_repo` (row 5), R3 guard | reservation check on target name under the rename's control section (merged: control first), admission gates already held | `destination_name_reserved` (R3 wording per 131 §1) |
| 14 | Destruction executor re-publication avoidance | boot loop + executor | covered by 131 §4d (skip + never publish) | n/a |
| 15 | `DbInstance::with_repos` | db_instance.rs:34-48 (**OBSERVED** construct-time insertion) | **DECIDED:** construct-time insertion bypasses the runtime seam; allowed ONLY for boot-time construction before the server accepts requests (the admission registry is empty, reservations already published by 131 §4d step 2); any post-boot caller must go through seam (4) | boot-only constraint; post-boot use rejected in review/CI |
| 16 | `register_seed_image` | crates/shamir-db/src/shamir_db/shamir_db/seed/catalogue.rs:113-198 (**OBSERVED** writes `databases`/`tables`/`repositories` catalogue rows) | **DECIDED:** runs under the total order (131 §4e): db gate (shared) → repo-level admission mutex → control lock → reservation check BEFORE its catalogue writes | `destination_name_reserved`; no catalogue rows written |

**Reads/writes against reserved names (round 3 correction):** the quarantine cache gates READS only (**OBSERVED** db_execute.rs:76-90 reads-only gate; :131-150 walks `BatchOp::Read`) — reserved/quarantined targets get WRITE fencing via the engine write-admission seam (`RepoWriteDomain`/`RepoWritePermit`, **OBSERVED** db_instance.rs:79-96), not via the cache; the cache remains the read gate (reserved targets are never in the live map by I3/I4 of 131).

### 7c. Recovery exception — RESUME / replacement-rebind into a reserved destination (F3-iii)

**DECIDED.** Publication into a reserved destination is permitted **only** for an authorized recovery RESUME / install-retry, and only when **all four** hold:

**DECIDED (round 3 — the ONE recovery path this series):** R2' (destroy) → R1 (release the name) → fresh install into the ABSENT target as a NEW incarnation. In-place replacement of a destroyed destination's storage into the CURRENT incarnation is OUT of scope for this series (listed as a follow-up; 131 §1 non-goals). §8's flows implement exactly this path.

1. The subscription row already binds THIS destination (name match at bind time).
2. **(round 5, P2-8 — applies ONLY when the destination is LIVE or CATALOGUED):** the live-or-catalogued `storage_object_id` **equals** the id recorded in the binding/incident (no replacement incarnation). For an ABSENT destination this item is vacuous and the exception reduces to items 1, 3 and 4 (there is no live object to compare).
3. The installed receipt/incident evidence matches (same receipt digest / incident evidence; the exact retry-identity tuple is (db, repo, receipt `artifact_digest`, lineage_id) per §8c).
4. No non-terminal destruction operation covers the name; a `Destroyed` operation covering it is permitted exactly when its recorded target (db, repo) equals the install target (the destroyed-then-reinstall path of §8). **(round 5, P2-8: this item is LIVE text because item 2 no longer requires a live/catatogued object — an absent target whose covering operation completed passes item 4, which is precisely the fresh-incarnation reinstall.)**

Additionally, an ABSENT destination with NO covering destruction operation still reserves (131 §4c (a) round 5) and is NOT admitted by this exception — it needs a validated recovery or an explicit destruction first.

Anything else → `destination_name_reserved`. **Forbidden replacement publication (a NEW object under a reserved name) is never exempt.** **Round 4 (D3):** a completed destruction resolves the destination's incidents (`Resolved(by destruction)`, 131 §4a step 6) and R1 records NOTHING for an absent destination with a completed destruction operation — the name is genuinely released after R1, and item 4 is what admits the fresh-incarnation reinstall.

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

1. **Absent-target install:** preflight requires NO component of any PRIOR incarnation (by storage-object id from the destruction journal) and no reservation; the install creates a NEW incarnation (fresh storage_object_id). **Round 4:** the name is free here because destruction completion resolved the incidents (`Resolved(by destruction)`) and R1 released the reservation without recording an incident for the absent destination (131 §4c (a), §4a step 6).
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
- Single-stream seed scope stays: `matches_scope` accepts exactly one Pull, ReadOnly, single-target stream (**OBSERVED** crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:52-58 (**Citation re-verified (round 5)**). Documented; multi-target supported path is "destroy and recreate as single-target subscriptions".
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

**A.2 `destruction_operations` row (system store table, v1)** — the full row is specified in 131 §3b; bounds per the common conventions; `record_version` starts at 1 and increments on every CAS write; `executor_epoch` starts at 0 and increments on every claim (round 5). **Round-3 note:** the operation row now carries `nonce: [u8;16]`, `incident_refs: Vec<IncidentRef>` (empty in stage 1) and `audit_pending: Vec<PendingAuditEvent>` (the in-record outbox; 131 §3b/§3e). **Round-4 note:** the row additionally carries `target_digest: [u8;32]` (the signed preview digest, frozen at Prepare) and `challenge_consumed: bool` (DIAGNOSTIC copy — the authoritative single-use mechanism is the `destruction_challenges` delete-if-matches, §6a round 5).

**A.2a `destruction_challenges` table (new, v1 — round 5, P2-4):** `{ challenge_id: [u8;32], digest: [u8;32], session_binding: [u8;32], created_at_ns: u64, expires_at_ns: u64 }` — single use enforced by durable DELETE-IF-MATCHES at Prepare; expiry sweep is opportunistic (§6a).

**A.2c Phase enum + safe-restart mapping (DECIDED round 3):** `Phase ∈ { Preflight, Prepare, Quiesce, Detach, Destroy, Catalogue, Complete }`. The phase→safe_restart_phase mapping: a `Blocked` during phase X restarts at X (via `safe_restart_phase`). All phases are idempotent. `Destroy` restarts at `Destroy` with the absence re-check (per-component `verify_absent`); `Catalogue` restarts at `Catalogue` with the I14 catalogue re-read (131 §4a step 5).

**A.2b Install-journal record (new, v1 — DECIDED round 3):**

```text
{ attempt_id: String(≤64), storage_object_id: [u8;16], db: String(≤255),
  repo: String(≤255), receipt_digest: [u8;32], created_at: u64 (ns),
  format_version: u16 = 1 }
```

Records the NEW attempt's storage-object id so an interrupted install's preflight accepts components owned by THAT attempt only (§8c flow 2).

**A.3 `destination_incidents` row (system store table, v1)** — 131 §3d; `incident_id`/`storage_object_id` `[u8;16]`; `cause.generation` u64; `cleared_by: Option<ClearedBy>` where `ClearedBy = ByRecovery { evidence_digest: [u8;32], cleared_at_ns: u64 } | ByDestruction { operation_id: String(≤64), cleared_at_ns: u64 }` (round 4); plus the recovery baseline fields (**DECIDED round 3**): `receipt_artifact_digest: [u8;32]`, `source_lineage_id: [u8;16]`, `source_history_epoch: u64`, `bookmark_floor: u64` (plus cause/reason/timestamps/storage_object_id/db/repo as above).

**Recovery-clear predicate (EXACT, normative):** the incident is resolved ONLY when a replacement incarnation has `storage_object_id ≠` the incident's, its installed receipt `artifact_digest ==` the recorded `receipt_artifact_digest`, its `lineage_id ==` the recorded `source_lineage_id`, its `history_epoch ==` the recorded `source_history_epoch`, and its new bookmark `== bookmark_floor + 1`. The clear rewrites EXACTLY that incident's copies (row + repo record) by `incident_id`. Deletion of a subscription NEVER resolves an incident. **Round 4:** the only other sanctioned clear path is DESTRUCTION COMPLETION — `Resolved(by destruction)`: `state = Cleared`, `cleared_by = ByDestruction { operation_id }`, written in the SAME durable write as the operation's `Destroyed` transition (131 §4a step 6); R1-stage-2 consequently records NO incident for an absent destination whose covering destruction operation has completed (131 §4c (a)).

**A.4 `MetaKey::ConsistencyIncidents` (repo info store, MetaEnvelope v1)** — **DECIDED round 3:** the repo-carried record holds the FULL incident copy (ALL A.3 fields, incl. the recovery baseline), not only id + digest, so missing catalogue rows are rebuilt from it at boot when the repo is openable:

```text
{ unresolved: Vec<IncidentCopy>,   // IncidentCopy = the full A.3 row (all fields)
  format_version: u16 = 1 }
```

**Degraded-copy rule (round 5 — aligned with 131 §4c (a)):** repo unopenable ⇒ the catalogue copy alone is authoritative. The R1-stage-2 handoff completes when the catalogue copy is verified AND (the repo copy is verified OR the destination is NOT live (absent or destroyed) — no repo copy is possible for an absent destination, and the round-4 text requiring one was the S-vs-M contradiction). Retry is idempotent by `(cause, storage_object_id)` with incident-id reuse (131 §3d).

**A.5 Audit event envelope (extension of `AuditEntry`)** — `details_canonical_msgpack` payload:

```text
{ event_id: { operation_id: String(≤64), transition_ordinal: u32 },
  operation_kind: "drop_repo" | "drop_db",
  storage_object_ids: Vec<[u8;16]>,
  anchors: Vec<{ name: String(≤255), generation: u64 }>,
  actor: String(≤255), reason: String(1..=512), target_state: enum, outcome: enum }
```

**Round-3 id note:** `event_id = { operation_id, transition_ordinal }` — `record_version` and `target_state` are NOT part of the id (`target_state` remains a separate payload field). A retried `Blocked` emits a second `blocked` event for the same state, so distinct ordinals are required for idempotent replay/reconciliation (131 §3e reconciliation rule).

Enums (normative): `event_kind ∈ { prepared, rejected, quiesced, catalogued, destroyed, blocked }` — there is NO `destroying` event kind: the `Destroying` transition emits no dedicated audit event (131 §3e); **`destroyed` is THE terminal event kind for `Destroyed` (round 5, P2-9: unified with 131 §3e item 2 — the kind name is `destroyed` everywhere, payload and transition table alike)**; a retired `Blocked`'s final `blocked` event likewise gates retirement (131 §3e). The event kind names map to the operation state whose completion they record (`prepared` records `Prepared` reached, `catalogued` records `Cataloguing` finished, etc.). `target_state ∈` operation states (the 3b state enum); `outcome ∈ { emitted, replayed, acked }`.

**A.6 Wire DTO `destroy_quarantined_data`** — `{ reason: String(1..=512, printable), operation_id: String(≤64), nonce: String (32 hex chars), digest: String (64 hex chars — the `destruction_preview` digest; round 4), challenge: String (from `destruction_preview`; round 4) }`; echoed back as `{ operation_id, state, nonce }`. **Round 4 — the preview exchange (new admin wire op):** `destruction_preview(targets, cascade) -> { targets: Vec<{db, repo, storage_object_id}> (sorted by (db, repo)), digest: [u8;32], challenge: String (single-use), expires_at_ns }`; the challenge is consumed at Prepare and the executor re-resolves the live/catalogued set against the signed digest (fail-closed `destruction_target_changed`, §6c).

**A.7 `AuditAck`** — `{ seq: u64, checkpoint_position: [u8;32] }` (131 §3e).

**A.7a `checkpoint_durable` (DECIDED round 3; round 5 watermark rules):** `checkpoint_durable() -> Result<AckWatermark, AuditError>` — flushes + fsyncs the log FIRST and FAILS if the flush failed (unlike the OBSERVED void `appender.checkpoint()`, audit_appender.rs:706-713, which logs the flush failure and still writes the checkpoint), then writes the checkpoint keyspace durably (SyncAll) and returns `AckWatermark = next_seq − 1` — the highest assigned seq covered by the durable log + checkpoint, advanced ONLY by a successful write+sync of a contiguous prefix (131 §3e item 0b); the chain poisons on appender failure; a sync error is never proof. An outbox event is acknowledged only when `ack_watermark >= event.audit_seq`; the periodic scheduler (scheduler.rs:204-216) switches from void `appender.checkpoint()` to `checkpoint_durable` + an ack POST to each operation's executor (single-writer model, 131 §3b round 5). `checkpoint_now` (audit_chain.rs:436-441) is a manual force, NOT a durability barrier. **Round 4 (D2):** `checkpoint_durable`'s durable acknowledgement record is one of the two sanctioned sources for the §3e item 6 reconciliation read of an ambiguous append (the other: a forced-fsync read of the appender's own durable log tail, `read_durable_tail`); in-memory chain presence (`AuditChain::snapshot`) is NEVER evidence.

**A.7b `AuditSink` (NEW — round 5, P1-4/P2-1; deliverable of 8b-3b):** the `shamir-db`-side seam, no new crate dependency:

```rust
pub trait AuditSink: Send + Sync {
    fn append_durable(&self, entry: &AuditEntry) -> Result<DurableAck, AuditSinkError>;
    fn contains(&self, event_id: &EventId) -> Result<bool, AuditSinkError>;   // durable-tail lookup (skip torn FINAL line only)
    fn durable_watermark(&self) -> Result<u64, AuditSinkError>;               // durable-through seq (contiguous-prefix rule)
}
pub struct DurableAck { pub seq: u64, pub checkpoint_position: [u8;32] }      // = AuditAck
pub enum AuditSinkError { Unavailable, Poisoned, Io }
```

The server (shamir-server) implements it over the batched appender; the launcher injects `Arc<dyn AuditSink>` into `ShamirDb::init_with_env_policy(config, audit_sink: Option<Arc<dyn AuditSink>>)` BEFORE replay (server_launcher.rs:137/:349); outbox replay waits for the sink. Invariant protected: the barrier proves durable intent without in-memory chain state and without a server dependency in shamir-db.

## Appendix B — administrative inspect/retry API (F7-iv, DECIDED)

On the existing admin surface (wire DTOs alongside the other `destruction*` operations; the HTTP/wire handler returns immediately — status is a pure read, retry spawns a detached executor per §4f of 131):

- `destruction_status(operation_id) -> { state, failed_phase?, error_class?, safe_restart_phase?, targets: Vec<{db, repo, storage_object_id}>, progress: Vec<{component_id, kind, absent: bool | "unknown"}> — **DERIVED** from `verify_absent` probes at status time, not durably stored, audit_acked: bool — true when `audit_pending` is empty (all outbox entries drained), anchors_remaining }` — a **pure read of `destruction_operations`** (+ a `verify_absent` probe only when the caller asks for `?verify=true`); a lost response is answered by re-issuing the call (idempotent, no side effects).
- `destruction_retry(operation_id)` — resumes a `Blocked` operation from `safe_restart_phase`, **or claims a stranded `Prepared` operation (audit-barrier failure) by re-running the barrier (idempotent by event id) and spawning the executor (round 4, 131 §3e item 3)**; refuses on terminal states (`Destroyed` → status; `Rejected` → new operation required); authorization required. **Claim enumeration (round 5, P2-2/P1-6): every claim path — same-id resubmission, `destruction_retry`, periodic pass (`Prepared` older than 5 min), boot — performs ONE CAS `(record_version, executor_epoch): (v, e) → (v+1, e+1)`; the winner runs the operation for that epoch, losers get the current status immediately (never block for the run); every claim path re-runs FULL PREFLIGHT against the frozen `target_digest` before Quiesce.**
- **Destruction-authority surface (round 5, P1-3 — NEW PRODUCTION CODE, deliverable of 8b-3b):** `pub struct DestructionAuthority { operation_id: String(≤64), targets: Vec<(String, String)>, /* + opaque minted token */ }` minted by the executor per operation id + frozen target set; `remove_repo_locked_destructive(db, repo, &DestructionAuthority, &ReplicationControlGuard)` (db_management.rs, beside the merged `remove_repo_locked` :561) skips `ensure_destinations_unbound` (validate.rs:143) EXACTLY for the token's targets — the destruction path is the only caller that may remove a repo referenced by a durable binding; every other caller stays refused. The executor takes control for the detach section (131 §4a step 3).
- `destruction_preview(targets, cascade)` — **round 4 (D4, §6a):** returns the frozen target list (sorted by (db, repo)), its SHA-256 digest, a single-use challenge (≤ 60 s) and `expires_at_ns`; the client signs the digest + challenge on the subsequent `DESTROY QUARANTINED DATA` statement. A pure read + challenge mint; no durable lifecycle state.
- `destruction_list(limit: u32, pagination_token: Option<String>) -> { items: Vec<{ operation_id, state, targets }>, next_token: Option<String> }` — bounded (≤ `limit` items), ordered by `created_at`, paginated via the opaque `pagination_token`/`next_token` pair.
- Optional (8b-3d+): `destruction_archive(operation_id)` — owner-approved archival removing a terminal, fully-acked row (the only sanctioned path to shrink the journal; 131 §3e retention rule).

**Authorization:** same **Delete** authority as DROP **plus** replication-management authority. **Idempotence/safety:** status/list are pure reads; retry is idempotent for an already-running operation (returns current state); no admin call can bypass the audit barrier or delete a journal row with an unacked terminal event.
