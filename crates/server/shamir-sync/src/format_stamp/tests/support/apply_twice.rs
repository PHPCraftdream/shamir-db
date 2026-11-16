use std::path::Path;

use crate::format_stamp::migration::{FormatStep, JournalWriter, StepContext};
use crate::format_stamp::model::EpochPolicy;

use super::dir_snapshot::DirSnapshot;

pub fn apply_twice_is_noop(step: &dyn FormatStep, dir: &Path, policy: EpochPolicy) {
    let journal = JournalWriter::plan_handle(dir);
    let mut context = StepContext {
        data_dir: dir,
        journal: &journal,
        policy,
    };
    assert!(step.apply(&mut context).is_ok());
    let snapshot = DirSnapshot::take(dir);
    assert!(step.apply(&mut context).is_ok());
    snapshot.assert_unchanged(dir);
}
