mod acknowledgement;

#[cfg(test)]
pub(crate) use acknowledgement::apply_backpressure;
#[allow(unused_imports)] // Preserve the original crate-internal commit paths.
pub(crate) use acknowledgement::{
    acknowledge_known_outcome, apply_backpressure_with_budget, commit_tx_with_options,
};
