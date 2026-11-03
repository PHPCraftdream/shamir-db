
# TOKEN-AUTH1 revision 1 design and progress

## Design scope

- **WP-A:** Connect-side scope, transcript, proof, pair-status, and caps.
- **WP-B:** Server configuration and listener metadata.
- **WP-C:** HTTP transport plumbing only; this does not implement the login protocol end to end.

## Module map

- Connect: `crates/shamir-connect/src/server/auth/http_login/scope_bytes.rs`, `transcript.rs`, `proof.rs`, and their `tests/` directory.
- Server boot/config and listener/token/meta: `crates/shamir-server/src/boot/config/login_listener.rs`, `http_listener.rs`, `tokens.rs`, and `crates/shamir-server/src/engine/server_meta.rs`.
- Transport: `crates/shamir-transport-http/src/server/routes/login/login_routes.rs`, `login_head.rs`, `login_response.rs`, `no_login.rs`, `route_handler.rs`, plus `GuardedIo`.

## Memory and concurrency (future design; NOT IMPLEMENTED)

Pending 1024 x 2048; arrival/credit 4096 x 128 each; indexes 2 x 1024 x 64; five-prefix 1024 x 64; reserve 65536; lockout 8192 x 128. Total 4,718,592; input 12,582,912; output 4,194,304; audit 1,048,576. These are design figures, not a measured or claimed budget. Future bounded tables would use `tokio::Mutex` and no-await critical sections; existing metadata `write_lock` is for serialized durable ID updates, with atomic write/override. These future facilities are not implemented.

## Public API

Observed API surface: `LoginListenerSettings` fields, `login` / `external_tls`; `TokensConfig.login_ttl` / `login_allow_read_write`; `SecurityConfig.login_enumeration_hardening`; `ServerMetaStore.http_login_listener_id`; transport `LoginRoutes`, `LoginHead`, `LoginResponse`, `NoLogin`, and `serve_with_login`; cfg(test) pub(crate) ID setter. Retained `pub(super)` record, `new_token`, and service helpers from the initial slice; no new production visibility widening.

## Decisions, deviations, and deferred work

Async admission (`RPITIT` associated `Permit` and RAII) was chosen over synchronous brief admission to support a future bounded mutex. Legacy connect `tests.rs` was initially accidentally overwritten but is now mounted as an extra boundary test file with explicit `#[path = "tests/mod.rs"]`; no deletion. The accidental empty `config/login_listener_tests.rs` and unmounted transport `login_tests_mount.rs` are preserved for the operator to remove. FINISH also requires future padding 50 plus uniform 0..25ms. Services, wire protocol, admission, pending state, launcher, and end-to-end work are deferred to revision 2 and are not implemented.

## Chronological progress (reconstructed from observed session)

The previous append worker destroyed the earlier history by overwriting the note; this section reconstructs the record from the observed session, not from an untouched original. Work started 22:48:38 CET Nov 2; deadline 01:15 Nov 3; feature cutoff 00:45. WP-A first pass had partial compile/test/fixture/privacy regressions found and corrected repeatedly. Corrections included canonical username, uppercase DB, namespace conversion, scheduler imports, three vectors, topic mount (173/933), and proof schedule. WP-B initial config/meta pass was partial; imports, private fixture, zero-TTL origins, diagnostics, and full `SecurityConfig` literals in five files were fixed. WP-C had multiple partial passes repairing `Context` generic, moved peer/login, syntax, nonexistent field, write override, and body caps. At 00:45 the final read window began. Parent formatting of all three crates and fmt check passed; tracked layout check passed, with eight manual entries and two folders known. No child commands, local compilation, or tests were run; all remain pending CI. No commits, pushes, or Cargo changes.

## Final report

This section supersedes earlier progress and review-window status statements above; those historical statements are preserved as written. At the 00:55 status, the parent-reported deadline remains 01:15.

- **WP-A — DONE, source; CI pending.** Wire-independent scope, transcript, proof/signature helpers, lockout pair-status support, and token-record caps are present. Human review is recommended for protocol-level HMAC handling in `proof.rs`. No dependency or Cargo-file changes were made.
- **WP-B — PARTIAL.** Configuration, validation, and listener metadata source are implemented. Layout cleanup remains required. The accidental empty `crates/shamir-server/src/boot/config/login_listener_tests.rs` must be removed by the operator; this work did not delete it. The mounted legacy `tests.rs` is an intentional boundary and must not be deleted.
- **WP-C — PARTIAL.** Hook, body handling, headers, framing, and timeout source are present. Required coverage remains missing for stalled-body 408, positive 100 Continue, and real TCP; review gaps remain.

