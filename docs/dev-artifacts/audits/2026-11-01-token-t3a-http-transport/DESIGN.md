# TOKEN-T3a HTTP transport design and implementation status

Status is based on end-to-end source/fixture review, formatting and layout checks only. **No compile, tests, clippy, TLS runtime, or shutdown matrix was run; CI is required.** DONE below means source/fixture review only, never runtime success. No worker remains active.

## P1 requirements

| ID | Status | Source-grounded assessment |
|---|---|---|
| P1-1 Strict request-head scanner and sequential framing gate | DONE (source/fixture review only); runtime NOT RUN | `src/framing/scanner.rs` rejects bare LF/CR, obs-fold, invalid field names, duplicate Content-Length/Transfer-Encoding, CL+TE, non-chunked TE, malformed/overflowing decimal CL; OWS is trimmed. `guarded_io.rs:485-508,432-441,618-637` scans before exposing heads and advances fixed/chunked bodies and trailers. Pipeline tests and loopback transport coverage are present (`src/tests/transport_tests.rs:549`). Hyper remains the semantic parser; sequential requests are serialized, and the next head is withheld until the prior response flushes. |
| P1-2 Hyper head timeout and canned-rejection recovery | DONE (source/fixture review only); runtime NOT RUN | Hyper uses `TokioTimer`/header timeout (`src/server/accept.rs:133-139`); guarded recovery/deadline paths are at `src/framing/guarded_io.rs:105-145,548-569`. Service body deadlines apply to the whole request (`src/server/service.rs:177-214`, `src/server/service.rs:34-35,268`). TCP write-hold coverage is at `src/tests/hold_tests.rs:404`; lifecycle coverage at `src/tests/lifecycle_tests.rs:195,237,287`. Hyper timeout recovery and fixtures reviewed; CI/runtime validation remains required. |
| P1-3 Hold lifetime through response transmission | DONE (source/fixture review only); runtime NOT RUN | Body holds (`src/server/body.rs:6-14`) and guarded-I/O release paths (`guarded_io.rs:574-585,616-637`) retain holds through final write/flush or teardown. Real-TCP tests at `src/tests/hold_tests.rs:355,389,404` exercise blocked writes. Socket flush is not application-level acknowledgment. |
| P1-4 Typed serve failure and TLS setup | DONE (source/fixture review only); runtime NOT RUN | Typed `serve` and TLS setup (`src/server/accept.rs:30-38,55-60`) fail closed on TLS configuration; TLS 1.2 rejection lifecycle coverage at `src/tests/lifecycle_tests.rs:445`, TLS 1.3 tests at `src/tests/tls_tests.rs:175,276`. |

## P2 requirements

