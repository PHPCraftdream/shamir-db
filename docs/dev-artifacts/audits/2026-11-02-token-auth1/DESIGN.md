
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

## Revision 3 progress — steps 1–2 (source only)

- Step 1: fixed C1 (both lockout assertions use the real Alice/source pair key), C2 (`ADMIN_PASSWORD` is `pub(crate)`), L1 (typed `spawn_blocking` result returned directly), and L2 (audit receiver remainder uses `remaining != 0`).
- Step 2: extracted the audit consumer into named async `run_worker(..)`, spawned with the current tracing span. Formatting and tests were not run; CI verification remains pending.

## Revision 3 progress — step 3 (source only)

- Step 3: parent reread `routes.rs` and `launcher.rs`. FINISH workers are detached `Arc`-owned in-flight tasks with an RAII completion guard; SeqCst shutdown-versus-handle-registration recheck closes the registration race. `shutdown(Duration) -> bool` is bounded; launcher uses five seconds and warns on timeout. Audit has an independent cancellation token and branch-local drop guard, canceled after graceful worker drain. Parent caught an introduced out-of-scope `audit_shutdown` name; a child corrected it before this checkpoint.
- Residual: runtime teardown may cancel at an await, leaving a lost-response token active for up to 24 hours; crash-durable compensation requires an out-of-scope journal. Worker-drain timeout or forced listener abort stops audit before remaining detached-worker events; no claim that audit always outlives timed-out workers.
- Source inspected, not CI green; no tests, compilation, check, or clippy commands were run this session.

## Revision 3 progress — step 4 (source only)

- Step 4: parent reread `table.rs` and `table_tests.rs`; root `parent_slot` and one-pass child/held marks remove nested scans, and subtree removal is one pass. Per-table `cfg(test)` slot visits cover find/child_count/free/eviction passes. New `full_table_eviction_scan_has_linear_slot_visit_bound` fills 2,048 distinct IPv4/16 root+child pairs, asserts 4,096 resident/Charged and nonzero visits `<= 20*4096`. Initial child `Option` mismatch, bad same-/48 test setup and counter borrow conflict were caught by parent and source repaired; CI tests unrun.

## Revision 3 progress — step 5 (partial; source only)

- Step 5 partial: E2E `security0` assertion and fresh compensation timestamp floor are source-present; hardened mismatched-default BEGIN->FINISH exactly-one failure test and synthetic mathematically-valid-proof ineligible test are source-present. Compensation injection is being implemented, not complete. Parent observed `cargo fmt -p shamir-server` exit 0; this is not compilation or test evidence.

## Revision 3 checkpoint — compensation injection and remaining deviations (source only)

- Compensation injection test source now exists. Its fixture token clock is advanced after the service pre-snapshot and before durable create/post-snapshot; assertions require the persisted record to be revoked and `active_count == 0`. The mirror retains one `REVOKED` entry by contract: `mirror_entry_count_hint` includes revoked entries. The requested literal mirror-count-zero oracle contradicted the API and was corrected; this is not a production change. The clock starts at 90e9 and advances to 91e9, and compensation asserts the fresh revoke time. The parent is still reviewing this source; it is not CI green.
- Ineligible-complete path tests are present, along with `security0` and S7 source changes.
- **Verification:** parent `cargo fmt --check` recently reported drift. Python layout passed, but the overall shell exit 0 came from layout being the last command; it does not mean fmt passed. No CI-green claim.
- **Known deviations/gaps:** S3 retains a per-listener audit queue. S5 still lacks pair-drop reasons, pairSlots native/unlock freshness, and head totals; real histogram buckets are not evidenced. E2E launches are b and d. S6 still lacks the blocking-thread redaction oracle and no-BEGIN coverage. S8 collision TOCTOU remains. Remaining S9 gaps: PendingId Debug/ID lookup is not constant-time; flat-policy BEGIN uses 50ms; expiry scan; input bucket pins; response permit/output accounting; audit-failure classification; bound field; origin diagnostic; and deploy-shutdown claim. Overall memory remains unproven.

## Revision 3 authoritative status — source complete for this revision; CI pending

This section supersedes earlier revision-3 checkpoints, not the preserved history. The mandatory edits in revision-3 steps 1–5 are present in source. Step 6 explicitly deferred the items below; step 7 is this corrected acceptance table. The complete HTTP-login feature remains PARTIAL and is not merge-ready without CI.

