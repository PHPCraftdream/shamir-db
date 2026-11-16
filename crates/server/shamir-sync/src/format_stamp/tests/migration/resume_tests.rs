use crate::format_stamp::migration::{resume_plan, JournalRecord, ResumePlan};

fn modules() -> Vec<String> {
    vec!["e0001_0002".to_owned()]
}

fn chain_started(
    modules: Vec<String>,
    from_epoch: u64,
    to_epoch: u64,
    binary_version: &str,
) -> JournalRecord {
    JournalRecord::ChainStarted {
        from_epoch,
        to_epoch,
        binary_version: binary_version.to_owned(),
        modules,
        unix_ms: 0,
    }
}

fn step_started(module: &str, from_epoch: u64, to_epoch: u64) -> JournalRecord {
    JournalRecord::StepStarted {
        module: module.to_owned(),
        from_epoch,
        to_epoch,
        unix_ms: 0,
    }
}

fn step_finished(module: &str) -> JournalRecord {
    JournalRecord::StepFinished {
        module: module.to_owned(),
        unix_ms: 0,
    }
}

fn plan(records: &[JournalRecord]) -> ResumePlan {
    resume_plan(records, &modules(), 1, 2, "0.1.0")
}

#[test]
fn resume_empty_records_are_fresh() {
    assert_eq!(plan(&[]), ResumePlan::Fresh);
}

#[test]
fn resume_step_started_without_chain_is_mismatch() {
    assert_eq!(
        plan(&[step_started("e0001_0002", 1, 2)]),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_matching_chain_starts_at_zero() {
    assert_eq!(
        plan(&[chain_started(modules(), 1, 2, "0.1.0")]),
        ResumePlan::Resume {
            next_module_index: 0
        }
    );
}

#[test]
fn resume_started_step_retries_that_step() {
    assert_eq!(
        plan(&[
            chain_started(modules(), 1, 2, "0.1.0"),
            step_started("e0001_0002", 1, 2),
        ]),
        ResumePlan::Resume {
            next_module_index: 0
        }
    );
}

#[test]
fn resume_finished_step_advances_cursor() {
    assert_eq!(
        plan(&[
            chain_started(modules(), 1, 2, "0.1.0"),
            step_started("e0001_0002", 1, 2),
            step_finished("e0001_0002"),
        ]),
        ResumePlan::Resume {
            next_module_index: 1
        }
    );
}

#[test]
fn resume_inside_last_step_retries_last_step() {
    let expected = vec!["a".to_owned(), "b".to_owned()];
    let records = vec![
        chain_started(expected.clone(), 1, 3, "0.1.0"),
        step_started("a", 1, 2),
        step_finished("a"),
        step_started("b", 2, 3),
    ];
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 1
        }
    );
}

#[test]
fn resume_two_finished_steps_advances_to_end() {
    let expected = vec!["a".to_owned(), "b".to_owned()];
    let records = vec![
        chain_started(expected.clone(), 1, 3, "0.1.0"),
        step_started("a", 1, 2),
        step_finished("a"),
        step_started("b", 2, 3),
        step_finished("b"),
    ];
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 2
        }
    );
}

#[test]
fn resume_after_chain_finished_remains_at_end() {
    let expected = vec!["a".to_owned(), "b".to_owned()];
    let records = vec![
        chain_started(expected.clone(), 1, 3, "0.1.0"),
        step_started("a", 1, 2),
        step_finished("a"),
        step_started("b", 2, 3),
        step_finished("b"),
        JournalRecord::ChainFinished {
            to_epoch: 3,
            unix_ms: 0,
        },
    ];
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 2
        }
    );
}

#[test]
fn resume_reuses_first_matching_chain_start() {
    let expected = modules();
    assert_eq!(
        resume_plan(
            &[
                chain_started(expected.clone(), 1, 2, "0.1.0"),
                chain_started(expected.clone(), 1, 2, "0.1.0"),
            ],
            &expected,
            1,
            2,
            "0.1.0"
        ),
        ResumePlan::Resume {
            next_module_index: 0
        }
    );
}

