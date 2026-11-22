use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use shamir_sync::format_stamp::migration::{
    chain_for, run_migration, FormatStep, MigrationError, ModuleDescriptor, PlanLine, StepContext,
};
use shamir_sync::format_stamp::EpochPolicy;

use super::backup_gate::{assess_backup, VerifySnapshot};
use super::cli::FormatMigrateArgs;
use super::integrity::check_wire_tables_registry;
use super::plan::{configured_log_name, inspect_preflight, render_plan, Preflight};

use crate::backup::{verify_exported_manifest, verify_manifest};

/// Run the production format-migration command against a validated server configuration.
pub fn run(config: &crate::config::Config, args: &FormatMigrateArgs) -> Result<()> {
    run_with(
        config,
        args,
        EpochPolicy::compiled(),
        shamir_sync::format_stamp::migration::FORMAT_MODULES,
        &check_wire_tables_registry,
        io::stdout(),
        &mut io::stderr(),
    )
}

/// Plain policy, module and integrity injection used by command tests; production calls `run`.
pub(crate) fn run_with<W: Write + Send + 'static>(
    config: &crate::config::Config,
    args: &FormatMigrateArgs,
    policy: EpochPolicy,
    modules: &[&dyn FormatStep],
    integrity: &dyn Fn(&Path) -> Result<(), MigrationError>,
    mut stdout: W,
    stderr: &mut dyn Write,
) -> Result<()> {
    let data_dir = &config.data_dir;
    let configured_log = configured_log_name(data_dir, config.logging.file.as_deref())?;
    let log_name = configured_log.as_deref();
    let initial = inspect_preflight(data_dir, policy, log_name)?;

    if !initial.needs_migration {
        writeln!(stdout, "format migration: nothing to do")?;
        writeln!(
            stdout,
            "stamp: {} (epoch {})",
            initial.state,
            initial.epoch.unwrap_or(policy.current)
        )?;
        writeln!(stdout, "journal: not consulted")?;
        writeln!(stdout, "backup gate: not required")?;
        writeln!(
            stdout,
            "integrity scope: wire_tables MessagePack decode only; not table/index/WAL health"
        )?;
        return Ok(());
    }

    if args.yes && initial.adopted {
        writeln!(stderr, "WARNING: this format migration uses an adopted stamp; original epoch evidence is weaker than a created stamp.")?;
    }
    if args.yes && initial.stampless_data {
        writeln!(stderr, "WARNING: this format migration uses stampless data treated as base epoch {}; the final stamp will record this binary, not an adoption.", policy.base)?;
    }

    if !args.yes || args.plan {
        // The plan gate must take no lock and write nothing (review finding
        // 4): a managed timestamped source refuses instead of being locked.
        let backup_verdict =
            backup_verdict(data_dir, args, &initial, log_name, verify_exported_manifest);
        if args.i_have_a_backup {
            writeln!(stderr, "WARNING: --i-have-a-backup bypasses manifest, identity, and freshness verification; proceed only after independently confirming a usable pre-migration backup.")?;
        }
        render_plan(
            data_dir,
            policy,
            modules,
            &initial,
            &backup_verdict,
            args.yes,
            &mut stdout,
        )?;
        if initial.adopted && !args.yes {
            writeln!(stderr, "WARNING: this data directory has an adopted format stamp; operator confirmation with --yes is required.")?;
            anyhow::bail!("adopted older format stamp requires --yes; no format migration was run");
        }
        if initial.stampless_data {
            writeln!(stderr, "WARNING: stampless older-base format data requires --yes and cannot pass the default copied-stamp backup gate.")?;
        }
        return Ok(());
    }

    if args.i_have_a_backup {
        writeln!(stderr, "WARNING: --i-have-a-backup bypasses manifest, identity, and freshness verification; proceed only after independently confirming a usable pre-migration backup.")?;
    }
    let lock_result = crate::maintenance_lock::acquire(data_dir);
    let _lock = lock_result.with_context(|| {
        format!(
            "could not acquire maintenance lock for {}; stop the server first",
            data_dir.display()
        )
    })?;

    // A second read-only check closes the preflight/lock race before the gate or chain is used.
    let locked_preflight = inspect_preflight(data_dir, policy, log_name)?;
    if !locked_preflight.needs_migration {
        writeln!(stdout, "format migration: nothing to do (another process completed it before the lock was acquired)")?;
        return Ok(());
    }

    if (locked_preflight.adopted || locked_preflight.stampless_data) && !args.yes {
        anyhow::bail!("format-migration source state changed before locking and now requires operator confirmation with --yes");
    }
    if locked_preflight.adopted && !initial.adopted {
        writeln!(stderr, "WARNING: the locked data directory now has an adopted format stamp; original epoch evidence is weaker than a created stamp.")?;
    }
    if locked_preflight.stampless_data && !initial.stampless_data {
        writeln!(stderr, "WARNING: the locked directory is now stampless and treated as base epoch {}; its final stamp will record this binary, not an adoption.", policy.base)?;
    }

    let locked_backup_verdict =
        backup_verdict(data_dir, args, &locked_preflight, log_name, verify_manifest);
    if !locked_backup_verdict.starts_with("PASS:")
        && !locked_backup_verdict.starts_with("OVERRIDE:")
    {
        anyhow::bail!("format migration backup gate refused: {locked_backup_verdict}; run `backup --to <dir>` first");
    }
    let from_epoch = locked_preflight.epoch.unwrap_or(policy.base);
    let chain = chain_for(EpochPolicy::new(policy.current, from_epoch), modules)
        .context("resolve format-migration chain")?;
    if chain.is_empty() {
        anyhow::bail!("format-migration chain is empty for an older epoch; refusing to write a stamp without a step");
    }

    // FormatStep requires Sync; this offline serial runner only locks for progress writes, so there is no hot-path contention.
    let output = Arc::new(Mutex::new(stdout));
    let progress_steps: Vec<_> = chain
        .iter()
        .enumerate()
        .map(|(index, step)| ProgressStep {
            inner: *step,
            index: index + 1,
            total: chain.len(),
            output: Arc::clone(&output),
        })
        .collect();
    let progress_modules: Vec<&dyn FormatStep> = progress_steps
        .iter()
        .map(|step| step as &dyn FormatStep)
        .collect();

    let outcome = run_migration(
        data_dir,
        policy,
        &progress_modules,
        env!("CARGO_PKG_VERSION"),
        integrity,
    )?;

    let mut writer = output
        .lock()
        .map_err(|_| anyhow::anyhow!("format-migration progress output lock was poisoned"))?;
    writeln!(
        writer,
        "format migration complete: epoch {} (last module: {})",
        outcome.final_epoch,
        outcome.last_module_id.as_deref().unwrap_or("none")
    )?;
    writeln!(writer, "backup gate: {locked_backup_verdict}")?;
    writeln!(writer, "integrity scope: wire_tables MessagePack decode only; table/index/WAL health was not checked; run stopped-server doctor after successful format migration")?;
    Ok(())
}

