# Service lifecycle & health

How `shamir-server` behaves as an OS service — install, readiness, shutdown,
restart, single-instance, live reconfig, and health probing. The run side is a
single `serve(config, bootstrap, shutdown, on_ready)` core reused by every
mode; only the shutdown trigger and the readiness callback differ.

## Install / register

`shamir-server service install` writes the right init-system artifact per OS
and registers it:

| OS | Artifact | Generator |
|----|----------|-----------|
| Linux | `/etc/systemd/system/shamir-server.service` (`Type=notify`) | `systemd_unit` |
| macOS | `/Library/LaunchDaemons/com.shamir.server.plist` (`RunAtLoad`/`KeepAlive`) | `launchd_plist` |
| FreeBSD/*BSD | `/usr/local/etc/rc.d/shamir_server` (`daemon -r`) | `rcd_script` |
| Windows | SCM service (`sc create` + `sc failure` recovery) | `sc.exe` |

`service uninstall` / `service status` mirror these per OS.

## Readiness — start succeeds only after the listeners bind

The init system is told the server is **ready** only after `launch()` has bound
its listeners (not at fork / before-bind), so `systemctl start` / `sc start`
returns success when the socket actually accepts connections:

- **Linux/systemd** — the unit is `Type=notify`; the foreground process sends
  `sd_notify(READY=1)` (`runtime::notify_ready`) post-bind. Best-effort, a no-op
  when not run under systemd.
- **Windows/SCM** — `Running` is reported from the `on_ready` callback (after
  bind), not before `serve()`.
- **macOS/BSD/plain** — no-op (launchd/rc.d don't have a readiness protocol).

## Graceful shutdown (bounded)

`SIGTERM` (systemd / `kill`), `Ctrl+C`, or the SCM `Stop`/`Shutdown` control
triggers the same graceful drain: stop accepting, finish in-flight work, flush
buffers. The drain is bounded by `SHUTDOWN_DEADLINE` (30 s); if it is exceeded
(e.g. a stuck connection) the server logs a warning and exits rather than
blocking until the init system SIGKILLs.

## Restart on failure

| OS | Mechanism |
|----|-----------|
| Linux | `Restart=on-failure` in the unit |
| macOS | `KeepAlive` in the plist |
| FreeBSD/*BSD | `daemon(8) -r` (supervise + restart) |
| Windows | `sc failure … actions= restart/5000/restart/5000/restart/5000 reset= 86400` |

## Single-instance guard

At boot, before opening any store, `launch()` takes a crash-safe advisory OS
file lock (`fs4`, flock/LockFileEx) on `<data_dir>/.shamir.lock`. A second
process on the same `data_dir` fails fast with `BootError::AlreadyRunning`
instead of contending/corrupting the redb stores. The kernel releases the lock
on exit or crash (no stale-pidfile problem); the lock is held for the process
lifetime and released via RAII on shutdown. The pid is written into the file
for diagnostics.

## Live reconfig — `SIGHUP`

`kill -HUP <pid>` (unix) re-reads the config file and applies the new **log
level** live via the lock-free `ArcSwap<LogMask>` — no restart needed. Only the
log level is hot-reloadable; listeners, `data_dir`, TLS, and the log-file sink
require a restart. (Log-file rotation/reopen is a future addition — the
non-blocking appender holds the file handle.)

## Health probing

No separate HTTP health endpoint is shipped — it would add a port and surface
that the lean, self-contained design avoids. Use the existing signals:

- **Liveness** — a plain **TCP connect** to a bound listener. If it accepts,
  the process is up. Works without TLS/SCRAM, so every load balancer / init
  watchdog / orchestrator can use it.
- **Readiness** — the init-system readiness signal above (systemd `Type=notify`
  / SCM `Running`) marks the service started only once the listener is bound.
- **Authenticated round-trip health** — the wire `Ping` request (over
  TLS+SCRAM) proves the full protocol path is serving, for monitors that hold
  credentials.
