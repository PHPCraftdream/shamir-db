<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-transport-ipc — SUMMARY revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The IPC crate is unchanged from its introduction in 74e64493. All 18 numbered entries were revalidated; 3.1 duplicates 1.4, leaving 17 unique items. No source-proven fixes exist. The Windows accept-state defect, Unix cleanup/restart defects, missing endpoint resolution, test gaps, and API/documentation debt remain. Several original reachability and non-finding guarantees require qualification.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 18 | 18 | 0 | 0 | 0 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1-1"></a>

### Claim 1.1 — `accept()` error path leaves `next = None`; next call panics via `expect` and silently kills the accept loop

Status: `confirmed-open`. Current risk: `high`.

take() precedes both fallible operations and restoration. Either error leaves next=None; the server retries, reaches expect, and loses its listener task. JoinHandle errors are ignored at shutdown. Exhaustion-class reachability remains conditional; the claimed deterministic 255-instance cap is false because pinned Tokio defaults to unlimited instances.

Evidence: [crates/shamir-transport-ipc/src/windows.rs:83](../../../../../crates/shamir-transport-ipc/src/windows.rs#L83); [crates/shamir-transport-ipc/src/windows.rs:113](../../../../../crates/shamir-transport-ipc/src/windows.rs#L113); [crates/shamir-server/src/server/server_launcher.rs:671](../../../../../crates/shamir-server/src/server/server_launcher.rs#L671); [crates/shamir-server/src/server/server_launcher.rs:1271](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1271); [crates/shamir-server/src/server/server_handle.rs:114](../../../../../crates/shamir-server/src/server/server_handle.rs#L114); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

<a id="review-1-2"></a>

### Claim 1.2 — Unix `bind()`: `set_permissions` failure leaks the socket file — with default (wide) permissions — and poisons the next start

Status: `confirmed-open`. Current risk: `medium`.

chmod failure returns before Self construction, so the listener fd closes but its pathname is not removed by IpcListener::Drop. Subsequent bind encounters the residue. Its mode depends on umask; universally wide permissions and a live-channel security bypass are not established.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:48](../../../../../crates/shamir-transport-ipc/src/unix.rs#L48); [crates/shamir-transport-ipc/src/unix.rs:67](../../../../../crates/shamir-transport-ipc/src/unix.rs#L67); [crates/shamir-server/src/server/server_launcher.rs:645](../../../../../crates/shamir-server/src/server/server_launcher.rs#L645).

<a id="review-1-3"></a>

### Claim 1.3 — No recovery from a crash-left stale socket path (unclean exit bricks the listener until manual rm)

Status: `confirmed-open`. Current risk: `medium`.

bind directly delegates to Tokio without stale-path reclamation; cleanup exists only in Drop. A process exit that skips destructors can therefore prevent restart. Ordinary unwinding panics do not necessarily leave residue. Probe-then-unlink alone is not race-safe between concurrent reclaimers.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:50](../../../../../crates/shamir-transport-ipc/src/unix.rs#L50); [crates/shamir-transport-ipc/src/unix.rs:68](../../../../../crates/shamir-transport-ipc/src/unix.rs#L68); [crates/shamir-server/src/server/server_launcher.rs:645](../../../../../crates/shamir-server/src/server/server_launcher.rs#L645).

<a id="review-1-4"></a>

### Claim 1.4 — chmod-after-bind race window (documented, spec-prescribed — report as accepted risk with a structural option)

Status: `confirmed-open`. Current risk: `low`.

Socket publication still precedes chmod, as the normative contract prescribes. Cross-user reachability depends on initial mode, directory access, and platform permission enforcement; SCRAM or authenticated-ticket validation still follows. No await does not exclude other runtime threads or OS preemption, and microsecond timing is unmeasured.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:3](../../../../../crates/shamir-transport-ipc/src/unix.rs#L3); [crates/shamir-transport-ipc/src/unix.rs:50](../../../../../crates/shamir-transport-ipc/src/unix.rs#L50); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:49](../../../../../docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md#L49); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:56](../../../../../docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md#L56); [crates/shamir-server/src/connection/handshake.rs:735](../../../../../crates/shamir-server/src/connection/handshake.rs#L735).

<a id="review-1-5"></a>

### Claim 1.5 — Windows test gaps: DACL content and first-instance exclusivity are untested (spec §11 checklist items)

Status: `confirmed-open`. Current risk: `low`.

The registered Windows module contains only byte round-trip and second-client tests. Neither inspects the created DACL nor attempts a competing listener bind. Source construction matches the intended SID-based DACL, but these tests cannot detect its widening or removal of first-instance exclusivity.

Evidence: [crates/shamir-transport-ipc/src/lib.rs:34](../../../../../crates/shamir-transport-ipc/src/lib.rs#L34); [crates/shamir-transport-ipc/src/tests/mod.rs:3](../../../../../crates/shamir-transport-ipc/src/tests/mod.rs#L3); [crates/shamir-transport-ipc/src/tests/windows_tests.rs:16](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L16); [crates/shamir-transport-ipc/src/tests/windows_tests.rs:41](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L41); [crates/shamir-transport-ipc/src/windows.rs:148](../../../../../crates/shamir-transport-ipc/src/windows.rs#L148); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:157](../../../../../docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md#L157).

<a id="review-1-6"></a>

### Claim 1.6 — No test drives `accept()` through an error, and no Unix test pins bind-on-existing-path semantics

Status: `confirmed-open`. Current risk: `low`.

Reachable unit tests exercise successful accept, permissions, and clean Unix Drop only. Neighbor IPC integration tests exercise successful authentication/resumption, not injected accept errors, stale/live bind collisions, or nonexistent-endpoint errors. Unit clients also bypass the crate's public connect wrapper.

Evidence: [crates/shamir-transport-ipc/src/tests/mod.rs:1](../../../../../crates/shamir-transport-ipc/src/tests/mod.rs#L1); [crates/shamir-transport-ipc/src/tests/unix_tests.rs:8](../../../../../crates/shamir-transport-ipc/src/tests/unix_tests.rs#L8); [crates/shamir-transport-ipc/src/tests/windows_tests.rs:16](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L16); [crates/shamir-server/tests/ipc_e2e.rs:125](../../../../../crates/shamir-server/tests/ipc_e2e.rs#L125); [crates/shamir-server/tests/ipc_resume_e2e.rs:218](../../../../../crates/shamir-server/tests/ipc_resume_e2e.rs#L218); [crates/shamir-client/tests/smoke_local.rs:98](../../../../../crates/shamir-client/tests/smoke_local.rs#L98).

<a id="review-2-1"></a>

### Claim 2.1 — Rotation leaves a no-pending-instance window → concurrent second client gets `ERROR_PIPE_BUSY`; the client does no BUSY retry

Status: `confirmed-open`. Current risk: `low`.

The replacement instance is created only after connect completes; public connect performs one open and the SDK only adds an optional timeout. Pinned Tokio documents BUSY retry as necessary. Reordering reduces the rotation gap but does not eliminate busy states when clients consume pending instances faster than the server replenishes them.

Evidence: [crates/shamir-transport-ipc/src/windows.rs:51](../../../../../crates/shamir-transport-ipc/src/windows.rs#L51); [crates/shamir-transport-ipc/src/windows.rs:88](../../../../../crates/shamir-transport-ipc/src/windows.rs#L88); [crates/shamir-client/src/client.rs:234](../../../../../crates/shamir-client/src/client.rs#L234); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

<a id="review-3-1"></a>

### Claim 3.1 — Unix bind→chmod window is the only transport-reachable security gap; consider the atomic-rename hardening

Status: `confirmed-open`. Current risk: `low`.

Same unchanged publication-before-chmod mechanism as 1.4. The assertion that this is the only security gap is unverified, not a source-proven exhaustive guarantee. Dependency reuse is verified: IPC and socket2 both use windows-sys 0.60.2, which existed before IPC introduction.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:50](../../../../../crates/shamir-transport-ipc/src/unix.rs#L50); [crates/shamir-transport-ipc/Cargo.toml:25](../../../../../crates/shamir-transport-ipc/Cargo.toml#L25); [Cargo.lock:3735](../../../../../Cargo.lock#L3735); [Cargo.lock:3953](../../../../../Cargo.lock#L3953); [Cargo.lock:5182](../../../../../Cargo.lock#L5182).

Grouping/duplicate: `SUMMARY.md#1.4`. This row is not another independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Stale doc: "reject_remote_clients … is left at its default" while the code explicitly pins it

Status: `confirmed-open`. Current risk: `nit`.

The module documentation still describes reliance on a default, while create_instance explicitly sets reject_remote_clients(true). This is documentation drift, not a missing remote-client rejection setting.

Evidence: [crates/shamir-transport-ipc/src/windows.rs:11](../../../../../crates/shamir-transport-ipc/src/windows.rs#L11); [crates/shamir-transport-ipc/src/windows.rs:117](../../../../../crates/shamir-transport-ipc/src/windows.rs#L117).

<a id="review-4-1"></a>

### Claim 4.1 — `tokio features = ["full"]` pulls subsystems this crate never uses

Status: `confirmed-open`. Current risk: `nit`.

The manifest still enables full, including process/signal/fs facilities unused by the shim. Runtime/macros/io-util are also used by tests. Feature breadth is source-proven; additional build cost is unmeasured and may already be unified through consumers. No runtime-performance severity is established.

Evidence: [crates/shamir-transport-ipc/Cargo.toml:14](../../../../../crates/shamir-transport-ipc/Cargo.toml#L14); [crates/shamir-transport-ipc/src/tests/windows_tests.rs:1](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L1); [crates/shamir-client/Cargo.toml:19](../../../../../crates/shamir-client/Cargo.toml#L19); [crates/shamir-server/Cargo.toml:56](../../../../../crates/shamir-server/Cargo.toml#L56); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

<a id="review-5-1"></a>

### Claim 5.1 — Spec §9 logical endpoint names are unimplemented — and fail *differently* per OS

Status: `confirmed-open`. Current risk: `medium`.

Both transport implementations pass addresses verbatim; server validation accepts any nonempty IPC address and the SDK performs no resolution. Thus the specified Windows logical-name mapping and client documentation promise remain unmet. Unix relative paths are accepted rather than resolved to a defined logical namespace. An existing stale/live pathname prevents server bind; the original successful-bind-to-existing-socket scenario is incorrect.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:39](../../../../../crates/shamir-transport-ipc/src/unix.rs#L39); [crates/shamir-transport-ipc/src/windows.rs:51](../../../../../crates/shamir-transport-ipc/src/windows.rs#L51); [crates/shamir-server/src/config.rs:772](../../../../../crates/shamir-server/src/config.rs#L772); [crates/shamir-client/src/client.rs:119](../../../../../crates/shamir-client/src/client.rs#L119); [crates/shamir-client/src/client.rs:680](../../../../../crates/shamir-client/src/client.rs#L680); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:131](../../../../../docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md#L131); [crates/shamir-client/tests/smoke_local.rs:45](../../../../../crates/shamir-client/tests/smoke_local.rs#L45).

<a id="review-5-2"></a>

### Claim 5.2 — Platform-divergent public signatures: `connect`/`bind`/`path()` differ across `cfg`

Status: `confirmed-open`. Current risk: `low`.

Unix still accepts AsRef<Path> and returns &Path; Windows connect accepts &str, bind accepts Into<String>, and path returns &str. Current string-based callers fit both, but Path-based generic callers are not portable. This is API portability debt, not a demonstrated failure of current consumers.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:39](../../../../../crates/shamir-transport-ipc/src/unix.rs#L39); [crates/shamir-transport-ipc/src/unix.rs:48](../../../../../crates/shamir-transport-ipc/src/unix.rs#L48); [crates/shamir-transport-ipc/src/unix.rs:62](../../../../../crates/shamir-transport-ipc/src/unix.rs#L62); [crates/shamir-transport-ipc/src/windows.rs:51](../../../../../crates/shamir-transport-ipc/src/windows.rs#L51); [crates/shamir-transport-ipc/src/windows.rs:68](../../../../../crates/shamir-transport-ipc/src/windows.rs#L68); [crates/shamir-transport-ipc/src/windows.rs:98](../../../../../crates/shamir-transport-ipc/src/windows.rs#L98).

<a id="review-5-3"></a>

### Claim 5.3 — `IpcStream`/`IpcClientStream` are transparent aliases — OS-specific inherent methods leak through the "unified" surface

Status: `confirmed-open`. Current risk: `low`.

The public streams remain transparent aliases to different Tokio types. Shared callers currently use generic I/O, but platform-specific inherent methods remain available, and lib.rs does not document an AsyncRead/AsyncWrite-only portability rule. Newtypes are an optional enforcement choice rather than proof of a present runtime defect.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:30](../../../../../crates/shamir-transport-ipc/src/unix.rs#L30); [crates/shamir-transport-ipc/src/windows.rs:39](../../../../../crates/shamir-transport-ipc/src/windows.rs#L39); [crates/shamir-transport-ipc/src/lib.rs:3](../../../../../crates/shamir-transport-ipc/src/lib.rs#L3); [crates/shamir-client/src/client.rs:150](../../../../../crates/shamir-client/src/client.rs#L150); [crates/shamir-server/src/framer.rs:275](../../../../../crates/shamir-server/src/framer.rs#L275).

<a id="review-5-4"></a>

### Claim 5.4 — Bind-collision error text is OS-asymmetric and unmapped

Status: `confirmed-open`. Current risk: `nit`.

Unix delegates bind errors; Windows uses first_pipe_instance(true) and propagates raw creation errors. Pinned Tokio documents competing first-instance creation as PermissionDenied. No mapping or explanatory collision note exists. AccessDenied can also represent genuine permission failures, so blanket remapping would be misleading.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:50](../../../../../crates/shamir-transport-ipc/src/unix.rs#L50); [crates/shamir-transport-ipc/src/windows.rs:71](../../../../../crates/shamir-transport-ipc/src/windows.rs#L71); [crates/shamir-transport-ipc/src/windows.rs:116](../../../../../crates/shamir-transport-ipc/src/windows.rs#L116); [Cargo.lock:4195](../../../../../Cargo.lock#L4195).

<a id="review-6-1"></a>

### Claim 6.1 — Declared-but-unused `thiserror` dependency — the typed error this crate needs was never written

Status: `confirmed-open`. Current risk: `low`.

thiserror remains declared but unused; the crate exposes io::Result and no IpcError. The unused dependency is confirmed, but the assertion that an enum would force correct lifecycle handling is unsupported. Repairing listener invariants does not inherently require replacing io::Error; removing the unused dependency is a valid alternative.

Evidence: [crates/shamir-transport-ipc/Cargo.toml:17](../../../../../crates/shamir-transport-ipc/Cargo.toml#L17); [crates/shamir-transport-ipc/src/windows.rs:83](../../../../../crates/shamir-transport-ipc/src/windows.rs#L83); [crates/shamir-transport-ipc/src/unix.rs:48](../../../../../crates/shamir-transport-ipc/src/unix.rs#L48); [Cargo.lock:3739](../../../../../Cargo.lock#L3739).

<a id="review-6-2"></a>

### Claim 6.2 — No lifecycle tests for the Windows listener; Unix-only `Drop` coverage

Status: `confirmed-open`. Current risk: `low`.

No registered Windows test explicitly binds, drops the final instance, and rebinds the same name. Existing tests cannot pin this lifecycle property. The intended test must have no surviving accepted server streams: dropping the listener alone need not release a name still retained by another pipe instance.

Evidence: [crates/shamir-transport-ipc/src/tests/windows_tests.rs:16](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L16); [crates/shamir-transport-ipc/src/tests/windows_tests.rs:41](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L41); [crates/shamir-transport-ipc/src/tests/unix_tests.rs:47](../../../../../crates/shamir-transport-ipc/src/tests/unix_tests.rs#L47); [crates/shamir-transport-ipc/src/windows.rs:57](../../../../../crates/shamir-transport-ipc/src/windows.rs#L57).

<a id="review-7-1"></a>

### Claim 7.1 — Crate-level `src/tests/` instead of per-module `tests/` directories

Status: `confirmed-open`. Current risk: `nit`.

Tests remain grouped beneath the crate root rather than beside each implementation module. The root manifest and platform registration are correct; this is only the original literal-layout policy concern. The review's no-action recommendation remains reasonable for this small public API test group.

Evidence: [CLAUDE.md:575](../../../../../CLAUDE.md#L575); [crates/shamir-transport-ipc/src/lib.rs:34](../../../../../crates/shamir-transport-ipc/src/lib.rs#L34); [crates/shamir-transport-ipc/src/tests/mod.rs:1](../../../../../crates/shamir-transport-ipc/src/tests/mod.rs#L1).

<a id="review-7-2"></a>

### Claim 7.2 — CLAUDE.md's workspace roster predates this crate (repo-level drift)

Status: `confirmed-open`. Current risk: `nit`.

CLAUDE.md still lists 23 crates without IPC, while the workspace glob includes IPC as the 24th default Rust member. AGENTS.md has the same omission. This is repository-context drift only.

Evidence: [CLAUDE.md:30](../../../../../CLAUDE.md#L30); [CLAUDE.md:39](../../../../../CLAUDE.md#L39); [AGENTS.md:34](../../../../../AGENTS.md#L34); [Cargo.toml:2](../../../../../Cargo.toml#L2); [Cargo.toml:14](../../../../../Cargo.toml#L14); [crates/shamir-transport-ipc/Cargo.toml:2](../../../../../crates/shamir-transport-ipc/Cargo.toml#L2).

## Current fix-plan state

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 11 | 11 | 0 | 0 | 0 | 0 | 0 |

A source-fixed item closes only its stated mechanism. Partial items retain the obligations named below; proposed fixes must obey the corrections and current contracts, not merely copy the historical recipe.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`. Current risk: —.

No invariant repair or error redesign exists. Preserve listener ownership across every failure before replacing the instance; simple reordering after take can still poison state. Early replenishment alone does not fully close BUSY failures under bursts.

Evidence: [crates/shamir-transport-ipc/src/windows.rs:83](../../../../../crates/shamir-transport-ipc/src/windows.rs#L83); [crates/shamir-transport-ipc/src/windows.rs:89](../../../../../crates/shamir-transport-ipc/src/windows.rs#L89); [crates/shamir-transport-ipc/src/windows.rs:51](../../../../../crates/shamir-transport-ipc/src/windows.rs#L51).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`. Current risk: —.

Neither accept-error recovery nor same-name bind/drop/rebind tests are registered. Cover each fallible branch deterministically; a client closing immediately is not verified to induce an accept error. Add the Unix collision/nonexistent-endpoint cases too before claiming all of 1.6 closed.

Evidence: [crates/shamir-transport-ipc/src/lib.rs:34](../../../../../crates/shamir-transport-ipc/src/lib.rs#L34); [crates/shamir-transport-ipc/src/tests/mod.rs:1](../../../../../crates/shamir-transport-ipc/src/tests/mod.rs#L1); [crates/shamir-transport-ipc/src/tests/windows_tests.rs:16](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L16); [crates/shamir-transport-ipc/src/tests/unix_tests.rs:8](../../../../../crates/shamir-transport-ipc/src/tests/unix_tests.rs#L8).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`. Current risk: —.

chmod failure still returns directly without pathname cleanup. An unlink attempt must be added and its failure semantics considered; silently ignoring a failed unlink cannot guarantee absence of residue.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:51](../../../../../crates/shamir-transport-ipc/src/unix.rs#L51); [crates/shamir-transport-ipc/src/unix.rs:74](../../../../../crates/shamir-transport-ipc/src/unix.rs#L74).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`. Current risk: —.

No stale-path recovery exists. The proposed connect-probe/unlink sequence additionally needs ownership and concurrent-reclaimer safety; a prior refusal does not authorize deleting a pathname subsequently rebound by another process.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:48](../../../../../crates/shamir-transport-ipc/src/unix.rs#L48); [crates/shamir-transport-ipc/src/unix.rs:68](../../../../../crates/shamir-transport-ipc/src/unix.rs#L68); [crates/shamir-server/src/server/server_launcher.rs:645](../../../../../crates/shamir-server/src/server/server_launcher.rs#L645).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`. Current risk: —.

There is neither shared endpoint resolution nor validation forbidding logical names; the client promise is unchanged. Specify Unix logical-name semantics and distinguish URI parsing from raw endpoint resolution before implementing; alternatively consistently require explicit platform endpoints.

Evidence: [docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:131](../../../../../docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md#L131); [crates/shamir-server/src/config.rs:772](../../../../../crates/shamir-server/src/config.rs#L772); [crates/shamir-client/src/client.rs:119](../../../../../crates/shamir-client/src/client.rs#L119); [crates/shamir-transport-ipc/src/windows.rs:51](../../../../../crates/shamir-transport-ipc/src/windows.rs#L51).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`. Current risk: —.

DACL inspection and competing-bind tests remain absent. Assertions should check effective ACE membership/protection and exclusion of unintended principals, not merely accept an SDDL prefix that permits extra ACEs.

Evidence: [crates/shamir-transport-ipc/src/tests/windows_tests.rs:16](../../../../../crates/shamir-transport-ipc/src/tests/windows_tests.rs#L16); [crates/shamir-transport-ipc/src/windows.rs:148](../../../../../crates/shamir-transport-ipc/src/windows.rs#L148); [crates/shamir-transport-ipc/src/windows.rs:116](../../../../../crates/shamir-transport-ipc/src/windows.rs#L116).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`. Current risk: —.

No typed IPC error enum was introduced and the unused dependency remains. The enum is an optional API design, not a required lifecycle mechanism; retaining io::Result with restored state and removing thiserror also addresses the actionable dependency claim.

Evidence: [crates/shamir-transport-ipc/Cargo.toml:17](../../../../../crates/shamir-transport-ipc/Cargo.toml#L17); [crates/shamir-transport-ipc/src/windows.rs:83](../../../../../crates/shamir-transport-ipc/src/windows.rs#L83); [crates/shamir-transport-ipc/src/unix.rs:48](../../../../../crates/shamir-transport-ipc/src/unix.rs#L48).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`. Current risk: —.

connect, bind, and path signatures are still different. A common endpoint contract remains necessary if signature parity is desired; existing string consumers are not proof of generic portability.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:39](../../../../../crates/shamir-transport-ipc/src/unix.rs#L39); [crates/shamir-transport-ipc/src/unix.rs:62](../../../../../crates/shamir-transport-ipc/src/unix.rs#L62); [crates/shamir-transport-ipc/src/windows.rs:51](../../../../../crates/shamir-transport-ipc/src/windows.rs#L51); [crates/shamir-transport-ipc/src/windows.rs:98](../../../../../crates/shamir-transport-ipc/src/windows.rs#L98).

<a id="plan-p2-9"></a>

### Plan P2.9 — P2.9

Status: `confirmed-open`. Current risk: —.

Optional pre-publication permission hardening is absent. Temporary bind/chmod/rename requires a protected directory and collision-safe publication; ordinary replacing rename can overwrite an existing endpoint and is not an unconditional security fix.

Evidence: [crates/shamir-transport-ipc/src/unix.rs:50](../../../../../crates/shamir-transport-ipc/src/unix.rs#L50); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:56](../../../../../docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md#L56).

<a id="plan-p2-10"></a>

### Plan P2.10 — P2.10

Status: `confirmed-open`. Current risk: —.

Neither newtypes nor the proposed portability-only usage documentation exist. Current consumers use generic I/O, so this remains optional abstraction hardening.

Evidence: [crates/shamir-transport-ipc/src/lib.rs:3](../../../../../crates/shamir-transport-ipc/src/lib.rs#L3); [crates/shamir-transport-ipc/src/unix.rs:30](../../../../../crates/shamir-transport-ipc/src/unix.rs#L30); [crates/shamir-transport-ipc/src/windows.rs:39](../../../../../crates/shamir-transport-ipc/src/windows.rs#L39).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `confirmed-open`. Current risk: —.

All three requested documentation corrections remain outstanding: explicit remote rejection, collision diagnostics, and the workspace roster. Item 7.1 was explicitly assigned no action; item 4.1 likewise remains optional feature trimming.

Evidence: [crates/shamir-transport-ipc/src/windows.rs:11](../../../../../crates/shamir-transport-ipc/src/windows.rs#L11); [crates/shamir-transport-ipc/src/windows.rs:66](../../../../../crates/shamir-transport-ipc/src/windows.rs#L66); [crates/shamir-transport-ipc/src/unix.rs:44](../../../../../crates/shamir-transport-ipc/src/unix.rs#L44); [CLAUDE.md:30](../../../../../CLAUDE.md#L30).

## Corrections and qualified non-findings

- Counts: the report contains 18 numbered entries, not 17; deduplicating 3.1 against 1.4 yields 17 unique items: 1 high, 3 medium, 8 low, and 5 nits. The original table omitted 4.1.
- Executive summary and 1.1: retain the error-to-poison-to-panic mechanism, but remove the deterministic accept-255 failure scenario. Pinned Tokio defaults to PIPE_UNLIMITED_INSTANCES, not a finite 255-instance quota. IPC also shares a loopback per-IP limiter defaulting to 100, so max_active_connections alone never establishes the claimed concurrency scenario. Evidence: crates/shamir-transport-ipc/src/windows.rs:113; crates/shamir-server/src/server/server_launcher.rs:1246; crates/shamir-server/src/server/server_launcher.rs:1291; crates/shamir-server/src/config.rs:480; Cargo.lock:4195.
- 1.1: the immediate-client-death absorption claim is unverified against pinned mio 1.1.1, whose source was unavailable. Do not substitute cached neighboring versions. The absent error-regression test is 1.6, not 1.3.
- 1.2–1.4 and concurrency overview: pre-chmod permissions are umask-dependent; 0755 does not universally imply cross-user connect permission. No await only excludes cooperative suspension of this task, not concurrent runtime workers or OS scheduling delays. Neither microsecond duration nor exploit success was measured. Evidence: crates/shamir-transport-ipc/src/unix.rs:3; crates/shamir-transport-ipc/src/unix.rs:50.
- Concurrency non-finding: the shim itself contains no Mutex/RwLock/parking_lot and accept remains single-owner through &mut self. This does not prove the dependency path is lock-free. Pinned Tokio registration takes an internal lock. Evidence: crates/shamir-transport-ipc/src/windows.rs:83; crates/shamir-transport-ipc/src/unix.rs:56; Cargo.lock:4195.
- Security non-findings: the protected single-user SID SDDL, synchronous security-attribute borrow, explicit remote rejection, non-inheritable handle flag, and normal Result-path LocalFree/CloseHandle ownership remain source-supported. Account parity is not logon-session parity and does not exclude privileged OS principals. Blanket unsafe soundness remains unverified because Vec<u8> does not itself establish TOKEN_USER alignment. Evidence: crates/shamir-transport-ipc/src/windows.rs:108; crates/shamir-transport-ipc/src/windows.rs:117; crates/shamir-transport-ipc/src/windows.rs:148; crates/shamir-transport-ipc/src/windows.rs:172; crates/shamir-transport-ipc/src/windows.rs:226; crates/shamir-transport-ipc/src/windows.rs:246; crates/shamir-transport-ipc/src/windows.rs:269; crates/shamir-transport-ipc/src/windows.rs:277.
- Performance non-finding: descriptor/SDDL construction is still once per Windows bind and the shim has no connection-count-dependent loop. However, the absolute zero-allocation/no-lock claim for Unix accept/connect is refuted by inspected pinned Tokio registration, which allocates Arc<ScheduledIo> and locks registration state. Windows instance creation also includes runtime registration/name conversion, not only one OS call. No latency or allocation-size measurements were performed. Evidence: crates/shamir-transport-ipc/src/windows.rs:70; crates/shamir-transport-ipc/src/windows.rs:103; crates/shamir-transport-ipc/src/unix.rs:40; crates/shamir-transport-ipc/src/unix.rs:57; Cargo.lock:4195.
- API/wire non-finding: compile-time public-name dispatch and generic framing remain valid for current callers, not identical signatures or platform-independent addressing. Both SDK and server use the common transport I/O traits without OS branches. Evidence: crates/shamir-transport-ipc/src/lib.rs:24; crates/shamir-client/src/client.rs:150; crates/shamir-server/src/framer.rs:275; crates/shamir-server/src/server/server_launcher.rs:1305.
- 5.1: an existing stale or live Unix socket makes server bind fail; it does not let that bind succeed against the existing inode. A client-only relative-path connection to an unintended live endpoint remains a valid possibility. Spec §9 does not define a Unix logical-name mapping sufficiently to infer one automatically. Evidence: crates/shamir-transport-ipc/src/unix.rs:50; docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:131.
- Lifecycle/style non-findings: public errors still use io::Result with clean consumer conversion; descriptors and owned Tokio handles have normal Drop ownership. Windows name release requires the last relevant pipe instance to close, not necessarily listener Drop while streams survive. Test manifests remain registered, with top-level imports and no inline test modules. These guarantees do not close any missing behavioral test. Evidence: crates/shamir-client/src/error.rs:7; crates/shamir-server/src/server/boot_error.rs:25; crates/shamir-transport-ipc/src/windows.rs:182; crates/shamir-transport-ipc/src/tests/mod.rs:1; crates/shamir-transport-ipc/src/lib.rs:34.
- History establishes absence of an IPC fix, not behavioral correctness: git diff from 74e64493 to the requested HEAD is empty for the crate, and SUMMARY.md was added in 6df2afa9 without changing IPC implementation. No completed-task or commit message was treated as fix proof.

## Current follow-up order

1. P0: restore Windows listener state on every accept error and add deterministic, registered error-recovery/lifecycle regressions; address BUSY retry separately from replenishment ordering.
2. P0: clean up Unix pathname residue after chmod failure, with explicit cleanup-failure handling.
3. P1: choose race-safe stale-path recovery semantics and a consistent logical-name versus explicit-endpoint contract.
4. P1: add effective Windows DACL/exclusivity tests; resolve signature portability and remove the unused dependency or deliberately define an error API.
5. P2: correct review counts, reachability claims, allocation/lock guarantees, and outstanding documentation; retain layout/feature/newtype/atomic-publication changes as scoped optional work.

## Coverage and limitations

- Only SUMMARY.md exists in the assigned directory; there are no separate lens reports or TASK_GROUPS.md.
- Read-only source/history validation only; no compilation, tests, benchmarks, reproductions, or modifications.
- Pinned Tokio 1.49.0 sources were available and inspected; pinned mio 1.1.1 sources were unavailable, so the original ERROR_NO_DATA/ERROR_PIPE_CONNECTED absorption assertion remains unverified.
- Actual error frequencies, scheduling-window durations, OS access denials, and latency were not experimentally verified.
- The review's blanket Windows unsafe-soundness guarantee is not established: the TOKEN_USER reference is formed from a Vec<u8> without an explicit typed-alignment guarantee.

## Reviewed document inventory

- [SUMMARY.md](./SUMMARY.md) — 18 claim decisions; 11 explicit plan items.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-ipc — Synthesized 7-lens review (lean single-file follow-up to the 2026-08-14 cross-crate review)

Crate reviewed: `crates/shamir-transport-ipc/` (new since the 2026-08-14 sweep; commit `74e64493`).
Normative refs: repo `CLAUDE.md`, `docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md`.
Cross-checked against consumers: `crates/shamir-server/src/server/server_launcher.rs` (`accept_loop_ipc`),
`crates/shamir-client/src/client.rs` (`connect_local` / `connect_ipc` / `WriteSink`), `tests/smoke_local.rs`.
Read-only review — no build/test/lint commands were run; no source file was modified.

## Executive summary

The crate is a thin, well-documented OS-transport shim whose `unsafe` Windows block is genuinely
careful — SDDL matches spec §5.2 exactly, the DACL anchors the real user SID (no `OW`/`BA` shorthand),
`PIPE_REJECT_REMOTE_CLIENTS` is explicitly pinned, and every `LocalAlloc`/`LocalFree`/`CloseHandle`
pair is balanced on all exit paths; I found no security bypass. The one serious defect is structural,
not cryptographic: `IpcListener::accept()` (windows.rs:83-95) can leave `next = None` on *any* error,
and the very next `accept()` panics via `expect(...)` — killing the accept-loop task **silently**
(`tokio::spawn`'s JoinHandle is stored at server_launcher.rs:972 and only awaited at shutdown), taking
the whole `unix` transport down until process restart. Fix that first (P0), then the two Unix
lifecycle leaks (chmod-failure leaves a wide-permission stale socket file; no recovery from a
crash-left stale path), then the spec §9 logical endpoint naming that is currently unimplemented.

---

## 1. correctness-tdd

### 1.1 [HIGH] `accept()` error path leaves `next = None`; next call panics via `expect` and silently kills the accept loop
- **File:line:** `crates/shamir-transport-ipc/src/windows.rs:83-95` (esp. `:87` `expect`, `:88` `?`, `:89-93` `?`); interaction: `crates/shamir-server/src/server/server_launcher.rs:1271-1278` (`Err => warn; sleep; continue`), `:671`/`:972` (unsupervised spawned task).
- **Issue:** `accept()` takes the pending instance with `self.next.take()`, then has two fallible steps before restoring `next`: `pending.connect().await?` and `create_instance(...)?`. If **either** fails, the method returns `Err` with `self.next == None`. The next `accept()` call hits `.expect("IpcListener::accept: no pending instance — invariant violated")` → panic. The failure is *guaranteed structural*: the server's accept loop treats any `accept()` error as transient and `continue`s (`server_launcher.rs:1274-1278`), so the panic fires on the iteration immediately after the first error; the spawned loop task dies, nobody monitors its JoinHandle, and the IPC transport is permanently unavailable while the rest of the server keeps running. Additionally, on the `create_instance` failure path the **already-connected** `pending` stream is dropped un-return — the client that just connected sees an abrupt broken pipe.
- **Verified reachability (tokio 1.49 / mio 1.1.1, per vendored sources):** the *client-race* trigger I first suspected — client opens the pipe and dies immediately — is actually absorbed: mio's `connect_overlapped` maps `ERROR_PIPE_CONNECTED` **and** `ERROR_NO_DATA` to `Ok(true)` (`mio-1.1.1/src/sys/windows/named_pipe.rs:159-160`), so `connect()` returns `Ok` there and the handler just reads EOF off a dead stream. That leaves the trigger set as: (a) `create_instance` failure — `CreateNamedPipe` errors such as instance exhaustion (tokio default `max_instances = PIPE_UNLIMITED_INSTANCES`, i.e. 255 live instances, so only reachable when `max_active_connections` config permits ~254 concurrent IPC connections), system handle/desktop-heap exhaustion under memory pressure; (b) the rarer non-mapped `ConnectNamedPipe` errors from `pending.connect()`. Rare — but the consequence (silent permanent transport loss + killed live connection) and the one-line-grade fix make this the crate's top finding. Severity **high**, not critical, precisely because the realistic triggers are exhaustion-class.
- **Failure scenario:** server runs with `max_active_connections` ≥ 255; 254 IPC clients are connected and one more connects; accept #255 succeeds but the rotation's `create_instance` hits the 255-instance cap → `Err` → `continue` → next `accept()` → `expect` panic → accept-loop task aborts → every subsequent `shamir+unix://` client gets connection-refused with zero log output beyond one `warn` and the task's panic message buried in task output.
- **Suggested fix:** never return with `next == None`: create the replacement instance **before** awaiting `pending.connect()` (this also closes finding 2.1's busy-window), and on any residual error attempt to recreate `self.next` before propagating (or surface a distinct poisoned-listener error so the caller can decide to bail instead of `continue`). Replace the `expect` with typed-error propagation as defense-in-depth. Add the missing error-path test (finding 1.3).

### 1.2 [MEDIUM] Unix `bind()`: `set_permissions` failure leaks the socket file — with default (wide) permissions — and poisons the next start
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:48-53` (no unlink on the `?` at `:51`); contrast the clean-shutdown `Drop` at `:67-76`; consumer mapping: `server_launcher.rs:645-647` (`BootError::Bind` → boot aborts).
- **Issue:** `bind()` creates the socket file (tokio `bind`), then `set_permissions(...) ?`. If the chmod fails, the function returns `Err` **without unlinking the file**. The listener local is dropped (fd closed), so the leftover is a *dead* socket inode — no live-socket hijack — but it is (a) still carrying the pre-chmod default mode (0755-class) and (b) enough to make every subsequent `bind` to that path fail with EADDRINUSE until an operator manually removes it. The `Drop` impl only runs when `Self` is constructed, which never happens on this path.
- **Failure scenario:** server starts with `kind: unix` listener on a filesystem/LSL context where `chmod` on the socket is denied (hardened LSM policy, exotic FUSE mount) → boot fails with `BootError::Bind("unix <path>: Operation not permitted")`; operator fixes permissions context and restarts → boot fails again with `Address already in use` (inherited from the leaked file, now also `0755`); requires manual `rm <path>` to recover.
- **Suggested fix:** on the chmod-failure path, `let _ = std::fs::remove_file(&path);` before returning the error (mirror `Drop`'s cleanup), so the failed start leaves no residue.

### 1.3 [MEDIUM] No recovery from a crash-left stale socket path (unclean exit bricks the listener until manual rm)
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:48-53` (bind fails on existing path) + `:67-76` (Drop's doc names exactly this failure mode, but Drop only helps on clean shutdown).
- **Issue:** after SIGKILL/OOM/panic of the server process, the socket file survives (the kernel unlinks nothing while the path exists). Restart fails with EADDRINUSE forever. The `Drop` doc comment documents the problem and the code does not solve it. The standard safe pattern — on bind's EADDRINUSE, attempt a client `connect()`; if it fails with ECONNREFUSED the path is a dead socket → unlink and rebind; if it connects, another live server owns it → fail with "already running" — is absent.
- **Failure scenario:** the DB is OOM-killed overnight; systemd/supervisor restarts it; every restart fails `Address already in use` for `shamir+unix://` until a human notices and deletes the stale `.sock`. Other listeners (TCP/WS) recover on their own, making the IPC outage the odd one out.
- **Suggested fix:** implement the connect-probe-then-unlink-and-rebind pattern inside `bind()` ( Unix-only), or expose an explicit `bind_or_reclaim_stale` and have the server call it. Do **not** blind-unlink — that would race a concurrently-running second instance.

### 1.4 [LOW] chmod-after-bind race window (documented, spec-prescribed — report as accepted risk with a structural option)
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:3-14` (module doc's own analysis), `:50-51`.
- **Issue:** between `bind` (file created at default mode, connectable by any local user) and `set_permissions(0600)` there is a real window. The module doc's mitigation argument is correct as far as it goes — no `.await` sits between the two calls, so no in-process task interleaves, and the wall-clock window is microseconds — but it is a probabilistic argument: a local process holding an inotify watch on the directory can race it. Spec §5.1 *prescribes exactly this approach* ("СРАЗУ после bind, до принятия первого соединения"), so this is compliance, not a bug; and a second-layer exploit needs directory write access to even attempt a path-swap TOCTOU (chmod would then land on the attacker's file, leaving the socket wide).
- **Failure scenario:** a same-host low-priv process inotify-watches `/run/shamir`, sees `db.sock` appear, and connects within the microseconds window — reaching `auth_init` (it still faces full SCRAM; the OS boundary is defense-in-depth per spec §5.3, so impact is exposure of the pre-auth surface, not DB access).
- **Suggested fix (optional hardening):** bind at a temporary path inside the same directory, `chmod 0600`, then `rename()` to the final path — rename is atomic, so a connector sees either nothing or the already-narrowed socket. Otherwise, leave as-is with the doc as the record of the accepted risk (current state).

### 1.5 [LOW] Windows test gaps: DACL content and first-instance exclusivity are untested (spec §11 checklist items)
- **File:line:** `crates/shamir-transport-ipc/src/tests/windows_tests.rs:1-65` (only round-trip + sequential second client).
- **Issue:** spec §11 requires "DACL ограничен SID текущего пользователя" and the rotation check; the rotation is tested (`listener_serves_a_second_client_after_the_first_disconnects`), but nothing asserts the **content** of the created pipe's security descriptor (e.g. `GetSecurityInfo` on a bound listener's handle → convert DACL to SDDL → assert it is `D:P(A;;GA;;;S-1-<current-user>)`), and nothing asserts that a **second `IpcListener::bind` on the same name fails** (`first_pipe_instance(true)` exclusivity — the EADDRINUSE analogue; a regression flipping that flag would silently allow a same-user impostor to shadow the pipe name and would still be green). `PIPE_REJECT_REMOTE_CLIENTS` is likewise unobservable from tests without SMB — acceptable, but the other two are testable.
- **Failure scenario:** a future refactor constructs the SDDL with `OW` (OWNER RIGHTS) or drops the `P` modifier; every test stays green while the access boundary silently widens (owner-relative or inheritable ACEs).
- **Suggested fix:** add `dacl_restricted_to_current_user_sid` (query + SDDL-prefix assertion) and `bind_second_listener_on_same_name_fails` to `windows_tests.rs`.

### 1.6 [LOW] No test drives `accept()` through an error, and no Unix test pins bind-on-existing-path semantics
- **File:line:** `crates/shamir-transport-ipc/src/tests/unix_tests.rs:1-54` (happy paths + permission + drop only); no test corresponds to `windows.rs:83-95`'s failure branches.
- **Issue:** the single highest-consequence code path in the crate (finding 1.1) has zero test coverage — any test that makes `create_instance` fail (e.g. exhaust instances via a bounded `ServerOptions` equivalent, or simply assert the *listener remains usable after an `accept()` error* once the fix lands) would have caught it. On the Unix side, `bind` onto an existing stale/live path (finding 1.3's surface) is untested, so the chosen EADDRINUSE semantics are unpinned. Error propagation for `connect()` to a nonexistent endpoint is also unexercised (trivial passthrough, cheap to pin).
- **Failure scenario:** exactly finding 1.1's scenario, reintroduced by a refactor, with a fully green suite.
- **Suggested fix:** per CLAUDE.md's Red-Green-Refactor: write the failing rotation-under-error test first (Red), then land the 1.1 fix (Green).

---

## 2. concurrency-lockfree

**General verdict: clean.** No `Mutex`/`RwLock`/`parking_lot` anywhere; the listener is single-owner by construction (`&mut self accept()` enforces it at compile time — the right lock-free answer for a one-client-per-instance OS primitive). `unsafe impl Send/Sync for OwnerOnlySecurityDescriptor` (windows.rs:139-140) is justified: the `LocalAlloc`-backed SD is read-only, has no thread affinity, and every use is under a shared borrow with no `.await` between pointer hand-out and `CreateNamedPipe`. The bind→chmod sequence (unix.rs:50-51) contains no await point, so the module doc's no-interleaving claim holds. Remaining finding:

### 2.1 [LOW] Rotation leaves a no-pending-instance window → concurrent second client gets `ERROR_PIPE_BUSY`; the client does no BUSY retry
- **File:line:** `crates/shamir-transport-ipc/src/windows.rs:88-93` (instance N+1 created only **after** `pending.connect()` completes); consumer without retry: `crates/shamir-client/src/client.rs:234-245` (`connect_ipc` maps the raw io error straight to `ClientError::Io`).
- **Issue:** between a client's connect completing and the next `create_instance` finishing, the pipe name has zero pending instances. Windows clients connecting in that window get `ERROR_PIPE_BUSY`. `shamir-transport-ipc::connect` and `shamir-client`'s `connect_ipc` never retry on BUSY (the canonical Win32 remedy, `WaitNamedPipe`), so a burst of near-simultaneous connects — two sidecars/CLI tools starting together — yields a spurious connection failure for whichever lands in the window (microseconds of task-scheduling latency, no await between the steps in-process, but real wall-clock).
- **Failure scenario:** two same-user processes call `connect_local` at boot within microseconds of each other; the server's accept loop is between `connect()` and `create_instance` for client #1; client #2's `CreateFile` returns `ERROR_PIPE_BUSY` → `ClientError::Io` → boot of the second process fails with an opaque OS error instead of waiting its turn. (Unix has a kernel listen backlog and is immune.)
- **Suggested fix:** reorder `accept()` to create the replacement instance *before* `pending.connect().await` (pending-availability is then continuous; also one half of finding 1.1's fix), and/or add an `ERROR_PIPE_BUSY`→`WaitNamedPipe`-style retry in the client path.

---

## 3. security-crypto

**The `unsafe` block holds up under scrutiny.** Verified line-by-line (windows.rs):

- **SDDL correctness:** `format!("D:P(A;;GA;;;{sid})")` (`:148`) is character-identical to spec §5.2 — protected DACL (`P` blocks inheritance of laxer parent ACEs), single Generic-All ACE. The SID is the **real user SID** from `OpenProcessToken(TOKEN_QUERY)` + `GetTokenInformation(TokenUser)` + `ConvertSidToStringSidW` (`:200-280`) — not a shorthand (`OW`/`BA`/`SY`) that could outscan the intended principal, and it cannot contain SDDL metacharacters (OS-generated `S-1-…`), so no injection into the SDDL string. Running as a service yields the service account's SID, and the module doc (`:6-10`) explicitly documents that account-parity (not logon-session-parity) is the chosen model — consistent with spec §5.2.
- **Pointer lifetime:** `as_security_attributes()` hands out `&self.attrs` (`:177-179`); `create_instance` receives `sec_desc: &OwnerOnlySecurityDescriptor` — a field of `IpcListener`, which outlives every `create_with_security_attributes_raw` call; the pointer is consumed synchronously inside `CreateNamedPipe`, and there is no `.await` in `create_instance` (`:103-123`). The SAFETY comment at `:108-112` states exactly this and is accurate. Moving `IpcListener` (bind → return → spawn) copies the `SECURITY_ATTRIBUTES` by value; the pointed-to heap SD does not move, so no pinning is needed — sound.
- **Null-check / resource discipline:** `ConvertStringSecurityDescriptorToSecurityDescriptorW` result checked for both `ok == 0` and null `psd` (`:164-166`), freed once in `Drop` via `LocalFree` (`:182-195`); `OpenProcessToken` failure returns before `CloseHandle`, and the token is closed exactly once on every path via the closure-plus-close structure (`:205-208`, `:276-278`); the two-call `GetTokenInformation` size query is correct, including the explicit `ERROR_INSUFFICIENT_BUFFER` verification (`:215-224` — the second `last_os_error()` read happens before the `vec!` allocation, so no intervening FFI can clobber it); the `TOKEN_USER.User.Sid` PSID points into `buf`, whose lifetime outlives both `ConvertSidToStringSidW` and the string free (`:242-247`); the SID string is `LocalAlloc`-backed and freed exactly once (`:267-270`). `bInheritHandle: 0` (`:172`) — no handle leak to children. Least privilege: `TOKEN_QUERY` only (`:205`).
- **`PIPE_REJECT_REMOTE_CLIENTS`:** explicitly `.reject_remote_clients(true)` at `:117` — good (see nit 3.2 about the doc). `PipeMode::Byte` matches the length-prefixed framing spec §2.

Findings:

### 3.1 [LOW] Unix bind→chmod window is the only transport-reachable security gap; consider the atomic-rename hardening
- Cross-reference finding 1.4 (full scenario there). Security-lens verdict: microsecond window, spec-acknowledged, second-layer mitigated by SCRAM; the structural fix (bind-temp → chmod → atomic `rename` into place) is cheap and closes it outright. `windows-sys` versioning note: `0.60` resolves to `0.60.2`, which already existed in the graph via `socket2 0.6.2`, so the Cargo.toml pinning comment (`Cargo.toml:22-24`) is factually satisfied (lock carries 0.52/0.60/0.61 from other crates; this crate added no new version).

### 3.2 [NIT] Stale doc: "reject_remote_clients … is left at its default" while the code explicitly pins it
- **File:line:** `crates/shamir-transport-ipc/src/windows.rs:11-13` (doc) vs `:117` (explicit `.reject_remote_clients(true)`).
- **Issue:** the code does the *better* thing (explicit pinning survives a tokio default flip); the doc describes the weaker behavior. One-line doc fix — the security property shouldn't read as accidental.

---

## 4. performance-hotpath

**Clean — as expected for a thin OS shim.** Per-accept cost is one `CreateNamedPipe` call; the `SECURITY_DESCRIPTOR` and SDDL string are built **once** at `bind` and reused for every instance (windows.rs:70-76 — the right design; a per-accept SDDL rebuild would have been the thing to flag, and it isn't there). Unix `accept`/`connect` allocate nothing beyond the single `PathBuf` clone at bind (unix.rs:49). No hidden O(N), no per-row work, no locks on any path. One compile-time observation:

### 4.1 [NIT] `tokio features = ["full"]` pulls subsystems this crate never uses
- **File:line:** `crates/shamir-transport-ipc/Cargo.toml:14`.
- **Issue:** only `net` (+ runtime) is exercised; `full` drags in io-util/process/signal/etc. for any downstream that doesn't already have them. The comment documents that this matches the `shamir-transport-tcp` convention, and the workspace shares one tokio resolution anyway — compile-time-only cost, no runtime effect. Listed for completeness; no action required unless the workspace ever trims features wholesale.

---

## 5. api-wire-protocol

The *type-level* unification works: one `IpcListener`/`IpcStream`/`IpcClientStream`/`connect` set, compile-time dispatch, callers never `cfg`-branch to *use* a connection (`WriteSink` in the client and `accept_loop_ipc` in the server are the proof). But the *addressing* layer and the signature layer both leak:

### 5.1 [MEDIUM] Spec §9 logical endpoint names are unimplemented — and fail *differently* per OS
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:39-41` (`connect` passes the string to `UnixStream::connect` verbatim), `crates/shamir-transport-ipc/src/windows.rs:51-53` (verbatim into `ClientOptions::open`), `crates/shamir-server/src/config.rs:772-787` (validates only non-empty), doc claim: `crates/shamir-client/src/client.rs:119-122`.
- **Issue:** spec §9 defines a logical form — `shamir+unix://alice@shamir-db` → Windows client maps it to `\\.\pipe\shamir-db`. No crate implements that mapping. Consequences diverge by OS: on Windows an unprefixed name fails inside `CreateNamedPipe`/`CreateFile` with an obscure OS error; on Unix a logical name is silently treated as a **CWD-relative socket path** — so `connect("shamir-db")` can even *succeed* against an unrelated socket file in the working directory. The client's own doc ("`shamir_transport_ipc::connect` resolves the same string platform-appropriately") is currently false, and `tests/smoke_local.rs:45-62` has to `#[cfg]`-branch to build full endpoint strings — the "callers never branch on OS" goal stops at the transport type.
- **Failure scenario:** operator configures `addr: "shamir-db"` per spec §9's logical form: on Windows, boot/connect fails with `\\.\pipe\`-less name error; on a Unix server started from a directory where a socket named `shamir-db` happens to exist (e.g. a stale artifact), the server binds *that* file and the client connects to it — wrong endpoint, silently.
- **Suggested fix:** either implement §9 in the crate (a `resolve_endpoint(name) -> String` that prefixes `\\.\pipe\` when the input has no separator, on Windows) and have client+server call it, or change spec/docs to mandate full paths and make `Config::validate`/`ConnectLocalOptions` reject separator-free names with a clear error. Current state is the worst quadrant: undocumented-in-code, unenforced, OS-divergent.

### 5.2 [LOW] Platform-divergent public signatures: `connect`/`bind`/`path()` differ across `cfg`
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:39` (`connect(impl AsRef<Path>)`), `:48` (`bind(impl AsRef<Path>)`), `:62-64` (`path() -> &Path`); `crates/shamir-transport-ipc/src/windows.rs:51` (`connect(name: &str)`), `:68` (`bind(impl Into<String>)`), `:98-100` (`path() -> &str`).
- **Issue:** the same call sites compile on both OSes *for the argument types the consumers happen to use today* (`&String` derefs to `&str` and satisfies `AsRef<Path>`), but the signatures are not the same API. Generic code written against the Unix signature breaks on Windows: `connect(&path_buf)` (via `&PathBuf: AsRef<Path>`) does not compile on Windows; nothing at the type level forces a fix until someone builds for the other OS.
- **Failure scenario:** a new consumer writes transport-agnostic helper code taking `&Path` and calls `connect(path)` — compiles and passes CI on Linux CI runners, fails `cargo check` the first time a Windows build runs.
- **Suggested fix:** converge on one signature (`&str` on both — Unix paths are UTF-8 in practice here, or `impl AsRef<str>`), or route both through a common `IpcEndpoint` type. Note this interacts with 5.1: a shared endpoint resolver would naturally own the one true signature.

### 5.3 [LOW] `IpcStream`/`IpcClientStream` are transparent aliases — OS-specific inherent methods leak through the "unified" surface
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:30-36` (`pub type IpcStream = UnixStream`), `crates/shamir-transport-ipc/src/windows.rs:39-44` (`NamedPipeServer` / `NamedPipeClient`), goal statement: `crates/shamir-transport-ipc/src/lib.rs:3-10`.
- **Issue:** an alias *is* the underlying type: on Unix any caller can call `UnixStream::pair`, `.peer_addr()`, `.take_error()` on an `IpcStream`; on Windows `NamedPipeServer::disconnect`/`info` are reachable. Code that does so compiles fine and breaks only on the other platform — the abstraction is convention, not enforcement, and `lib.rs` doesn't document the leak. Error *types* are uniformly `io::Error` (good — no OS-specific error types escape), and `accept()` deliberately takes `&mut self` on both sides (good parity).
- **Failure scenario:** a contributor adds a diagnostics helper calling `stream.peer_addr()` (Unix-only inherent) inside shared code; green on Linux, broken build on Windows — discovered at packaging time, not review time.
- **Suggested fix:** either newtype both sides with `impl AsyncRead + AsyncWrite` forwarding (small, the shim has four methods to forward), or add a documented rule to `lib.rs` that callers must treat `IpcStream` as `AsyncRead + AsyncWrite` only. The newtype also gives finding 5.2's convergence a home.

### 5.4 [NIT] Bind-collision error text is OS-asymmetric and unmapped
- **File:line:** `crates/shamir-transport-ipc/src/unix.rs:50` (`Address already in use`), `crates/shamir-transport-ipc/src/windows.rs:71,116` (`first_pipe_instance(true)` → `ERROR_ACCESS_DENIED`, surfaced as "Access is denied (os error 5)").
- **Issue:** same operational mistake (double bind / name already owned) produces unrelated-sounding errors; on Windows "Access is denied" invites the wrong debugging instinct (permissions) for what is a name collision. Inherent to the OS primitives, but worth one mapping line in the error path or docs.

---

## 6. error-handling-lifecycle

Lifecycle verdicts per OS: Windows has no stale-name problem (the namespace entry vanishes with the last handle) and `Drop` closes instances correctly; Unix has the two file-lifecycle leaks below (also logged as correctness 1.2/1.3 — repeated here as the lens's summary, not double-counted in the table). Error *types* are uniform `io::Result` throughout with `?` propagation (CLAUDE.md-compliant for a shim whose only failure mode is OS errors), and consumer mapping is clean (`ClientError::Io(#[from] io::Error)`, `BootError::Bind`).

### 6.1 [LOW] Declared-but-unused `thiserror` dependency — the typed error this crate needs was never written
- **File:line:** `crates/shamir-transport-ipc/Cargo.toml:17` (`thiserror = "2.0"`; zero occurrences in `src/`).
- **Issue:** dead dependency weight, and a smell: the crate's two lifecycle bugs (1.1's poisoned-listener `expect`; 1.2's leak-on-chmod-failure) are exactly the places a small `IpcError` enum (`Bind`, `Accept { source, listener_poisoned }`, `#[from] io::Error`) would have forced the error paths to be designed rather than `?`-ed through. As-is, callers cannot distinguish "transient accept error, keep looping" from "listener is dead, stop looping" — which is precisely the distinction `accept_loop_ipc` gets wrong today.
- **Failure scenario:** a downstream `matches!`-based retry policy can only match on `io::ErrorKind`, so it cannot avoid re-calling a listener whose next `accept()` will panic (finding 1.1).
- **Suggested fix:** either introduce the small `thiserror` enum when landing the 1.1 fix (preferred — it gives the poisoned-listener state a name), or drop the dependency.

### 6.2 [LOW] No lifecycle tests for the Windows listener; Unix-only `Drop` coverage
- **File:line:** `crates/shamir-transport-ipc/src/tests/windows_tests.rs` (no drop/shutdown test; none possible at OS level) vs `unix_tests.rs:47-54` (`drop_removes_the_socket_file`).
- **Issue:** the asymmetry is fine at runtime (Windows needs no unlink), but nothing pins the Windows contract that a dropped listener releases the name (i.e. a `bind` → drop → `bind` same-name sequence succeeds). That is the exact property an operator relies on for restart, and it depends on tokio's handle-drop behavior staying as-is.
- **Failure scenario:** a future tokio change lazily defers instance teardown, or a refactor caches instances elsewhere; name release breaks and same-process rebind tests would catch it — today nothing would.
- **Suggested fix:** add `bind_drop_rebind_same_name_succeeds` to `windows_tests.rs`.

---

## 7. style-claude-md

**Largely exemplary.** Verified against CLAUDE.md: `src/tests/mod.rs` is a manifest of re-exports only (✔ §Test organisation); no inline `#[cfg(test)]` blocks in implementation files (✔); all imports at file top in every file (✔, including the cfg-gated `windows-sys` imports — the sanctioned exception applied correctly); `mod.rs`/`lib.rs` contain re-exports + docs only (✔); one-file-one-primary-export respected — `unix.rs`/`windows.rs` each own one tightly-coupled API family (listener + its aliases + its connect fn), with `OwnerOnlySecurityDescriptor`/`create_instance`/`to_wide_null` private internals (✔); no `anyhow`, no leaked `Box<dyn Error>`, no `panic!` outside the one contested `expect` (covered under 1.1); SAFETY comments on every `unsafe` block exceed the repo's written bar; doc comments cite the spec by name.

### 7.1 [NIT] Crate-level `src/tests/` instead of per-module `tests/` directories
- **File:line:** `crates/shamir-transport-ipc/src/tests/` (manifest `mod.rs` + `unix_tests.rs` + `windows_tests.rs`).
- **Issue:** CLAUDE.md prescribes "one `tests/` directory per module". Defensible here (the modules are two flat `cfg`-gated files, so `src/unix/tests/` would be heavier than the code), and it is the same deviation the sibling `shamir-transport-tcp` review logged — noting it for consistency, no action recommended until the crate grows a third module.

### 7.2 [NIT] CLAUDE.md's workspace roster predates this crate (repo-level drift)
- **File:line:** `CLAUDE.md:31-39` (23-crate list, no `shamir-transport-ipc`).
- **Issue:** the crate is the 24th member; the normative context file's roster and the 2026-08-14 audit set both predate it (this document closes the latter half of that gap). One-line roster update owed in the next docs pass.

---

## Finding counts

| Severity | Count | Findings |
|---|---|---|
| critical | 0 | — |
| high | 1 | 1.1 (accept-poison panic) |
| medium | 3 | 1.2 (chmod-fail socket leak) · 1.3 (no stale-path recovery) · 5.1 (§9 naming unimplemented) |
| low | 8 | 1.4/3.1 (chmod window) · 1.5 (Windows DACL/exclusivity tests) · 1.6 (error-path tests) · 2.1 (PIPE_BUSY window) · 5.2 (signature divergence) · 5.3 (alias leak) · 6.1 (unused thiserror) · 6.2 (Windows lifecycle test) |
| nit | 4 | 3.2 (stale reject_remote doc) · 5.4 (bind-error text asymmetry) · 7.1 (tests layout) · 7.2 (CLAUDE.md roster) |
| **total** | **16** | 1 high · 3 medium · 8 low · 4 nit |

*(1.4 and 3.1 are the same underlying item viewed from two lenses and counted once — as one `low`.)*

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Fix the poisoned-`accept()` invariant (1.1):** create the replacement pipe instance *before* `await`ing `pending.connect()`; on any residual error, restore `self.next` (or surface a named poisoned-listener error) so a subsequent `accept()` cannot reach the `expect`. Replace `expect` with typed-error propagation. Closes: 1.1, and by the same reorder closes 2.1 (PIPE_BUSY window) as a side effect.
2. **Add the regression tests first (Red per CLAUDE.md TDD):** an accept-error-path test (listener remains usable after an accept error) and `bind_drop_rebind_same_name_succeeds` (6.2). Closes: 1.6, 6.2.
3. **Unlink the socket file on the chmod-failure path (1.2):** one `let _ = std::fs::remove_file(&path);` before the error return. Closes: 1.2.

**P1 — soon**
4. **Stale-path recovery on Unix (1.3):** on bind EADDRINUSE, connect-probe; on ECONNREFUSED unlink + rebind, else surface "already running". Closes: 1.3.
5. **Implement or forbid spec §9 logical names (5.1):** add `resolve_endpoint` prefixing `\\.\pipe\` on Windows, call it from `bind`/`connect` — or reject separator-free names in `Config::validate` + `ConnectLocalOptions` with a clear message, and fix the false doc at `client.rs:119-122`. Closes: 5.1.
6. **Windows DACL + exclusivity tests (1.5):** assert the created pipe's SDDL is `D:P(A;;GA;;;S-1-<current-user>)`; assert second-bind-on-same-name fails. Closes: 1.5.
7. **Introduce the small `thiserror` enum (6.1)** alongside the 1.1 fix (`Accept`/`Bind`/`Poisoned` variants) so retry policy can distinguish transient from fatal. Closes: 6.1.
8. **Unify the public signatures (5.2)** — one `connect`/`bind`/`path` signature across both cfgs, ideally via the endpoint resolver from item 5. Closes: 5.2.

**P2 — backlog**
9. **Atomic-rename hardening for the Unix bind→chmod window (1.4/3.1)** — optional; spec accepts the current approach.
10. **Consider newtyping `IpcStream`/`IpcClientStream` (5.3)** or, minimally, document the aliases-are-transparent rule in `lib.rs`. Closes: 5.3.
11. **Doc nits:** fix the `reject_remote_clients` doc (3.2); add the EADDRINUSE↔ACCESS_DENIED bind-collision note (5.4); refresh CLAUDE.md's crate roster (7.2). Closes: 3.2, 5.4, 7.2; 7.1 needs no action.

</details>
