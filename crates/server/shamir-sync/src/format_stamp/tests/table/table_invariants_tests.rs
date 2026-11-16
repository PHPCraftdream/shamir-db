//! Decision-table invariants: totality, refusal stability, inspect
//! mode, D6 rewrite rule and the epoch/change-log invariants (design
//! 3.4, 4.1, 6.2).

use crate::format_stamp::check::{check_data_dir, StampMode, StampOutcome};
use crate::format_stamp::codec::{decode, STAMP_FILE_NAME};
use crate::format_stamp::io::{write_stamp, IgnoreSet};
use crate::format_stamp::migration::FORMAT_CHANGES;
use crate::format_stamp::model::{
    CorruptKind, EpochPolicy, FormatStamp, StampError, BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH,
};
use std::fs;

use crate::format_stamp::tests::support::stamp_fixture::{
    add_data, add_lock, policy, temp_dir, write_stamp_with_epoch, DirSnapshot, FIXTURE_UNIX_MS,
    OTHER_VERSION,
};

fn this_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[test]
fn table_cross_product_is_total() {
    // Full 3 x 2 x 3 Writer-mode cross product of design 3.4. Rows 1-2
    // are impossible as comparisons and behave as row 3; the epoch cells
    // of absent/corrupt rows are N/A and behave per the row that carries
    // the real behaviour (3, 5a/5b, 15, 18).
    enum Expected {
        Outcome(StampOutcome),
        OlderEpoch,
        NewerEpoch,
        AbsentOlderBase,
        CorruptShortFile,
    }
    // Fixture stamp epoch is 1 and its created_by version differs from
    // this binary, so an equal-epoch Writer run rewrites (D6).
    let cases: &[(&str, &str, EpochPolicy, Expected)] = &[
        // Rows 1-2 -> row 3: an absent stamp has no epoch to compare, so
        // "newer" is meaningless; policy(1, 1) hits row 3 for every cell.
        (
            "absent",
            "empty/older",
            policy(3, 1),
            Expected::Outcome(StampOutcome::Created),
        ),
        (
            "absent",
            "empty/equal",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Created),
        ),
        (
            "absent",
            "empty/newer",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Created),
        ),
        // Row 4 splits by BASE vs cur, not by a stamp epoch. "older" picks
        // policy(3, 1) so BASE < cur hits 5b; "equal"/"newer" pick
        // policy(1, 1) so BASE == cur hits 5a (row 5c is impossible).
        (
            "absent",
            "data/older",
            policy(3, 1),
            Expected::AbsentOlderBase,
        ),
        (
            "absent",
            "data/equal",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Adopted),
        ),
        (
            "absent",
            "data/newer",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Adopted),
        ),
        // Rows 7-12: the stamp carries epoch 1, so the relation is real.
        ("present", "empty/older", policy(3, 1), Expected::OlderEpoch),
        (
            "present",
            "empty/equal",
            policy(1, 1),
            Expected::Outcome(StampOutcome::VersionUpdated),
        ),
        ("present", "empty/newer", policy(0, 1), Expected::NewerEpoch),
        ("present", "data/older", policy(3, 1), Expected::OlderEpoch),
        (
            "present",
            "data/equal",
            policy(1, 1),
            Expected::Outcome(StampOutcome::VersionUpdated),
        ),
        ("present", "data/newer", policy(0, 1), Expected::NewerEpoch),
        // Rows 13-14 -> 15, 16-17 -> 18: a corrupt stamp has no trusted
        // epoch, so every cell behaves as its carrier row; policy(1, 1)
        // keeps the row-15 replacement writable (current >= BASE).
        (
            "corrupt",
            "empty/older",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Created),
        ),
        (
            "corrupt",
            "empty/equal",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Created),
        ),
        (
            "corrupt",
            "empty/newer",
            policy(1, 1),
            Expected::Outcome(StampOutcome::Created),
        ),
        (
            "corrupt",
            "data/older",
            policy(1, 1),
            Expected::CorruptShortFile,
        ),
        (
            "corrupt",
            "data/equal",
            policy(1, 1),
            Expected::CorruptShortFile,
        ),
        (
            "corrupt",
            "data/newer",
            policy(1, 1),
            Expected::CorruptShortFile,
        ),
    ];
    assert_eq!(cases.len(), 18);
    for (stamp_state, cell, pol, expected) in cases {
        let dir = temp_dir();
        add_lock(dir.path());
        if cell.starts_with("data") {
            add_data(dir.path());
        }
        match *stamp_state {
            "absent" => {}
            "present" => write_stamp_with_epoch(dir.path(), 1),
            _ => fs::write(dir.path().join(STAMP_FILE_NAME), b"broken").expect("corrupt"),
        }
        let outcome = check_data_dir(dir.path(), StampMode::Writer, &IgnoreSet::none(), pol);
        let label = format!("{stamp_state}/{cell}");
        match expected {
            Expected::Outcome(want) => assert_eq!(outcome.expect(&label), *want, "{label}"),
            Expected::OlderEpoch => {
                assert!(
                    matches!(outcome, Err(StampError::OlderEpoch { .. })),
                    "{label}"
                )
            }
            Expected::NewerEpoch => {
                assert!(
                    matches!(outcome, Err(StampError::NewerEpoch { .. })),
                    "{label}"
                )
            }
            Expected::AbsentOlderBase => {
                assert!(
                    matches!(outcome, Err(StampError::AbsentOlderBase { .. })),
                    "{label}"
                )
            }
            Expected::CorruptShortFile => assert!(
                matches!(
                    outcome,
                    Err(StampError::Corrupt {
                        kind: CorruptKind::ShortFile,
                        ..
                    })
                ),
                "{label}"
            ),
        }
    }
}