#[test]
fn resume_rejects_different_second_module_list() {
    let expected = modules();
    assert_eq!(
        resume_plan(
            &[
                chain_started(expected.clone(), 1, 2, "0.1.0"),
                chain_started(vec!["foreign".to_owned()], 1, 2, "0.1.0"),
            ],
            &expected,
            1,
            2,
            "0.1.0"
        ),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_wrong_from_epoch() {
    assert_eq!(
        plan(&[chain_started(modules(), 3, 2, "0.1.0")]),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_wrong_to_epoch() {
    assert_eq!(
        plan(&[chain_started(modules(), 1, 3, "0.1.0")]),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_wrong_binary_version() {
    assert_eq!(
        plan(&[chain_started(modules(), 1, 2, "9.9.9")]),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_foreign_module_list() {
    assert_eq!(
        plan(&[chain_started(vec!["foreign".to_owned()], 1, 2, "0.1.0")]),
        ResumePlan::Mismatch
    );
}

fn two_module_chain() -> (Vec<String>, Vec<JournalRecord>) {
    let expected = vec!["a".to_owned(), "b".to_owned()];
    let records = vec![chain_started(expected.clone(), 1, 3, "0.1.0")];
    (expected, records)
}

fn assert_two_module_mismatch(records: Vec<JournalRecord>) {
    let (expected, _) = two_module_chain();
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_chain_epoch_range_that_does_not_match_module_count() {
    let expected = vec!["a".to_owned(), "b".to_owned()];
    assert_eq!(
        resume_plan(
            &[chain_started(expected.clone(), 1, 4, "0.1.0")],
            &expected,
            1,
            4,
            "0.1.0"
        ),
        ResumePlan::Mismatch
    );
    assert_eq!(resume_plan(&[], &[], 2, 2, "0.1.0"), ResumePlan::Fresh);
    assert_eq!(resume_plan(&[], &[], 2, 3, "0.1.0"), ResumePlan::Mismatch);
}

#[test]
fn resume_rejects_skipped_or_unknown_step_start() {
    let (_, mut records) = two_module_chain();
    records.push(step_started("b", 2, 3));
    assert_two_module_mismatch(records);

    let (_, mut records) = two_module_chain();
    records.push(step_started("unknown", 1, 2));
    assert_two_module_mismatch(records);
}

#[test]
fn resume_rejects_out_of_order_or_unknown_step_finish() {
    let (_, mut records) = two_module_chain();
    records.push(step_started("a", 1, 2));
    records.push(step_finished("b"));
    assert_two_module_mismatch(records);

    let (_, mut records) = two_module_chain();
    records.push(step_finished("a"));
    assert_two_module_mismatch(records);
}

#[test]
fn resume_rejects_incorrect_step_epochs() {
    let (_, mut records) = two_module_chain();
    records.push(step_started("a", 2, 3));
    assert_two_module_mismatch(records);

    let (_, mut records) = two_module_chain();
    records.push(step_started("a", 1, 3));
    assert_two_module_mismatch(records);
}

#[test]
fn resume_rejects_foreign_second_chain_start_version_or_epochs() {
    let (expected, mut version_records) = two_module_chain();
    version_records.push(chain_started(expected.clone(), 1, 3, "9.9.9"));
    assert_eq!(
        resume_plan(&version_records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Mismatch
    );

    let (expected, mut epoch_records) = two_module_chain();
    epoch_records.push(chain_started(expected.clone(), 2, 4, "0.1.0"));
    assert_eq!(
        resume_plan(&epoch_records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_early_chain_finish_and_chain_finish_before_steps() {
    let (expected, mut records) = two_module_chain();
    records.push(JournalRecord::ChainFinished {
        to_epoch: 3,
        unix_ms: 1,
    });
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Mismatch
    );

    let (expected, mut records) = two_module_chain();
    records.push(step_finished("a"));
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_rejects_trailing_records_after_chain_finish() {
    let (expected, mut records) = two_module_chain();
    records.extend([
        step_started("a", 1, 2),
        step_finished("a"),
        step_started("b", 2, 3),
        step_finished("b"),
        JournalRecord::ChainFinished {
            to_epoch: 3,
            unix_ms: 1,
        },
        step_started("b", 2, 3),
    ]);
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Mismatch
    );
}

#[test]
fn resume_accepts_repeated_start_for_same_unfinished_step() {
    let (expected, mut records) = two_module_chain();
    records.extend([step_started("a", 1, 2), step_started("a", 1, 2)]);
    assert_eq!(
        resume_plan(&records, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 0
        }
    );
}

#[test]
fn resume_torn_suffix_uses_parsed_prefix_cursor() {
    let chain = chain_started(modules(), 1, 2, "0.1.0");
    let started = step_started("e0001_0002", 1, 2);
    let finished = step_finished("e0001_0002");
    let mut bytes = Vec::new();
    for record in [&chain, &started] {
        bytes.extend(serde_json::to_vec(record).expect("serialize complete record"));
        bytes.push(b'\n');
    }
    let torn_suffix = serde_json::to_vec(&finished).expect("serialize torn suffix");
    bytes.extend_from_slice(&torn_suffix[..torn_suffix.len() - 3]);

    let parsed =
        crate::format_stamp::migration::parse_records(&bytes).expect("discard torn final suffix");

    assert_eq!(
        resume_plan(&parsed, &modules(), 1, 2, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 0
        }
    );
}

#[test]
fn resume_crash_table_prefixes_resume_at_the_first_unfinished_module() {
    let (expected, chain) = two_module_chain();
    let after_first_finished = [
        chain.clone(),
        vec![step_started("a", 1, 2), step_finished("a")],
    ]
    .concat();
    assert_eq!(
        resume_plan(&after_first_finished, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 1
        }
    );

    let inside_first = [chain.clone(), vec![step_started("a", 1, 2)]].concat();
    assert_eq!(
        resume_plan(&inside_first, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 0
        }
    );

    let inside_last = [
        chain.clone(),
        vec![
            step_started("a", 1, 2),
            step_finished("a"),
            step_started("b", 2, 3),
        ],
    ]
    .concat();
    assert_eq!(
        resume_plan(&inside_last, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 1
        }
    );

    let all_finished_before_stamp = [
        chain.clone(),
        vec![
            step_started("a", 1, 2),
            step_finished("a"),
            step_started("b", 2, 3),
            step_finished("b"),
        ],
    ]
    .concat();
    assert_eq!(
        resume_plan(&all_finished_before_stamp, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 2
        }
    );

    let finished = [
        all_finished_before_stamp,
        vec![JournalRecord::ChainFinished {
            to_epoch: 3,
            unix_ms: 1,
        }],
    ]
    .concat();
    assert_eq!(
        resume_plan(&finished, &expected, 1, 3, "0.1.0"),
        ResumePlan::Resume {
            next_module_index: 2
        }
    );
}
