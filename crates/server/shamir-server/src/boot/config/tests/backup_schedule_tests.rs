use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;
use std::time::Duration;

use shamir_private_dir::create_private_dir;
use tempfile::TempDir;

use crate::config::{BackupRetentionConfig, BackupScheduleConfig, Config, ConfigError};

const EXAMPLE: &str = include_str!("../../../../../../../deploy/server.example.ktav");

fn backup(destination: PathBuf) -> BackupScheduleConfig {
    BackupScheduleConfig {
        destination,
        interval: "1m".into(),
        window: None,
        run_on_start: false,
        retention: BackupRetentionConfig::default(),
    }
}

fn private(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    create_private_dir(&path).unwrap();
    path
}

fn assert_invalid<T: std::fmt::Debug>(result: Result<T, ConfigError>, expected: &str) {
    match result {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains(expected),
                "expected {expected:?}: {message}"
            );
        }
        other => panic!("expected {expected:?} validation failure, got {other:?}"),
    }
}

#[test]
fn omitted_backup_is_disabled_and_enabled_block_has_defaults() {
    let original: Config = ktav::from_str(EXAMPLE).unwrap();
    assert!(original.backup.is_none());
    for retention in ["", "retention: {}\n"] {
        let text =
            format!("{EXAMPLE}\nbackup: {{\ndestination: backups\ninterval: 1m\n{retention}}}\n");
        let parsed: Config = ktav::from_str(&text).unwrap();
        let schedule = parsed.backup.unwrap();
        assert_eq!(schedule.destination, Path::new("backups"));
        assert_eq!(schedule.interval, "1m");
        assert!(schedule.window.is_none());
        assert!(!schedule.run_on_start);
        assert_eq!(schedule.retention.keep_last, 7);
        assert_eq!(schedule.retention.max_age_days, None);
    }
}

#[test]
fn ktav_accepts_custom_values_and_refuses_invalid_schema() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("server.ktav");
    let parse = |block: &str| {
        fs::write(&file, format!("{EXAMPLE}\nbackup: {{\n{block}\n}}\n")).unwrap();
        Config::from_file(&file)
    };
    let mut config = parse(
        "destination: copies\ninterval: 2h\nwindow: 22:00-06:00\nrun_on_start: true\nretention: {\nkeep_last: 3\nmax_age_days: 14\n}",
    )
    .unwrap();
    let schedule = config.backup.as_mut().unwrap();
    assert_eq!(schedule.destination, Path::new("copies"));
    assert_eq!(schedule.interval, "2h");
    assert_eq!(schedule.window.as_deref(), Some("22:00-06:00"));
    assert!(schedule.run_on_start);
    assert_eq!(schedule.retention.keep_last, 3);
    assert_eq!(schedule.retention.max_age_days, Some(14));
    schedule.destination = private(dir.path(), "copies");
    config.data_dir = private(dir.path(), "source");
    config.validate().unwrap();
    let resolved = config.backup.unwrap().resolve(&config.data_dir).unwrap();
    assert!(resolved.run_on_start);
    assert_eq!(resolved.interval, Duration::from_secs(7_200));
    assert!(resolved.window.unwrap().contains(0));
    assert_eq!(resolved.retention.keep_last, 3);
    assert_eq!(resolved.retention.max_age_days, Some(14));

    for missing in ["", "destination: copies", "interval: 1m"] {
        assert!(
            matches!(parse(missing), Err(ConfigError::Parse(_))),
            "{missing}"
        );
    }
    for extra in [
        "intervals: 5m",
        "run_on_start: maybe",
        "retention: {\nkeep_lats: 3\n}",
        "retention: {\nkeep_last: -1\n}",
        "retention: {\nmax_age_days: -1\n}",
        "retention: {\nmax_age_days: 4294967296\n}",
        "retention: {\nmax_age_days: 1.5\n}",
    ] {
        let block = format!("destination: copies\ninterval: 1m\n{extra}");
        assert!(
            matches!(parse(&block), Err(ConfigError::Parse(_))),
            "{extra}"
        );
    }
}

#[test]
fn intervals_require_one_ascii_integer_unit_and_representable_deadline() {
    let dir = TempDir::new().unwrap();
    let source = private(dir.path(), "source");
    let mut schedule = backup(private(dir.path(), "backups"));
    for (input, seconds) in [("60s", 60), ("1m", 60), ("2h", 7_200), ("1d", 86_400)] {
        schedule.interval = input.into();
        assert_eq!(
            schedule.resolve(&source).unwrap().interval,
            Duration::from_secs(seconds)
        );
    }
    for input in [
        "", "m", "60", "1M", "1w", "1h1m", "1.5h", "+1m", "-1m", "１m", " 1m", "1m ", "1 m",
    ] {
        schedule.interval = input.into();
        assert_invalid(schedule.resolve(&source), "whole ASCII number");
    }
    for input in ["0m", "59s"] {
        schedule.interval = input.into();
        assert_invalid(schedule.resolve(&source), "at least 60 seconds");
    }
    for input in [
        "18446744073709551616s",
        "18446744073709551615d",
        "18446744073709551615s",
    ] {
        schedule.interval = input.into();
        assert_invalid(schedule.resolve(&source), "interval is too large");
    }
}