- C1 uses the real Alice/source pair at `crates/shamir-server/src/security/perimeter/http_login/service/tests/begin_tests.rs:36,89`; C2 is `pub(crate)` at `crates/shamir-server/tests/e2e/http_rest/common.rs:17`. L1 returns the typed blocking result directly at `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs:249`. L2/L3 use a named, formatted audit worker and `remaining != 0` at `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:106,215`.
- S1 uses detached workers, RAII in-flight accounting, a stopping recheck and bounded shutdown at `crates/shamir-server/src/security/perimeter/http_login/routes.rs:17,59,161`. The launcher gives workers five seconds and warns on timeout at `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:378`. S2 uses an independent audit token and branch-local drop guard at that file's lines 251 and 367, with explicit cancellation after worker drain at line 381.
- S4 replaces nested eviction scans with root-slot links and linear marking/selection/subtree passes at `crates/shamir-server/src/security/perimeter/http_login/admission/table.rs:17,249,264,324`. `full_table_eviction_scan_has_linear_slot_visit_bound` at `crates/shamir-server/src/security/perimeter/http_login/admission/tests/table_tests.rs:20` fills 4,096 slots as 2,048 distinct IPv4 root/child pairs and bounds all instrumented scans for a fresh source by 20 × 4,096 visits. This is a source-level complexity argument and unrun regression test, not measured throughput.
- Step 5 adds the class-zero assertion at `crates/shamir-server/tests/e2e/http_login/common.rs:227`, hardened mismatched-default BEGIN→FINISH accounting and synthetic valid-proof/ineligible tests at `crates/shamir-server/src/security/perimeter/http_login/service/tests/finish_tests.rs:238,307`, and compensation clock refresh/flooring at `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs:259`.
- Compensation injection now uses the existing token audit appender, not a pre-create clock callback: `crates/shamir-server/src/security/perimeter/http_login/service/tests/compensation_tests.rs:30` raises the user floor on `api_token_created`, after the durable store and mirror insertion but before the post-create snapshot. The helper at line 71 asserts exact 401, no pair failure, consumed claim, a persisted revoked record, and zero ACTIVE tokens. Three tests at lines 200,205,210 cover fresh, unavailable and backward clocks. The mirror deliberately retains one revoked record; `mirror_entry_count_hint == 0` would be an invalid oracle because that API includes revoked entries. Production mirror behavior was not changed to fit the test.
- Four route lifecycle tests in `crates/shamir-server/src/security/perimeter/http_login/service/tests/routes_tests.rs:29,70,111,167` cover empty shutdown, route-drop survival, bounded wait/receiver cancellation/permit release, and simultaneous waiters. They use malformed BEGIN padding and paused time to avoid blocking-pool scheduling as their synchronization oracle; they do not alone prove durable FINISH completion under forced shutdown.

### Acceptance 4–12

Statuses below mean source coverage, not observed passing tests. All listed tests need CI compilation, discovery, linting and execution.

| Item | Status | Source evidence and remaining CI/coverage work |
|---|---|---|
| 4 Successful login | DONE in source; CI pending | `crates/shamir-server/tests/e2e/http_login/login_flow.rs:11` covers proof/signature, returned ID/owner/TTL, Ping and an explicitly granted read. C2 is corrected; execute the unit success and E2E flows in CI. |
| 5 Failures | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/tests/finish_tests.rs:238,307,431` covers ineligible, wrong-proof, replay and exactly-one failure. Still missing the three-way next-BEGIN Retry-After parity, 61-second expiry through FINISH and strict last-failure timestamp equality case. |
| 6 Policy | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/tests/compensation_tests.rs:71,200` now covers a floor change after durable creation and revocation timestamps. Replicator, token-limit, role-change and FINISH-time read-write/replica recheck coverage remain incomplete. |
| 7 Admission | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/admission/tests/table_tests.rs:20` covers the full-table work bound; existing pin/churn and slot/pending tests remain. Explicit atomic/no-refund, Dropped fallback and 503 mapping with existing pending state preserved remain incomplete. |
| 8 Classes/proxies | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/admission/head.rs:90` and existing head/config tests cover the policy path. Class-zero non-loopback rejection and TLS/HSTS/restart E2E launch (d) remain missing. |
| 9 Lockout | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs:106` uses the shared native/HTTP pair store. Native↔HTTP shared-pair, HTTP admin-unlock and pair-drop accounting coverage remain missing. |
| 10 Metrics | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/metrics.rs:50,54,67` contains fixed vocabularies and duration recording. Pair-quota/table-full reasons, native/unlock-live occupancy, head totals and real fixed histogram buckets remain missing. |
| 11 Audit/redaction | PARTIAL | `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:251,367,381` repairs graceful audit ordering; `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:106` is readable/formatted. Per-listener queue budgeting, late events after drain timeout/abort and the blocking-thread/BEGIN redaction oracle remain open. |
| 12 Transcript vectors | DONE in source; CI pending | Unchanged `crates/shamir-connect/src/server/auth/http_login/tests/transcript_vector_tests.rs:53` and existing proof/boundary tests remain mounted. No new vector execution result is claimed. |

### Known deviations and residual risk

- S3: one audit queue per listener, not one process-wide queue. S5: pair-drop reasons, pair-slot freshness and head totals remain incomplete; histogram buckets are deferred. E2E launches (b) and (d) are MISSING, not present.
- S6: the existing FINISH redaction subscriber misses blocking-thread events and does not cover BEGIN. S8: handshake-ID check/insert collision retry remains a TOCTOU gap.
- Remaining S9: `PendingId` Debug can expose the ID; separately, ID lookup is not constant-time. Policy BEGIN denial has flat 50 ms padding; expiry scans instead of an ordered queue; in-flight inputs do not pin budget buckets; input permits end before response writing; audit failure classification, the unused bound-field workaround, Origin startup diagnostics and deployment shutdown wording remain as reviewed. The full allocator/task/input/output/audit memory proof is still absent.
- Detached work survives listener/route destruction, not runtime teardown or process crash. A lost response can leave an active token for at most its configured 24-hour TTL. Crash-durable compensation requires an out-of-scope journal. A five-second worker timeout or forced listener abort may stop the audit consumer before remaining detached workers emit their final events; no claim that audit always outlives timed-out workers.
- Manual directory inventory: `http_login/` 7, `service/` 7, `service/tests/` 7, `admission/` 6, `admission/tests/` 4; the current wire directory is 7, so the older eight-entry cleanup statements are stale. Every changed/new source file is below 1,000 lines. New test files are mounted by the existing service test manifest.
- Parent-observed scoped fmt and Python layout checks passed during source review; a final receipt follows after the final read-through. No compilation, clippy, builds, behavioral tests, npm, Cargo manifest/lockfile edits, git index mutation, commit, push, moves or file deletion were performed in this revision. Documentation append tooling accidentally split two historical tables; the original text was reconstructed and the parent checked the diff for preservation before this appendix.