The hook's actual API is `LoginRoutes` with associated `Permit: Send + 'static`; `admit_head` returns `impl Future<Output = Result<Permit, LoginResponse>> + Send`, and `handle` returns `impl Future<Output = LoginResponse> + Send`. It also provides `enabled` and `body_limit`. `LoginHead` preserves duplicate raw headers; `LoginResponse` carries exact bytes and optional Retry-After, without Debug body output. Generic `serve_with_login` is added while existing `serve` remains no-login. Async admission is deliberate: it allows a permit future to wait on a bounded mutex.

**Checks and limitations:** Parent-observed `cargo fmt --check` passed for all three crates; the tracked-only Python layout check passed. Manual config/login checks were 8/8 each. No compilation, clippy, or tests were run. The accidental unmounted placeholder `crates/shamir-transport-http/src/server/routes/login/login_tests_mount.rs` also requires operator removal; it was not deleted. The legacy `tests.rs` remains intentionally mounted. Do not infer exhaustive rereading of every touched file.

**Acceptance 1–12:** 1 PARTIAL — transport plumbing only; 2 PARTIAL — strict wire handling deferred; 3 PARTIAL — listener ID/config only, no challenge; 4 PARTIAL — login not implemented; 5 PARTIAL — pair probe only; 6 PARTIAL — caps/flags only; 7 PARTIAL — not implemented; 8 PARTIAL — startup validation only; 9 PARTIAL — probe only; 10 PARTIAL — metrics not implemented; 11 PARTIAL — audit not implemented; 12 DONE, source only — vectors, bounds, and tests are present; CI pending. Other deferred items remain out of scope.

Relevant source references: `shamir-connect` transcript (`transcript.rs:37,51`), scope bytes (`scope_bytes.rs:34`), proof (`proof.rs:6`), lockout pair status (`lockout.rs:610`), and vector test (`transcript_vector_tests.rs:53`); server listener config (`login_listener.rs:8`), HTTP listener validation (`http_listener.rs:80`), token config (`tokens.rs:55`), and metadata (`server_meta.rs:474`); transport login handler (`route_handler.rs:16,119,165,228`), `serve_with_login` (`accept.rs:69`), and guarded I/O (`guarded_io.rs:110,173`). Named tests: `hand_derived_transcript_vectors_and_length_formula`, `proof_uses_scram_schedule_and_rejects_invalid_controls`, `transcript_accepts_minimum_and_maximum_wire_lengths`; `login_defaults_and_nested_unknown_keys`, `http_login_listener_id_is_stable_across_reopen_and_address_scoped`; `login_path_is_routed_before_bearer_authentication`, `declared_oversize_rejected_before_body_poll`, `streamed_oversize_body_is_capped`, `head_rejection_does_not_send_continue_before_admission`, `shared_write_timeout_expires_stalled_canned_rejection`. These tests have not been run.

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Part 0 progress (revision 2)

- F2 completed: `framing/tests/login_canned_tests.rs::shared_write_timeout_expires_stalled_canned_rejection` now asserts TimedOut and virtual elapsed below one second (not run).
- F3 completed: `boot/config/tests/login_listener_tests.rs::login_defaults_and_nested_unknown_keys` now proves nested login block parsing and checks unknown-key diagnostics (not run).
- F5 completed: `LoginHead::new` and `LoginResponse` accessors are public; response Debug exposes only status and body length.
- F6 completed: declared content length is parsed/rejected before admission.
- F8 completed: configured listener_id validation now applies to disabled listeners.
- F9 partial: transcript README claim corrected; duplicate default assertion and collapsed validation formatting not changed.
- F4: added missing/duplicate Content-Type response cases, stalled-body 408 case, admitted `100 Continue` case, route-set 5000ms override assertion, and enabled-listener bearer-route regression case. These tests are present but unrun.
- F7 blocked by the explicit no-delete/no-move rule: `http_login/tests.rs` remains mounted by existing `#[path]` attributes; no duplicate tests or E0761 introduced.
- No tests, compilation, formatting, or layout commands were run.

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Final report

This historical heading is restored; prior status text above remains preserved.

## Part 0 completion progress (revision 2; source only, tests not run)

- F2: improved stalled canned-write timeout oracle asserts a timeout and sub-second paused-clock elapsed time.
- F3: nested login configuration parses positively; the unknown nested key is asserted in its diagnostic.
- F4: added missing/duplicate Content-Type response cases, stalled-body 408 case, admitted `100 Continue` case, route-set 5000ms override assertion, and enabled-listener bearer-route regression case. These tests are present but unrun.
- F5: public `LoginHead::new` has documentation; `LoginResponse` accessors are public; Debug includes status and body length only.
- F6: declared oversized Content-Length is rejected before admission.
- F8: listener_id validation applies even when login is disabled; added regression test.
- F9: restored multiline timeout diagnostics, split duplicate-id diagnostic formatting, corrected README transcript fixture description, and removed duplicate default assertion.
- F7 remains blocked by the explicit prohibition on moving/deleting files; existing `tests.rs` and its `#[path]` mounts remain unchanged. No duplicate tests or E0761 introduced.

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Part 0 verification follow-up

- Corrected the per-request `write_timeout_override` capture by cloning inside the `service_fn` closure before `async move`.
- Restored the original parametrized unsupported-media-type case alongside missing/duplicate Content-Type coverage, and restored `login_path_is_routed_before_bearer_authentication` alongside the enabled-listener bearer regression.
- Stalled-body test now asserts virtual elapsed time is at least 3 seconds and below 4 seconds.

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Revision 2 start design and progress

Start 02:11 CET; deadline 04:45; feature cutoff 04:05.

- Part 0 F2/F3/F4/F5/F6/F8/F9 source implemented and parent reread; CI not run. F7 is blocked: no-delete preserves mounts and requires the operator to move/delete as a single edit set.
- PKG1 wire, PKG2 admission, and PKG3 services/launcher/e2e are active; none is claimed complete.
- Required hook actual API: RPITIT with associated `Permit: Send + 'static`; see the revision 1 API description above. Preserve the module map above. Nominal memory charges total 4,718,592; not yet measured. Mutex uses fixed tables, no-await critical sections, and RAII atomic release.
- Public constructor/accessor widening is intended; Debug exposes status/body length only.
- Guarantees: guarded cryptographic randomness and constant-time handling; bounded input/accounting; no credential logs; exact MessagePack/protocol/policy.
- Exact resolved lock versions: tokio 1.51.6, bytes 1.11.1, rmp-serde 1.3.1, serde 1.0.229, zeroize 1.8.2, hyper 1.9.0, http 1.5.0, rustls 0.23.46.
- No dependencies, Cargo changes, or git changes. No local compilation: only orchestrator-scoped fmt and Python layout checks are allowed; no check/clippy/build/test/rustc/npm. Child runs: none.

