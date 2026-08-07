# Brief — P1-5 (#1018): design RFC for online CREATE INDEX (snapshot → CDC → catch-up → short barrier)

## Context

S.H.A.M.I.R. Database. Source: review 2026-08-05 §P1-5. Documented in
full at `docs/guide-docs/KNOWN_LIMITATIONS.md` (search "CREATE INDEX
blocks all writers for the ENTIRE backfill scan"). Bench proving the
severity: `crates/shamir-engine/benches/f78_writer_latency.rs` — at
100k rows the backfill takes ~140-160s and every concurrent writer
queues for the same duration (a multi-minute write OUTAGE, not a brief
pause); the scan is superlinear, so 1M rows extrapolates to hours.

**The user has explicitly chosen the full architectural fix over the
cheaper alpha-limits workaround** — this is accepted as a genuinely
large, multi-round engineering effort, not something to rush into one
shot. **This first pass produces a design RFC, not an implementation.**
Do not write production code in this pass — that comes in follow-up
tasks once the design is reviewed. This mirrors how
`docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md` was
written and reviewed BEFORE #1015's implementation landed (read that RFC
for the expected depth/rigor/citation style — file+line grounded, not
hand-wavy).

## The target architecture (from the task's own framing — validate, refine, don't reinvent)

1. **Snapshot scan** — read the table's current state at some consistent
   point, WITHOUT holding the write barrier for the whole scan (the
   current `create_index_from_stream` design's core flaw).
2. **Durable delta capture** — while the snapshot scan runs, concurrent
   writes to the table must be captured somewhere durable (a "shadow
   log" of mutations affecting the index-in-progress) so they aren't
   lost.
3. **Catch-up** — after the snapshot scan finishes, apply the captured
   deltas to the partially-built index, bringing it current.
4. **Short publish barrier** — only for the brief final step (flip the
   index from `Building` to `Ready`, ensuring no writer observes a
   half-caught-up index), hold the write barrier — this should be
   milliseconds, not minutes.
5. **Progress must be queryable, not just log-visible** — the task
   explicitly calls this out; today's `log::info!` progress lines are
   not queryable by an operator/client. Consider whether this connects
   to #1015's new `DdlOpStatus`/op-status-log machinery (just landed
   this session, `crates/shamir-engine/src/table/ddl_op_log.rs`) —
   `CREATE INDEX` was explicitly OUT of scope for #1015's first slice
   (RFC §4 deferred it precisely because of the ownership-split question
   with self-heal, see `table_manager_index_mgmt.rs:1039-1055`'s doc
   comment) — investigate whether this task is the natural place to
   finally wire `CREATE INDEX` into that status log, or whether it's
   still better kept separate. State your conclusion either way.

## What to investigate and ground the RFC in (file+line citations required, same rigor as the DDL RFC)

- **Current barrier mechanism**: `begin_write_barrier` / `F-70`'s design
  — read `create_index_from_stream`'s doc comment in
  `crates/shamir-index/src/base_index/index_manager.rs` (cited by
  KNOWN_LIMITATIONS.md) in full, plus wherever `begin_write_barrier` /
  `drain_writers` / `unique_write_lock` are actually defined and used
  across the CREATE INDEX path (`table_manager_index_mgmt.rs` and
  friends). Understand exactly what invariant the barrier currently
  protects, precisely, before proposing to shrink its scope.
- **Existing MVCC/versioning infrastructure this could piggyback on** —
  this session's own #1011/#1037/#1038 work (`ReaderDrainGate`,
  `docs/dev-artifacts/research/2026-08-06-p0-3a-reader-drain-gate-plan.md`)
  built exactly this kind of "readers/writers don't fully serialize on a
  coarse lock" mechanism for a DIFFERENT problem (DROP-vs-in-flight-reader
  races) — read it for prior art and precedent-setting decisions in this
  codebase, even though it's not solving the same problem, the design
  taste/conventions should be consistent.
