בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-23 (native↔WASM function parity campaign COMPLETE; awaiting commit)

## Session summary

Two campaigns this session. (1) A perf-implementation campaign (committed earlier: 5
commits 111baae..e145489, still UNPUSHED — 5 ahead of origin). (2) The main work:
a native↔WASM function-parity campaign run via /babygoal + sequential /crush
sub-agents under /opti, now COMPLETE and fully green but UNCOMMITTED.

The parity goal: the DB used as an EMBEDDED library must let an embedder register
ordinary native Rust functions — for filters, validators, and plain functions — at
parity with WASM, "otherwise it looks half-baked". After /oxx contemplation the design
crystallized as: TWO function contracts (scalar pure-sync `ScalarFn`; procedural
async-effectful `ShamirFunction`), where native-vs-WASM is just two CONSTRUCTORS of a
contract, tied together by a durable/ephemeral split (catalogue stores name+kind+bindings
durably; the in-memory artifact is WASM-materialized-from-bytes OR native-re-registered-
by-the-embedder-at-boot, fail-closed until then). 5 phases (Phase 3 = pure-WASM-scalar
bridge, DEFERRED behind separate sanction):

- Ph0 (#186): ArtifactKind{Native,Wasm} into the schema-less QueryValue::Map catalogue
  rows (migration-safe via from_record→Wasm default) + a 5-seam findings doc at
  docs/dev-artifacts/design/native-wasm-parity-phase0-findings.md.
- Ph1 (#187): FnAdapter<F> (blanket impl impossible → concrete wrapper) + register_fn;
  host-side validation_to_query_value encoder (validator/encode.rs) + NativeValidatorAdapter
  + register_native_validator (no more replace_artifact dance); boot loop dispatches on
  ArtifactKind, native rows skipped fail-closed.
- Ph2 (#188): ScalarResolver 2-layer (de-globalized builtin_scalars static → per-DB user
  layer + builtin fallback) threaded through ~all FilterContext::new sites; db.scalars()
  + .trusted_pure() index-safety gate; /opti computed_lower_eq −2.7% (no regression). The
  reopen path was made to thread the resolver + FAIL-CLOSED (the index build became
  Vec→Result<Vec> fallible so an unresolved user scalar errors instead of silently hashing
  every record to a Null key).
- Ph4 (#189): kind in list/show, drop parity, db.unresolved_native_artifacts() boot
  diagnostic + warn.
- Ph5 (#190): cross-cutting parity matrix tests (mixed native+wasm validators on one
  table, native↔wasm equivalence, three-plane independence, mixed-kind list, wasm
  reopen round-trip) in crates/shamir-db/tests/parity_matrix_gap_tests.rs.

crush was extremely flaky today (zai stream stalls, one hard 5-hour limit at 08:35, repeated
"temporarily overloaded" 429s) — survived ~10 mid-run deaths via retry-into-same-session.
Zero-trust review caught 3 real defects the agent's "exit 0" would have hidden: (a) reopen
silent-Null functional-index corruption → drove the fail-closed fix; (b) a fjall-"Locked"
flaky-test class (3 reopen-test retry helpers matched redb lock strings but not fjall's
"Locked") → I added "Locked" to all 3; (c) a half-applied multi-file refactor (try_all_backends
method didn't exist) after a mid-edit death. I made 5 manual recovery edits (2 Some()-wraps in
functional_backend.rs, 3 Locked-retry fixes).

Final holistic gate ALL GREEN: fmt --all 0, clippy --workspace --all-targets 0, workspace lib
3682 tests 0 fail, shamir-db --full 373/373 (incl all parity tests), db+engine+funclib --full
1673/1673. Nothing in flight. babysit cron (9a3a92cb) cancelled. TaskList empty.

## Active goal

None (`/goal` not set). babysit cron cancelled (TaskList emptied). No /loop.

## TaskList

Empty. All parity-campaign tasks completed: #186 Ph0, #187 Ph1, #188 Ph2, #189 Ph4,
#190 Ph5. (Phase 3 — pure-WASM-scalar bridge — deliberately NOT created; deferred behind
separate explicit sanction.)

## Decisions

- **Design: two contracts, native/wasm = two constructors, durable/ephemeral split** (chose
  this over merging the planes — purity/async is a real semantic boundary — and over a new
  persistence model — native functions reuse the existing catalogue-entry + in-memory-artifact
  split that replace_artifact already embodied).
- **Phase 3 (pure-WASM scalar bridge) DEFERRED** — native scalars already close the embedded
  need; sync-callable pure-WASM-scalar is the optional symmetry, separate sanction.
- **Reopen made FAIL-CLOSED, not silent-Null** — an unresolved user scalar in a functional
  index must error loudly, never corrupt the index by hashing everything to Null.
- **Fixed the fjall-Locked flaky class** (per CLAUDE.md "test-locks are BUGS, never tolerate")
  rather than tolerate the parallel-load flake the campaign's new reopen tests surfaced.
- **Did NOT commit/push** — awaiting explicit sanction.

## Open questions

- **Commit the parity campaign?** 41 files +858/−147 + new files
  (artifact_kind.rs, fn_adapter.rs, validator/encode.rs, validator/native_adapter.rs,
  scalar_resolver.rs, 4 test files, findings doc). Phases are heavily interleaved by file
  (Ph1/4/5 all touch native_parity_e2e.rs / function_management.rs / core.rs), so clean
  per-phase commits need risky hunk-splitting. Offered 3 thematic commits (procedural plane /
  scalar plane / tests+flake-fix) OR one feat(parity) commit. Awaiting "коммит" + choice.
- **Push the earlier perf campaign + the parity work** — 5 perf commits are still unpushed
  (5 ahead of origin) on top of the uncommitted parity diff. Awaiting "пуш".
- **The perf-hunt roadmap doc** (docs/dev-artifacts/checkpoints/2026-06-22-perf-hunt-roadmap.md) is still
  untracked from the prior session.

## Repo state

```
41 files modified (+858/−147) + untracked: artifact_kind.rs, fn_adapter.rs,
validator/encode.rs, validator/native_adapter.rs, scalar_resolver.rs,
user_scalar_tests.rs, native_parity_e2e.rs, parity_matrix_gap_tests.rs,
encode_tests.rs, docs/dev-artifacts/design/native-wasm-parity-phase0-findings.md,
+ prior-session checkpoint docs. Nothing staged.
```

```
e145489 perf(query): apply_distinct_qv keep-mask — drop redundant index set
cfd83f7 perf(tx): elide WAL deep-clone + single-pass phantom-predicate validation
8f1ce70 perf(auth): in-memory tickets-invalid-before cache — kill per-request fjall+msgpack
093ef3e perf(subscriptions): compile filter once at subscribe + defer value_qv decode
111baae perf(query): $in @ref semi-join O(N²)→O(N) — materialize ref column once
```

5 commits ahead of origin/master (the perf campaign), all unpushed. The parity campaign
(41 files) sits UNCOMMITTED on top. Working tree compiles + full gate green. babysit cancelled.
