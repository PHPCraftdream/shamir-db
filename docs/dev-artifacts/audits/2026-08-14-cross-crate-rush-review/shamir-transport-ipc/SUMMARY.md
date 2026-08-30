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
