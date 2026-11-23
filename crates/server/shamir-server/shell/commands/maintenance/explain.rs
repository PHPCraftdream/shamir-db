//! The single home of the operator text for the explained-only offline
//! commands (design `docs/dev-artifacts/design/admin-shell.md` §4.10).
//!
//! Every offline command the shell refuses to run itself is described
//! here, in one place: the manual command lines, the OS rights the run
//! needs, the destructive effects, the implementation limits and the
//! caveats. No other module repeats this text.
//!
//! The printed command lines are pinned against the server's own argument
//! parser by the integration tests, so a rename or a moved flag on the
//! server side fails the tests instead of silently drifting from the
//! manual the operator reads.

use crate::shell::output::ShellError;

/// The server binary an explained manual invokes.
pub(in crate::shell) const BINARY: &str = "shamir-server";

/// The config placeholder appended to a manual that needs a server
/// configuration file.
pub(in crate::shell) const CONFIG_PLACEHOLDER: &str = "--config CONFIG";

/// The explained-only offline commands, one variant per operator manual.
#[derive(Debug, Clone, Copy)]
pub(in crate::shell) enum Explained {
    /// Serves `backup verify`.
    BackupVerify,
    /// Serves `backup restore`.
    BackupRestore,
    /// Serves `backup snapshot`.
    BackupSnapshot,
    /// Serves `logical export`.
    ExportLogical,
    /// Serves `logical import`.
    ImportLogical,
    /// Serves `diag integrity`.
    DiagIntegrity,
    /// Serves `version migrate`.
    VersionMigrate,
}

