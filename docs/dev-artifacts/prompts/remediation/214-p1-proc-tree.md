# Stage G5d — RV-WASM-HOST-58C052B0: compiler process-tree teardown via new crate `shamir-proc-tree`

## Owner decision (2026-10-27)

- The Windows Job Object FFI goes into a **new workspace crate `crates/shamir-proc-tree`**, mirroring `crates/shamir-private-dir`. Read that crate first and copy its Cargo.toml shape: workspace inheritance, lints, and where unsafe is allowed.
- `shamir-wasm-host` KEEPS `#![forbid(unsafe_code)]` (DEC-3). Do not touch that attribute.
- The `windows-sys` 0.60 dependency is approved, as a `[target.'cfg(windows)'.dependencies]` entry of `shamir-proc-tree` only. Use the features needed for JobObjects, Threading, ToolHelp and Foundation. No other new dependency. On Unix use `rustix` with the features `shamir-wasm-host` already enables; if an extra rustix feature is needed, say so in the final message.
- Add the crate to the CLAUDE.md "Workspace" list and change "26 crates" to "27 crates". `members = ["crates/*"]` picks it up automatically.

Card text: `### RV-WASM-HOST-58C052B0` in `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md`. A previous failed attempt is at `D:/system_artefact/orch/g5c_backup_r1/full.diff`. It is for reference only and its teardown code is NOT acceptable.

## What is wrong today (`crates/shamir-wasm-host/src/compile.rs`)

- `:721-726` sets up no process group or job.
- `:729-745` joins the reader threads without a bound.
- `:756-759` ignores a `kill()` failure, then calls `wait()` and joins, both unbounded.
- `:765-767` handles a wait error by joining without killing.
- `:772-773` joins without a bound on the success path, so a leftover descendant that holds the pipes hangs it.

## Crate `shamir-proc-tree`

`src/` has at most 7 entries:

- `lib.rs`: re-exports only, plus crate docs.
  - `#![cfg_attr(not(windows), forbid(unsafe_code))]`
  - `#![cfg_attr(windows, deny(unsafe_code))]`
- `process_tree_unix.rs` and `process_tree_windows.rs`: the same inherent API for `ProcessTree`.
- `job_object_windows.rs`:
  - the ONLY unsafe code, under `#[allow(unsafe_code)]`;
  - `#![warn(clippy::undocumented_unsafe_blocks)]` at its top;
  - a `// SAFETY:` comment on every `unsafe {}` block.
- `supervise.rs`: the supervisor state machine, plus a private `PipeReader`.
- `tests/`: a `mod.rs` manifest and `teardown_tests.rs`.

### Public API

```rust
pub struct ProcessTree { .. }
impl ProcessTree {
    pub fn spawn(cmd: &mut Command) -> io::Result<(Child, ProcessTree)>;
    fn wait_exit(&mut self, child: &mut Child, timeout: Duration) -> io::Result<bool>; // observes exit; on Unix never reaps
    fn terminate(&self) -> io::Result<()>;
    fn reap(&mut self, child: &mut Child, problems: &mut Vec<String>) -> Option<ExitStatus>;
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TeardownFaults { pub fail_tree_kill: bool, pub fail_wait: bool } // per-call test seam, never global
pub fn supervise<O: Read + Send + 'static, E: Read + Send + 'static>(
    child: Child, tree: ProcessTree, stdout: Option<O>, stderr: Option<E>,
    timeout: Duration, faults: TeardownFaults) -> Result<Output, String>;
```

### Unix `ProcessTree` (rustix process API)

**`spawn`**
- Set `CommandExt::process_group(0)`, then spawn, then compute `pgid = Pid::from_child(&child)`.
- Start a named waiter thread. It loops on `waitid(WaitId::Pid(pgid), WaitIdOptions::EXITED | WaitIdOptions::NOWAIT)`, retries on `Errno::INTR`, and sends the result on a `sync_channel(1)`.
- If the thread fails to spawn: `kill_process_group`, then `child.wait()`, then return Err.

**`wait_exit`**
- Calls `recv_timeout`.
- `Ok(Ok)` sets `exit_seen`. A timeout returns false. A disconnected channel is an error.

