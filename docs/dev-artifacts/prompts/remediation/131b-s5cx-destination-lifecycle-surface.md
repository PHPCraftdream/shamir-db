# 131b — destination lifecycle: DDL/wire surface, creation exclusion, seed install, schemas, admin API

**Status:** normative (§§6–8 + Appendices A/B), amended by [DEC-3 Q3](../../design/dec3-owner-rulings-2026-10-21.md#q3---pr8b-3-design-and-draft-196) per amendments A1 (authorization atomicity), A2 (archival no-release), A7/A9 (durable-mutation, log framing), A8 (migration interlock), A14 (staging gate), A15 (round-tag consolidation). Citation discipline: **OBSERVED** = verified against this checkout (inventory in 131a Appendix 13); **DECIDED** = binding design choice; **INFERRED** = derived consequence. PR8b-2 (#188) and #189 are merged; no unmet dependency markers remain.

## 6. DDL / wire / builder / client surface

### 6a. Grammar and DTOs

**Grammar sketch (DECIDED):**

```text
DROP REPOSITORY <name> [CASCADE] [IF EXISTS] [DESTROY QUARANTINED DATA '<reason>' OPERATION ID '<op-id>' NONCE '<32 hex chars>' DIGEST '<64 hex chars>' CHALLENGE '<preview challenge>'];
DROP DATABASE <name> [CASCADE] [IF EXISTS] [DESTROY QUARANTINED DATA '<reason>' OPERATION ID '<op-id>' NONCE '<32 hex chars>' DIGEST '<64 hex chars>' CHALLENGE '<preview challenge>'];
```

**DECIDED:** the `DESTROY QUARANTINED DATA` modifier additionally carries `digest` + `challenge` from the mandatory preceding `destruction_preview` call (below); the HMAC covers the digest (§6b tag 0x0B).

- `CASCADE` controls child scope only; it never authorizes crossing replication bindings or physical destruction.
- **DTO (wire):** optional field `destroy_quarantined_data: Option<{ reason: String (nonempty, 1..=512, printable UTF-8), operation_id: String (≤64, [0-9A-Za-z._-]+), nonce: String (exactly 32 lowercase hex chars — the hex form of 16 bytes), digest: String (exactly 64 lowercase hex chars), challenge: String (from destruction_preview) }>` on `DropRepoOp` (**OBSERVED** crates/shamir-query-types/src/admin/types/repo_ops.rs:43-55: no `deny_unknown_fields` on the Drop ops) and `DropDbOp` (**OBSERVED** crates/shamir-query-types/src/admin/types/db/db_ops.rs:27-40). Unknown-field tolerance on Drop ops is intentional for forward-compatible builders; the new field is `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- **Builder (Rust):** `.destroy_quarantined_data(reason, operation_id, nonce, digest, challenge)` on the drop builders — **OBSERVED** current locations `crates/shamir-query-builder/src/ddl/repo/drop_repo.rs` and `crates/shamir-query-builder/src/ddl/db/drop_db.rs` (panel paths `ddl/drop_repo.rs` are stale). HMAC computation moves behind the modifier check.
- **TS client:** `.destroyQuarantinedData(reason, operationId, nonce, digest, challenge)` on the drop builders in `crates/shamir-client-ts/src/core/builders/ddl/` (**OBSERVED** `ddl-schema.ts` holds `dropDb` at :83-97 and `dropRepo` at :99-113, both computing the canonical bytes via `canonicalDropDb`/`canonicalDropRepo` imported from `core/protocol/hmac.ts`); request flows through `crates/shamir-client-ts/src/core/client/client.ts` unchanged.
- `IF EXISTS` semantics per 131 §4a (inspect incomplete operations first; **OBSERVED** early exits admin_db_repo.rs:106-112 and :415-423, `if op.if_exists` at :416).
- `nonce`: supplied by the caller; the wire DTO carries `nonce: String` (32 hex chars) inside `destroy_quarantined_data`; **echoed verbatim in the response** (`{ operation_id, state, nonce }`), and bound into the v2 canonical encoding (the canonicalizer encodes the 16 decoded bytes) — a replayed confirmation over different nonces fails, and a lost response cannot be confused with a different request. The nonce is still consumed ONCE per terminal outcome (existing rule, unchanged in round 4); the CHALLENGE (below) is a separate, server-issued single-use credential.
- **Destruction preview — DECIDED (round 4, the two-step confirmation exchange):** new admin wire op `destruction_preview(targets, cascade)` → response `{ targets: Vec<{db, repo, storage_object_id}> (canonical order: sorted by (db, repo)), digest: [u8;32] — SHA-256 over the canonical length-prefixed encoding of the target list (the encoding described in §6b for tag `0x0B`; the same encoder, used standalone), challenge: String — short-lived, server-issued, SINGLE-USE, expires_at_ns (≤ 60 s ahead) }`. The client MUST call `destruction_preview` first and sign the returned digest + challenge.
- **TS client:** new method `.destructionPreview(targets, cascade)` (returns the preview response); `.destroyQuarantinedData(reason, operationId, nonce, digest, challenge)` — the builder carries the preview's digest + challenge into the DTO and the canonical encoding (§6b fields 0x0B/0x0C).
- **Builder (Rust):** `.destroy_quarantined_data(reason, operation_id, nonce, digest, challenge)` on the drop builders, plus a Rust admin-call analogue of `destruction_preview(targets, cascade)`; `digest` is the 32-byte value (hex on the wire).
- **Challenge rules:** expiry ≤ 60 s, single use, consumed at Prepare. **DECIDED: the challenge store/format** — a new system-store table `destruction_challenges` (v1): `{ challenge_id: [u8;32] /* random */, digest: [u8;32], session_binding: [u8;32] /* HMAC-key fingerprint of the issuing session */, created_at_ns: u64, expires_at_ns: u64 }`. SINGLE USE is enforced by a durable DELETE-IF-MATCHES (challenge_id + digest) at the Prepare write — not by a per-operation `challenge_consumed` boolean (a boolean cannot stop two concurrent prepares; a durable delete is atomic). **A1 — authorization is atomic:** the challenge DELETE-IF-MATCHES and the `Prepared` PUT-IF-ABSENT occur in ONE system-store transaction under the held admissions, followed by one flush. A failed challenge writes NO row and authorizes no execution; the remedy is re-run `destruction_preview` and retry with a FRESH operation id (or `destruction_retry` only if a prior authorized row genuinely exists). Expired rows are swept opportunistically; deletion is the single-use mechanism.
- **Preview authorization — DECIDED:** `destruction_preview` requires the SAME authority as the destroy it precedes (Delete + replication-management), and the challenge it mints is SESSION-KEYED (`session_binding` above) — a challenge minted in one session is unusable in another; the digest is otherwise a pure read.
- **Actor — DECIDED:** the actor is ALREADY in the canonical bytes (tag `0x08`); the SERVER recomputes the canonical encoding from the authenticated session's canonical actor string and REJECTS a client-supplied actor that differs (the client value is informational); the TS client obtains the actor from the session/hello response (the exact server actor string is exposed there — if the hello does not currently carry it, 8b-3c adds it to the hello payload; the shared test vectors pin the string).
- **Verification seam:** the digest is what `check_destructive_hmacs` (**OBSERVED** `crates/shamir-server/src/engine/db_handler/handlers/admin.rs:656-658`) can verify WITHOUT a Prepared operation — the HMAC is over the digest (not only the logical names), so the seam verifies it session-keyed exactly as today; the executor's re-resolution of the live/catalogued set against the signed digest at Prepare (131 §4a preflight) is the second half.

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
- **Test vectors:** add shared JSON test vectors (Rust + TS) covering: plain drop, destruction drop, each omission, overlong/boundary lengths, control-char rejection, nonce mismatch, digest mismatch, stale/consumed challenge (round 4). The old pinned NUL expectations (hmac_tests.rs:29-32; `crates/shamir-client-ts/src/core/protocol/__tests__/hmac.test.ts:42-46`) are replaced.

### 6c. Error codes — the eleven typed wire codes (DECIDED, normative)

Survive dispatch incl. rename (**OBSERVED** `db_error_to_batch` at execute/helpers.rs:85). Each `DbError` variant maps 1:1 to its wire code; the remedy contract is what the client/operator does on receipt:

| DbError variant | Wire code | Remedy contract |
|---|---|---|
| `ReplicationDestinationBound` | `replication_destination_bound` | The name is bound by live replication bindings; release via R2'/R1 first (rename: format limitation per 131 §1). |
| `QuarantinedSubscriptionDestinationsRemain` | `quarantined_subscription_destinations_remain` | Destroy each remaining bound destination, then retry R1 (stage-1 semantics; I9); a catalogued destination with ZERO referencing rows is destroyed via the explicit unbound-repo destruction path (131 §4a step 0). |
| `QuarantinePersistencePending` | `quarantine_persistence_pending` | A pending terminal cause for a referencing generation is unresolved (§3f of 131); retry after the flush is confirmed. |
| `DestinationNameReserved` | `destination_name_reserved` | The name is reserved (anchor or non-`Destroyed`/non-`Rejected` operation of `kind: Repo | Database`; a `Completed` `SubscriptionRemoval` never reserves); or a `Destroyed` operation with non-empty `audit_pending` (released when the pending set drains); only the §7c recovery exception admits publication. |
| `DestructionInProgress` | `destruction_in_progress` | An incomplete operation covers the target; poll `destruction_status`, then `destruction_retry`/wait. |
| `DestructionBlocked` | `destruction_blocked` | External block; clear the cause, then `destruction_retry(operation_id)`. |
| `StorageDestructionUnsupported` | `storage_destruction_unsupported` | The backend cannot destroy this component; no retry path until the backend gains support. |
| `UnsafeStorageOwnership` | `unsafe_storage_ownership` | A live replacement/foreign object occupies the recorded identity; never deleted (I6); operator investigation required. |
| `DestructionOperationConflict` | `destruction_operation_conflict` | Same operation id with different frozen parameters; issue a new operation id. |
| `DestructionAuditUnavailable` | `destruction_audit_unavailable` | The audit chain could not durably record the intent; retry the same operation id — the resubmission RE-RUNS the audit barrier (idempotent by event id) and spawns the executor — or call `destruction_retry` (accepts `Prepared`). (The operation STAYS `Prepared`, reservations PUBLISHED, nothing deleted — 131 §3e.) |
| `DestructionTargetChanged` | `destruction_target_changed` | The resolved live/catalogued target set no longer matches the signed preview digest, OR the challenge is stale/expired/consumed. **Because challenge consumption is atomic with the Prepared PUT-IF-ABSENT (131 §3b), a failed challenge means NO row was written**; the remedy is re-run `destruction_preview` and retry with a FRESH operation id. `destruction_retry` applies only when a prior authorized row genuinely exists (from an earlier successful Prepare). |

`ErrorClass` mapping (journal `error_class` field → wire code, 131 §3b): `OwnershipMismatch → unsafe_storage_ownership`; `ExternalLock` / `PermissionDenied` / `UnknownIo → destruction_blocked`. Unsupported backend is refused at preflight and has no row variant.

## 7. Creation exclusion matrix — complete (F3)

Reservation check = `(db, repo)` covered by anchors, non-`Destroyed`/non-`Rejected` operations **of `kind: Repo | Database`** (a `Completed` `SubscriptionRemoval` record never reserves; 131 §3c), or a `Destroyed` operation with non-empty `audit_pending` (released when the pending set drains) (131 §3c; `repo = "*"` = database-scope reservation covering every `(db, *)`).

### 7a. Proposed NamespaceAdmission guard and lifecycle exclusion

- **Proposed TWO-level hierarchy:** a per-database RW-style gate plus per-(db, repo) admission mutexes beneath it, held in an in-process registry on `ShamirDb`: `admissions: DashMap<(String, String), Arc<tokio::sync::Mutex<()>>>`. A publication acquires db gate then sorted target mutexes before validation; cancellation-safe RAII holds through publication/abort. Scope modes and complete path compositions appear below.
- **DECIDED:** Reclaim an entry only while holding the same map-entry/shard lock used by all get-or-create and Arc-clone operations, and only if `Arc::strong_count(&entry) == 1`; do not create a temporary clone for counting. No Weak, external-clone, or bypass path may retain/manufacture handles; the registry is the sole synchronized source. Count 1 then proves no user retains the old mutex when a new identity is installed. At capacity, backpressure/refuse; never evict a referenced entry.
- **Stage boundary:** 8b-3b delivers and tests only the admission-registry/control primitive and destruction-side reservation publication; 8b-3c wires every real publisher and tests publisher-versus-reservation races end to end. No composed-path guarantee applies before 8b-3c. **8b-3b executor and authority are `pub(crate)` and not wire/admin dispatchable (A14); destructive dispatch becomes reachable only in 8b-3c together with all publisher wiring.**
- **Proposed scope modes:** the database gate is acquired once at final mode and held through the operation; no upgrade. Repository-scope work (including CREATE REPO) is SHARED; database-scope work (including CREATE DATABASE) is EXCLUSIVE.
- **Proposed order and R1 exception:** proposed admission gates/mutexes precede control, then lease write gate, then write admission. Reservation mutation and authoritative runtime check/use paths use a short control section; the installer precheck is read-only and uses maintenance-lock exclusion instead, without control.
- **Lifetime:** the proposed guard lives only within one operation; cancellation drops it (RAII); it is not persisted or needed at boot. On Blocked/exit the set releases; retry reacquires after revalidation.
- **Boot/executor guard:** boot publishes/skips reservations before request acceptance; runtime executor uses the same proposed admission guard.

Remove/rename/destroy/R1 take scope-appropriate gates and sorted repo mutexes before control. **Admission-registry reclamation:** remove an entry only while holding the same registry map-entry/shard lock used by every Arc clone/get-or-create path and iff `Arc::strong_count(&entry) == 1`, without a temporary clone. The registry is sole source; no Weak/external cloning/bypass may retain old mutex. Capacity uses backpressure, never eviction of a referenced entry.


**Proposed path order:** CREATE DATABASE exclusive `(db, *)` → control through insert. CREATE REPO shared db → repo mutex → `repo_create_locks` → control through publication. Generic add-repo shared db → target mutex → control through publication. CREATE SUBSCRIPTION/RESUME shared db + sorted bound mutexes → control. Remove/rename/destroy/R1 scope gate(s) + sorted target mutexes → control. Seed full wrapper is 8b-3e; 8b-3c early check is maintenance-lock based; full wrapper remains 8b-3e.

Each row below separates OBSERVED merged API location from proposed reservation/gate insertion. File:line facts are merged inventory; placement language marked proposed is not current behavior.

### 7b. Publication-API inventory (every managed/engine path — F3-i)

| # | Publication API | file:line | Check placement | On violation |
|---|---|---|---|---|
| 1 | `ShamirDb::create_db` / `create_db_as` | db_management.rs:43-80 | Proposed EXCLUSIVE db gate → control; reservation check + mirror write + insert in same section through publication. | `destination_name_reserved`; no instance inserted |
| 2 | `handle_create_db` wire path | admin_db_repo.rs:18+ | Proposed EXCLUSIVE db gate → control; route to create_db_as; no second insertion. | As (1) |
| 3 | `DbInstance::add_repo` / `add_repo_with_durability` | db_instance.rs:52-75 (recovers/certifies in the constructor, then `repos.insert` :73) | **DECIDED:** direct call on a managed home fails with `destination_name_reserved`; the only allowed engine seam is (4). Config-driven/boot callers go through the boot gate instead | typed refusal before `repos.insert` |
| 4 | `publish_repo_admitted` | db_instance.rs:79-96 (vacant-only insert, debug-checked `RepoWritePermit`) | reservation + admission check inside the seam before the `entry()` call | `destination_name_reserved`; nothing published |
| 5 | `DbInstance::rename_repo` registry re-key | db_instance.rs:176-184 (`remove(from)` + `insert(to)`) | admission on `to` held across both operations (atomicity of the guard makes the re-key check-then-act safe) | `destination_name_reserved`; no re-key |

| 6 | Wire CREATE REPO handler | admin_db_repo.rs:201+ (`handle_create_repo`; `repo_create_locks` :267, `create_dir_all` :314/:339) (**Citation re-verified (round 5)**) | create-path order (131 §4e, CREATE REPO path paragraph): admission guard (sorted) → per-db create mutex (`repo_create_locks`) → control lock, all before `create_dir_all`, with the reservation check under the guard | `destination_name_reserved`; no directory created |
| 7 | Generic managed path `add_repo_as[_with_durability]` | db_management.rs:371-465 | Proposed SHARED db gate → target mutex → control held through reservation check, mirror write, registry/catalogue publication; mirror write propagates errors. | `destination_name_reserved` before open |
| 8 | Seed install: `check_seed_target` + installer | installer.rs:115-122; staging.rs:60, 74-80; catalogue.rs:155-162 | **8b-3c early authority check (OPEN1):** after exclusive maintenance lock :115 and before `find_complete` :117, call the new `SeedAuthorityReader::open_existing(system_store_path: &Path, maintenance: &InstanceMaintenanceGuard) -> Result<SeedAuthoritySnapshot, SeedAuthorityError>` (async if I/O), then `snapshot.check_install(db, repo, receipt, subscription)` using the verified artifact receipt/metadata only. It is a logical authority-only reader, not `ShamirDb::init`; its required read-only existing-system-store adapter must never create directories/keyspaces/tables or perform writes, flushes, migrations, checkpoints, or write-producing WAL recovery. If a consistent read-only snapshot cannot be established (dirty journal/recovery required/unsupported backend), fail closed; do not claim Fjall factory currently supports read-only access. Open/read ONLY `<data_dir>/shamir_db_meta.redb`; never instantiate a factory from user repo records, glob/open candidates, or inspect repository-carried evidence before check. Reconstruct exactly the §3c/§4d stage-1 reservation union from validated destruction rows plus #188 anchor-subscription coverage, including SubscriptionRemoval Pending/RowDeleted boot reservations and Destroyed audit_pending; exclude Rejected. Publish readiness only after full validated load; incomplete/unavailable/corrupt/unreadable state => `AuthorityUnavailable`, dedicated installer/admin `destruction_authority_unavailable`, never empty/bootstrap fallback. Explicit fresh bootstrap provisions authority schema in a separately authorized initialization. Apply existing authorized quarantined rebind predicate from row/scope metadata only; no candidate reads. Nonterminal destruction => `destruction_in_progress`; other reservation without authorized recovery => `destination_name_reserved`. Keep the eleven typed lifecycle DDL codes unchanged; this is a distinct authority-read diagnostic, not an unreviewed twelfth DDL code. Drop reader handles before later SystemStore init to avoid exclusive-open conflicts; retain maintenance guard through install. Decision invariant: no candidate/user-repo open or write, catalogue logical write, import, publish, or registration before check (maintenance-lock diagnostic PID writes are outside this invariant). The catalogue.rs:161 control-locked recheck uses same predicates and confirms current snapshot predicate/row generation/scope before optional subscription publish; 8b-3e wrapper adds online gate serialization, identity/remnant preflight, and is not required for this early gate; no-subscription install remains covered by early reader + maintenance exclusion. E case 14 must fault-inject unavailable/corrupt/unpublished/dirty/unsupported authority and instrument zero candidate open/import/publish/register and zero logical writes before refusal; authorized retry remains possible. | Typed refusal before candidate access/mutation; covers rebind and no-optional-subscription case. |
| 9 | Boot/config publication | core.rs:329-397 (loop, `seed_boot_gate` :381, `add_repo_with_durability` :394) (**Citation re-verified (round 5)**) + `publish_repo_admitted` | 131 §4d step 4: reserved targets skipped fail-closed, never opened | skip + log; never publish |
| 10 | Migration publication (experimental API) | core.rs field :142-143, `enable_experimental_migration_api` :705 (**OBSERVED**), MigrationCoordinator import core.rs:20, field :99 | **DECIDED (A8):** migration admits two repos per instance; it acquires BOTH namespace admissions in global `(db, repo)` order before control and write locks, re-validates identity after acquisition, and refuses reserved targets; each admitted repo passes the admission+reservation check via seam (4); migration refuses when either target namespace is reserved. The CI case "migration during reserved name" is MANDATORY in 8b-3c. | migration refused with `destination_name_reserved` |
| 11 | `ShamirDb::remove_db` / `remove_repo` (merged guarded variants exist — `remove_db_locked` db_management.rs:95, `remove_repo_locked` :561, both taking `&ReplicationControlGuard`) | db_management.rs:82-100, :549-561 | not a creation path; 8b-3b adds `remove_repo_locked_destructive` (the `DestructionAuthority` variant, Appendix B) beside the merged guarded variants | n/a (I2 prevents their use for destruction) |
| 12 | CREATE SUBSCRIPTION / RESUME binding | admin_replication.rs:295 | Proposed SHARED db gate + sorted bound-target mutexes before control; check held through publication. | `destination_name_reserved` |
| 13 | RENAME into a reserved target | `rename_db_as` db_management.rs:165, `rename_repo_as` :622, `DbInstance::rename_repo` (row 5), R3 guard | reservation check on target name under the rename's control section (merged: control first), admission gates already held | `destination_name_reserved` (R3 wording per 131 §1) |
| 14 | Destruction executor re-publication avoidance | boot loop + executor | covered by 131 §4d (skip + never publish) | n/a |
| 15 | `DbInstance::with_repos` | db_instance.rs:34-48 (**OBSERVED** construct-time insertion) | **DECIDED:** construct-time insertion bypasses the runtime seam; allowed ONLY for boot-time construction before the server accepts requests (the admission registry is empty, reservations already published by 131 §4d step 2); any post-boot caller must go through seam (4) | boot-only constraint; post-boot use rejected in review/CI |
| 16 | `register_seed_image` | seed/catalogue.rs:113-198 (OBSERVED catalogue writes) | Proposed 8b-3e wrapper provides shared db gate → repo mutex → control and reservation check before catalogue writes. The 8b-3c early precheck is separate, before find_complete under maintenance lock; it does not assert register_seed_image gates. | `destination_name_reserved`; no catalogue rows written |

**Reads/writes against reserved names:** the quarantine cache gates READS only (**OBSERVED** db_execute.rs:76-90 reads-only gate; :131-150 walks `BatchOp::Read`) — reserved/quarantined targets get WRITE fencing via the engine write-admission seam (`RepoWriteDomain`/`RepoWritePermit`, **OBSERVED** db_instance.rs:79-96), not via the cache; the cache remains the read gate (reserved targets are never in the live map by I3/I4 of 131).

**Merged inventory versus proposed mapping:** all API names/line locations above are observed inventory. Gate/check placement statements identify design insertions. The composed paths (CREATE DATABASE/REPO, add-repo, subscription/RESUME, remove/rename/destroy/R1, seed wrapper) are proposed paths, not current merged leaves.

### 7c. Recovery exception — RESUME / replacement-rebind into a reserved destination (F3-iii)

**Recovery path boundary:** R2' (destroy) → R1 (release the name) → fresh install into ABSENT target as NEW incarnation. In-place replacement into CURRENT incarnation is out of scope.

1. The subscription row already binds THIS destination (name match at bind time).
2. **(round 5, P2-8 — applies ONLY when the destination is LIVE or CATALOGUED):** the live-or-catalogued `storage_object_id` **equals** the id recorded in the binding/incident (no replacement incarnation). For an ABSENT destination this item is vacuous and the exception reduces to items 1, 3 and 4 (there is no live object to compare).
3. The installed receipt/incident evidence matches (same receipt digest / incident evidence; the exact retry-identity tuple is (db, repo, receipt `artifact_digest`, lineage_id) per §8c).
4. No non-terminal destruction operation covers the name; a `Destroyed` operation covering it is permitted exactly when its recorded target (db, repo) equals the install target (the destroyed-then-reinstall path of §8). **(round 5, P2-8: this item is LIVE text because item 2 no longer requires a live/catatogued object — an absent target whose covering operation completed passes item 4, which is precisely the fresh-incarnation reinstall.)**

Additionally, an ABSENT destination with NO covering destruction operation still reserves (131 §4c (a) round 5) and is NOT admitted by this exception — it needs a validated recovery or an explicit destruction first.

Anything else → `destination_name_reserved`. **Forbidden replacement publication (a NEW object under a reserved name) is never exempt.** A completed destruction resolves the destination's incidents (`Resolved(by destruction)`, 131 §4a step 6) and R1 records NOTHING for an absent destination with a completed destruction operation — the name is genuinely released after R1, and item 4 is what admits the fresh-incarnation reinstall.

**Stage boundary:** 8b-3b delivers and tests only the admission-registry/control primitive and destruction-side reservation publication; 8b-3c wires every real publisher and tests publisher-versus-reservation races end to end. 8b-3b primitive coverage makes no composed-path guarantee before 8b-3c.



## 8. Seed install changes (8b-3c early check; 8b-3e full extension)

**OBSERVED** (installer.rs, all re-verified): Unix gate :92-95; source/destination name-equality check :95-96; `create_dir_all` :113; `canonicalize` :114; `maintenance_lock::acquire` :115; `staging::destination` :116; `find_complete` :117 — which **opens candidate storage** via `staging.rs` `completed/find_complete` → `BoxRepoFactory::fjall_raw(path).create()` (**OBSERVED** staging.rs:60, :78-80, and `open_image` :116-119); `init_seed` :118-121; `check_seed_target` :122; import :142; `staging::publish` :170; `register_seed_image` :176; `subscription.persist` :179. Staging layout (**OBSERVED** staging.rs): `seed-<key>` target dir :16; `<target>-attempt-N` dirs :27-32; `.seed-install` marker :32; completed `SeedInstallState`/receipt inside backing store `__tx__` :60-71; published rename to final target :121. Installer path: `crates/shamir-server/src/sync/replication_seed/install/installer.rs`.




### 8a. Verified current installer ordering (F6 baseline)

**8b-3c early authority check (DECIDED):** after exclusive maintenance lock :115 and before `find_complete` :117, use the new authority-only API:

```rust
pub async fn open_existing(
    system_store_path: &Path,
    maintenance: &InstanceMaintenanceGuard,
) -> Result<SeedAuthoritySnapshot, SeedAuthorityError>

fn check_install(
    &self,
    db: &str,
    repo: &str,
    receipt: &SeedReceipt,
    subscription: Option<&str>,
) -> Result<(), SeedAuthorityError>
```



The system-store read-only adapter is a new 8b-3c deliverable, not an observed Fjall factory capability. It reads only the existing `<data_dir>/shamir_db_meta.redb` path; it may not create directories/keyspaces/tables, write, flush, migrate, checkpoint, or perform write-producing WAL replay/recovery. It never builds factories from user repo records, globs/opens candidates, or reads repo-carried evidence before decision. Reconstruct exactly §3c/§4d validated stage-1 membership from destruction rows and #188 anchor bindings, including SubscriptionRemoval `Pending|RowDeleted` and Destroyed rows with nonempty `audit_pending`, excluding Rejected; reuse the same pure membership decoder as boot publisher. Snapshot readiness only after full validation. Missing/corrupt/unreadable/unpublished/incomplete/unsupported authority or dirty journal needing recovery => `AuthorityUnavailable` → installer/admin diagnostic `destruction_authority_unavailable`, fail closed, no empty/bootstrap fallback. Separately authorized fresh initialization must provision the authority schema. Preserve authorized quarantined rebind predicate (quarantined row + same bound destination, no nonterminal destruction, expected receipt/scope as applicable) using metadata only; no exception without matching subscription. Nonterminal destruction maps `destruction_in_progress`; other reserved state without recovery maps `destination_name_reserved`. Existing eleven lifecycle DDL codes stay unchanged; this authority diagnostic is separate unless a future explicit protocol change is reviewed.



Any missing/corrupt/unreadable/unpublished/incomplete/unsupported authority view returns `SeedAuthorityError::AuthorityUnavailable`, mapped to dedicated installer/admin diagnostic `destruction_authority_unavailable`; never fallback to empty or bootstrap. Fresh bootstrap must provision authority schema through separately authorized initialization. Check uses supplied verified artifact receipt and metadata only; no candidate evidence. Preserve authorized quarantined rebind predicate (quarantined row + same bound destination, no nonterminal destruction, expected receipt/scope as applicable); no subscription exception absent matching stored row. Nonterminal destruction maps `destruction_in_progress`; other reservation without authorized recovery maps `destination_name_reserved`. The existing eleven lifecycle DDL wire codes remain unchanged; authority unavailable is a distinct installer/admin diagnostic unless a future explicit protocol change is reviewed.

Order: acquire maintenance guard → open/read/check snapshot → drop reader handles before later SystemStore init (avoid exclusive-open conflicts) → `find_complete` → `init_seed` in the original order → remaining install. Retain the same exclusive maintenance guard through the entire install. Invariant is no candidate or user-repo open/write, no catalogue logical write, import, publish, or registration before decision; maintenance-lock diagnostic PID writes are pre-existing and are not covered by a false promise of zero filesystem writes. Catalogue control recheck at catalogue.rs:161 uses the same predicates and verifies current snapshot predicate/row generation/scope before optional subscription publication. The 8b-3e wrapper adds online gate serialization, identity/remnant preflight, and is not required for this early check; no-subscription install is still protected by reader + maintenance exclusion. Acceptance (E case 14): fault-inject missing/corrupt/unpublished/dirty/unsupported authority and assert typed error pre-candidate, instrument zero candidate open/import/publish/register and zero logical writes before refusal; authorized retry remains possible.

**8b-3e full lifecycle preflight:**
```text
lifecycle_preflight(db: &str, repo: &str, receipt: Option<&SeedReceipt>) -> Result<(), LifecycleRefusal>
```



**8b-3e full-preflight contract:** `storage_object_id` is resolved internally from journal/ownership records, never caller-supplied. Refuse an anchor/nonterminal reservation unless §7c authorized recovery predicates hold; verify no owned component of a completed Destroyed target remains (`verify_absent` over recorded descriptor; any remnant refuses, never silent reuse). `LifecycleRefusal { NameReserved, OwnershipMismatch }` maps to `destination_name_reserved` / `unsafe_storage_ownership`. Only after this full preflight may find_complete/check_seed_target/import/publish/register/persist run. The outer gate wrapper is also 8b-3e; the 8b-3c precheck above remains distinct.




### 8c. Authorized install retries — DECIDED

Same operation semantics: same `(db, repo, receipt artifact_digest, lineage_id)` → may **resume a completed stage** (the existing `find_complete` path). A DIFFERENT receipt/incident over a name covered by any anchor or non-`Destroyed` operation → `destination_name_reserved`. Retry remnants may be reused only when the receipt matches exactly; **else they are deleted first, within the same admission guard** (§7a).

**DECIDED — the three install flows this series supports (the ONE recovery path of §7c):**

1. **Absent-target install:** preflight requires NO component of any PRIOR incarnation (by storage-object id from the destruction journal) and no reservation; the install creates a NEW incarnation (fresh storage_object_id). The name is free here because destruction completion resolved the incidents (`Resolved(by destruction)`) and R1 released the reservation without recording an incident for the absent destination (131 §4c (a), §4a step 6).
2. **Interrupted-new-install retry:** the install journal records the NEW attempt's storage-object id (the new Appendix-A install-journal record); preflight accepts components owned by THAT attempt and rejects only foreign / old-incarnation ones.
3. **Remnant classification:** owned by this attempt → resume or delete; owned by a destroyed prior incarnation → delete as destruction completion; FOREIGN → REFUSE, never delete (this resolves the §8d table contradiction).

### 8d. Seed staging/publication remnant inventory (ownership + deletion rules)

**DECIDED:** all seed remnants are **OWNED COMPONENTS** of the target's ownership descriptor when the target is seeded (component kinds SeedStagingDir, SeedInstallMarker, SeedReceiptStore; 131 §3a). Destruction destroys them (I5); install retries may only reuse remnants whose receipt matches exactly, else delete them first (within the same admission).

| Remnant | Where (OBSERVED) | Ownership rule | Deletion rule |
|---|---|---|---|
| `seed-<key>` target dir | staging.rs:14-17 | SeedStagingDir component of the target | destroyed by R2' step 4; retried install deletes stale dirs with mismatched receipt |
| `<target>-attempt-N` staging dirs | staging.rs:19-42 | SeedStagingDir (one descriptor entry per attempt dir recorded at preflight) | destroyed with the target; incomplete attempts deleted by a matching-receipt retry |
| `.seed-install` marker | staging.rs:32 | SeedInstallMarker | as above |
| completed `SeedInstallState`/receipt in backing store `__tx__` | staging.rs:60-71 | SeedReceiptStore (lives inside the opened candidate store — reachable only because `find_complete` opens storage; hence preflight MUST run before :117) | classified per the §8 remnant-classification rule: owned by THIS attempt → resume or delete; owned by a DESTROYED prior incarnation → delete as destruction completion; FOREIGN → REFUSE, never delete |
| published rename (attempt dir → final target) | staging.rs publish step (:121 region) | the final path IS the PrimaryStore + sibling WAL descriptor | standard R2' component destruction |

**§8d contradiction resolution:** the earlier "retry deletes stale/mismatched dirs" rows and the "non-matching receipts make the store a foreign object → refusal" row are resolved by the three-way remnant classification above — deletion happens only for remnants owned by this attempt or by a destroyed prior incarnation; anything FOREIGN (a live different incarnation, an unowned object) is REFUSED, never deleted (I6 of 131).

### 8e. Other seed-install items (re-affirmed)

- **Repo-scoped installation into an existing database:** new `ShamirDb` API + installer mode touching only the target repository; preflight requires the target ABSENT or `Destroyed`-with-journal-evidence; honours reservations (I4); reuses the maintenance lock (installer.rs:115) for the instance-exclusive window.
- **Database-wide rule replacement at seed/catalogue.rs:54-105 — DECIDED (**OBSERVED**):** the current code validates database-wide (every repository row/path/marker). Replacement: repo-scoped install validates ONLY the target repository — its catalogue row, its path, its `.seed-install` marker, and its completed state; unrelated repositories and other tables are untouched and unvalidated. STAYS enforced: the per-repo ordinary-repository refusal for a live non-seed target, the reservation/lifecycle preflight (§8b), receipt equality for the target, and the unix gate (installer.rs:92-95).
- Source==destination name equality kept and documented as a limitation until PR8b-4 (**OBSERVED** installer.rs:95-96; seed/catalogue.rs:38-41 in `check_seed_target_inner`).
- Single-stream seed scope stays: `matches_scope` accepts exactly one Pull, ReadOnly, single-target stream (**OBSERVED** `crates/shamir-db/src/shamir_db/execute/replication_seed/catalogue.rs:52-58`). Documented; multi-target supported path is "destroy and recreate as single-target subscriptions".
- Runbook rewrite plan for `docs/guide-docs/guide/09-seed-recovery.md` (**OBSERVED** :28-42 currently instructs DROP of the quarantined subscription, which #188's R1 refuses): (s0) truthful statement of what this version supports; then R2' per destination → R1 (after stage-appropriate preconditions) → repo-scoped install → Resume; abandonment stop-point is "stop after R1" — names are released, no reseed needed; Windows: destruction + name release supported, reseed NOT (installer Unix-only, **OBSERVED** installer.rs:92-95). The new-data-directory bootstrap procedure (:103-145) stays as an alternative.

## Contract recap

OPEN1: the early installer authority reader is new, read-only, and fail-closed before candidate access; see §8a. OPEN2: 8b-3b primitive-only (executor/authority `pub(crate)`, not wire/admin dispatchable per A14); 8b-3c wires/tests every real publisher race and makes destructive dispatch reachable; see §7a/§7b.

## Appendix A — durable-schema appendix: all records, types, bounds (F7-ii, normative)

Common conventions: names (db/repo/subscription/profile) ≤ 255 bytes UTF-8, no NUL/C0; paths ≤ 4096 bytes; reason 1..=512 chars printable UTF-8; operation_id ≤ 64 chars `[0-9A-Za-z._-]+`; actor ≤ 255 bytes printable UTF-8; ids/digests fixed-size byte arrays; generations/timestamps u64 (ns since UNIX_EPOCH for timestamps); every record carries `format_version: u16`.

**A.1 `MetaKey::StorageOwnership` (repo info store, MetaEnvelope v1)**

```text
{ storage_object_id: [u8;16], durable: bool,
  components: Vec<{ component_id: u32, kind: ComponentKind,
                    path: Option<PathBuf(≤4096)>, bytes_repr: Option<String> }>,
  created_at: u64 (ns), format_version: u16 = 1 }
```

**A.2 `destruction_operations` row (system store table, v1)** — the full row is specified in 131 §3b; bounds per the common conventions; `record_version` starts at 1 and increments on every CAS write; `executor_epoch` starts at 0 and increments on every claim. The operation row carries `nonce: [u8;16]`, `incident_refs: Vec<IncidentRef>` (empty in stage 1) and `audit_pending: Vec<PendingAuditEvent>` (the in-record outbox; 131 §3b/§3e). It also carries `target_digest: [u8;32]` (the signed preview digest, frozen at Prepare) and `challenge_consumed: bool` (DIAGNOSTIC copy only — the authoritative single-use mechanism is the `destruction_challenges` delete-if-matches, §6a). A `Rejected` row NEVER has a non-empty `audit_pending` — its best-effort `destruction_rejected` event is emitted directly, never enqueued; boot loading excludes `Rejected` from the non-empty-`audit_pending` set (131 §4d step 1). `error_class` contains only `ExternalLock | PermissionDenied | OwnershipMismatch | UnknownIo`; unsupported backends are refused at preflight and have no row variant.

**A.2a `destruction_challenges` table (new, v1):** `{ challenge_id: [u8;32], digest: [u8;32], session_binding: [u8;32], created_at_ns: u64, expires_at_ns: u64 }` — single use enforced by durable DELETE-IF-MATCHES at Prepare; expiry sweep is opportunistic (§6a). **A1: the DELETE-IF-MATCHES and the `Prepared` PUT-IF-ABSENT are ONE system-store transaction; a failed challenge writes no row.**

**A.2d `SubscriptionRemoval` operation record (round 6, K3 — NEW production code, deliverable of 8b-3c):** a `destruction_operations` row with `kind: SubscriptionRemoval` (131 §3b/§4c); v1 row: `{ operation_id: String(≤64), format_version: u16 = 1, kind: SubscriptionRemoval, subscription: String(≤255), binding_generation: u64, destinations: Vec<{db, repo}> (frozen), phase: Pending | RowDeleted | Completed, state: Prepared | Blocked (Blocked carries `failed_phase`/`error_class` like §3b of 131), executor_epoch: u64 (same claim/epoch-CAS discipline as §3b of 131), record_version: u64, created_at/updated_at: u64 (ns) }`. **Round 6 (C-A3) — generation width:** the subscription ROW stores `binding_generation` as `QueryValue::Int` (i64); the record field and the delete predicate compare in the ROW's stored (i64) domain; the schema/validation pins `binding_generation < 2^63` so the u64 record field never wraps on cast — a wrap-attempt at creation is a validation error. **The R1 lookup key is `(subscription, binding_generation)` — NEVER the subscription row**; resumed removals take ALL revalidation predicates from the RECORD. **Round 6 (C1):** the delete predicate is `(name, binding_generation == record.binding_generation)` — an additional `Eq` filter on the row's `binding_generation` field (`Filter::And` pattern of system_store.rs:330-352); the merged name-only delete (admin_replication.rs:420-428) could destroy a REBOUND G+1 subscription (rebind bumps the generation, execute/replication_seed/catalogue.rs:227-251 under its own control section); a generation mismatch fails closed: the record is marked `Completed` WITHOUT deleting (recovery landed, causes cured) and only its own generation's claims are released. Retried via DROP SUBSCRIPTION or `destruction_retry` (Appendix B); a resumed `RowDeleted` removal whose frozen-destination re-verification fails goes `state = Blocked` (`error_class = OwnershipMismatch`, `blocked_component` unset, `last_error` names the live incarnation — round 6, F1b: resolvable by `destruction_retry` after the squatter is removed, or by owner-approved archival of the BLOCKED record, Appendix B); crash/ambiguity table: 131 §4c.

**A.2c Phase enum + safe-restart mapping (DECIDED):** `Phase ∈ { Preflight, Prepare, Quiesce, Detach, Destroy, Catalogue, Complete }`. The phase→safe_restart_phase mapping: a `Blocked` during phase X restarts at X (via `safe_restart_phase`). All phases are idempotent. `Destroy` restarts at `Destroy` with the absence re-check (per-component `verify_absent`); `Catalogue` restarts at `Catalogue` with the I14 catalogue re-read (131 §4a step 5).

**A.2b Install-journal record (new, v1 — DECIDED):**

```text
{ attempt_id: String(≤64), storage_object_id: [u8;16], db: String(≤255),
  repo: String(≤255), receipt_digest: [u8;32], created_at: u64 (ns),
  format_version: u16 = 1 }
```

Records the NEW attempt's storage-object id so an interrupted install's preflight accepts components owned by THAT attempt only (§8c flow 2).

**A.3 `destination_incidents` row (system store table, v1)** — 131 §3d; `incident_id`/`storage_object_id` `[u8;16]`; `cause.generation` u64; `cleared_by: Option<ClearedBy>` where `ClearedBy = ByRecovery { evidence_digest: [u8;32], cleared_at_ns: u64 } | ByDestruction { operation_id: String(≤64), cleared_at_ns: u64 }`; plus the recovery baseline fields (**DECIDED**): `receipt_artifact_digest: [u8;32]`, `source_lineage_id: [u8;16]`, `source_history_epoch: u64`, `bookmark_floor: u64` (plus cause/reason/timestamps/storage_object_id/db/repo as above).

**Recovery-clear predicate (EXACT, normative):** the incident is resolved ONLY when a replacement incarnation has `storage_object_id ≠` the incident's, its installed receipt `artifact_digest ==` the recorded `receipt_artifact_digest`, its `lineage_id ==` the recorded `source_lineage_id`, its `history_epoch ==` the recorded `source_history_epoch`, and its new bookmark `== bookmark_floor + 1`. The clear rewrites EXACTLY that incident's copies (row + repo record) by `incident_id`. Deletion of a subscription NEVER resolves an incident. The only other sanctioned clear path is DESTRUCTION COMPLETION — `Resolved(by destruction)`: `state = Cleared`, `cleared_by = ByDestruction { operation_id }`, written in the SAME durable write as the operation's `Destroyed` transition (131 §4a step 6); R1-stage-2 consequently records NO incident for an absent destination whose covering destruction operation has completed (131 §4c (a)).

**A.4 `MetaKey::ConsistencyIncidents` (repo info store, MetaEnvelope v1)** — **DECIDED:** the repo-carried record holds the FULL incident copy (ALL A.3 fields, incl. the recovery baseline), not only id + digest, so missing catalogue rows are rebuilt from it at boot when the repo is openable:

```text
{ unresolved: Vec<IncidentCopy>,   // IncidentCopy = the full A.3 row (all fields)
  format_version: u16 = 1 }
```

**Degraded-copy rule — aligned with 131 §4c (a):** repo unopenable ⇒ the catalogue copy alone is authoritative. The R1-stage-2 handoff completes when the catalogue copy is verified AND (the repo copy is verified OR the destination is NOT live (absent or destroyed) — no repo copy is possible for an absent destination). Retry is idempotent by `(cause, storage_object_id)` with incident-id reuse (131 §3d).

**A.5 Audit event envelope (extension of `AuditEntry`)** — `details_canonical_msgpack` payload:

```text
{ event_id: { operation_id: String(≤64), transition_ordinal: u32 },
  operation_kind: "drop_repo" | "drop_db",
  storage_object_ids: Vec<[u8;16]>,
  anchors: Vec<{ name: String(≤255), generation: u64 }>,
  actor: String(≤255), reason: String(1..=512), target_state: enum, outcome: enum }
```

**Id note:** `event_id = { operation_id, transition_ordinal }` — `record_version` and `target_state` are NOT part of the id (`target_state` remains a separate payload field). A retried `Blocked` emits a second `blocked` event for the same state, so distinct ordinals are required for idempotent replay/reconciliation (131 §3e reconciliation rule).

Enums (normative): `event_kind ∈ { prepared, rejected, quiesced, catalogued, destroyed, blocked }` — there is NO `destroying` event kind: the `Destroying` transition emits no dedicated audit event (131 §3e); **`destroyed` is THE terminal event kind for `Destroyed`**; a retired `Blocked`'s final `blocked` event likewise gates retirement (131 §3e). The `rejected` kind is emitted best-effort DIRECTLY at the pre-write refusal and is NEVER enqueued in `audit_pending` (which stays empty for `Rejected`). The event kind names map to the operation state whose completion they record (`prepared` records `Prepared` reached, `catalogued` records `Cataloguing` finished, etc.). `target_state ∈` operation states (the 3b state enum); `outcome ∈ { emitted, replayed, acked }`.

**A.6 Wire DTO `destroy_quarantined_data`** — `{ reason: String(1..=512, printable), operation_id: String(≤64), nonce: String (32 hex chars), digest: String (64 hex chars — the `destruction_preview` digest), challenge: String (from `destruction_preview`) }`; echoed back as `{ operation_id, state, nonce }`. **The preview exchange (new admin wire op):** `destruction_preview(targets, cascade) -> { targets: Vec<{db, repo, storage_object_id}> (sorted by (db, repo)), digest: [u8;32], challenge: String (single-use), expires_at_ns }`; the challenge is consumed at Prepare and the executor re-resolves the live/catalogued set against the signed digest (fail-closed `destruction_target_changed`, §6c).

**A.7 `AuditAck`** — `{ seq: u64, checkpoint_position: [u8;32] }` (131 §3e).

**A.7a `checkpoint_durable` (DECIDED):** `checkpoint_durable() -> Result<AckWatermark, AuditError>` — flushes + fsyncs the log FIRST and FAILS if the flush failed (unlike the OBSERVED void `appender.checkpoint()`, audit_appender.rs:706-713, which logs the flush failure and still writes the checkpoint), then writes the checkpoint keyspace durably (SyncAll) and returns `AckWatermark = next_seq − 1` — the highest assigned seq covered by the durable log + checkpoint, advanced ONLY by a successful write+sync of a contiguous prefix (131 §3e item 0b); the chain poisons on appender failure. **Poison attribution:** ONLY by a durability failure ATTRIBUTED to a specific append (the synchronous barrier/append path — the caller learns the event id); a BACKGROUND-flusher failure is NOT a poison: the batched flusher RETAINS the failed batch in the buffer (never drops it) and marks the sink DEGRADED; the next successful flush of the retained batch advances the watermark; if it cannot be re-flushed, the NEXT attributed barrier append fails and poisons normally. When the barrier's batch flush fails, ALL co-batched entries' seqs are NOT advanced in the watermark (contiguous-prefix rule only); their owners learn via their own ack paths; no per-entry action is required. A poison that survives reconciliation retries surfaces as `AuditSinkError::Poisoned` on every append and requires operator/ops action — never silent clearing. **CLOSED RULE — the ONLY proofs that a barrier event is durable are** (a) the original append's own successful sync (ack returned); (b) a FRESH append of the SAME event_id that succeeded (idempotent by event id — duplicates tolerated by the verifier, deduplicated on read; includes the reconciliation RE-WRITE); (c) an ack record written by `checkpoint_durable` AFTER its own successful flush+sync naming a durable-through seq ≥ the event's seq. A found-but-unconfirmed entry in the log tail is NOT evidence and must never be adopted; `AuditSink::contains`/`read_durable_tail` only answer (c)-style watermark questions and locate entries for the (b) RE-WRITE — never authorize adoption by themselves. Restart: the executor holds no memory of (a); it uses (b) by re-appending the pending entry or (c) by reading the persisted checkpoint_durable ack record (which survives in the checkpoint keyspace) — never by trusting the log tail alone. An outbox event is acknowledged only when `ack_watermark >= event.audit_seq`; the periodic scheduler (scheduler.rs:204-216) switches from void `appender.checkpoint()` to `checkpoint_durable` + an ack POST to each operation's executor (single-writer model, 131 §3b round 5). **Round 6 (C3) — the unpoison path:** a POISONED chain admits exactly ONE append class — the reconciliation RE-WRITE of the failed event id (the sink bypasses the poison check for that single event id, tracked by the chain) and UNPOISONS upon that entry's own successful `write_all` + `sync_all`; any other append during poison returns `AuditSinkError::Poisoned`. Invariant: poison ⇒ either the failed entry is re-written durably or the sink stays poisoned (fail-closed), never silently cleared. `checkpoint_now` (audit_chain.rs:436-441) is a manual force, NOT a durability barrier.

**A.7b `AuditSink` (NEW — deliverable of 8b-3b):** the `shamir-db`-side seam, no new crate dependency:

```rust
pub trait AuditSink: Send + Sync {
    fn append_durable(&self, entry: &AuditEntry) -> Result<DurableAck, AuditSinkError>;
    fn contains(&self, event_id: &EventId) -> Result<bool, AuditSinkError>;   // durable-tail lookup — answers (c)-style
                                                                              // watermark questions and LOCATES entries for
                                                                              // the (b) RE-WRITE only; NEVER authorizes adoption
    fn durable_watermark(&self) -> Result<u64, AuditSinkError>;               // durable-through seq (contiguous-prefix rule)
}
pub struct DurableAck { pub seq: u64, pub checkpoint_position: [u8;32] }      // = AuditAck
pub enum AuditSinkError { Unavailable, Poisoned, Io }   // Poisoned: the chain has a failed event id awaiting its
                                                        //   reconciliation RE-WRITE (the only admitted append class);
                                                        //   all other appends fail until it is durable
                                                        //   or the poison stands (never silently cleared)
```

**Poison attribution:** a poison is set ONLY by a durability failure ATTRIBUTED to a specific append (the synchronous barrier/append path — the caller learns the event id). A BACKGROUND-flusher failure is NOT a poison: the batched flusher RETAINS the failed batch in the buffer (never drops it) and marks the sink DEGRADED; the next successful flush of the retained batch advances the watermark; if it cannot be re-flushed, the NEXT attributed barrier append fails and poisons normally. When the barrier's batch flush fails, ALL co-batched entries' seqs stay unadvanced in the watermark (contiguous-prefix rule only); their owners learn via their own ack paths; no per-entry action is required. A poison that survives reconciliation retries surfaces as `AuditSinkError::Poisoned` on every append and requires operator/ops action — never silent clearing.

**A.9 Log framing (A9, normative):** each audit log record carries length + checksum framing. On open, the sink truncates an incomplete or bad-checksum FINAL record before any append; a corrupt MIDDLE record fails closed (poisoned chain). Same event id + different payload SHA-256 = corruption: poison plus incident, never dedup. The barrier I/O runs on `spawn_blocking` with a bounded timeout; on timeout the row stays `Prepared` with `destruction_audit_unavailable`. Group commit is allowed only if each acknowledgement covers its own sequence number (A10). Latency is exposed as a metric.

The server (shamir-server) implements it over the batched appender; the launcher injects `Arc<dyn AuditSink>` into `ShamirDb::init_with_env_policy(config, audit_sink: Option<Arc<dyn AuditSink>>)` BEFORE replay; outbox replay waits for the sink. Invariant protected: the barrier proves durable intent without in-memory chain state and without a server dependency in shamir-db.

## Appendix B — administrative inspect/retry API (F7-iv, DECIDED)

On the existing admin surface (wire DTOs alongside the other `destruction*` operations; the HTTP/wire handler returns immediately — status is a pure read, retry spawns a detached executor per §4f of 131):

- `destruction_status(operation_id) -> { state, failed_phase?, error_class?, safe_restart_phase?, targets: Vec<{db, repo, storage_object_id}>, progress: Vec<{component_id, kind, absent: bool | "unknown"}> — **DERIVED** from `verify_absent` probes at status time, not durably stored, audit_acked: bool — true when `audit_pending` is empty (all outbox entries drained), anchors_remaining }` — a **pure read of `destruction_operations`** (+ a `verify_absent` probe only when the caller asks for `?verify=true`); a lost response is answered by re-issuing the call (idempotent, no side effects).
- `destruction_retry(operation_id)` — resumes a `Blocked` operation from `safe_restart_phase`, **or claims a stranded `Prepared` operation (audit-barrier failure) by re-running the barrier (idempotent by event id) and spawning the executor (131 §3e)**; **also accepts a `SubscriptionRemoval` operation (the R1-stage-1 record) — looked up by `(subscription name, binding generation)` (the record's `operation_id` accepted too) FIRST, resumed from the recorded `phase` (`RowDeleted` ⇒ destination-absence re-verification + completion, predicates from the RECORD); `Completed` ⇒ status**; refuses on terminal states (`Destroyed` → status; `Rejected` → new operation required); **a resumed `Pending`-phase removal whose live row's `binding_generation` ≠ the record's NEVER deletes — the record completes WITHOUT deleting (a new generation means a recovery landed, causes cured) and only the record generation's claims are released; the delete predicate is `(name, binding_generation == record.binding_generation)`**; authorization required. **Claim enumeration: every claim path INCLUDING the INITIAL executor spawn — initial spawn, same-id resubmission, `destruction_retry`, periodic pass (`Prepared` older than 5 min), boot — performs ONE CAS `(record_version, executor_epoch): (v, e) → (v+1, e+1)`; the winner runs the operation for that epoch, losers get the current status immediately (never block for the run; the initial-spawn loser returns the current status and does NOT spawn); every claim path re-runs FULL PREFLIGHT against the frozen `target_digest` before Quiesce. The claim CAS is CONDITIONAL on the claimer's observed `(v, e)`; the executor spawn is IMMEDIATELY ADJACENT to the successful CAS (no await between); and the executor RE-CHECKS `executor_epoch` currency before EACH destructive durable step (Quiesce/Detach/Destroy/Catalogue/Complete entry), aborting to status if superseded — these three together give the single-writer property (131 §3b).**
- **Destruction-authority surface (NEW PRODUCTION CODE, deliverable of 8b-3b; `pub(crate)` per A14, NOT dispatchable from wire or admin):** `pub(crate) struct DestructionAuthority { operation_id: String(≤64), targets: Vec<(String, String)>, /* + opaque minted token */ }` minted by the executor per operation id + frozen target set; `pub(crate) fn remove_repo_locked_destructive(db, repo, &DestructionAuthority, &ReplicationControlGuard)` (db_management.rs, beside the merged `remove_repo_locked` :561) skips `ensure_destinations_unbound` (validate.rs:143) EXACTLY for the token's targets — the destruction path is the only caller that may remove a repo referenced by a durable binding; every other caller stays refused. The executor takes control for the detach section (131 §4a step 3). **A14: destructive dispatch becomes wire/admin reachable only in 8b-3c, together with all publisher wiring.**
- `destruction_preview(targets, cascade)` — returns the frozen target list (sorted by (db, repo)), its SHA-256 digest, a single-use challenge (≤ 60 s) and `expires_at_ns`; the client signs the digest + challenge on the subsequent `DESTROY QUARANTINED DATA` statement. A pure read + challenge mint; no durable lifecycle state.
- `destruction_list(limit: u32, pagination_token: Option<String>) -> { items: Vec<{ operation_id, state, targets }>, next_token: Option<String> }` — bounded (≤ `limit` items), ordered by `created_at`, paginated via the opaque `pagination_token`/`next_token` pair.
- Optional (8b-3d+): `destruction_archive(operation_id)` — owner-approved archival. **DECIDED (A2, DEC-3 item 7): `destruction_archive` NEVER releases protection.** Eligible only for Repo/Database `Destroyed` + fully acked, and SubscriptionRemoval `Completed`. Archival REPLACES the row with a permanent compact tombstone `{operation_id, kind, final_state, frozen storage_object_ids, sha256(full row), archived_at, archived_by}`; operation ids are NEVER reusable. A final `Blocked` operation exits ONLY through `destruction_retry` → `Destroyed`. A `Blocked` SubscriptionRemoval at `RowDeleted` whose re-verification found data converts that data into an R2' target or an incident, NEVER a release. Backpressure: a quota on concurrent non-terminal operations; new destruction is refused when the quota is reached; nothing protected is ever evicted. No automatic expiry. This supersedes the earlier C8/F1b archival-eligible-with-release forms (now marked superseded in 131c/131d).

**Authorization:** same **Delete** authority as DROP **plus** replication-management authority. **Idempotence/safety:** status/list are pure reads; retry is idempotent for an already-running operation (returns current state); no admin call can bypass the audit barrier or delete a journal row with an unacked terminal event.
