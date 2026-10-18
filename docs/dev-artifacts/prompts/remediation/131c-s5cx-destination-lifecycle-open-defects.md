# 131c — PR8b-3 destination lifecycle: open design defects (after review round 3)

STATUS: the design in `131`, `131a`, `131b` passed two consistency/code-fact
auditors (round 3: both `CLEAN`) but an independent reviewer found the
defects below. They MUST be resolved (round 4) before any PR8b-3x
implementation starts. Each defect names the lines of the documents it
touches. This file is deleted when they are resolved.

## D1 (P1) The database-gate reacquisition admits a deadlock

`131:272` releases the shared database gate and reacquires it exclusively
WHILE keeping the repository admission mutexes; `131b:85` acquires the
database gate first and then the repository mutex. Cycle: destruction A keeps
repository mutex R and released the shared gate; publisher B takes the gate
shared and waits for R; A requests the gate exclusive and waits for B. The
per-operation token does not break it (B is another operation). This
contradicts "single global order, never crossed in reverse" (`131:256`).
Needed: a coherent strategy — e.g. a destruction that may touch a database
gate takes it EXCLUSIVE up front (never upgrades), or the repository mutexes
are released before the exclusive request and revalidated afterwards; prove
the chosen order by listing every acquisition site.

## D2 (P1) Audit reconciliation uses in-memory presence as durable evidence

`131:171` prescribes scanning `AuditChain::snapshot` after an ambiguous append
and adopting the matching entry's sequence. The chain stores the entry in
memory BEFORE the appender call (`audit_chain.rs:212-215`, `:417-427`),
`snapshot` is a test helper (`:226-228`), `verify_against_checkpoint` reads no
durable log (`:267-291`) and a restart starts with an empty entry vector
(`:170-177`). A memory match may exist although persistence failed, and after a
restart the previous tail is not in memory. Needed: reconciliation by reading
the DURABLE log tail (or a durable acknowledgement record) and a required
fsync on the found-entry branch.

## D3 (P1) Stage-2 R1 can re-reserve an already destroyed destination

`131:234` upserts an Unresolved incident for EVERY bound destination ALWAYS,
including absent ones, and `131:237` keeps incident-derived reservations while
incidents are unresolved; destruction completion (`131:199`) defines no
incident-resolution transition; `131b:154` needs no reservation for
absent-target install, `131b:123-128` forbids publication of a new incarnation
under a reserved name, and the clear predicate (`131b:209`) needs an already
installed replacement incarnation. So destroy -> R1 -> fresh install can
re-create unresolved protection AFTER destruction and then refuse the install.
Needed: an explicit `Resolved(by destruction)` transition at destruction
completion, R1 never upserting an incident for a destination that is absent
with a completed destruction operation, and consistency with evidence case 15
(names released after R1).

## D4 (P1) The frozen-target confirmation has no client/server exchange

`131b:47-55` makes the HMAC cover the frozen `{db, repo, storage_object_id}`
list from the Prepared operation, but the wire modifier only carries reason,
operation id and nonce (`131b:17`, A.6 `:234`) and the builder supplies no
targets; the server verification seam (`admin.rs:636-659`) has no Prepared
operation. Needed: a preview/challenge exchange (server returns the frozen
target set and a challenge; the client signs it) or sign the logical targets
and have the executor verify the frozen set against a server-side resolved
digest recorded in the operation; specify ordering, canonical form, and
failure.

## D5 (P2) The stage-1 R1 failure guarantee repeats the commit/flush mistake

`131:230` says any failure before the deletion flush leaves the anchor intact,
but the catalogue deletion path commits an implicit transaction before the
later flush (`admin_replication.rs:688-702`; `repo_instance.rs:1295-1297`), as
`131:198` itself acknowledges. A flush failure can therefore leave the row
already deleted, and retries that require the row are wrong. Needed:
idempotent retry that tolerates the row being already gone.

## D6 (P2) A live-process retry after a Prepared-barrier failure is undefined

A barrier failure leaves the operation `Prepared` and the caller is told to
retry the same id (`131:168`, `131b:75`), but the executor is spawned only after
Prepare (`131:297`), a same-id submission returns status (`131:209,299`) and
`destruction_retry` resumes `Blocked`, not `Prepared` (`131b:245`). The
documented remedy can return a stranded Prepared status forever until a
restart. Needed: same-id submission of a `Prepared` operation re-runs the
barrier and spawns the executor (or `destruction_retry` accepts `Prepared`);
define the ownership handoff when the future is cancelled between Prepare and
the spawn.