Revision-3 source work is complete subject to final read-through and CI; the broader feature remains PARTIAL with the explicit deferred items above.

## Revision 3 final-review checkpoint (06:18 CET; ongoing)

OBSERVED: the parent started the feature-frozen final review at 06:15:11 CET and reread every revision-3 touched/new file in full, including both new mounted test files and all of this document. Review continues through the final window; no 30-minute completion claim is made yet. The scoped `cargo fmt -p shamir-server -- --check`, `python -I scripts/ci/check_layout.py` and `git diff --check` chain passed with exit 0 at 06:16:28 CET. Manual listings include the untracked tests: service/tests has seven entries; all changed/new source files remain below 1,000 lines. Cargo manifest/lockfile diff is empty. Compiler, clippy and behavioral results remain UNVERIFIED; no such commands were run in revision 3. The source/API cross-check covers the five compensation call sites, two context-clock calls, durable-create → audit injection → post-create snapshot ordering, retained revoked mirror semantics, RPITIT Send bounds, guard drop paths and enabled Notify registration. No new source repair was identified in this bounded read; CI is still required.

## Final report — revision 3 disposition (06:40 CET)

Revision-3 steps 1–7 are implemented at source level: C1/C2 and L1–L3 repairs; detached route workers with RAII in-flight accounting and bounded shutdown; independent audit cancellation ordered after graceful worker drain; linear admission eviction with a deterministic full-table work-bound test; class-zero assertion, ineligible-FINISH exactly-once accounting tests, post-durable-create floor-bump compensation injection, and fresh/unavailable/backward compensation clocks. The authoritative acceptance table at lines 474–488 remains the current source-only status. The broader HTTP-login feature is PARTIAL, not merge-ready.

Changed source is confined to `shamir-server`: HTTP launcher; admission table and tests; routes; audit/BEGIN/FINISH services; BEGIN/FINISH test files, fixture and test manifest; new `compensation_tests.rs` and `routes_tests.rs`; E2E HTTP-login and HTTP-REST common helpers. This document records the handoff. No Cargo manifest/lockfile, dependency, git-index, commit, push, file move or file deletion was made in revision 3.

OBSERVED verification: scoped fmt and Python layout passed again at 06:40:10 CET. The full fmt/layout/whitespace chain passed at 06:16:28 CET; source fmt/layout/whitespace passed again after 06:40, and the parent observed the complete document whitespace check passing at 06:41:11 CET after the final report append. Manual inventory includes both untracked mounted tests, seven service-test entries, and all touched source files below 1,000 lines. Every revision-3 touched/new file was reread in full after 06:15; subsequent source/API review covered cancellation, Send/static ownership, Notify registration, root-slot validity, pin lifetimes, exact failure accounting and durable store/mirror compensation oracles. This was a bounded review in the final 06:15–06:45 window, not 30 continuous minutes of active reading or a compiler/linter proof. No compilation, cargo check, clippy, builds, behavioral tests, rustc or npm commands were run in revision 3. CI compilation, linting, test discovery and execution remain REQUIRED and UNVERIFIED.

Deferred work remains S3 process-wide audit budgeting, S5 pair-drop/live-occupancy/head metrics, fixed histogram buckets, E2E launches (b)/(d), S6 blocking-thread/BEGIN redaction coverage, S8 collision retry, remaining S9 items, the full charged-memory proof and missing acceptance-matrix cases. Detached workers survive route/listener destruction but not runtime teardown or process crash; a lost response may leave a token active for its configured TTL, at most 24 hours. Audit may stop before late detached-worker events after drain timeout or listener abort. These residual risks are not represented as resolved. Next step is operator-reviewed CI and the documented deferred work; no local behavioral result or authorization to merge is implied.

## Tails — 2026-11-03

OBSERVED parent date: 15:07:10 CET; deadline 17:00 CET; final 40-minute read starts 16:20 CET.

Status: IN_PROGRESS for all 8 items. No claims implemented. This is a crash-resilient report start; item status and evidence remain to be recorded during review.



