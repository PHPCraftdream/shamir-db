בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Release audit 09 — Documentation accuracy vs. current code (2026-07-17)

Read-only cross-check of `docs/guide-docs/guide/*.md`, `docs/dev-artifacts/design/oql-*.md`,
`docs/guide-docs/client-server-protocol-spec/`, `README.md`, `CONTRIBUTING.md` and `CLAUDE.md`
against the actual code in `crates/shamir-engine`, `crates/shamir-query-types`,
`crates/shamir-server`, `crates/shamir-client-ts`, `crates/shamir-storage`,
`crates/shamir-index`, `.cargo/config.toml` and `scripts/`.

## Executive summary

The recently-touched query-language surface is in **good** shape: everything this
session changed (BatchLimits.max_iterations + serde default #662, the cooperative
`ExecutionTimedOut` deadline #666, the `ABSOLUTE_MAX_FOR_EACH_ITERATIONS` clamp,
`InvalidWhenFilter` #651 / `InvalidCondCondition` #663, `ValueCompare`, planner
recursion #642, write-value marker resolution #641, `distinct_repos` recursion #660,
nested-tx participation #661) is accurately reflected in `01-queries.md` and the OQL
ADRs. The serious drift is elsewhere, in three clusters:

1. **Stale-by-omission-of-progress**: `08-interconnect.md` tells users that
   leader-follower replication, the network changefeed and live subscriptions have
   "❌ Нет кода" — all three now have substantial working code (a full follower
   replication engine, a `repl` wire protocol, a first-class `Subscribe` batch op and
   a server subscriptions subsystem).
2. **Broken contributor instructions**: `README.md` and `CONTRIBUTING.md` (and one
   block inside `CLAUDE.md` itself) instruct raw `cargo test` commands that the
   repo's own cargo-runner perimeter guard now refuses with exit 2; `scripts/test-all.sh`
   is a dead script for the same reason.
3. **Never-implemented or renamed knobs presented as real**: `03-storage.md` names
   `redb` as the durable engine (actual: `fjall`; `redb` is rejected as
   "Unsupported engine"), `07-operations.md` documents an `allow_public_metrics`
   config field that does not exist, and the protocol spec's `IMPLEMENTATION_GUIDE.md`
   §2 documents an entire config schema (TOML sections, field names) that diverges
   from the implemented `.ktav` `Config`.

One drift is *about to be created* by in-flight work: the `$cond` perf warning in
`01-queries.md` (and the header of `benches/cond_expr_eval.rs`) says the resolver
recompiles the condition "на каждую эвалюацию … план устранения — #643"; the
uncommitted #643 `CondCache` in the working tree fixes this for **select projections
only**, so the guide sentence will need re-wording (not deletion) when #643 lands.

## Drift findings (ranked by likelihood to mislead)

| # | Sev | Doc location | Claim | Actual current behavior | Evidence |
|---|-----|--------------|-------|-------------------------|----------|
| 1 | HIGH | `docs/guide-docs/guide/08-interconnect.md` (§1 table, and the banner "Ни один из описанных ниже P2P/gossip/replication механизмов не имеет рабочего кода") | "Leader-follower репликация — ❌ Нет кода"; "Network changefeed (pull API over wire) — ❌ Нет кода"; "Live subscriptions — ❌ Design doc … status: PROPOSED" | All three exist as working code. Follower replication engine: `crates/shamir-server/src/replication/` (`follower_loop.rs`, `wire_source.rs`, `supervisor.rs`, `prod_factory.rs`) driven over a real `repl` wire protocol (`shamir_query_types::wire::repl::{ReplRequest, ReplResponse}`, `crates/shamir-server/src/replication/wire_source.rs:15`), with `ReplicationConfig` in `crates/shamir-server/src/config.rs:121` and convergence e2e tests (`crates/shamir-server/tests/repl_convergence_e2e.rs`). Live subscriptions: `BatchOp::Subscribe(SubscribeOp)` is a first-class wire op (`crates/shamir-query-types/src/batch/batch_op.rs:150-151`, `crates/shamir-query-types/src/subscribe/subscribe_op.rs`), served by `crates/shamir-server/src/subscriptions/` (bridge/push/reactive/registry), and the protocol spec lists `SUBSCRIPTIONS.md` under **Core (нормативные)** as "v1.1" (`docs/guide-docs/client-server-protocol-spec/README.md:35`) — not PROPOSED. | see cells |
| 2 | HIGH | `README.md:90-99` (§Testing) and `CONTRIBUTING.md:8-9`; also `CLAUDE.md` "Pre-commit gate" block | Run tests via `bash scripts/test-all.sh` and `cargo test -p shamir-engine` / `cargo test --workspace --lib` / `cargo test --workspace --test '*'` | Raw `cargo test` is **blocked** by the cargo-runner perimeter guard: `.cargo/config.toml` `[target.'cfg(all())'] runner` refuses any test binary not launched under nextest (`$NEXTEST` unset → banner + `exit 2`). `scripts/test-all.sh` itself builds `cargo_args=(test --workspace --tests)` (`scripts/test-all.sh:49`), so following README's first command dead-ends on the guard. The only working entry points are `./scripts/test.sh` / `cargo t` / `cargo tl` — exactly what CLAUDE.md's later "🧪 Centralised test entry point" section mandates, contradicting its own earlier "Pre-commit gate" block that still lists `cargo test --workspace --lib`. A new contributor following README/CONTRIBUTING verbatim cannot run tests. | `.cargo/config.toml` (runner block, ~line 95); `scripts/test-all.sh:49`; `README.md:92-98`; `CONTRIBUTING.md:8-9` |
| 3 | HIGH | `docs/guide-docs/guide/03-storage.md:69-71` ("wire-созданные репозитории — durable (**redb**)"); also `crates/shamir-storage/src/lib.rs:9` and `crates/shamir-engine/src/repo/repo_types.rs:7` doc comments (`features = ["redb"]`) | Durable repo engine is `redb`; a `redb` cargo feature exists | The durable engine is **fjall**. `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:203-225`: accepted engine strings are `Some("fjall") | None` and `in_memory`; anything else (including `"redb"`) errors "Unsupported engine '{}'. Supported: in_memory, fjall." `crates/shamir-storage/Cargo.toml` has no `redb` feature (`all-backends = ["fjall"]`). README's backend table ("Fjall ✅ Supported") is the correct one. | see cells |
| 4 | HIGH | `docs/guide-docs/guide/07-operations.md:249-253` (§Безопасность observability) | "Для non-loopback — нужно явно разрешить `allow_public_metrics: true`" | No such config field exists. `ObservabilityConfig` has exactly one field, `addr` (`crates/shamir-server/src/config.rs:157-163`). `allow_public_metrics` is an internal function parameter hardcoded to `false` at the call site (`crates/shamir-server/src/server/server_launcher.rs:664-669`: "Operators that need a public scrape endpoint can promote this to a config flag in a follow-up"), so a non-loopback observability addr is rejected with **no** config opt-in. The doc's own TODO at line 255 admits this was never verified. | see cells |
| 5 | MED | `docs/guide-docs/guide/01-queries.md:231-251` (perf warning) and `crates/shamir-engine/benches/cond_expr_eval.rs` header | "резолвер сегодня перекомпилирует условие фильтра (`compile_filter`) на каждую эвалюацию, а не один раз на запрос … план устранения — #643" | Half-stale **as of the in-flight working tree**: #643's `CondCache` exists (`crates/shamir-engine/src/query/filter/cond_cache.rs`, uncommitted) but is opt-in and wired **only** into `SelectProjection::new` (`crates/shamir-engine/src/query/read/select_projection.rs:79-123`); WHERE, `when`, `for_each` `over` and write-value resolution still recompile per evaluation (`crates/shamir-engine/src/query/filter/resolve.rs:227-240`). When #643 commits, the guide sentence must be re-scoped ("fixed for SELECT projections; per-row recompile remains for WHERE-embedded `$cond`") and the bench numbers re-measured — a blanket "исправлено #643" would be the *opposite* over-claim. | see cells |
| 6 | MED | `docs/guide-docs/guide/01-queries.md:397-399, 459-472` (`resp.results[alias].skipped` shown as TS client API) | TS clients read `resp.results.debit.skipped` | The Rust wire type has `skipped` (`crates/shamir-query-types/src/read/query_result.rs:90`), but the TS `QueryResult` interface does **not** declare it (`crates/shamir-client-ts/src/core/types/batch.ts:199-210` — records/stats/pagination/value/explain only). A TypeScript user copying the guide gets a type error (`Property 'skipped' does not exist`). Note `edge_provenance` *was* added to the TS `BatchResponse` (batch.ts:258) — `skipped` is the one that fell through. This is client-type drift rather than server drift, but the guide presents it as working TS code. | see cells |
| 7 | MED | `docs/guide-docs/guide/01-queries.md:376-399` (`when` + `filter.valueGte`/`valueLt` "debit/decline" example, described as complementary branches) | Implicit claim: exactly one of `debit`/`decline` runs | True only when `balance_check[0].balance` resolves. If the query returns **0 rows**, both `valueGte(ref, 40)` and `valueLt(ref, 40)` evaluate with an unresolvable left operand → per the adopted #667 contract only `Ne` is true for an absent operand, so **both branches skip silently**. The three-way null/absent contract (absent operand vs. resolved `null == null` → Equal vs. resolved type-mismatch) is documented only in code (`crates/shamir-engine/src/query/filter/filter_node.rs:132-173`, `value_compare_null_tests.rs`) and appears nowhere in the guide. Not a false statement — a user-misleading omission in the canonical example. | see cells |
| 8 | MED | `docs/guide-docs/client-server-protocol-spec/IMPLEMENTATION_GUIDE.md` §2.1 (config schema) | Server config: TOML with `[server] bootstrap_token_output`, `[kdf] max_concurrent_argon2`, `[strict]`, `[resumption]`, `[limits] per_session_mem_mb` / `max_total_session_mem_per_subnet_mb` / `max_connections_per_ip`, `[[listener]] transport = "tcp"`, `[admin_ui]` | Implemented config (`crates/shamir-server/src/config.rs`) is `.ktav` with different names and shape: `listeners[].kind` (not `transport`), `kdf_defaults` + top-level `argon2_concurrent_max` (config.rs:81-84), `security.connection.{auth_init_timeout_ms,max_active_connections,max_active_connections_per_ip}`, `security.query_limits.{max_result_size_bytes,max_execution_time_secs,max_queries_per_batch}` (config.rs:274-286). `bootstrap_token_output`, `[strict]`, `[resumption]`, per-session/subnet memory quotas and `[admin_ui]` have **no** implemented config surface. The spec is normative-aspirational; nothing marks which §2 knobs are implemented vs. planned. Guide 07's `.ktav` example, by contrast, matches the code exactly. | see cells |
| 9 | MED | `CLAUDE.md` (bench convention section) | "no `shamir_bench_utils` (that helper predates the migration and is gone)" | The crate exists, is a workspace member (and is listed in CLAUDE.md's *own* 23-crate list a few paragraphs earlier), and is actively used by 4 bench files: `crates/shamir-engine/benches/{filtered_vector_search,vector_bulk_compaction,vector_search}.rs`, `crates/shamir-index/benches/sq8_hot_path.rs`; dev-deps in `crates/shamir-engine/Cargo.toml:89`, `crates/shamir-index/Cargo.toml:62`. An agent following CLAUDE.md would wrongly refuse to touch/use it. | see cells |
| 10 | LOW | `docs/guide-docs/client-server-protocol-spec/` (scope) | (per CLAUDE.md, this dir is "reference documentation of the wire format itself") | The spec covers only auth/session/transport/subscriptions (`README.md:3` — "Спецификация transport-agnostic аутентификации и сессий"). The batch/query wire layer — `BatchLimits.max_iterations`, `BatchError::ExecutionTimedOut`, `Filter::ValueCompare`, `QueryResult.skipped`, `BatchResponse.edge_provenance`, `ForEachOp` — has **no** prose wire reference anywhere in `docs/guide-docs/`; the only documentation is rustdoc on the types. Scope gap, not a falsehood, but the recently-added fields are exactly the ones an external client implementer cannot discover. | `docs/guide-docs/client-server-protocol-spec/README.md:3,30-56` |
| 11 | LOW | `docs/guide-docs/guide/00-quickstart.md:37` and `04-access.md:51` (`port: 13760`) vs `07-operations.md` / spec examples (7331/7332/7333) | Example port 13760 | No default port exists in code (listeners are mandatory config); `13760` appears nowhere in server sources, all other docs use 733x. Cosmetic inconsistency that can confuse a first-run user cross-reading floors 0 and 7. | `crates/shamir-server/src/config.rs:29` |
| 12 | LOW | `docs/guide-docs/guide/02-durability.md:69-71` | "`materialized: true` — проекции успели построиться…" — referenced without appearing in the preceding example's field list | Field exists and defaults true (`crates/shamir-query-types/src/batch/transaction_info.rs:42-47`; TS `TransactionInfo.materialized` typed at batch.ts:219). Content is correct; the paragraph just dangles — add `resp.transaction?.materialized` to the example. | see cells |
| 13 | LOW | `CLAUDE.md` workspace note; `.cargo/config.toml` `bench-tool` alias comment | CLAUDE.md: workspace "excludes `shamir-client-node`" (only); config comment: bench-scale-tool "is a sibling path-dependency (see the root Cargo.toml's TEMPORARY note)" | Root `Cargo.toml:14` excludes **two** dirs (`shamir-client-node`, `shamir-client-ts`); bench-scale-tool is now a published crates.io dep (`Cargo.toml:29`), the "TEMPORARY note" no longer exists, yet the alias still hardcodes `D:/dev/rust/bench-scale-tool/Cargo.toml` (machine-local path). | `Cargo.toml:14,25-29`; `.cargo/config.toml` alias block |
| 14 | LOW | Guides 02/03/04/05/07 — 7 inline `<!-- TODO: verify … -->` markers | Self-flagged unverified claims (shutdown deadline, repo engine derivation, setuid surface ×2, batch scratchpad API, wasm-opt, allow_public_metrics) | Finding #4 above confirms the `allow_public_metrics` TODO was hiding a real falsehood; the remaining six are still unverified in-doc. Treat each TODO as a probable drift site before release. | `grep -rn TODO docs/guide-docs/guide/` |

## Claims spot-checked and found ACCURATE (no action)

- `01-queries.md` `for_each` block: `max_iterations` default 1000 with pre-iteration-0
  error (`crates/shamir-engine/src/query/batch/query_runner.rs:566-573`); #660
  `distinct_repos` recursion (`crates/shamir-query-types/src/batch/query_entry.rs:91`
  + `tests/distinct_repos_tests.rs`); #661 genuine tx participation of loop/sub-batch
  bodies (`query_runner.rs:169-203, 387-391, 601-609`); tx-abort-rolls-back-all vs.
  non-tx stop-at-first matches `oql-04` ADR Decision 4. The guide does not mention the
  server-side hard ceiling `ABSOLUTE_MAX_FOR_EACH_ITERATIONS = 100_000`
  (`query_runner.rs:34`) — an acceptable omission (server clamps down, never up).
- `01-queries.md` `when`: field-based comparisons rejected at plan time
  (`BatchError::InvalidWhenFilter`, #651 — `crates/shamir-query-types/src/batch/batch_error.rs:93`);
  `isNull`/`isNotNull` still allowed; `valueEq…valueLte` exist in both Rust
  (`Filter::ValueCompare`) and TS (`filter.ts:82-107`); `switchCase`/`forEach`/`handle().column()`
  exist on the TS batch builder (`batch.ts:128,210,384-402`).
- `01-queries.md` marker-error claim: codes `malformed_marker`/`unbound_param`
  are real (`query_runner.rs:238-249`); #642 planner recursion into
  `$cond`/`$expr`/`$fn` for deps is real (`planner.rs:588-640`), including the
  #663 `InvalidCondCondition` plan-time rejection.
- `01-queries.md` batch section: `edge_provenance` on wire + TS (`batch_plan.rs:43`,
  `batch_response.rs:48`, TS batch.ts:258); stages executed sequentially per
  `oql-01` ADR ("deferred"); `@`-prefix on `$query` aliases optional
  (`resolve.rs:193`, `query_runner.rs:519`).
- `BatchLimits` defaults table in rustdoc matches `Default` impl exactly
  (50 / 10 / 30 s / 10 MB / nesting 4 / iterations 1000, `batch_limits.rs:75-86`);
  `#[serde(default)]` on `max_iterations` (#662); TS `BatchLimits` carries both
  `max_nesting_depth` and `max_iterations` (batch.ts:119-133).
- #666 `ExecutionTimedOut`: cooperative checkpoints, `0 → 1 s` minimum budget,
  `u64::MAX → unbounded` overflow fallback, error only ever raised pre-commit —
  rustdoc on `execution_deadline.rs` and `batch_error.rs:108-123` match the code.
- `02-durability.md`: SI default / `'serializable'` opt-in (`batch_request.rs:60`,
  TS `transactional(isolation?)`), `tx_conflict` mapping (`db_tx.rs:248-252`),
  cross-repo `tx_cross_repo_not_supported` (`db_message.rs:256`,
  `BatchError::CrossRepoNotSupported`), HMAC gate codes `hmac_required`/`hmac_mismatch`.
- `07-operations.md` `.ktav` config example: every shown key exists with that name
  (`security.query_limits.{max_result_size_bytes,max_execution_time_secs,max_queries_per_batch}`,
  `argon2_concurrent_max`, `listeners[].{kind,addr,profile,path,browser_origin_allowlist}`,
  `observability.addr` incl. empty-string-disables).
- `00-quickstart.md`: bootstrap token → `data_dir/bootstrap_token.txt`
  (`bootstrap.rs:43`).
- `06-search.md` constants: `BRUTE_FORCE_MAX = 256`, `ef_construction = 200`,
  `MAX_TOPK = 10_000` (`hnsw_adapter.rs:33,58,177`).
- Workspace description: CLAUDE.md's and README's "23 default crates" both match
  `crates/*` (25 dirs) minus the two Cargo.toml excludes.

## Not deeply verified (out of focused-sampling budget)

`04-access.md` §§2-6 (POSIX mode/chmod details, permission action list),
`05-functions.md` SDK macro signatures and funclib inventory (~120 scalars),
`06-search.md` §§6-11 (per-query ef_search/oversample, SQ8, tombstones),
`SUBSCRIPTIONS.md` field-level accuracy vs. `subscribe_op.rs`. Each contains
precise claims worth a follow-up pass; the three TODO markers in `05-functions.md`
are the highest-risk spots.