#[test]
fn older_epoch_not_modified_without_a_command() {
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    let before = DirSnapshot::take(dir.path());
    for _ in 0..3 {
        let err = check_data_dir(
            dir.path(),
            StampMode::Writer,
            &IgnoreSet::none(),
            &policy(2, 1),
        )
        .expect_err("older epoch refused");
        assert!(matches!(err, StampError::OlderEpoch { .. }), "got {err}");
    }
    before.assert_unchanged(dir.path());
}

#[test]
fn inspect_mode_never_writes() {
    // Stampless data dir.
    let dir = temp_dir();
    add_lock(dir.path());
    add_data(dir.path());
    let before = DirSnapshot::take(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Inspect,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("stampless inspect allows");
    assert_eq!(outcome, StampOutcome::InspectedStampless);
    before.assert_unchanged(dir.path());

    // Stampless empty dir.
    let dir = temp_dir();
    add_lock(dir.path());
    let before = DirSnapshot::take(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Inspect,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("empty inspect allows");
    assert_eq!(outcome, StampOutcome::InspectedStampless);
    before.assert_unchanged(dir.path());

    // Older epoch (observed, not refused, by Inspect mode).
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 1);
    let before = DirSnapshot::take(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Inspect,
        &IgnoreSet::none(),
        &policy(2, 1),
    )
    .expect("older inspect observes");
    assert_eq!(outcome, StampOutcome::InspectedOlder);
    before.assert_unchanged(dir.path());

    // Newer epoch: inspect refuses and writes nothing.
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), 5);
    let before = DirSnapshot::take(dir.path());
    let err = check_data_dir(
        dir.path(),
        StampMode::Inspect,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect_err("newer epoch refused");
    assert!(matches!(err, StampError::NewerEpoch { .. }), "got {err}");
    before.assert_unchanged(dir.path());
}

#[test]
fn last_opened_by_not_rewritten_when_version_unchanged() {
    let dir = temp_dir();
    add_lock(dir.path());
    let stamp = FormatStamp::with_epoch(CURRENT_FORMAT_EPOCH, this_version(), FIXTURE_UNIX_MS);
    write_stamp(dir.path(), &stamp).expect("seed stamp");
    let before = DirSnapshot::take(dir.path());
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("unchanged");
    assert_eq!(outcome, StampOutcome::Unchanged);
    // Stamp file mtime and bytes unchanged.
    before.assert_unchanged(dir.path());
}

#[test]
fn last_opened_by_rewritten_when_version_differs() {
    let dir = temp_dir();
    add_lock(dir.path());
    write_stamp_with_epoch(dir.path(), CURRENT_FORMAT_EPOCH);
    let outcome = check_data_dir(
        dir.path(),
        StampMode::Writer,
        &IgnoreSet::none(),
        &policy(1, 1),
    )
    .expect("rewritten");
    assert_eq!(outcome, StampOutcome::VersionUpdated);
    let bytes = fs::read(dir.path().join(STAMP_FILE_NAME)).expect("read stamp");
    let stamp = decode(&bytes, dir.path()).expect("decode");
    let actor = stamp.last_opened_by().expect("last_opened_by");
    assert_eq!(actor.version, this_version());
    assert_eq!(stamp.created_by().version, OTHER_VERSION);
}

#[test]
fn epoch_invariants_hold() {
    // Design 4.1: BASE <= CURRENT. The current epoch is taken from a real
    // stamp so the assertion is runtime, not a constant comparison.
    let current = FormatStamp::new(this_version(), FIXTURE_UNIX_MS).format_epoch();
    assert!(BASE_FORMAT_EPOCH <= current);
    let bumps = FORMAT_CHANGES
        .iter()
        .filter(|change| change.bumps_epoch)
        .count() as u64;
    assert_eq!(
        CURRENT_FORMAT_EPOCH,
        BASE_FORMAT_EPOCH + bumps,
        "a bump without a log entry, or an entry without a bump, fails here"
    );
    let mut ids: Vec<_> = FORMAT_CHANGES.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    let unique = ids.windows(2).all(|pair| pair[0] != pair[1]);
    assert!(unique, "change-log ids must be unique");
}