- Item 4/S9 PARTIAL (source-read): `crates/shamir-server/src/security/perimeter/http_login/admission/state.rs:126-129` attaches arrival/credit pins to admitted InputPermit; `crates/shamir-server/src/security/perimeter/http_login/admission/head.rs:45` rejects class-zero nonloopback bind and removes unused bound field; `crates/shamir-server/src/security/perimeter/http_login/admission/tests/head_tests.rs:27` adds constructor regression. `crates/shamir-server/src/security/perimeter/http_login/routes.rs:143-187` records admit_head rejection totals, but transport-level pre-hook rejects still bypass it. Input permit still drops at `crates/shamir-server/src/security/perimeter/http_login/routes.rs:211` before transport write; output accounting remains BLOCKED pending transport lifecycle design, not claimed fixed. Admission matrix tests remain in progress; no compilation/tests run.
- OBSERVED: current parent report placement restored, preserving original revision 3 line 499; Tails is now at EOF.
- Item 3/S8 and item 4 pending/BEGIN PARTIAL (source review): `crates/shamir-server/src/security/perimeter/http_login/pending/store.rs` now classifies live-ID collision inside the insertion lock and redacts PendingId Debug; `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs` retries insertion up to three generated IDs and samples policy-denial jitter after the policy decision. The first child pass introduced duplicate imports, an undeclared subtle import, moved-key retry and duplicate-match syntax defects; parent source review identified them and delegated repairs. Ordered expiry is under correction because released claims and stale queue records were not handled safely in the first pass. Forced collision is tested only at store insertion so far, not through BEGIN retry. No compiled or runtime result is claimed.
- Item 6/S3 PARTIAL: source introduces a static 1,024-event semaphore shared by listener audit queues in `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs`; parent review found channel-capacity regression, permit release before blocking I/O completes, and malformed loop closure. Corrections are in progress; aggregate queued/in-flight memory and late-event accounting are not certified. RateLimited/ServerBusy no longer count as credential failures and compensation events use critical outcome in source. No tests ran.
- Item 8/N1 PARTIAL: launcher source derives worker drain from `request_timeout_ms`, but the outer ServerHandle listener deadline remains 10 seconds plus abort; a proof/test covering that bound is missing. Origin count diagnostic source exists; the added diagnostic test does not exercise the diagnostic. Deployment/example shutdown wording still needs targeted correction.
- Command policy clarification: no child compile/test invocation has been observed in this tails session; references to three children/2 GB describe earlier runs. Parent fmt --check has failed on syntax/formatting; behavioral checks remain forbidden.
- Items 1 and 2 PARTIAL (source): acceptance-test additions now include three-way bad-FINISH/next-BEGIN Retry-After parity, 61-second FINISH expiry using a fixture clock, strict last-failure equality, replicator/token-cap/read-write/replica checks, and shared-pair/admin-unlock paths in service/tests/finish_tests.rs; route tests now preserve and subsequently claim an existing pending entry after a global-input 503. E2E policy/TLS/restart sources exist, but multiple child passes lost baseline test structure or introduced API/nonce/connection-reuse defects; parent review delegated corrections, preserving original disabled-route coverage. No runtime results are claimed; role-change compensation is still being added.
- Item 7/S5 PARTIAL (source): connect lockout.rs adds PairDropReason and PairStoreObserver plus observer constructors, with mutation occupancy deltas and quota/table-full events; server launcher wires fixed accounting_dropped labels. Actual Prometheus builder bucket configuration is source-present for BEGIN and FINISH; cached 0.16.2 signature supports the chosen Result handling, but resolved 0.18 source/compiled API remains unverified. Transport pre-hook head totals remain outside the application hook.
- Item 3/S8 source now includes a cfg(test) injected-ID path and collision/retry/exhaustion/entropy tests; parent caught and repaired an undefined parsed request in the first helper. Pending expiry uses a bounded heap with per-index cleanup and fixed-width constant-time ID comparisons. Verification is source-only, CI required.
- OBSERVED: parent cargo fmt for shamir-server/shamir-connect completed with exit 0 after syntax repairs; Python layout gate passed. A fresh fmt check and full final read are still pending. An accidental empty untracked root DESIGN.md exists; it is retained under the no-delete instruction and is not the report.

### Final-review checkpoint (16:24 CET; ongoing)

This checkpoint supersedes conflicting Tails status statements above; prior text is preserved. Status is source-only, and the final per-item report remains pending.

- **Review/freeze:** Parent reread all 36 changed/new files, including the empty root `DESIGN.md`. No compiler claim follows from that source read. The final 16:20–17:00 review is ongoing and not complete. Parent observed scoped fmt/check exit 0 and layout OK; a newer one-line correction means those checks need repeating.
- **Concrete source fixes:** `proof_for_mode` was corrected to `proof_for`; validated salt storage now uses `[u8; 16]` rather than `ByteBuf`; an unused import and unnecessary parentheses were removed; the test now exercises the actual drain helper.
- **Dependency/API evidence:** `Cargo.lock` records `metrics-exporter-prometheus` version `0.18.3` at lines 2134–2135. Its relevant signature remains unverified; do not treat the earlier 0.16.2 signature assumption as proof.
- **Deployment wording:** `deploy/server.example.ktav` lines 121–123 now accurately describe volatile state, outer shutdown preemption, and late-audit best effort.
- **Eight-item disposition:** All eight items remain source-only; item 3 may be marked DONE at source level, with CI pending. The final per-item report is still pending.
- **Known gaps:** `routes.rs:238` drops the response permit before writing. Transport pre-hook/head and body-read errors bypass metrics in `route_handler.rs:26–140,185–224`. Shared-lockout coverage directly calls store/admin methods, not native wire/admin dispatch. Role-change handling also floor-bumps. Redaction capture covers appender dispatch, not all blocking `TokenService` logs. The logical 1,024-audit semaphore is not proof of a physical single queue or 1 MiB bound. Pair-gauge teardown/replacement limitations remain. The outer 10-second listener deadline can preempt drain.
- **Verification limits:** No compilation, clippy, build, or tests were run in this tails session. Only the explicitly allowed parent formatting and Python layout checks are permitted; repeat them after the latest correction. The accidental empty root `DESIGN.md` is retained. No commands were run for this checkpoint.

### Tails disposition — source review, CI pending

Overall: **PARTIAL — not implemented in full and not merge-ready.** All findings below are source evidence only; CI compilation, linting, test discovery and execution remain pending.

