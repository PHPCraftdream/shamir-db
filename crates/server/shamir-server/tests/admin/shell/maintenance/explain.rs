//! Explained-only maintenance manuals (design `admin-shell.md` §4.10):
//! the shell prints the server's own manual and never runs it.
//!
//! The five tests here cover:
//!
//! * the payload contract of every explained command — `kind: result`, no
//!   wire `op`, `data.executed == false`, the four operator texts and the
//!   EXACT printed command lines. The lines are HARD-CODED below: the
//!   production `EXPLAINED` table is `pub(in crate::shell)`, so this
//!   integration test cannot reach it and a golden literal is the only
//!   honest oracle;
//! * the NO-EFFECT proof: every regular file under the fixture data
//!   directory is hashed before and after all seven commands. The shell
//!   opens nothing for an explained command, so the two maps must be
//!   identical — a future implementation that "helpfully" opens the store
//!   fails here instead of silently mutating an operator's data;
//! * `backup schedule show` is the typed `unsupported_prerequisite`
//!   refusal, never a placeholder success;
//! * an explained command accepts NO options: `backup restore --yes` is
//!   refused at parse time with the typed `usage` code and exit 1;
//! * the PIN against the server's own argument parser.
//!
//! ## The pin, in two halves
//!
//! Every printed line must exist in the server binary, otherwise the manual
//! the operator reads drifts from the parser that would run it. The pin is
//! split by WHERE the parser is declared:
//!
//! * **real-parser pin** — `logical-export`, `logical-import` and
//!   `format-migrate` delegate to `clap::Args` structs owned by the server
//!   (`shamir_server::logical_dump::LogicalExportArgs`,
//!   `shamir_server::logical_import::LogicalImportArgs` and
//!   `shamir_server::migrate::FormatMigrateArgs`). Each manual's tokens
//!   (minus the binary name and the `--config CONFIG` placeholder, which
//!   belong to the top-level command, not to the subcommand's own args) are
//!   fed to a one-field `clap::Parser` wrapper carrying that REAL struct, so
//!   a renamed, moved or removed flag fails HERE, in the parser itself.
//!   The flags those manuals advertise BEYOND their command line
//!   (`--repo`, `--table`, `--schema-only`, `--as-db`,
//!   `--max-backup-age-hours`) are declared only on those same structs and
//!   are pinned the same way.
//! * **source pin** — `backup-verify`, `backup`, `restore` and `doctor` are
//!   declared INLINE in `src/main.rs` as `Subcmd` variants, not as reusable
//!   `clap::Args` types, so no test can instantiate them. Their tokens are
//!   pinned against the text of `src/main.rs` (read once): the `Subcmd`
//!   variant that declares the subcommand and the field declaration that
//!   makes clap derive each long flag (`--config`/`--from`/`--to`/`--db`/
//!   `--repo`/`--table`/`--apply`/`--pretty`/`--json`). For those flags the
//!   field declaration (`from: PathBuf`, `json: bool`) is the needle, since
//!   the literal `--flag` text never appears in derive-based source.
//!   `--max-backup-age-hours`, `--schema-only` and `--as-db` are NOT
//!   declared in `src/main.rs` at all — they live on the `Args` structs
//!   covered by the real-parser half, which is why they are pinned there.

// ── Fixtures (hard-coded goldens; see the module doc) ────────────────

/// One explained-only command: the argv tail typed after the shell's `--`
/// delimiter and the EXACT manual lines production prints for it.
struct ExplainedCase {
    /// Tokens typed after `--`.
    tail: &'static [&'static str],
    /// The `shamir-server ...` lines the payload must carry, in order.
    manual: &'static [&'static str],
}

/// The seven explained-only manuals, in production `EXPLAINED` order.
const EXPLAINED_CASES: &[ExplainedCase] = &[
    ExplainedCase {
        tail: &["backup", "verify", "SNAPSHOT"],
        manual: &["shamir-server backup-verify SNAPSHOT"],
    },
    ExplainedCase {
        tail: &["backup", "restore"],
        manual: &["shamir-server --config CONFIG restore --from SNAPSHOT"],
    },
    ExplainedCase {
        tail: &["backup", "snapshot"],
        manual: &["shamir-server --config CONFIG backup --to DIR"],
    },
    ExplainedCase {
        tail: &["export", "logical"],
        manual: &["shamir-server --config CONFIG logical-export --db DATABASE --output PATH"],
    },
    ExplainedCase {
        tail: &["import", "logical"],
        manual: &["shamir-server --config CONFIG logical-import --input PATH --target DIR"],
    },
    ExplainedCase {
        tail: &["diag", "integrity"],
        manual: &["shamir-server --config CONFIG doctor"],
    },
    ExplainedCase {
        tail: &["version", "migrate"],
        manual: &[
            "shamir-server --config CONFIG format-migrate --plan",
            "shamir-server --config CONFIG format-migrate --yes --from-backup SNAPSHOT",
        ],
    },
];