**`terminate`**
- If already `reaped`, return Ok: the group id is no longer pinned.
- Otherwise `kill_process_group(pgid, KILL)`, treating `Errno::SRCH` as Ok.

**`reap`**
- Only when `exit_seen`. First a final `terminate()`; the zombie leader still pins the group id. Then `child.wait()`, which returns instantly, and set `reaped = true`.
- If exit was not seen, record "leader not reaped" and never block.
- The leader is NEVER reaped before the group kill.

**`Drop`**: `let _ = self.terminate();`

### Windows `ProcessTree`

Fields: `{ job: JobObject, status: Option<ExitStatus> }`. `wait_timeout::ChildExt` may be used if it is already a workspace dependency; otherwise use `WaitForSingleObject` inside `job_object_windows.rs`.

**`spawn`**
- `JobObject::kill_on_close()?`, then `creation_flags(CREATE_SUSPENDED)`, then spawn.
- Then `job.assign(&child)` followed by `resume_primary_thread(child.id())`.
- On error: `child.kill()`, `child.wait()`, return Err.
- While the child is suspended it cannot have descendants.

**`wait_exit`** stores the status. **`terminate`** calls `job.terminate()`. **`reap`** returns the stored status.

**`Drop`**: closing the job's `OwnedHandle` triggers `KILL_ON_JOB_CLOSE`. Document this on the struct.

### `job_object_windows.rs`

All windows-sys structs derive `Default`, so `mem::zeroed` is not needed. `HANDLE` is the same as `RawHandle`.