| # | Status | Evidence and disposition |
|---|---|---|
| 1 | PARTIAL | Acceptance source tests: `crates/shamir-server/src/security/perimeter/http_login/service/tests/finish_tests.rs:395,417,441,470,550,574,636,711` cover expiry, equality, replicator, write/read-write, replica, cap, three-way parity and shared-store cases. `crates/shamir-server/src/security/perimeter/http_login/service/tests/compensation_tests.rs:405` covers role compensation; the updated `update_roles(..., 0)` path preserves the floor timestamp. `crates/shamir-server/src/security/perimeter/http_login/admission/tests/table_tests.rs:20,42`, `crates/shamir-server/src/security/perimeter/http_login/admission/tests/slot_tests.rs:22`, `crates/shamir-server/src/security/perimeter/http_login/service/tests/routes_tests.rs:92` cover atomic admission, Dropped, no-refund, and pending-preserving 503. `crates/shamir-server/src/security/perimeter/http_login/admission/tests/head_tests.rs:27,50` and `crates/shamir-server/src/security/perimeter/http_login/service/tests/routes_tests.rs:32` check class zero. Real native wire and admin-dispatch tests are missing. |
| 2 | PARTIAL | `crates/shamir-server/tests/e2e/http_login/login_policy.rs:102,136` covers read-write and eligible-superuser 403; `crates/shamir-server/tests/e2e/http_login/login_lifecycle.rs:12` covers TLS 1.3/trust, HSTS, stable restart ID, old pending 401 and fresh 200. These are unrun; new read-write/TLS successes do not verify server signature or token use and are not a full client oracle. |
| 3 | DONE in source; CI pending | `crates/shamir-server/src/security/perimeter/http_login/pending/store.rs:186,248,386` implements atomic collision handling, constant-time comparison and ordered expiry; `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs:47,180` implements policy-before-jitter and three tries. `crates/shamir-server/src/security/perimeter/http_login/service/tests/begin_tests.rs:101,130,159` covers forced retry, exhaustion and entropy. |
| 4 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/pending/store.rs:24,248,386`; `crates/shamir-server/src/security/perimeter/http_login/service/begin.rs:47`; `crates/shamir-server/src/security/perimeter/http_login/admission/state.rs:126`; `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:109,210`; `crates/shamir-server/src/security/perimeter/http_login/admission/head.rs:46`; `crates/shamir-server/src/security/perimeter/http_login/routes.rs:62`; `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:102`; `crates/shamir-server/src/security/perimeter/http_login/routes.rs:205`; `deploy/server.example.ktav:121`; `crates/shamir-server/src/engine/server/server_handle.rs:183` evidence Debug redaction, CT lookup, ordered expiry, policy ordering, pins, audit classification, bounded field removal, origin/launcher handling, and docs/shutdown. Response permit still drops at `crates/shamir-server/src/security/perimeter/http_login/routes.rs:238` before write; resolving this requires transport lifecycle handoff, not body-poll facade. Existing completion markers can still scan. Claimed capacity remains held until Drop by normative contract. AlreadyClaimed and Expired both map to generic 401; this is not a new bug. |
| 5 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/tests/compensation_tests.rs:164` injects real BEGIN/FINISH with a blocking appender and thread sentinel; explicit appender dispatch is captured and snapshots propagated. Blocking `TokenService` logs outside explicit appender dispatch are not covered. |
| 6 | PARTIAL | `crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:16,58,98,238,304` implements static 1,024 semaphore, startup, queued permits, blocking capture and cap; `crates/shamir-server/src/security/perimeter/http_login/service/tests/audit_tests.rs:114` exercises consumers. Logical queued/in-flight event capacity is bounded, but per-listener physical queues remain (`crates/shamir-server/src/security/perimeter/http_login/service/audit.rs:76`) and aggregate 1 MiB allocator proof is absent; late events are documented. |
| 7 | PARTIAL | `crates/shamir-connect/src/server/limits/lockout.rs:284,293,356,378,607,750,775` adds pair-drop reason/observer, constructor wiring, mutation and unlock/GC paths; `crates/shamir-server/src/engine/server/server_launcher/launch.rs:276` uses fixed labels. Hook tests: `crates/shamir-connect/src/server/tests/security/lockout_storage_tests.rs:703,750`. `crates/shamir-server/src/security/perimeter/http_login/routes.rs:165` counts application-head totals, but transport pre-hook/body-read/timeout bypasses remain (`crates/shamir-transport-http/src/server/routes/login/route_handler.rs:26,140,185,224`). `crates/shamir-server/src/security/observability/mod.rs:317` has configured fixed buckets. `Cargo.lock:2135` records 0.18.3; exact API remains locally unverified because dependency source is absent. No Drop decrement/observer replacement; single-process multi-server counts may exceed 8192; new-pair scan cost is unmeasured. |
| 8 | DONE in source; CI pending | `crates/shamir-server/src/security/perimeter/http_login/routes.rs:75` uses min(request timeout, 120s); `crates/shamir-server/src/engine/server/server_launcher/http/launcher.rs:432` consumes it; `crates/shamir-server/src/security/perimeter/http_login/service/tests/routes_tests.rs:189,287` covers bounds and derived short shutdown. Outer `crates/shamir-server/src/engine/server/server_handle.rs:20,192` 10-second abort can still preempt; docs do not claim full completion. |

### Verification and remaining limits

Only parent-allowed scoped formatting/check and layout passed with exit 0. No compilation, clippy, build, test discovery, runtime test, or CI was run; no source claim is green. The full review window 16:20–17:00 was still in progress at draft time and is not claimed complete. Cryptographic implementation reuses existing CT/HMAC/CSPRNG helpers; no new dependencies or unsafe code. Full memory proof and pin/3072-spec reconciliation remain absent: current input-plus-pending-pin union may exceed 3072 while the table cap is 4096; report this, do not claim fixed. Root empty `DESIGN.md` retained; no git mutation/deletion; concurrent `token_matrix` and main untouched.

### Final handoff wording

PARTIAL — the brief is not implemented in full and is not merge-ready. Changes cover HTTP-login admission/pending/services/tests, server launcher/observability, the connect lockout observer, E2E login policy/TLS restart sources, and shutdown documentation/example configuration. Scoped formatting and layout checks passed; no compilation, clippy, test discovery or behavioral tests were run. Remaining work includes response-write permit ownership, complete transport outcome metrics, exhaustive blocking-thread redaction, real native/admin cross-path tests, and certified aggregate memory/audit bounds. The accidental empty root DESIGN.md is retained. Per-item evidence and limitations are recorded in docs/dev-artifacts/audits/2026-11-02-token-auth1/DESIGN.md under Tails.

