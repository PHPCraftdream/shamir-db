use std::path::Path;

use crate::format_stamp::model::EpochPolicy;

use super::error::MigrationError;
use super::journal::JournalWriter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleDescriptor {
    pub id: &'static str,
    pub from_epoch: u64,
    pub to_epoch: u64,
    pub summary: &'static str,
    pub touches: &'static [&'static str],
    pub work_estimate: &'static str,
}

pub trait FormatStep: Sync {
    fn descriptor(&self) -> &ModuleDescriptor;
    fn apply(&self, ctx: &mut StepContext<'_>) -> Result<(), MigrationError>;
    fn plan(&self, ctx: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError>;
}

pub struct StepContext<'a> {
    pub data_dir: &'a Path,
    pub journal: &'a JournalWriter,
    pub policy: EpochPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLine {
    pub description: String,
}
