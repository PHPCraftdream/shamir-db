Task: LOW-MEDIUM compliance — write
`docs/security/data-protection.md` documenting the at-rest encryption
model and the PII retention/erasure procedure (audit finding C6,
`docs/audits/2026-07-06-security-compliance-supplychain.md`). This is
a DOCUMENTATION task — no code/behavior changes.

## Context (read before writing)

Records are stored as MessagePack on disk (field names interned to
`u64` ids for compression — see the project's own `CLAUDE.md`
description). Per the audit, NO document currently exists describing:
1. **At-rest encryption model.** Transport is TLS-protected (rustls),
   but the audit found no documentation of whether/how data AT REST
   (on disk) is protected. For a database handling PII, compliance
   regimes (GDPR Art.32, SOC2) require either at-rest encryption OR an
   explicit, documented decision that encryption is delegated to the
   OS/filesystem/volume layer (e.g. LUKS, BitLocker, cloud-provider
   disk encryption) — silence on this point is itself the compliance
   gap, not necessarily the absence of encryption.
2. **Retention/erasure procedure for PII (GDPR Art.17 "right to
   erasure").** The codebase has `SetRetention`/`PurgeHistory`
   mechanisms (find and read their actual current implementation/API
   before writing about them — do not describe them from memory or
   guess; locate them via grep, e.g. `SetRetention`, `PurgeHistory`,
   `purge_below_ts`, retention-policy config), but no document connects
   these primitives to an actual "how do I make a specific data
   subject's data unrecoverable" procedure. The complication the audit
   specifically flags: the interner maps field NAMES to `u64` ids, and
   those NAME→id mappings **survive** even after the records that used
   them are purged (the interner isn't purged per-record) — so "was
   this person's data referenced" can leave a very faint residual trace
   (an interned field name existing, even with zero records using it)
   even after full erasure of their actual record content. This nuance
   must be documented honestly, not glossed over.

## What to write

Create `docs/security/data-protection.md` (check if a `docs/security/`
directory already exists; create it if not) covering:

### 1. At-rest encryption — state the model precisely

- Investigate the ACTUAL current state: does `shamir-storage` (or
  whichever crate owns the physical storage backend, `fjall`/`lsm-tree`
  per the dependency tree) perform ANY encryption of data at rest, or
  is data written to disk in plaintext MessagePack? Check the storage
  backend's actual code/config (grep for `encrypt`, `cipher`, `aes`,
  etc. in `shamir-storage` and the fjall/lsm-tree dependency
  configuration) to determine the TRUE current state — do not assume
  either answer, verify it.
- Document whichever is actually true:
  - If NO at-rest encryption exists: state this explicitly and
    document the RECOMMENDED mitigation (full-disk encryption at the
    OS/volume layer — LUKS on Linux, BitLocker on Windows, cloud
    provider disk encryption for cloud deployments) as the operator's
    responsibility, matching what many embedded/single-binary database
    products do (SQLite, for example, documents the same delegation
    model by default). Be explicit that this is a DELEGATED model, not
    an absence of a compliance answer.
  - If some encryption already exists (verify first, don't guess):
    document what it covers, what key-management model it uses, and
    what it does NOT cover (if partial).
- Note whether `server-cert.pem`/TLS private keys and other
  secret-bearing files on disk have any special handling (the
  project's own `SECURITY.md`, added by task #483, already notes
  `server-cert.pem` is gitignored — cross-reference that, don't
  duplicate/contradict it).

### 2. PII retention/erasure procedure

- Describe the ACTUAL current `SetRetention`/`PurgeHistory` (or
  whatever the current API surface is called — verify via grep,
  names/signatures may have evolved) mechanisms: what they do, what
  scope they operate at (per-table? per-key? time-based?), and how an
  operator invokes them to satisfy a genuine "forget this data subject"
  request.
- Document the interner caveat explicitly and honestly: explain that
  purging a record's VALUE does not purge the interned FIELD NAMES that
  record referenced (since those are shared, repo-wide, u64-keyed
  mappings used by potentially many other records) — so a full erasure
  removes the person's actual data (values) but a residual signal (that
  some field name like `"ssn"` or `"email"` was ONCE used somewhere in
  the repo) may remain discoverable via the interner even after
  erasure. Assess (based on what field names actually look like in
  this system — are they typically generic schema field names like
  `"email"`/`"created_at"`, or could they ever themselves BE PII, e.g.
  a dynamically-named field containing a person's name?) whether this
  residual signal itself constitutes meaningful PII exposure, and state
  your honest assessment (for a system with a fixed, small, schema-like
  set of field names — which is the typical use case — this residual
  is very unlikely to be independently identifying; state this
  reasoning rather than asserting it without justification).
- Document interaction with WAL and history: does a purge need to wait
  for WAL truncation / history garbage collection to fully take effect
  on disk (i.e., is there a window where purged data is logically gone
  but still physically present in an un-truncated WAL segment or
  un-GC'd history entry)? Check the actual GC/truncation code paths
  (the `vacuum_key`/`gc_overlay_to`/WAL segment-truncation machinery
  already extensively covered by this campaign's earlier A10/A14 fixes)
  to give an accurate answer about the LATENCY between "purge issued"
  and "data physically gone from disk," rather than implying it's
  instantaneous if it isn't.

### 3. Retention defaults

- Document what the DEFAULT retention policy actually is today (per
  the codebase — check `RetentionPolicy`/`CurrentOnly`/whatever the
  default variant is called, referenced elsewhere in this campaign's
  A10 work) — state plainly whether the out-of-the-box behavior
  already leans toward minimal retention (which would be a
  privacy-favorable default worth noting) or toward indefinite
  retention (which would be a gap worth flagging for operators who care
  about GDPR-style minimization).

## What NOT to do

- Do NOT implement actual at-rest encryption as part of this task —
  the audit's own fix ask is to DOCUMENT the model (whichever it
  currently is), not to build encryption. If you determine encryption
  does NOT currently exist, documenting "delegated to OS/volume" is a
  fully acceptable, complete answer to this MEDIUM-severity finding —
  do not scope-creep into implementing transparent data encryption.
- Do NOT change any `SetRetention`/`PurgeHistory` code — this is a
  read-and-document task on the EXISTING mechanisms, not a mechanism
  change.
- Do NOT invent capabilities the codebase doesn't have (e.g. don't
  claim a "right to erasure API endpoint" exists if it's actually just
  an internal/administrative function with no client-facing wire
  protocol — be precise about who can invoke this and how, e.g. is it
  a server-operator-only administrative action, or is it exposed to
  end clients via the wire protocol?).

## Verification requirement (no TDD — this is documentation)

Since this is a pure documentation task:
1. Show the exact grep commands used to verify each factual claim in
   the document (at-rest encryption presence/absence, retention API
   signatures, WAL/purge latency) — do not write the document from
   assumption.
2. Confirm the new file doesn't contradict anything already stated in
   `SECURITY.md` (added by task #483) — cross-reference where relevant
   rather than duplicating.

## Gate (must be clean before finishing)

No code changes are expected, so no fmt/clippy/test gate applies. If
you end up touching ANY `.rs` file for this task, stop — that would
mean the task has drifted out of its documentation-only scope; report
that instead of proceeding.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- What you found regarding at-rest encryption (encrypted/not, and
  exactly how you verified this — grep commands + what they showed).
- The exact current `SetRetention`/`PurgeHistory` (or current
  equivalent) API you documented, with file:line citations.
- Your assessment of the interner-residual-PII question (is it a
  meaningful exposure for this system's typical field-name shapes, or
  not, and why).
- The WAL/history purge-latency finding (instantaneous vs. delayed
  until truncation/GC, with citations).
- The current default retention policy, cited.
- The full content of `docs/security/data-protection.md` (or a summary
  if very long — but the actual file is what matters).