### Final verification receipt

OBSERVED final permitted gates after restoring baseline lockout assertions at `crates/shamir-connect/src/server/tests/security/lockout_storage_tests.rs:161–162`: `cargo fmt -p shamir-server -p shamir-connect` (job01D) exit0; `cargo fmt -p shamir-server -p shamir-connect -- --check` (job01E) exit0; `python -I scripts/ci/check_layout.py`: layout check: OK (0 baselined, 0 hint(s)). Parent diff shows observer imports/new tests without baseline assertion loss. No source changes followed these gates. No compilation, clippy, discovery, tests or CI. Review is bounded manual source review, not compiler proof or 40 continuous minutes. Overall PARTIAL, not merge-ready.


### Tails review fixes (orchestrator, source only, CI pending)

Two independent compiler-style reviews of the tails commit were applied. No compilation or test result is claimed.

- N1 corrected: the earlier `min(request_timeout, 120 s)` worker drain exceeded the server's 10 s listener deadline (workers 120 s + audit 2 s), so a hung worker made shutdown unclean. The worker drain is now capped at 7 s (`WORKER_DRAIN_CAP` in `http_login/routes.rs`), leaving the 2 s audit drain inside the 10 s deadline; the unit test asserts the cap and the docs/example config state 7 s.
- Pair-slots gauge: the constructors no longer push an initial occupancy delta (the recorder did not exist yet and the delta was lost); `InMemoryLockoutStore::occupied_pair_count` is public and the launcher republishes the absolute value every 30 s. The observer now reports only real changes (no zero deltas) and the occupancy scans run only when an observer is installed; the drop reason is computed lazily after a failed admission.
- Admission: a `Dropped` debit (no evictable bucket, possible once in-flight inputs pin buckets) now fails closed with `ServerBusy` instead of admitting an undebited source.
- Metrics: a 404 from a disabled route is counted as `InvalidRequest`, not as an authentication failure. Histogram bucket configuration moved out of `observability/mod.rs` into `observability/login_histogram.rs`.
- Test fixes: clippy `needless_borrows_for_generic_args` in `compensation_tests.rs`, a tautological Origin sentinel assertion removed from `audit_tests.rs`, the e2e `login_as` helper no longer derives the Argon2 keys twice.
- Still open (unchanged status): S9 response-write permit ownership (the input permit is released before the transport writes the response; this needs a transport API change), S6 blocking-thread redaction (the compensation test captures through an explicitly passed dispatcher, so it does not prove propagation into `TokenService` blocking logs), per-listener physical audit queues, full memory proof, the ordered expiry queue still scans on reclaim, and the `TOKEN_LOGIN_HTTP.md` status banner still says "not implemented".

## Tails 2 — 2026-11-03

Parent observed date: 2026-11-03 20:19:02 CET. Deadline: 22:30 CET. Feature cutoff: 21:50 CET; final 40 minutes reserved for compiler-style review.

Items 1–7: IN_PROGRESS, source-only. No local compilation or tests were run. Children are edit-only; no manifests, lockfiles, or dependencies may be changed. The allowed local checks, if authorized by the parent, are limited to `cargo fmt -p shamir-server -p shamir-connect -p shamir-transport-http` and `cargo fmt -p shamir-server -p shamir-connect -p shamir-transport-http -- --check`, plus `python -I scripts/ci/check_layout.py`.

Item6 BLOCKED — observed service/audit.rs:53–90 creates per-listener bounded channel, writer-specific worker and JoinHandle; run_worker:138–267 owns listener cancellation, aggregate counters and drain; replacing with process-wide physical queue needs routing to independent AuditChainWriters, listener-scoped shutdown barriers/joins and reinitializable runtime ownership. A static first-runtime receiver is not sufficient across runtime teardown/restart. Per brief no larger lifecycle redesign attempted; existing global1024 logical semaphore retained, aggregate physical1MiB proof not claimed; tests/compilation unrun. No child cargo check/clippy/build/test/npm/tsc; parent only fmt three scoped crates and Python layout. No manifests/lock/dependency/git mutations.

| 4 | DONE, source; CI pending | `lockout.rs` owns an `AtomicUsize` occupancy mirror updated under the store mutex for insertion/free, eviction, removal, and one-time rehydration; `occupied_pair_count` is O(1). The observer reports absolute `usize` snapshots; replacement clears the old observer and reports zero, and store Drop reports zero. Tests cover absolute values, observer replacement/Drop, reload, and multiple stores; quota-64 and full-8192 assertions were updated. The launcher sets the gauge rather than incrementing it; each store reports its own snapshot, not a global sum. Parent reviewed the actual diff and edited sections, fixing the missing report method, E0509 struct-update issue, and expected sequences. Tests unrun; no compile/runtime claim. Full-file final review remains pending.

### Final report — Tails 2 (22:25 CET, deadline reached)

Overall: PARTIAL — all seven items source-only; no compilation, clippy, test discovery or execution; CI required before any merge claim.