| ID | Status | Source-grounded assessment |
|---|---|---|
| P2-1 Body-budget accounting | DONE (source/fixture review only); runtime NOT RUN | Service accounting is at `src/server/service.rs:149-246`; declared-limit matrix at `src/tests/transport_tests.rs:629`, chunked at `src/tests/transport_tests.rs:692`. Global budget 4096 isolates max-body limit 1024; tests cover N/N+1, full payload, budget release, and zero-budget 503 behavior. Reservation starts at `src/tests/budget_tests.rs:37`, growth at `src/tests/budget_tests.rs:67` (concurrency). |
| P2-2 Task and accept lifecycle | DONE (source/fixture review only); runtime NOT RUN | Accept path `src/server/accept.rs:93-187`; retry behavior `src/server/accept.rs:203-229`; tests `src/server/tests/accept_tests.rs:5,26,55`; lifecycle/shutdown coverage at `src/tests/lifecycle_tests.rs:83,381`. Tasks are tracked; transient accept errors continue immediately, while resource/generic errors back off; shutdown cancels backoff and drains/aborts/joins as configured. |
| P2-3 HTTP version and dispatch | DONE (source/fixture review only); runtime NOT RUN | HTTP/1.1 and service routing/auth at `src/server/service.rs:27-33`; scanner rejects non-HTTP/1.1 input (`src/framing/scanner.rs:83-96`; transport test around `src/tests/transport_tests.rs:866`) using read-exact behavior. |
| P2-4 Scanner line-ending and OWS handling | DONE (source/fixture review only); runtime NOT RUN | Bare CR expects BadRequest; trailing OWS is trimmed before chunked parsing. `src/framing/tests/scanner_tests.rs:4,150` covers all split-point verdicts. |
| P2-5 Socket-level integration and backpressure coverage | PARTIAL (source/fixture review only); runtime NOT RUN | Raw-TCP `serve` auth/body/gate/status-routing fixtures and robust read/close are present (`src/tests/transport_tests.rs:191,214`). New TCP hold/write fixtures exercise Hyper plus `GuardedIo` directly, not public `serve` with a fake executor. Full CI behavior is unverified. |
| P2-6 Private test layout and formatting gate | DONE (source/layout/format review only); runtime NOT RUN | `src/lib.rs:13-14` keeps modules private; `src/server/accept.rs:240-242` privately mounts accept tests (parent-module wiring deviation avoids widening visibility). `cargo fmt` and `cargo fmt --check` exited 0; `python -I scripts/ci/check_layout.py` exited 0. Manual counts: `src` 7, `framing` 4, `framing/tests` 5, `server` 6, `server/tests` 2, `src/tests` 6. Largest source files: `transport_tests.rs` 939 lines, `guarded_io.rs` 650; remaining sources under 1000 lines. |

## Architecture and deviations

Intended path: TCP accept → admission → optional TLS → `GuardedIo` scanner/framing → Hyper HTTP/1 → authentication/service → executor. Scanner head metadata is retained until complete validation; body bytes cannot be mistaken for the next request head. Canned malformed-input responses follow completion/flush of any preceding response. The protocol spec's no-pipelining text at line 170 needs an orchestrator amendment to allow sequential requests; no spec edit was made.

The public API is `serve(...) -> Result<(), ServeError>` rather than a bare `Future<Output=()>`; TLS setup is typed and fail-closed, limits include `write_timeout`, and executor uses `impl Future + Send` (RPITIT). Holds reside in guarded I/O rather than a body wrapper. `shamir-transport-tcp` remains direct for transport material/bind policy; `shamir-connect` is transitive. `src/server/response.rs` is the sole wired response implementation; no moves/removals are needed. `Cargo.lock` has the approved `tokio-util` `rt` feature and `futures-util` edge; it was unchanged this session. Pinned versions: hyper 1.9.0, hyper-util 0.1.20, rustls 0.23.46, Tokio 1.51.6. Exact rustls/Tokio sources were absent from local cache; CI must validate APIs.

T6a remains an `Endpoint::Ws` plus `/shamir/v1/ws` authorization/routing seam, not a WebSocket upgrade. No session changes to `Cargo.toml` or root lockfile.

## Progress checkpoints