fn backup_verdict(
    data_dir: &Path,
    args: &FormatMigrateArgs,
    preflight: &Preflight,
    log_name: Option<&OsStr>,
    verify: VerifySnapshot,
) -> String {
    if args.i_have_a_backup {
        return "OVERRIDE: operator assertion (--i-have-a-backup); not independently verified"
            .to_owned();
    }
    let Some(backup_dir) = args.from_backup.as_deref() else {
        return "MISSING: run `backup --to <dir>` first, then provide --from-backup <dir>, or explicitly use --i-have-a-backup".to_owned();
    };
    match assess_backup(
        data_dir,
        backup_dir,
        preflight.stamp.as_ref(),
        args.max_backup_age_hours,
        log_name.map(Path::new),
        verify,
    ) {
        Ok(evidence) => format!(
            "PASS: verified {} manifest file(s), fresh and matching pre-migration stamp",
            evidence.files_checked
        ),
        Err(reason) => format!("FAIL: {reason}"),
    }
}

struct ProgressStep<'a, W> {
    inner: &'a dyn FormatStep,
    index: usize,
    total: usize,
    output: Arc<Mutex<W>>,
}

impl<W: Write + Send> FormatStep for ProgressStep<'_, W> {
    fn descriptor(&self) -> &ModuleDescriptor {
        self.inner.descriptor()
    }

    fn apply(&self, context: &mut StepContext<'_>) -> Result<(), MigrationError> {
        let start = Instant::now();
        self.inner.apply(context)?;
        let elapsed = start.elapsed();
        let descriptor = self.inner.descriptor();
        let mut writer = self.output.lock().map_err(|_| MigrationError::Io {
            dir: context.data_dir.to_path_buf(),
            source: io::Error::other("format-migration progress output lock was poisoned"),
        })?;
        writeln!(
            writer,
            "[{}/{}] {} {} … ok ({:.3}s)",
            self.index,
            self.total,
            descriptor.id,
            descriptor.summary,
            elapsed.as_secs_f64()
        )
        .map_err(|source| MigrationError::Io {
            dir: context.data_dir.to_path_buf(),
            source,
        })
    }

    fn plan(&self, context: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        self.inner.plan(context)
    }
}