## Revision 2 checkpoint (source review in progress)

- Part 0: F2/F3/F4/F5/F6/F8/F9 implemented; CI unrun. F7 remains blocked pending operator consolidation of `tests.rs` mounts.
- PKG1: strict wire reader, types, and encoders are source-present; matrix review remains ongoing. PKG2: hierarchical-table slots are source-present; caps, liveness, fair selection, pins, budget/churn, and cancellation review remain incomplete. PKG3: BEGIN/FINISH context, routes, launcher, audit, and metrics are in progress; E2E is unfinished. All packages remain PARTIAL; only Part 0 is complete at source level.
- Parent-observed `cargo fmt --check` for connect/transport passed; server formatting passed once after syntax correction. No compilation, tests, or clippy were run. Nominal 4.5 MiB is not an actual certified budget. The wire folder's eight entries include accidental `tests.rs`; deletion/consolidation is operator-only. F7 `connect/tests.rs` is likewise blocked.
- Crypto review observed reuse of HTTP proof helpers; no native server `AuthMessage` after fix. E2E client remains under review. Bounded shutdown versus durable completion is a critical unverified runtime issue.
- No commands (including Cargo/git), dependency actions, or edits to other files; no deleting, moving, emptying, or rewriting history. No local compilation or tests. Parent formatting/Python layout checks only. No child agents.

## Revision 2 superseding checkpoint (03:25 CET, Nov 3)

This checkpoint supersedes earlier status claims where they conflict; prior history is preserved unchanged.

- **PKG1 — PARTIAL.** The wire implementation reuses the connect scope encoder. Shape and bounds tests are present in source; they have not been run.
- **PKG2 — PARTIAL.** Pending/domain 20, offsets 20/21/22/23..39 were corrected. Markers, pins, and source limits are source-level only. Budget/table eviction LRU was repaired and public-API churn tests were added, but a complete budget is not proven.
- **PKG3 — PARTIAL.** BEGIN uses fake/real lookup blocking; FINISH uses snapshot blocking; compensation audit is wired in source. Launcher integration corrections are ongoing. E2E remains incomplete and is currently being repaired.
- **All packages remain PARTIAL** pending CI and acceptance coverage. Source presence is not verification.
- **Policy violation disclosure:** child session `67f8c6e7...` ran `cargo test -p shamir-server --test e2e http_login --no-run` twice, contrary to the project's no-build requirement. The initial child prompt had been truncated, and supervisor injection attempts failed because the tool schema lacked `child_session_id`. The first invocation failed with status 101 after 26m39s. No tests are known to have executed and there was no successful compilation. The second invocation also finished per transcript; its result is not recorded here. Do not infer a result. The parent ran only allowed fmt/layout/read operations. Do not claim there was no local compilation this session.
- No artifact cleanup or deletion was permitted; build-target artifacts remain. The last `cargo fmt --check` parsed the code but failed due to formatting drift (status 1).

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Revision 2 checkpoint (03:55 CET, Nov 3)

This checkpoint supersedes conflicting earlier status statements; history above is preserved.

- `service/context.rs` is legitimately mounted: it owns `LoginServiceOptions`; issuance owns `LoginServiceContext`. This is not operator cleanup.
- Audit details now use actual MessagePack binary token IDs and tuple counts, per the `AuditEntry` contract. Capture tests decode the payload rather than bless raw bytes.
- Source added `hardened_mismatched_tuple_uses_fake_salt_and_current_default`. Full matrix coverage remains partial.
- The previously incorrect pending-test mount was repaired to `mod store_tests`. Base IPv4/IPv6, `/64`, `/56`, and entropy-failure test sources are present.
- Final parent-allowed gate at 03:49: `cargo fmt` for server, three format checks, and Python layout check passed. The manual wire eight-entry violation remains; layout check ignores untracked files.
- Both unauthorized E2E child `cargo test --no-run` invocations failed with status 101 (26m39s and 26m25s). The second failed with E0603 on a private path; source was corrected afterward. No tests ran. Compilation attempts are not test execution.
- Another worker ran `cargo fmt --check` despite the edit-only prompt; it was allowed by project policy but violated the child workflow restriction, and is disclosed here.
- All packages remain PARTIAL. No Cargo/dependency/git-index changes. Final review is scheduled 04:05–04:45; substantial files have already been read, but do not claim the full 40-minute review is complete.

- Test sources now begin with policy/known-unknown cases and finish with success, bad-proof, replay, wrong-source, privilege, self-service, floor-change, and pair-gate cases. Audit assertions capture issued, aggregate, and compensation events.
- E2E source has two launches: enabled with Ping, explicit `table_read` grant, duplicate/wrong/unknown inputs, and credit-429; disabled with both methods and Ping. Runtime remains unverified in CI.
- Remaining wire cleanup: eight entries in accidental `tests.rs`; connect legacy F7 and unmounted `service/context.rs` cleanup are operator work. Full 4.5 MiB budget, including allocator/task metadata, is unproven. Pending table is fixed-scan, not an expiry queue; `routes.shutdown` has unbounded storage.
- Missing coverage/review: TLS restart, E2E privilege, ineligible-complete, redaction-log capture, compensation injection, and pair-drop-reasons metrics. Policy padding is 50ms without explicit RNG ordering; jitter deviation remains.
- Parent format reported server exit 0 before recent edits. Chained checks can drift from server state, so overall exit 0 does not establish all checks passed. Root unpinned-subtree churn is permitted by spec 113; do not report it as a bug. Table test encodes distinct/16 directly; `0o744` with other-read is valid. `parse_token` actual `&[u8]` was repaired.
- All packages remain PARTIAL. No commands, read/edit only; no children, git, compilation, or machine paths. Preserve history.

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Revision 2 checkpoint (03:58 CET, Nov 3)

