pub mod commit;

pub use commit::{commit_tx, TxError as CommitError, TxOutcome};

#[cfg(test)]
mod tests;
