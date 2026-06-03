//! OS service install / uninstall / status — pure generation + cfg-gated side-effects.
//!
//! **Pure functions** (`systemd_unit`, `launchd_plist`, `rcd_script`,
//! `windows_image_path`, `absolute`, `SERVICE_NAME`) are cross-platform and
//! unit-tested on every OS.
//!
//! OS side-effects (writing unit files, calling `sc.exe` / `systemctl`) live
//! in cfg-gated functions that are NOT exercised by the test suite — they
//! require elevated privileges and their target OS.

use std::path::{Path, PathBuf};

use clap::Subcommand;

/// Service name used for both the systemd unit (`shamir-server.service`)
/// and the Windows SCM registration.
pub const SERVICE_NAME: &str = "shamir-server";

/// Subcommands for `shamir-server service <action>`.
#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Install the OS service. Requires elevated privileges.
    Install {
        /// Optional service user (Linux only: sets `User=` in the systemd unit).
        #[arg(long)]
        user: Option<String>,
    },
    /// Uninstall (stop + remove) the OS service.
    Uninstall,
    /// Print the current service status.
    Status,
}

/// Resolve `path` to an absolute form. A service runs with a different cwd,
/// so every path the service manager records must be absolute.
///
/// Uses [`std::path::absolute`] (stable since Rust 1.79). Falls back to
/// `current_dir().join(path)` if the std helper is somehow unavailable
/// (defensive — should not happen on 1.79+).
pub fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    // std::path::absolute does NOT consult the filesystem — it just prepends
    // cwd for relative paths and normalises `.`/`..`.
    std::path::absolute(path)
}

/// Generate a systemd `.service` unit file (pure, no IO).
///
/// The unit simply runs the existing foreground `run` subcommand;
/// `foreground_shutdown()` already handles SIGTERM (systemd's `KillSignal`).
pub fn systemd_unit(exe: &Path, config: &Path, user: Option<&str>) -> String {
    let exe_display = exe.display();
    let config_display = config.display();
    let user_line = match user {
        Some(u) => format!("User={u}\n"),
        None => String::new(),
    };
    format!(
        r#"[Unit]
Description=ShamirDB Server
After=network.target

[Service]
ExecStart="{exe_display}" --config "{config_display}" run
KillSignal=SIGTERM
Restart=on-failure
Type=simple
{user_line}[Install]
WantedBy=multi-user.target
"#
    )
}

/// Generate the Windows SCM `ImagePath` / `binPath` string (pure, no IO).
///
/// Returns something like `"<exe>" --config "<config>" run`.
pub fn windows_image_path(exe: &Path, config: &Path) -> String {
    format!(r#""{}" --config "{}" run"#, exe.display(), config.display())
}

/// Generate a macOS launchd `.plist` (pure, no IO). launchd sends SIGTERM on
/// stop, which `foreground_shutdown()` already handles.
pub fn launchd_plist(exe: &Path, config: &Path) -> String {
    let exe_display = exe.display();
    let config_display = config.display();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.shamir.server</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe_display}</string>
        <string>--config</string>
        <string>{config_display}</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// Generate a FreeBSD rc.d script (pure, no IO). rc.d stop sends SIGTERM via
/// daemon(8); `foreground_shutdown()` handles it.
pub fn rcd_script(exe: &Path, config: &Path) -> String {
    let exe_display = exe.display();
    let config_display = config.display();
    format!(
        r#"#!/bin/sh

# PROVIDE: shamir_server
# REQUIRE: NETWORKING
# KEYWORD: shutdown

. /etc/rc.subr

name="shamir_server"
rcvar="shamir_server_enable"
pidfile="/var/run/${{name}}.pid"
command="/usr/sbin/daemon"
command_args="-p ${{pidfile}} -f \"{exe_display}\" --config \"{config_display}\" run"

load_rc_config $name
run_rc_command "$1"
"#
    )
}

// ---------------------------------------------------------------------------
// OS side-effects (cfg-gated, NOT tested in the suite)
// ---------------------------------------------------------------------------

/// Install the service on the current platform.
///
/// Resolves absolute paths for the executable and config file, then delegates
/// to the platform-specific installer. Prints a human-readable summary and
/// next-steps.
pub fn install(config: &Path, user: Option<&str>) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let exe = absolute(&exe)?;
    let config = absolute(config)?;

    #[cfg(target_os = "linux")]
    {
        install_systemd(&exe, &config, user)?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = user;
        install_launchd(&exe, &config)?;
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        let _ = user;
        install_rcd(&exe, &config)?;
    }
    #[cfg(windows)]
    {
        let _ = user;
        install_windows(&exe, &config)?;
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        windows
    )))]
    {
        let _ = (&exe, &config, user);
        anyhow::bail!("service install is not supported on this platform");
    }

    Ok(())
}

