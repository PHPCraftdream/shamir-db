use std::fs;

use shamir_sync::format_stamp::migration::{JournalRecord, JournalWriter};

use super::support::{args, config, invoke, older_data_dir, temp, DirSnapshot};
use crate::migrate::tests::support::write_epoch;

#[test]
fn default_plan_mode_is_read_only_and_does_not_create_lock_or_journal() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let before = DirSnapshot::take(&data);

    let result = invoke(&config(&data), &args(false, false, None, false));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(result.stdout.contains("format migration plan"));
    assert!(result.stdout.contains("chain: 1 -> 2"));
    assert!(result.stdout.contains("e0001_0002: empty test step"));
    assert!(result
        .stdout
        .contains("lock probe: not attempted (plan mode)"));
    assert!(result.stdout.contains("execution requires --yes"));
    assert!(result.stdout.contains("backup gate: MISSING:"));
    before.assert_unchanged(&data);
    assert!(!data.join(".shamir.lock").exists());
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn explicit_plan_with_yes_stays_read_only_and_preserves_existing_lock_bytes() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    fs::write(data.join(".shamir.lock"), b"preexisting diagnostic").unwrap();
    let before = DirSnapshot::take(&data);

    let result = invoke(&config(&data), &args(true, true, None, false));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(result.stdout.contains("format migration plan"));
    assert!(result.stdout.contains("remaining refusals:"));
    assert!(result.stdout.contains("backup gate is not satisfied"));
    before.assert_unchanged(&data);
    assert_eq!(
        fs::read(data.join(".shamir.lock")).unwrap(),
        b"preexisting diagnostic"
    );
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn plan_succeeds_while_another_owner_holds_the_maintenance_lock() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let _held = crate::maintenance_lock::acquire(&data).expect("hold maintenance lock");
    let before = DirSnapshot::take(&data);

    let result = invoke(&config(&data), &args(false, true, None, false));

    assert!(
        result.result.is_ok(),
        "plan tried to acquire held lock: {}",
        result.stderr
    );
    assert!(result
        .stdout
        .contains("lock probe: not attempted (plan mode)"));
    before.assert_unchanged(&data);
    assert!(!data.join("format_migration.journal").exists());
}

#[test]
fn current_noop_does_not_lock_or_read_bad_journal_or_backup_gate() {
    let root = temp();
    let data = root.path().join("data");
    write_epoch(&data, 2);
    fs::write(data.join("payload.bin"), b"current data").unwrap();
    fs::write(
        data.join("format_migration.journal"),
        b"not a valid journal\n",
    )
    .unwrap();
    let _held = crate::maintenance_lock::acquire(&data).expect("hold maintenance lock");
    let before = DirSnapshot::take(&data);
    let config = config(&data);
    let result = invoke(
        &config,
        &args(true, false, Some(root.path().join("missing-backup")), false),
    );

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert_eq!(super::support::mapping_exit_code(&result.result), 0);
    assert!(result.stdout.contains("nothing to do"));
    assert!(result.stdout.contains("journal: not consulted"));
    assert!(result.stdout.contains("backup gate: not required"));
    assert!(!result.stdout.contains("missing-backup"));
    before.assert_unchanged(&data);
}

#[test]
fn plan_resumes_nonempty_journal_with_started_step_at_cursor_zero() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    write_started_journal(&data);
    let before = DirSnapshot::take(&data);

    let result = invoke(&config(&data), &args(false, true, None, false));

    assert!(result.result.is_ok(), "{}", result.stderr);
    assert!(result
        .stdout
        .contains("journal: resume cursor 0/1; next e0001_0002"));
    before.assert_unchanged(&data);
    assert!(!data.join(".shamir.lock").exists());
}

fn write_started_journal(data: &std::path::Path) {
    let writer = JournalWriter::create(data).expect("create real format-migration journal");
    let modules = vec!["e0001_0002".to_owned()];
    writer
        .append(&JournalRecord::ChainStarted {
            from_epoch: 1,
            to_epoch: 2,
            binary_version: env!("CARGO_PKG_VERSION").to_owned(),
            modules,
            unix_ms: super::support::FIXED_UNIX_MS,
        })
        .expect("append chain start");
    writer
        .append(&JournalRecord::StepStarted {
            module: "e0001_0002".to_owned(),
            from_epoch: 1,
            to_epoch: 2,
            unix_ms: super::support::FIXED_UNIX_MS,
        })
        .expect("append step start");
}

#[test]
fn configured_log_file_inside_data_dir_is_unchanged_by_yes_plan_mode() {
    let root = temp();
    let data = root.path().join("data");
    older_data_dir(&data);
    let log = data.join("server.log");
    fs::write(&log, b"existing log bytes").unwrap();
    let mut configured = config(&data);
    configured.logging.file = Some(log.to_string_lossy().into_owned());
    let before = DirSnapshot::take(&data);

    let result = invoke(&configured, &args(true, true, None, true));

    assert!(
        result.result.is_ok(),
        "configured log plan failed: {}",
        result.stderr
    );
    assert!(result.stdout.contains("format migration plan"));
    assert!(result.stdout.contains("backup gate: OVERRIDE:"));
    before.assert_unchanged(&data);
    assert_eq!(fs::read(log).unwrap(), b"existing log bytes");
    assert!(!data.join(".shamir.lock").exists());
    assert!(!data.join("format_migration.journal").exists());
}
