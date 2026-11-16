use crate::format_stamp::model::EpochPolicy;

use super::error::MigrationError;
use super::module::FormatStep;

pub const FORMAT_MODULES: &[&dyn FormatStep] = &[];

pub fn chain_for<'a>(
    policy: EpochPolicy,
    modules: &[&'a dyn FormatStep],
) -> Result<Vec<&'a dyn FormatStep>, MigrationError> {
    if policy.base == 0 || policy.base > policy.current {
        return Err(MigrationError::BrokenChain {
            from: policy.base,
            expected: policy.current,
            found: None,
        });
    }

    for (index, step) in modules.iter().enumerate() {
        let descriptor = step.descriptor();
        let expected = descriptor.from_epoch.saturating_add(1);
        for other in modules.iter().skip(index + 1) {
            let other_descriptor = other.descriptor();
            if descriptor.id == other_descriptor.id {
                return Err(MigrationError::BrokenChain {
                    from: descriptor.from_epoch,
                    expected,
                    found: Some(other_descriptor.from_epoch),
                });
            }
            if descriptor.from_epoch == other_descriptor.from_epoch {
                return Err(MigrationError::BrokenChain {
                    from: descriptor.from_epoch,
                    expected,
                    found: Some(other_descriptor.from_epoch),
                });
            }
        }
        if descriptor.from_epoch.checked_add(1) != Some(descriptor.to_epoch) {
            return Err(MigrationError::BrokenChain {
                from: descriptor.from_epoch,
                expected,
                found: Some(descriptor.to_epoch),
            });
        }
    }

    let mut chain = Vec::new();
    let mut epoch = policy.base;
    while epoch < policy.current {
        let Some(step) = modules
            .iter()
            .find(|step| step.descriptor().from_epoch == epoch)
        else {
            return Err(MigrationError::BrokenChain {
                from: epoch,
                expected: epoch + 1,
                found: None,
            });
        };
        chain.push(*step);
        epoch += 1;
    }
    Ok(chain)
}