#[test]
fn windows_are_strict_and_half_open_including_midnight() {
    let dir = TempDir::new().unwrap();
    let source = private(dir.path(), "source");
    let mut schedule = backup(private(dir.path(), "backups"));
    for input in [
        "",
        "1:00-02:00",
        "01:0-02:00",
        "01:00/02:00",
        "01.00-02:00",
        "01:00-02:00 ",
        "01:00-24:00",
        "24:00-01:00",
        "01:60-02:00",
        "01:00-02:60",
        "01:00-01:00",
        "０1:00-02:00",
    ] {
        schedule.window = Some(input.into());
        assert_invalid(schedule.resolve(&source), "backup.window");
    }
    schedule.window = Some("01:00-02:00".into());
    let window = schedule.resolve(&source).unwrap().window.unwrap();
    assert!(!window.contains(59));
    assert!(window.contains(60));
    assert!(window.contains(119));
    assert!(!window.contains(120));

    schedule.window = Some("22:00-06:00".into());
    let window = schedule.resolve(&source).unwrap().window.unwrap();
    assert!(!window.contains(1_319));
    assert!(window.contains(1_320));
    assert!(window.contains(1_439));
    assert!(window.contains(0));
    assert!(window.contains(359));
    assert!(!window.contains(360));
    assert!(!window.contains(1_440));
}

#[test]
fn retention_requires_positive_values_and_widens_days_without_overflow() {
    let dir = TempDir::new().unwrap();
    let source = private(dir.path(), "source");
    let mut schedule = backup(private(dir.path(), "backups"));
    schedule.retention.keep_last = 0;
    assert_invalid(schedule.resolve(&source), "backup.retention.keep_last");
    schedule.retention.keep_last = 1;
    schedule.retention.max_age_days = Some(0);
    assert_invalid(schedule.resolve(&source), "backup.retention.max_age_days");
    schedule.retention.max_age_days = Some(u32::MAX);
    assert_eq!(
        schedule.resolve(&source).unwrap().retention,
        schedule.retention
    );
}

#[test]
fn boot_validation_requires_existing_private_root_but_not_existing_source() {
    let dir = TempDir::new().unwrap();
    let mut config: Config = ktav::from_str(EXAMPLE).unwrap();
    config.data_dir = dir.path().join("source");
    let destination = private(dir.path(), "backups");
    config.backup = Some(backup(destination.clone()));
    config.validate().unwrap();
    assert!(!config.data_dir.exists());
    let schedule = config.backup.as_ref().unwrap();
    assert_invalid(
        schedule.resolve(&config.data_dir),
        "data_dir cannot be canonicalized",
    );
    fs::create_dir(&config.data_dir).unwrap();
    assert_eq!(
        schedule.resolve(&config.data_dir).unwrap().destination,
        destination.canonicalize().unwrap()
    );

    let missing = dir.path().join("missing");
    config.backup = Some(backup(missing.clone()));
    assert_invalid(config.validate(), "backup.destination must be private");
    assert!(!missing.exists());
    let file = dir.path().join("file");
    fs::write(&file, []).unwrap();
    config.backup = Some(backup(file));
    assert_invalid(config.validate(), "backup.destination must be private");
    let public = dir.path().join("public");
    fs::create_dir(&public).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    config.backup = Some(backup(public));
    assert_invalid(config.validate(), "backup.destination must be private");
}

#[test]
fn rejects_lexical_overlap_and_invalid_root_spellings() {
    let dir = TempDir::new().unwrap();
    let root = private(dir.path(), "backups");
    let drive_root = root.ancestors().last().unwrap();
    let mut config: Config = ktav::from_str(EXAMPLE).unwrap();
    config.data_dir = root.join("not_created");
    config.backup = Some(backup(root.clone()));
    assert_invalid(config.validate(), "must be disjoint");
    config.data_dir.clone_from(&root);
    for path in [root.clone(), root.join("nested")] {
        config.backup = Some(backup(path));
        assert_invalid(config.validate(), "must be disjoint");
    }
    for path in [
        PathBuf::from("relative"),
        drive_root.to_path_buf(),
        drive_root.join("."),
        root.join("../backups"),
    ] {
        config.backup = Some(backup(path));
        assert_invalid(config.validate(), "absolute non-root path");
    }
}

#[cfg(unix)]
fn link_directory(target: &Path, link: &Path) {
    symlink(target, link).unwrap();
}

#[cfg(windows)]
fn link_directory(target: &Path, link: &Path) {
    let output = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn refuses_link_roots_and_canonical_overlap_via_alias() {
    let dir = TempDir::new().unwrap();
    let root = private(dir.path(), "backups");
    let nested = private(&root, "nested");
    let alias = dir.path().join("alias");
    link_directory(&root, &alias);
    let mut config: Config = ktav::from_str(EXAMPLE).unwrap();
    config.data_dir = dir.path().join("source");
    for path in [alias.clone(), alias.join(""), alias.join(".")] {
        config.backup = Some(backup(path));
        assert_invalid(config.validate(), "backup.destination must be private");
    }

    for (destination, source) in [
        (&root, alias.clone()),
        (&root, alias.join("nested")),
        (&nested, alias),
    ] {
        config.data_dir = source;
        config.backup = Some(backup(destination.clone()));
        config.validate().unwrap();
        assert_invalid(
            config.backup.as_ref().unwrap().resolve(&config.data_dir),
            "must be disjoint",
        );
    }
}
