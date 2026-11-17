# Brief: design of operational views and query cancellation (release-1 R4b of the observability direction, design only)

## Why

A release gate reads: "the operator sees query delays, data and WAL size, the state of backups and replication in
`/metrics` and can find and cancel a stuck query" (`docs/dev-artifacts/release-1/README.md` section 5). The metrics half is
being implemented (`docs/dev-artifacts/design/engine-metrics.md`). Finding and cancelling a query has no design yet. The
direction is described in `docs/dev-artifacts/release-1/04-observability.md` sections 4, 6 and 7. This task produces the
DESIGN only.

Note on names: in this repository "R4a / R4b / R4c" also label stages of the engine crate split
(`docs/dev-artifacts/design/engine-layering.md`). This brief is about the observability direction only; call the document
and its items "operational views", never "R4b" alone.

## What

Write `docs/dev-artifacts/design/operational-views.md` (English, the style of `format-stamp.md`: numbered sections, a file
and line reference for every claim about the current code, OBSERVED and PROPOSED kept apart). It must:

1. **Map what exists (OBSERVED):** how a request travels from the transports to execution (sessions, the interactive
   transaction registry, batch execution, cursors, subscriptions, stored-procedure calls), which identifiers already exist
   for a session, a request, a transaction and a cursor, the existing execution budgets and deadlines and how they stop
   work today, the slow-query log, the lock and pinned-snapshot holders, the admin dispatch and its permission check, and
   what `/metrics`, `/info` and the audit log already expose about running work.
2. **Activity view:** an administrative operation returning current sessions, requests and transactions — actor,
   database, kind of operation, start time, duration, state (running, waiting for a lock, waiting for confirmation). State
   exactly which registry holds each fact, the cost of maintaining it on the hot path (the code ideology forbids locks and
   O(N) work per operation: lock-free registration, bounded memory), and the privacy rule: no values from queries appear in
   the output. Paging and bounds for large servers.
3. **Cancellation:** an administrative operation that cancels a request or a transaction by identifier. Cooperative: name
   every point where the flag is checked (reuse the existing budget and deadline checks where they exist), the error the
   caller receives, and the proof obligations: a cancelled request releases its pinned snapshot, its locks and its
   transaction slot; a cancelled interactive transaction rolls back; cancellation of a commit in its durable phase is
   refused or deferred (decide, with the reason). Behaviour on a follower and for replication-apply work.
4. **Statistics by query shape:** accumulated counters and total time per "shape" — builder operations without literals.
   Define the shape canonically (reuse an existing canonical encoding if one fits; say which), the bounded table with
   eviction, the reset operation, and the hot-path cost.
5. **Surfaces:** the admin operations and DTOs (typed, through the builders), permissions, the client methods, what the
   future administrative shell will call, and the metrics that accompany these (counts of active requests, cancellations).
6. **Tests and acceptance** (CI only): each property above as a named test, including the release-after-cancel proofs
   under concurrency, and the three OS.
7. **Implementation plan:** at most four PR-sized steps with the crate and layer of each change. The engine is being cut
   into crates: nothing is added to the `repo`, `table`, `tx` or `query` layers without listing it and its layer; say for
   each step whether it must land before or after the transaction layer moves into its own crate.
8. **Open questions** that need the owner, each with a default (answer the questions of `04-observability.md` section 7
   that concern views and cancellation).

## Do not

Change any code, test or CI file; design tracing or new metrics beyond the ones named in item 5; add a dependency (if an
option needs one, write it as an owner decision with the alternative that needs none).

## Observable acceptance

One new file `docs/dev-artifacts/design/operational-views.md`; every statement about the current code carries a path (and
a line where it matters); the open questions have defaults. `python -I scripts/ci/check_layout.py` OK. The final report
lists the sections, the decisions taken and the open questions.