/// One explained-only command: its manuals and the operator-facing facts
/// about running it.
pub(in crate::shell) struct Explainer {
    /// The stable identifier the shell looks the entry up by.
    id: &'static str,
    /// Accepted manual forms; each one is a token sequence after the
    /// binary (and optional config placeholder).
    manuals: &'static [&'static [&'static str]],
    /// Whether every manual form carries [`CONFIG_PLACEHOLDER`].
    needs_config: bool,
    /// OS access the run requires.
    os_rights: &'static str,
    /// What the run writes, renames or deletes.
    destructive_effects: &'static str,
    /// What the implementation does not do, or does only partially.
    limits: &'static str,
    /// What the operator should know before relying on the result.
    caveats: &'static str,
}

impl Explainer {
    /// Renders every accepted manual as a printable command line.
    pub(in crate::shell) fn command_lines(&self) -> Vec<String> {
        self.manuals
            .iter()
            .map(|tokens| {
                let mut line = String::from(BINARY);
                if self.needs_config {
                    line.push(' ');
                    line.push_str(CONFIG_PLACEHOLDER);
                }
                for token in tokens.iter() {
                    line.push(' ');
                    line.push_str(token);
                }
                line
            })
            .collect()
    }
}

/// The explained-only manuals, one entry per [`Explained`] variant.
pub(in crate::shell) const EXPLAINED: &[Explainer] = &[
    Explainer {
        id: "backup verify",
        manuals: &[&["backup-verify", "SNAPSHOT"]],
        needs_config: false,
        os_rights: "read access to the snapshot directory and to every file it lists; nothing is opened in the server data directory, no server process and no --config are needed",
        destructive_effects: "none: the command only reads and hashes; it never writes, renames or deletes anything",
        limits: "it verifies manifest.json and hashes every listed file, so it is not a cheap check: the run takes time proportional to the snapshot size",
        caveats: "verify proves the snapshot matches its own manifest; it does not prove the data is usable — that is what a restore into a scratch directory shows",
    },
    Explainer {
        id: "backup restore",
        manuals: &[&["restore", "--from", "SNAPSHOT"]],
        needs_config: true,
        os_rights: "OS ownership of the server data directory and of its parent directory; the server whose --config points at that data directory must be stopped",
        destructive_effects: "destructive: after verifying the snapshot's manifest it swaps the live data directory — the current directory is renamed to a .pre_restore_backup_<timestamp> sibling (preserved, not deleted) and the snapshot takes its place; every outstanding resumption ticket in the restored user directory is invalidated",
        limits: "the liveness probe refuses while a server holds the data directory, and nothing here skips it",
        caveats: "take a fresh copy first, and restore only a snapshot this tool has verified",
    },
    Explainer {
        id: "backup snapshot",
        manuals: &[&["backup", "--to", "DIR"]],
        needs_config: true,
        os_rights: "write access to DIR and OS ownership of the server data directory; stop the server first for a fully consistent copy",
        destructive_effects: "writes <DIR>/YYYYMMDD_HHMMSS/ together with a manifest.json, creating DIR when missing; the data directory itself is not modified",
        limits: "a copy taken while the server runs races in-flight writes",
        caveats: "verify the snapshot with backup-verify before you rely on it",
    },
    Explainer {
        id: "version migrate",
        manuals: &[
            &["format-migrate", "--plan"],
            &["format-migrate", "--yes", "--from-backup", "SNAPSHOT"],
        ],
        needs_config: true,
        os_rights: "OS ownership of the server data directory; the server must be stopped",
        destructive_effects: "destructive: with --yes it rewrites the on-disk format stamps; without --yes it prints a plan and changes nothing",
        limits: "the applying form requires a recent verified snapshot; --max-backup-age-hours N bounds its age (default 24 hours)",
        caveats: "run the plan first and read it; the shell never applies a format migration",
    },
    Explainer {
        id: "export logical",
        manuals: &[&["logical-export", "--db", "DATABASE", "--output", "PATH"]],
        needs_config: true,
        os_rights: "OS ownership of the server data directory; the server must be stopped (offline only)",
        destructive_effects: "reads the stopped database and writes one dump artifact at --output; the data directory is not modified",
        limits: "the documented initial lossless subset only: Fjall repositories with default durability, plain unbound and index-free tables, default buffer sizing plus TTL, unambiguous scalar and map records; indexes, stored schemas, functions, validators, groups, guest modules, in-memory or hybrid repositories, non-default durability overrides and non-default buffer or flusher settings are refused rather than omitted",
        caveats: "narrow the scope with --repo REPO and --table TABLE (--table requires --repo); --schema-only writes a schema-only artifact; the internal system database cannot be exported",
    },
    Explainer {
        id: "import logical",
        manuals: &[&["logical-import", "--input", "PATH", "--target", "DIR"]],
        needs_config: true,
        os_rights: "write access to the parent of DIR; DIR must be absent or empty, and no server may hold it",
        destructive_effects: "creates a NEW data directory at --target and loads the dump into it; an existing data directory is never replaced",
        limits: "the artifact must come from the same lossless subset as export; the import stages beside the target and publishes with one rename, and a failed attempt leaves no partial target",
        caveats: "--as-db NAME renames the imported database; --schema-only requires a schema-only artifact; the shell never imports into a live data directory",
    },
    Explainer {
        id: "diag integrity",
        manuals: &[&["doctor"]],
        needs_config: true,
        os_rights: "OS ownership of the server data directory; the server must be stopped, because the check opens the data directory directly",
        destructive_effects: "read-only without --apply; with --apply it repairs unhealthy tables in place by dropping and rebuilding all of their indexes",
        limits: "a scan of a live store can see transient miscounts, so integrity diagnostics are offline only",
        caveats: "the shell never runs the doctor; it prints this command and the reasoning only",
    },
];

impl Explained {
    /// The stable identifier of this explained-only command.
    pub(in crate::shell) fn id(self) -> &'static str {
        match self {
            Explained::BackupVerify => "backup verify",
            Explained::BackupRestore => "backup restore",
            Explained::BackupSnapshot => "backup snapshot",
            Explained::ExportLogical => "export logical",
            Explained::ImportLogical => "import logical",
            Explained::DiagIntegrity => "diag integrity",
            Explained::VersionMigrate => "version migrate",
        }
    }

    /// The machine-readable payload the shell prints for this manual.
    pub(in crate::shell) fn payload(self) -> Result<serde_json::Value, ShellError> {
        let entry = EXPLAINED
            .iter()
            .find(|entry| entry.id == self.id())
            .ok_or(ShellError::Server)?;
        Ok(serde_json::json!({
            "manual": entry.command_lines(),
            "os_rights": entry.os_rights,
            "destructive_effects": entry.destructive_effects,
            "implementation_limits": entry.limits,
            "caveats": entry.caveats,
            "executed": false
        }))
    }
}
