#[allow(clippy::module_inception)]
mod group_commit;
pub use group_commit::GroupCommit;

#[cfg(test)]
mod tests;
