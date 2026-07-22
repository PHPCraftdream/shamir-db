# FG-4: `docs/guide-docs/KNOWN_LIMITATIONS.md` — public limitations document

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## DECIDED (orchestrator, do not re-litigate)

- **File path**: `docs/guide-docs/KNOWN_LIMITATIONS.md`. `docs/guide-docs/`
  has no existing top-level file (only subdirectories:
  `architecture/`, `client-server-protocol-spec/`, `guide/`, `security/`) —
  this is a new top-level doc, not a subdirectory addition.
- **Language: English-primary**, matching its closest siblings-in-spirit
  (`client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md`,
  `client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md` — both landed this
  campaign, both pure-EN reference docs). `guide/*.md` is RU-primary
  conversational content; this doc is a reference/compatibility list, not a
  walkthrough — do NOT make it bilingual, do NOT split into RU/EN copies.
- **This is a DOCUMENTATION-ONLY task.** No source code changes except the
  two cross-reference additions specified below (README.md, CHANGELOG.md).

## Purpose

A single, honest, citation-backed list of S.H.A.M.I.R.'s current
architectural limitations — alpha-roadmap item #8 (2026-07-21 review). Every
bullet MUST carry a `path/to/file.rs:LINE` citation that you have personally
verified against the CURRENT tree (not copied from an old review) before
writing it down. **If a citation below has drifted (line moved, behavior
changed, or the claim is simply no longer true), do NOT write the stale
claim — re-verify against the actual current code and either correct the
citation or drop the bullet with a one-line note in your final report.**
Do not invent a limitation that isn't independently confirmed by reading
the actual source.

## Sections and starting-point citations (VERIFY EACH BEFORE WRITING)

### 1. Transactions

- One repository per transaction — cross-repo transactions are rejected.
  Find and cite the actual guard/error site.
- No savepoints / nested transactions.
- A WASM function `Call` inside an open transactional batch is rejected —
  a deliberate atomicity guard. Find the actual guard in
  `crates/shamir-engine/src/query/batch/query_runner.rs` (grep for how
  `Call`/WASM ops are validated against an open tx — the exact function/line
  may have moved since the original 2026-07-21 review; verify, don't assume
  the old line number).
- Transactional DDL (create table/index inside a tx) is not documented as
  supported — confirm current behavior and cite it, or state plainly if
  DDL-in-tx has no explicit guard either way.
- **`expected_version` (FG-2, this campaign) CAS isolation caveat**: the
  commit-time SSI race-window backstop only fires under `Serializable`
  isolation. A plain non-transactional `expected_version` write (which runs
  under the hardcoded-`Snapshot` implicit-tx path) gets only the immediate
  stale-read check — two concurrent non-transactional writers with the same
  `expected_version` can both succeed. Cite
  `docs/guide-docs/client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md`'s
  own "⚠️ Isolation caveat" section (already written, just cross-reference
  it here rather than re-deriving) and the underlying test
  `crates/shamir-server/tests/version_cas_e2e.rs`.
