use crate::format_stamp::migration::{
    FormatStep, MigrationError, ModuleDescriptor, PlanLine, StepContext,
};

pub struct EmptyStep;

static DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0001_0002",
    from_epoch: 1,
    to_epoch: 2,
    summary: "empty test step",
    touches: &[],
    work_estimate: "none",
};

impl FormatStep for EmptyStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        &DESCRIPTOR
    }

    fn apply(&self, _ctx: &mut StepContext<'_>) -> Result<(), MigrationError> {
        Ok(())
    }

    fn plan(&self, _ctx: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        Ok(vec![PlanLine {
            description: "e0001_0002: empty test step".to_owned(),
        }])
    }
}
