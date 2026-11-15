# Brief: independent review of the engine metrics design (release-1 R4a)

## Why

`docs/dev-artifacts/design/engine-metrics.md` was written by one executor from
`docs/dev-artifacts/prompts/release/09-r4a-engine-metrics-design.md`. The project cycle is design, independent review,
brief, implementation. You are the independent review: do not trust the author's claims, check them against the code and
against the release plan (`docs/dev-artifacts/release-1/README.md`, `04-observability.md`).

## What

1. **Verify every OBSERVED claim.** Open each cited `path:line`; mark claims that are wrong, stale or unsupported
   (the author lists some line numbers as carried unverified: settle those first).
2. **Attack the primitive.** The fixed-bucket atomic histogram with a seqlock-epoch snapshot: memory orderings of every
   observe and every snapshot, a writer preempted between its two updates, wrap of the epoch, a scrape racing thousands of
   observers (is the reader starved or does it spin?), overflow and bucket-boundary semantics (`le` is inclusive in
   Prometheus), the sum field (float or integer, overflow), per-thread contention on one cache line, and whether it can be
   modelled with `loom` as claimed. Check the ~176 B per instance arithmetic. State any simpler design that is as
   correct if you find one.
3. **Check the placement.** A new `shamir-metrics` crate as the 7th crate of the `foundation` group: the group limit
   (7), the dependency graph without a cycle, no metrics-library dependency, and compatibility with the cut of
   `shamir-engine` into crates (`docs/dev-artifacts/design/engine-layering.md`) and with the layering gate
   (`.rust-guard/layers.toml`): the design says the metrics bundle is passed downward as parameters; confirm that no
   backward edge appears and that the parameter plumbing does not touch the very files being converted by the running
   engine-layering stages (R2c2, R2d, R4).
4. **Check the catalogue against the code.** For a sample of at least ten rows across all areas: does the named source
   counter exist, is the cost on the hot path stated correctly (a clock read per observed operation is not free), are the
   labels bounded, do the names follow the existing Prometheus naming of the server, is the normal range plausible.
   Check that no row duplicates an existing series (the author kept `shamir_gate_pending_versions`).
5. **Check cardinality and scrape cost.** The caps (64 tables, 32 indexes, deterministic top-N, `(other)` aggregation):
   is top-N selection itself cheap on thousands of tables, does a table appearing and disappearing between scrapes break
   Prometheus `rate()`, is the truncation flag honest.
6. **Check the cost proof plan:** do the named benches exist and cover the claimed hot paths, is the 3 % threshold
   measurable in CI noise, and is the plan CI-only (nothing is run locally).
7. **Check the PR plan** (PR-1..PR-6): file and folder limits, layer of each change, stage mapping to R4a-1..3, and the
   open questions with their defaults against the release plan.

## Output

Edit `docs/dev-artifacts/design/engine-metrics.md` in place: fix every defect you can fix with certainty, and append a
final section "Review" (English, numbered findings: severity, what was wrong, evidence as `path:line`, what you changed or
what stays open for the owner). Findings you could not settle go to the open questions with a recommended default. Do not
change sound decisions; do not touch any other file. If the design is sound, say so and list what you checked.

## Do not

Change code, tests, CI or dashboard files. Rewrite the document for style. Add scope (R4b, R4c).

## Observable acceptance

The Review section exists, every finding carries evidence, `python -I scripts/ci/check_layout.py` is OK. The final report
lists the findings by severity and what changed in the document.