/// Uninstall the service on the current platform.
pub fn uninstall() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        uninstall_systemd()?;
    }
    #[cfg(target_os = "macos")]
    {
        uninstall_launchd()?;
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        uninstall_rcd()?;
    }
    #[cfg(windows)]
    {
        uninstall_windows()?;
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        windows
    )))]
    {
        anyhow::bail!("service uninstall is not supported on this platform");
    }

    Ok(())
}

/// Print the current service status.
pub fn status() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        status_systemd()?;
    }
    #[cfg(target_os = "macos")]
    {
        status_launchd()?;
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        status_rcd()?;
    }
    #[cfg(windows)]
    {
        status_windows()?;
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        windows
    )))]
    {
        println!("service status is not supported on this platform");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Linux (systemd)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn install_systemd(exe: &Path, config: &Path, user: Option<&str>) -> anyhow::Result<()> {
    use std::io::Write;

    let unit = systemd_unit(exe, config, user);
    let unit_path = format!("/etc/systemd/system/{SERVICE_NAME}.service");

    let mut f = match std::fs::File::create(&unit_path) {
        Ok(f) => f,
        Err(e) => {
            anyhow::bail!("failed to create {unit_path}: {e}\nHint: run with sudo or as root");
        }
    };
    f.write_all(unit.as_bytes())?;

    // Best-effort daemon-reload.
    let _ = std::process::Command::new("systemctl")
        .arg("daemon-reload")
        .status();

    println!("installed {unit_path}");
    println!();
    println!("Next steps:");
    println!("  systemctl enable  {SERVICE_NAME}");
    println!("  systemctl start   {SERVICE_NAME}");
    println!();
    println!("NOTE: set `logging.file` to an ABSOLUTE path in your config so the");
    println!("service can write logs (systemd captures stdout to journald, but a");
    println!("dedicated log file is easier to tail and rotate).");

    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> anyhow::Result<()> {
    let unit_path = format!("/etc/systemd/system/{SERVICE_NAME}.service");

    // Best-effort stop + disable.
    let _ = std::process::Command::new("systemctl")
        .args(["stop", SERVICE_NAME])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["disable", SERVICE_NAME])
        .status();

    match std::fs::remove_file(&unit_path) {
        Ok(()) => println!("removed {unit_path}"),
        Err(e) => println!("could not remove {unit_path}: {e}"),
    }

    let _ = std::process::Command::new("systemctl")
        .arg("daemon-reload")
        .status();

    println!("service uninstalled");
    Ok(())
}

#[cfg(target_os = "linux")]
fn status_systemd() -> anyhow::Result<()> {
    let exit = std::process::Command::new("systemctl")
        .args(["status", SERVICE_NAME])
        .status();
    match exit {
        Ok(s) => {
            if !s.success() {
                println!("(systemctl returned {} — service may not be installed)", s);
            }
        }
        Err(e) => println!("failed to run systemctl: {e}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// macOS (launchd)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn install_launchd(exe: &Path, config: &Path) -> anyhow::Result<()> {
    use std::io::Write;

    let plist = launchd_plist(exe, config);
    let plist_path = "/Library/LaunchDaemons/com.shamir.server.plist";

    let mut f = match std::fs::File::create(plist_path) {
        Ok(f) => f,
        Err(e) => {
            anyhow::bail!("failed to create {plist_path}: {e}\nHint: run with sudo or as root");
        }
    };
    f.write_all(plist.as_bytes())?;

    // Best-effort load.
    let _ = std::process::Command::new("launchctl")
        .args(["load", "-w", plist_path])
        .status();

    println!("installed {plist_path}");
    println!();
    println!("Next steps:");
    println!("  sudo launchctl load -w {plist_path}");
    println!();
    println!("NOTE: set `logging.file` to an ABSOLUTE path in your config so the");
    println!("service can write logs (launchd does not capture stdout by default).");

    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> anyhow::Result<()> {
    let plist_path = "/Library/LaunchDaemons/com.shamir.server.plist";

    // Best-effort unload.
    let _ = std::process::Command::new("launchctl")
        .args(["unload", "-w", plist_path])
        .status();

    match std::fs::remove_file(plist_path) {
        Ok(()) => println!("removed {plist_path}"),
        Err(e) => println!("could not remove {plist_path}: {e}"),
    }

    println!("service uninstalled");
    Ok(())
}

#[cfg(target_os = "macos")]
fn status_launchd() -> anyhow::Result<()> {
    let exit = std::process::Command::new("launchctl")
        .args(["print", "system/com.shamir.server"])
        .status();
    match exit {
        Ok(s) => {
            if !s.success() {
                println!("(launchctl returned {} — service may not be installed)", s);
            }
        }
        Err(e) => println!("failed to run launchctl: {e}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BSD (rc.d)
// ---------------------------------------------------------------------------

#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn install_rcd(exe: &Path, config: &Path) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let script = rcd_script(exe, config);
    let script_path = "/usr/local/etc/rc.d/shamir_server";

    let mut f = match std::fs::File::create(script_path) {
        Ok(f) => f,
        Err(e) => {
            anyhow::bail!("failed to create {script_path}: {e}\nHint: run with sudo or as root");
        }
    };
    f.write_all(script.as_bytes())?;

    // Set executable permission (0o755).
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(script_path, perms)?;

    // Best-effort enable via sysrc.
    let _ = std::process::Command::new("sysrc")
        .arg("shamir_server_enable=YES")
        .status();

    println!("installed {script_path}");
    println!();
    println!("Next steps:");
    println!("  service shamir_server start");
    println!();
    println!("NOTE: set `logging.file` to an ABSOLUTE path in your config so the");
    println!("service can write logs (rc.d does not capture stdout by default).");

    Ok(())
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn uninstall_rcd() -> anyhow::Result<()> {
    let script_path = "/usr/local/etc/rc.d/shamir_server";

    // Best-effort stop.
    let _ = std::process::Command::new("service")
        .args(["shamir_server", "stop"])
        .status();

    match std::fs::remove_file(script_path) {
        Ok(()) => println!("removed {script_path}"),
        Err(e) => println!("could not remove {script_path}: {e}"),
    }

    // Best-effort remove sysrc variable.
    let _ = std::process::Command::new("sysrc")
        .args(["-x", "shamir_server_enable"])
        .status();

    println!("service uninstalled");
    Ok(())
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn status_rcd() -> anyhow::Result<()> {
    let exit = std::process::Command::new("service")
        .args(["shamir_server", "status"])
        .status();
    match exit {
        Ok(s) => {
            if !s.success() {
                println!("(service returned {} — service may not be installed)", s);
            }
        }
        Err(e) => println!("failed to run service: {e}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows (sc.exe)
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn install_windows(exe: &Path, config: &Path) -> anyhow::Result<()> {
    let bin_path = windows_image_path(exe, config);

    let exit = std::process::Command::new("sc.exe")
        .args([
            "create",
            SERVICE_NAME,
            "binPath=",
            &bin_path,
            "start=",
            "auto",
            "DisplayName=",
            "ShamirDB Server",
        ])
        .status();

    match exit {
        Ok(s) if s.success() => {
            println!("installed Windows service '{SERVICE_NAME}'");
            println!();
            println!("Next steps:");
            println!("  sc start {}", SERVICE_NAME);
            println!();
            println!("NOTE: set `logging.file` to an ABSOLUTE path in your config so");
            println!("the service can write logs (Windows services have no console).");
        }
        Ok(s) => {
            anyhow::bail!(
                "sc.exe create returned exit code {}.\nHint: run from an elevated (Administrator) prompt",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            anyhow::bail!("failed to run sc.exe: {e}");
        }
    }

    Ok(())
}

#[cfg(windows)]
fn uninstall_windows() -> anyhow::Result<()> {
    // Best-effort stop first.
    let _ = std::process::Command::new("sc.exe")
        .args(["stop", SERVICE_NAME])
        .status();

    let exit = std::process::Command::new("sc.exe")
        .args(["delete", SERVICE_NAME])
        .status();

    match exit {
        Ok(s) if s.success() => println!("service '{SERVICE_NAME}' deleted"),
        Ok(s) => println!(
            "sc.exe delete returned exit code {} — service may not be installed",
            s.code().unwrap_or(-1)
        ),
        Err(e) => println!("failed to run sc.exe: {e}"),
    }

    Ok(())
}

#[cfg(windows)]
fn status_windows() -> anyhow::Result<()> {
    let exit = std::process::Command::new("sc.exe")
        .args(["query", SERVICE_NAME])
        .status();
    match exit {
        Ok(s) => {
            if !s.success() {
                println!(
                    "(sc.exe query returned {} — service may not be installed)",
                    s.code().unwrap_or(-1)
                );
            }
        }
        Err(e) => println!("failed to run sc.exe: {e}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — pure generation only (cross-platform, Windows-safe)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
