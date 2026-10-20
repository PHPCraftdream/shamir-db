# S5c-X PR6b-3 / W6 — documentation: receipt-only follower contract

Task 121 brief: `docs/dev-artifacts/prompts/remediation/121-s5cx-retire-legacy-follower.md`
(§5 "Documentation changes"). The code (W1-W5) makes the receipt protocol the ONLY follower
apply path: v1/v2/u64 bookmarks and floor-evidence states are REFUSED with the typed
`DbError::FollowerStateRefused { reason: LegacyBookmarkFormat | MalformedBookmark |
LegacyFollowerState }` at open / first admission; there is NO migration, NO rollback
tooling; the operator reseeds. Durable follower provenance IS the presence of a valid v3
record (bookmark zero, fence-less included); fresh adoption and seed installation write the
v3 baseline before service; visibility is derived from that durable state; leaders keep
their normal open/recovery behavior (a `LastCommittedVersion` floor without a bookmark does
not reject a leader — only a follower adoption/apply is refused).

## Hard rules
- ⛔ NEVER run any git command that mutates the working tree or index. Only edit files; the
  orchestrator commits.
- NO compile/test commands of any kind. No cargo anything.
- No machine-specific absolute paths. Keep each file's existing voice/structure; edit
  surgically, don't rewrite unrelated sections.

## Deliverable 1: `docs/guide-docs/follower-journal-durability.md`
Replace the contradictory historical activation narrative with the CURRENT receipt-only
contract. Specifically:
- Remove the legacy acceptance and migration/rollback promises in the intro block
  (~lines 3-12: "legacy u64 and v1 records have unknown J and retain conservative recovery
  behavior", the crash-image/negative-control paragraph's retired helper references, the
  "legacy u64 and v1 records ... legacy wire forms" sentences ~23-25) — replace with the
  strict contract: exactly ONE durable bookmark representation (v3); legacy records are
  rejection fixtures only; malformed/unknown fail closed; refusal happens at open AND at
  first follower admission; `LastCommittedVersion` floor without v3 = reseed required for
  follower use but a leader still opens.
- Keep the explicit distinction between graceful subprocess termination/reopen tests and
  real power-loss modelling (~lines 7-10) — do NOT upgrade those tests' claims.
- Update the "Activation rule" bullet (~line 65): the selector is GONE — admission is
  `admit_follower` (classify → Native | Fresh; Fresh self-adopts by writing the v3 baseline
  before any apply; v1/v2/u64/malformed refuse; floor-without-bookmark refuses for follower
  use; `AtomicBatchUnsupported` for incapable backends, checked before allocation).
- Replace the "Remaining step: PR6b-3: bookmark v1/v2 migration + rollback tooling" bullet
  (~line 77) and any other PR6b-3 migration promise with: DONE — receipt-only; unsupported
  durable follower states are refused; the operator action is to stop the affected
  subscription and reseed (existing seed/reseed workflow); no in-place conversion, floor
  clearing, bookmark deletion or force-continue exists.
- Document the new invariants: durable follower provenance = valid v3 record (survives
  receipt retirement, pause, subscription removal, restart; a zero-bookmark fence-less v3
  still identifies the follower); seed completion writes a v3 baseline (upstream = source
  version, local J = 1, no receipt); fresh adoption writes v3 before admitting apply;
  identity adoption updates the fence IN PLACE preserving bookmark/J/receipt pointer;
  cold-open classification runs BEFORE the non-persistent-WAL and `h <= j` early returns.
- Update test references to the matrices/files that changed (crash_image_tests now proves
  repeated refusal of the legacy fixture; the legacy subprocess replay block is gone; the
  legacy flush-order matrix is gone). Check every file path you cite still exists (the test
  layout is final after W4 — cite only real files/tests).

## Deliverable 2: `docs/guide-docs/KNOWN_LIMITATIONS.md`
- Replace the legacy-until-migration statements (~lines 659-660 and ~676-677, inside the
  cold-start bullet and the "Follower applies are receipt-protected" bullet): v1/v2/u64
  bookmarks are refused (typed error), no migration tooling is provided, reseeding is
  required. PRESERVE the receipt residual limits (admission/domain permit, atomic-batch
  backend requirement, `MAX_RECEIPT_EVENTS`/`MAX_RECEIPT_OPS` chunking, index/vector
  restriction, leader-path-unchanged statements) exactly as they are otherwise.
- Sweep both docs for any other "PR6b-3 will migrate / legacy until PR6b-3 / v3 migration"
  phrasing and fix it the same way (also check `docs/dev-artifacts/roadmap/REPLICATION.md`
  read-only: if it makes the same promise, do NOT edit it in this stage — report it).

## Report back
Sections changed per file (quote the before/after claim for the two KNOWN_LIMITATIONS
spots), any stale doc cross-references you found but did not edit, and the final list of
test files you cite so the orchestrator can cross-check them against the W4 outcome.