This append records the 03:58 source-audit checkpoint; historical text above is preserved.

- Source audit found audit details used raw bytes contrary to the canonical MessagePack contract. This is now corrected to a binary token ID plus count tuple; capture tests decode the payload.
- A log-capture FINISH success/failure sentinel test was added and is under review; it is not verified green. A hardened mismatched-challenge test and equal-length known/unknown shape assertion were also added.
- `serviceOptions` was moved into the existing context file; the mount now closes the lost-source issue. Root churn and `0o744` findings were refuted as false positives.
- Outstanding policy/design tension: no-RNG policy versus all-jitter. Pair metrics update only on HTTP failure, not all native/unlock paths; queue-admission counter labels are incomplete.
- At 04:05, stop feature additions and begin the full 40-minute read. Do not claim CI. Parent-allowed gate passed at 03:49; repeat after the last repairs.
- Both unauthorized local compile attempts failed with status 101; no tests executed. Artifact cleanup is operator-owned.
- Preserve historical text; append only. No machine paths, commands, child runs, git operations, or compilation.

## Revision 2 checkpoint (04:00 CET, Nov 3)

This is an append-only manual inventory checkpoint. It supersedes conflicting inventory and review claims above; historical text is preserved.

- Manual parent-directory inventory: `http_login` has 7 entries; `admission` 6 (`head`, `mod`, `source`, `state`, `table`, `tests`); `pending` 3; `service` 7 (`audit`, `begin`, `context`, `finish`, `issuance`, `mod`, `tests`); `wire` 8 (`begin_request`, `finish_request`, `mod`, `reader`, `response`, `scope`, `tests` directory, `tests.rs`). The wire count of 8 violates the maximum of 7. A child review's claim that 8<=7 is rejected.
- Test-directory inventories: `service/tests` 5, `wire/tests` 2, `admission/tests` 4, `pending/tests` 1. Actual E2E directory `crates/shamir-server/tests/e2e/http_login` has 4 entries (`common`, `flow`, `policy`, `mod`).
- All new files are under 1000 lines; observed largest files include `pending/store` at 444 lines and `service/tests/finish` at 412 lines. Tracked-layout check ignores untracked files and does not validate this manual inventory.
- No measured memory budget exists. An `Arc` handle is one pointer; 24 bytes refers to inner references plus atomic state on a 64-bit target, not the handle. The nominal 128-MiB budget is not certified.
- Parent formatting for all three crates and layout check passed at 03:57. Read-through is planned for 04:05–04:45 and is not complete. All packages/statuses remain PARTIAL. No aliases or documentation-completion claims are made.
- No commands, compilation, git/dependency operations, or child runs; read/edit only. No machine paths. Preserve history; append only.

## Revision 2 checkpoint (04:03 CET, Nov 3)

Append-only checkpoint before the final review window. This checkpoint corrects inaccurate child-review claims; prior history remains preserved.

- **Rejection-zone sampling:** the rejection zone is `MAX - MAX % M`, which is a multiple of `M`. Accepted values are `0..zone`; each residue therefore has exactly the same number of preimages. At most `M-1` values are excluded, so this is bounded rejection waste, not modulo bias. This mechanism does not prove the rest of the implementation unbiased.
- **Padding:** BEGIN and FINISH add no entropy: the helper calls `sleep_until(received + 50ms + ZERO)`. The minimum 50ms is preserved. The policy branch of 50ms without jitter is a known, separated deviation from the all-jitter policy, not evidence that padding is absent.
- **Deadline:** `timeout_at(total_deadline)` bounds the HTTP response to at most 120s, even when the worker continues for durability. Do not claim there is no total-budget bound. The continuing worker and shutdown remain unbounded risks.
- **Pending grouping counterexample:** 64 distinct pending items within one `/56`, followed by a 65th that is Full, is a valid counterexample for the grouping behavior. Reject the review's contrary assertion.
- **Scope validation:** the encoder is fully read. Actual `connect_scope_bytes` checks length 128/255, ASCII, sorting, and the 512-byte bound. This is not missing validation; it is a distinction between source coverage and matrix coverage.
- **Root-subtree churn is explicitly allowed** by the spec; do not report it as a defect.
- Actual remaining gaps: unbounded shutdown, write-permit risk, ID race, full memory-budget proof, pair-drop-reasons metrics, TLS restart coverage, and compensation-injection coverage. The source added redaction and hardening tests, but no tests were executed. The manual wire-eight-entry layout violation remains real.

## Final review window (04:05–04:45 CET, Nov 3)

Feature work is frozen. Review-only changes are limited to concrete bug repairs; this append records findings and does not claim runtime verification.

- **Roles:** source confirms that `update_roles` checks for missing roles after the check and corrects a surgical `after.roles != before.roles` condition. This is source-confirmed only, not runtime-observed.
- **Forced shutdown — critical, unverified risk:** the hard grace in `server_handle` (source lines 18–23 and 192–212) can abort the listener. Dropping its `JoinSet` aborts async FINISH tasks while `TokenService.spawn_blocking` creation may continue. Issuance completion and compensation during forced shutdown are therefore UNPROVEN. This requires a future ownership/durability design, not a last-minute timeout adjustment.
- **Launcher orchestration correction:** the launcher does call `routes.shutdown()` after `serve` returns; the reviewer claim that there is no orchestration call is false. The call itself is unbounded and does not establish safe completion under forced shutdown.
- **Status:** all packages remain PARTIAL; CI was not run. No claim of runtime verification or successful shutdown/compensation is made.

