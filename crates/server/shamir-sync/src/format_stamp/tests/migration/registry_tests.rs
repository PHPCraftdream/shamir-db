use crate::format_stamp::migration::{
    chain_for, FormatStep, MigrationError, ModuleDescriptor, FORMAT_CHANGES, FORMAT_MODULES,
};
use crate::format_stamp::model::{EpochPolicy, BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH};

const _: () = assert!(BASE_FORMAT_EPOCH <= CURRENT_FORMAT_EPOCH);

struct TestStep {
    descriptor: ModuleDescriptor,
}

impl FormatStep for TestStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn apply(
        &self,
        _ctx: &mut crate::format_stamp::migration::StepContext<'_>,
    ) -> Result<(), MigrationError> {
        Ok(())
    }

    fn plan(
        &self,
        _ctx: &crate::format_stamp::migration::StepContext<'_>,
    ) -> Result<Vec<crate::format_stamp::migration::PlanLine>, MigrationError> {
        Ok(Vec::new())
    }
}

fn step(id: &'static str, from_epoch: u64, to_epoch: u64) -> TestStep {
    TestStep {
        descriptor: ModuleDescriptor {
            id,
            from_epoch,
            to_epoch,
            summary: "test step",
            touches: &[],
            work_estimate: "none",
        },
    }
}

fn steps_refs(steps: &[TestStep]) -> Vec<&dyn FormatStep> {
    steps.iter().map(|step| step as &dyn FormatStep).collect()
}

fn assert_broken_chain(result: Result<Vec<&dyn FormatStep>, MigrationError>) {
    assert!(matches!(result, Err(MigrationError::BrokenChain { .. })));
}

#[test]
fn registry_matches_epoch_change_log() {
    let bumps: Vec<_> = FORMAT_CHANGES
        .iter()
        .filter(|change| change.bumps_epoch)
        .collect();
    assert_eq!(CURRENT_FORMAT_EPOCH, BASE_FORMAT_EPOCH + bumps.len() as u64);
    assert_eq!(FORMAT_MODULES.len(), bumps.len());

    let mut change_ids: Vec<_> = FORMAT_CHANGES.iter().map(|change| change.id).collect();
    change_ids.sort_unstable();
    assert!(change_ids.windows(2).all(|pair| pair[0] != pair[1]));

    let mut module_ids = Vec::new();
    let mut from_epochs = Vec::new();
    for module in FORMAT_MODULES {
        let descriptor = module.descriptor();
        module_ids.push(descriptor.id);
        from_epochs.push(descriptor.from_epoch);
        let matches: Vec<_> = bumps
            .iter()
            .filter(|change| change.migration_module == Some(descriptor.id))
            .collect();
        assert_eq!(matches.len(), 1, "{} has one bump change", descriptor.id);
        assert_eq!(matches[0].epoch_after, descriptor.to_epoch);
    }
    module_ids.sort_unstable();
    assert!(module_ids.windows(2).all(|pair| pair[0] != pair[1]));
    from_epochs.sort_unstable();
    assert!(from_epochs.windows(2).all(|pair| pair[0] != pair[1]));

    let chain = chain_for(EpochPolicy::compiled(), FORMAT_MODULES).expect("compiled chain");
    assert_eq!(chain.len(), FORMAT_MODULES.len());
    for (offset, module) in chain.iter().enumerate() {
        assert_eq!(
            module.descriptor().from_epoch,
            BASE_FORMAT_EPOCH + offset as u64
        );
        assert_eq!(
            module.descriptor().to_epoch,
            module.descriptor().from_epoch + 1
        );
    }
}

#[test]
fn chain_for_accepts_complete_epoch_hops() {
    let steps = [step("e1_2", 1, 2), step("e2_3", 2, 3)];
    let refs = steps_refs(&steps);
    let chain = chain_for(EpochPolicy::new(3, 1), &refs).expect("consecutive chain");
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].descriptor().id, "e1_2");
    assert_eq!(chain[1].descriptor().id, "e2_3");
}

#[test]
fn chain_for_accepts_same_base_and_current_epoch() {
    let steps = [step("unused", 1, 2)];
    let refs = steps_refs(&steps);
    assert!(chain_for(EpochPolicy::new(1, 1), &refs)
        .expect("empty range does not need steps")
        .is_empty());
}

#[test]
fn chain_for_rejects_missing_epoch() {
    let steps = [step("e1_2", 1, 2), step("e3_4", 3, 4)];
    assert_broken_chain(chain_for(EpochPolicy::new(4, 1), &steps_refs(&steps)));
}

#[test]
fn chain_for_rejects_short_chain() {
    let steps = [step("e1_2", 1, 2)];
    assert_broken_chain(chain_for(EpochPolicy::new(3, 1), &steps_refs(&steps)));
}

#[test]
fn chain_for_rejects_duplicate_ids() {
    let steps = [step("same", 1, 2), step("same", 2, 3)];
    assert_broken_chain(chain_for(EpochPolicy::new(3, 1), &steps_refs(&steps)));
}

#[test]
fn chain_for_rejects_duplicate_from_epochs() {
    let steps = [step("first", 1, 2), step("second", 1, 2)];
    assert_broken_chain(chain_for(EpochPolicy::new(2, 1), &steps_refs(&steps)));
}

#[test]
fn chain_for_rejects_nonconsecutive_step() {
    let steps = [step("e1_3", 1, 3)];
    assert_broken_chain(chain_for(EpochPolicy::new(3, 1), &steps_refs(&steps)));
}

#[test]
fn chain_for_rejects_invalid_epoch_policy() {
    let steps = [];
    let refs = steps_refs(&steps);
    assert_broken_chain(chain_for(EpochPolicy::new(0, 0), &refs));
    assert_broken_chain(chain_for(EpochPolicy::new(1, 2), &refs));
}