/// The single LF-terminated JSON Lines v1 record of one run.
fn record(run: &super::harness::ShellRun, case: &str) -> serde_json::Value {
    let line = run
        .stdout
        .strip_suffix(b"\n")
        .expect("explained command emits one LF-terminated record");
    assert!(
        !line.contains(&b'\n'),
        "expected exactly one record (`{case}`)"
    );
    serde_json::from_slice(line).expect("explained record is valid JSON")
}

/// A SUCCESS envelope: exit 0, then the parsed single record.
fn envelope(run: &super::harness::ShellRun, case: &str) -> serde_json::Value {
    super::harness::assert_exit(run, 0);
    record(run, case)
}

/// Hash every regular file below `root` into relative-path → hex digest.
fn hash_tree(root: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let mut hashes = std::collections::BTreeMap::new();
    collect_hashes(root, root, &mut hashes);
    hashes
}

/// Recursive half of [`hash_tree`]; directories are descended, regular
/// files are read whole and hashed, anything else (a symlink on a
/// platform that reports one) contributes no entry.
fn collect_hashes(
    root: &std::path::Path,
    dir: &std::path::Path,
    hashes: &mut std::collections::BTreeMap<String, String>,
) {
    for entry in std::fs::read_dir(dir).expect("read fixture data directory") {
        let entry = entry.expect("fixture data directory entry");
        let path = entry.path();
        let file_type = entry
            .file_type()
            .expect("fixture data directory entry type");
        if file_type.is_dir() {
            collect_hashes(root, &path, hashes);
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("data file below the data directory")
                .to_string_lossy()
                .replace('\\', "/");
            let digest = match std::fs::read(&path) {
                Ok(bytes) => hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)),
                // Windows: the running fixture server holds a byte-range lock
                // on some data files (os error 33, ERROR_LOCK_VIOLATION), so
                // they cannot be read. Such a file is pinned by its length:
                // weaker than a content hash, but the idle server does not
                // resize it and an explained command must not touch it.
                Err(error) if error.raw_os_error() == Some(33) => {
                    let length = std::fs::metadata(&path)
                        .expect("metadata of a locked fixture data file")
                        .len();
                    format!("locked:{length}")
                }
                Err(error) => panic!("read fixture data file: {error}"),
            };
            hashes.insert(relative, digest);
        }
    }
}

/// The server argument-parser source, read once: the inline `Subcmd`
/// declarations of `backup-verify`, `backup`, `restore` and `doctor`.
fn server_main_source() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))
        .expect("read the server argument parser source")
}

// ── The real-parser wrappers (one real `clap::Args` struct each) ─────

/// `logical-export`'s REAL argument struct, behind a one-field parser.
#[derive(clap::Parser)]
struct LogicalExportParser {
    #[command(flatten)]
    args: shamir_server::logical_dump::LogicalExportArgs,
}

/// `logical-import`'s REAL argument struct, behind a one-field parser.
#[derive(clap::Parser)]
struct LogicalImportParser {
    #[command(flatten)]
    args: shamir_server::logical_import::LogicalImportArgs,
}

/// `format-migrate`'s REAL argument struct, behind a one-field parser.
#[derive(clap::Parser)]
struct FormatMigrateParser {
    #[command(flatten)]
    args: shamir_server::migrate::FormatMigrateArgs,
}

/// Tokens printed by the four INLINE manuals and the `src/main.rs`
/// declaration that makes them parse: the `Subcmd` variant for a
/// subcommand, the field for a flag (clap derives the long form from it).
const MAIN_PARSER_DECLARATIONS: &[(&str, &str)] = &[
    ("backup-verify", "BackupVerify {"),
    ("backup", "Backup {"),
    ("restore", "Restore {"),
    ("doctor", "Doctor {"),
    ("--config", "config: PathBuf"),
    ("--from", "from: PathBuf"),
    ("--to", "to: PathBuf"),
    ("--db", "db: Option<String>"),
    ("--repo", "repo: Option<String>"),
    ("--table", "table: Option<String>"),
    ("--apply", "apply: bool"),
    ("--pretty", "pretty: bool"),
    ("--json", "json: bool"),
];

