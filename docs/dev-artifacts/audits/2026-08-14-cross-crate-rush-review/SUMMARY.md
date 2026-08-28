# Cross-Crate Review Summary

Consolidated result of the 2026-08-14 cross-crate rush review: 23 crates x 7 themed lenses = 161 individual reports under `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/<crate>/<theme>.md`, all read in full and synthesized read-only (no builds, tests, or code execution; no source files modified).

## Executive Summary

A 23-crate, 7-lens review (correctness/TDD, concurrency, security, performance, wire/API, error handling, style) of the entire ShamirDB workspace produced 1,610 individual review passes — one per crate×theme — yielding **1,220 findings: 2 critical, 120 high, 352 medium, 487 low, 259 nits**, all read-only with no build/test invocations. Overall verdict: the codebase is **architecturally sound but unevenly hardened** — the core concurrency ideology, test layout, and comment discipline are exemplary in most crates, but per-incident hardening, silent-swallow error paths, and vacuum-sealed test gaps mean roughly a dozen silent-data-loss, hang-class, and process-abort bugs are live in production paths today rather than latent.

The three things needing attention right now:

1. **The "one low-privilege query kills the server" family in shamir-funclib** — the uncapped `validate/is_json` parser and unbounded `random_bytes`/`repeat`/`pad` allocations let any authenticated session abort the whole process from a single query, defeating the per-connection panic isolation the architecture depends on; fix is a shared cap/depth-limit at the scalar boundary.
2. **The silent-write-loss / hang class on the hot path** — shamir-storage's cache-fill races (get_many tombstone poisoning, non-atomic CachedStore upserts), shamir-wal's circuit-breaker/leader latches that strand committers permanently after a single disk-full event, and shamir-client's subscription channels that hang forever on any disconnect — all concretely reachable by routine concurrent load or restarts, not just attacker input.
3. **The server-side authorization gap in shamir-server** — the interactive-transaction path (`TxBegin`/`TxExecute`/`TxCommit`) skips the read-only-replica write gate that `execute()` enforces, so any authenticated client can silently write through a replica and split-brain it from the leader; a two-line fix plus a Red test.

Everything else (1,100+ medium/low/nit findings, ~90 high-severity perf and interop items, and the systemic patterns — per-incident hardening instead of shared boundary utilities, docs that rot because doctests are banned, scc insert-misuse, duplicated cross-crate constants) is prioritized into a three-tier roadmap but does not block the next release.

## Finding Counts

### Whole-workspace totals (sum over all 161 files)

| Severity | Count |
|---|---|
| **critical** | **2** |
| **high** | **120** |
| **medium** | **352** |
| **low** | **487** |
| **nit** | **259** |
| **Total** | **1220** |

### Per-crate breakdown (crit / high / med / low / nit / total)

| Crate | crit | high | med | low | nit | total |
|---|---|---|---|---|---|---|
| shamir-types | 0 | 5 | 23 | 21 | 15 | 64 |
| shamir-collections | 0 | 1 | 3 | 9 | 5 | 18 |
| shamir-storage | 0 | 7 | 17 | 19 | 10 | 53 |
| shamir-wal | 0 | 5 | 19 | 23 | 12 | 59 |
| shamir-tx | 0 | 7 | 21 | 26 | 11 | 65 |
| shamir-engine | 0 | 12 | 23 | 33 | 19 | 87 |
| shamir-index | 0 | 10 | 27 | 29 | 13 | 79 |
| shamir-query-types | 0 | 7 | 21 | 27 | 11 | 66 |
| shamir-server | **1** | 1 | 6 | 7 | 3 | 18 |
| shamir-db | 0 | 10 | 22 | 27 | 12 | 71 |
| shamir-query-builder | 0 | 3 | 12 | 18 | 9 | 42 |
| shamir-connect | 0 | 8 | 19 | 29 | 20 | 76 |
| shamir-funclib | **1** | 8 | 17 | 23 | 15 | 64 |
| shamir-wasm-host | 0 | 7 | 17 | 31 | 14 | 69 |
| shamir-client | 0 | 9 | 19 | 26 | 15 | 69 |
| shamir-sdk | 0 | 6 | 15 | 19 | 10 | 50 |
| shamir-query-builder-macros | 0 | 3 | 6 | 16 | 7 | 32 |
| shamir-numa | 0 | 3 | 12 | 15 | 8 | 38 |
| shamir-transport-ws | 0 | 3 | 11 | 21 | 14 | 49 |
| shamir-transport-tcp | 0 | 1 | 12 | 22 | 14 | 49 |
| shamir-sdk-macros | 0 | 2 | 14 | 20 | 7 | 43 |
| shamir-bench-utils | 0 | 1 | 9 | 16 | 10 | 36 |
| shamir-tunables | 0 | 1 | 7 | 10 | 5 | 23 |