- Item 1 PARTIAL: permit now shared with `handle` as `Arc<Permit>` and retained on body-read errors, handler timeout and stopped-route replies; success-path hold travels inside the app `LoginResponse::with_hold` into the response extension, moved by `service.rs` into `GuardedIo`'s hold channel and released exactly once at full-response flush or connection drop (`guarded_io.rs`). New transport tests: write+flush release, stalled-write abort release, response-extension survival. Not done: full connection-level integration test; response write path of canned/interim responses not exercised.
- Item 2 DONE in source; CI pending: `token_service.rs` + `gc.rs` capture the caller dispatcher before every `spawn_blocking` and re-enter via `with_default` around each full body; `compensation_tests.rs` installs a normal thread-default subscriber (dispatcher never passed to the appender); the blocking appender alone emits the sentinel; secrets asserted absent. Test `begin_finish_redaction_captures_blocking_audit_control` unrun.
- Item 3 PARTIAL: `reclaim_released` replaced by a bounded `mpsc::channel(CAPACITY)` release queue drained per released entry (invariant comment: claimed slots cannot be reused before their one Drop notification is drained), with cfg(test) visit counter and proportionality test `reclaim_released_work_is_proportional_to_release_count_at_capacity`; final claim/expiry-heap interplay recheck was still in progress when the deadline hit (worker unfinished), so treat the queue-reuse edge as unreviewed.
- Item 4 DONE in source; CI pending: per-store `AtomicUsize` occupancy mirror updated at insert/evict/removal under the mutex, one rehydrate scan; O(1) `occupied_pair_count`; observer signature is absolute `usize`; replacement reports 0 to the old observer, store Drop reports 0; launcher gauge uses `set`. Tests for absolute values, replacement/drop/reload, multiple stores. Note: per-store gauge snapshots are not a global sum across multiple stores in one process.
- Item 5 PARTIAL: conservative pin lease added (`pinned_buckets.fetch_update(+3, cap 3072)` under the bounded table lock; `BudgetPin` carries a shared lease; cap failure returns None fail-closed); `pin_source`/`admit` map cap exhaustion to `ServerBusy`; new `pin_tests.rs` at 3,072 cap and slot_tests fail-closed coverage. Not done: full union accounting review (input+pending pins) and exact no-bypass proof were still being checked by the unfinished worker; treat bound as asserted-by-tests, not proven.
- Item 6 BLOCKED with evidence recorded above (per-listener channel/worker/writer/cancellation lifecycle; static receiver cannot survive runtime teardown/restart).
- Item 7 PARTIAL: `LoginRoutes::refused` hook; app records transport-refusal results; `GuardedIo::with_refusal_hook` covers raw framing/head-timeout refusals exactly once per request (partial routes fail closed, documented); body-phase refusals counted via route_handler; transport tests for 413 and framing-hook counts. Not done: app-level metric assertions and canned-response-path hook coverage.

Verification (parent-run, allowed only): `cargo fmt -p shamir-server -p shamir-connect -p shamir-transport-http` then `-- --check` exit 0; `python -I scripts/ci/check_layout.py` OK (0 baselined, 0 hints). No other commands. Known compile-risk left for CI: `PendingStore::claim` was downgraded to `pub(super)` during item 3 edits — `crates/shamir-server/src/security/perimeter/http_login/service/finish.rs` calls it across modules; if CI reports E0624 there, restore `pub async fn claim`. A child worker for items 3/5 was still active at the deadline; per its last checkpoint both items are PARTIAL and its edited files were not fully reread by that worker.


### Tails 2 review corrections (orchestrator, source only, CI pending)

Two independent compiler-style reviews of the tails-2 commit were applied: the transport fixes (a borrow error in `route_handler`, a missing `BodyExt` import, `unused_io_amount`, local `use` in tests, the `RefusalHook` alias, the refusal route taken from the scanner instead of a stale `"/"`, no double count of a 408 during the response phase, the response hold attached to every reply, the shutdown in the hold test) and the server fixes (duplicate import in `pending/store.rs`, `handle(Arc<Permit>)` callers in `routes_tests.rs` and the slot check before the 16-slot loop).

**Item 5 (pin budget) was reverted to the code of master.** The half-finished lease counted leases, not buckets: with a cap of 3,072 and three units per lease at most 1,024 leases fit, shared between the input slots and the pending entries, so roughly 512 concurrent logins or about a thousand BEGIN requests from spread prefixes answer every `admit` with `ServerBusy` (a regression against master), `BudgetPin::clone` no longer incremented the markers while `drop` decremented them (an `AtomicUsize` underflow) and three of its tests could not pass. A correct design counts unique pinned buckets (reserve one unit only on the 0 to 1 transition of a bucket, release on 1 to 0) and is left open. The fail-closed `ServerBusy` on a dropped debit from the first tails round stays.

Still open after this round: items 1 (the transport path of the response hold has no end-to-end test; the hold can outlive a dead connection by the linger window; a handler timeout is counted by the transport and again by the detached worker), 3 (reclaim is proportional, but `insert` and `claim` still run O(capacity) passes), 5, 6, 7 (metrics of refusals before the hook), the absolute gauge can briefly show an older value until the 30-second republish, the gauge is process-wide and not per store, and the blocking `spawn_blocking` calls in `tokens/store.rs` still log outside the caller's dispatcher.

## Tails 3 — 2026-11-04

