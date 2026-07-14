בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Milestone: Runtime modes — foreground + service (one binary)

ONE binary that runs both as a normal foreground app (like `node app.js`)
and as a managed OS service (Windows Service / Linux systemd), preserving the
charter's single self-contained binary.

**Depends on `GRACEFUL_SHUTDOWN.md`** — every mode funnels into the same
graceful shutdown; do that milestone first.

## The shape
The binary picks a mode by SUBCOMMAND, but the actual work — `serve(cfg,
shutdown)` (the existing `ServerLauncher`) — is identical across modes. Only
the **shutdown trigger** and a thin **per-OS start wrapper** differ.

```
shamir-server run                       → foreground
shamir-server service run               → run under the OS service manager
shamir-server service install|uninstall|start|stop|status
```

```rust
async fn serve(cfg, shutdown: impl Future) -> Result<()> {
    ServerLauncher::new(cfg)?.run_until(shutdown).await   // GRACEFUL_SHUTDOWN
}
// foreground:      shutdown = ctrl_c() ∪ SIGTERM
// windows service: shutdown = SCM Stop event (control handler)
// linux service:   shutdown = SIGTERM (systemd)
```

## Single-binary preservation
All OS-specific glue is `#[cfg(windows)]` / `#[cfg(unix)]`. One per-OS binary
(one binary = OS × arch) carries only its platform's code. Deps are small
Rust crates linking SYSTEM libs (`windows-service` → advapi32; `sd-notify`
→ pure Rust), NOT external runtimes → self-contained, charter intact.

## Slices (each = one agent delegation; zero-trust + green gate)

### RM-1 — Subcommand dispatch + `serve(cfg, shutdown)` core
- Extract the run core into `serve(cfg, shutdown)` parameterised by the
  shutdown source (uses the `ShutdownController` from GRACEFUL_SHUTDOWN).
- A CLI dispatcher (clap or hand-rolled): `run` (default) + `service
  run|install|uninstall|start|stop|status`.
- Where: `shamir-server` binary `main`.
- Tests: dispatch picks the right mode; `run` works as today.

### RM-2 — Foreground mode
- `run` → `serve(cfg, ctrl_c ∪ SIGTERM)`. Logs to stdout. Ctrl+C / SIGTERM →
  graceful shutdown. This is the "как запустят" mode.
- Acceptance: start in a terminal, Ctrl+C → clean exit within the deadline.

### RM-3 — Windows service (cfg(windows))
- `windows-service` crate: `service run` enters `service_dispatcher::start`;
  `service_main` builds the `ShutdownController`, registers a control handler
  (Stop/Shutdown → `controller.trigger()`), reports `Running`/`StopPending`/
  `Stopped` to the SCM within its timeouts, and runs `serve(cfg, controller
  .triggered())`.
- Acceptance: installed service starts, `sc stop` triggers graceful
  shutdown, SCM sees `Stopped`.

### RM-4 — Linux service (cfg(unix))
- `service run` → `serve(cfg, SIGTERM)`; optional `sd_notify` (Type=notify:
  READY=1 after bind, optional WATCHDOG). Logs to stdout → journald.
- Acceptance: under systemd, `systemctl stop` → graceful shutdown.

### RM-5 — ServiceManager (self-install)
- `trait ServiceManager { install(cfg); uninstall(); start(); stop(); status(); }`
  + `WindowsServiceManager` (SCM API via `windows-service`, or `sc.exe`;
  ImagePath = `<exe> service run`) + `SystemdManager` (write a `.service`
  unit with `ExecStart=<exe> service run`, `systemctl enable/start`).
- Acceptance: `service install` then the OS shows/starts the service;
  `uninstall` removes it.

### RM-6 — Mode-aware logging + absolute paths
- Foreground → stdout; Windows service (no console!) → Event Log or a file;
  Linux service → stdout (journald). Route the log layer by mode.
- Data/cert/config paths resolved to ABSOLUTE (a service runs with a
  different cwd/user). Document the service user/permissions interplay with
  the data-dir (ties into Shomer file ownership).

## Acceptance for the milestone
One binary runs foreground AND as a Win/Linux service; self-install works;
all modes shut down gracefully (GRACEFUL_SHUTDOWN); single-binary +
self-contained preserved; existing server e2e green. Gate: `fmt --all
--check`, `clippy --workspace --all-targets -D warnings`, `test --workspace`.

## Honest sharp edges
- **Windows service has no console** → stdout is lost; logs MUST go to Event
  Log/file (RM-6).
- **Absolute paths** — service cwd/user differs.
- **SCM status timeouts** — report Running/Stopped promptly or SCM kills it.
- **Service privileges** — which user runs it (Linux `User=`, Windows
  account) vs data-dir ownership.
- Service install/start/stop tests need OS privileges → gate them (skip in
  CI without admin); test the dispatch + the unit-file/ImagePath generation
  deterministically instead.

## For agents
Order RM-1 → RM-2 → (RM-3 ∥ RM-4, per-OS) → RM-5 → RM-6. Each a `/crush`
slice, zero-trust + gate. Do AFTER the graceful-shutdown milestone.