// ── Tests ────────────────────────────────────────────────────────────

/// Every explained-only command prints the documented sections: a result
/// record without a wire op, `executed: false`, a non-empty `manual` array
/// starting with the server binary, the four non-empty operator texts, and
/// the EXACT command lines from the module-level goldens.
#[test]
fn explained_commands_print_the_documented_sections() {
    let fixture = super::harness::boot("explained_commands_print_the_documented_sections");
    for case in EXPLAINED_CASES {
        let case_name = case.tail.join(" ");
        fixture.guard.mark(format!("explained: {case_name}"));
        let mut argv = vec!["--"];
        argv.extend_from_slice(case.tail);
        let run = super::harness::run_shell(&fixture, &argv);
        assert!(
            !run.contains_canary(fixture.canary.as_str()),
            "shell secret leaked (`{case_name}`)"
        );
        let value = envelope(&run, &case_name);
        assert_eq!(
            value["kind"].as_str(),
            Some("result"),
            "`{case_name}` is not a result record"
        );
        assert!(
            value["op"].is_null(),
            "a local manual must not carry a wire op (`{case_name}`)"
        );
        let data = &value["data"];
        assert_eq!(
            data["executed"].as_bool(),
            Some(false),
            "an explained command must report `executed: false` (`{case_name}`)"
        );
        let manual = data["manual"].as_array().expect("manual is a JSON array");
        assert!(
            !manual.is_empty(),
            "manual must not be empty (`{case_name}`)"
        );
        let first = manual[0].as_str().expect("manual line is a string");
        assert!(
            first.starts_with("shamir-server "),
            "the manual line must name the server binary (`{case_name}`)"
        );
        for field in [
            "os_rights",
            "destructive_effects",
            "implementation_limits",
            "caveats",
        ] {
            assert!(
                data[field].as_str().is_some_and(|text| !text.is_empty()),
                "`{field}` must be a non-empty string (`{case_name}`)"
            );
        }
        let printed: Vec<&str> = manual
            .iter()
            .map(|line| line.as_str().expect("manual line is a string"))
            .collect();
        assert_eq!(
            printed, case.manual,
            "printed manual lines drifted (`{case_name}`)"
        );
    }
}

/// The no-effect proof: the whole fixture data directory is content-hashed
/// before and after all seven explained commands, and the two maps must be
/// EQUAL. The shell opens nothing for these commands (no store, no
/// manifest, no config), so not one byte may change. On Windows a data
/// file that the running fixture server holds byte-range locked cannot be
/// read: such a file is pinned by its length instead.
#[test]
fn explained_commands_change_nothing_on_disk() {
    let fixture = super::harness::boot("explained_commands_change_nothing_on_disk");
    let data_dir = super::harness::secure_fixture::data_dir(&fixture).to_path_buf();
    let before = hash_tree(&data_dir);
    assert!(
        !before.is_empty(),
        "the fixture data directory must hold files to hash"
    );
    for case in EXPLAINED_CASES {
        let mut argv = vec!["--"];
        argv.extend_from_slice(case.tail);
        let run = super::harness::run_shell(&fixture, &argv);
        super::harness::assert_exit(&run, 0);
    }
    let after = hash_tree(&data_dir);
    assert_eq!(
        before, after,
        "an explained command changed the fixture data directory"
    );
}

/// `backup schedule show` is a capability this release does not carry: the
/// typed `unsupported_prerequisite` error (exit 6), never a placeholder
/// success record.
#[test]
fn schedule_show_is_unsupported_not_a_placeholder_success() {
    let fixture = super::harness::boot("schedule_show_is_unsupported_not_a_placeholder_success");
    let run = super::harness::run_shell(&fixture, &["--", "backup", "schedule", "show"]);
    super::harness::assert_exit(&run, 6);
    let value = record(&run, "backup schedule show");
    assert_eq!(
        value["kind"].as_str(),
        Some("error"),
        "an unsupported capability is an error record"
    );
    assert_eq!(
        value["code"].as_str(),
        Some("unsupported_prerequisite"),
        "the schedule view must refuse with the typed capability code"
    );
    assert!(
        value["data"].is_null(),
        "an error envelope never carries data"
    );
}