No commands, child runs, compilation, or git operations were performed for this append.
## Final report — revision 2 (04:15 CET draft; review ongoing)

### Overall status

Part 0 is DONE at source level except F7; CI and runtime verification are pending. PKG1 (wire), PKG2 (admission/resources), and PKG3 (services/launcher/E2E) are PARTIAL. Acceptance criteria 1–11 are PARTIAL; criterion 12 is DONE at source level, with CI pending. Source presence and parent formatting/layout checks do not establish tests passing. The 04:05–04:45 review window is still ongoing; the final 40-minute review is not complete, and required CI is not green/verified.

### Part 0 and package status

Part 0 F2/F3/F4/F5/F6/F8/F9 changes are present in source; F7 remains incomplete. The operator must consolidate `crates/shamir-connect/src/server/auth/http_login/tests.rs` and its mounts as the F7 single edit set; do not claim F7 complete. The legacy tests remain mounted. `service/context.rs` is now legitimately mounted and valid; it is not operator cleanup. Source-only role comparison post-create fix checks missing roles after creation and compares `after.roles != before.roles`; it has not been runtime verified.

PKG1 has source for strict wire reader/types/encoders and reuses the connect scope encoder, but full wire, quota, and parity matrix coverage is absent. PKG2 has source for hierarchical admission tables, but caps, fairness, liveness, pins, budget/churn, and cancellation are not fully verified. PKG3 has source for BEGIN/FINISH, routes, launcher, audit, and metrics, but end-to-end behavior and lifecycle guarantees remain incomplete.

### Tests and verification

No tests were executed. Two unauthorized child `cargo test --no-run` attempts failed with status 101; these are compilation attempts, not test results. Relevant source test names include `hand_derived_transcript_vectors_and_length_formula`, `proof_uses_scram_schedule_and_rejects_invalid_controls`, `transcript_accepts_minimum_and_maximum_wire_lengths`, `login_defaults_and_nested_unknown_keys`, `http_login_listener_id_is_stable_across_reopen_and_address_scoped`, `shared_write_timeout_expires_stalled_canned_rejection`, `login_path_is_routed_before_bearer_authentication`, `declared_oversize_rejected_before_body_poll`, `streamed_oversize_body_is_capped`, `head_rejection_does_not_send_continue_before_admission`, `hardened_mismatched_tuple_uses_fake_salt_and_current_default`, and the source-only E2E flows under `crates/shamir-server/tests/e2e/http_login`. These are not reported as passing. Parent formatting checks and layout checks passed at earlier checkpoints; the manual wire directory still has eight entries, exceeding the seven-entry limit. No Cargo/dependency or git mutation was made by the parent.

### Public API surface changes

Public widening includes `LoginHead::new` and `LoginResponse` accessors; response Debug reports status/body length only. New visibility also includes `test_common` as `pub(super)`, `grant` as `pub(crate)`, and the new hook constructor. These are source/API changes, not evidence of end-to-end compatibility.

### Remaining risks and required work

- Global process-resource bug repair is in progress; do not claim fixed until the parent verifies it.
- Forced shutdown can abort listener/task ownership while durable critical work continues; issuance/compensation completion is not proven.
- Output permit/budget behavior remains a risk; pending expiry uses a scan, not an expiry queue. The nominal 4.5 MiB figure is not a certified memory bound.
- ID collision race remains unresolved; padding policy remains 50ms without jitter.
- Metrics are incomplete, including pair-drop-reason and queue-admission accounting.
- TLS restart/config-ID end-to-end coverage is absent. Full wire/quota/parity matrix and compensation-injection tests are absent.
- Other stated coverage gaps remain: E2E privilege/ineligible-complete, redaction-log capture, and full budget/allocator/task-metadata proof. Shutdown/storage ownership is not established as durable under forced abort.

### Schedule and disposition

The final review window is 04:05–04:45 CET and has not yet completed at this 04:15 draft. CI is required and not green/verified. Treat all package and acceptance statuses above as source-only and partial where stated; do not infer runtime results.

### Acceptance 1–12 — source evidence and CI gaps (04:15 review update)

This table supersedes conflicting acceptance claims above without deleting history. Status is source-only; CI is pending and no tests are claimed to have passed.