- **The tombstone/recovery machinery** from #1015's RFC (crash-recovery
  tombstones, `recover_hash_renames`/`recover_index2_drops` et al.) —
  a crash mid-snapshot-scan or mid-catch-up needs a coherent recovery
  story; investigate whether the existing `Building`-state +
  `doctor::verify()`/`repair()` self-heal (#966, wired to a CLI in
  #1014 this session) is sufficient, or whether the new architecture
  needs its OWN recovery primitive (e.g., can a half-caught-up index
  safely resume catch-up from where it left off, or does a crash during
  catch-up require restarting the whole snapshot+catchup cycle — if the
  latter, is that acceptable given `docs/dev-artifacts/research/
  f50-step3-crash-restart-spike.md`'s existing "resume-from-checkpoint
  rejected as over-engineering" precedent, or does THIS task's much
  larger backfill duration change that calculus?).
- **`f78_writer_latency.rs`** bench — understand exactly what it measures
  today so the RFC can propose what the SAME bench should measure after
  the redesign (e.g., "writer p99 latency during CREATE INDEX should be
  bounded by the catch-up apply rate, not the full scan duration").
- **The unique-family O(table) memory issue** (mentioned in
  KNOWN_LIMITATIONS.md, `create_unique_index_body`'s F-78 deferral
  comment) — this task's #1044 (already tracked, PERF backlog,
  blockedBy #1040) may or may not be the same problem; note the
  relationship in the RFC (are they solved by the same redesign, or
  genuinely independent?) without necessarily solving #1044 here.

## RFC deliverable — required sections

Write `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`
(today's date; adjust if it differs when you actually write it) covering
AT MINIMUM:

1. **Problem statement** (can largely restate KNOWN_LIMITATIONS.md's
   framing, grounded with your own citations).
2. **Design** — the concrete mechanism for each of the 4 phases above:
   what exactly changes at the code level (which functions, which new
   data structures), how the delta-capture log is keyed/scoped/GC'd,
   how catch-up decides it's "close enough" to flip to the short
   publish barrier (a moving target — writes keep arriving during
   catch-up; there needs to be a convergence criterion), and exactly
   what the short publish barrier protects.
3. **Concurrency/correctness argument** — why this is safe: no writer
   can observe a partially-built index as `Ready`, no committed write
   during the snapshot+catchup window is lost, no crash leaves an
   unrecoverable half-state (or if one can, say so and propose the
   mitigation/acceptance).
4. **Crash recovery story** — building on the existing tombstone/
   `doctor` machinery where possible, extending it where necessary.
5. **Rollout / implementation slicing** — this is NOT a one-PR feature.
   Propose a concrete phased breakdown (mirroring how #1015's RFC §4
   scoped a "first implementation slice" vs. explicit deferrals) —
   e.g., slice 1 might be just the regular/hash family for CREATE
   (not unique, not sorted), slice 2 extends to unique, etc. Be
   explicit and honest about what's deferred and why, exactly like the
   DDL RFC was.
6. **Open questions for review** — anything genuinely undecided that
   needs a human call before implementation starts (mirror the DDL
   RFC's §6 style).
7. **Bench/test plan** — what needs to prove the new design actually
   shrinks the writer-stall window (extending `f78_writer_latency.rs` or
   a new bench), and what crash-recovery tests are needed.

## Constraints

- **This pass writes a document, not code.** Do not touch
  `crates/shamir-index`, `crates/shamir-engine`, or any other production
  crate in this pass.
- Follow this repo's existing RFC citation discipline: every claim about
  *existing* code behavior needs a file+line grounding, exactly like
  `2026-08-05-ddl-result-contract-rfc.md`'s own preamble states as its
  rule ("every claim about existing behavior is grounded in a file +
  line range read for this RFC").
- No gate commands apply to this pass (no code changed) — but do a final
  sanity pass confirming every file+line citation in your RFC is
  accurate (re-open the cited file, confirm the line range still says
  what you claim) before finishing.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files (in this case, only create the one new RFC file); the
orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`
      written, covering all 7 required sections above.
- [ ] Every claim about existing code behavior is file+line grounded and
      verified accurate.
- [ ] A concrete, honest implementation-slicing proposal exists (this is
      what follow-up tasks will be created from).
- [ ] No production code touched in this pass.
