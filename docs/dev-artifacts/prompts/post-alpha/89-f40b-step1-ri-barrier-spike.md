# Brief for F-40b Step 1 (#855, P2) — RI barrier design spike for explicit-Snapshot FK parent mutations

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is a **timeboxed design spike**, mirroring F-28 Step 3's own
spike-before-implement discipline exactly — **read
`docs/dev-artifacts/research/f28-s3-mechanism-decision.md` in full first**
as the template for this spike's shape, rigor, and deliverable format.

**Read `docs/dev-artifacts/research/f40-explicit-snapshot-ri-gap-memo.md`
in full next** — it already did the investigation and recommended the "RI
barrier" direction (an isolation-independent commit-time re-check),
rejecting a mid-flight isolation-upgrade alternative as unsound. This
spike's job is to SETTLE that memo's two open design questions (§4.4) and
PROVE the mechanism works via a minimal prototype + deterministic race
test — **not** to ship the full production implementation (that's a
separate follow-up step, #856, after this spike lands).

## What to settle

### Open question 1 — token shape

The memo's design sketch proposes a flat `TFxSet<u64>` of `table_token`s
(mirroring `TxContext.footprint_tokens`'s existing shape exactly), checked
via `PredicateDep::TableScan { table_token }` at commit time. The
alternative is storing richer `PredicateDep` values directly (including
`PredicateDep::IndexRange { table_token, index_id, lo, hi }`, which exists
in `crates/shamir-tx/src/predicate_set.rs:26-42`) so an indexed FK scan
records a TIGHTER conflict dep than a full-table-scan, reducing false
aborts.

**Investigate**: do the FK reverse-check scans
(`fk_restrict.rs::child_has_reference`, `fk_actions.rs`'s cascade probes,
`fk_on_update.rs`'s on-update probes) already go through an INDEXED path
(since a FK's referenced field requires a supporting index —
`validate_fk_indexes` in `admin_schema.rs` enforces this at DDL time), or
do they do a raw table scan regardless? If they're already index-covered,
check whether `table_manager_streaming.rs`'s EXISTING `TableScan`-recording
call site (~line 244-248, used by the Serializable predicate_set path
today) could ALSO be made to record an `IndexRange` instead when an index
serves the scan — if the Serializable path itself is already accepting
the coarser `TableScan` dep for these same scans (check this), matching
that same coarseness for the RI barrier is consistent and simpler; if the
Serializable path already gets a tighter dep somewhere you can reuse, the
barrier should too. Decide and document your reasoning — a flat
`TFxSet<u64>` (simpler, matches `footprint_tokens`) is an acceptable
outcome if the investigation shows the existing Serializable path doesn't
already do better.

### Open question 2 — retry/error-code contract for explicit-tx clients

The implicit path wraps its commit in `retry_on_tx_conflict`
(`query_runner.rs:504-522`, a small bounded retry that absorbs an
already-resolved race transparently so it never surfaces as a client
error). The EXPLICIT-tx path has no such wrapper today — the client owns
its own transaction lifecycle and retry decisions.

**Decide**: should an RI-barrier-triggered abort on an explicit tx surface
as the SAME coded `tx_conflict` a generic SSI conflict already produces
(so existing client retry logic that already handles `tx_conflict` picks
it up for free), or as a DISTINCT error code (so a client can tell "the RI
barrier caught a referential-integrity race" apart from "a generic
Serializable conflict")? Investigate how `interactive_tx.rs`'s explicit-tx
commit path surfaces errors today (does it already have ANY coded-error
convention for aborts an explicit tx should consider retryable?) before
deciding. State your reasoning either way — there is no clearly mandated
answer here, this is a genuine design choice this spike must make so
Step 2 doesn't have to guess.

## What to prototype

1. **One recording site** — pick the simplest of `fk_restrict.rs` (the
   RESTRICT-only path is the smallest of the three) and add a
   `TxContext.ri_barrier_tokens` field (whatever shape §1 settles on),
   recorded regardless of isolation level (unlike the existing
   `record_predicate_shared`, which gates on `Serializable`).
2. **One guard-widening site** — `pre_commit.rs`'s main Phase 2-bis
   phantom check (~line 460-467) widened to ALSO fire when
   `ri_barrier_tokens` is non-empty, reusing
   `gate.predicate_conflicts_batch`/`record_conflicts` verbatim (per the
   memo's §4.1 — this machinery takes a slice of `PredicateDep` and a
   snapshot version, no isolation-level coupling in the check itself).
3. **Commit-lock acquisition** — check whether the prototype's scope
   needs `commit.rs:742`'s guard widened too for the prototype's own race
   test to pass correctly (the memo's §4.2 point 4 says yes for the full
   mechanism; determine if the spike's single-site prototype can validly
   demonstrate the race closure without it, or if it's load-bearing even
   for a minimal proof — don't skip it if the race test would be
   unconvincing without it).

This prototype does NOT need to touch `fk_actions.rs`/`fk_on_update.rs` or
`group_commit.rs` — those are Step 2's job, once this spike settles the
design.

## What to prove

A deterministic race harness, adapting `fk_race_closure_tests.rs`'s
`RaceInjectingResolver` shape (the exact same `resolve_repo`-call-ordinal
deterministic injection seam — read that file's existing pattern in full
before writing anything new) but with an EXPLICIT `Snapshot` transaction
as the outer operation instead of an implicit one:

1. An explicit-Snapshot parent DELETE (RESTRICT case only, matching the
   prototype's scope) races a concurrent child INSERT at the exact
   after-begin/before-commit seam. Prove the barrier catches it: the
   parent's commit either aborts (with whatever error shape §2 settled
   on) or, if it wins the race, the RESTRICT check itself catches the
   child — never "delete succeeded AND a dangling child reference
   exists."
2. **Quiescent trial**: run the SAME explicit-Snapshot parent delete with
   NO concurrent writer, ~50 times, and confirm ZERO spurious aborts —
   mirroring F-28 Step 3's own quiescent-trial methodology exactly (check
   that spike's test file/memo for the precise shape to replicate).

## Deliverable

A decision memo at `docs/dev-artifacts/research/f40b-ri-barrier-spike.md`
(mirroring `f28-s3-mechanism-decision.md`'s structure: context, options,
the settled decisions for both open questions with reasoning, the
prototype's proof-of-concept results including the quiescent-trial
numbers, and a PRECISE implementation plan for Step 2 — exact touch
points: the 2 remaining recording sites, the 3 remaining commit-pipeline
guard-widening sites, per the original F-40 memo's §4.2/§7 citation
list, updated with whatever this spike's investigation refined).

The prototype code itself (the one recording site + one guard site + the
race test) MAY be committed as a clearly-scoped spike artifact (mirroring
how F-28 Step 3 handled its own spike code — check that precedent's
actual commit to see whether it kept the spike code or documented-only)
— Step 2 will do the complete, all-sites implementation regardless of
whether this step's prototype code is kept or superseded.

## Constraints

- Timebox this — it's a spike, not the full implementation. If the
  prototype takes an unreasonable amount of rework to get the race test
  passing, STOP, document what you found (including the difficulty) in
  the memo, and let Step 2 handle the harder mechanics with the design
  questions still settled from what you learned.
- Do NOT touch `FkReverseCache` internals (F-35/F-36, already landed).
- Do NOT touch `group_commit.rs`, `fk_actions.rs`, or `fk_on_update.rs` —
  out of scope for the prototype (Step 2's job).
- Do NOT update `KNOWN_LIMITATIONS.md` in this step — that happens in
  Step 2, once the real fix actually lands.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-tx -- --check` and
  `cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings`
  must be clean if any prototype code is committed.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-tx -- --check
cargo clippy -p shamir-engine -p shamir-tx --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine --full
```