- P1-1: FINALDONE source except P2-5 PARTIAL; evidence `src/framing/scanner.rs`, `src/framing/guarded_io.rs`, `src/tests/transport_tests.rs:549`; source/fixture review only, runtime not run.
- P1-2: FINALDONE source except P2-5 PARTIAL; evidence `src/server/accept.rs`, `src/server/service.rs`, `src/tests/hold_tests.rs:404`, `src/tests/lifecycle_tests.rs`; source/fixture review only, runtime not run.
- P1-3: FINALDONE source except P2-5 PARTIAL; evidence `src/server/body.rs`, `src/framing/guarded_io.rs`, `src/tests/hold_tests.rs:355,389,404`; source/fixture review only, runtime not run.
- P1-4: FINALDONE source except P2-5 PARTIAL; evidence `src/server/accept.rs`, `src/tests/lifecycle_tests.rs`, `src/tests/tls_tests.rs`; source/fixture review only, runtime not run.
- P2-1: FINALDONE source except P2-5 PARTIAL; evidence `src/tests/transport_tests.rs:629,692`, `src/tests/budget_tests.rs:37,67`; source/fixture review only, runtime not run.
- P2-2: FINALDONE source except P2-5 PARTIAL; evidence `src/server/accept.rs`, `src/server/tests/accept_tests.rs`, `src/tests/lifecycle_tests.rs`; source/fixture review only, runtime not run.
- P2-3: FINALDONE source except P2-5 PARTIAL; evidence `src/server/service.rs`, `src/framing/scanner.rs`, `src/tests/transport_tests.rs:866`; source/fixture review only, runtime not run.
- P2-4: FINALDONE source except P2-5 PARTIAL; evidence `src/framing/tests/scanner_tests.rs`; source/fixture review only, runtime not run.
- P2-5: PARTIAL; evidence `src/tests/transport_tests.rs:191,214` and `src/tests/hold_tests.rs`; direct Hyper/GuardedIo hold tests are not public `serve` with fake executor; runtime not run.
- P2-6: FINALDONE source except P2-5 PARTIAL; evidence `src/lib.rs`, `src/server/accept.rs`, formatting/layout check results recorded above; static review only.

## Final report

All ten per-item dispositions: **P1-1 DONE**, scanner reset/head-before-Hyper guarded I/O and pipeline/loopback fixtures reviewed; **P1-2 DONE**, timeout/recovery source and deadline/lifecycle/TCP-write fixtures reviewed; **P1-3 DONE**, body hold and guarded release source plus real-TCP hold fixtures reviewed; **P1-4 DONE**, typed serve/TLS configuration and TLS 1.2 refusal/TLS 1.3 fixtures reviewed; **P2-1 DONE**, reservation accounting, declared/chunked limit matrices and concurrency/release fixtures reviewed; **P2-2 DONE**, accept/retry/backoff and shutdown lifecycle source/fixtures reviewed; **P2-3 DONE**, HTTP version, service routing and read-exact source/fixtures reviewed; **P2-4 DONE**, bare-CR/OWS and split-verdict fixtures reviewed; **P2-5 PARTIAL**, because new TCP hold/write fixtures use Hyper+GuardedIo directly rather than public `serve` plus fake executor, despite raw-TCP serve auth/body/gate/status-routing fixtures and robust read/close being present; **P2-6 DONE**, private layout and static formatting/layout gates reviewed. Every DONE is source/fixture review only. Runtime/CI for all items is NOT RUN.

### Disputed P2-1 subclaim

“N+1 after declared N” on the wire is inherently an overlong-body oracle problem: fixed framing consumes N bytes, then the next byte belongs to the next request head. Boundary tests therefore configure max limit N versus N+1; they do not claim an invalid-wire oracle for a body declared N but carrying N+1. The chunked boundary uses global budget 4096 to isolate body limit 1024. The no-pipelining spec text at line 170 should be amended by the orchestrator to permit sequential requests; no spec edit was made.

### Added or strengthened tests reviewed (not run)

- `every_invalid_pipelined_head_is_canned_after_first_response`
- `malformed_second_head_waits_for_first_response_flush_and_then_is_canned`
- `fixed_and_chunked_bodies_do_not_scan_payload_as_next_head` (mutable-client compile fix reviewed, not compiler-verified)
- `routing_and_gate_statuses_have_expected_headers_and_bodies` (lost wrapper restored)
- `still_invalid_grant_returns_401_without_executor_and_releases_budget`
- `declared_body_limit_matrix_accepts_exact_and_rejects_one_over_early`
- `chunked_body_limit_matrix_accepts_exact_and_rejects_one_over`
- `concurrent_reservations_never_exceed_cap`
- `concurrent_growth_accounts_against_one_shared_cap_and_releases_on_failure`
- `each_split_point_preserves_scanner_verdict`
- `tcp_hold_remains_until_blocked_response_is_fully_read`
- `tcp_disconnect_releases_hold_after_observed_blocked_write`
- `tcp_write_deadline_releases_hold_when_observed_blocked`
- `allowed_expect_continue_body_stall_times_out_after_interim_response`
- `transient_accept_errors_retry_without_backoff`
- `resource_and_generic_errors_use_increasing_capped_backoff`
- `shutdown_cancels_accept_error_backoff`

