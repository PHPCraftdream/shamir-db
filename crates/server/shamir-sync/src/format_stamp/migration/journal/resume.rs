use super::record::JournalRecord;

/// Result of validating a journal against the expected format-migration chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlan {
    Fresh,
    Resume { next_module_index: usize },
    Mismatch,
}

/// Compute the resume cursor without I/O from parsed journal records.
pub fn resume_plan(
    records: &[JournalRecord],
    expected_modules: &[String],
    from_epoch: u64,
    to_epoch: u64,
    binary_version: &str,
) -> ResumePlan {
    let Some(epoch_count) = to_epoch.checked_sub(from_epoch) else {
        return ResumePlan::Mismatch;
    };
    let Ok(module_count) = u64::try_from(expected_modules.len()) else {
        return ResumePlan::Mismatch;
    };
    if epoch_count != module_count {
        return ResumePlan::Mismatch;
    }
    if records.is_empty() {
        return ResumePlan::Fresh;
    }

    let Some(JournalRecord::ChainStarted {
        from_epoch: recorded_from,
        to_epoch: recorded_to,
        binary_version: recorded_version,
        modules: recorded_modules,
        ..
    }) = records.first()
    else {
        return ResumePlan::Mismatch;
    };
    if *recorded_from != from_epoch
        || *recorded_to != to_epoch
        || recorded_version != binary_version
        || recorded_modules != expected_modules
    {
        return ResumePlan::Mismatch;
    }

    let mut position = 1;
    while let Some(JournalRecord::ChainStarted {
        from_epoch: recorded_from,
        to_epoch: recorded_to,
        binary_version: recorded_version,
        modules: recorded_modules,
        ..
    }) = records.get(position)
    {
        if *recorded_from != from_epoch
            || *recorded_to != to_epoch
            || recorded_version != binary_version
            || recorded_modules != expected_modules
        {
            return ResumePlan::Mismatch;
        }
        position += 1;
    }

    let mut cursor = 0usize;
    let mut active = false;
    let mut finished = false;
    for record in &records[position..] {
        if finished {
            return ResumePlan::Mismatch;
        }
        match record {
            JournalRecord::ChainStarted { .. } => return ResumePlan::Mismatch,
            JournalRecord::StepStarted {
                module,
                from_epoch: step_from,
                to_epoch: step_to,
                ..
            } => {
                let Some(expected_module) = expected_modules.get(cursor) else {
                    return ResumePlan::Mismatch;
                };
                let Ok(offset) = u64::try_from(cursor) else {
                    return ResumePlan::Mismatch;
                };
                let Some(expected_from) = from_epoch.checked_add(offset) else {
                    return ResumePlan::Mismatch;
                };
                let Some(expected_to) = expected_from.checked_add(1) else {
                    return ResumePlan::Mismatch;
                };
                if module != expected_module
                    || *step_from != expected_from
                    || *step_to != expected_to
                {
                    return ResumePlan::Mismatch;
                }
                active = true;
            }
            JournalRecord::StepFinished { module, .. } => {
                if !active || expected_modules.get(cursor) != Some(module) {
                    return ResumePlan::Mismatch;
                }
                let Some(next_cursor) = cursor.checked_add(1) else {
                    return ResumePlan::Mismatch;
                };
                cursor = next_cursor;
                active = false;
            }
            JournalRecord::ChainFinished {
                to_epoch: finished_to,
                ..
            } => {
                if active || cursor != expected_modules.len() || *finished_to != to_epoch {
                    return ResumePlan::Mismatch;
                }
                finished = true;
            }
        }
    }

    ResumePlan::Resume {
        next_module_index: cursor,
    }
}