/// An explained command takes NO options: `backup restore --yes` is
/// rejected at PARSE time with the typed `usage` code and exit 1, so no
/// record is emitted (stdout stays empty) and nothing can be persuaded to
/// run the manual locally.
#[test]
fn explained_command_options_are_refused() {
    let fixture = super::harness::boot("explained_command_options_are_refused");
    let run = super::harness::run_shell(&fixture, &["--", "backup", "restore", "--yes"]);
    super::harness::assert_exit(&run, 1);
    assert!(
        run.stdout.is_empty(),
        "an option-refused explained command must not print a record"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("error: usage "),
        "the refusal must be the fixed typed `usage` line"
    );
    assert!(!stderr.contains("--yes"), "the refusal must not echo argv");
}

/// The PIN (module doc: which half is which and why).
///
/// Real-parser half: each of `logical-export`, `logical-import` and
/// `format-migrate` parses the EXACT tokens of its manual line (without the
/// binary name and without `--config CONFIG`, which belong to the top-level
/// command) through the server's REAL `clap::Args` struct; the flags those
/// manuals advertise beyond their command line (declared only on those
/// structs) are pinned by parsing them too.
///
/// Source half: `backup-verify`, `backup`, `restore`, `doctor` and the
/// `--config`/`--from`/`--to`/`--db`/`--repo`/`--table`/`--apply`/
/// `--pretty`/`--json` flags are declared inline in `src/main.rs`, read
/// once; each declaration the manual relies on must be present.
#[test]
fn printed_flags_exist_in_the_server_argument_parser() {
    // Real-parser pin: the three manuals whose parser is an `Args` struct.
    let export = <LogicalExportParser as clap::Parser>::try_parse_from([
        "logical-export",
        "--db",
        "DATABASE",
        "--output",
        "PATH",
    ])
    .expect("logical-export manual tokens parse with the server's real args");
    assert_eq!(export.args.db, "DATABASE");
    assert_eq!(export.args.output, std::path::PathBuf::from("PATH"));

    let import = <LogicalImportParser as clap::Parser>::try_parse_from([
        "logical-import",
        "--input",
        "PATH",
        "--target",
        "DIR",
    ])
    .expect("logical-import manual tokens parse with the server's real args");
    assert_eq!(import.args.input, std::path::PathBuf::from("PATH"));
    assert_eq!(import.args.target, std::path::PathBuf::from("DIR"));

    let plan = <FormatMigrateParser as clap::Parser>::try_parse_from(["format-migrate", "--plan"])
        .expect("the `format-migrate --plan` manual token parses with the real args");
    assert!(plan.args.plan, "`--plan` must be the read-only form");
    assert!(!plan.args.yes, "`--plan` must not apply the migration");

    let applying = <FormatMigrateParser as clap::Parser>::try_parse_from([
        "format-migrate",
        "--yes",
        "--from-backup",
        "SNAPSHOT",
    ])
    .expect("the applying format-migrate manual tokens parse with the real args");
    assert!(applying.args.yes, "`--yes` must be the applying form");
    assert_eq!(
        applying.args.from_backup,
        Some(std::path::PathBuf::from("SNAPSHOT"))
    );

    // Real-parser pin for the flags the manuals advertise BEYOND their
    // command lines: they exist only on these `Args` structs, so a parse is
    // the only honest proof that the promise is real.
    <LogicalExportParser as clap::Parser>::try_parse_from([
        "logical-export",
        "--db",
        "DATABASE",
        "--output",
        "PATH",
        "--repo",
        "REPO",
        "--table",
        "TABLE",
        "--schema-only",
    ])
    .expect("the flags `logical-export` advertises must exist on its real args");
    <LogicalImportParser as clap::Parser>::try_parse_from([
        "logical-import",
        "--input",
        "PATH",
        "--target",
        "DIR",
        "--as-db",
        "NAME",
        "--schema-only",
    ])
    .expect("the flags `logical-import` advertises must exist on its real args");
    <FormatMigrateParser as clap::Parser>::try_parse_from([
        "format-migrate",
        "--yes",
        "--from-backup",
        "SNAPSHOT",
        "--max-backup-age-hours",
        "24",
    ])
    .expect("the flag `format-migrate` advertises must exist on its real args");

    // Source pin: the inline `Subcmd` declarations, read once.
    let source = server_main_source();
    for &(token, declaration) in MAIN_PARSER_DECLARATIONS {
        assert!(
            source.contains(declaration),
            "a manual prints `{token}` but `{declaration}` is absent from src/main.rs"
        );
    }
}