All fixtures were read end-to-end; no observed red/green runtime result.

### Session edits and source review scope

Session-edited files: `src/server/accept.rs` (retry extraction/private wiring); `src/server/tests/{mod.rs,accept_tests.rs}` (new); `src/server/mod.rs` (transient-test declaration removed, final state same as initial); `src/framing/tests/{scanner_tests.rs,pipeline_tests.rs}`; `src/tests/{transport_tests.rs,budget_tests.rs,hold_tests.rs,lifecycle_tests.rs}`; and this document. `Cargo.toml` and root `Cargo.lock` were not changed by the session. Formatting may touch formatting within the scoped crate only. No file moves/removals were needed.

All 25 Rust files were read end-to-end, including fixtures:
- `src/{lib,budget,config,gate}.rs`
- `src/framing/{mod,scanner,guarded_io}.rs`
- `src/framing/tests/{mod,scanner_tests,guarded_io_tests,pipeline_tests,deadline_tests}.rs`
- `src/server/{mod,accept,body,response,service}.rs`
- `src/server/tests/{mod,accept_tests}.rs`
- `src/tests/{mod,budget_tests,transport_tests,hold_tests,lifecycle_tests,tls_tests}.rs`

Imports, privacy, module paths, cfgs, trait bounds, Send/static, pin/futures, match exhaustiveness, and signatures were reviewed; this is not compiler proof.

### Public-item inventory and deviations

No session-added public items. Stage public surface inventory: modules `budget`, `config`, `framing`, `gate`, `server` and nested `scanner`, `accept`; `BodyBudget::{new,try_reserve,in_use}`; `BodyReservation::{reserved_bytes,grow}`; `Endpoint::{Rest,Ws}`; `EndpointSet::{rest,ws}`; `HttpLimits` fields `max_head_bytes`, `max_headers`, `max_body_bytes`, `head_timeout`, `body_timeout`, `request_timeout`, `body_budget`, `write_timeout`, and `new`; `HttpServerConfig::{limits,endpoints,tls,hsts,forbidden_body,ping_body}`; `AuthHeader::{Missing,Single,Multiple}`; `PeerInfo::{addr,tls,forwarded_for}`; `GateDecision::{Allow,Unauthorized,Forbidden,RateLimited,Unavailable}`; `TokenGate::{Grant,authenticate,still_valid}`; `ExecOutcome::{status,body,hold}`; `ExecStatus::{Ok,BadRequest,Forbidden,RateLimited,Unavailable}`; `RestExecutor::execute`; `ConnAdmission::admit`; `gate::BoxFuture`; `framing::scanner::Scanner::{new,feed,bytes}`; `Verdict::{NeedMore,Complete,BadRequest,TooLarge,TooManyHeaders}`; `serve -> Result<(),ServeError>`; `ServeError::TlsConfiguration`.

Notable deviations: typed fail-closed serve result; explicit write timeout; Send RPITIT executor future; hold stored in I/O rather than body; sequential pipeline serialization; private child-test mounting rather than widened visibility. No requirements were dropped.

### Verification and handoff

Latest `cargo fmt` and `cargo fmt --check`: exit 0. `python -I scripts/ci/check_layout.py`: exit 0. Manual layout counts and source line counts are above. No compile, tests, clippy, TLS/shutdown runtime matrix, or source-integrity suite ran; CI is required. Do not infer tests passed from static review. No commit or push was made this session.