- **Read-your-own-writes (FG-3, this campaign, JUST LANDED)**: streaming
  scans (`list_stream_tx`/`filter_stream_tx`) and `execute_update_tx`/
  `execute_delete_tx`'s match-scans now overlay a tx's own staged
  `write_set` — describe the NEW behavior (a staged insert/update/delete is
  visible/hidden correctly to the SAME tx's own reads), not the old removed
  limitation. Cite `crates/shamir-engine/src/table/tx_scan_overlay.rs` and
  `crates/shamir-engine/src/table/table_manager_streaming.rs`'s updated doc
  comments (read them — they already describe the current behavior
  accurately, written this campaign).
  - **Residual limitation, still real, must be documented**: no SSI
    predicate/range locking over streams — a concurrent OTHER transaction's
    phantom insert into a range this tx is scanning is NOT detected. This is
    a SEPARATE, harder problem than RYOW and remains out of scope. Cite the
    "Streaming-scan SSI scope" doc comment in `table_manager_streaming.rs`
    (same file, just above the RYOW doc comment).
  - `AsOf`/`History` temporal reads (`read_temporal.rs`) do NOT get tx
    overlay — point-in-time historical views are exempt from RYOW by
    design (FG-3's own "Out of scope" section). Cite `read_temporal.rs`.

### 2. Schemas

- `defaults`/`auto_now` apply to top-level fields only (verify current line
  in `admin_schema.rs`, cite it — the review cited `:164` but re-verify).
- `unique` constraint: single field only (no composite unique).
- `foreign_key`: single field, target table must be in the same repo (no
  cross-repo FK) — verify current behavior in `validator_db.rs` (which you
  or the FG-3 work may have just touched — re-read it fresh) and
  `table_manager_validators.rs`.
- No composite FK, no deferred constraints, no self-referential cascade.
- Renaming a table that has a bound schema is rejected — verify current
  behavior/line in `rename_table_e2e.rs` (review cited `:139`, re-verify).
- The "migration" API changes the storage engine, NOT schema evolution —
  find and cite the actual migration entry point/doc that supports this
  framing.

### 3. Indexes

- `unique` and `sorted` index flags are mutually exclusive on the same
  index — verify and cite.
- One vector index per table — cite `docs/guide-docs/guide/06-search.md`'s
  relevant section (search for `staged_vectors` or the vector-index-per-
  table constraint).
- No partial indexes, no TTL indexes, no geo indexes.

### 4. Subscriptions

- Best-effort delivery; a supported subset of filter shapes only — cite
  `docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md`'s relevant
  section (the review cited "§7" — verify the section still covers this).
- No durable offsets / resume tokens.
- A slow consumer can experience a gap (describe current fanout/backpressure
  behavior briefly, cite the subscription bridge/fanout code).

### 5. Replication

- Experimental, pull-based, read-only follower — align wording with RI-10
  (this campaign): `docs/guide-docs/guide/08-interconnect.md`'s existing
  "(Experimental)" label and limitations paragraph (already landed, just
  cross-reference, don't re-derive).
- Journal-gap behavior: **after RI-10**, a gap is now a terminal
  `JournalGap` error handled via
  `ShamirDb::mark_subscription_resync_required` (cite
  `crates/shamir-db/src/shamir_db/execute/admin_replication.rs` and
  `crates/shamir-server/src/replication/error.rs`) — describe the CURRENT
  (post-RI-10) behavior, not the old silent-skip behavior.

### 6. Results

- Query results materialize fully into a `Vec` (no true server-side
  streaming to the client yet) — cite the relevant `QueryResult`/response
  assembly site.
- Result-size cap: **after RI-8 (this campaign)**, cite the CURRENT default
  values in `crates/shamir-server/src/config.rs`
  (`default_max_result_size_bytes`, `default_max_active_connections`) — the
  review's numbers are stale (RI-8 changed 1GiB→64MiB and 10000→1000), use
  the actual current values you read from the file.
- No server-side cursors yet — this is task #747 (FG-5), explicitly
  deferred post-alpha. Link to it as a roadmap item (reference the task by
  its description, not an internal task-tracker id the reader can't see —
  phrase it as "planned, see roadmap" or similar, since task IDs are
  internal to this session's tracker, not a public artifact).
- No global inflight response-memory budget across concurrent connections
  yet — also deferred post-alpha (task #749/RI-15), same phrasing note as
  above.

### 7. Numbers

- **u64 → Big promotion contract (FG-1, this campaign)**: a `u64` value
  `> i64::MAX` promotes losslessly to `Value::Big`/`QueryValue::Big`
  instead of silently wrapping or clamping. Cross-reference
  `docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md`
  (already written, just cite it) rather than re-deriving the full contract
  here.
  - **Known residual gap, must be documented**: an `Eq` filter (or
    `ORDER BY`) against a promoted `Big` value does not currently match /
    cross-compare correctly — a real structural gap in the filter-eval
    extraction layer (`scalar_at`/`ScalarRef`), tracked as a follow-up (task
    #750/FG-6, not yet fixed as of this doc). Cite
    `NUMERIC_WIRE_SEMANTICS.md`'s own description of this gap (it already
    documents it) rather than re-deriving.

### 8. `ttl_ms`

- `ttl_ms` on the in-memory buffer (`storage_membuffer.rs`) governs how long
  data stays in the RAM buffer before flushing to the durable backend — it
  is NOT a data-expiration/TTL-eviction feature (no automatic deletion of
  "expired" records). Verify and cite the exact doc comment / field in
  `storage_membuffer.rs`.

## Cross-references (small, additive edits)

- `README.md`: add ONE short link/sentence pointing to
  `docs/guide-docs/KNOWN_LIMITATIONS.md` — near the honest-positioning
  section RI-12 added this campaign (search for it), NOT a new section.
- `CHANGELOG.md`: one `[Unreleased]` bullet noting the new doc exists (not a
  code change, a docs bullet — keep it one line).

## Verification (MANDATORY before you report done)

- Every bullet in the doc has a citation YOU personally verified by reading
  the cited file/line in the CURRENT tree (not trusting this brief's
  starting-point citations blindly — some are flagged above as
  "re-verify", but verify ALL of them, including ones not explicitly
  flagged).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean (should be a no-op since this is docs-only, but
  confirm nothing else in the tree was accidentally touched).
- `./scripts/test.sh @oracle --full` green (confirms no accidental code
  drift from a docs-only change).
- Report the full list of citations you verified, and explicitly flag any
  bullet from this brief's starting list that you found to be stale,
  inaccurate, or already superseded — do not silently "fix" the brief's
  wording without saying so in your report.

If, after real investigation, some cited behavior turns out to not exist or
to be structurally different from how this brief describes it — do not
force a bullet that isn't true. Report the discrepancy precisely (mirroring
this campaign's established "STOP and report" discipline for FG-1/FG-2/
FG-3) rather than papering over it.