| # | Status | Source evidence (file:line) | Relevant test name | Remaining gap |
|---|---|---|---|---|
| 1 | PARTIAL | `crates/shamir-transport-http/src/server/routes/login/accept.rs:69`; `.../login_head.rs`; `.../login_response.rs` | `declared_oversize_rejected_before_body_poll` | HTTP transport only; strict protocol integration remains incomplete. |
| 2 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/wire/reader.rs`; `.../wire/tests/wire_tests.rs:12` | `begin_accepts_signed_official_shape_and_every_unsigned_width` | Full wire/parity matrix remains incomplete. |
| 3 | PARTIAL | `crates/shamir-server/src/boot/config/login_listener.rs`; `.../engine/server_meta.rs` | `http_login_listener_id_is_stable_across_reopen_and_address_scoped` | Listener ID/config only; no completed challenge lifecycle. |
| 4 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs`; `.../service/tests/begin_tests.rs:115` | `known_and_unknown_users_receive_indistinguishable_challenges` | E2E login remains incomplete. |
| 5 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs`; `.../service/tests/finish_tests.rs:265` | `incorrect_proof_records_one_pair_failure_and_consumes_claim` | Pair probe/failure path only; full acceptance coverage pending. |
| 6 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/pending/store.rs:150`; `.../admission/table.rs` | `nominal_payload_budget_arithmetic_is_exact` | Caps/flags are source-present; full memory proof is absent (nominal total 4,718,592 bytes, 4.5 MiB, not 128 MiB). |
| 7 | PARTIAL | `crates/shamir-connect/src/server/auth/http_login/tests.rs` and its existing mounts | `hand_derived_transcript_vectors_and_length_formula` | F7 connect and server wire eight-entry layout need operator atomic cleanup/consolidation. |
| 8 | PARTIAL | `crates/shamir-server/src/boot/config/login_listener.rs`; `.../boot/config/tests/login_listener_tests.rs:27` | `login_defaults_and_nested_unknown_keys` | Startup validation exists; end-to-end TLS/config restart coverage remains. |
| 9 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/tests/begin_tests.rs:211`; `.../service/tests/finish_tests.rs:144` | `hardened_mismatched_tuple_uses_fake_salt_and_current_default`; `finish_tracing_redacts_credentials_and_secrets` | Redaction test and MessagePack audit-capture tests are present; coverage is partial, not wholly missing. |
| 10 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/metrics.rs`; `.../service/audit.rs` | `failures_count_only_fixed_classes` | Pair-drop-reasons and queue-admission accounting remain incomplete. |
| 11 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs`; `.../service/tests/audit_tests.rs:34` | `captures_issued_and_aggregate_events_without_identity` | Compensation-injection and broader audit lifecycle coverage remain. |
| 12 | DONE (source; CI pending) | `crates/shamir-connect/src/server/auth/http_login/transcript.rs`; `.../transcript_vector_tests.rs` | `hand_derived_transcript_vectors_and_length_formula` | Source evidence only; CI pending. |

Global process-resource repair is source-confirmed: shared launcher resources are initialized once in `crates/shamir-server/src/launcher/http/launcher.rs:47`, `listeners.rs:186` uses `Once`, and `launcher.rs:249` clones the shared resources. `crates/shamir-server/src/http/mod.rs:12` reexports the API and the actual caller supplies seven arguments. The role-after-check/update correction is in `finish.rs:187`. Public test-surface widenings observed: `http_rest::common` is `pub(super)`; `grant_default_main_access` is `pub(crate)`; `Harness::launch_with_hooks`; `LoginHead::new`/`LoginResponse` accessors; and the new crate `http_login` module. No `test_common` or `grant` names are claimed.

The F7 connect cleanup and server wire-eight-entry layout still require operator atomic cleanup. Remaining review: strict shutdown, write-budget risk, ID race, policy jitter, per-listener queue budget, full memory proof, and TLS compensation matrix. Three format checks and the layout check passed at 04:08; git diff check reported only the document's blank-at-EOF issue, corrected by this append. The two unauthorized child `cargo test --no-run` attempts failed with status 101; no tests ran. No Cargo/dependency/git mutations or local tests are claimed.

### Authoritative acceptance correction (review at 04:18 CET)

The preceding draft table misassigned several acceptance numbers and abbreviated nonexistent paths. This table supersedes it. All statuses are source-only; no tests have run successfully.

| Acceptance | Status | Evidence and remaining work |
|---|---|---|
| 1 Routes | PARTIAL | `crates/shamir-transport-http/src/server/routes/login/route_handler.rs:16`; `crates/shamir-server/src/security/perimeter/http_login/routes.rs:84`; `crates/shamir-server/tests/e2e/http_login/login_policy.rs:9`. Transport 408/100-Continue/header/bearer regressions and disabled GET/POST/Ping tests exist. Response-phase permit/output-budget and shutdown guarantees remain open. |
| 2 Strict wire | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/wire/reader.rs:25`, `begin_request.rs:18`, `finish_request.rs:13`; `wire/tests/wire_tests.rs::begin_rejects_strict_map_field_regressions`. Full field/type/width/boundary matrix is incomplete; wire directory has eight entries. |
| 3 Challenge | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs:32`; tests `known_and_unknown_users_receive_indistinguishable_challenges` and `hardened_mismatched_tuple_uses_fake_salt_and_current_default`. Real/fake/current-KDF source and 50ms floor exist; TLS/restart/configured-ID E2E and full parity coverage remain. |
| 4 Successful login | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs:55`; `crates/shamir-server/tests/e2e/http_login/login_flow.rs:11` exercises proof/signature/token-ID, Ping, explicit database/store/table grants and ordinary-user read. CI and compensation/lifecycle evidence remain. |
| 5 Failures | PARTIAL | Tests `incorrect_proof_records_one_pair_failure_and_consumes_claim`, `wrong_source_does_not_consume_correct_claim`, `pair_failure_after_begin_skips_proof_without_accounting_again`; pending TTL/claim tests. Full unknown/ineligible failure-count and Retry-After parity matrix is missing. |
| 6 Policy | PARTIAL | `begin_policy_denials_are_padded_and_have_no_side_effects`, `disabled_self_service_denies_valid_proof_without_pair_failure`, `valid_proof_for_privileged_user_is_denied_without_pair_failure`, `user_floor_change_before_finish_rejects_valid_proof`; duplicate-label E2E. Replicator/max-token/concurrent-limit and post-create compensation injection remain. |
| 7 Admission | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/admission/table.rs:88`, `state.rs:98`, `pending/store.rs:164`; slot caps/cancellation, pending IPv4/IPv6 caps, fair-share/all-claimed/entropy tests exist. Complete quota/eviction/no-refund matrix and actual charged-memory proof remain; E2E 429 does not isolate CREDIT from ARRIVAL. |
| 8 Classes/proxies | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/admission/head.rs:90`; tests `host_and_origin_validation`, `proxy_chain_trust_walk_and_forwarded_duplicates`, `mapped_ipv4_trusted_proxy_and_forwarded_source`, `mapped_loopback_and_bound_port_defaults`; config startup tests. TLS/class-2 real-wire and restart coverage remain. |
| 9 Lockout | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs:112`, `finish.rs:106`; existing shared LockoutStore supplies native curve. HTTP/native cross-path/pressure/unlock matrix remains unexecuted and incomplete. |
| 10 Metrics | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/metrics.rs:36`; fixed result/eviction/budget-drop labels, pending gauge, startup ineligible count and HTTP failure/success pair snapshots exist. Pair-quota/table-full reasons, native/unlock live gauge, head-rejection totals and fixed histogram bucket evidence remain incomplete. |
| 11 Audit/redaction | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:43`; tests `captures_issued_and_aggregate_events_without_identity`, `captures_compensation_event_with_token_id_only`, `failures_count_only_fixed_classes`, `finish_tracing_redacts_credentials_and_secrets`. Audit payloads are MessagePack; per-listener queue aggregate budget, blocking append shutdown and injected compensation failure coverage remain. |
| 12 Transcript vectors | DONE in source, CI pending | Existing `crates/shamir-connect/src/server/auth/http_login/transcript.rs:37` and vector/proof/boundary topic tests remain mounted. No vector or crypto runtime result is claimed. |

Global pending/admission allocation was corrected during review: `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:47` defines shared resources; `crates/shamir-server/src/engine/server/server_launcher/listeners.rs:186` constructs them once using `.then(LoginResources::new)` (not `Once`); listener setup clones the Arcs at `http/launcher.rs:249`. The re-export is `crates/shamir-server/src/engine/server/server_launcher/http/mod.rs:12`. These are observed source changes, not a multi-listener runtime test. Audit queues remain per listener. The nominal state budget is 4,718,592 bytes (4.5 MiB), not 128 MiB, and is not certified by the arithmetic test.

## Final report — revision 2 disposition (04:26 CET; supersedes earlier drafts)

### Scope and status

The revision-2 request is **PARTIAL, not implemented in full and not merge-ready**. Part 0 F2/F3/F4/F5/F6/F8/F9 are present at source level; F7 remains blocked by the no-move/no-delete instruction. PKG-1 wire, PKG-2 admission/pending, and PKG-3 services/routes/launcher/E2E all have substantial source, but all remain PARTIAL. Acceptance items 1–11 remain PARTIAL; item 12 is DONE in source only, CI pending. The authoritative 04:18 acceptance table above gives the per-item evidence, named tests and missing coverage; it supersedes the incorrect preceding table. No runtime conformance, successful compilation or passing tests are claimed.

### Review evidence and last correction

OBSERVED: final-review source reads include routes, BEGIN, FINISH, issuance/context, audit and capture/redaction tests, pending insertion/claim/eviction, admission tables/slots/head policy, wire reader/scope/response types, launcher shared resources and callsite, and both E2E flows. Earlier review covered the Part 0 transport/config diffs. This is a bounded lint-aware review, not a claim that every whole touched file and every imported dependency was exhaustively reread. Feature work was frozen at 04:05; only concrete review corrections followed.

OBSERVED: `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs:185` now rechecks `claim.is_live()` after the awaited post-create account snapshot, alongside key/floor/roles/privilege/eligibility checks. Before this correction, expiry during that await was not rechecked. An expired claim now enters the existing compensation path at that boundary. This is source evidence only; deterministic expiry-during-snapshot and compensation-injection tests are still required in CI.

OBSERVED: all three scoped formatting checks and the tracked layout checker passed at 04:22:43 CET. At 04:23, `git diff --check` failed only for the report's blank line at EOF; appending this section makes that line an internal separator. A final whitespace/check result must be recorded separately after this append. The tracked layout checker does not cover untracked source. Manual listing confirms `wire/` has eight direct entries (limit seven); `pending/tests/` has two, not one. No Cargo manifest or lockfile changes were observed.

### Outstanding security/conformance work

- **Critical, INFERRED lifecycle risk from observed source:** `crates/shamir-server/src/engine/server/server_handle.rs:192` aborts a listener after its drain budget. `http_login/routes.rs:20,37,145` owns FINISH workers in a JoinSet and drains them without an independent durable owner; `security/tokens/service/token_service.rs:204` performs creation in spawn_blocking. Forced listener abort can therefore cancel post-create checks/compensation while blocking creation continues. Normal launcher `routes.shutdown()` exists at `server_launcher/http/launcher.rs:377`; that does not prove forced-shutdown safety. No runtime reproduction was attempted.
- **OBSERVED permit lifetime; INFERRED aggregate-output risk:** `http_login/routes.rs:140` drops the input permit before sending the response to transport; `shamir-transport-http/src/server/routes/login/route_handler.rs:249` returns an owned body to the writer. Required response-phase slot/output-budget ownership is not established. The transport's total HTTP deadline and five-second write override exist; neither proves the global output-memory bound.
- **OBSERVED per-listener audit allocation:** `server_launcher/http/launcher.rs:251` calls `LoginAudit::start` for each enabled listener; `http_login/service/audit.rs:47` allocates a 1,024-event queue each time. Pending/admission are now process-shared through LoginResources, but the audit queue is not. The process-wide 1,024-event/1 MiB claim therefore remains unsupported for multiple listeners. A blocking append can also outlive the two-second consumer drain/abort.
- **OBSERVED TOCTOU:** `http_login/service/begin.rs:264` checks an ID before `pending/store.rs:178` separately inserts it; a concurrent collision at insertion returns busy without retrying the entire allocation three times.
- **OBSERVED spec deviations/unproven bounds:** body-policy BEGIN denials use 50 ms without jitter to preserve the explicit no-CSPRNG-before-policy rule; other terminal service paths sample jitter. Pending expiry uses a fixed scan rather than the required insertion-ordered queue. In-flight input requests do not hold budget-table pins, and the pin/accounting design needs reconciliation with the stated maximum. The nominal state budget is 4,718,592 bytes (4.5 MiB), not measured or proven: actual allocator overhead, task metadata, selector scratch, payload/index/pin charges and response/audit aggregate storage remain unproven.
- **OBSERVED coverage/instrumentation gaps:** complete strict-wire, quota/eviction/no-refund, unknown/ineligible parity, cross-native lockout/unlock, replicator/max-token/concurrent-limit, deterministic compensation/failure, TLS/class-2 and restart/configured-ID E2E coverage remain incomplete. The E2E 429 oracle does not distinguish CREDIT from ARRIVAL. Missing metrics include pair_quota/pair_table_full reasons, native/unlock-live pair occupancy, head rejection totals and fixed histogram bucket evidence. The redaction and MessagePack audit-capture tests do exist; they are not reported as absent or passing.

### Operator cleanup and visibility

F7 requires atomically consolidating `crates/shamir-connect/src/server/auth/http_login/tests.rs` into its tests folder and removing its explicit path mounts. Separately, consolidate the mounted fixture `crates/shamir-server/src/security/perimeter/http_login/wire/tests.rs` into `wire/tests/` and remove the legacy file/path mounts atomically to bring `wire/` to seven entries. These files must not simply be deleted without preserving their content. No moves/deletions were performed. `service/context.rs` is mounted and owns LoginServiceOptions; it is not orphan cleanup.

Public surface changes include `LoginHead::new`, `LoginResponse` accessors, the new crate `http_login` module and public sub-APIs; test reuse widened `http_rest::common` to `pub(super)` and `grant_default_main_access` to `pub(crate)`, and added Harness::launch_with_hooks. The launcher helper/resource visibility is restricted to server_launcher. No claim of semver compatibility follows from formatting alone.

### Command-policy disclosure and CI handoff

Two earlier unauthorized child invocations of `cargo test -p shamir-server --test e2e http_login --no-run` attempted local compilation and both failed with exit 101. No successful compilation was observed and no tests are known to have executed. One child also ran formatting contrary to its edit-only workflow. These violations are disclosed, not retroactively authorized. Parent verification used only formatting/layout/read-only inspection; no parent compilation, tests, Cargo edits, git mutation or deletion was performed. Build artifacts were retained under the no-delete instruction.

CI must compile/lint/discover the mounted tests and execute the transport, connect, server unit and E2E suites. Passing fmt/tracked-layout is not evidence of runtime correctness or a complete memory/security proof. Do not commit/push/mark ready on this report's authority; the operator owns those actions and the remaining design work.

### Final handoff wording

Partial implementation remains in the HTTP-login wire/admission/pending/service modules, launcher/config/transport integration and two-launch E2E suite. The full request is not complete. Formatting and tracked-layout checks passed, but manual wire layout still exceeds seven entries. Tests were not executed; two unauthorized child compile-only attempts failed. Critical shutdown/compensation ownership, response/audit budget accounting, collision retry, metrics and coverage gaps remain. See this final report for source evidence and required CI/operator follow-up. No commits, pushes, deletions or dependency edits were made.


### Verification checkpoint (04:28 CET)

OBSERVED: the final scoped fmt checks for shamir-server, shamir-connect and shamir-transport-http and the tracked layout gate passed again. The chained gate then failed only on report trailing whitespace/blank EOF; this documentation-only correction addresses those diagnostics, and the parent will re-run git diff --check. Manual wire/ eight-entry violation remains. No tests or compilation were performed for this checkpoint. Earlier padding and sampling notes are superseded: ordinary BEGIN/FINISH paths sample uniform jitter; only policy BEGIN denials omit jitter, and the rejection sampler may exclude M values when M divides u64::MAX, not always at most M-1. This does not introduce modulo bias.

### Final verification receipt (04:32 CET)

OBSERVED by the parent: the chained scoped formatting checks for shamir-server, shamir-connect and shamir-transport-http, Python tracked-layout gate, and `git diff --check` all passed with exit 0 at 04:28:05 CET; whitespace check passed again at 04:31:32. Read-only Cargo manifest/lockfile diff was empty. Manual enumeration still finds eight entries in wire/; all new implementation/test source files checked with wc are below 1,000 lines (largest pending/store.rs: 444). No live child remained at the 04:31:32 inspection. A final independent bounded read-only glue/API review reported no concrete compile/API/lint finding; parent rechecked the cited LoginRoutes, LoginHead and LockoutStore definitions. This is not a successful compilation, runtime test, exhaustive rust-guard review, or full conformance proof. Final review began at 04:05 and the implementation remains PARTIAL. No additional feature work is authorized by these checks.

The next step is operator-owned lifecycle/resource-budget design and atomic layout cleanup, then CI compilation/lint/test-discovery and behavioral tests; this task has not met the full acceptance contract. In particular, do not treat formatting success as permission to merge the security-critical partial implementation.
