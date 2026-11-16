use clap::Parser;

use crate::migrate::FormatMigrateArgs;

#[derive(Debug, Parser)]
struct TestCli {
    #[command(flatten)]
    migrate: FormatMigrateArgs,
}

#[test]
fn format_migrate_args_parse_all_five_flags_and_default_age() {
    let parsed = TestCli::try_parse_from([
        "shamir-server",
        "--plan",
        "--from-backup",
        "snapshot-dir",
        "--max-backup-age-hours",
        "7",
        "--yes",
    ])
    .expect("valid flattened FormatMigrateArgs");
    assert!(parsed.migrate.plan);
    assert_eq!(
        parsed.migrate.from_backup.as_deref(),
        Some(std::path::Path::new("snapshot-dir"))
    );
    assert_eq!(parsed.migrate.max_backup_age_hours, 7);
    assert!(!parsed.migrate.i_have_a_backup);
    assert!(parsed.migrate.yes);

    let parsed = TestCli::try_parse_from([
        "shamir-server",
        "--plan",
        "--max-backup-age-hours",
        "7",
        "--i-have-a-backup",
        "--yes",
    ])
    .expect("plan, age, override and confirmation parse together");
    assert!(parsed.migrate.plan);
    assert!(parsed.migrate.i_have_a_backup);
    assert!(parsed.migrate.yes);
    assert_eq!(parsed.migrate.max_backup_age_hours, 7);

    let parsed = TestCli::try_parse_from(["shamir-server", "--from-backup", "snapshot-dir"])
        .expect("backup source parses");
    assert_eq!(
        parsed.migrate.from_backup.as_deref(),
        Some(std::path::Path::new("snapshot-dir"))
    );
    assert_eq!(parsed.migrate.max_backup_age_hours, 24);
    assert!(!parsed.migrate.plan);
    assert!(!parsed.migrate.i_have_a_backup);
    assert!(!parsed.migrate.yes);

    let parsed = TestCli::try_parse_from(["shamir-server"]).expect("defaults parse");
    assert_eq!(parsed.migrate.max_backup_age_hours, 24);
    assert!(parsed.migrate.from_backup.is_none());
    assert!(!parsed.migrate.plan);
    assert!(!parsed.migrate.i_have_a_backup);
    assert!(!parsed.migrate.yes);
}

#[test]
fn format_migrate_args_usage_errors_and_backup_override_coexist() {
    let missing_value = TestCli::try_parse_from(["shamir-server", "--from-backup"])
        .expect_err("missing backup path is invalid");
    assert_eq!(missing_value.exit_code(), 2);

    let bad_value =
        TestCli::try_parse_from(["shamir-server", "--max-backup-age-hours", "not-hours"])
            .expect_err("non-integer backup age is invalid");
    assert_eq!(bad_value.exit_code(), 2);

    let parsed = TestCli::try_parse_from([
        "shamir-server",
        "--from-backup",
        "snapshot-dir",
        "--i-have-a-backup",
    ])
    .expect("backup verification and explicit override are both accepted");
    assert_eq!(
        parsed.migrate.from_backup.as_deref(),
        Some(std::path::Path::new("snapshot-dir"))
    );
    assert!(parsed.migrate.i_have_a_backup);
}
