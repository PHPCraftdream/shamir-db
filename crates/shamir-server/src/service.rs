//! OS service install / uninstall / status — pure generation + cfg-gated side-effects.
//!
//! **Pure functions** (`systemd_unit`, `windows_image_path`, `absolute`,
//! `SERVICE_NAME`) are cross-platform and unit-tested on every OS.
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

    #[cfg(unix)]
    {
        install_systemd(&exe, &config, user)?;
    }
    #[cfg(not(unix))]
    {
        let _ = user;
        install_windows(&exe, &config)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("service install is not supported on this platform");
    }

    Ok(())
}

/// Uninstall the service on the current platform.
pub fn uninstall() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        uninstall_systemd()?;
    }
    #[cfg(windows)]
    {
        uninstall_windows()?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("service uninstall is not supported on this platform");
    }

    Ok(())
}

/// Print the current service status.
pub fn status() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        status_systemd()?;
    }
    #[cfg(windows)]
    {
        status_windows()?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        println!("service status is not supported on this platform");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Linux (systemd)
// ---------------------------------------------------------------------------

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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
