# RFC: Unified DDL Result Contract (operation ID + status)

**Status: DRAFT — pending review**
**Author:** S.H.A.M.I.R. DB engineering
**Date:** 2026-08-05
**Tracks:** #985 (this RFC), #967 (enriched-error-string workaround this replaces),
#997/#988/#972/#962/#961/#959 (crash-recovery tombstone mechanisms this surfaces).

> This is a **proposal** for review, not a final contract. Every wire type named
> below is illustrative (`DRAFT — pending review`); none exist yet. Every claim
> about *existing* behavior is grounded in a file + line range read for this RFC.

---

## 0. TL;DR

There is today **no per-operation identity and no per-operation status** anywhere
on the wire: not on `QueryResult`, not on `BatchResponse`, not on `DbResponse`.
A DDL op either populates `results[alias]` on success, or — on failure — aborts
the **entire** batch and replaces the whole response with a top-level
`DbResponse::Error` (sibling aliases already computed are discarded, not
returned). Worse, the tombstone crash-recovery mechanisms built this session
(#959, #961/#988, #962, #972, #997) can now **silently finish** an interrupted
DDL op on the *next* server restart, with **no client ever learning** the op
eventually completed — because the recovery clears its tombstone on success and
leaves no queryable trace.

**Recommendation:** adopt design (a) — a **separate, durable, poll-by-op-id
status endpoint** — as the load-bearing mechanism, **plus** an additive
`op_id` field embedded in the *synchronous* DDL `QueryResult` so a client
that gets a successful synchronous reply has something to quote later (and a
client that timed out can correlate). The embedded field carries no status of
its own for the *crash* case; the durable poll log is the only thing that can
answer "did my CREATE INDEX from before the crash finish?"

This requires a **new durable append-only operation-status log**, not a layer
over the existing tombstones — because the tombstones are *cleared on success*
and are *keyed by name*, not by a client-supplied/stable op id.

---

## 1. Design question — poll endpoint vs. embedded fields

### 1.1 The two options, stated precisely

**(a) Separate poll endpoint.** A new `DbRequest` variant, e.g.
`GetDdlOpStatus { op_id } -> DdlOpStatus { op_id, kind, state, … }`, queried
independently of (and possibly long after) the original synchronous DDL call.

**(b) Embedded in every synchronous DDL `QueryResult`.** Add fields directly
to the DDL op's result, e.g. `op_id: RecordId` and `status: DdlOpStatus`, so a
client reads the status off the same `BatchResponse` it already awaited.

### 1.2 The asymmetry that settles the core of the question

The decisive constraint is the **temporal asymmetry between when a DDL call is
made and when a crash-recovered op actually completes**:

- Today every DDL call is synchronous request/response. The client awaits a
  single `BatchResponse` (engine entry `execute_batch`,
  `crates/shamir-engine/src/query/batch/batch_execute.rs:59-80`; the synchronous
  round-trip is `ShamirDb::execute_as` in
  `crates/shamir-db/src/shamir_db/execute/db_execute.rs:29-105`).
- A durable op record only becomes *interesting* **after** a crash + restart —
  i.e. after the original request/response pair is long gone. The tombstone
  recovery that finishes the op runs at **open time, before any writer can reach
  the table** (`recover_hash_renames` / `recover_index2_drops` are called from
  `TableManager::create`, `crates/shamir-engine/src/table/table_manager.rs:500-512`,
  strictly before the table is open for client traffic).
- A field on the *original synchronous response* **cannot carry information
  from after a crash that happens after that response was already sent** (or
  never sent — see §1.3). Once the connection has torn down, there is no
  response object left to read a `status` field off of.

**This asymmetry alone settles the load-bearing part: the only way a client
can ever learn "did my CREATE INDEX from an hour ago, before the server
crashed, actually finish?" is to ask again, later, by some stable identifier.
That is a poll endpoint (design a). Embedded status (design b) is structurally
incapable of answering the post-crash question it exists to answer.**

### 1.3 Why (b) is still wanted — as a *correlation handle*, not a status carrier

Embedded-only is insufficient, but that does **not** make (b) useless. There is
a real role for embedding `op_id` (and *only* `op_id`, not `status`) in the
synchronous DDL `QueryResult`:

- **The client needs something to poll *by*.** A poll endpoint keyed by `op_id`
  is useless if the client never receives an `op_id`. The natural place to mint
  and return that id is the synchronous reply — the server assigns the op id at
  dispatch time and echoes it back in the `QueryResult` it is about to return.
- **Correlation across the client's own crash/reconnect.** A client whose own
  process restarted, or an operator's monitoring tool, needs a stable id logged
  client-side to poll with later. The synchronous `op_id` is what gets logged.
- **Degenerate-but-common case: the synchronous reply succeeds normally.** For
  the overwhelming majority of DDL ops (no crash), the op completes inline and
  the synchronous reply already carries the outcome. A `status: DdlOpStatus`
  field set to `Succeeded { … }` on the synchronous path is harmless and saves a
  round-trip — but it must be understood as an *optimization for the common
  case*, not as the mechanism that handles the crash case.

**Conclusion: adopt (a) as the contract of record (the durable poll log is the
source of truth), and adopt (b) in the *narrow* form of an additive `op_id`
field on the synchronous DDL `QueryResult` (with a `status` field that is
authoritative only for the inline-succeeded case).** They compose; they are not
alternatives. The brief's framing of "(a) or (b)" resolves to "**(a) is the
contract; (b) is the handle plus the fast-path optimization.**"

### 1.4 Why a NEW durable log is required, not a layer over existing tombstones

The existing tombstones are **not** a suitable backing store for a client-facing
"status by op_id" contract, for two structural reasons:

1. **Tombstones are cleared on success.** Every recovery path ends by clearing
   the tombstone once the op is durably complete — `recover_index2_drops`
   overwrites `system:_m.idx.drop` with an empty `Vec<u32>`
   (`table_manager_index_mgmt.rs:986-991`); `recover_hash_renames` calls
   `clear_from_renaming(...)` for each finished entry
   (`table_manager_index_mgmt.rs:1126-1130` regular, `1174-1176` unique). The
   *non-recovery* success paths clear them too (`drop_index2` clears at
   `table_manager_index_mgmt.rs:910-923`; `rename_index` clears at `1459-1471`
   regular / `1559-1573` unique). So **a successfully-completed op leaves no
   durable trace**. A client polling "did op X succeed?" would find *nothing* —
   indistinguishable from "op X never existed."
2. **Tombstones are keyed by name / name-pair / id-set, not by a stable op id.**
   The keys are `system:idx_drop`, `system:uidx_drop`, `system:sidx_drop` (sets
   of interned ids), `system:idx_ren` / `system:uidx_ren` (lists of
   `HashRenameTombstone { old_name, new_name, paths }`), `system:sidx_ren` (an
   id-pair map), `system:_m.idx.drop` (a `Vec<u32>` of index2 ids)
   (`crates/shamir-index/src/base_index/index_manager.rs:508-719`,
   `crates/shamir-index/src/base_index/sorted_index_manager.rs:659-1307`,
   `crates/shamir-index/src/persistence.rs:295`). There is no field a client
   could supply. These are an **internal recovery mechanism**, designed never to
   be queried by a client.

A client-facing status contract needs to answer **"yes, this succeeded"** for a
window *after* completion, not just "still in progress" / "still recovering."
That is a **different data structure**: an **append-only operation-status log**
keyed by a stable `op_id`, with its own retention/GC policy (e.g. TTL or
entry-count cap), surviving success. The tombstones and the op-status log are
layered, not unified: the tombstone remains the recovery mechanism (it drives
*what to do*); the op-status log is the *visibility* mechanism (it records *what
happened*, queryable by id). See §3.4 and §5.

---

## 2. Interaction with #997 / #988 / #972 crash recovery

### 2.1 The gap, stated precisely

For an op that crashed mid-flight and was silently finished by recovery on the
**next** restart, the client never sent a new request and never got a new
response for that op. Today:

- **Before the crash:** the client sent the request and was either still
  waiting, or the connection had already torn down. On a server crash, TCP does
  not guarantee a clean FIN — the client typically observes a connection reset
  or its own client-side timeout (`ShamirTimeoutError`, phase `request`,
  `crates/shamir-client-ts/src/core/errors.ts:87-103`).
- **After recovery:** the op **is** done. But recovery clears the tombstone and
  writes **nothing** client-visible. There is no op id recorded anywhere, no
  status entry, no changelog the client can see. Polling by *anything* the
  client has returns "unknown."

So with the current code, a post-recovery poll has nothing to find. **This is
the core defect this RFC exists to fix.**

### 2.2 The design that makes recovery coherent: recovery writes the SAME op_id

The fix is that **every DDL op is assigned an `op_id` at dispatch time, and that
same `op_id` is carried into the tombstone**, so recovery — which already reads
the tombstone — can write a "completed via crash-recovery" entry under that
same id into the new op-status log (§1.4). Concretely:

1. **Dispatch:** when the server begins executing a recoverable DDL op (the
   index CREATE/RENAME/DROP family that already has tombstones), it mints an
   `op_id` and writes a `{ op_id, kind, state: InProgress, … }` record to the
   op-status log *before* the first mutating step. (This mirrors exactly how the
   tombstone itself is written before the first mutating step today — e.g.
   `add_to_renaming(...)` precedes `create_index` at
   `table_manager_index_mgmt.rs:1427-1440`.)
2. **Tombstone carries the op_id:** the tombstone payload gains an `op_id`
   field. (The `HashRenameTombstone` at `index_manager.rs` and the index2 drop
   set would carry it; the sorted tombstones analogously.)
3. **Normal success:** on the synchronous success path, the op-status record is
   flipped to `Succeeded` (and the tombstone cleared, as today).
4. **Recovery success:** `recover_hash_renames` / `recover_index2_drops` /
   `recover_in_progress_drops` / `recover_in_progress_renames`, *as they clear
   each tombstone*, also write the `op_id` they just read off that tombstone to
   the op-status log as `SucceededViaCrashRecovery { completed_at_restart, … }`.
   Recovery already iterates the tombstone payloads (e.g.
   `recover_hash_renames` loops `regular_renames` / `unique_renames` at
   `table_manager_index_mgmt.rs:1098-1177`), so the `op_id` is in hand at the
   exact moment the entry should be written.

A client (or a *new* client instance, or an operator's monitoring tool) polling
by `op_id` after restart then sees `SucceededViaCrashRecovery` — a coherent,
distinguishable-from-inline-success answer.

### 2.3 Worked example — #997 unique RENAME, SEVERE case

This is the most severe recovery scenario characterized this session. Tracing it
end-to-end against the actual code:

**Setup (synchronous path,**
`rename_index` unique branch,
`table_manager_index_mgmt.rs:1484-1574`**):**

1. Client sends `RENAME INDEX old → new` on a UNIQUE index, in a batch.
2. The engine resolves `old_id`/`new_id`, confirms `new` is free (guards at
   `1386-1391`).
3. **#997 tombstone write** (`add_to_renaming(true, old_id, HashRenameTombstone{old_name, new_name, paths})`,
   `1502-1512`) — *before* any mutation. *(Under this RFC: this is where the
   `op_id` is minted, written to the op-status log as `InProgress`, and stored
   in the tombstone payload.)*
4. **Barrier + `unique_write_lock`** acquired (`begin_write_barrier(UNIQUE_INDEX_CREATE)`,
   `1531-1533`) — blocks all writers for the drop→create span.
5. **`drop_unique_index(old_id)`** (`1535`) — the old unique index is gone from
   both memory and disk.
6. **THE SEVERE CRASH WINDOW** (`maybe_pause_rename_mid()` test seam at `1540`,
   and the real crash it models): the server dies here. **Both `old` and `new`
   are now absent.** The old `IndexDefinition` is already gone (the unique path
   drops-old-first by design, `1474-1483`), so nothing short of a full backfill
   can reconstruct the constraint.
7. *(Never reached in the crash case:)* `create_unique_index_body(new_name, paths)`
   (`1547-1557`), then `clear_from_renaming(true, old_id)` (`1563-1573`).

**What the client observes at the crash:** the TCP connection resets or the
client times out. The synchronous reply — if the server died before sending one
— never arrives. Even if the server had already serialized a reply, it would be
lost. The client's only artifact is the `op_id` it (under this RFC) minted or
received and logged.

**Recovery on next restart** (`recover_hash_renames`,
`table_manager_index_mgmt.rs:1083-1186`**):**

1. `load_renaming_list(true)` returns the stranded tombstone (`1085`).
2. Unique loop (`1134-1177`): `unique_index_exists(new_name)` is **false**
   (`1138` → else-branch at `1160`).
3. **`create_unique_index(new_name, paths)`** (`1165`) — rebuilds the constraint
   from the live record stream, re-running the uniqueness backfill. (Safe because
   recovery runs before any writer can reach the table — `1062-1073`.)
4. Drop-old if present (`1170-1172`) — no-op here, already gone.
5. **`clear_from_renaming(true, old_id)`** (`1176`) — tombstone gone.

*(Under this RFC: step 3's success is what flips the op-status record — keyed by
the `op_id` read off the tombstone at step 1 — to
`SucceededViaCrashRecovery { completed_at_restart }`.)*

**What the client/operator sees, post-recovery, by polling `op_id`:**
`DdlOpStatus { op_id, kind: RenameUniqueIndex, state: SucceededViaCrashRecovery,
old_name, new_name, completed_at_restart }`. **Without this RFC, the same poll
returns "unknown op_id" — the op silently completed and left no trace.**

### 2.4 Status vocabulary (DRAFT)

Grounded in the recovery code's actual terminal states, the `DdlOpState` enum
should distinguish:

| state | meaning | who writes it | code anchor (example) |
|---|---|---|---|
| `InProgress` | op started, not yet durably complete | dispatch, before first mutation | tombstone-write sites (`1502`) |
| `Succeeded` | completed on the synchronous path | normal success path | tombstone-clear sites (`1461`, `1563`) |
| `SucceededViaCrashRecovery` | completed by open-time recovery on a later restart | recovery fn, as it clears each tombstone | `recover_hash_renames` (`1126`,`1176`); `recover_index2_drops` (`986`) |
| `Failed` { detail } | op failed and was **not** recovered (needs `verify()`/`repair()`) | the existing `#967` enriched-error sites | `table_manager_index_mgmt.rs:1450`,`1549`; `table_manager_sorted_index.rs:176`,`276` |
| `Unknown` | op_id not found (GC'd, never existed, or pre-RFC op) | poll endpoint default | n/a (new) |

`SucceededViaCrashRecovery` is **essential and non-collapsing with `Succeeded`**:
an operator monitoring the "did my unique constraint come back?" question needs
to know the constraint was rebuilt by recovery, not by the original call — it
implies a restart happened in between and any in-flight writers at crash time
did not see the constraint transiently.

---

## 3. Blast radius — concretely enumerated

### 3.1 New response fields/shapes in `shamir-query-types` (DRAFT names)

All additive, all `#[serde(default, skip_serializing_if = …)]` to match the
existing convention pervasive in
`crates/shamir-query-types/src/read/query_result.rs:118-178` and
`crates/shamir-query-types/src/batch/batch_response.rs:29-65` (see the
`interner_delta` field at `batch_response.rs:57-64` for the canonical
backward-compatible additive-field precedent in this very struct).

- **`DdlOpStatus`** (new struct) and **`DdlOpState`** (new enum) — the poll
  response payload and its state vocabulary (§2.4). Lives alongside the existing
  `QueryResult` in the `read` module or a new `ddl` submodule.
- **`QueryResult::op_id: Option<RecordId>`** — additive, `#[serde(default, skip_serializing_if = "Option::is_none")]`. The synchronous handle a client polls by later. `None` for read/DML ops and for pre-RFC peers (old decoders never see the field; new decoders treat absence as "not a DDL op").
- **`QueryResult::ddl_status: Option<DdlOpState>`** — additive, same skip rule. Authoritative only for the inline-`Succeeded` common case; absent (or `InProgress`) when the op is recoverable-and-still-running (unusual for synchronous DDL, but the field must tolerate it).
- **`DbRequest::GetDdlOpStatus { op_id }`** (new enum variant,
  `crates/shamir-query-types/src/wire/db_message.rs:32`) and the matching
  **`DbResponse::DdlOpStatus { status: Option<DdlOpStatus> }`** (`db_message.rs:266`).
  These are new enum arms — old peers never send them; a server that does not
  understand `GetDdlOpStatus` should reply with `DbResponse::Error { code:
  "not_supported", … }` (an existing code, `db_message.rs:308`), which the SDK
  surfaces as a normal retryable/terminal error. *Caveat for review:* unlike the
  additive struct fields above, a new `DbRequest`/`DbResponse` enum arm is not
  transparent to an old server's deserializer — see §6 (open questions).

### 3.2 DDL execute handlers in `crates/shamir-db/src/shamir_db/execute/`

Verified against the directory (the brief's list was a subset). The files that
host DDL/admin handlers, with what changes for the *first slice* (index ops only)
vs. later:

| file | ops routed here (via `admin_dispatch.rs:25-90+`) | change for first slice? |
|---|---|---|
| `admin_dispatch.rs` | the router itself | **yes** — central place to mint `op_id` for recoverable ops and thread it into the `QueryResult` |
| `admin_table_index.rs` | `CreateTable`, `DropTable`, `CreateIndex`, `DropIndex`, `RenameIndex` | **yes (index ops only)** — `CreateIndex`/`DropIndex`/`RenameIndex` mint/return `op_id`; `Create/DropTable` defer |
| `admin_db_repo.rs` | `CreateDb`/`DropDb`/`CreateRepo`/`DropRepo`/`RenameRepo`/`RenameDb` | defer |
| `admin_schema.rs` | `SetTableSchema`/`AddSchemaRule`/… (+ `RenameTable`) | defer |
| `admin_function.rs` | `Create/Drop/RenameFunction`, `Create/RenameFunctionFolder` | defer |
| `admin_validator.rs` | `Create/Drop/RenameValidator`, `Bind/Unbind/ListValidators` | defer |
| `admin_migration.rs` | `Start/Commit/RollbackMigration`, `MigrationStatus` | **reference, not a change target** — `MigrationStatusOp` (`crates/shamir-query-types/src/admin/types/migration_ops.rs:48`) is the *existing* poll-by-id precedent this RFC generalizes; keep its model in view |
| `admin_users_roles.rs` | `Create/DropUser`, `Grant/RevokeRole` | defer |
| `admin_access.rs` | `Chmod/Chown/Chgrp`, group DDL | defer |
| `admin_retention.rs` | `SetRetention`/`PurgeHistory`/`ChangesSince` | defer |
| `helpers.rs` | `admin_result(...)` builder used by every handler | **yes** — likely the single function to extend to stamp `op_id` into the returned `QueryResult`, minimizing per-handler churn |

### 3.3 Engine layer (where the op_id actually rides the tombstone)

This is where §2.2 lands. The tombstone payloads and recovery functions in:

- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs` —
  `drop_index2` (`831-925`), `recover_index2_drops` (`952-999`),
  `recover_hash_renames` (`1083-1186`), the `rename_index` regular/unique
  branches (`1393-1574`).
- `crates/shamir-index/src/base_index/index_manager.rs` — `idx_drop`/`uidx_drop`
  set tombstones (`508-660`), `idx_ren`/`uidx_ren` list tombstones
  (`660-810`), `recover_in_progress_drops` (`857`).
- `crates/shamir-index/src/base_index/sorted_index_manager.rs` — `sidx_drop`
  (`642-856`), `sidx_ren` (`1083-1307`), `recover_in_progress_drops` (`796`),
  `recover_in_progress_renames` (`1274`).

Each tombstone payload gains an `op_id` field; each recovery fn writes the
op-status record as it clears its tombstone. The #967 enriched-error TEXT sites
(`table_manager_index_mgmt.rs:368-369, 390-391, 418-419, 1450-1457, 1549-1557,
…`; `table_manager_sorted_index.rs:176-177, 276-277`) become **structured
`Failed { detail }`** records under the same `op_id` instead of free-text — the
exact "parse error TEXT" workaround this RFC retires.

### 3.4 New durable store — the operation-status log

A new keyed, append-only (write-then-flip-state) store, distinct from the
tombstones (§1.4). Proposed key: `system:ddl_op:<op_id bytes>`; value: a
bincode/msgpack `DdlOpStatus`. Retention: TTL or bounded entry count with LRU
eviction of terminal (`Succeeded`/`Failed`) records (open question, §6). Lives
in the same `info_store` the tombstones already use (no new storage substrate).

### 3.5 Both SDKs

- **Rust (`crates/shamir-client/` + `crates/shamir-query-builder/`):**
  `Client::execute` returns `Result<BatchResponse, ClientError>`
  (`crates/shamir-client/src/client.rs:770-808`); DDL failures surface as
  `ClientError::Db { code, message }` (`crates/shamir-client/src/error.rs:24-25`).
  No `op_id`/poll concept exists today (grep-verified). New surface: a
  `Client::get_ddl_op_status(op_id) -> Result<Option<DdlOpStatus>, ClientError>`
  method, and the `QueryResult::op_id` field is read off the existing
  `BatchResponse::results[alias]`. The `Batch` builder
  (`crates/shamir-query-builder/src/batch/batch.rs:47-63`) needs **no** change —
  the `op_id` is server-minted, not client-supplied.
- **TypeScript (`crates/shamir-client-ts/`):**
  `client.execute(db, batch): Promise<BatchResponse>`
  (`crates/shamir-client-ts/src/core/client.ts:602-648`); DDL failures throw
  `ShamirDbError { code, detail, retryable }`
  (`crates/shamir-client-ts/src/core/errors.ts:60-77`). No `op_id`/poll concept
  today (grep-verified). New surface: a `client.getDdlOpStatus(op_id):
  Promise<DdlOpStatus | null>` method, and the `QueryResult` interface gains
  `op_id?: string` / `ddl_status?: DdlOpState`
  (`crates/shamir-client-ts/src/core/types/batch.ts:211-257`). The DDL builder
  functions (`crates/shamir-client-ts/src/core/builders/ddl.ts`) need **no**
  change.

### 3.6 Backward compatibility

- Existing `BatchResponse` / `QueryResult` msgpack shapes **must keep decoding**
  for old peers. The two struct additions (`op_id`, `ddl_status`) are additive
  with skip-if-empty, exactly mirroring `interner_delta`
  (`batch_response.rs:57-64`) and `versions`/`corrupt_records`
  (`query_result.rs:151-177`). **Safe.**
- The new `DbRequest::GetDdlOpStatus` / `DbResponse::DdlOpStatus` enum arms are
  the one compatibility hazard: a new client polling an **old** server hits an
  unknown-request arm. Mitigation: the SDK treats `not_supported` as "feature
  unavailable" and falls back to today's behavior (the synchronous `op_id` is
  still useful for client-side correlation even without a poll endpoint). A new
  server receiving an unknown request from an old client never happens (old
  clients never send `GetDdlOpStatus`). *See §6.*

---

## 4. Recommended first-implementation slice

**First PR — the hash-family index rename/drop ops, poll endpoint, and the
op-status log.** Rationale:

1. **The tombstone infrastructure already exists and is the most thoroughly
   tested.** #997 (hash rename, regular + unique — including the SEVERE unique
   case traced in §2.3), #959 (base index DROP), and the index2 DROP (#988) all
   land in this family. Recovery is wired and idempotent
   (`table_manager_index_mgmt.rs:952-1186`). Wiring `op_id` onto these is the
   highest-confidence, best-tested slice.
2. **It exercises the full lifecycle in one slice:** dispatch-mints-op_id →
   tombstone-carries-op_id → synchronous-`Succeeded` path → crash →
   recovery-writes-`SucceededViaCrashRecovery` → client-polls-and-sees-it. If the
   contract is wrong, this slice finds out before it spreads.
3. **It retires the most painful #967 sites** (the unique-rename SEVERE enriched
   errors at `1549-1557`, the drop errors at `877-923`) by replacing free-text
   with structured `Failed`/`SucceededViaCrashRecovery`.

**First PR scope (in):**
- `DdlOpStatus` / `DdlOpState` types in `shamir-query-types` (§3.1).
- The op-status log (§3.4) keyed by `op_id`.
- `op_id` field on `QueryResult` + the `helpers::admin_result` change to stamp it
  (§3.2).
- `DbRequest::GetDdlOpStatus` / `DbResponse::DdlOpStatus` (§3.1).
- Tombstone `op_id` carry + recovery status-write for: hash `DROP INDEX`
  (regular+unique), hash `RENAME INDEX` (regular+unique, incl. SEVERE), and
  index2 `DROP INDEX` — the three families whose recovery is already wired.
- `Client::get_ddl_op_status` (Rust) + `client.getDdlOpStatus` (TS).

**First PR scope (defer to follow-ups):**
- Sorted-family `DROP`/`RENAME` (`sidx_drop`/`sidx_ren`, `sorted_index_manager.rs`)
  — same pattern, mechanically identical, but defer to keep the first slice
  reviewable and to avoid touching the sorted rekey settle loop.
- `CREATE INDEX` status (it has a `Building` state worth surfacing, but its
  recovery story is partly owned by #966 self-heal — needs a careful ownership
  split, §2 of the `recover_hash_renames` doc at `1039-1055`).
- All non-index DDL (db/repo/table/schema/function/validator/user/access) —
  these have **no** tombstone recovery today, so `SucceededViaCrashRecovery` is
  not yet meaningful for them; they get `op_id` + `Succeeded`/`Failed` only when
  their own recovery lands.
- Retention/GC policy for the op-status log (ship with a generous fixed cap +
  FIFO eviction; tune later).

A different, arguably smaller first slice would be "embedded `op_id` only, no
poll endpoint" — **rejected**: without the poll endpoint the post-crash question
(the entire motivation) is unanswerable, so it would ship a field with no
contract behind it. The poll endpoint is the contract; it belongs in the first
slice.

---

## 5. What this retires

- The **#967 "parse error TEXT" workaround** — every
  `DbError::Internal(format!("…Call TableManager::verify()…"))` site in the
  index DDL paths becomes a structured `DdlOpState::Failed { detail }` (or
  `SucceededViaCrashRecovery`) record queryable by `op_id`, instead of a
  human-readable string the client must pattern-match.
- The **silent-completion-after-recovery gap** — ops finished by
  `recover_hash_renames`/`recover_index2_drops`/`recover_in_progress_*` become
  visible to the client/operator that issued them, instead of vanishing.

---

## 6. Open questions for review

1. **Op-id minting authority.** Server-minted (this RFC's assumption) vs.
   client-supplied correlation id. Server-minted is simpler and matches how
   `migration_id` is minted (`admin_migration.rs`), but a client-supplied id
   would let a client pre-log what it is about to do. Recommend server-minted;
   flag for reviewer.
2. **New `DbRequest` enum arm vs. a query-version gate.** A new
   `DbRequest::GetDdlOpStatus` arm does not transparently decode on an old server
   (§3.6). Options: (i) accept it and rely on `not_supported` fallback; (ii) gate
   behind `CURRENT_QUERY_LANG_VERSION` bump (`db_message.rs:21`) with the
   established `server_query_version` negotiation (`db_message.rs:17-25`).
   Recommend (ii) for cleanliness — it is exactly what the v2 mechanism was built
   for — but confirm with reviewer.
3. **Op-status log retention.** TTL vs. bounded-count FIFO vs. "survive until
   first successful poll." Ship-with-cap-and-FIFO is recommended for the first
   slice; tune after measuring operator usage.
4. **`CREATE INDEX` Building-state ownership split** between #966 self-heal and
   the proposed `InProgress`/`SucceededViaCrashRecovery` states
   (`recover_hash_renames` doc, `table_manager_index_mgmt.rs:1039-1055`).
   Deliberately deferred out of the first slice.
5. **Should `SucceededViaCrashRecovery` carry a `restart_epoch`/timestamp** so an
   operator can correlate *which* restart finished it? Lean yes; confirm scope.

---

### Appendix A — primary sources read for this RFC

- Wire: `crates/shamir-query-types/src/batch/batch_response.rs:29-65`;
  `crates/shamir-query-types/src/read/query_result.rs:118-178`;
  `crates/shamir-query-types/src/wire/db_message.rs:32-403`;
  `crates/shamir-query-types/src/batch/batch_op.rs:41-173`.
- Batch loop + failure propagation:
  `crates/shamir-engine/src/query/batch/batch_execute.rs:59-396` (the `?` at
  line 390 aborts the whole batch on a single op failure);
  `crates/shamir-db/src/shamir_db/execute/db_execute.rs:29-105`;
  `crates/shamir-server/src/db_handler/handler.rs:599-604` (the
  `Err(e) => DbResponse::Error { code, message }` convergence point).
- Recovery: `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:831-1186`
  (`drop_index2`, `recover_index2_drops`, `recover_hash_renames`),
  `1393-1574` (rename regular/unique); wired at
  `crates/shamir-engine/src/table/table_manager.rs:500-512`;
  `crates/shamir-index/src/base_index/index_manager.rs:508-857`;
  `crates/shamir-index/src/base_index/sorted_index_manager.rs:642-1307`;
  `crates/shamir-index/src/persistence.rs:295`.
- DDL handlers: `crates/shamir-db/src/shamir_db/execute/` (full directory
  listing + `admin_dispatch.rs:1-90+`).
- Migration poll precedent: `crates/shamir-db/src/shamir_db/execute/admin_migration.rs`;
  `crates/shamir-query-types/src/admin/types/migration_ops.rs:14-48`.
- SDKs: `crates/shamir-client/src/client.rs:770-808`,
  `crates/shamir-client/src/error.rs:24-25`,
  `crates/shamir-query-builder/src/batch/batch.rs:47-63`;
  `crates/shamir-client-ts/src/core/client.ts:602-648`,
  `crates/shamir-client-ts/src/core/types/batch.ts:211-314`,
  `crates/shamir-client-ts/src/core/errors.ts:60-103`,
  `crates/shamir-client-ts/src/core/builders/ddl.ts`.