Counting notes: bundle findings are counted per explicitly severity-tagged item (e.g. engine perf "### Nits" = 8 nits; types api #11 = 5 nits; single-bullet nit bundles counted as 1). Several root issues appear in multiple sibling reports (e.g. the WAL circuit-breaker hang in 3 themes, `execute_as` ACL cost in 2, `try_build` nested-batch false reject in 2, `Doc::set`/`query_from` codec hot paths) — each report's finding is listed separately in the catalog below, as filed.

## Per-Crate Health Scorecard

**Ranking rule:** critical count descending → high count descending → qualitative severity tiebreak within exact ties (silent-data-loss / memory-safety / security-control weight over style weight). Verdict scale: *high-risk → needs focused remediation → moderate → solid with isolated gaps → mostly clean*.

| # | Crate | Total findings | Crit+High | Verdict |
|---|---|---|---|---|
| 1 | shamir-funclib | 64 | 1c / 8h | **high-risk** — process-abort DoS on query-reachable scalars (uncapped recursion + allocations) |
| 2 | shamir-server | 18 | 1c / 1h | **high-risk** — single critical (read-only-replica write bypass); otherwise the cleanest crate audited |
| 3 | shamir-engine | 87 | 0c / 12h | **high-risk** — pervasive hot-path quadratic costs plus drain-path silent data loss |
| 4 | shamir-db | 71 | 0c / 10h | **needs focused remediation** — silent catalogue corruption (cascade mis-target, phantom writes, swallowed renames) |
| 5 | shamir-index | 79 | 0c / 10h | **needs focused remediation** — silent wrong-results class (hash collapse, tokenizer bug, vector persistence loss) |
| 6 | shamir-client | 69 | 0c / 9h | **needs focused remediation** — permanent-hang class plus resume-path MITM exposure |
| 7 | shamir-connect | 76 | 0c / 8h | **needs focused remediation** — security-gate asymmetries, rate-limit race, unbounded audit memory |
| 8 | shamir-storage | 53 | 0c / 7h | **needs focused remediation** — concurrency races that silently mask acked writes; flush() hang |
| 9 | shamir-tx | 65 | 0c / 7h | **needs focused remediation** — MVCC version regressions and no-rollback durability illusions |
| 10 | shamir-wasm-host | 69 | 0c / 7h | **needs focused remediation** — sandbox-boundary bypass, resource bounds that don't hold, vacuous security tests |
| 11 | shamir-query-types | 66 | 0c / 7h | **needs focused remediation** — decode-time stack-overflow DoS and silent wire coercion |
| 12 | shamir-sdk | 50 | 0c / 6h | **needs focused remediation** — fail-open decode, unbounded guest memory, spin-on-Pending executor |
| 13 | shamir-wal | 59 | 0c / 5h | **needs focused remediation** — hang-class liveness on the durability spine (stranded committers, wedged leadership) |
| 14 | shamir-types | 64 | 0c / 5h | **needs focused remediation** — decode abort DoS plus silent-wrong-results primitives (Hash/Eq, lossy wire) |
| 15 | shamir-numa | 38 | 0c / 3h | **moderate** — one real concurrency bug (replica mirror race); otherwise pillar-clean |
| 16 | shamir-transport-ws | 49 | 0c / 3h | **moderate** — spec/interop gaps and an untested security control |
| 17 | shamir-query-builder-macros | 32 | 0c / 3h | **moderate** — silent write miscompilation (dropped fields); zero error-path tests |
| 18 | shamir-query-builder | 42 | 0c / 3h | **moderate** — over-strict validation pushing users off the checked path; per-field codec waste |
| 19 | shamir-sdk-macros | 43 | 0c / 2h | **lean but untested** — zero coverage anywhere; spurious rejections of valid signatures |
| 20 | shamir-transport-tcp | 49 | 0c / 1h | **solid with isolated gaps** — one latent unsafe/UB site; framing/TLS otherwise well-tested |
| 21 | shamir-collections | 18 | 0c / 1h | **solid with isolated gaps** — zero tests, but on the pillar-4 anchor whose failures propagate workspace-wide |
| 22 | shamir-bench-utils | 36 | 0c / 1h | **solid with isolated gaps** — hygiene and bench-fidelity findings only |
| 23 | shamir-tunables | 23 | 0c / 1h | **mostly clean** — one unwired runtime API shipped as functional |

### Tie-break and caveat notes

- **#1 vs #2:** shamir-server's critical is arguably the single worst finding in the review (split-brain write-through on a read-only replica), but it is one defect in an otherwise exemplary crate; shamir-funclib pairs a critical with 8 highs concentrated on the same dispatch path, so it carries more aggregate risk.
- **7-high group order** (storage > tx > wasm-host > query-types): weighted toward silent-acked-write-loss and hang classes over DTO-layer coercion.
- **5-high group** (wal > types): WAL's hang class sits on the durability spine; types' defects hit every decode path but self-heal or surface as errors more often.
- **3-high group** (numa > ws > qb-macros > query-builder): permanent divergence with a live consumer > untested security control > silent write-data loss > false rejections.
- **1-high group** (tcp > collections > bench-utils > tunables): latent memory-safety > coverage on the anchor crate > style-only > documentation/correctness-of-claims.
- **Blast-radius caveat:** low totals do not mean low impact — shamir-collections (18) and shamir-tunables (23) sit at the base of the dependency graph, and their defects surface in *other* crates' behavior, which is exactly how their findings are framed.

## Systemic / Cross-Cutting Patterns

Ranked by number of crates touched (≥3 distinct crates). Each entry cites the crate and the specific finding (severity/theme in parens). Ties are noted.

### 1. TDD gaps: vacuous tests, missing error-path coverage, missing fault-injection seams — 21 crates

Tests that cannot fail on the regression they name, error paths with zero coverage, or crates with no tests at all.

- **shamir-collections** — entire crate untested; a hasher/ordering regression compiles and passes `@types` green (high/correctness).
- **shamir-sdk-macros** — zero tests anywhere; `#[validator]`/`#[function]` compiled by no test in the workspace (high/correctness).
- **shamir-query-builder-macros** — ~25 diagnostic branches, zero error-path tests, no trybuild in the workspace (high/error).
- **shamir-wasm-host** — aggregate-fuel test can't distinguish aggregate vs per-Store fuel; wall-clock/epoch test satisfied by an unrelated early return; no host-import trap tests (high ×2, med/low).
- **shamir-server** — the read-only-replica gate (the critical) is untested at handshake level; `/readyz` 503 and `safe_run` panic survival never driven (critical context, low ×2).
- **shamir-wal** — §1.5 dirty-flag restore test re-implements the fix instead of driving it; §2.4 PermissionDenied and §1.3 file-sink retry uncovered (med ×3).
- **shamir-numa** — concurrency test asserts node 0 only; the mirror race (its high) is invisible to CI (med).
- **shamir-funclib** — `scalar_resolver.rs` zero tests (×3 reports); cap boundaries and Box-Muller scale unpinned (low/med).
- **shamir-engine** — no test drives `drain_step` against a table lacking an `MvccStore` (the high); pre-read error paths untested (low).
- **shamir-db** — no seam can fail `SystemStore::save_*`, so all swallowed catalogue-write paths are untestable (med).
- **shamir-storage** — CachedStore and MirroredStore never run the shared backend-conformance suite (low ×2).
- **shamir-tx** — error-path tests stop at two functions on fresh keys; no pre-existing-key failure test (med).
- **shamir-client** — vacuous `AtomicU8` test asserts std, not the client; `resume` end-to-end untested (med, low).
- **shamir-tunables** — only happy-path set/get; degenerate/override-takes-effect never tested (low).
- **shamir-query-types** — `fts_default_mode_is_and` asserts a value the test itself supplies (med).
- **shamir-connect** — #1090 refill test races all callers at `now == watermark`, structurally blind to the refill race; TOFU+failed-verify combination untested (high contexts).
- **shamir-transport-tcp** — test name promises 0.0.0.0 TLS-bind coverage the body deliberately doesn't deliver; write-side `TooLarge` never executed (low).
- **shamir-transport-ws** — the only framing error test asserts bare `is_err()`; no accept-rejection tests (med).
- **shamir-index** — `plan_update` has zero tests; `rebuild` test manually zeroes counters, working around the bug (med).
- **shamir-bench-utils** — `peak_mem` zero tests; `dim == 0` panic path undocumented and untested (low/med).
- **shamir-sdk** — http/params/db/`__rt` have no tests; two of four macros lack compile-pass (high, med).

**Root cause:** TDD is mandated in CLAUDE.md but nothing verifies the Red test exercises the *production* branch. Fault-injection seams were built per-crate on demand (#539 hooks, `arm_fail_next_append`, `FailingStore`) rather than as a uniform harness, so every crate's un-seamed paths shipped green, and several tests "simulate the fix" in the test body.

### 2. Documentation rot: stale, contradictory, or phantom docs on live code — 18 crates

- **shamir-connect** — `dispatch_request_view` "functionally identical" is false; `encode_details_canonical` doc promises encoding the stub never does; stale test-vector path (high, med, low).
- **shamir-tunables** — override API documented as effective while unwired; phantom `SHAMIR_VECTOR_SNAPSHOT_DELTA_THRESHOLD` env var read nowhere (high, med).
- **shamir-wal** — module docs describe the retired KV-marker design as current; `SegmentSet` "wired into nothing yet" is false; `WalActiveKey` dead export documented as live (med ×3).
- **shamir-wasm-host** — `net_grants` fail-open vs fail-closed docs contradict in one file; `FnCtx::global_get` docs promise gating only the host import enforces (med ×3).
- **shamir-db** — "O(1) point lookup" comments encode a scan-based cost model and hide an O(N²) list path (med).
- **shamir-types** — codecs README documents removed APIs and wrong panic semantics; `HavingView` comment contradicts code; core README describes an obsolete locking model (med ×2, nit).
- **shamir-numa** — "Фаза 1" scope docs and README describe shipped `LinuxTopology`/`detect()` as future work; "zero overhead" claim false (med, low).
- **shamir-storage** — `key_bytes.rs` doc claims the type is unused while it is the production key; prefetch/phantom-engine rustdoc (med, low).
- **shamir-index** — crate-root "NO std::sync::Mutex" invariant contradicts seven sanctioned sites; lifecycle doc predates `IndexState::Failed` (low, nit).
- **shamir-server** — `/info` documented "pretty-printed for curl" returns raw msgpack (med).
- **shamir-engine** — `finalize.rs` asserts a footprint-ordering invariant the AsyncIndex path no longer has (low).
- **shamir-tx** — `apply_committed_ops` doc describes the opposite error-path ordering the code implements; stale `predicate_conflicts` rationale (low, nit).
- **shamir-client** — `ResumeOptions.pinned_hash` documented as verified, is carry-through; `get_ddl_op_status` comment contradicts code; lib.rs example doesn't compile (med, low, low).
- **shamir-funclib** — eleven category headers say "plain names" while wire names are folder-qualified; `substring` doc contradicts pinned behavior (med, low).
- **shamir-bench-utils** — `peak_mem` docs teach the removed Criterion harness; stale "criterion bench" reference (med, low).
- **shamir-transport-tcp** — `ConnectionExporter` doc misdescribes the adapter; UDS support promised, API can't express it (nit ×2).
- **shamir-transport-ws** — pre-auth 4 KiB cap attributed to this crate's framing (it lives at the caller); dead `BROWSER_CHANNEL_BINDING` documented as single source of truth (nit/low).
- **shamir-query-builder** — stale `val::query_ref` reference; `IntoBatchOp` doc describes removed impls (nit).

**Root cause:** the workspace bans doctests (`doctest = false` everywhere) and rustdoc is not in the pre-commit gate, so doc/code divergence is *never mechanically surfaced* — CLAUDE.md itself records that its own F-9 exception list drifted the same way. Comments are the enforcement mechanism for several conventions (contention models, O(N) acks, wire semantics), which makes doc rot a correctness hazard, not just hygiene.

### 3. Hidden O(N)/O(N²) per-op costs on hot paths (pillar-3 violations) — 16 crates

- **shamir-db** — ACL gate = full catalogue scans per ancestor per op (high ×2); O(tables²) FK guards; O(repos×tables) boot pairing.
- **shamir-engine** — O(N²·K) re-planning under commit lock; O(M²) staged-overlay probes; per-value FK scans; repo-wide scan per UPDATE; per-row `IndexDefinition` clones.
- **shamir-types** — projection codec O(k·f) per row; `for_each_field` O(f²); per-key `String` alloc in decode; scratch-buffer reuse defeated by `mem::take`.
- **shamir-index** — per-row deep-clone of all index definitions; sorted/unique apply = one round-trip per posting; FTS lookup materializes+sorts everything; O(N) registry dispatch.
- **shamir-connect** — session-cap insert = O(all sessions) under one global mutex on every login.
- **shamir-tx** — `vacuum_key` unbatched/duplicated per-write I/O; `min_alive` full traversal per write; linear interval checks under `commit_lock`.
- **shamir-funclib** — `count_distinct` O(N·C); per-row regex recompile; stddev buffers whole column.
- **shamir-wal** — `has_truncatable` O(frames) scan every drainer tick.
- **shamir-client** — per-request full clone of the serialized request; per-row `ByteBuf` clone + key allocs in de-intern; ambient sync walk per `execute`.
- **shamir-query-types** — `BatchOp::deserialize` triple codec pass + 75-probe linear dispatch per op; per-record collect+sort in `InsertedRecord::serialize`; deep clone per scalar lookup.
- **shamir-query-builder** — `Doc::set` full msgpack round-trip per field; `rows_as` per-record codec.
- **shamir-wasm-host** — deep params clone + double buffer copy per invocation; per-call export re-resolution.
- **shamir-sdk** — per-host-call buffer leaks (O(cumulative traffic) within an invocation); O(P) param scans.
- **shamir-storage** — reverse range streams drain entire range to RAM; eager whole-corpus materialize; `transact` drains the whole dirty buffer; default range filter scans past the upper bound.
- **shamir-numa** — `load_local` pays a virtual call where docs claim "zero overhead" (low).
- **shamir-bench-utils** — one heap `Vec` per generated point (alloc-in-loop in the shared fixture).

**Root cause:** pillar 3 is aspirational prose with exactly one mechanical enforcement (`scc::*::len()` disallowed-methods) — which several crates *evade* with `range(..).count()` (shamir-tx, shamir-index noted this verbatim). The amortizing primitives exist (`FieldIndex`, `get_many`, entry APIs, benches) but call sites don't use them, and multiple reviewers confirmed **no bench covers any of these paths**, so the costs are invisible to the gate.

### 4. Cross-crate contracts duplicated by hand instead of shared — 16 crates (tie)

- **shamir-tx + shamir-engine** — `SORTED_TAG` posting-key layout duplicated, "pinned by test" claim protects the wrong crate; a drift silently disables Serializable phantom detection (med).
- **shamir-types + shamir-storage** — `SYSTEM_RECORD_PREFIX` `[0,0,0,0]` re-encoded locally; a `RecordId::system` change silently makes every durable-config key ephemeral (med).
- **shamir-connect + shamir-transport-tcp** — `EXPORTER_LABEL` duplicated byte-for-byte; a v2 label in one place silently desyncs channel binding (low).
- **shamir-wasm-host + shamir-sdk** — HTTP header wire codec hand-mirrored in both; the map shape collapses duplicate headers on both sides (high).
- **shamir-numa + shamir-index + shamir-bench-utils** — the LCG "lineage contract" mirrored ~13 times with zero enforcement, justified by a claim ("not a dev-dependency") that is now false (med).
- **shamir-transport-ws + shamir-transport-tcp** — claimed-identical wire format with divergent zero-length semantics and send-cap asymmetry (med).
- **shamir-client + shamir-server** — positional handshake frames mirrored as two independent struct definitions, order enforced by nothing; resume frames carry no version (med).
- **shamir-engine + shamir-query-types** — two query parsers for one logical message; the legacy one silently drops semantics (high).
- **shamir-query-types + shamir-db + clients** — one `RecordId` identity rides the wire three ways (`op_id` bin vs string vs base58 `_id`) (med).
- **shamir-transport-ws + shamir-server** — `BROWSER_CHANNEL_BINDING` exported, then `[0u8; 32]` re-hardcoded at both consumption sites (low).
- **shamir-transport-tcp + shamir-server** — normative loopback predicate duplicated (low).

**Root cause:** dependency-direction rules (types ↛ storage, builder re-exports for guests, etc.) mean the "natural" shared home is often forbidden, and the workspace has **no cross-crate contract/golden tests** — the only guard is prose comments that routinely rot (see pattern 2).

### 5. Stringly-typed errors; thiserror rule unenforced — 13 crates

- **shamir-tx** — thiserror declared in Cargo.toml, used zero times; six `Result<_, String>` APIs incl. the public `ChangelogStore` trait (med).
- **shamir-wal** — no error enum at all; io::ErrorKind destroyed at wrap; tests substring-match messages (med ×2).
- **shamir-funclib** — free-form `ScalarError` string codes already drifting (`bad_regex` vs `bad_pattern`) (med).
- **shamir-index** — `IndexError::Storage(String)` flattens `DbError` at every boundary (med).
- **shamir-transport-tcp** — `Box<dyn Error + Send + Sync>` from boot-critical TLS constructors; `TooLarge` mislabeled for malformed buffers (med ×2, low).
- **shamir-types** — two rival public `CodecError` enums, one hand-rolled (med).
- **shamir-wasm-host** — gateway traits and SSRF guards return `Result<_, String>` (med).
- **shamir-sdk** — single-message `Error`; wire/protocol failures constructed as "user" errors (high/med).
- **shamir-client** — typed shamir-connect errors flattened via `to_string()` into `Handshake(String)`/`Protocol(String)` (med).
- **shamir-query-types** — hand-rolled Display/Error impls; `Result<(), String>` validators (low).
- **shamir-query-builder** — five enums hand-roll ~250 lines of Display/Error boilerplate (low).
- **shamir-connect** — `validate_client_kdf_safe -> Result<(), String>`; hand-rolled `AuditError` (low/nit).
- **shamir-db** — `DbError::Internal(e.to_string())` collapse sites; substring-based error classification (low, med).

**Root cause:** the rule exists in CLAUDE.md but is migrated per-crate on touch; two crates consciously avoided the dependency (query-types' minimal-deps stance, funclib's code-only error model) without documenting the deviation, and nothing in the gate distinguishes "stringly by decision" from drift.

### 6. Untrusted-input hardening applied per-incident, inconsistently — 12 crates

The same class (depth caps, allocation caps, size bounds) is fixed in one place and missing in its siblings.

- **shamir-types** — `SANE_PREALLOC_CAP` added to the serde visitor only; the zerocopy decoder and merge path still allocate from wire headers → 5-byte abort DoS (high ×2).
- **shamir-funclib** — `argon2id` fully capped while `is_json` (critical), `random_bytes`/`repeat`/`pad` (high) share the same dispatch path uncapped.
- **shamir-query-types** — `MAX_FILTER_DEPTH` documented as preventing stack overflow, but decode itself recurses unbounded (high ×2); `check_filter_depth` doesn't descend into `FilterValue`.
- **shamir-engine** — the depth guard covers 3 of ≥6 reachable filter surfaces (`when`, `having`, `FilterValue` nesting unguarded) (med).
- **shamir-wal** — bincode decode with no allocation bounds; unbounded `read_to_end`; 32-bit frame-length overflow (low ×3).
- **shamir-db** — curl egress: CRLF injection via headers/method (high); response `read_to_end` with no size cap (low).
- **shamir-sdk** — no depth guard on peer-frame msgpack decode (low).
- **shamir-client** — no depth guard on server-frame decode (low).
- **shamir-wasm-host** — SSRF private-IP set misses `0.0.0.0`/CGNAT ranges (med); unbounded `GlobalVars` (med); unbounded build-output pipes (low).
- **shamir-index** — `NgramTokenizer` unbounded output → indexing-time amplification (low); posting cache byte-unbounded (med).
- **shamir-numa** — `parse_cpulist` expands unbounded ranges → OOM abort (med/low); `CPU_SET` without `CPU_SETSIZE` bound (med).
- **shamir-transport-tcp** — 16 MiB upfront allocation from a 4-byte attacker header, no per-connection byte accounting (med).

**Root cause:** hardening is **reactive per-incident** (each audit/cap landed locally: `SANE_PREALLOC_CAP`, `A2_MAX_*`, `MAX_FILTER_DEPTH`, `MAX_PRE_AUTH_FRAME`) instead of a shared boundary toolkit (depth-counting deserializer wrapper, capped allocator helper). The argon2id reviewer said it outright: "the same posture is not applied to these three functions."

### 7. Security gates enforced at a single entry point, bypassable at siblings — 11 crates

- **shamir-server** — the read-only gate exists on `execute()` only; `tx_execute` bypasses it (**critical**).
- **shamir-query-types** — `is_admin()` omits `ForEach`; destructive-HMAC gate never descends into `Batch`/`ForEach`; `cascade`/`dst_path` absent from canonical inputs (high, med ×2).
- **shamir-db** — validator source path skips the #607 `WasmCompiler` gate its function twin has; ambient interner delta skips the Store-Read ACL the dump op requires (med ×2).
- **shamir-engine** — no structural authz; correctness rests solely on callers remembering `execute_as` (low).
- **shamir-connect** — `dispatch_request` exports the ungated twin of the rate-limited `_view` variant (high-style/med).
- **shamir-client** — `resume` performs none of `connect`'s identity checks while disclosing the bearer ticket (high ×2).
- **shamir-transport-tcp** — `NoCaVerify` disables all TLS authentication; safety depends entirely on the caller running the protocol-layer pin (med).
- **shamir-index** — `trusted_pure` enforced only at CREATE INDEX, never at this crate's dispatch; unique constraint enforced on hash alone (med ×2).
- **shamir-wasm-host** — forbidden-macro scanner bypassable by grammar-legal whitespace (high).
- **shamir-funclib** — scalar purity is a token-match lint, bypassable by a type alias (low).
- **shamir-sdk** — same lint-shaped purity guarantee (low).

**Root cause:** each gate was added at the "natural" entry point after an incident; there is no workspace invariant of the form "every entry point of kind X must call gate G" and no type-state/token (the engine reviewer explicitly proposes `Authorized<BatchRequest>`), so every newly added sibling entry point silently skips the gate.

### 8. TOCTOU / non-atomic read-modify-write on "lock-free" structures — 11 crates

- **shamir-numa** — `rcu` mirror: load-then-blind-store loses to a concurrent writer → permanent replica divergence (high ×3).
- **shamir-storage** — `CachedStore` remove+insert cache mutations; `get_many` ungated fill; `InMemoryStore::set` remove/re-insert (high ×2, med).
- **shamir-db** — table DDL guard windows acknowledged by a `debug_assert!`; drop/rename bypass the #546 create locks; group existence checked before the RMW lock (med ×2, low).
- **shamir-engine** — `ValidatorRegistry::add_binding` two critical sections, loser's insert Err discarded (med).
- **shamir-index** — posting-cache miss→scan→insert pins stale entries past invalidation; batch planner bypasses in-flight-build dirty-set capture (med ×2).
- **shamir-connect** — `ServerIdentityState::rotate` load-check-store on ArcSwap; per-subnet refill watermark regression (#1090 class at the sibling site) (low ×2, med).
- **shamir-tx** — `publish_committed` plain `store()` can regress the floor its doc claims is lock-protected (high, latent).
- **shamir-types** — `touch_ind` vs `touch_with_id` cross-API race guarded only by `debug_assert!` (low).
- **shamir-wasm-host** — `replace`/`rename`/`put`/`set` as remove-then-insert: transient `NotFound`, silent name-theft, lost rollback (low).
- **shamir-server** — `reserve_pending → spawn → attach_handle` racing `close_all` detaches a live bridge task (med).
- **shamir-client** — subscriptions/early-buffer two-lock handoff strands or reorders pushes (med/low).

**Root cause:** pillar 1 mandates lock-free primitives but never mandates *linearizability discipline*. scc/ArcSwap/DashMap make single ops atomic; every multi-step sequence (check-then-act, load-clone-store, remove+insert) is hand-rolled, and the safer primitives each crate needed — `entry_sync`, `upsert_sync`, `fetch_max`, epoch/versioned CAS — existed one call away in every case.

### 9. Persisted formats without versioning or verified forward-compat — 11 crates (tie with #7/#8)

- **shamir-storage** — `MemBufferConfig`: "stable wire-format" claim, no serde defaults, no version field, no golden test (high).
- **shamir-engine** — `MetaEnvelope` convention skipped for `MemBufferConfig`, `ShadowEntry`, and the index2 tombstone (med).
- **shamir-query-types** — durable changefeed journal: no envelope, decode failures silently skipped (high).
- **shamir-index** — SQ8 opt-in has no durable carrier (high); v1 snapshot back-compat claimed, decode path absent, test vacuous; FxHash output stability not version-coupled to persisted keys; ordinal-stability docs on some enums only (med ×2, low).
- **shamir-wal** — `idx_id` written as constant 0 with semantics "deferred"; no per-frame magic/seq or file header; version-skew decode aborts recovery (med, low, nit).
- **shamir-types** — Dec/Big/Set cannot round-trip; u64>i64::MAX decodes differently per decoder (high, med).
- **shamir-funclib** — `canonical_hash` byte format versionless and codec-coupled, doc's key-order claim factually wrong (med).
- **shamir-sdk** — guest ABI has no version/capability negotiation; compiled guests outlive host upgrades (med).
- **shamir-transport-ws** — spec subprotocol negotiation unimplemented; zero-length semantics diverge from the claimed-identical TCP format (high, med).
- **shamir-client** — resume frames carry no version axis; positional handshake correctness rests on two hand-maintained structs (med).
- **shamir-db** — error-`code` contract populated unevenly across handler families; `wasm_hash`/`version` fields dead (med, low).

**Root cause:** the correct pattern exists and is even cited by reviewers (`ddl_op_log`'s version byte + fail-closed dispatch; `MetaEnvelope`) — but adoption is opt-in per struct. There is no workspace rule "every persisted/byte format carries a version and a golden-bytes test," and `#[serde(default)]` masks drift instead of forcing a decision.

### 10. Dead or unwired public API shipped as if functional — 10 crates

- **shamir-tunables** — `RuntimeTunables` setters/getters with zero production readers, documented "takes effect on next read" (high, med ×3).
- **shamir-db** — dead `api::{Request,Response}` wire shim exported (high); `PendingCommit`/accessors exported from dead scaffolding (nit); `Actor::System` convenience wrappers first-class public (med).
- **shamir-connect** — `dispatch_request` (ungated) exported as peer API; `encode_details_canonical` stub; blocking `Argon2Semaphore::acquire` with zero production callers (high-style, med, low).
- **shamir-wal** — `WalActiveKey`/`looks_like_v2` exported, zero consumers, docs claim live (med); `mark_poisoned` un-gated pub test hook (nit).
- **shamir-bench-utils** — `measure`/`measure_async`/`current_allocated` zero callers, documented via a removed harness (med).
- **shamir-index** — `IndexDescriptor.options` dead public field misleading readers (low).
- **shamir-client** — `ClientError::RequestIdMismatch` never constructed (low/nit).
- **shamir-tx** — `enqueue_pending`/`drain_pending`/`PendingCommit` exported from sanctioned-dead scaffolding (nit).
- **shamir-engine** — `SessionPermissions` publicly exported while documented test-only (low).
- **shamir-sdk** — `pub mod __rt` contradicts its own "not public surface" doc (low).

**Root cause:** slice-based development ships the API half before the wiring half ("deferred to a follow-up slice" appears verbatim in shamir-server and shamir-tunables), and `#[doc(hidden)]` + SAFETY marking (the #606 precedent) is applied only when someone remembers. `dead_code` lint can't see `pub` items, so the gate never flags them.

### 11. Unbounded in-memory growth — 9 crates

- **shamir-connect** — `AuditChain` retains every audit event forever (high).
- **shamir-tx** — pessimistic `locks` registry never evicts (med ×2); A10 barrier can starve GC indefinitely (low).
- **shamir-client** — early-buffer key cardinality server-controlled and uncapped; orphaned `pending` entries on cancellation (med, low).
- **shamir-engine** — changelog journal has no retention; DDL op-log eviction is a stub; shadow log never purged (low ×2 + context).
- **shamir-wasm-host** — guest-writable process-lifetime `GlobalVars` with no quota or removal import; leaked ticker thread per engine; unbounded compile pipe buffers (med, low, low).
- **shamir-funclib** — stddev/variance buffer the entire column where O(1)-state Welford exists (med).
- **shamir-storage** — Async write-behind channel unbounded (contrasts with fjall's deliberate 1024 bound) (med).
- **shamir-index** — DROP leaks the whole `__vec_snap__` keyspace; generation flip never prunes q-chunks; posting cache bounded by count, not bytes (med ×3).
- **shamir-sdk** — per-call ABI leaks both directions (high).

**Root cause:** O(x→0) as written targets *per-op asymptotics*; "bounded cardinality/retention" is a separate invariant with no owner. Caches and queues get added with a size *comment* but no budget, eviction policy, or high-water metric (the CLAUDE.md `AtomicUsize`-mirror pattern exists for `len()` only).

### 12. Hang-class liveness: latches and waits without release-on-all-exits — 8 crates

- **shamir-wal** — circuit-breaker strands parked waiters; leader cancellation wedges `flushing` forever; unbounded leader tenure (high ×3, low).
- **shamir-engine** — panicking `GroupCommit` leader strands `leader_busy`; DashMap guard across `.await` can wedge workers; unbounded migration catch-up loop (med, high, nit).
- **shamir-funclib** — `CountingSemaphore` lost wakeup parks a runtime worker indefinitely on a quiescent system; blocking Argon2 acquire parks workers (med ×2).
- **shamir-client** — reader-drain race hangs requests under default no-timeout; subscription channels never close on EOF; undecodable frame hangs its waiter at debug log level (high ×2, low).
- **shamir-sdk** — `block_on` no-op-waker spin can never make progress (high).
- **shamir-storage** — `CachedStore::flush` parks forever if the worker task dies; `batch_size == 0` yields empty batches forever on InMemoryStore (high, med).
- **shamir-connect** — exported blocking semaphore wait is an executor-parking trap; `cap_lock` serialization under auth storms (low, high-context).
- **shamir-transport-ws** — unbounded consecutive control-frame loop with no progress guarantee (low).

**Root cause:** leadership flags, semaphores, and `Notify`-parks are acquired with plain stores/CAS but released only on the happy path; there is no RAII-guard convention for *logical* latches (the crate has excellent RAII for resources — `VersionGuard`, `OpGuard` — but not for `flushing`-style state), and cancellation (`select!`/`timeout`/task drop) is an exit path repeatedly forgotten. CLAUDE.md declares hangs bugs but provides no checklist for latch ownership.

### 13. Blocking work executed on async runtime threads (pillar-2) — 5 crates (three-way tie)

- **shamir-storage** — `FjallStore::submit` blocking `SyncSender::send` inside `async fn`; parks tokio workers at queue-full (med ×2 reports).
- **shamir-wasm-host** — `compile_rust_source` (≤120 s cargo build) called inline in async DDL paths (high).
- **shamir-funclib** — `argon2id` blocking semaphore acquire inline on runtime workers; regex compile under a global lock (med, high).
- **shamir-connect** — exports a blocking Mutex+Condvar `acquire` presented as the design; sync audit fsync invoked on the request thread (low, low).
- **shamir-numa** — `detect()` does O(nodes) blocking sysfs reads, already called on async construction paths via shamir-index (low).

**Root cause:** pillar 2 says "CPU/blocking → `spawn_blocking`" but several offending APIs are *sync `pub fn`s* with no documented offload contract, and the crate that owns the call site doesn't know. The funclib module doc itself names the fix ("project-wide spawn_blocking refactor flagged as follow-up") — a follow-up that never landed.

### 14. Banned hot-path lock primitives without the sanctioned justification — 5 crates (tie)

- **shamir-connect** — `parking_lot` `cap_lock` on the auth path; `AuditChain::inner`; five Mutex sites with no contention-model comment (high, med, med).
- **shamir-funclib** — process-global `std::sync::Mutex` regex cache on the per-row filter path (high).
- **shamir-client** — `std::sync::Mutex` `PendingMap`/`SubscriptionMap`/`EarlyBuffer` locked per request/frame (high, med).
- **shamir-engine** — DashMap shard `Ref` held across `.await` (the exact class the crate documents and fixed elsewhere) (high).
- **shamir-wal** — `MemSink` holds a `std::sync::Mutex` across O(N) decode inside async fns (nit; inline-sanctioned).

**Root cause:** the F-9/#1076 regime makes the *inline comment* the enforcement mechanism — i.e., enforcement is prose. No lint flags new `Mutex`/`parking_lot` fields or guard-live-across-`.await` shapes (clippy cannot see the latter), and CLAUDE.md itself documents that this exact drift already forced one revision. New code copies the nearest existing pattern, so one unjustified site legitimizes the next.

### 15. Non-keyed FxHash fed untrusted inputs (pillar-4 premise broken) — 5 crates (tie)

- **shamir-collections** — `THasher` origin: client-controlled strings demonstrably become `TMap`/`DashMap` keys downstream (med).
- **shamir-engine** — batch params/alias names from client payloads keyed on `THasher` (cited in the collections finding).
- **shamir-tx** — `pub` APIs accept arbitrary `Bytes` keys into Fx-keyed structures; premise holds only by upstream convention (nit).
- **shamir-storage** — `dirty: DashMap<RecordKey, _, THasher>` keyed by caller-supplied keys; posting keys embed indexed values (low).
- **shamir-index** — index/unique posting keys and FTS `token_hash` hash attacker-chosen values with correlated unkeyed Fx streams; collisions are the constraint (med ×2).

**Root cause:** pillar 4's justification — "we don't accept untrusted hash inputs here" — was written as a global premise and never validated at the ingress points that later appeared (client batch params, indexed field values, document text). No boundary validation or keyed-hasher exception exists, and CLAUDE.md explicitly rejects the RandomState trade-off globally, so each crate silently inherits a false assumption.

### 16. scc API misuse: `insert` assumed to upsert; entry/atomic primitives unused — 3 crates

- **shamir-engine** — `ValidatorRegistry::add_binding` loses the loser's table binding (scc `insert` never overwrites) (med).
- **shamir-funclib** — `UserScalarLayer::register` discards the Err; documented "(or replace)" never replaces (high).
- **shamir-wasm-host** — `replace`/`rename`/`put`/`set` hand-roll remove+insert instead of the entry API its own `update()` already uses (low).

**Root cause:** scc's `insert_sync` returning `Err` on an existing key is counter-intuitive relative to `std`/DashMap muscle memory; the workspace knows the semantics (`repo_instance.rs` documents "silently no-op"; `tx_context.rs` comments its deliberate discard) but there is no lint or doc convention requiring `entry_sync`/`upsert_sync` for overwrite intent.

### Shared root causes (meta-patterns)

1. **Enforcement by prose.** Nearly every pattern above maps to a CLAUDE.md rule that has no mechanical gate: TDD (1), doc accuracy (2), pillar-3 (3), thiserror (5), untrusted-input (6), pillar-2 (13), pillar-1 comments (14), pillar-4 premise (15). The workspace's one *mechanized* rule — the `scc::len()` disallowed-methods lint — is the only one that doesn't exhibit systemic drift (and is actively evaded with `range(..).count()`).
2. **Doctests banned + rustdoc ungated ⇒ comment rot is invisible** (feeds pattern 2, and via rotten comments, patterns 7/9/10, since docs *are* contracts for contention models, wire semantics, and exception registries).
3. **Slice-based shipping without half-shipped markers** — API/wiring, cap/hardening, and gate/check pairs land in different slices; the unwired half looks functional (10), the unhardened sibling mirrors a hardened one (6), the sibling entry point skips the gate (7).
4. **Per-incident hardening instead of shared boundary utilities** — every cap/depth-limit/version-byte is re-derived locally; a shared "untrusted input" deserializer wrapper and a "versioned blob" helper would collapse patterns 6 and 9.
5. **Hand-duplicated contracts with no golden tests** — dependency-direction rules forbid the natural shared home, and nothing asserts two crates' copies stay equal (4); the dup then rots independently (2).
6. **Lock-free primitives without a linearizability discipline, plus latches without RAII** — atomics make single ops safe and multi-step sequences wrong; `fetch_max`/`entry_sync`/epoch-CAS fixes existed one line away in nearly every TOCTOU finding (8, 16), and logical flags lack the Drop-guard culture the resource guards already have (12).

## Single Highest-Risk Finding

**The pick:** `shamir-funclib` — *"validate/is_json hand-rolled parser recurses without a depth limit — query-reachable stack overflow aborts the process"* (filed **CRITICAL** in error-handling-lifecycle; the same root filed HIGH in security-crypto). `crates/shamir-funclib/src/validate.rs:123-191`, registered builtin, entry at `validate.rs:87`.

One unprivileged query — `WHERE validate/is_json(<~10–20 KB of nested '['>)` — exhausts the thread stack. A Rust stack overflow is **not a panic**: it is a hard `abort` that `catch_unwind` cannot intercept.

### Why it outranks every other critical/high finding

**Axis 1 — Reachability from untrusted input: maximal, with the cheapest possible principal.**
- Trigger requires *only* an authenticated session with query permission — the lowest privilege tier in the system. No admin op, no WASM-compile permission, no replica topology, no network position, no privileged tenant.
- The input is 100% attacker-authored through a first-class, documented scalar reachable from WHERE filters, schema validator field rules, and WASM guests. No binary-frame shaping, no interleaving races to win, no fault (ENOSPC, cancellation) needed first.
- Trigger cost is tens of bytes of query text; every other remotely-reachable finding in the catalog needs more setup than this (the query-types decode-DoS needs deep msgpack framing and is *accidentally* backstopped by rmp-serde's ~1024-container limit; funclib's parser has **no limit at all**).

**Axis 2 — Blast radius: total, immediate, and architecturally uncontained.**
- The abort kills the **entire process** — every connection, every session, every tenant — not one request. This is precisely the containment boundary the root `Cargo.toml`'s `panic = "unwind"` design exists to guarantee (#895 / F-68 cluster C: "one connection's panic can't take down another's"), and this finding defeats it.
- It is trivially **repeatable**: restart, one query, dead again — a sustained remote DoS prosecuted by the cheapest principal in the system, with no mitigation short of code change (you cannot timeout, catch, or rate-limit your way out of a stack overflow).
- Contrast with the two next-largest blast radii: the WAL circuit-breaker hang (HIGH ×3) takes down the commit path but *requires an I/O fault or task cancellation to arm*; the db `DROP DATABASE ... CASCADE` mis-target (HIGH) destroys data but requires a *trusted admin* to issue an unusual cross-db batch shape. Both have preconditions this finding lacks.

**Axis 3 — Latent in production today, not theoretical.**
- `validate/is_json` is registered in `register_builtins` right now and dispatched on the standard query/guest paths. No feature flag, config, or deployment shape suppresses it — single-node included (whereas the server tx-bypass critical only matters where replicas run).
- The test suite covers only shallow inputs (`validate_tests.rs:186-195`), so nothing in CI will surface it before a user does.
- Contrast with findings of comparable or larger *durable* impact that are conditional: the client resume-MITM highs need a network-position attacker; the wasm-host scanner bypass needs an authorized compile-capable tenant; the engine DashMap-guard-across-await needs specific concurrent DDL/write interleave; the tx `finalize_reservation` regression self-heals on drainer republish.

### The honest head-to-head: vs. the only other filed CRITICAL

**shamir-server — interactive-tx bypasses the read-only-replica gate.** Its counterargument is real: its impact is *persistent silent corruption* (durable local writes on the replica, split-brain divergence, reads serving phantom data, resync required) versus my pick's *transient outage*. If the question were "which finding causes the most lasting damage per successful exploitation," the server critical wins on persistence.

It still ranks second on these grounds:
1. **Exposure set:** the funclib abort is live on *every* deployment — single-node, replica, embedded; the server bypass matters only where read-only replicas are deployed.
2. **Blast radius shape:** one replica's data integrity vs. 100% availability loss for all tenants simultaneously. An attacker with the is_json trigger doesn't need to *want* anything — any user's typo-depth JSON destroys service for everyone.
3. **Isolation-model violation:** the server bypass violates a *feature's* invariant (read-only mode); the funclib abort violates the *architecture's* containment guarantee, which is the assumption every other isolation argument in the review silently rests on.
4. **Family effect:** the same scalar dispatch path carries three sibling HIGHs (`random_bytes`/`repeat`/`pad` → allocator abort). The is_json finding is the worst member of a four-strong "one low-priv query kills the server" family, so fixing the class yields compound risk reduction.

**Ranking summary:** is_json (funclib) > server tx-bypass > WAL hang family > client resume-MITM > db cascade mis-target — ordered by (untrusted-input reachability × blast radius × zero-precondition latency), with persistence-to-recover as the one axis where the server critical leads.

## Full Critical + High Finding Catalog

Grouped by crate; ordered by severity (the two criticals first, then descending high count). Format: **[severity][theme]** verbatim title — file:line — one-sentence summary.

### shamir-server (1 critical, 1 high)

- **[CRITICAL][correctness-tdd] "Interactive-tx path bypasses the read-only-replica gate entirely"** — `db_handler/tx_handlers.rs:73-205` (vs `handler.rs:523-541`): `tx_execute`/`tx_begin`/`tx_commit` never check `NodeMode::ReadOnly`, so a client can write through a read-only replica via the transactional API and silently diverge replica state from the leader.
- **[HIGH][correctness-tdd] "Dead follower-loop registry entries block resubscription after a journal gap"** — `replication/supervisor.rs:286-337`: the spawned follower loop never removes its registry entry on exit, so `reconcile()` (which skips names present in the registry) can never restart a repaired subscription; the gap is self-admitted in `supervisor_tests.rs:398-401`.

### shamir-funclib (1 critical, 8 high)

- **[CRITICAL][error-handling-lifecycle] "validate/is_json hand-rolled parser recurses without a depth limit — query-reachable stack overflow aborts the process"** — `src/validate.rs:123-191`: ~10⁵–10⁶ nested `[` from one scalar call overflows the thread stack; stack overflow is an uncatchable abort that defeats the workspace's `panic="unwind"` per-connection isolation. (Same root filed [HIGH] in security-crypto.)
- **[HIGH][security-crypto] "validate/is_json hand-rolled parser recurses without a depth cap — stack-overflow abort from a ~20 KB query string"** — `validate.rs:123-191` (dup of the above; security framing).
- **[HIGH][error-handling-lifecycle] "Unbounded-allocation scalar paths: strings/repeat, strings/pad_left/pad_right, gen/random_bytes"** — `strings.rs:229-242, :406`; `gen.rs:70-78`: attacker-chosen `i64` lengths allocate in one shot → capacity-overflow panic / allocator abort killing the whole server; `argon2id`'s cap discipline was never applied here.
- **[HIGH][security-crypto] "Unbounded attacker-sized allocations → allocator abort in strings/repeat, strings/pad_left, strings/pad_right, gen/random_bytes"** — `strings.rs:237, :406`; `gen.rs:75` (dup of the above; security framing).
- **[HIGH][api-wire-protocol] "Query-reachable scalars allocate unbounded memory (random_bytes, repeat, pad)"** — `gen.rs:70-77`; `strings.rs:228-242, :389-411` (third report of the same exposure class).
- **[HIGH][concurrency-lockfree] "Global std::sync::Mutex regex cache on the hot filter path — regex compiled while holding the lock"** — `src/strings.rs:417-434`: all 8 regex scalars (per-row filter predicates) take one process-global mutex and run `Regex::new` (user-pattern-driven, ms–s) inside it, serializing all queries.
- **[HIGH][performance-hotpath] "validate/matches recompiles the regex on every call"** — `src/validate.rs:336-348`: per-row `Regex::new` — 100k-row scan = 100k NFA compilations, ignoring both in-crate caching patterns.
- **[HIGH][performance-hotpath] "count_distinct aggregator is O(N·C) — the exact legacy pattern arrays::distinct was fixed to remove"** — `src/agg.rs:197-215`: per-row linear scan over the grow-only `seen` vec; 1M-row all-distinct column ≈ 5×10¹¹ compares.
- **[HIGH][error-handling-lifecycle] "UserScalarLayer::register discards scc's Err — documented '(or replace)' silently never replaces"** — `src/scalar_resolver.rs:39-42`: `let _ = insert_sync(...)` eats scc's Err-on-existing-key, so `CREATE OR REPLACE FUNCTION`-style re-registration silently keeps the old implementation (module has zero tests).

### shamir-engine (12 high)

- **[HIGH][correctness-tdd] "StoreChangelog::range_from streams the ENTIRE journal tail before applying limit"** — `src/repo/changelog_store.rs:37-55`: replication catch-up buffers + sorts every event ≥ `from_key` before truncating to `limit` — O(N) RAM per poll on an unretained journal (OOM/DoS).
- **[HIGH][correctness-tdd] "Drainer Phase B silently drops Put/Delete for tables with no MvccStore — and its justification comment is false"** — `src/tx/drainer.rs:419-437, :504-519`: warm-path drain never re-applies data ops for unattached tables yet finalizes the entry as durable — permanent data loss if the WAL marker truncates.
- **[HIGH][concurrency-lockfree] "DashMap shard read-guard held across .await in DbInstance accessors"** — `src/db_instance/db_instance.rs:61-68` (+7 sites): shard `Ref` held across `get_table`/`create_index` (incl. minute-long online backfills) while writers block worker threads on the shard RwLock — the exact wedge class the crate fixed in `repo_instance.rs`.
- **[HIGH][performance-hotpath] "O(N²·K) re-planning scans inside rederive_stale_value_ops_post_stage"** — `src/tx/pre_commit.rs:1999-2329`: per-staged-row rebuild + linear rescans of `index_write_set` inside the locked pre-commit phase, under any concurrent write traffic.
- **[HIGH][performance-hotpath] "Staged-overlay probe clones and linearly scans the whole tx write-set per validated record"** — `src/validator/validator_db.rs:312, :427-440` (+`staging_store.rs:172-180`): per-unique/FK probe pays O(M) clone+scan → O(M²) on batch inserts.
- **[HIGH][performance-hotpath] "FK RESTRICT: one full child-table scan per parent value, values not deduplicated"** — `src/query/batch/fk_restrict.rs:145-164`: unindexed child column → per-value full table scan, multiplied by duplicate parent values.
- **[HIGH][performance-hotpath] "Changelog range_from buffers the entire journal tail, then truncates to limit"** — `src/repo/changelog_store.rs:37-55` (dup of correctness #1, perf framing).
- **[HIGH][performance-hotpath] "ON UPDATE discovery: repo-wide table scan on EVERY UPDATE, before the no-op gate"** — `src/query/batch/fk_on_update.rs:734-783`: O(tables) schema walk per UPDATE op, paid even when no FK-referenced field changes.
- **[HIGH][api-wire-protocol] "Exported hand-written query parser speaks a dead wire dialect and silently drops query semantics"** — `src/query/read/parser.rs:14-78`: builder-shaped `{"pagination": {...}}` queries fall to `Pagination::None` — a `.limit(20)` read returns the entire table; temporal/version/explain silently dropped.
- **[HIGH][error-handling-lifecycle] "Write-path pre-reads swallow all read errors as 'record does not exist'"** — `table_manager_crud.rs:431, :511`; `table_manager_tx_ops.rs:1075`: `.ok()` conflates I/O errors with absence → silent no-op deletes / insert-shaped re-plans with stale unique postings.
- **[HIGH][style-claude-md] "mod.rs contains a full implementation, not re-exports"** — `src/repo/group_commit/mod.rs:1-128`: the only logic-bearing `mod.rs` in the crate (bright-line CLAUDE.md breach).
- **[HIGH][style-claude-md] "Systemic mid-function `use` imports (~25 sites, ~15 production files)"** — multiple files (e.g. `migration/shadow_log.rs:50,109,129` duplicates the same import three times): imports-at-top rule violated at scale.

### shamir-db (10 high)

- **[HIGH][correctness-tdd] "DROP DATABASE ... CASCADE executed against a different database destroys the *batch's* database's tables"** — `execute/admin_db_repo.rs:126-141`: cascade passes the batch's `db_name` (not `op.drop_db`) to `drop_table_cleaning_validators`, so `drop_db("B")` inside a batch on "A" destroys A's tables/validator bindings.
- **[HIGH][correctness-tdd] "remove_group_member on a nonexistent group *creates* a phantom group record (and the wire remove path lacks the existence guard the add path has)"** — `system_store.rs:742-762`: `unwrap_or_default()` + unconditional `save_group` fabricates an empty-name group for any absent id; dispatcher guards the add path only.
- **[HIGH][concurrency-lockfree] "execute_as re-runs the full async ACL traversal per batch op — missing the inline dedup cache its sibling tx_execute_as already established"** — `execute/db_execute.rs:64-68`: every op re-pays 4–6+ system-store catalogue reads; the `tx_execute_as` FxHashMap cache was never ported back.
- **[HIGH][security-crypto] "Guest-controlled header values/method can inject arbitrary curl config directives (CRLF injection)"** — `shamir_db/curl_gateway.rs:83-89, :210-220`: `escape_curl_value` handles only `` and `"`, so a header value containing `\nproxy = ...` bypasses SSRF pinning to attacker-chosen proxies / file writes.
- **[HIGH][performance-hotpath] "ACL gate runs full catalogue scans per ancestor per op — O(ops × ancestors × catalogue) per request"** — `system_store.rs:808/828/860/687/613/484/1036/1131` + `access_control.rs:41-239, :849-908`: every "point" lookup is a filtered full scan (no indexes); a 10k-row catalogue ≈ 5×10k decodes per authorization.
- **[HIGH][performance-hotpath] "execute_as re-authorizes every op in a batch without dedupe"** — `execute/db_execute.rs:64-68` (perf framing of the concurrency finding): batched workloads are quadratic in practice.
- **[HIGH][api-wire-protocol] "replace=true on a WASM validator destroys persisted binding bookkeeping and can silently re-key its identity"** — `shamir_db/validator_management.rs:248-252, :282-285`: replace wipes `bound_in` (registry + catalogue) and can mint `RecordId::default()`, re-enabling an unsafe drop and orphaning table bindings.
- **[HIGH][api-wire-protocol] "Dead, exported api::{Command, Request, Response} wire shim that no server speaks"** — `src/api/types.rs:7-41`: a plausible-looking zero-consumer wire envelope invites SDK/FFI clients no shamir-server build will ever answer.
- **[HIGH][error-handling-lifecycle] "Catalogue-persistence failures are swallowed (warn! + continue) across the DB/repo/table lifecycle, so multi-step mutations can return Ok(()) half-migrated"** — `db_management.rs:184-201, :58-64, :396-426` + `table_management.rs:56-74, :323-350`: rename with a failed save-new still removes-old → catalogue holds no record while the caller got `Ok(())`; repos vanish after reboot with zero signal.
- **[HIGH][error-handling-lifecycle] "rename_function_as / rename_validator_as / rename_function_folder_as destroy the durable record *before* writing the new one (remove-before-write)"** — `function_management.rs:335-347`; `validator_management.rs:373-385`: the only rename paths inverting the crate's write-new-before-remove-old crash-safety convention — a failed save leaves the function under neither name after restart.

### shamir-types (5 high)

- **[HIGH][correctness-tdd] "F64(0.0) and F64(-0.0) compare equal but hash differently — Hash/Eq contract violated, and a test locks it in"** — `src/types/value.rs:697-711, :293-299` (+ test `value_tests.rs:525-530`): `0.0 == -0.0` but bits differ → `TSet<Value>` membership/dedup wrong; a test asserts the buggy behavior.
- **[HIGH][security-crypto] "Unbounded preallocation from attacker-controlled msgpack array/map headers (zerocopy decoder)"** — `codecs/interned/messagepack.rs:305, :318, :581-585`: a 5-byte `Array32 0xFFFFFFFF` header drives `Vec::with_capacity` to hundreds of GB → allocator abort DoS on the WAL-replay/S-write decode path (the `SANE_PREALLOC_CAP` fix never left the serde visitor).
- **[HIGH][error-handling-lifecycle] "Unbounded preallocation from attacker-controlled msgpack headers — allocator abort, unlike the capped tree visitor"** — `messagepack.rs:305, :318, :581-585` (second report of the same hole).
- **[HIGH][performance-hotpath] "Projection codec re-scans the whole record once PER selected field id — O(fields x selected) per row"** — `codecs/interned/projection.rs:62-67` + `record_view/lens.rs:1131-1168`: per-row projection bypasses `FieldIndex`, O(k·f) marker reads on the S-read path.
- **[HIGH][api-wire-protocol] "Wire format cannot represent Dec / Big / Set — types silently degrade to Str / List on every encode"** — `types/value.rs:72-73`; `messagepack.rs:375-376, :954-955`: no round-trip; `WHERE price = 123.45::dec` never matches a row written through a Dec literal; pre/post-reload equality silently changes shape.

### shamir-collections (1 high)

- **[HIGH][correctness-tdd] "Entirely untested crate — every documented behavioral contract has zero regression protection"** — `src/lib.rs:1-64`: the pillar-4 anchor (`THasher`, ordering, `_wc` capacities, serde round-trip) has zero tests anywhere; a hasher/ordering regression compiles and passes `@types` silently.

### shamir-storage (7 high)

- **[HIGH][correctness-tdd] "MemBufferStore::get_many cache-fill can poison a Tombstone over a concurrent write — the #539 bug class survives in the vectored read"** — `storage_membuffer.rs:1243-1260`: no dirty recheck before `cache.insert` → read-your-write broken permanently for the key (lasting mask until eviction).
- **[HIGH][concurrency-lockfree] "CachedStore: unordered, non-atomic cache-mutation sites can leave the cache permanently behind inner (silent acked-write loss, size-counter drift)"** — `storage_cached.rs:403-411, :427-467, :469-485, :151-168`: three uncoordinated mutation sites → acked `Ok` writes invisible to readers indefinitely.
- **[HIGH][concurrency-lockfree] "MemBufferStore::get_many is missing the #539 tombstone-poisoning guard its sibling get() has"** — `storage_membuffer.rs:1243-1261` (dup of correctness #1; the window spans the whole backend round-trip and covers Live values too).
- **[HIGH][performance-hotpath] "Reverse range streams drain the ENTIRE range into RAM before reversing; not overridden by InMemory/Cached/Mirrored"** — `types.rs:376-384, :391-412`: DESC/top-K/max queries on three backends pay O(range) time *and* memory even for K=1.
- **[HIGH][performance-hotpath] "InMemoryStore iter_stream / scan_prefix_stream eagerly materialize the whole corpus before the first yield"** — `storage_in_memory.rs:153-159, :240-247`: full collect under a pinned epoch guard (the 2026-07-06 fix never reached this backend).
- **[HIGH][api-wire-protocol] "Persisted MemBufferConfig wire format has no versioning guardrails despite a 'stable wire-format' claim"** — `storage_membuffer.rs:92-126`: no serde defaults/version field/golden test on an on-disk DDL blob; a future field change breaks every existing database at open.
- **[HIGH][error-handling-lifecycle] "CachedStore::flush() can hang forever if the async write-worker task dies before draining"** — `storage_cached.rs:68-113, :243-249, :383-399`: discarded `JoinHandle`; an inner panic/shutdown-drop leaves `pending_writes != 0` and every later `flush()` parked indefinitely.

### shamir-wal (5 high)

- **[HIGH][correctness-tdd] "Circuit breaker can strand parked appenders indefinitely (L1 violation, hang)"** — `wal_group_commit.rs:327-330`: the breaker exit releases `flushing` outside the `pending` lock without draining → committers that pushed during the failed window park forever on a quiescent system.
- **[HIGH][concurrency-lockfree] "Circuit-breaker exit strands parked waiters indefinitely (no wakeup, no self-rescue)"** — `wal_group_commit.rs:324-330` (dup; concurrency framing).
- **[HIGH][error-handling-lifecycle] "Cancellation or panic of the leader task wedges flushing forever — permanent WAL append hang"** — `wal_group_commit.rs:180-186, :273-275, :327-330`: the leader runs inline on the caller's task with no Drop guard; a cancelled leader leaves `flushing=true` permanently.
- **[HIGH][error-handling-lifecycle] "Seal-time fsync failure fails an already-successful append whose frames survive — 'acked-failed' tx resurrected on replay"** — `segment_set.rs:247-253, :367`: rotation-boundary fsync error converts a written batch to `Err` while the poisoned segment's prefix stays replayable — violates the §1.6 all-or-nothing contract (Buffered waiters told "failed" then resurrect).
- **[HIGH][style-claude-md] "Inline #[cfg(test)] mod tests in an implementation file"** — `segment_meta.rs:175-218`: the sole bright-line test-layout breach in the crate (rated high by its reviewer).

### shamir-tx (7 high)

- **[HIGH][correctness-tdd] "finalize_reservation is not max-monotonic — the ack-path publish can regress a cell below a newer committed version"** — `mvcc_store/mod.rs:664-679`: the live ack publisher lacks the A2 strictly-greater guard → stale "current" reads and masked SSI conflicts under out-of-order Phase-5a (Snapshot txs run without `commit_lock`).
- **[HIGH][concurrency-lockfree] "publish_committed plain store() can regress last_committed_version; its safety contract is unsound"** — `repo_tx_gate.rs:576-589` (latent, zero production callers): `commit_lock` does not establish global monotonicity vs non-tx `fetch_max` writers; an acked write can become invisible.
- **[HIGH][performance-hotpath] "vacuum_key: unbatched + duplicated per-version I/O on the write hot path"** — `mvcc_gc.rs:100-121, :229-250`: every write pays up to 3 sequential round-trips fast-path, up to 4×versions scan-path, with a duplicated `lookup_ts` per reclaimed version.
- **[HIGH][performance-hotpath] "gc_below / purge_below_ts materialise the whole history store before deleting"** — `mvcc_gc.rs:305-322, :396-473`: entire history buffered in `TFxMap<Vec<u8>, Vec<...>>` per GC tick — O(total history) RSS spike.
- **[HIGH][api-wire-protocol] "Durable journal events have no schema/version envelope; decode failures are silently skipped"** — `changefeed.rs:86-101, :542-546, :409-411`: bare msgpack into a durable journal + warn-and-skip decode → a format change silently deletes events from replication with `gap_at: None`.
- **[HIGH][error-handling-lifecycle] "Failed history.transact leaves RecordCell advanced with no rollback — prior version permanently masked on point reads"** — `mvcc_store/mod.rs:766-832, :853-938, :1035-1086`: publish-before-log without compensating restore → a failed SET reads as deleted; a failed DELETE appears to have succeeded in-process then resurrects after restart.
- **[HIGH][style-claude-md] "mvcc_store/mod.rs is a full implementation file, not a re-export manifest"** — `mvcc_store/mod.rs:1-1638`: the crate's most-edited type (struct + ~1,400-line impl) lives in the one file reserved for wiring.

### shamir-index (10 high)

- **[HIGH][correctness-tdd] "FunctionalBackend hash collapses every Dec/Big/Bin value to an identical posting hash"** — `functional_backend.rs:173`: catch-all `_ => h.write_u8(255)` → all distinct decimals/bigints/binary share one posting key — point queries return every row.
- **[HIGH][correctness-tdd] "Whitespace/Full tokenizers never case-fold words whose uppercase letters are all non-ASCII (Russian/Greek FTS broken)"** — `tokenizer.rs:55-57, :277-284`: `Москва` is indexed un-lowercased; `москва` queries never match (the Unicode-aware path exists only in `lowercase_cow`/UnicodeTokenizer).
- **[HIGH][correctness-tdd] "Vector delta-replay failure is warned away, then permanently baked in by the next background snapshot"** — `vector/vector_backend.rs:683-698` + `snapshot.rs:1287-1330`: one corrupt delta chunk → warn-and-continue, then the generation flip prunes the chunk — affected rows vanish from vector queries permanently.
- **[HIGH][concurrency-lockfree] "Background vector snapshot dumps the live adapter without quiescing; the multi-map sidecar scan is not atomic across maps (torn capture → permanent zombie graph node)"** — `vector_backend.rs:891-934` + `snapshot.rs:522-549`: four independent scans at four instants; a rid replacing upsert interleaved between them persists a double-surfacing zombie (the #402 quiesce assumption is stale).
- **[HIGH][performance-hotpath] "Per-row deep-clone of the whole index-definition set in every write planner"** — `base_index/index_info.rs:310-315`; `sorted_index_manager.rs:544-550` (+21 call sites): every insert/update/delete pays ~I×(P+1) allocations to read DDL-time definitions.
- **[HIGH][performance-hotpath] "Sorted/unique apply paths issue one store round-trip per posting — the transact batching landed only for the regular-hash family"** — `sorted_index_manager.rs:1847-1864`; `index_manager_unique.rs:473-554`: N sequential (fsync-bound) round-trips per row write where the hash family pays 1.
- **[HIGH][api-wire-protocol] "Persisted VectorConfig.backend is ignored on the reopen/rebuild path"** — `build_backend.rs:52-65`: hardcoded m=16/ef=200; `External` drivers silently rebuilt in-process; recall silently degrades and the wrong params are re-persisted.
- **[HIGH][api-wire-protocol] "SQ8 quantization opt-in has no durable carrier and is lost on most restarts"** — `kind.rs:168-186`; `build_backend.rs:53-63`; `hnsw_adapter.rs:526-549, :1292-1296`: `#[serde(skip)]` + `from_parts` hardcoding `quantization: None` → silent ~4× memory regression; `from_parts` adapters can never fit.
- **[HIGH][error-handling-lifecycle] "SortedIndexManager::load swallows ALL store errors, silently loading zero sorted definitions"** — `sorted_index_manager.rs:2706-2709`: blanket `Err(_) => Ok(())` treats I/O errors as "no sorted indexes"; the next DDL persists the empty set, permanently destroying definitions.
- **[HIGH][error-handling-lifecycle] "Compaction double-write errors silently discarded; an incomplete graph is then swapped in as live"** — `vector/vector_backend.rs:295-297, :316-318, :511-513` + swap at `:1102-1104`: `let _ = target.adapter.upsert(...)` → post-compaction primary silently missing vectors, zero signal.

### shamir-query-types (7 high)

- **[HIGH][correctness-tdd] "BatchOp::ForEach missing from is_admin() while Batch is included — gate-bypass-shaped classification asymmetry, unpinned by any test"** — `batch/batch_op.rs:634`: server's top-level-only superuser gates skip ForEach-wrapped admin ops (`DropDb`/`GrantRole`) for non-superusers; downstream containment verified but the gate shape is bypassed.
- **[HIGH][security-crypto] "No parse-time depth bound on recursive DTO deserialization — remote stack-overflow abort"** — `filter/filter_enum.rs:9,127`; `filter_value.rs:46,70-74`; `batch/batch_op.rs:262`: ~100k-deep msgpack nesting aborts the decode thread (whole server) before `MAX_FILTER_DEPTH` can run.
- **[HIGH][performance-hotpath] "BatchOp::deserialize — triple codec round-trip + key clones + linear dispatch chain per op"** — `batch/batch_op.rs:256-284, :287-438`: buffer → re-encode → re-decode per op (~75 sequential `has()` probes, "set" last), multiplied per nested batch level.
- **[HIGH][api-wire-protocol] "InsertedRecord::get_value_owned(\"_id\") returns None for every deserialized record, contradicting its own doc"** — `write/inserted_record.rs:81-104, :114-119`: the documented client-side `_id` access path is dead exactly where clients need it (pagination `after_id` chaining).
- **[HIGH][api-wire-protocol] "FilterValue silently coerces msgpack uint64 > i64::MAX to lossy Float — asymmetric with the crate's own u64 contract"** — `filter/filter_value.rs:9-81`: `u64::MAX` becomes a lossy f64; equality filters silently never match (response side implements the lossless `Big` contract; request side doesn't).
- **[HIGH][style-claude-md] "Types defined inside mod.rs (re-export-only rule breach)"** — `validator/mod.rs:5-30`; `call/mod.rs:13-43`: `WriteOp`/`ValidationError`/`CallOp` defined in manifests.
- **[HIGH][style-claude-md] "Inline #[cfg(test)] mod tests { ... } embedded in implementation files"** — `read/query_record.rs:302-434`; `write/inserted_record.rs:134-214`: wire-contract tests duplicated against existing `tests/` files.

### shamir-query-builder (3 high)

- **[HIGH][correctness-tdd] "Batch::try_build falsely rejects sub_batch/for_each entries whose inner batch has internal $query dependencies"** — `batch/batch.rs:1333-1345`: the msgpack fallback descends into inner batches and checks inner aliases against the OUTER set, contradicting the planner's documented scoping — callers drop to unvalidated `build()`.
- **[HIGH][performance-hotpath] "Doc::set does a full msgpack round-trip per field — the crate's per-row hot loop"** — `write/doc.rs:43-53`: N×F rows pay 2N·F codec passes + allocations when a typed converter already exists; hits WASM guests hardest.
- **[HIGH][api-wire-protocol] "Batch::try_build false-rejects valid nested sub_batch / for_each batches with inner $query refs"** — `batch/batch.rs:1333-1345` (dup of correctness #1).

### shamir-connect (8 high)

- **[HIGH][correctness-tdd] "TOFU pin callback fires before Ed25519 identity verification completes"** — `client/handshake.rs:264-294`: a MITM who proxies SCRAM can get its key pinned before `verify_identity` fails → persistent MITM on all later connections.
- **[HIGH][concurrency-lockfree] "Concurrent callers share the pre-fetch_max refill watermark and multiply refill by racer count — documented invariant does not hold"** — `server/session.rs:353-358`: k racers each credit the same elapsed span → one session sustains ~64× its post-auth rate limit; #1090 fixed the sibling site only.
- **[HIGH][concurrency-lockfree] "SessionStore::cap_lock: unjustified parking_lot::Mutex on the per-auth hot path, held across an O(all-sessions) full-store scan"** — `server/session.rs:416, :470-493`: every SCRAM auth serializes behind one global lock scanning all sessions (banned primitive + hidden O(N), no contention-model comment).
- **[HIGH][performance-hotpath] "Per-user session-cap insert does an O(total-sessions) full-map scan under a global mutex on every login"** — `server/session.rs:470-497` (perf framing of the same site).
- **[HIGH][performance-hotpath] "AuditChain accumulates every audit event in an ever-growing in-memory Vec (unbounded growth)"** — `server/audit_chain.rs:140, :214`: per-auth-event deep-cloned entries retained for process lifetime; the doc's "override with a streaming writer" is unreachable via any API.
- **[HIGH][api-wire-protocol] "client feature cannot build without server — client modules unconditionally import crate::server::*"** — `Cargo.toml:16-19` + all of `src/client/`: the README-advertised client-only build fails with E0433.
- **[HIGH][error-handling-lifecycle] "FjallConsumedCounters::try_advance conflates persistence failure with replay, mutates state before durability, and logs nothing"** — `server/durable_counters.rs:115-151`: a persist error at the fsync step bricks the ticket family (counter journalled first, retried resume now fails) with zero diagnostics.
- **[HIGH][style-claude-md] "dispatch_request_view doc claims 'functionally identical' but only it enforces the post-auth rate gate"** — `server/dispatch.rs:112-117` vs `:69-110`: the exported owning variant silently drops task-#608 rate limiting while the doc asserts equivalence (also filed [MED] in security/api/error themes).

### shamir-wasm-host (7 high)

- **[HIGH][correctness-tdd] "Aggregate fuel budget is enforced retroactively — in-flight chain can draw (depth+1) × fuel; doc overclaims the bound"** — `wasm/wasm_function.rs:433-447, :583-587`: grant-on-entry/debit-at-exit means nested `ctx.call` chains each draw a full grant (~33×10⁹ instructions default) — the #612 aggregate bound doesn't hold for the descent it was built for.
- **[HIGH][correctness-tdd] "fuel > i64::MAX immediately errors AND makes the wall-clock/epoch test vacuous"** — `wasm_function.rs:436, :438-443` + `wasm_tests.rs:239-268`: `fuel: u64::MAX` wraps the AtomicI64 seed to −1 (every invocation rejected), and the dedicated epoch-interruption test passes microseconds in on this early return.
- **[HIGH][correctness-tdd] "wasm_aggregate_fuel_exhausted_across_nested_calls cannot distinguish aggregate from per-Store fuel — vacuous regression test"** — `wasm_tests.rs:270-317`: the #612 regression test passes with or without the fix.
- **[HIGH][concurrency-lockfree] "Aggregate cross-Store fuel budget does not bound the nested-call descent (grant loaded before ancestors debit)"** — `wasm_function.rs:438-447` (dup of correctness #1).
- **[HIGH][concurrency-lockfree] "compile_rust_source blocks up to 120 s and is called directly on tokio workers (pillar 2)"** — `compile.rs:454-456` (+ async call sites `shamir-db/function_management.rs:172`, `validator_management.rs:221`): `CREATE FUNCTION ... FROM SOURCE` pins a worker for the whole cargo build; on a 1-worker runtime everything freezes.
- **[HIGH][security-crypto] "Forbidden-macro scan bypassed by whitespace/comment between macro name and `!`"** — `compile.rs:361-371`: `env !"HOME"` / `include_str !"C:/.../credentials"` are legal Rust but invisible to the scanner — host file reads baked into the artifact (CRIT-6 control bypassed).
- **[HIGH][api-wire-protocol] "HTTP wire codec collapses duplicate headers (Set-Cookie loss on both directions)"** — `wasm/host_http.rs:86-97, :39-65` (+`shamir-sdk/src/http.rs`): string-keyed map wire shape cannot represent repeated headers — cookie-jar auth silently breaks with no error.

### shamir-client (9 high)

- **[HIGH][correctness-tdd] "Connection death never closes subscription channels — SubscriptionHandle::next() hangs forever"** — `client.rs:318-327` + `subscription.rs:54-65`: reader exit drains only `pending`, never `subscriptions` — consumers hang permanently after server restart.
- **[HIGH][correctness-tdd] "batch_has_refs misses two ref carriers: QueryEntry.when guards and BatchOp::ForEach"** — `interner_cache_ops.rs:513-532`: `execute_with_touch` mis-decodes when/for-each batches as `Id` encoding → `$query` path resolution silently breaks (the exact bug the guard was built to prevent).
- **[HIGH][correctness-tdd] "resume() never verifies server identity — server_pub_key_pin() returns unverified caller input"** — `client.rs:588-677` (esp. :663): accept-any-cert TLS + no identity material in `WireResumeOk` → a MITM fabricates a resume and the client proceeds; the pin field's documented invariant is silently violated.
- **[HIGH][concurrency-lockfree] "Reader-exit drain races pending registration → permanent hang of an in-flight request"** — `client.rs:966, :993-998, :318-327`: closed-check/insert vs store/drain is unordered; with the default `request_timeout: None` the request hangs forever.
- **[HIGH][concurrency-lockfree] "std::sync::Mutex on the per-request hot path (PendingMap) without a sanctioned-category justification"** — `client.rs:161, :996, :302`: locked twice per request/response on the hottest SDK path; banned primitive, no contention-model comment (scc is already a dependency).
- **[HIGH][security-crypto] "Client::resume hands the bearer ticket to an unverified peer — no server-identity check exists on the resume path"** — `client.rs:588-677, :591-629, :663` (security framing of the resume hole; the ticket is the first payload disclosed).
- **[HIGH][performance-hotpath] "roundtrip clones the whole serialized request per call and ignores the zero-copy envelope built for exactly this path"** — `client.rs:975-989`: `buf.clone()` + owning envelope + session-id Vec = 3 allocs/2 full copies per request; `RequestEnvelopeRef` was built for this and is never used.
- **[HIGH][api-wire-protocol] "query_version: 2 stamped unconditionally — the server_query_version negotiation is parsed but never applied to the request version"** — `client.rs:786-790`: the entire graceful v1-fallback ladder is dead code; every request against a pre-v2 server fails `unsupported_query_version`.
- **[HIGH][error-handling-lifecycle] "Race between roundtrip's closed-check and the reader shutdown drain hangs the caller forever under default options"** — `client.rs:966, :993-998, :318-326` (error-handling framing of the same race).

### shamir-sdk (6 high)

- **[HIGH][correctness-tdd] "No tests for http.rs, params.rs, db.rs mapping logic, or __rt ABI helpers (TDD protocol not followed for most of the crate)"** — `src/http.rs`, `src/params.rs`, `src/db.rs:86-109`, `src/__rt.rs`: wire-shape, packing-convention (`ptr<<32|len`), and silent-loss branches all unpinned; `Table` mappings are untestable by construction (no seam).
- **[HIGH][concurrency-lockfree] "__rt::block_on busy-spins on Poll::Pending with a no-op waker; the 'pure functions only' guard is a stale comment, not an enforced invariant"** — `src/__rt.rs:36-61`: a guest that awaits anything genuinely-pending spins at 100% CPU forever (no waker can ever fire) — wedges the host worker; all four macros route through it.
- **[HIGH][performance-hotpath] "Host-import ABI leaks both directions' buffers on every call — unbounded guest linear-memory growth in loops"** — `host_imports.rs:60-66` (+7 sites, `__rt.rs:25-30`): a bulk procedure leaks ~2× cumulative traffic inside one invocation until wasm OOM.
- **[HIGH][api-wire-protocol] "Raw-Value filter surface (Table::get/Table::query) bypasses the builder-only rule, with no required exception comment"** — `src/db.rs:5-9, :79-109`: non-gated hand-built `Value` filters with silently-lossy semantics (Dec/Big/List → `FilterValue::Null` → silent no-matches).
- **[HIGH][error-handling-lifecycle] "Host-response decode failures silently become wrong data (empty results / None / Null)"** — `host_imports.rs:97, :106, :131, :146, :162, :183, :207`: a truncated host reply reads as "no rows"/"absent"/"null" — fail-open on the core data path.
- **[HIGH][error-handling-lifecycle] "No error taxonomy: single-message Error, Error::user used for infra failures, and trap transport flattens user errors into Compute"** — `error.rs:6-23` + `__rt.rs:64-69` + macro transport: user errors and guest crashes are indistinguishable `FunctionError::Compute` at the host.

### shamir-query-builder-macros (3 high)

- **[HIGH][correctness-tdd] "Malformed doc-map / call-args / select-fn args are silently truncated, not rejected (missing content.is_empty() checks)"** — `src/query_parse.rs:576-587, :685-700, :375-436`: a missing comma in `q!(insert into users values {"name" => "Alice" "age" => 30})` compiles and silently inserts without `age` — lossy write miscompilation with no diagnostic.
- **[HIGH][api-wire-protocol] "Silent token-drop in DSL sub-parsers miscompiles write ops (missing-comma in doc maps, call args, select-item args)"** — `src/query_parse.rs:572-588, :689-696, :409-431` (dup; api framing).
- **[HIGH][error-handling-lifecycle] "No error-path test coverage for any diagnostic branch of filter! / q!"** — crate-wide: ~25 `syn::Error` sites, zero tests, no trybuild anywhere in the workspace — a refactor can flip rejections into acceptances with a green suite.

### shamir-numa (3 high)

- **[HIGH][correctness-tdd] "NodeReplicated::rcu mirror loop can permanently strand non-node-0 replicas on a stale value under concurrent writers"** — `src/node_replicated.rs:96-108`: loser's mirror lands last → non-node-0 replicas diverge indefinitely (until the next unrelated write), contradicting the "few nanoseconds / eventual consistency" doc; live in shamir-index.
- **[HIGH][concurrency-lockfree] "rcu/store mirror phase can overwrite newer replicas with a stale value — non-zero nodes diverge from node 0 indefinitely"** — `node_replicated.rs:96-108, :82-87` (dup; includes plain `store` interleavings; the concurrency test asserts node 0 only, so CI is blind).
- **[HIGH][api-wire-protocol] "Documented consistency contract of NodeReplicated is wrong under concurrent writers — a replica can diverge permanently"** — `node_replicated.rs:20-30` (dup; contract framing).

### shamir-transport-ws (3 high)

- **[HIGH][correctness-tdd] "accept_browser_ws — the spec §9 Origin enforcement path — has zero test coverage"** — `src/server.rs:108-146`: the primary anti-CSWSH control is wired by no test anywhere in the workspace; a refactor can silently disable it green.
- **[HIGH][api-wire-protocol] "Spec-mandated WebSocket subprotocol negotiation is unimplemented"** — `src/server.rs:85-99, :120-143`: spec §2.1 requires echoing `Sec-WebSocket-Protocol: shamir-v1` (mismatch → 400); conformant browser clients fail to connect and downgrade defenses never fire.
- **[HIGH][api-wire-protocol] "Endpoint paths hardcoded as string literals; no shared constants; incompatible with the server's configurable path"** — `src/server.rs:90, :124`: an operator-configured ws path boots cleanly then 404s every upgrade — total silent connectivity loss on that listener.

### shamir-transport-tcp (1 high)

- **[HIGH][security-crypto] "read_frame_into: unsafe set_len before .await — formal UB on &mut [u8] over uninit memory, and cancellation leaves buf.len() covering uninitialized bytes"** — `src/framing.rs:117-136` (unsafe at :124-128): creation of `&mut [u8]` over uninit capacity is UB, and a cancelled future (exactly what shamir-server's `select!`/`timeout` do around this call) leaves the poisoned buffer with no cleanup — potential stale-heap disclosure for the next caller that inspects it.

### shamir-sdk-macros (2 high)

- **[HIGH][correctness-tdd] "Zero test coverage — TDD protocol not honored; even pure helpers are untested"** — crate-wide: 572-line proc-macro crate with no `tests/`, no `#[cfg(test)]`, no trybuild; two of four macros have no compile coverage anywhere in the workspace.
- **[HIGH][api-wire-protocol] "Return-type validation is string-based, per-macro inconsistent, and rejects valid spellings"** — `src/lib.rs:63-72, :193-202, :408-420`: `-> shamir_sdk::Result<Value>` is accepted by `#[procedure]`/`#[scalar]` and rejected by `#[function]`; `shamir_sdk::Validation` rejected by `#[validator]` — the repo's own tests use the rejected spellings.

### shamir-bench-utils (1 high)

- **[HIGH][style-claude-md] "Entire test module embedded inline in vector_data.rs, violating the mandatory tests/ layout"** — `src/vector_data.rs:217-363`: the crate's only test suite (~150 lines, 9 tests) lives inline, institutionalized by a self-granted Cargo.toml comment.

### shamir-tunables (1 high)

- **[HIGH][correctness-tdd] "Runtime override API is unwired — all three setters are silent no-ops for real behavior"** — `src/runtime.rs:36-76`: `RuntimeTunables` is constructed and published on the public `ServerHandle::tunables` field, but every production read goes to the compile-time consts — an operator's override is silently inert while the getter dutifully reports the new value.

## Prioritized Remediation Roadmap

Scope basis: the 122 critical/high findings plus the catalogued mediums/nits. P0 is deliberately tight — 10 entries, each severe **and** reachable from either untrusted input or routine production events (concurrent load, restarts, disk-full, ordinary DML).

### P0 — Fix before next release (severe + concretely reachable)

1. **shamir-funclib:** `validate/is_json` unbounded recursion — one low-priv query aborts the whole process (uncatchable, defeats `panic="unwind"` isolation)
2. **shamir-funclib:** `random_bytes`/`repeat`/`pad` unbounded allocations — allocator abort from a single query (same dispatch path as #1; fix together)
3. **shamir-server:** `TxBegin`/`TxExecute`/`TxCommit` bypass the read-only-replica gate — silent split-brain writes on every replica deployment
4. **shamir-wal:** group-commit circuit breaker strands parked committers — permanent commit-path hang after any write/fsync failure (ENOSPC is routine)
5. **shamir-wal:** leader cancellation/panic wedges `flushing` forever — commit hang until restart; no RAII release
6. **shamir-storage:** `MemBufferStore::get_many` missing the (proven, #539) tombstone guard — permanent read-your-write break under ordinary concurrent writes
7. **shamir-storage:** `CachedStore` unordered non-atomic cache upserts — acked `Ok` writes invisible to readers indefinitely
8. **shamir-client:** reader exit never closes subscription channels — every live subscriber hangs forever on any disconnect (unconditional, no race needed)
9. **shamir-db:** `DROP DATABASE ... CASCADE` executes against the *batch's* database — silently destroys the wrong db's tables and validator bindings
10. **shamir-db:** curl-gateway CRLF injection via guest-controlled headers — internal-network SSRF / config-directive injection reachable by ordinary function callers (FunctionNamespace Create defaults open)

### P1 — Fix soon (real correctness / perf bugs, known trigger)

#### P1.a — Data correctness / silent wrong results

- shamir-wal: seal-time fsync failure reports a durable batch as failed — "acked-failed" tx resurrects on replay (h)
- shamir-engine: drainer Phase B silently drops Put/Delete for tables without an MvccStore (h)
- shamir-engine: write-path pre-read `.ok()` conflates I/O error with absent row — wrong delete/update plans, stale postings (h)
- shamir-db: rename paths remove-before-write and swallow catalogue errors — silent record loss on store fault (h ×2)
- shamir-db: `remove_group_member` fabricates phantom group records (h)
- shamir-db: validator `replace=true` wipes `bound_in` and can re-key identity (h)
- shamir-db: `save_database` + replication writes skip the Durable-DDL flush (m)
- shamir-db: FK scans silently skip decode-corrupt parent rows (m)
- shamir-tx: failed `history.transact` leaves the cell advanced — record reads as deleted until rewritten (h)
- shamir-tx: `finalize_reservation` not max-monotonic — stale reads, masked SSI conflicts (h)
- shamir-types: F64 ±0.0 Hash/Eq violation — wrong set/dedup semantics, enshrined by test (h)
- shamir-types: Dec/Big/Set cannot round-trip the wire — silently lossy (h); u64>i64::MAX decodes differently per path (m)
- shamir-index: functional-index hash collapses every Dec/Big/Bin value — all rows match (h)
- shamir-index: Whitespace/Full tokenizers never fold non-ASCII uppercase — Cyrillic FTS dead (h)
- shamir-index: vector delta-replay failure baked in permanently by the next snapshot flip (h)
- shamir-index: compaction double-write failures swap an incomplete graph live (h)
- shamir-index: `SortedIndexManager::load` swallows store errors — all sorted definitions silently lost (h)
- shamir-index: DROP leaks `__vec_snap__` keyspace; q-chunks never pruned (m ×2)
- shamir-storage: InMemoryStore inclusive-resume scan drops the boundary successor (m)
- shamir-engine: RecordCounter lazy init swallows errors — durable count silently zeroed (m)
- shamir-funclib: `UserScalarLayer::register` silently never replaces (h)
- shamir-funclib: rust_decimal overflow panics in agg/array reductions (m)
- shamir-query-builder-macros: missing-comma token drop silently miscompiles insert/update — fields vanish (h ×2)
- shamir-query-builder: `try_build` false-rejects nested sub_batch/for_each — pushes users to unvalidated path (h)
- shamir-wasm-host: HTTP codec collapses duplicate headers — Set-Cookie lost both directions (h)
- shamir-wasm-host: `fuel: u64::MAX` fails every invocation; epoch-interruption test is vacuous (h ×2)
- shamir-sdk: host decode failures silently become empty results / None / Null (h)
- shamir-sdk-macros: `#[validator]` coerces undecodable params to Null/None — UPDATE presents as INSERT (m)
- shamir-sdk: empty-map key on `Table::get` returns an arbitrary first row (m)
- shamir-engine: panicking GroupCommit leader strands `leader_busy` — later flushes hang (m)
- shamir-numa: `rcu`/`store` mirror can permanently strand non-node-0 replicas on stale values (h ×3)

#### P1.b — Security / authorization (reachable, permission-gated or partial)

- shamir-client: `resume()` sends the bearer ticket with zero server-identity verification (h ×2)
- shamir-connect: TOFU pin callback fires before Ed25519 identity verification (h)
- shamir-connect: refill-watermark race multiplies the post-auth rate limit by racer count (h)
- shamir-connect: `dispatch_request` owning variant skips the post-auth rate gate (h-style)
- shamir-db: ambient interner delta exposes any repo's dictionary without Store-Read ACL (m)
- shamir-db: validator Rust-source path skips the WasmCompiler gate (m)
- shamir-query-types: `ForEach` missing from `is_admin()`; HMAC gate never descends into Batch/ForEach (h + m)

#### P1.c — Hangs / liveness

- shamir-client: reader-drain race orphans in-flight requests — permanent hang under default no-timeout (h ×2)
- shamir-engine: DashMap shard guard held across `.await` — runtime wedge during online index builds (h)
- shamir-storage: `CachedStore::flush` parks forever if the async worker dies (h)
- shamir-sdk: `__rt::block_on` busy-spins forever on `Pending` — no waker can fire (h ×2)

#### P1.d — Performance / unbounded memory (known trigger)

- shamir-db: `execute_as` re-authorizes per op without the dedup cache; ACL gate = full catalogue scans (h ×2)
- shamir-engine: O(N²·K) `rederive` under commit lock; O(M²) staged-overlay probes; per-value FK scans; repo-wide scan per UPDATE; changelog `range_from` buffers the whole tail (h ×5–6); ForEach replans; unique-index clone storm (m ×2)
- shamir-tx: `vacuum_key` duplicated/unbatched I/O per write; GC materializes the whole history store (h ×2); linear interval checks under `commit_lock`; `min_alive` full traversal per write (m ×2)
- shamir-types: projection codec O(fields × selected) per row; `for_each_field` O(f²) (h + m)
- shamir-index: per-row deep-clone of all index definitions in every planner; sorted/unique apply one store round-trip per posting (h ×2); FTS lookup materializes + sorts everything (m)
- shamir-connect: `cap_lock` O(all-sessions) scan under a global mutex on every login; AuditChain unbounded in-memory Vec (h ×2)
- shamir-storage: reverse range streams drain range to RAM; eager whole-corpus materialize (h ×4 across reports); `FjallStore::submit` blocks tokio workers (m ×2); Async write channel unbounded; `transact` drains entire dirty buffer (m ×2)
- shamir-wal: `has_truncatable` O(frames) scan every drainer tick (m)
- shamir-client: per-request full request clone, zero-copy envelope unused (h); per-row de-intern allocations (m); unbounded early-buffer keys + orphaned pending entries (m ×2)
- shamir-funclib: `count_distinct` O(N·C); per-row regex recompile; global-Mutex regex cache compiling under lock (h ×3)
- shamir-query-types: `BatchOp::deserialize` triple codec pass + 75-probe linear chain (h)
- shamir-query-builder: `Doc::set` full msgpack round-trip per field (h)
- shamir-wasm-host: `compile_rust_source` blocks tokio workers up to 120 s (h ×2)
- shamir-sdk: ABI leaks both directions' buffers per host call — unbounded guest memory in loops (h)

#### P1.e — Interop / API traps

- shamir-transport-ws: spec subprotocol negotiation unimplemented; hardcoded endpoint paths vs configurable path (h ×2)
- shamir-transport-tcp: unsafe `set_len` UB on cancelled reads (call sites already use `select!`/`timeout`) (h); zero-length write emits the close sentinel (m)
- shamir-client: `batch_has_refs` misses `when` guards and ForEach — silent Id-encoding corruption (h)
- shamir-connect: `client` feature cannot build without `server` (h)
- shamir-db: wire error-`code` contract uneven; `tx_begin` silently downgrades unknown isolation to Snapshot (m + med)
- shamir-query-types: one RecordId identity rides the wire three ways (bin / string / base58) (m)

### P2 — Technical debt / style / hardening (no urgent trigger)

Grouped by remediation theme (each is one sweep, not per-finding churn):

- **Dead / unwired public API:** shamir-tunables (RuntimeTunables unwired), shamir-db (`api::{Request,Response}` shim, `PendingCommit` export), shamir-connect (owning `dispatch_request`, `encode_details_canonical` stub, blocking semaphore), shamir-wal (`WalActiveKey`, `looks_like_v2`), shamir-bench-utils (measure/measure_async), shamir-index (`IndexDescriptor.options`), shamir-client (`RequestIdMismatch`), shamir-tx (`PendingCommit` re-export), shamir-sdk (`pub mod __rt`)
- **Missing / vacuous tests:** shamir-collections (zero), shamir-sdk-macros (zero), shamir-query-builder-macros (no trybuild), shamir-funclib (scalar_resolver), shamir-sdk (http/params/db), shamir-wasm-host (host-import trap paths, aggregate-fuel and epoch tests), shamir-wal (§1.5 tautology, §2.4), shamir-client (AtomicU8), shamir-query-types (fts default), shamir-storage/shamir-index (conformance-suite gaps)
- **Stale / contradictory docs sweep (one docs-only commit):** shamir-connect, shamir-wal, shamir-numa, shamir-tunables, shamir-wasm-host, shamir-types README, shamir-storage key_bytes, shamir-server `/info`, shamir-client examples, shamir-funclib headers, shamir-bench-utils Criterion-era docs, shamir-index invariant doc, shamir-tx status block
- **thiserror adoption / stringly-error removal:** shamir-tx, shamir-wal, shamir-index, shamir-funclib (code catalog), shamir-transport-tcp (TLS `Box<dyn Error>`), shamir-query-types, shamir-query-builder, shamir-connect, shamir-wasm-host gateways, shamir-db collapse sites, shamir-sdk error taxonomy, shamir-types duplicate `CodecError`
- **Versioning for persisted formats:** shamir-storage MemBufferConfig, shamir-engine MetaEnvelope gaps, shamir-wal frame magic/seq + `idx_id`, shamir-index SQ8 durable carrier + rustc-hash pin + v1-snapshot compat, shamir-funclib canonical-hash version, shamir-sdk ABI version
- **Bounded-growth caps:** shamir-tx locks registry, shamir-engine DDL op-log + shadow purge, shamir-index posting-cache byte budget, shamir-wasm-host GlobalVars + engine thread, shamir-sdk dealloc import
- **Untrusted-input hardening:** decode-depth guards (shamir-query-types, shamir-sdk, shamir-client), allocation bounds (shamir-wal bincode, shamir-numa parse_cpulist, shamir-db egress size, shamir-index n-grams), 32-bit frame arithmetic (shamir-wal), staged prealloc (shamir-transport-tcp), SSRF range gaps (shamir-wasm-host)
- **Structure / style sweeps (own commits per CLAUDE.md):** imports-at-top (shamir-engine ~25 sites, shamir-index, shamir-query-types, shamir-db, shamir-transport-tcp/ws, shamir-sdk, shamir-bench-utils, shamir-numa, shamir-funclib, shamir-connect, shamir-server), inline `#[cfg(test)]` blocks (shamir-wal, shamir-engine ×2, shamir-index, shamir-numa, shamir-bench-utils, shamir-query-types), mod.rs-split violations (shamir-tx mvcc_store, shamir-engine group_commit, shamir-query-types validator/call, shamir-query-builder wire/macros, shamir-sdk-macros lib.rs, shamir-db inception), dead deps (shamir-transport-ws tungstenite/dev-deps, shamir-db TLS stack, shamir-wasm-host serde, shamir-sdk serde), missing benches for every hot path named in P1.d

**Sequencing note:** P0 items 1–2 share one fix (per-call caps on the scalar boundary), 4–5 share one (RAII leadership + drain-under-lock breaker exit), and 6–7 share the cache-mutation-helper fix in shamir-storage — so ten entries collapse to roughly six PRs. P1.a/b should land before P1.d: silent wrong answers and security gates outrank throughput.

## Review Coverage Gaps

### 1. shamir-server's triple "No findings" is the most suspicious zero in the corpus

Three of seven server reviewers (concurrency, performance, error-handling-lifecycle) returned literally zero findings for a **113-file crate** — while the correctness reviewer found a *critical* in the same crate. The concurrency/perf zeros are defensible (server is the most heavily audited crate, with dense F-12/F-19/#1073-style incident citations and a documented Optim sweep), but the error-handling zero has concrete counter-evidence: the correctness reviewer filed `try_join_next()` silently swallowing non-panic `JoinError`s (`request_loop.rs:240-249`) — an error-handling-lifecycle finding *by definition*, inside territory that theme declared a "clean bill of health." At least one real miss is provable; the perf zero (not even a nit, where engine/db/index perf found 2–5 highs each) is plausible-but-statistically odd and worth a spot re-check.

### 2. No reviewer owned dependency / supply-chain hygiene

There is no Cargo.lock audit, unused-dependency pass, or duplicate-version check anywhere in the 7 themes — yet reviewers *stumbled into* the same class repeatedly and independently: shamir-transport-ws's phantom `tungstenite 0.29` (so serious the security reviewer flagged the CVE-false-assurance angle: the code parsing untrusted frames runs 0.24 while audits see 0.29), shamir-db's dead TLS/rcgen/argon2 stack compiled into every consumer, unused `serde` (wasm-host, sdk), unused `unicode-normalization` (connect), `tokio features=["full"]` (tcp), four dead dev-deps (ws). Five crates, one unowned theme.

### 3. No owner for cross-crate integration — and the excluded surfaces inherit findings verbatim

Several of the worst findings live *at seams* and were found only because individual reviewers grepped across crates (SORTED_TAG duplication tx↔engine, `SYSTEM_RECORD_PREFIX` types↔storage, exporter-label tcp↔connect, HTTP header codec host↔guest, the subprotocol client↔server drift). Nobody walked one request end-to-end through transport→connect→server→db→engine→tx→storage→wal, and nobody owned the replication lifecycle (supervisor/follower/bookmarks/resync got only fragmentary coverage). Additionally, **shamir-client-node** (napi binding, mirrored 1:1 from shamir-client per the client reviewers) and the TS client sit outside the 161 — the client findings (query_version stamping, resume MITM, subscription hangs) almost certainly propagate there unreviewed.

### 4. Entirely static methodology: no fuzzing, no loom/TSan, no bench execution

Every concurrency finding is *argued*, never demonstrated (the storage tombstone race and numa mirror race are interleavings — precisely what the crate's own deterministic-seam or loom infrastructure could prove). Multiple reviewers *recommended* fuzz targets (wal's `WalEntryV2::decode`, types' huge-header test, index's aarch64 alignment) without any fuzz deliverable. And the ~40 P1 performance findings are all code-shape reasoning — nobody ran a single existing bench to confirm or refute a claimed hot-path cost, despite the workspace's bench-first culture.

### 5. The CI/gate-integrity axis is unowned

Each of these was discovered *incidentally* by a different reviewer, and none is anyone's theme: the project-wide `doctest = false` ban defeating all doc verification; the broken `--no-default-features --features client` build (connect — a feature matrix nothing checks); clippy-lint evasion via `range(..).count()` (tx, index); no aarch64 CI leg while NEON `unsafe` ships (index); and numa's README referencing a QEMU CI workflow that doesn't exist. "Does the gate actually catch any of this?" is a blind spot, not a finding.

### 6. Cross-reviewer severity calibration drift (affects the counts in this document)

Identical violations were rated differently by different style reviewers: inline `#[cfg(test)] mod tests` and `mod.rs`-with-logic are **"high"** in shamir-wal, shamir-engine, shamir-tx, shamir-query-types, and shamir-bench-utils, but **"medium"/"low"** for the same violations in shamir-numa, shamir-types, and shamir-storage. Likewise "entirely untested crate" is high in collections but low in bench-utils. This inflates some crates' high counts (wal, engine, tx, query-types each carry 1–2 style-rated highs) — worth remembering when comparing the scorecard column; those highs are real CLAUDE.md breaches but not runtime risk.

### Non-gaps (checked, genuinely clean)

The other "No findings" reports are legitimate, not under-investigation: shamir-collections/concurrency, shamir-query-types/concurrency, and shamir-query-builder/concurrency are pure-DTO/leaf crates with zero concurrency surface, and each of those reviewers documented exhaustive verification evidence (grep matrices, dependency-listing proofs) rather than bare assertions.
