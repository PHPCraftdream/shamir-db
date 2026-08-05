# Brief — #985: RFC for a unified DDL result contract (operation ID + status)

Task: #985 in the session TaskList. **Deliverable is a design document, NOT
code.** Do not touch any wire type, handler, or SDK in this pass — write the
RFC to `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md`
(date-prefixed, matching this repo's existing research-doc naming
convention — check `docs/dev-artifacts/research/` for examples before
naming yours).

## Context already established this session — do not re-derive, build on it

**Current wire shape** (read these files first):
- `crates/shamir-query-types/src/batch/batch_response.rs` — `BatchResponse`:
  `{ id, results: TMap<alias, QueryResult>, execution_plan, edge_provenance,
  execution_time_us, transaction, interner_delta }`. No per-alias
  success/failure signal beyond "the alias key exists in `results`".
- `crates/shamir-query-types/src/read/query_result.rs` — `QueryResult`:
  `{ records, stats, pagination, value, explain, skipped }`. **No `op_id`,
  no `status` field exists for ANY op today, DDL or otherwise.** A DDL op
  that fails does not populate `results[alias]` at all — the whole batch
  fails via a top-level `DbResponse::Error` (check
  `crates/shamir-query-types/src/wire/db_message.rs`'s `DbResponse` enum to
  confirm exactly how a mid-batch DDL failure currently propagates — does it
  abort sibling aliases already computed, or return what succeeded so far?
  Read the actual batch-execution loop in `shamir-engine`/`shamir-db` to
  answer this precisely, don't assume).
- **#967's existing partial mitigation**: every CREATE/RENAME/DROP INDEX
  site that can fail AFTER an earlier phase already durably persisted state
  returns an enriched *error message string* explaining what was persisted
  and pointing at `TableManager::verify()`/`repair()`. This is the "parse
  error TEXT" workaround the RFC exists to replace with a structured
  contract.
- **The tombstone-recovery mechanisms already built** (#959 base_index DROP,
  #961/#988 index2 RENAME/DROP, #962 sorted RENAME, #972 sorted/index2 DROP,
  #997 regular/unique RENAME): every one of these can now SILENTLY finish an
  interrupted DDL op on the NEXT server restart, with no client ever
  learning the op eventually completed (or what it did). A durable
  "operation ID + status" contract is the natural place to surface this:
  read `TableManager::recover_hash_renames` (#997, most recently added) and
  `recover_index2_drops` (#988) for the exact shape of "what recovery does
  and when" — your RFC's answer to "what status does a since-recovered op
  report" must be grounded in how these ACTUALLY behave, not a hypothetical.

## Required RFC content

### 1. Explicit design-question answer (do not hedge)

**Should "operation ID + status" be:**
(a) a NEW poll endpoint (`GetDdlOpStatus { op_id } -> DdlOpStatus`, queried
separately from the synchronous DDL response), or
(b) fields embedded directly in every synchronous DDL `QueryResult` (e.g.
`op_id: RecordId, status: DdlOpStatus`)?

Argue this out concretely against THIS codebase's actual constraints — do
not produce a generic pros/cons list. Specifically address:
- Every DDL call today is synchronous request/response (the client awaits
  the `BatchResponse`) — a durable op record only becomes INTERESTING after
  a crash+restart, i.e. after the original request/response pair is long
  gone. A poll endpoint queried later is the only way a client can ever
  learn "did my CREATE INDEX from an hour ago, before the server crashed,
  actually finish?" — a field on the ORIGINAL synchronous response cannot
  carry information from AFTER a crash that happens after that response was
  already sent. Reason through whether this asymmetry alone settles the
  question, or whether there's still a role for embedding `op_id` in the
  synchronous response (e.g. so the client has something to poll BY, even
  if the poll mechanism itself is separate).
- Where would durable op-status records live? The existing tombstones
  (`idx_drop`/`uidx_drop`/`sidx_drop`/`idx_ren`/`uidx_ren`/`sidx_ren`/index2
  equivalents) are keyed by NAME (or name-pair for renames), cleared on
  success — they are not designed to be QUERIED by a client after the fact,
  they're an internal recovery mechanism. Does a client-facing "status by
  op_id" contract require a NEW, separate durable log (append-only, with
  its own retention/GC policy), or can it be layered over the existing
  tombstones with additions? Reason through the tension: tombstones are
  cleared on success (so a successful op leaves no trace to poll), but a
  client-facing status contract needs to answer "yes, this succeeded" for
  a WHILE after completion, not just "still in progress" or "still
  recovering."

### 2. Interaction with #997/#988/#972's crash recovery — answer explicitly

For an op that crashed mid-flight and was silently finished by recovery on
the NEXT restart (client never sent a new request, never got a new
response): what does polling that op's status report?
- Before the crash: client sent the request, was waiting (or the connection
  dropped before a response arrived — TCP behavior on a server crash).
- After recovery: the op IS done. If the client (or a NEW client instance,
  or an operator's monitoring tool) polls by `op_id`, what should it see?
  You need a design that makes this coherent — e.g. does recovery need to
  itself write a "completed via crash-recovery" status entry using the SAME
  `op_id` the tombstone carried, so a later poll finds it? Trace this through
  concretely for at least one family (recommend: #997's unique RENAME
  SEVERE case, the most severe recovery scenario this session characterized
  in depth) as a worked example in the RFC.

### 3. Blast radius — enumerate concretely, do not hand-wave

- New response fields/shapes in `shamir-query-types` (name them).
- Every DDL handler in `crates/shamir-db/src/shamir_db/execute/` (list the
  files: `admin_table_index.rs`, `admin_db_repo.rs`, `admin_function.rs`,
  `admin_validator.rs`, `admin_schema.rs` — verify this list against the
  directory yourself, don't trust it blindly) — what changes in each.
- Both SDKs: `shamir-query-builder` (Rust) and `shamir-client-ts`. What new
  builder/method surfaces would a client use to poll?
- Backward compatibility: existing `BatchResponse`/`QueryResult` msgpack
  shapes must keep decoding for old peers. Any new field must be additive
  (`#[serde(default, skip_serializing_if = ...)]`, matching this repo's
  existing convention throughout `query_result.rs`/`batch_response.rs`).

### 4. Recommended scope for a FIRST implementation slice

Given the size, recommend what a first PR should cover vs. defer (e.g.
"land the poll endpoint + op_id assignment for the hash-family rename/drop
ops first, since those already have the tombstone infrastructure #997 built
this session; defer sorted/index2 wiring to a follow-up" — or argue for a
different first slice if you find one more sensible). This section exists so
the RFC produces an actionable next task, not just analysis.

## Scope discipline

- Do NOT write or modify any `.rs`/`.ts` source file in this pass.
- Do NOT invent wire types as final — this is a PROPOSAL for review, mark it
  clearly as such (e.g. a "Status: DRAFT — pending review" header).
- Ground every claim about existing behavior in an actual file you read
  this session — cite file paths and line ranges, do not describe behavior
  from memory/assumption.

## What to report back

- Confirm the RFC file was written to
  `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md` (or
  explain if you chose a different, better-justified path/name).
- Summarize your answer to the poll-vs-embedded design question in 3-5
  sentences.
- Summarize your answer to the crash-recovery interaction question in 3-5
  sentences.
- List the recommended first-implementation-slice scope.