(append-only progress; final report at the end)
- Item 1 DONE, source; CI pending: pin budget now counts unique pinned buckets — `LoginBudgetTable` carries an `AtomicUsize` budget capped at `PIN_BUDGET = 3_072`; `pin` reserves one unit only on a bucket marker's 0-to-1 transition (`fetch_update` with the cap), rolls back markers and units on refusal and returns `None`; `BudgetPin::drop` releases one unit per 1-to-0 transition, `Clone` only increments markers, no lease object. `pin_source` returns `Option` and `admit` maps a pin refusal to `ServerBusy` with `record_budget_drop("pin_budget")` (permit dropped, slot freed); `begin` maps a `pin_source` refusal to `busy()`. A FINISH whose source is already pinned through its pending entry costs nothing (markers >= 1). Tests: exact 3,072 bound, clone/drop no drift, two clones release once, refused pin leaves counters unchanged, `pin_source` of an already-pinned source succeeds at the cap and re-admit after a permit drop succeeds at the cap. Existing pin-count tests were adapted to the cap (2047 debits kept, only 1500/1535 pins stored). A lease-counting regression was not reintroduced.
- Item 2 DONE, source; CI pending: `GuardedIo::release_holds()` added and called right after `into_parts()` in `accept.rs` before drain/linger; e2e tests in `login_routing_tests` prove the hold is alive while a stalled client has not drained a 64 KiB response, released exactly once after full read, and released exactly once when a stalled client is aborted (300 ms write deadline). `release_holds_drops_pending_login_hold_immediately` covers the unit path.
- Item 3 DONE, source; CI pending: `route_handler` no longer counts a 408 after the request was handed to `routes.handle` (the detached application worker records the outcome); pre-`handle` refusals still count. `result_for_status` and the app `refused` mapping are unified into one function (408/431 mapped to InvalidRequest). Tests: bad Content-Type refusal recorded exactly once (415); handler timeout after hand-off records nothing at the transport.
- Item 4 DONE, source; CI pending: all five `spawn_blocking` sites in `tokens/store.rs` (open/scan/get/put/delete) capture `tracing::dispatcher::get_default(Clone::clone)` and re-enter with `with_default` around the whole body, matching gc.rs/token_service.rs. The redaction sentinel oracle could not cover store.rs without a new store-side audit-appender fixture and was not extended.
- Item 5 DONE, source; CI pending: observer publications in `lockout.rs` (`register_failure` both branches, `reset_on_success`, `admin_unlock_user`, `gc`) now run while the slots mutex is held; rehydrate and Drop were already under the lock/safe; the `PairStoreObserver` doc and the stale "deltas" comment in `launch.rs` were corrected. Test `observer_publications_are_ordered_by_the_store_mutex` uses a recording observer that blocks once on occupancy 1 and proves a second registrar cannot publish until the first publication returns.
- Item 6 DONE, source; CI pending: the login hold hand-off in `service.rs` uses `try_send` (failed send = dead connection, hold dropped, slot released) and the needless `into_parts`/`from_parts` round trip was removed.
- Verification (parent-run, allowed only): `cargo fmt -p shamir-server -p shamir-connect -p shamir-transport-http` then `-- --check` exit 0; `python -I scripts/ci/check_layout.py` OK (0 baselined, 0 hints). No compilation, clippy or tests were run anywhere (children are edit-only and ran no commands). One test bug introduced by a child (`.expect("pin within budget")` on iterations past the exact 3,072 cap in two budget tests) was caught in review and fixed. Item-1 slot test deviation: IPv6 sources put the varying group in the 3rd group (unique /48 per source) because the brief's literal address exhausts the shared /48 rate budget at 161 admits; the test's invariants are otherwise exactly as briefed.
- Final report — Tails 3: all six items DONE, source only; nothing compiled or run locally; CI pending before any merge claim. Touched files: `admission/table.rs`, `admission/state.rs`, `service/begin.rs`, `admission/tests/{pin,slot,table}_tests.rs`, `http_login/routes.rs` (server), `tokens/store.rs`, `launch.rs`, `lockout.rs` + `lockout_tests.rs` (connect), transport `route_handler.rs`, `service.rs`, `accept.rs`, `guarded_io.rs`, `guarded_io_tests.rs`, `login_routing_tests.rs`. No Cargo/lockfile/dependency/workflow changes, no git mutations, no machine-local paths written.

### Tails 3 review corrections (orchestrator, source only, CI pending)

The executor's own reviewer rejected the tails-3 result; corrections applied: the observer-ordering test no longer holds its signalling mutex while parked (an `AtomicBool` replaces it, the previous version deadlocked before releasing its gate); `attach_hold` now keeps a hold the application already attached (both live until the flush; before, the transport insert replaced the application hold, which made the new end-to-end hold tests meaningless).

**Item 1 (pin budget by unique buckets) is BLOCKED again and was reverted to the code of master for the second time.** The tails-3 attempt passed two closures to `AtomicUsize::fetch_update` (a compile error), decided the reservation from a separate marker load (a lost-accounting race against `BudgetPin::drop`), rolled back by a per-call flag instead of the 1-to-0 transition, and left three tests with impossible expectations (a refusal test that never fills the budget, two full-table tests that keep unpinned evictable buckets). A correct implementation needs the 0-to-1 reservation and the 1-to-0 release to be one atomic step per marker (for example reserve under the table lock and release in `Drop` via the same `fetch_sub` transition result) and tests built from a helper that really pins 3,072 buckets; it should be written and reviewed on its own.

Still open: the periodic gauge task in `launch.rs` reads the occupancy and publishes it outside the store mutex and can overwrite a newer mutation publication; the detached application worker's eventual result is not asserted by the handler-timeout test (it checks transport silence only).

Superseded lines of the "Tails 3" section above: every statement that item 1 (pin budget) is DONE, the mention of `admission/*` and `service/begin.rs` among the changed files, and the "all six items DONE" final report are superseded by the corrections above (item 1 was reverted to master). The periodic gauge task now calls `InMemoryLockoutStore::republish_occupancy`, which publishes under the store mutex, so a stale periodic write can no longer overwrite a newer mutation publication. Remaining findings of the last review, not fixed: the stalled-client hold test does not assert that the hold is alive while the client stalls and does not pass through `accept.rs` (no test protects the `release_holds()` call there); the `stopping` and receiver-error replies of the login routes return 503 without recording a result.