- `pub(crate) struct JobObject(OwnedHandle)`
- `kill_on_close()`
  - `CreateJobObjectW(null, null)`; null means error. Wrap the result with `OwnedHandle::from_raw_handle`.
  - Fill `JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default()` with `BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
  - Call `SetInformationJobObject(h, JobObjectExtendedLimitInformation, ptr.cast(), size_of as u32)`.
- `assign(&Child)` calls `AssignProcessToJobObject(job, child.as_raw_handle())`.
- `terminate()` calls `TerminateJobObject(job, 1)`.
- `resume_primary_thread(pid)`
  - `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)`; `INVALID_HANDLE_VALUE` means error. Wrap it in `OwnedHandle`.
  - Build `THREADENTRY32 { dwSize: size_of as u32, ..Default::default() }` and walk `Thread32First` / `Thread32Next`.
  - For each thread whose `th32OwnerProcessID` matches: `OpenThread(THREAD_SUSPEND_RESUME, 0, tid)` (null means error), wrapped in `OwnedHandle`, then `ResumeThread` (`u32::MAX` means error).
  - Finding zero matching threads is an error.
- Do NOT set `BREAKAWAY_OK`.

### Supervisor state machine (`supervise.rs`)

`GRACE` is 5 s. Each `PipeReader` runs in its own `thread::Builder` thread: `read_to_end`, ignoring read errors as today, then send the bytes on a `sync_channel(1)`. Its `collect(deadline)` uses `recv_timeout`. A reader still pending at the end records "{name} drain timed out" and its thread is detached.

1. Start both readers. If one fails to spawn, the outcome is `Failed("pipe reader spawn failed: …")`; skip step 2.
2. Determine the outcome from `wait_exit(timeout)`, or from an injected `io::Error::other("injected wait failure")` when `faults.fail_wait` is set. The outcome is one of:
   - `Exited`
   - `TimedOut`
   - `Failed("cargo wait failed: {e}")`
3. If the outcome is not `Exited`:
   - Run `kill_tree`: call `terminate()`, or produce the injected "injected tree kill failure".
   - If that errors: `log::error!` it, record `tree kill: {e}`, fall back to `child.kill()`, and record `child kill: {e2}` if the fallback fails too. Keep BOTH errors.
   - Then call `wait_exit(GRACE)`. If it returns false or errors, record "leader did not exit within 5s" or the error.
4. Drain both readers with `deadline = now + GRACE`.
5. If the outcome is `Exited` but a reader is still pending (a straggler holds the pipe): run `kill_tree`, then drain again with a fresh `GRACE`.
6. `tree.reap(&mut child, &mut problems)`. The tree drops when the function returns.
7. Build the result:

   | Outcome | Result |
   |---|---|
   | `Exited`, no problems, status present | `Ok(Output { status, stdout, stderr })` |
   | `TimedOut` | `"compilation timed out after {N}s"` (`compile_tests.rs:481` checks this text) |
   | `Failed(m)` | `m` |
   | `Exited` with problems | `"cargo build teardown incomplete: …"` |

   When there are problems, append `"; teardown incomplete: " + problems.join("; ")`.

The worst case is bounded by `timeout + 3×GRACE`.

## `shamir-wasm-host` changes

- **`Cargo.toml`**: add `shamir-proc-tree = { path = "../shamir-proc-tree" }`, matching how other internal crates are referenced.
- **`compile.rs`**, call site at `:721-776`:
  - Add `cmd.stdin(Stdio::null())`, keep stdout and stderr piped.
  - Replace the spawn/wait/join block with:
    ```rust
    let (mut child, tree) = ProcessTree::spawn(&mut cmd).map_err(|e| FunctionError::Compute(format!("cargo invocation: {e}")))?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let output = supervise(child, tree, out, err, timeout, TeardownFaults::default()).map_err(FunctionError::Compute)?;
    ```
    Then check `output.status` / `output.stderr` exactly as `:775-780` does today.
  - Drop the now-unused imports.
  - Do NOT move `compile.rs` into a folder.
  - Do NOT touch any other code or comment in it.

## Tests (`crates/shamir-proc-tree/src/tests/teardown_tests.rs`)

Run every scenario inside a helper thread. The main thread waits with `recv_timeout(60 s)` and fails with "teardown hung" if it fires. No timing assertions.

**Readiness handshake.** The test calls `ProcessTree::spawn`, wraps stdout in a `BufReader`, and reads lines until it sees "ready". Only then does it pass `Some(bufreader)` and `stderr` to `supervise`.

Commands:
- Unix: `sh -c 'sleep 1000 & echo ready; wait'`.
- Windows: `cmd` with `raw_arg("/C cmd /C \"echo ready& ping -n 1000 127.0.0.1 >NUL\"")`.
- Straggler variant:
  - Unix: `sh -c 'sleep 1000 & echo ready'`.
  - Windows: `raw_arg("/C start \"\" /b cmd /C \"echo ready& ping -n 1000 127.0.0.1 >NUL\"")`.
- The Windows cmd quoting is a best guess. Keep it simple and explain it in a comment; CI verifies it.

| Test `rv_wasm_host_58c052b0_…` | Check | Revert that makes it fail |
|---|---|---|
| `timeout_kills_descendants_holding_pipes` | timeout ZERO: error contains "compilation timed out" and no "teardown incomplete" | killing only the direct child |
| `wait_error_kills_tree` | `fail_wait`, timeout 600 s: "cargo wait failed: injected wait failure" and no "teardown incomplete" | no kill on the wait-error path |
| `kill_failure_is_bounded` | `fail_tree_kill`, timeout ZERO: returns; contains "tree kill: injected tree kill failure", "teardown incomplete" and "drain timed out" | dropping the kill error, or an unbounded drain (hang) |
| `exited_leader_straggler_is_torn_down` | straggler variant: `Ok`, `status.success()` | no kill + re-drain after a reader timeout |
| `dropping_tree_kills_descendants` | after the handshake: `drop(tree)`; `read_to_end` on the stdout the test kept reaches EOF; then `child.wait()` | no `Drop` / `KILL_ON_JOB_CLOSE` |

The message `"compilation timed out after {N}s"` must be produced by `supervise`, so pass the timeout through for the message.

## Known risk (document it in the crate docs, do not engineer around it)

Killing the tree also kills an sccache server that the compile itself started.

## Gates (all must exit 0)

- `cargo clippy -p shamir-proc-tree -p shamir-wasm-host --all-targets -- -D warnings`
- `cargo fmt -p shamir-proc-tree -p shamir-wasm-host -- --check`
- `python -I scripts/ci/check_layout.py`

The orchestrator regenerates `Cargo.lock` if needed. If your edits change it, say so; do not run `cargo update`.

## Final message

List the files changed. For each test, state the revert that makes it fail.
